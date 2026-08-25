//! Download queue selection, orchestration, retries, and lifecycle transitions.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use chrono::{Local, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use tauri::Manager;
use url::Url;

use super::{
    batch_service,
    client::{FetchError, NewspaperClient},
    commands::run_optimization_pass,
    downloader::{
        download_validated_page, validate_existing_page, DownloadedPage, PageDownloadError,
    },
    job_repository, manifest,
    models::{NewspaperJob, OptimizationRunOptions},
    naming,
    state::NewspaperState,
    storage,
};

/// Jobs whose terminal download status carries at least one optimizable
/// page. The optimization queue's own SQL filter is the source of truth;
/// this list is the cheap pre-check used by the per-edition trigger so we
/// don't spawn a no-op task for jobs that the queue would skip anyway.
const OPTIMIZATION_ELIGIBLE_STATUSES: &[&str] = &["completed", "partial"];

async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

pub(super) async fn process_queue(
    db_path: &Path,
    cancelled: &Arc<AtomicBool>,
    app: &tauri::AppHandle,
) -> Result<Vec<NewspaperJob>, String> {
    let client = NewspaperClient::new().map_err(|error| error.to_string())?;
    let mut processed = Vec::new();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let next = {
            let db_path = db_path.to_path_buf();
            run_blocking(move || {
                let connection =
                    crate::cache::open_runtime(&db_path).map_err(|error| error.to_string())?;
                next_due_job(&connection)
            })
            .await?
        };
        let Some((job, delay_seconds, scheduled_at)) = next else {
            let db_path = db_path.to_path_buf();
            run_blocking(move || {
                let connection =
                    crate::cache::open_runtime(&db_path).map_err(|error| error.to_string())?;
                mark_retry_waiting_batches_scheduled(&connection)
            })
            .await?;
            break;
        };
        if scheduled_at.is_some_and(|scheduled| scheduled < Utc::now().timestamp()) {
            let db_path = db_path.to_path_buf();
            let batch_id = job.batch_id.clone();
            let job_id = job.id.clone();
            run_blocking(move || {
                let connection =
                    crate::cache::open_runtime(&db_path).map_err(|error| error.to_string())?;
                connection
                    .execute(
                        "INSERT INTO newspaper_events
                     (batch_id, job_id, event_type, message, created_at)
                     VALUES (?1, ?2, 'overdue_start',
                             'Scheduled batch started after its requested time.', ?3)",
                        params![batch_id, job_id, Utc::now().timestamp()],
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .await?;
        }
        let outcome = process_job(db_path, &client, job.clone(), cancelled).await?;
        let outcome_status = outcome.status.clone();
        processed.push(outcome);
        if OPTIMIZATION_ELIGIBLE_STATUSES.contains(&outcome_status.as_str()) {
            // The eligibility check is cheap and avoids spawning a task for
            // jobs the optimization queue would skip anyway. The actual
            // "is optimization already running" guard lives inside
            // `run_optimization_pass` via the `optimization_running` flag,
            // so a manual "Optimize now" overlapping with this trigger
            // resolves to whichever caller arrived second becoming a no-op.
            spawn_per_edition_optimization(app.clone(), outcome_status);
        }
        let has_next_due_job = {
            let db_path = db_path.to_path_buf();
            run_blocking(move || {
                let connection =
                    crate::cache::open_runtime(&db_path).map_err(|error| error.to_string())?;
                Ok(next_due_job(&connection)?.is_some())
            })
            .await?
        };
        if has_next_due_job && delay_seconds > 0 && !cancelled.load(Ordering::SeqCst) {
            let mut remaining = u64::from(delay_seconds);
            while remaining > 0 && !cancelled.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                remaining -= 1;
            }
        }
    }
    Ok(processed)
}

/// Fires the optimization queue as soon as a download job reaches a
/// terminal status. Returns immediately if a previous optimization pass is
/// still in flight (the `optimization_running` flag inside
/// `run_optimization_pass` is the source of truth, so a manual "Optimize
/// now" or a sibling per-edition trigger racing with this one resolves to a
/// no-op rather than overlapping workers).
fn spawn_per_edition_optimization(app: tauri::AppHandle, job_status: String) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<NewspaperState>();
        let result =
            run_optimization_pass(&app, state.inner(), OptimizationRunOptions::default()).await;
        if let Err(error) = result {
            eprintln!("per-edition optimization trigger failed after {job_status} job: {error}");
        }
    });
}

