//! Queue-level and whole-job newspaper image optimization workflows.

use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection};

use super::{
    job_repository,
    models::NewspaperJob,
    optimizer::{optimize_page, OptimizationOutcome},
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
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
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
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = 'optimizing', updated_at = ?2 WHERE id = ?1",
            params![job.id, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    let pages = {
        let mut statement = connection
            .prepare(
                "SELECT id, original_path FROM newspaper_pages
                 WHERE job_id = ?1 AND status = 'completed'
                   AND original_path IS NOT NULL AND optimized_path IS NULL",
            )
            .map_err(|error| error.to_string())?;
        let result = statement
            .query_map(params![job.id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        result
    };
    let mut replacements = Vec::new();
    let mut warnings = Vec::new();
    for (page_id, source) in pages {
        match optimize_page(Path::new(&source), settings.1) {
            Ok(OptimizationOutcome::Replaced { path, bytes }) => {
                connection
                    .execute(
                        "UPDATE newspaper_pages SET optimized_path = ?2, final_bytes = ?3,
                         media_version = media_version + 1, updated_at = ?4 WHERE id = ?1",
                        params![
                            page_id,
                            path.to_string_lossy(),
                            bytes,
                            Utc::now().timestamp()
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                replacements.push(source);
            }
            Ok(OptimizationOutcome::KeptOriginal { bytes }) => {
                connection
                    .execute(
                        "UPDATE newspaper_pages
                         SET optimized_path = original_path, final_bytes = ?2, updated_at = ?3
                         WHERE id = ?1",
                        params![page_id, bytes, Utc::now().timestamp()],
                    )
                    .map_err(|error| error.to_string())?;
            }
            Err(error) => {
                connection
                    .execute(
                        "UPDATE newspaper_pages SET optimized_path = original_path, updated_at = ?2
                         WHERE id = ?1",
                        params![page_id, Utc::now().timestamp()],
                    )
                    .map_err(|sql_error| sql_error.to_string())?;
                warnings.push(error.to_string());
            }
        }
    }
    if !settings.2 {
        for source in replacements {
            if let Err(error) = std::fs::remove_file(&source) {
                warnings.push(format!("Could not remove original {}: {error}", source));
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
