//! Queue-level and whole-job newspaper image optimization workflows.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

use chrono::Utc;
use rusqlite::params;

use super::{
    job_repository,
    models::{NewspaperJob, OptimizationRunOptions, OptimizationRuntimeStatus},
    naming,
    optimization_tasks::{self, FailureDisposition},
    optimizer::{optimize_page, OptimizationError, OptimizationOutcome},
    resource_governor::ResourceGovernor,
    storage,
};

type RuntimeReporter = Arc<dyn Fn(OptimizationRuntimeStatus) + Send + Sync>;

fn eligible_job_ids(
    connection: &rusqlite::Connection,
    now: i64,
    job_id: Option<&str>,
    allow_queued: bool,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT j.id
         FROM newspaper_jobs j JOIN newspaper_batches b ON b.id = j.batch_id
         JOIN newspaper_pages p ON p.job_id = j.id
         LEFT JOIN newspaper_optimization_tasks t ON t.page_id = p.id
         WHERE (?4 = 1 OR j.status IN ('optimizing', 'completed', 'partial'))
           AND j.status != 'cancelled' AND j.paused = 0 AND j.dismissed = 0
           AND b.optimize_images = 1 AND b.status NOT IN ('paused', 'cancelled')
           AND p.status = 'completed' AND p.original_path IS NOT NULL
           AND p.optimized_path IS NULL AND (?3 IS NULL OR j.id = ?3)
           AND (t.page_id IS NULL OR (t.attempts < ?2 AND
                ((t.status = 'pending' AND COALESCE(t.retry_at, ?1) <= ?1)
                 OR (t.status = 'running' AND COALESCE(t.lease_expires_at, ?1) <= ?1))))
         ORDER BY j.created_at",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(
            params![now, optimization_tasks::MAX_ATTEMPTS, job_id, allow_queued],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

fn job_enabled(connection: &rusqlite::Connection, job_id: &str) -> Result<bool, String> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM newspaper_jobs j JOIN newspaper_batches b ON b.id = j.batch_id
         WHERE j.id = ?1 AND j.paused = 0 AND j.dismissed = 0 AND j.status != 'cancelled'
           AND b.optimize_images = 1 AND b.status NOT IN ('paused', 'cancelled'))",
        [job_id], |row| row.get(0),
    ).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(super) async fn process_queue(db_path: &Path) -> Result<Vec<NewspaperJob>, String> {
    process_queue_with_options(
        db_path,
        OptimizationRunOptions::default(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(|_| {}),
    )
    .await
}

pub(super) async fn process_queue_with_options(
    db_path: &Path,
    options: OptimizationRunOptions,
    cancelled: Arc<AtomicBool>,
    reporter: RuntimeReporter,
) -> Result<Vec<NewspaperJob>, String> {
    let db_path = db_path.to_path_buf();
    // Keep discovery, each job's workers, and finalization on one blocking
    // thread. SQLite waits must not occupy the async runtime between jobs.
    tauri::async_runtime::spawn_blocking(move || {
        process_queue_blocking(&db_path, options, cancelled, reporter)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn process_queue_blocking(
    db_path: &Path,
    options: OptimizationRunOptions,
    cancelled: Arc<AtomicBool>,
    reporter: RuntimeReporter,
) -> Result<Vec<NewspaperJob>, String> {
    let job_ids = {
        let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
        eligible_job_ids(&connection, Utc::now().timestamp(), None, false)?
    };
    let mut processed = Vec::new();
    for job_id in job_ids {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let job = {
            let connection =
                crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
            if eligible_job_ids(&connection, Utc::now().timestamp(), Some(&job_id), false)?
                .is_empty()
            {
                continue;
            }
            job_repository::find(&connection, &job_id)?
                .ok_or_else(|| format!("Newspaper job disappeared before optimization: {job_id}"))?
        };
        let changed = optimize_job_with_options(
            db_path,
            &job,
            options.clone(),
            cancelled.clone(),
            reporter.clone(),
        )?;
        let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
        if !changed || cancelled.load(Ordering::SeqCst) || !job_enabled(&connection, &job.id)? {
            continue;
        }
        storage::finalize_job(&connection, &job.id, Utc::now().timestamp())
            .map_err(|error| error.to_string())?;
        let refreshed = job_repository::find(&connection, &job.id)?
            .ok_or_else(|| format!("Newspaper job disappeared after optimization: {}", job.id))?;
        processed.push(refreshed);
    }
    Ok(processed)
}

pub(super) fn optimize_job(db_path: &Path, job: &NewspaperJob) -> Result<(), String> {
    optimize_job_with_options(
        db_path,
        job,
        OptimizationRunOptions::default(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(|_| {}),
    )
    .map(|_| ())
}

fn optimize_job_with_options(
    db_path: &Path,
    job: &NewspaperJob,
    options: OptimizationRunOptions,
    cancelled: Arc<AtomicBool>,
    reporter: RuntimeReporter,
) -> Result<bool, String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    if cancelled.load(Ordering::SeqCst)
        || eligible_job_ids(&connection, Utc::now().timestamp(), Some(&job.id), true)?.is_empty()
    {
        return Ok(false);
    }
    let unfinished_before: i64 = connection.query_row(
        "SELECT COUNT(*) FROM newspaper_pages WHERE job_id = ?1 AND status = 'completed' AND optimized_path IS NULL",
        [&job.id], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    let settings: (bool, u8, bool) = connection
        .query_row(
            "SELECT optimize_images, optimization_quality, keep_original_jpg
             FROM newspaper_batches WHERE id = ?1",
            params![job.batch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if !settings.0 {
        return Ok(false);
    }
    let started_at = Utc::now().timestamp();
    optimization_tasks::ensure_for_job(&connection, &job.id, started_at)
        .map_err(|error| error.to_string())?;
    optimization_tasks::reconcile(&connection, started_at).map_err(|error| error.to_string())?;
    if cancelled.load(Ordering::SeqCst) || !job_enabled(&connection, &job.id)? {
        return Ok(false);
    }
    let activated = connection
        .execute(
            "UPDATE newspaper_jobs SET status = 'optimizing', updated_at = ?2 WHERE id = ?1
             AND paused = 0 AND dismissed = 0 AND status != 'cancelled'
             AND EXISTS(SELECT 1 FROM newspaper_batches b WHERE b.id = newspaper_jobs.batch_id AND b.status NOT IN ('paused', 'cancelled'))",
            params![job.id, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    if activated == 0 {
        return Ok(false);
    }
    let lease_owner = naming::unique_id(&format!("optimizer-{}", std::process::id()));
    let mut warnings = Vec::new();
    let mut governor = ResourceGovernor::new(options);
    let (sender, receiver) = mpsc::channel::<WorkerResult>();
    let mut claimed_any = false;
    std::thread::scope(|scope| -> Result<(), String> {
        let mut active_workers = 0_usize;
        let mut queue_exhausted = false;
        loop {
            governor.refresh();
            while !queue_exhausted
                && !cancelled.load(Ordering::SeqCst)
                && active_workers < governor.admitted_workers()
            {
                let now = Utc::now().timestamp();
                if !job_enabled(&connection, &job.id)? {
                    queue_exhausted = true;
                    break;
                }
                let Some(task) =
                    optimization_tasks::claim_next(&mut connection, &job.id, &lease_owner, now)
                        .map_err(|error| error.to_string())?
                else {
                    queue_exhausted = true;
                    break;
                };
                claimed_any = true;
                let worker_sender = sender.clone();
                let quality = settings.1;
                scope.spawn(move || {
                    let page_started = Instant::now();
                    let source_path = task.source_path.clone();
                    let outcome = optimize_page(&source_path, quality);
                    let _ = worker_sender.send(WorkerResult {
                        task,
                        outcome,
                        elapsed_ms: elapsed_ms(page_started),
                    });
                });
                active_workers += 1;
            }
            reporter(governor.runtime_status(active_workers));
            if active_workers == 0 {
                break;
            }
            let result = receiver
                .recv()
                .map_err(|error| format!("Optimization worker channel closed: {error}"))?;
            active_workers -= 1;
            match result.outcome {
                Ok(OptimizationOutcome::Replaced { path, bytes }) => {
                    let now = Utc::now().timestamp();
                    optimization_tasks::complete_replaced(
                        &mut connection,
                        &result.task,
                        &path,
                        bytes,
                        result.elapsed_ms,
                        now,
                    )
                    .map_err(|error| error.to_string())?;
                    if let Some(warning) = optimization_tasks::cleanup_completed_source(
                        &connection,
                        &result.task,
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
                        &result.task,
                        bytes,
                        result.elapsed_ms,
                        Utc::now().timestamp(),
                    )
                    .map_err(|error| error.to_string())?;
                }
                Err(error) => {
                    let (error_kind, retryable) = classify_error(&error);
                    let message = error.to_string();
                    let disposition = optimization_tasks::complete_failure(
                        &mut connection,
                        &result.task,
                        &message,
                        error_kind,
                        retryable,
                        result.elapsed_ms,
                        Utc::now().timestamp(),
                    )
                    .map_err(|sql_error| sql_error.to_string())?;
                    if disposition == FailureDisposition::Failed {
                        warnings.push(format!("{}: {message}", result.task.source_path.display()));
                    }
                }
            }
            reporter(governor.runtime_status(active_workers));
        }
        Ok(())
    })?;
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
    let unfinished_after: i64 = connection.query_row(
        "SELECT COUNT(*) FROM newspaper_pages WHERE job_id = ?1 AND status = 'completed' AND optimized_path IS NULL",
        [&job.id], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    Ok(claimed_any || unfinished_after < unfinished_before)
}

struct WorkerResult {
    task: optimization_tasks::ClaimedTask,
    outcome: Result<OptimizationOutcome, OptimizationError>,
    elapsed_ms: u64,
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

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::atomic::AtomicUsize};

    use image::{ImageBuffer, Rgb};
    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn optimization_discovery_keeps_async_timers_live_during_sqlite_contention() {
        let fixture = swarm_fixture(1);
        let lock_path = fixture.db_path.clone();
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let lock_thread = std::thread::spawn(move || {
            let connection = Connection::open(lock_path).unwrap();
            connection.execute_batch("BEGIN EXCLUSIVE").unwrap();
            locked_tx.send(()).unwrap();
            // A broken synchronous discovery blocks the only async thread,
            // preventing its timer from releasing the lock until this timeout.
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(1));
            connection.execute_batch("ROLLBACK").unwrap();
        });
        locked_rx.recv().unwrap();
        let db_path = fixture.db_path.clone();
        let processing = tokio::spawn(async move {
            process_queue_with_options(
                &db_path,
                OptimizationRunOptions::default(),
                Arc::new(AtomicBool::new(false)),
                Arc::new(|_| {}),
            )
            .await
        });
        let started = Instant::now();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let timer_elapsed = started.elapsed();
        let _ = release_tx.send(());
        lock_thread.join().unwrap();
        processing.await.unwrap().unwrap();
        assert!(
            timer_elapsed < std::time::Duration::from_millis(500),
            "async timer blocked for {timer_elapsed:?}"
        );
    }

    struct SwarmFixture {
        _directory: TempDir,
        db_path: PathBuf,
        job: NewspaperJob,
    }

    fn swarm_fixture(page_count: u32) -> SwarmFixture {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("newspaper.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        storage::initialize(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, delay_seconds,
                  optimize_images, optimization_profile, optimization_quality,
                  keep_original_jpg, created_at, updated_at)
                 VALUES ('batch', 'active', ?1, 0, 0, 1, 'webp_balanced', 25, 1, 1, 1)",
                [directory.path().to_string_lossy().as_ref()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_jobs
                 (id, batch_id, edition_code, edition_publication_date,
                  publication_date, status, output_dir, page_count,
                  completed_count, queue_position, created_at, updated_at)
                 VALUES ('job', 'batch', 'NY', '', '2026-07-26', 'optimizing',
                         ?1, ?2, ?2, 1, 1, 1)",
                params![directory.path().to_string_lossy(), page_count],
            )
            .unwrap();
        for index in 0..page_count {
            let source_path = directory.path().join(format!("page-{index:04}.jpg"));
            let image = ImageBuffer::from_fn(64, 64, |x, y| {
                Rgb([
                    ((x + index) % 255) as u8,
                    ((y * 3 + index) % 255) as u8,
                    ((x + y * 2 + index) % 255) as u8,
                ])
            });
            image.save(&source_path).unwrap();
            let bytes = std::fs::metadata(&source_path).unwrap().len();
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                     (id, job_id, page_number, source_url, original_path, status,
                      original_bytes, final_bytes, created_at, updated_at)
                     VALUES (?1, 'job', ?2, '', ?3, 'completed', ?4, ?4, 1, 1)",
                    params![
                        format!("page-{index:04}"),
                        format!("{index:04}"),
                        source_path.to_string_lossy(),
                        bytes,
                    ],
                )
                .unwrap();
        }
        let job = job_repository::find(&connection, "job").unwrap().unwrap();
        drop(connection);
        SwarmFixture {
            _directory: directory,
            db_path,
            job,
        }
    }

    #[test]
    fn future_optimization_retry_produces_no_job_pass_until_its_deadline() {
        let fixture = swarm_fixture(1);
        let connection = Connection::open(&fixture.db_path).unwrap();
        let now = Utc::now().timestamp();
        optimization_tasks::ensure_for_job(&connection, "job", now).unwrap();
        connection
            .execute(
                "UPDATE newspaper_optimization_tasks SET retry_at = ?1",
                [now + 3600],
            )
            .unwrap();
        assert!(eligible_job_ids(&connection, now, None, false)
            .unwrap()
            .is_empty());
        assert_eq!(
            eligible_job_ids(&connection, now + 3600, None, false).unwrap(),
            vec!["job"]
        );
        let reports = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&reports);
        let processed = process_queue_blocking(
            &fixture.db_path,
            OptimizationRunOptions::default(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
        assert!(processed.is_empty());
        assert_eq!(
            reports.load(Ordering::SeqCst),
            0,
            "A deferred retry must not launch an empty worker pass"
        );
        let state: (String, i64) = connection
            .query_row(
                "SELECT status, updated_at FROM newspaper_jobs WHERE id = 'job'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, ("optimizing".to_string(), 1));
        connection
            .execute(
                "UPDATE newspaper_optimization_tasks SET retry_at = ?1",
                [now - 1],
            )
            .unwrap();
        let processed = process_queue_blocking(
            &fixture.db_path,
            OptimizationRunOptions::default(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {}),
        )
        .unwrap();
        assert_eq!(processed.len(), 1);
        assert!(process_queue_blocking(
            &fixture.db_path,
            OptimizationRunOptions::default(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {})
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn automatic_optimization_skips_paused_job_alongside_eligible_work() {
        let fixture = swarm_fixture(1);
        let connection = Connection::open(&fixture.db_path).unwrap();
        connection.execute(
            "INSERT INTO newspaper_jobs(id, batch_id, edition_code, edition_publication_date,
                publication_date, status, output_dir, page_count, completed_count, paused, created_at, updated_at)
             SELECT 'paused-job', batch_id, edition_code, edition_publication_date,
                publication_date, status, output_dir || '/paused-fixture', page_count, completed_count, 1, 0, 0 FROM newspaper_jobs WHERE id = 'job'", [],
        ).unwrap();
        connection.execute(
            "INSERT INTO newspaper_pages(id, job_id, page_number, source_url, original_path, status,
                 original_bytes, final_bytes, created_at, updated_at)
             SELECT 'paused-page', 'paused-job', page_number, source_url, original_path, status,
                 original_bytes, final_bytes, 0, 0 FROM newspaper_pages WHERE job_id = 'job'", [],
        ).unwrap();
        let processed = process_queue_blocking(
            &fixture.db_path,
            OptimizationRunOptions::default(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {}),
        )
        .unwrap();
        assert_eq!(processed.len(), 1);
        assert_eq!(processed[0].id, "job");
        let untouched: (Option<String>, i64) = connection.query_row("SELECT p.optimized_path, j.updated_at FROM newspaper_pages p JOIN newspaper_jobs j ON j.id = p.job_id WHERE p.id = 'paused-page'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(untouched, (None, 0));
    }

    #[test]
    fn manual_optimization_and_atomic_claim_honor_pause_dismiss_and_cancellation() {
        for condition in [
            "job_pause",
            "dismiss",
            "job_cancel",
            "batch_pause",
            "batch_cancel",
        ] {
            let fixture = swarm_fixture(1);
            let mut connection = Connection::open(&fixture.db_path).unwrap();
            optimization_tasks::ensure_for_job(&connection, "job", 1).unwrap();
            let sql = match condition {
                "job_pause" => "UPDATE newspaper_jobs SET paused = 1",
                "dismiss" => "UPDATE newspaper_jobs SET dismissed = 1",
                "job_cancel" => "UPDATE newspaper_jobs SET status = 'cancelled'",
                "batch_pause" => "UPDATE newspaper_batches SET status = 'paused'",
                _ => "UPDATE newspaper_batches SET status = 'cancelled'",
            };
            connection.execute(sql, []).unwrap();
            optimize_job(&fixture.db_path, &fixture.job).unwrap();
            assert!(
                optimization_tasks::claim_next(
                    &mut connection,
                    "job",
                    "fixture-worker",
                    Utc::now().timestamp()
                )
                .unwrap()
                .is_none(),
                "{condition} must gate the atomic claim"
            );
            let attempts: i64 = connection
                .query_row(
                    "SELECT attempts FROM newspaper_optimization_tasks",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                attempts, 0,
                "{condition} must retain recoverable pages without claiming"
            );
        }
    }

    #[test]
    fn optimization_discovery_respects_live_lease_and_attempt_ceiling() {
        let fixture = swarm_fixture(1);
        let connection = Connection::open(&fixture.db_path).unwrap();
        let now = Utc::now().timestamp();
        optimization_tasks::ensure_for_job(&connection, "job", now).unwrap();
        connection
            .execute(
                "UPDATE newspaper_optimization_tasks SET status = 'running', lease_expires_at = ?1",
                [now + 3600],
            )
            .unwrap();
        assert!(eligible_job_ids(&connection, now, None, false)
            .unwrap()
            .is_empty());
        assert_eq!(
            eligible_job_ids(&connection, now + 3600, None, false).unwrap(),
            vec!["job"]
        );
        connection
            .execute(
                "UPDATE newspaper_optimization_tasks SET attempts = ?1",
                [optimization_tasks::MAX_ATTEMPTS],
            )
            .unwrap();
        assert!(eligible_job_ids(&connection, now + 3600, None, false)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn five_hundred_page_swarm_finishes_once_with_bounded_concurrency() {
        let fixture = swarm_fixture(500);
        let max_active = Arc::new(AtomicUsize::new(0));
        let observed = max_active.clone();
        optimize_job_with_options(
            &fixture.db_path,
            &fixture.job,
            OptimizationRunOptions {
                mode: "manual".to_string(),
                worker_ceiling: 20,
                ..OptimizationRunOptions::default()
            },
            Arc::new(AtomicBool::new(false)),
            Arc::new(move |runtime| {
                observed.fetch_max(usize::from(runtime.active_workers), Ordering::SeqCst);
                assert!(runtime.admitted_workers <= 20);
            }),
        )
        .unwrap();

        let connection = Connection::open(&fixture.db_path).unwrap();
        let terminal: u32 = connection
            .query_row(
                "SELECT COUNT(*) FROM newspaper_optimization_tasks
                 WHERE status IN ('succeeded', 'kept_original')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let duplicates: u32 = connection
            .query_row(
                "SELECT COUNT(*) FROM newspaper_optimization_tasks WHERE attempts != 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(terminal, 500);
        assert_eq!(duplicates, 0);
        assert!(max_active.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn cancellation_stops_new_claims_and_leaves_unfinished_pages_recoverable() {
        let fixture = swarm_fixture(40);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation_signal = cancelled.clone();
        optimize_job_with_options(
            &fixture.db_path,
            &fixture.job,
            OptimizationRunOptions {
                mode: "manual".to_string(),
                worker_ceiling: 8,
                ..OptimizationRunOptions::default()
            },
            cancelled,
            Arc::new(move |runtime| {
                if runtime.active_workers > 0 {
                    cancellation_signal.store(true, Ordering::SeqCst);
                }
            }),
        )
        .unwrap();

        let connection = Connection::open(&fixture.db_path).unwrap();
        let (terminal, pending): (u32, u32) = connection
            .query_row(
                "SELECT
                    SUM(CASE WHEN status IN ('succeeded', 'kept_original') THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END)
                 FROM newspaper_optimization_tasks",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(terminal > 0);
        assert!(pending > 0);
        assert_eq!(terminal + pending, 40);
    }
}