pub(crate) async fn process_job(
    db_path: &Path,
    client: &NewspaperClient,
    mut job: NewspaperJob,
    cancelled: &Arc<AtomicBool>,
) -> Result<NewspaperJob, String> {
    let now = Utc::now().timestamp();
    {
        let db_path = db_path.to_path_buf();
        let job_id = job.id.clone();
        let batch_id = job.batch_id.clone();
        run_blocking(move || activate_queued_job(&db_path, &job_id, &batch_id, now)).await?;
    }
    job.status = "active".to_string();
    let manifest = match client
        .fetch_manifest(&job.edition_code, &job.publication_date, cancelled)
        .await
    {
        Ok(value) => value,
        Err(FetchError::Unavailable) => {
            let db_path = db_path.to_path_buf();
            let retry_release = publication_is_today_or_future(&job.publication_date);
            return run_blocking(move || {
                if retry_release {
                    schedule_release_retry(
                        &db_path,
                        &mut job,
                        "Edition has not been released yet.",
                    )?;
                } else {
                    update_job_terminal(&db_path, &job.id, "unavailable", None)?;
                    job.status = "unavailable".to_string();
                }
                Ok(job)
            })
            .await;
        }
        Err(FetchError::Cancelled) => {
            let db_path = db_path.to_path_buf();
            return run_blocking(move || {
                job.status = apply_interrupted_job_state(&db_path, &job)?.to_string();
                Ok(job)
            })
            .await;
        }
        Err(error) => {
            let db_path = db_path.to_path_buf();
            let retry_release = publication_is_today_or_future(&job.publication_date)
                && matches!(
                    &error,
                    FetchError::Manifest(
                        manifest::ManifestError::InvalidContentType
                            | manifest::ManifestError::HtmlBody
                            | manifest::ManifestError::Empty
                    )
                );
            let error_message = error.to_string();
            return run_blocking(move || {
                if retry_release {
                    schedule_release_retry(
                        &db_path,
                        &mut job,
                        "Edition has not been released yet.",
                    )?;
                } else {
                    update_job_terminal(&db_path, &job.id, "failed", Some(&error_message))?;
                    job.status = "failed".to_string();
                }
                Ok(job)
            })
            .await;
        }
    };

    let origin = client.origin().clone();
    let referer = origin
        .join(&format!("/{}/{}", job.edition_code, job.publication_date))
        .map_err(|error| error.to_string())?;
    let pages: Vec<_> = manifest.pages().cloned().collect();
    {
        let db_path = db_path.to_path_buf();
        let job = job.clone();
        let pages = pages.clone();
        let origin = origin.clone();
        run_blocking(move || upsert_job_pages(&db_path, &job, &origin, &pages)).await?;
    }

    for (page_index, page) in pages.into_iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let source_url = manifest::resolve_page_url_with_origin(&page.pagefile, &origin)
            .map_err(|error| error.to_string())?;
        let extension = manifest::page_file_extension(&source_url);
        let destination = PathBuf::from(&job.output_dir).join(format!(
            "{}.{}",
            naming::sanitize_segment(&page.pageno),
            extension
        ));
        let existing = if destination.exists() {
            validate_existing_page(&destination).await.ok()
        } else {
            None
        };
        let result = match existing {
            Some(value) => Ok(value),
            None => {
                download_validated_page(
                    client,
                    source_url,
                    referer.as_str(),
                    &destination,
                    cancelled,
                )
                .await
            }
        };
        let db_path = db_path.to_path_buf();
        let cancelled_now = cancelled.load(Ordering::SeqCst);
        match run_blocking(move || {
            persist_page_download(
                &db_path,
                job,
                &page,
                page_index,
                &destination,
                result,
                cancelled_now,
            )
        })
        .await?
        {
            PersistPageOutcome::Continue(updated) => job = updated,
            PersistPageOutcome::Stop(updated) => return Ok(updated),
        }
    }
    if cancelled.load(Ordering::SeqCst) {
        let db_path = db_path.to_path_buf();
        return run_blocking(move || {
            job.status = apply_interrupted_job_state(&db_path, &job)?.to_string();
            Ok(job)
        })
        .await;
    }
    let db_path = db_path.to_path_buf();
    run_blocking(move || finalize_processed_job(&db_path, job)).await
}

enum PersistPageOutcome {
    Continue(NewspaperJob),
    Stop(NewspaperJob),
}

