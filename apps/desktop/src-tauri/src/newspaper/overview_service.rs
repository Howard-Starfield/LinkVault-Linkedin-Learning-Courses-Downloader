//! Cross-domain bootstrap and activity snapshot read models.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use super::{
    batch_service, catalog_service, job_repository,
    models::{
        NewspaperActivitySnapshot, NewspaperBootstrap, NewspaperJob, NewspaperJobProgress,
        OptimizationRuntimeStatus,
    },
    reader_service, schedule_service, storage,
};

struct RawOptimizationProgress {
    total: u32,
    completed: u32,
    failed: u32,
    pending: u32,
    recovered: u32,
    active_workers: u32,
    original_bytes: u64,
    optimized_bytes: u64,
    first_started_at: Option<i64>,
    last_completed_at: Option<i64>,
}

pub(super) fn bootstrap(db_path: &Path) -> Result<NewspaperBootstrap, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    storage::initialize(&connection).map_err(|error| error.to_string())?;
    Ok(NewspaperBootstrap {
        catalog: catalog_service::list_with_connection(&connection)?,
        batches: batch_service::list(&connection)?,
        jobs: job_repository::list(&connection, None)?,
        schedules: schedule_service::list(&connection)?,
        reading_progress: reader_service::list_progress(&connection)?,
        settings: load_settings(&connection)?,
    })
}

pub(super) fn activity(
    db_path: &Path,
    revision: u64,
    optimization_runtime: OptimizationRuntimeStatus,
) -> Result<NewspaperActivitySnapshot, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let jobs = job_repository::list(&connection, None)?;
    let progress = job_progress(&connection, &jobs)?;
    let has_live_activity = jobs.iter().any(|job| {
        matches!(job.status.as_str(), "queued" | "active" | "optimizing") || job.retry_at.is_some()
    }) || optimization_runtime.active;
    Ok(NewspaperActivitySnapshot {
        jobs,
        progress,
        batches: batch_service::list(&connection)?,
        schedules: schedule_service::list(&connection)?,
        has_live_activity,
        optimization_runtime,
        revision,
    })
}

