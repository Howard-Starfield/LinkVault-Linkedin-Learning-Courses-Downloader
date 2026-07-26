//! Queue-level and whole-job newspaper image optimization workflows.

use std::{path::Path, time::Instant};

use chrono::Utc;
use rusqlite::{params, Connection};

use super::{
    job_repository,
    models::NewspaperJob,
    naming,
    optimization_tasks::{self, FailureDisposition},
    optimizer::{optimize_page, OptimizationError, OptimizationOutcome},
    storage,
};

pub(super) async fn process_queue(db_path: &Path) -> Result<Vec<NewspaperJob>, String> {
    let job_ids = {
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT j.id
                 FROM newspaper_jobs j
                 JOIN newspaper_batches b ON b.id = j.batch_id
                 JOIN newspaper_pages p ON p.job_id = j.id
                 WHERE j.status IN ('optimizing', 'completed', 'partial')
                   AND b.optimize_images = 1
                   AND p.status = 'completed'
                   AND p.original_path IS NOT NULL
                   AND p.optimized_path IS NULL
                 ORDER BY j.created_at",
            )
            .map_err(|error| error.to_string())?;
        let result = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        result
    };
    let mut processed = Vec::new();
    for job_id in job_ids {
        let job = {
            let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            job_repository::list(&connection, None)?
                .into_iter()
                .find(|item| item.id == job_id)
                .ok_or_else(|| format!("Newspaper job disappeared before optimization: {job_id}"))?
        };
        let optimization_db_path = db_path.to_path_buf();
        let optimization_job = job.clone();
        tauri::async_runtime::spawn_blocking(move || {
            optimize_job(&optimization_db_path, &optimization_job)
        })
        .await
        .map_err(|error| error.to_string())??;
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        storage::finalize_job(&connection, &job.id, Utc::now().timestamp())
            .map_err(|error| error.to_string())?;
        let refreshed = job_repository::list(&connection, None)?
            .into_iter()
            .find(|item| item.id == job.id)
            .ok_or_else(|| format!("Newspaper job disappeared after optimization: {}", job.id))?;
        processed.push(refreshed);
    }
    Ok(processed)
}

pub(super) fn optimize_job(db_path: &Path, job: &NewspaperJob) -> Result<(), String> {
    let mut connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let settings: (bool, u8, bool) = connection
        .query_row(
            "SELECT optimize_images, optimization_quality, keep_original_jpg
             FROM newspaper_batches WHERE id = ?1",
            params![job.batch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if !settings.0 {
        return Ok(());
    }
    let started_at = Utc::now().timestamp();
    optimization_tasks::ensure_for_job(&connection, &job.id, started_at)
        .map_err(|error| error.to_string())?;
    optimization_tasks::reconcile(&connection, started_at).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = 'optimizing', updated_at = ?2 WHERE id = ?1",
            params![job.id, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    let lease_owner = naming::unique_id(&format!("optimizer-{}", std::process::id()));
    let mut warnings = Vec::new();
    loop {
        let now = Utc::now().timestamp();
        let Some(task) =
            optimization_tasks::claim_next(&mut connection, &job.id, &lease_owner, now)
                .map_err(|error| error.to_string())?
        else {
            break;
        };
        let page_started = Instant::now();
        match optimize_page(&task.source_path, settings.1) {
            Ok(OptimizationOutcome::Replaced { path, bytes }) => {
                let now = Utc::now().timestamp();
                optimization_tasks::complete_replaced(
                    &mut connection,
                    &task,
                    &path,
                    bytes,
                    elapsed_ms(page_started),
                    now,
                )
                .map_err(|error| error.to_string())?;
                if let Some(warning) = optimization_tasks::cleanup_completed_source(
                    &connection,
                    &task,
                    &path,
                    settings.2,
                    now,
                )
                .map_err(|error| error.to_string())?
                {
                    warnings.push(warning);
                }
            }
            Ok(OptimizationOutcome::KeptOriginal { bytes }) => {
                optimization_tasks::complete_kept_original(
                    &mut connection,
                    &task,
                    bytes,
                    elapsed_ms(page_started),
                    Utc::now().timestamp(),
                )
                .map_err(|error| error.to_string())?;
            }
            Err(error) => {
                let (error_kind, retryable) = classify_error(&error);
                let message = error.to_string();
                let disposition = optimization_tasks::complete_failure(
                    &mut connection,
                    &task,
                    &message,
                    error_kind,
                    retryable,
                    elapsed_ms(page_started),
                    Utc::now().timestamp(),
                )
                .map_err(|sql_error| sql_error.to_string())?;
                let suffix = if disposition == FailureDisposition::RetryScheduled {
                    format!(
                        " Retrying attempt {} of {}.",
                        task.attempts,
                        optimization_tasks::MAX_ATTEMPTS
                    )
                } else {
                    String::new()
                };
                warnings.push(format!("{}: {message}{suffix}", task.source_path.display()));
            }
        }
    }
    if !warnings.is_empty() {
        connection
            .execute(
                "UPDATE newspaper_jobs SET warning = ?2, updated_at = ?3 WHERE id = ?1",
                params![job.id, warnings.join("; "), Utc::now().timestamp()],
            )
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute(
            "UPDATE newspaper_jobs SET
                original_bytes = COALESCE((SELECT SUM(original_bytes) FROM newspaper_pages WHERE job_id = ?1), 0),
                final_bytes = COALESCE((SELECT SUM(final_bytes) FROM newspaper_pages WHERE job_id = ?1), 0),
                updated_at = ?2
             WHERE id = ?1",
            params![job.id, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn elapsed_ms(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn classify_error(error: &OptimizationError) -> (&'static str, bool) {
    match error {
        OptimizationError::Io(_) => ("io", true),
        OptimizationError::Encoder => ("encoder", true),
        OptimizationError::Image(_) => ("invalid_image", false),
        OptimizationError::DimensionMismatch => ("dimension_mismatch", false),
        OptimizationError::UnsupportedQuality(_) => ("unsupported_quality", false),
    }
}