fn activate_queued_job(
    db_path: &Path,
    job_id: &str,
    batch_id: &str,
    now: i64,
) -> Result<(), String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'active', retry_at = NULL, warning = NULL, updated_at = ?2
             WHERE id = ?1",
            params![job_id, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_batches SET status = 'active', scheduled_at = NULL, updated_at = ?2 WHERE id = ?1",
            params![batch_id, now],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn upsert_job_pages(
    db_path: &Path,
    job: &NewspaperJob,
    origin: &Url,
    pages: &[manifest::Page],
) -> Result<(), String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE newspaper_jobs SET page_count = ?2, updated_at = ?3 WHERE id = ?1",
            params![job.id, pages.len() as i64, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    for page in pages {
        let source_url = manifest::resolve_page_url_with_origin(&page.pagefile, origin)
            .map_err(|error| error.to_string())?;
        let extension = manifest::page_file_extension(&source_url);
        let destination = Path::new(&job.output_dir).join(format!(
            "{}.{}",
            naming::sanitize_segment(&page.pageno),
            extension
        ));
        transaction
            .execute(
                "INSERT INTO newspaper_pages
                (id, job_id, page_number, section_name, source_url, original_path,
                 status, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)
                ON CONFLICT(job_id, page_number) DO UPDATE SET
                    section_name = excluded.section_name,
                    source_url = excluded.source_url,
                    original_path = excluded.original_path,
                    updated_at = excluded.updated_at",
                params![
                    naming::unique_id("newspaper-page"),
                    job.id,
                    page.pageno,
                    page.name,
                    source_url.as_str(),
                    destination.to_string_lossy(),
                    Utc::now().timestamp(),
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}

fn persist_page_download(
    db_path: &Path,
    mut job: NewspaperJob,
    page: &manifest::Page,
    page_index: usize,
    destination: &Path,
    result: Result<DownloadedPage, PageDownloadError>,
    cancelled: bool,
) -> Result<PersistPageOutcome, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    if matches!(&result, Err(PageDownloadError::NotReleased)) {
        drop(connection);
        schedule_release_retry(db_path, &mut job, "Edition has not been released yet.")?;
        return Ok(PersistPageOutcome::Stop(job));
    }
    match result {
        Ok(downloaded) => {
            if page_index == 0
                && first_page_matches_another_date(
                    &connection,
                    &job,
                    &page.pageno,
                    &downloaded.checksum_sha256,
                )?
            {
                let _ = std::fs::remove_file(destination);
                drop(connection);
                schedule_release_retry(
                    db_path,
                    &mut job,
                    "Edition is still showing an earlier newspaper; retry scheduled.",
                )?;
                return Ok(PersistPageOutcome::Stop(job));
            }
            connection
                .execute(
                    "UPDATE newspaper_pages SET status = 'completed', attempts = attempts + 1,
                     original_bytes = ?3, final_bytes = ?3, checksum = ?4,
                     pixel_width = ?5, pixel_height = ?6,
                     media_version = media_version + 1,
                     error = NULL, updated_at = ?7
                     WHERE job_id = ?1 AND page_number = ?2",
                    params![
                        job.id,
                        page.pageno,
                        downloaded.size_bytes,
                        downloaded.checksum_sha256,
                        downloaded.width,
                        downloaded.height,
                        Utc::now().timestamp(),
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        Err(error) => {
            if cancelled {
                drop(connection);
                job.status = apply_interrupted_job_state(db_path, &job)?.to_string();
                return Ok(PersistPageOutcome::Stop(job));
            }
            connection
                .execute(
                    "UPDATE newspaper_pages SET status = 'failed', attempts = attempts + 1,
                     error = ?3, updated_at = ?4 WHERE job_id = ?1 AND page_number = ?2",
                    params![
                        job.id,
                        page.pageno,
                        error.to_string(),
                        Utc::now().timestamp()
                    ],
                )
                .map_err(|sql_error| sql_error.to_string())?;
        }
    }
    refresh_job_progress(&connection, &job.id)?;
    Ok(PersistPageOutcome::Continue(job))
}

fn finalize_processed_job(db_path: &Path, mut job: NewspaperJob) -> Result<NewspaperJob, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let completion = storage::finalize_job(&connection, &job.id, Utc::now().timestamp())
        .map_err(|error| error.to_string())?;
    job.status = completion.status;
    job.page_count = completion.page_count as u32;
    job.completed_count = completion.completed_count as u32;
    job.failed_count = completion.failed_count as u32;
    batch_service::finish_if_terminal(&connection, &job.batch_id)?;
    Ok(job)
}

pub(super) fn next_due_job(
    connection: &Connection,
) -> Result<Option<(NewspaperJob, u32, Option<i64>)>, String> {
    let now = Utc::now().timestamp();
    connection
        .query_row(
            "SELECT j.id, j.batch_id, j.edition_code, e.name_zh, j.publication_date,
                    j.status, j.output_dir, j.page_count, j.completed_count,
                    j.failed_count, j.retry_at, j.retry_count, j.warning,
                    j.queue_position, j.paused, j.dismissed, j.created_at,
                    j.updated_at, j.completed_at,
                    b.delay_seconds,
                    b.scheduled_at
             FROM newspaper_jobs j
             JOIN newspaper_batches b ON b.id = j.batch_id
             JOIN newspaper_editions e ON e.code = j.edition_code
                 AND e.publication_date = j.edition_publication_date
             WHERE j.status = 'queued' AND j.paused = 0 AND j.dismissed = 0
               AND b.status IN ('queued', 'scheduled', 'active')
               AND (b.scheduled_at IS NULL OR b.scheduled_at <= ?1)
               AND (j.retry_at IS NULL OR j.retry_at <= ?1)
             ORDER BY j.queue_position, j.created_at LIMIT 1",
            params![now],
            |row| Ok((job_repository::row_to_job(row)?, row.get(19)?, row.get(20)?)),
        )
        .optional()
        .map_err(|error| error.to_string())
}

pub(super) fn update_job_terminal(
    db_path: &Path,
    job_id: &str,
    status: &str,
    warning: Option<&str>,
) -> Result<(), String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = ?2, warning = ?3, updated_at = ?4 WHERE id = ?1",
            params![job_id, status, warning, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn apply_interrupted_job_state(
    db_path: &Path,
    job: &NewspaperJob,
) -> Result<&'static str, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let (paused, dismissed, current_status, batch_status): (bool, bool, String, String) =
        connection
            .query_row(
                "SELECT j.paused, j.dismissed, j.status, b.status
                 FROM newspaper_jobs j
                 JOIN newspaper_batches b ON b.id = j.batch_id
                 WHERE j.id = ?1",
                params![job.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|error| error.to_string())?;
    let (page_status, job_status) = if paused || batch_status == "paused" {
        ("pending", "queued")
    } else if dismissed || current_status == "cancelled" || batch_status == "cancelled" {
        ("cancelled", "cancelled")
    } else {
        ("pending", "queued")
    };
    let now = Utc::now().timestamp();
    connection
        .execute(
            "UPDATE newspaper_pages
             SET status = ?2, error = NULL, updated_at = ?3
             WHERE job_id = ?1 AND status IN ('pending', 'downloading', 'optimizing')",
            params![job.id, page_status, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![job.id, job_status, now],
        )
        .map_err(|error| error.to_string())?;
    Ok(job_status)
}

pub(super) fn publication_is_today_or_future(publication_date: &str) -> bool {
    NaiveDate::parse_from_str(publication_date, "%Y-%m-%d")
        .map(|date| date >= Local::now().date_naive())
        .unwrap_or(false)
}

pub(super) fn schedule_release_retry(
    db_path: &Path,
    job: &mut NewspaperJob,
    reason: &str,
) -> Result<(), String> {
    const RETRY_SECONDS: i64 = 30 * 60;
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let now = Utc::now().timestamp();
    let retry_at = now + RETRY_SECONDS;
    let warning = format!("{reason} Retrying automatically in 30 minutes.");
    connection
        .execute(
            "UPDATE newspaper_pages
             SET status = 'pending', error = NULL, updated_at = ?2
             WHERE job_id = ?1 AND status IN ('failed', 'cancelled')",
            params![job.id, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'queued', retry_at = ?2, retry_count = retry_count + 1,
                 failed_count = 0, warning = ?3, completed_at = NULL, updated_at = ?4
             WHERE id = ?1",
            params![job.id, retry_at, warning, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_batches
             SET status = 'scheduled', scheduled_at = NULL,
                 completed_at = NULL, updated_at = ?2
             WHERE id = ?1",
            params![job.batch_id, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO newspaper_events
             (batch_id, job_id, event_type, message, payload_json, created_at)
             VALUES (?1, ?2, 'release_retry_scheduled', ?3, ?4, ?5)",
            params![
                job.batch_id,
                job.id,
                warning,
                serde_json::json!({ "retryAt": retry_at }).to_string(),
                now
            ],
        )
        .map_err(|error| error.to_string())?;
    job.status = "awaiting_release".to_string();
    job.retry_at = Some(retry_at);
    job.retry_count = job.retry_count.saturating_add(1);
    job.failed_count = 0;
    job.warning = Some(warning);
    job.updated_at = now;
    Ok(())
}

pub(super) fn first_page_matches_another_date(
    connection: &Connection,
    job: &NewspaperJob,
    page_number: &str,
    checksum: &str,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM newspaper_pages p
                 JOIN newspaper_jobs j ON j.id = p.job_id
                 WHERE j.edition_code = ?1
                   AND j.publication_date <> ?2
                   AND p.page_number = ?3
                   AND p.checksum = ?4
                   AND p.status = 'completed'
             )",
            params![
                job.edition_code,
                job.publication_date,
                page_number,
                checksum
            ],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

pub(super) fn refresh_job_progress(connection: &Connection, job_id: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET completed_count = (
                     SELECT COUNT(*) FROM newspaper_pages
                     WHERE job_id = ?1 AND status = 'completed'
                 ),
                 failed_count = (
                     SELECT COUNT(*) FROM newspaper_pages
                     WHERE job_id = ?1 AND status IN ('failed', 'cancelled')
                 ),
                 original_bytes = COALESCE((
                     SELECT SUM(original_bytes) FROM newspaper_pages WHERE job_id = ?1
                 ), 0),
                 final_bytes = COALESCE((
                     SELECT SUM(final_bytes) FROM newspaper_pages WHERE job_id = ?1
                 ), 0),
                 updated_at = ?2
             WHERE id = ?1",
            params![job_id, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn mark_retry_waiting_batches_scheduled(connection: &Connection) -> Result<(), String> {
    let now = Utc::now().timestamp();
    connection
        .execute(
            "UPDATE newspaper_batches
             SET status = 'scheduled', updated_at = ?1
             WHERE status = 'active'
               AND EXISTS (
                   SELECT 1 FROM newspaper_jobs j
                   WHERE j.batch_id = newspaper_batches.id
                     AND j.status = 'queued' AND j.retry_at > ?1
               )
               AND NOT EXISTS (
                   SELECT 1 FROM newspaper_jobs j
                   WHERE j.batch_id = newspaper_batches.id
                     AND j.status = 'queued'
                     AND (j.retry_at IS NULL OR j.retry_at <= ?1)
               )",
            params![now],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::OPTIMIZATION_ELIGIBLE_STATUSES;

    #[test]
    fn process_job_does_not_open_sqlite_on_the_async_executor() {
        let source = include_str!("queue_service.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        let process_job = production
            .split("pub(crate) async fn process_job")
            .nth(1)
            .and_then(|rest| rest.split("enum PersistPageOutcome").next())
            .unwrap_or_default();
        assert!(
            process_job.contains("run_blocking"),
            "process_job SQLite must run on the blocking pool"
        );
        assert!(
            !process_job.contains("crate::cache::open_runtime"),
            "process_job must not open rusqlite on the async executor; call helpers from run_blocking"
        );
        for helper in [
            "activate_queued_job",
            "schedule_release_retry",
            "update_job_terminal",
            "apply_interrupted_job_state",
            "upsert_job_pages",
            "persist_page_download",
            "finalize_processed_job",
        ] {
            assert!(
                process_job.contains(helper),
                "process_job should delegate {helper} through run_blocking"
            );
        }
    }

    #[test]
    fn per_edition_trigger_fires_only_for_terminal_download_statuses() {
        // The per-edition trigger fires for `completed` and `partial`
        // jobs only. `queued`, `active`, `failed`, and `optimizing`
        // statuses must not enqueue a redundant optimization pass.
        for status in OPTIMIZATION_ELIGIBLE_STATUSES {
            assert!(
                matches!(*status, "completed" | "partial"),
                "unexpected eligible status: {status}"
            );
        }
        assert!(!OPTIMIZATION_ELIGIBLE_STATUSES.contains(&"queued"));
        assert!(!OPTIMIZATION_ELIGIBLE_STATUSES.contains(&"active"));
        assert!(!OPTIMIZATION_ELIGIBLE_STATUSES.contains(&"failed"));
        assert!(!OPTIMIZATION_ELIGIBLE_STATUSES.contains(&"optimizing"));
    }
}