fn job_progress(
    connection: &Connection,
    jobs: &[NewspaperJob],
) -> Result<Vec<NewspaperJobProgress>, String> {
    let mut statement = connection
        .prepare(
            "SELECT
                COUNT(t.page_id),
                COALESCE(SUM(CASE WHEN t.status IN ('succeeded', 'kept_original') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.status = 'failed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.status IN ('pending', 'running') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.recovered = 1 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.status = 'running' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(t.source_bytes), 0),
                COALESCE(SUM(t.output_bytes), 0),
                MIN(t.started_at),
                MAX(t.completed_at)
             FROM newspaper_optimization_tasks t
             WHERE t.job_id = ?1",
        )
        .map_err(|error| error.to_string())?;
    let mut progress = Vec::with_capacity(jobs.len());
    for job in jobs {
        let raw = statement
            .query_row([&job.id], |row| {
                Ok(RawOptimizationProgress {
                    total: row.get(0)?,
                    completed: row.get(1)?,
                    failed: row.get(2)?,
                    pending: row.get(3)?,
                    recovered: row.get(4)?,
                    active_workers: row.get(5)?,
                    original_bytes: row.get(6)?,
                    optimized_bytes: row.get(7)?,
                    first_started_at: row.get(8)?,
                    last_completed_at: row.get(9)?,
                })
            })
            .map_err(|error| error.to_string())?;
        let terminal = raw.completed.saturating_add(raw.failed);
        let elapsed_seconds = raw
            .first_started_at
            .zip(raw.last_completed_at)
            .map(|(start, end)| end.saturating_sub(start).max(1));
        let pages_per_minute = if terminal >= 3 {
            elapsed_seconds.map(|elapsed| f64::from(terminal) * 60.0 / elapsed as f64)
        } else {
            None
        };
        let eta_seconds = pages_per_minute
            .filter(|rate| *rate > 0.0 && raw.pending > 0)
            .map(|rate| (f64::from(raw.pending) * 60.0 / rate).ceil() as u64);
        let current_stage = current_stage(job, raw.total, raw.completed, raw.failed);
        progress.push(NewspaperJobProgress {
            job_id: job.id.clone(),
            current_stage,
            download_total: job.page_count,
            download_completed: job.completed_count,
            download_failed: job.failed_count,
            optimization_total: raw.total,
            optimization_completed: raw.completed,
            optimization_failed: raw.failed,
            optimization_pending: raw.pending,
            optimization_recovered: raw.recovered,
            active_workers: raw.active_workers,
            pages_per_minute,
            eta_seconds,
            original_bytes: raw.original_bytes,
            optimized_bytes: raw.optimized_bytes,
            bytes_saved: raw.original_bytes.saturating_sub(raw.optimized_bytes),
        });
    }
    Ok(progress)
}

fn current_stage(
    job: &NewspaperJob,
    optimization_total: u32,
    optimization_completed: u32,
    optimization_failed: u32,
) -> String {
    if job.paused {
        return "paused".to_string();
    }
    if job.status == "queued" {
        return "queued".to_string();
    }
    if job.status == "active"
        || job.completed_count.saturating_add(job.failed_count) < job.page_count
    {
        return "downloading".to_string();
    }
    if optimization_total > optimization_completed.saturating_add(optimization_failed) {
        return "optimizing".to_string();
    }
    if job.status == "optimizing" {
        return "finalizing".to_string();
    }
    match job.status.as_str() {
        "completed" => "complete",
        "partial" => "partial",
        "failed" => "failed",
        "unavailable" => "unavailable",
        "cancelled" => "cancelled",
        other => other,
    }
    .to_string()
}

pub(super) fn load_settings(connection: &Connection) -> Result<serde_json::Value, String> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value_json FROM newspaper_settings WHERE key = 'preferences'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(value
        .map(|json| serde_json::from_str(&json).unwrap_or_default())
        .unwrap_or_else(|| serde_json::json!({})))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::newspaper::optimization_tasks;

    #[test]
    fn activity_keeps_optimization_incomplete_after_download_reaches_total() {
        let directory = TempDir::new().unwrap();
        let db_path = directory.path().join("newspaper.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        storage::initialize(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, delay_seconds,
                  optimize_images, optimization_profile, optimization_quality,
                  keep_original_jpg, created_at, updated_at)
                 VALUES ('batch', 'active', '', 0, 0, 1, 'webp_balanced', 45, 1, 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_jobs
                 (id, batch_id, edition_code, edition_publication_date,
                  publication_date, status, output_dir, page_count,
                  completed_count, queue_position, created_at, updated_at)
                 VALUES ('job', 'batch', 'NY', '', '2026-07-26', 'optimizing',
                         '', 4, 4, 1, 1, 1)",
                [],
            )
            .unwrap();
        for index in 0..4 {
            let page_id = format!("page-{index}");
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                     (id, job_id, page_number, source_url, original_path, status,
                      original_bytes, final_bytes, created_at, updated_at)
                     VALUES (?1, 'job', ?2, '', ?3, 'completed', 100, 100, 1, 1)",
                    rusqlite::params![page_id, index.to_string(), format!("page-{index}.jpg")],
                )
                .unwrap();
        }
        optimization_tasks::ensure_for_job(&connection, "job", 1).unwrap();
        connection
            .execute(
                "UPDATE newspaper_optimization_tasks SET
                    status = CASE page_id
                        WHEN 'page-0' THEN 'succeeded'
                        WHEN 'page-1' THEN 'kept_original'
                        WHEN 'page-2' THEN 'failed'
                        ELSE 'pending'
                    END,
                    recovered = CASE WHEN page_id = 'page-1' THEN 1 ELSE 0 END,
                    source_bytes = 100,
                    output_bytes = CASE WHEN page_id = 'page-0' THEN 60
                                        WHEN page_id = 'page-1' THEN 100
                                        ELSE NULL END,
                    started_at = CASE WHEN page_id != 'page-3' THEN 10 END,
                    completed_at = CASE WHEN page_id != 'page-3' THEN 20 END",
                [],
            )
            .unwrap();
        drop(connection);

        let snapshot = activity(
            &db_path,
            7,
            OptimizationRuntimeStatus {
                active: true,
                ..OptimizationRuntimeStatus::default()
            },
        )
        .unwrap();
        let progress = &snapshot.progress[0];
        assert_eq!(progress.current_stage, "optimizing");
        assert_eq!(progress.download_completed, 4);
        assert_eq!(progress.optimization_total, 4);
        assert_eq!(progress.optimization_completed, 2);
        assert_eq!(progress.optimization_failed, 1);
        assert_eq!(progress.optimization_pending, 1);
        assert_eq!(progress.optimization_recovered, 1);
        assert!(progress.pages_per_minute.is_some());
        assert!(progress.eta_seconds.is_some());
        assert!(snapshot.has_live_activity);
        assert_eq!(snapshot.revision, 7);
    }
}
