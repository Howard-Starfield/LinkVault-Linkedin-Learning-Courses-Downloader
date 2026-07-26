use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use chrono::{Local, NaiveDate, NaiveTime, Utc};
use image::GenericImageView;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager, State};

use super::{
    client::{FetchError, NewspaperClient},
    downloader::{download_validated_page, validate_existing_page, PageDownloadError},
    manifest,
    models::{
        expand_dates, CreateNewspaperBatchRequest, CreateNewspaperBatchResponse,
        CreateNewspaperScheduleRequest, DateMode, EditionKind, NewspaperActivitySnapshot,
        NewspaperBatch, NewspaperBootstrap, NewspaperEdition, NewspaperJob, NewspaperLibraryItem,
        NewspaperLibraryPage, NewspaperPage, NewspaperReadingProgress, NewspaperSchedule,
        PublicationSchedule, RepairNewspaperLibraryResult,
    },
    optimizer::{optimize_page, OptimizationOutcome},
    page_metadata, reader_service, storage,
    thumbnails::{EnsureThumbnailResult, ThumbnailCoordinator},
};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct NewspaperState {
    db_path: PathBuf,
    cancelled: Arc<AtomicBool>,
    running: AtomicBool,
    library_revision: AtomicU64,
    dimension_backfill_running: Arc<AtomicBool>,
}

impl NewspaperState {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            cancelled: Arc::new(AtomicBool::new(false)),
            running: AtomicBool::new(false),
            library_revision: AtomicU64::new(1),
            dimension_backfill_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn library_revision(&self) -> u64 {
        self.library_revision.load(Ordering::SeqCst)
    }

    pub fn invalidate_library(&self) -> u64 {
        self.library_revision.fetch_add(1, Ordering::SeqCst) + 1
    }
}

pub fn schedule_page_dimension_backfill(app: &tauri::AppHandle) {
    let state = app.state::<NewspaperState>();
    page_metadata::schedule(
        state.db_path.clone(),
        state.dimension_backfill_running.clone(),
    );
}

#[tauri::command]
pub fn bootstrap_newspaper_state(
    state: State<'_, NewspaperState>,
) -> Result<NewspaperBootstrap, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    storage::initialize(&connection).map_err(|error| error.to_string())?;
    Ok(NewspaperBootstrap {
        catalog: list_catalog_records(&connection)?,
        batches: list_batches(&connection)?,
        jobs: list_jobs(&connection, None)?,
        schedules: list_schedules(&connection)?,
        reading_progress: reader_service::list_progress(&connection)?,
        settings: load_settings(&connection)?,
    })
}

#[tauri::command]
pub fn list_newspaper_catalog(
    state: State<'_, NewspaperState>,
) -> Result<Vec<NewspaperEdition>, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    list_catalog_records(&connection)
}

#[tauri::command]
pub async fn refresh_newspaper_catalog(
    state: State<'_, NewspaperState>,
) -> Result<Vec<NewspaperEdition>, String> {
    let html = reqwest::Client::new()
        .get("https://ep.worldjournal.com/")
        .header(
            reqwest::header::USER_AGENT,
            super::client::CHROME_USER_AGENT,
        )
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .text()
        .await
        .map_err(|error| error.to_string())?;
    let discovered = super::catalog::discover_specials(&html);
    let mut connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    for edition in discovered {
        let publication_date = edition
            .publication_date
            .map(|value| value.to_string())
            .unwrap_or_default();
        transaction
            .execute(
                "INSERT INTO newspaper_editions
                (code, publication_date, name_zh, name_en, kind, schedule, source_url,
                 active, discovered, discovered_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, 'special', 'ad_hoc', ?5, 1, 1, ?6, ?6)
                ON CONFLICT(code, publication_date) DO UPDATE SET
                    name_zh = excluded.name_zh, source_url = excluded.source_url,
                    active = 1, discovered = 1, discovered_at = excluded.discovered_at,
                    updated_at = excluded.updated_at",
                params![
                    edition.code,
                    publication_date,
                    edition.name_zh,
                    edition.name_en,
                    edition.source_url,
                    Utc::now().timestamp()
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    list_catalog_records(&connection)
}

#[tauri::command]
pub fn create_newspaper_batch(
    state: State<'_, NewspaperState>,
    request: CreateNewspaperBatchRequest,
) -> Result<CreateNewspaperBatchResponse, String> {
    let mut connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    create_batch(&mut connection, request)
}

fn create_batch(
    connection: &mut Connection,
    request: CreateNewspaperBatchRequest,
) -> Result<CreateNewspaperBatchResponse, String> {
    validate_request(&request)?;
    let start = if request.date_mode == DateMode::Last7Days {
        Local::now().date_naive()
    } else {
        parse_date(&request.start_date)?
    };
    let end = request.end_date.as_deref().map(parse_date).transpose()?;
    let dates = expand_dates(request.date_mode, start, end).map_err(|error| error.to_string())?;
    let catalog = list_catalog_records(connection)?;
    let selected: Vec<NewspaperEdition> = catalog
        .into_iter()
        .filter(|edition| {
            request
                .edition_codes
                .iter()
                .any(|code| code == &edition_key(edition))
        })
        .collect();
    if selected.is_empty() {
        return Err("Select at least one supported newspaper edition.".to_string());
    }

    let now = Utc::now().timestamp();
    let batch_id = unique_id("newspaper-batch");
    let scheduled = request.scheduled_at.filter(|timestamp| *timestamp > now);
    let batch_status = if scheduled.is_some() {
        "scheduled"
    } else {
        "queued"
    };
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let mut next_queue_position = transaction
        .query_row(
            "SELECT COALESCE(MAX(queue_position), 0) + 1 FROM newspaper_jobs",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO newspaper_batches
            (id, status, destination, scheduled_at, delay_minutes, delay_seconds, optimize_images,
             optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                batch_id,
                batch_status,
                request.destination,
                scheduled,
                request.delay_seconds,
                request.optimize_images,
                request.optimization_profile,
                request.optimization_quality,
                request.keep_original_jpg,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;

    let mut created = Vec::new();
    let mut skipped_count = 0_u32;
    for edition in selected {
        let valid_dates: Vec<NaiveDate> = if edition.kind == EditionKind::Special {
            edition.publication_date.into_iter().collect()
        } else {
            dates
                .iter()
                .copied()
                .filter(|date| edition.schedule.accepts(*date))
                .collect()
        };
        for date in valid_dates {
            let date_string = date.format("%Y-%m-%d").to_string();
            let job_id = unique_id("newspaper-job");
            let output_dir = Path::new(&request.destination)
                .join(sanitize_segment(&format!(
                    "{} - {}",
                    edition.name_zh, edition.code
                )))
                .join(&date_string);
            transaction
                .execute(
                    "INSERT INTO newspaper_jobs
                    (id, batch_id, edition_code, edition_publication_date, publication_date,
                     status, output_dir, queue_position, created_at, updated_at)
                    VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?8, ?8)
                    ON CONFLICT(edition_code, publication_date, output_dir) DO NOTHING",
                    params![
                        job_id,
                        batch_id,
                        edition.code,
                        edition
                            .publication_date
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                        date_string,
                        output_dir.to_string_lossy(),
                        next_queue_position,
                        now,
                    ],
                )
                .map_err(|error| error.to_string())?;
            if transaction.changes() == 0 {
                skipped_count = skipped_count.saturating_add(1);
                let existing: Option<(String, String, String)> = transaction
                    .query_row(
                        "SELECT id, batch_id, status FROM newspaper_jobs
                         WHERE edition_code = ?1 AND publication_date = ?2 AND output_dir = ?3",
                        params![edition.code, date_string, output_dir.to_string_lossy()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(|error| error.to_string())?;
                if let Some((existing_job_id, existing_batch_id, existing_status)) = existing {
                    let marker_missing = !output_dir.join(".complete").is_file();
                    if marker_missing
                        && matches!(
                            existing_status.as_str(),
                            "completed" | "failed" | "unavailable" | "partial" | "cancelled"
                        )
                    {
                        transaction
                            .execute(
                                "UPDATE newspaper_pages
                                 SET status = 'pending', error = NULL, updated_at = ?2
                                 WHERE job_id = ?1 AND status IN ('failed', 'cancelled')",
                                params![existing_job_id, now],
                            )
                            .map_err(|error| error.to_string())?;
                        transaction
                            .execute(
                                "UPDATE newspaper_jobs
                                 SET status = 'queued', retry_at = NULL, failed_count = 0,
                                     warning = NULL, paused = 0, dismissed = 0,
                                     queue_position = ?3, completed_at = NULL, updated_at = ?2
                                 WHERE id = ?1",
                                params![existing_job_id, now, next_queue_position],
                            )
                            .map_err(|error| error.to_string())?;
                        next_queue_position += 1;
                        transaction
                            .execute(
                                "UPDATE newspaper_batches
                                 SET status = 'queued', scheduled_at = NULL, completed_at = NULL,
                                     updated_at = ?2 WHERE id = ?1",
                                params![existing_batch_id, now],
                            )
                            .map_err(|error| error.to_string())?;
                    }
                }
                continue;
            }
            created.push(NewspaperJob {
                id: job_id,
                batch_id: batch_id.clone(),
                edition_code: edition.code.clone(),
                edition_name: edition.name_zh.clone(),
                publication_date: date_string,
                status: "queued".to_string(),
                output_dir: output_dir.to_string_lossy().into_owned(),
                page_count: 0,
                completed_count: 0,
                failed_count: 0,
                retry_at: None,
                retry_count: 0,
                warning: None,
                queue_position: next_queue_position,
                paused: false,
                dismissed: false,
                created_at: now,
                updated_at: now,
                completed_at: None,
            });
            next_queue_position += 1;
        }
    }
    if created.is_empty() && skipped_count == 0 {
        return Err("The selected editions do not publish on the chosen dates.".to_string());
    }
    if created.is_empty() {
        transaction
            .execute(
                "UPDATE newspaper_batches
                 SET status = 'completed', completed_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params![batch_id, now],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let batch = NewspaperBatch {
        id: batch_id,
        status: if created.is_empty() {
            "completed".to_string()
        } else {
            batch_status.to_string()
        },
        destination: request.destination,
        scheduled_at: scheduled,
        delay_seconds: request.delay_seconds,
        optimize_images: request.optimize_images,
        optimization_profile: request.optimization_profile,
        optimization_quality: request.optimization_quality,
        keep_original_jpg: request.keep_original_jpg,
        created_at: now,
        updated_at: now,
    };
    Ok(CreateNewspaperBatchResponse {
        batch,
        jobs: created,
        skipped_count,
    })
}

#[tauri::command]
pub fn create_newspaper_schedule(
    state: State<'_, NewspaperState>,
    request: CreateNewspaperScheduleRequest,
) -> Result<NewspaperSchedule, String> {
    validate_schedule_request(&request)?;
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let catalog = list_catalog_records(&connection)?;
    if !request.edition_codes.iter().any(|selected| {
        catalog
            .iter()
            .any(|edition| edition_key(edition) == *selected)
    }) {
        return Err("Select at least one supported newspaper edition.".to_string());
    }
    let now = Utc::now().timestamp();
    let schedule = NewspaperSchedule {
        id: unique_id("newspaper-schedule"),
        enabled: true,
        cron_time: request.cron_time,
        destination: request.destination,
        edition_codes: request.edition_codes,
        delay_seconds: request.delay_seconds,
        optimize_images: request.optimize_images,
        optimization_profile: request.optimization_profile,
        optimization_quality: request.optimization_quality,
        keep_original_jpg: request.keep_original_jpg,
        last_run_date: None,
        last_error: None,
        created_at: now,
        updated_at: now,
    };
    connection
        .execute(
            "INSERT INTO newspaper_schedules
             (id, enabled, cron_time, destination, edition_codes_json, delay_seconds,
              optimize_images, optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                schedule.id,
                schedule.cron_time,
                schedule.destination,
                serde_json::to_string(&schedule.edition_codes).map_err(|error| error.to_string())?,
                schedule.delay_seconds,
                schedule.optimize_images,
                schedule.optimization_profile,
                schedule.optimization_quality,
                schedule.keep_original_jpg,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(schedule)
}

#[tauri::command]
pub fn toggle_newspaper_schedule(
    state: State<'_, NewspaperState>,
    schedule_id: String,
    enabled: bool,
) -> Result<(), String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_schedules
             SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![schedule_id, enabled, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_newspaper_schedule(
    state: State<'_, NewspaperState>,
    schedule_id: String,
) -> Result<(), String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "DELETE FROM newspaper_schedules WHERE id = ?1",
            params![schedule_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn process_newspaper_queue(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
) -> Result<Vec<NewspaperJob>, String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    state.cancelled.store(false, Ordering::SeqCst);
    let result = match materialize_due_schedules(&state.db_path) {
        Ok(()) => process_queue(&state.db_path, &state.cancelled).await,
        Err(error) => Err(error),
    };
    state.running.store(false, Ordering::SeqCst);
    if let Ok(jobs) = &result {
        emit_library_invalidation(&app, &state, jobs);
    }
    result
}

#[tauri::command]
pub async fn process_newspaper_optimization_queue(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
) -> Result<Vec<NewspaperJob>, String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    let result = process_optimization_queue(&state.db_path).await;
    state.running.store(false, Ordering::SeqCst);
    if let Ok(jobs) = &result {
        emit_library_invalidation(&app, &state, jobs);
    }
    result
}

fn emit_library_invalidation(
    app: &tauri::AppHandle,
    state: &NewspaperState,
    jobs: &[NewspaperJob],
) {
    if jobs.is_empty() {
        return;
    }
    let revision = state.invalidate_library();
    let job_ids = jobs.iter().map(|job| job.id.clone()).collect::<Vec<_>>();
    let _ = app.emit(
        "newspaper://library-invalidated",
        serde_json::json!({ "revision": revision, "jobIds": job_ids.clone() }),
    );
    schedule_thumbnail_generation(app, job_ids);
}

fn schedule_thumbnail_generation(app: &tauri::AppHandle, job_ids: Vec<String>) {
    for job_id in job_ids.into_iter().take(14) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let coordinator = app.state::<ThumbnailCoordinator>();
            let _ = coordinator.ensure(job_id).await;
        });
    }
}

#[tauri::command]
pub fn pause_newspaper_batch(
    state: State<'_, NewspaperState>,
    batch_id: String,
    paused: bool,
) -> Result<(), String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_batches SET status = ?2, updated_at = ?3
             WHERE id = ?1 AND status IN ('queued', 'scheduled', 'active', 'paused')",
            params![
                batch_id,
                if paused { "paused" } else { "queued" },
                Utc::now().timestamp()
            ],
        )
        .map_err(|error| error.to_string())?;
    if paused {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_newspaper_batch(
    state: State<'_, NewspaperState>,
    batch_id: String,
) -> Result<(), String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let now = Utc::now().timestamp();
    connection
        .execute(
            "UPDATE newspaper_batches SET status = 'cancelled', updated_at = ?2 WHERE id = ?1",
            params![batch_id, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = 'cancelled', updated_at = ?2
             WHERE batch_id = ?1 AND status IN ('queued', 'active', 'optimizing')",
            params![batch_id, now],
        )
        .map_err(|error| error.to_string())?;
    state.cancelled.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn retry_newspaper_job(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<usize, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    storage::retry_missing_pages(&connection, &job_id, Utc::now().timestamp())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_newspaper_job_pause(
    state: State<'_, NewspaperState>,
    job_id: String,
    paused: bool,
) -> Result<(), String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let status = set_job_pause(&connection, &job_id, paused, Utc::now().timestamp())?;
    if paused && status == "active" {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    Ok(())
}

fn set_job_pause(
    connection: &Connection,
    job_id: &str,
    paused: bool,
    updated_at: i64,
) -> Result<String, String> {
    let status: String = connection
        .query_row(
            "SELECT status FROM newspaper_jobs WHERE id = ?1 AND dismissed = 0",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|_| "Newspaper queue item was not found.".to_string())?;
    if matches!(
        status.as_str(),
        "completed" | "partial" | "failed" | "unavailable" | "cancelled"
    ) {
        return Err("Only queued or active downloads can be paused.".to_string());
    }
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET paused = ?2,
                 status = CASE WHEN ?2 = 1 AND status = 'active' THEN 'queued' ELSE status END,
                 updated_at = ?3
             WHERE id = ?1",
            params![job_id, paused, updated_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(status)
}

#[tauri::command]
pub fn reorder_newspaper_jobs(
    state: State<'_, NewspaperState>,
    job_ids: Vec<String>,
) -> Result<(), String> {
    let mut connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    reorder_jobs(&mut connection, &job_ids, Utc::now().timestamp())
}

fn reorder_jobs(
    connection: &mut Connection,
    job_ids: &[String],
    updated_at: i64,
) -> Result<(), String> {
    if job_ids.is_empty() {
        return Ok(());
    }
    let unique: HashSet<&String> = job_ids.iter().collect();
    if unique.len() != job_ids.len() {
        return Err("Queue order contains duplicate items.".to_string());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    for (index, job_id) in job_ids.iter().enumerate() {
        let changed = transaction
            .execute(
                "UPDATE newspaper_jobs
                 SET queue_position = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'queued' AND dismissed = 0",
                params![job_id, index as i64 + 1, updated_at],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("Only visible queued downloads can be reordered.".to_string());
        }
    }
    transaction.commit().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_newspaper_job(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<(), String> {
    let mut connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let now = Utc::now().timestamp();
    let (batch_id, status) = dismiss_job(&mut connection, &job_id, now)?;
    if matches!(status.as_str(), "active" | "optimizing") {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    finish_batch_if_terminal(&connection, &batch_id)
}

fn dismiss_job(
    connection: &mut Connection,
    job_id: &str,
    updated_at: i64,
) -> Result<(String, String), String> {
    let (batch_id, status): (String, String) = connection
        .query_row(
            "SELECT batch_id, status FROM newspaper_jobs WHERE id = ?1",
            params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "Newspaper queue item was not found.".to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE newspaper_jobs
             SET dismissed = 1, paused = 0,
                 status = CASE
                     WHEN status IN ('queued', 'active', 'optimizing') THEN 'cancelled'
                     ELSE status
                 END,
                 updated_at = ?2
             WHERE id = ?1",
            params![job_id, updated_at],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE newspaper_pages
             SET status = 'cancelled', updated_at = ?2
             WHERE job_id = ?1 AND status IN ('pending', 'downloading', 'optimizing')",
            params![job_id, updated_at],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO newspaper_events
             (batch_id, job_id, event_type, message, created_at)
             VALUES (?1, ?2, 'queue.dismissed',
                     'Removed from progress. Downloaded files were left on disk.', ?3)",
            params![batch_id, job_id, updated_at],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok((batch_id, status))
}

#[tauri::command]
pub fn list_newspaper_library(
    state: State<'_, NewspaperState>,
    query: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<NewspaperJob>, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let bounded_limit = limit.clamp(1, 50);
    let pattern = format!("%{}%", query.unwrap_or_default());
    let mut statement = connection
        .prepare(
            "SELECT j.id, j.batch_id, j.edition_code, e.name_zh, j.publication_date,
                    j.status, j.output_dir, j.page_count, j.completed_count,
                    j.failed_count, j.retry_at, j.retry_count, j.warning,
                    j.queue_position, j.paused, j.dismissed, j.created_at,
                    j.updated_at, j.completed_at
             FROM newspaper_jobs j
             JOIN newspaper_editions e ON e.code = j.edition_code
                 AND e.publication_date = j.edition_publication_date
             WHERE j.status IN ('completed', 'partial') AND j.dismissed = 0
               AND (?1 = '%%' OR e.name_zh LIKE ?1 OR e.name_en LIKE ?1 OR j.edition_code LIKE ?1)
             ORDER BY j.publication_date DESC, j.updated_at DESC
             LIMIT ?2 OFFSET ?3",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params![pattern, bounded_limit, offset], row_to_job)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

#[tauri::command]
pub async fn get_newspaper_library_page(
    state: State<'_, NewspaperState>,
    query: String,
    kind: String,
    status: String,
    offset: u32,
    limit: u32,
) -> Result<NewspaperLibraryPage, String> {
    if query.chars().count() > 200
        || !matches!(kind.as_str(), "all" | "daily" | "weekly" | "special")
        || !matches!(
            status.as_str(),
            "all" | "completed" | "partial" | "optimizing"
        )
        || !(1..=100).contains(&limit)
    {
        return Err("INVALID_LIBRARY_QUERY".to_string());
    }
    let db_path = state.db_path.clone();
    let revision = state.library_revision();
    tauri::async_runtime::spawn_blocking(move || {
        query_library_page(&db_path, &query, &kind, &status, offset, limit, revision)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn query_library_page(
    db_path: &Path,
    query: &str,
    kind: &str,
    status: &str,
    offset: u32,
    limit: u32,
    revision: u64,
) -> Result<NewspaperLibraryPage, String> {
    let connection = Connection::open(db_path).map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let escaped = query
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let total = connection
        .query_row(
            "SELECT COUNT(*)
             FROM newspaper_jobs j
             JOIN newspaper_editions e ON e.code = j.edition_code
                AND e.publication_date = j.edition_publication_date
             WHERE j.status IN ('completed', 'partial', 'optimizing')
               AND j.dismissed = 0
               AND (?1 = 'all' OR e.kind = ?1)
               AND (?2 = 'all' OR j.status = ?2)
               AND (
                   ?3 = '%%'
                   OR e.name_zh LIKE ?3 ESCAPE '\\'
                   OR e.name_en LIKE ?3 ESCAPE '\\'
                   OR j.edition_code LIKE ?3 ESCAPE '\\'
                   OR j.publication_date LIKE ?3 ESCAPE '\\'
               )",
            params![kind, status, pattern],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT
                j.id, j.edition_code, e.name_zh, j.publication_date, j.status,
                j.output_dir, j.page_count, j.completed_count, j.warning, j.updated_at,
                p.last_page_id, p.last_page_index, p.furthest_page_index, p.updated_at,
                fp.id, fp.media_version,
                t.cache_path, t.source_page_id, t.source_media_version,
                t.cache_schema_version, t.byte_count
             FROM newspaper_jobs j
             JOIN newspaper_editions e ON e.code = j.edition_code
                AND e.publication_date = j.edition_publication_date
             LEFT JOIN newspaper_reading_progress p ON p.job_id = j.id
             LEFT JOIN newspaper_pages fp ON fp.id = (
                 SELECT first_page.id
                 FROM newspaper_pages first_page
                 WHERE first_page.job_id = j.id
                   AND first_page.status = 'completed'
                   AND COALESCE(first_page.optimized_path, first_page.original_path) IS NOT NULL
                 ORDER BY CASE WHEN first_page.page_number = 'A01' THEN 0 ELSE 1 END,
                          first_page.page_number
                 LIMIT 1
             )
             LEFT JOIN newspaper_thumbnail_cache t ON t.job_id = j.id
             WHERE j.status IN ('completed', 'partial', 'optimizing')
               AND j.dismissed = 0
               AND (?1 = 'all' OR e.kind = ?1)
               AND (?2 = 'all' OR j.status = ?2)
               AND (
                   ?3 = '%%'
                   OR e.name_zh LIKE ?3 ESCAPE '\\'
                   OR e.name_en LIKE ?3 ESCAPE '\\'
                   OR j.edition_code LIKE ?3 ESCAPE '\\'
                   OR j.publication_date LIKE ?3 ESCAPE '\\'
               )
             ORDER BY j.publication_date DESC, j.updated_at DESC, j.id
             LIMIT ?4 OFFSET ?5",
        )
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let rows = statement
        .query_map(params![kind, status, pattern, limit, offset], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<u32>>(11)?,
                row.get::<_, Option<u32>>(12)?,
                row.get::<_, Option<i64>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<i64>>(15)?,
                row.get::<_, Option<String>>(16)?,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, Option<i64>>(18)?,
                row.get::<_, Option<i64>>(19)?,
                row.get::<_, Option<u64>>(20)?,
            ))
        })
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let items = rows
        .into_iter()
        .map(
            |(
                job_id,
                edition_code,
                edition_name,
                publication_date,
                status,
                output_dir,
                page_count,
                completed_count,
                warning,
                updated_at,
                last_page_id,
                last_page_index,
                furthest_page_index,
                reading_updated_at,
                first_page_id,
                first_media_version,
                cache_path,
                cache_page_id,
                cache_media_version,
                cache_schema_version,
                cache_bytes,
            )| {
                let thumbnail_version = first_media_version
                    .zip(cache_schema_version)
                    .map(|(media, schema)| format!("{media}-{schema}"));
                let thumbnail_ready = first_page_id.is_some()
                    && first_page_id == cache_page_id
                    && first_media_version == cache_media_version
                    && cache_schema_version == Some(1)
                    && cache_path
                        .as_ref()
                        .and_then(|path| std::fs::metadata(path).ok())
                        .is_some_and(|metadata| {
                            metadata.is_file()
                                && metadata.len() > 0
                                && Some(metadata.len()) == cache_bytes
                        });
                NewspaperLibraryItem {
                    thumbnail_url: thumbnail_ready.then(|| {
                        super::thumbnails::thumbnail_url(
                            &job_id,
                            thumbnail_version.as_deref().unwrap_or_default(),
                        )
                    }),
                    thumbnail_version: thumbnail_ready.then_some(thumbnail_version).flatten(),
                    thumbnail_ready,
                    job_id,
                    edition_code,
                    edition_name,
                    publication_date,
                    status,
                    output_dir,
                    page_count,
                    completed_count,
                    warning,
                    updated_at,
                    last_page_id,
                    last_page_index,
                    furthest_page_index,
                    reading_updated_at,
                }
            },
        )
        .collect();
    Ok(NewspaperLibraryPage {
        items,
        total,
        offset,
        limit,
        revision,
    })
}

#[tauri::command]
pub async fn get_newspaper_activity_snapshot(
    state: State<'_, NewspaperState>,
) -> Result<NewspaperActivitySnapshot, String> {
    let db_path = state.db_path.clone();
    let revision = state.library_revision();
    tauri::async_runtime::spawn_blocking(move || {
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        let jobs = list_jobs(&connection, None)?;
        let has_live_activity = jobs.iter().any(|job| {
            matches!(job.status.as_str(), "queued" | "active" | "optimizing")
                || job.retry_at.is_some()
        });
        Ok(NewspaperActivitySnapshot {
            jobs,
            batches: list_batches(&connection)?,
            schedules: list_schedules(&connection)?,
            has_live_activity,
            revision,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn get_newspaper_reader_manifest(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<Vec<NewspaperPage>, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || reader_service::manifest(&db_path, &job_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn save_newspaper_reading_progress(
    state: State<'_, NewspaperState>,
    job_id: String,
    page_id: String,
) -> Result<NewspaperReadingProgress, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    reader_service::save_progress(&connection, &job_id, &page_id, Utc::now().timestamp())
}

#[tauri::command]
pub async fn ensure_newspaper_thumbnail(
    state: State<'_, ThumbnailCoordinator>,
    job_id: String,
) -> Result<EnsureThumbnailResult, String> {
    state.ensure(job_id).await
}

#[tauri::command]
pub fn open_newspaper_download_folder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        return Err("Opening newspaper folders is currently supported on Windows.".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn import_existing_newspaper_archive(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    path: String,
) -> Result<usize, String> {
    let db_path = state.db_path.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || import_archive(&db_path, Path::new(&path)))
            .await
            .map_err(|error| error.to_string())?;
    if result.is_ok() {
        let candidates = thumbnail_candidate_job_ids(&state.db_path, 14)?;
        let revision = state.invalidate_library();
        let _ = app.emit(
            "newspaper://library-invalidated",
            serde_json::json!({ "revision": revision, "jobIds": candidates.clone() }),
        );
        schedule_thumbnail_generation(&app, candidates);
        schedule_page_dimension_backfill(&app);
    }
    result
}

#[tauri::command]
pub async fn repair_newspaper_library(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
) -> Result<RepairNewspaperLibraryResult, String> {
    let db_path = state.db_path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || repair_library_files(&db_path))
        .await
        .map_err(|error| error.to_string())?;
    if result.is_ok() {
        let candidates = thumbnail_candidate_job_ids(&state.db_path, 14)?;
        let revision = state.invalidate_library();
        let _ = app.emit(
            "newspaper://library-invalidated",
            serde_json::json!({ "revision": revision, "jobIds": candidates.clone() }),
        );
        schedule_thumbnail_generation(&app, candidates);
        schedule_page_dimension_backfill(&app);
    }
    result
}

fn thumbnail_candidate_job_ids(db_path: &Path, limit: u32) -> Result<Vec<String>, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT j.id
             FROM newspaper_jobs j
             JOIN newspaper_pages p ON p.job_id = j.id
             WHERE j.status IN ('completed', 'partial')
               AND j.dismissed = 0
               AND p.status = 'completed'
             ORDER BY j.updated_at DESC
             LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let candidates = statement
        .query_map(params![limit], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(candidates)
}

async fn process_queue(
    db_path: &Path,
    cancelled: &Arc<AtomicBool>,
) -> Result<Vec<NewspaperJob>, String> {
    let client = NewspaperClient::new().map_err(|error| error.to_string())?;
    let mut processed = Vec::new();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let next = {
            let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            next_due_job(&connection)?
        };
        let Some((job, delay_seconds, scheduled_at)) = next else {
            let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            mark_retry_waiting_batches_scheduled(&connection)?;
            break;
        };
        if scheduled_at.is_some_and(|scheduled| scheduled < Utc::now().timestamp()) {
            let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            connection
                .execute(
                    "INSERT INTO newspaper_events
                     (batch_id, job_id, event_type, message, created_at)
                     VALUES (?1, ?2, 'overdue_start',
                             'Scheduled batch started after its requested time.', ?3)",
                    params![job.batch_id, job.id, Utc::now().timestamp()],
                )
                .map_err(|error| error.to_string())?;
        }
        let outcome = process_job(db_path, &client, job.clone(), cancelled).await?;
        processed.push(outcome);
        let has_next_due_job = {
            let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            next_due_job(&connection)?.is_some()
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

async fn process_job(
    db_path: &Path,
    client: &NewspaperClient,
    mut job: NewspaperJob,
    cancelled: &Arc<AtomicBool>,
) -> Result<NewspaperJob, String> {
    let now = Utc::now().timestamp();
    {
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        connection
            .execute(
                "UPDATE newspaper_jobs
                 SET status = 'active', retry_at = NULL, warning = NULL, updated_at = ?2
                 WHERE id = ?1",
                params![job.id, now],
            )
            .map_err(|error| error.to_string())?;
        connection.execute(
            "UPDATE newspaper_batches SET status = 'active', scheduled_at = NULL, updated_at = ?2 WHERE id = ?1",
            params![job.batch_id, now],
        ).map_err(|error| error.to_string())?;
    }
    job.status = "active".to_string();
    let manifest = match client
        .fetch_manifest(&job.edition_code, &job.publication_date, cancelled)
        .await
    {
        Ok(value) => value,
        Err(FetchError::Unavailable) => {
            if publication_is_today_or_future(&job.publication_date) {
                schedule_release_retry(db_path, &mut job, "Edition has not been released yet.")?;
            } else {
                update_job_terminal(db_path, &job.id, "unavailable", None)?;
                job.status = "unavailable".to_string();
            }
            return Ok(job);
        }
        Err(FetchError::Cancelled) => {
            job.status = apply_interrupted_job_state(db_path, &job)?.to_string();
            return Ok(job);
        }
        Err(error) => {
            if publication_is_today_or_future(&job.publication_date)
                && matches!(
                    &error,
                    FetchError::Manifest(
                        manifest::ManifestError::InvalidContentType
                            | manifest::ManifestError::HtmlBody
                            | manifest::ManifestError::Empty
                    )
                )
            {
                schedule_release_retry(db_path, &mut job, "Edition has not been released yet.")?;
            } else {
                update_job_terminal(db_path, &job.id, "failed", Some(&error.to_string()))?;
                job.status = "failed".to_string();
            }
            return Ok(job);
        }
    };

    let referer = client
        .origin()
        .join(&format!("/{}/{}", job.edition_code, job.publication_date))
        .map_err(|error| error.to_string())?;
    let pages: Vec<_> = manifest.pages().cloned().collect();
    {
        let mut connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE newspaper_jobs SET page_count = ?2, updated_at = ?3 WHERE id = ?1",
                params![job.id, pages.len() as i64, Utc::now().timestamp()],
            )
            .map_err(|error| error.to_string())?;
        for page in &pages {
            let source_url =
                manifest::resolve_page_url_with_origin(&page.pagefile, client.origin())
                    .map_err(|error| error.to_string())?;
            let extension = manifest::page_file_extension(&source_url);
            let destination = Path::new(&job.output_dir).join(format!(
                "{}.{}",
                sanitize_segment(&page.pageno),
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
                        unique_id("newspaper-page"),
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
    }

    for (page_index, page) in pages.into_iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let source_url = manifest::resolve_page_url_with_origin(&page.pagefile, client.origin())
            .map_err(|error| error.to_string())?;
        let extension = manifest::page_file_extension(&source_url);
        let destination = Path::new(&job.output_dir).join(format!(
            "{}.{}",
            sanitize_segment(&page.pageno),
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
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        if matches!(&result, Err(PageDownloadError::NotReleased)) {
            drop(connection);
            schedule_release_retry(db_path, &mut job, "Edition has not been released yet.")?;
            return Ok(job);
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
                    let _ = std::fs::remove_file(&destination);
                    drop(connection);
                    schedule_release_retry(
                        db_path,
                        &mut job,
                        "Edition is still showing an earlier newspaper; retry scheduled.",
                    )?;
                    return Ok(job);
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
                if cancelled.load(Ordering::SeqCst) {
                    drop(connection);
                    job.status = apply_interrupted_job_state(db_path, &job)?.to_string();
                    return Ok(job);
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
    }
    if cancelled.load(Ordering::SeqCst) {
        job.status = apply_interrupted_job_state(db_path, &job)?.to_string();
        return Ok(job);
    }
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let completion = storage::finalize_job(&connection, &job.id, Utc::now().timestamp())
        .map_err(|error| error.to_string())?;
    job.status = completion.status;
    job.page_count = completion.page_count as u32;
    job.completed_count = completion.completed_count as u32;
    job.failed_count = completion.failed_count as u32;
    finish_batch_if_terminal(&connection, &job.batch_id)?;
    Ok(job)
}

async fn process_optimization_queue(db_path: &Path) -> Result<Vec<NewspaperJob>, String> {
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
            list_jobs(&connection, None)?
                .into_iter()
                .find(|item| item.id == job_id)
                .ok_or_else(|| format!("Newspaper job disappeared before optimization: {job_id}"))?
        };
        let optimization_db_path = db_path.to_path_buf();
        let optimization_job = job.clone();
        tauri::async_runtime::spawn_blocking(move || {
            optimize_completed_pages(&optimization_db_path, &optimization_job)
        })
        .await
        .map_err(|error| error.to_string())??;
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        storage::finalize_job(&connection, &job.id, Utc::now().timestamp())
            .map_err(|error| error.to_string())?;
        let refreshed = list_jobs(&connection, None)?
            .into_iter()
            .find(|item| item.id == job.id)
            .ok_or_else(|| format!("Newspaper job disappeared after optimization: {}", job.id))?;
        processed.push(refreshed);
    }
    Ok(processed)
}

fn optimize_completed_pages(db_path: &Path, job: &NewspaperJob) -> Result<(), String> {
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

fn next_due_job(
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
            |row| Ok((row_to_job(row)?, row.get(19)?, row.get(20)?)),
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn list_catalog_records(connection: &Connection) -> Result<Vec<NewspaperEdition>, String> {
    let mut statement = connection
        .prepare(
            "SELECT code, name_zh, name_en, kind, schedule, source_url,
                NULLIF(publication_date, ''), discovered
         FROM newspaper_editions WHERE active = 1
         ORDER BY CASE kind WHEN 'daily' THEN 0 WHEN 'weekly' THEN 1 ELSE 2 END,
                  code, publication_date DESC",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map([], |row| {
            let kind: String = row.get(3)?;
            let schedule: String = row.get(4)?;
            let publication_date: Option<String> = row.get(6)?;
            Ok(NewspaperEdition {
                code: row.get(0)?,
                name_zh: row.get(1)?,
                name_en: row.get(2)?,
                kind: match kind.as_str() {
                    "weekly" => EditionKind::Weekly,
                    "special" => EditionKind::Special,
                    _ => EditionKind::Daily,
                },
                schedule: match schedule.as_str() {
                    "weekly_sunday" => PublicationSchedule::WeeklySunday,
                    "ad_hoc" => PublicationSchedule::AdHoc,
                    _ => PublicationSchedule::Daily,
                },
                source_url: row.get(5)?,
                publication_date: publication_date
                    .and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok()),
                discovered: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

fn list_batches(connection: &Connection) -> Result<Vec<NewspaperBatch>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, status, destination, scheduled_at, delay_seconds, optimize_images,
                optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at
         FROM newspaper_batches ORDER BY created_at DESC",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map([], |row| {
            Ok(NewspaperBatch {
                id: row.get(0)?,
                status: row.get(1)?,
                destination: row.get(2)?,
                scheduled_at: row.get(3)?,
                delay_seconds: row.get(4)?,
                optimize_images: row.get(5)?,
                optimization_profile: row.get(6)?,
                optimization_quality: row.get(7)?,
                keep_original_jpg: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

fn list_schedules(connection: &Connection) -> Result<Vec<NewspaperSchedule>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, enabled, cron_time, destination, edition_codes_json, delay_seconds,
                    optimize_images, optimization_profile, optimization_quality, keep_original_jpg,
                    last_run_date, last_error, created_at, updated_at
             FROM newspaper_schedules ORDER BY created_at DESC",
        )
        .map_err(|error| error.to_string())?;
    let schedules = statement
        .query_map([], |row| {
            let edition_codes_json: String = row.get(4)?;
            Ok(NewspaperSchedule {
                id: row.get(0)?,
                enabled: row.get(1)?,
                cron_time: row.get(2)?,
                destination: row.get(3)?,
                edition_codes: serde_json::from_str(&edition_codes_json).unwrap_or_default(),
                delay_seconds: row.get(5)?,
                optimize_images: row.get(6)?,
                optimization_profile: row.get(7)?,
                optimization_quality: row.get(8)?,
                keep_original_jpg: row.get(9)?,
                last_run_date: row.get(10)?,
                last_error: row.get(11)?,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(schedules)
}

fn list_jobs(connection: &Connection, batch_id: Option<&str>) -> Result<Vec<NewspaperJob>, String> {
    let mut statement = connection
        .prepare(
            "SELECT j.id, j.batch_id, j.edition_code, e.name_zh, j.publication_date,
                j.status, j.output_dir, j.page_count, j.completed_count,
                j.failed_count, j.retry_at, j.retry_count, j.warning,
                j.queue_position, j.paused, j.dismissed, j.created_at,
                j.updated_at, j.completed_at
         FROM newspaper_jobs j JOIN newspaper_editions e
           ON e.code = j.edition_code AND e.publication_date = j.edition_publication_date
         WHERE (?1 IS NULL OR j.batch_id = ?1)
         ORDER BY j.dismissed, j.queue_position, j.created_at DESC",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params![batch_id], row_to_job)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<NewspaperJob> {
    Ok(NewspaperJob {
        id: row.get(0)?,
        batch_id: row.get(1)?,
        edition_code: row.get(2)?,
        edition_name: row.get(3)?,
        publication_date: row.get(4)?,
        status: row.get(5)?,
        output_dir: row.get(6)?,
        page_count: row.get(7)?,
        completed_count: row.get(8)?,
        failed_count: row.get(9)?,
        retry_at: row.get(10)?,
        retry_count: row.get(11)?,
        warning: row.get(12)?,
        queue_position: row.get(13)?,
        paused: row.get(14)?,
        dismissed: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        completed_at: row.get(18)?,
    })
}

fn load_settings(connection: &Connection) -> Result<serde_json::Value, String> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value_json FROM newspaper_settings WHERE key = 'preferences'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    value
        .map(|json| serde_json::from_str(&json).unwrap_or_default())
        .unwrap_or_else(|| serde_json::json!({}))
        .pipe(Ok)
}

fn validate_request(request: &CreateNewspaperBatchRequest) -> Result<(), String> {
    if request.destination.trim().is_empty() {
        return Err("Choose a newspaper download folder.".to_string());
    }
    if request.delay_seconds > 3_600 {
        return Err("Delay must be between 0 and 3,600 seconds.".to_string());
    }
    if !matches!(
        request.optimization_profile.as_str(),
        "webp_high" | "webp_balanced"
    ) {
        return Err("Unsupported image optimization profile.".to_string());
    }
    if !(25..=95).contains(&request.optimization_quality) {
        return Err("Image quality must be between 25 and 95.".to_string());
    }
    Ok(())
}

fn validate_schedule_request(request: &CreateNewspaperScheduleRequest) -> Result<(), String> {
    if request.destination.trim().is_empty() {
        return Err("Choose a newspaper download folder.".to_string());
    }
    if request.edition_codes.is_empty() {
        return Err("Select at least one newspaper edition.".to_string());
    }
    NaiveTime::parse_from_str(&request.cron_time, "%H:%M")
        .map_err(|_| "Choose a valid daily schedule time.".to_string())?;
    if request.delay_seconds > 3_600 {
        return Err("Delay must be between 0 and 3,600 seconds.".to_string());
    }
    if !matches!(
        request.optimization_profile.as_str(),
        "webp_high" | "webp_balanced"
    ) {
        return Err("Unsupported image optimization profile.".to_string());
    }
    if !(25..=95).contains(&request.optimization_quality) {
        return Err("Image quality must be between 25 and 95.".to_string());
    }
    Ok(())
}

fn materialize_due_schedules(db_path: &Path) -> Result<(), String> {
    let now_local = Local::now();
    let today = now_local.date_naive().to_string();
    let schedules = {
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        list_schedules(&connection)?
    };
    for schedule in schedules {
        let cron = match NaiveTime::parse_from_str(&schedule.cron_time, "%H:%M") {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !schedule.enabled
            || schedule.last_run_date.as_deref() == Some(today.as_str())
            || now_local.time() < cron
        {
            continue;
        }
        let request = CreateNewspaperBatchRequest {
            edition_codes: schedule.edition_codes.clone(),
            date_mode: DateMode::Single,
            start_date: today.clone(),
            end_date: None,
            destination: schedule.destination.clone(),
            scheduled_at: None,
            delay_seconds: schedule.delay_seconds,
            optimize_images: schedule.optimize_images,
            optimization_profile: schedule.optimization_profile.clone(),
            optimization_quality: schedule.optimization_quality,
            keep_original_jpg: schedule.keep_original_jpg,
        };
        let result = {
            let mut connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            create_batch(&mut connection, request)
        };
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        match result {
            Ok(_) => {
                connection
                    .execute(
                        "UPDATE newspaper_schedules
                         SET last_run_date = ?2, last_error = NULL, updated_at = ?3
                         WHERE id = ?1",
                        params![schedule.id, today, Utc::now().timestamp()],
                    )
                    .map_err(|error| error.to_string())?;
            }
            Err(error) => {
                connection
                    .execute(
                        "UPDATE newspaper_schedules
                         SET last_error = ?2, updated_at = ?3 WHERE id = ?1",
                        params![schedule.id, error, Utc::now().timestamp()],
                    )
                    .map_err(|sql_error| sql_error.to_string())?;
            }
        }
    }
    Ok(())
}

fn parse_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("Invalid publication date: {value}"))
}

fn edition_key(edition: &NewspaperEdition) -> String {
    edition
        .publication_date
        .map(|date| format!("{}@{date}", edition.code))
        .unwrap_or_else(|| edition.code.clone())
}

fn unique_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        Utc::now().timestamp_millis(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn sanitize_segment(value: &str) -> String {
    value
        .chars()
        .filter(|character| !r#"\/:*?"<>|"#.contains(*character) && !character.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

fn update_job_terminal(
    db_path: &Path,
    job_id: &str,
    status: &str,
    warning: Option<&str>,
) -> Result<(), String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = ?2, warning = ?3, updated_at = ?4 WHERE id = ?1",
            params![job_id, status, warning, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn apply_interrupted_job_state(db_path: &Path, job: &NewspaperJob) -> Result<&'static str, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
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

fn publication_is_today_or_future(publication_date: &str) -> bool {
    NaiveDate::parse_from_str(publication_date, "%Y-%m-%d")
        .map(|date| date >= Local::now().date_naive())
        .unwrap_or(false)
}

fn schedule_release_retry(
    db_path: &Path,
    job: &mut NewspaperJob,
    reason: &str,
) -> Result<(), String> {
    const RETRY_SECONDS: i64 = 30 * 60;
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
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

fn first_page_matches_another_date(
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

fn refresh_job_progress(connection: &Connection, job_id: &str) -> Result<(), String> {
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

fn mark_retry_waiting_batches_scheduled(connection: &Connection) -> Result<(), String> {
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

fn repair_library_files(db_path: &Path) -> Result<RepairNewspaperLibraryResult, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let legacy_pages = {
        let mut statement = connection
            .prepare(
                "SELECT id, original_path FROM newspaper_pages
                 WHERE status = 'completed' AND optimized_path IS NULL
                   AND LOWER(original_path) LIKE '%.php'",
            )
            .map_err(|error| error.to_string())?;
        let result = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        result
    };
    let mut renamed_files = 0_u32;
    let mut warnings = Vec::new();
    for (page_id, original_path) in legacy_pages {
        let source = PathBuf::from(&original_path);
        let bytes = match std::fs::read(&source) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("Could not read {}: {error}", source.display()));
                continue;
            }
        };
        let extension = match image::guess_format(&bytes) {
            Ok(image::ImageFormat::Jpeg) => "jpg",
            Ok(image::ImageFormat::Png) => "png",
            Ok(image::ImageFormat::WebP) => "webp",
            Ok(_) => "jpg",
            Err(error) => {
                warnings.push(format!("Could not identify {}: {error}", source.display()));
                continue;
            }
        };
        let destination = source.with_extension(extension);
        if destination.exists() && destination != source {
            warnings.push(format!(
                "Could not rename {} because {} already exists.",
                source.display(),
                destination.display()
            ));
            continue;
        }
        if destination != source {
            std::fs::rename(&source, &destination).map_err(|error| error.to_string())?;
        }
        connection
            .execute(
                "UPDATE newspaper_pages SET original_path = ?2, updated_at = ?3 WHERE id = ?1",
                params![
                    page_id,
                    destination.to_string_lossy(),
                    Utc::now().timestamp()
                ],
            )
            .map_err(|error| error.to_string())?;
        renamed_files = renamed_files.saturating_add(1);
    }

    let jobs = {
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT j.id
                 FROM newspaper_jobs j
                 JOIN newspaper_batches b ON b.id = j.batch_id
                 JOIN newspaper_pages p ON p.job_id = j.id
                 WHERE j.status IN ('completed', 'partial')
                   AND b.optimize_images = 1
                   AND p.status = 'completed'
                   AND p.optimized_path IS NULL
                   AND p.original_path IS NOT NULL",
            )
            .map_err(|error| error.to_string())?;
        let result = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        result
    };
    let mut optimized_jobs = 0_u32;
    for job_id in jobs {
        let job = list_jobs(&connection, None)?
            .into_iter()
            .find(|item| item.id == job_id)
            .ok_or_else(|| format!("Newspaper job disappeared during repair: {job_id}"))?;
        connection
            .execute(
                "UPDATE newspaper_jobs SET warning = NULL WHERE id = ?1",
                params![job.id],
            )
            .map_err(|error| error.to_string())?;
        optimize_completed_pages(db_path, &job)?;
        storage::finalize_job(&connection, &job.id, Utc::now().timestamp())
            .map_err(|error| error.to_string())?;
        optimized_jobs = optimized_jobs.saturating_add(1);
    }
    let (removed_source_files, cleanup_warnings) = remove_redundant_optimized_sources(&connection)?;
    warnings.extend(cleanup_warnings);
    Ok(RepairNewspaperLibraryResult {
        renamed_files,
        optimized_jobs,
        removed_source_files,
        warnings,
    })
}

fn remove_redundant_optimized_sources(
    connection: &Connection,
) -> Result<(u32, Vec<String>), String> {
    let candidates = connection
        .prepare(
            "SELECT p.original_path, p.optimized_path
             FROM newspaper_pages p
             JOIN newspaper_jobs j ON j.id = p.job_id
             JOIN newspaper_batches b ON b.id = j.batch_id
             WHERE p.status = 'completed'
               AND b.keep_original_jpg = 0
               AND p.original_path IS NOT NULL
               AND p.optimized_path IS NOT NULL
               AND p.original_path <> p.optimized_path",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut removed = 0_u32;
    let mut warnings = Vec::new();
    for (source, optimized) in candidates {
        let source = PathBuf::from(source);
        let optimized = PathBuf::from(optimized);
        if !source.exists() {
            continue;
        }
        let source_reader = match image::io::Reader::open(&source)
            .and_then(|reader| reader.with_guessed_format())
        {
            Ok(reader) => reader,
            Err(error) => {
                warnings.push(format!(
                    "Could not validate source {}: {error}",
                    source.display()
                ));
                continue;
            }
        };
        let optimized_reader = match image::io::Reader::open(&optimized)
            .and_then(|reader| reader.with_guessed_format())
        {
            Ok(reader) => reader,
            Err(error) => {
                warnings.push(format!(
                    "Could not validate optimized image {}: {error}",
                    optimized.display()
                ));
                continue;
            }
        };
        if source_reader.format() != Some(image::ImageFormat::Jpeg)
            || optimized_reader.format() != Some(image::ImageFormat::WebP)
        {
            warnings.push(format!(
                "Kept source {} because the validated pair is not JPEG and WebP.",
                source.display()
            ));
            continue;
        }
        let source_dimensions = image::image_dimensions(&source);
        let optimized_dimensions = image::image_dimensions(&optimized);
        if source_dimensions.is_err()
            || optimized_dimensions.is_err()
            || source_dimensions.as_ref().ok() != optimized_dimensions.as_ref().ok()
        {
            warnings.push(format!(
                "Kept source {} because optimized dimensions could not be matched.",
                source.display()
            ));
            continue;
        }
        match std::fs::remove_file(&source) {
            Ok(()) => removed = removed.saturating_add(1),
            Err(error) => warnings.push(format!(
                "Could not remove redundant source {}: {error}",
                source.display()
            )),
        }
    }
    Ok((removed, warnings))
}

fn import_archive(db_path: &Path, root: &Path) -> Result<usize, String> {
    if !root.is_dir() {
        return Err("The selected newspaper archive folder does not exist.".to_string());
    }
    let mut stack = vec![root.to_path_buf()];
    let mut groups: std::collections::BTreeMap<(String, String, PathBuf), Vec<PathBuf>> =
        std::collections::BTreeMap::new();
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| error.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some((code, date)) = archive_identity(file_name, path.parent()) else {
                continue;
            };
            groups
                .entry((code, date, path.parent().unwrap_or(root).to_path_buf()))
                .or_default()
                .push(path);
        }
    }

    let mut connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let now = Utc::now().timestamp();
    let batch_id = unique_id("newspaper-import");
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO newspaper_batches
            (id, status, destination, delay_minutes, optimize_images,
             optimization_profile, keep_original_jpg, created_at, updated_at, completed_at)
            VALUES (?1, 'completed', ?2, 0, 0, 'webp_high', 1, ?3, ?3, ?3)",
            params![batch_id, root.to_string_lossy(), now],
        )
        .map_err(|error| error.to_string())?;
    let mut imported = 0;
    for ((code, date, directory), mut files) in groups {
        let edition_exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM newspaper_editions WHERE code = ?1 AND publication_date = '')",
                params![code],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !edition_exists {
            continue;
        }
        files.sort();
        let job_id = unique_id("newspaper-import-job");
        let mut valid_pages = Vec::new();
        for file in files {
            let bytes = match std::fs::read(&file) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let image = match image::load_from_memory(&bytes) {
                Ok(image) => image,
                Err(_) => continue,
            };
            let (width, height) = image.dimensions();
            let page_number = archive_page_number(&file);
            valid_pages.push((
                file,
                page_number,
                bytes.len() as u64,
                format!("{:x}", Sha256::digest(&bytes)),
                width,
                height,
            ));
        }
        if valid_pages.is_empty() {
            continue;
        }
        transaction
            .execute(
                "INSERT INTO newspaper_jobs
                (id, batch_id, edition_code, publication_date, status, output_dir,
                 page_count, completed_count, original_bytes, final_bytes,
                 created_at, updated_at, completed_at)
                VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?6, ?6, ?7, ?7, ?8, ?8, ?8)
                ON CONFLICT(edition_code, publication_date, output_dir) DO NOTHING",
                params![
                    job_id,
                    batch_id,
                    code,
                    date,
                    directory.to_string_lossy(),
                    valid_pages.len() as i64,
                    valid_pages.iter().map(|item| item.2).sum::<u64>(),
                    now,
                ],
            )
            .map_err(|error| error.to_string())?;
        if transaction.changes() == 0 {
            continue;
        }
        for (file, page_number, bytes, checksum, width, height) in valid_pages {
            transaction
                .execute(
                    "INSERT INTO newspaper_pages
                    (id, job_id, page_number, source_url, original_path, status,
                     attempts, original_bytes, final_bytes, checksum,
                     pixel_width, pixel_height, created_at, updated_at)
                    VALUES (?1, ?2, ?3, 'archive://local', ?4, 'completed', 0,
                            ?5, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![
                        unique_id("newspaper-import-page"),
                        job_id,
                        page_number,
                        file.to_string_lossy(),
                        bytes,
                        checksum,
                        width,
                        height,
                        now,
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        imported += 1;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(imported)
}

fn archive_identity(file_name: &str, parent: Option<&Path>) -> Option<(String, String)> {
    let code = file_name
        .split('_')
        .next()
        .filter(|value| value.len() == 2 && value.chars().all(|ch| ch.is_ascii_uppercase()))?
        .to_string();
    let parent_date = parent
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
        .filter(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok())
        .map(str::to_string);
    let file_date = file_name
        .split('_')
        .nth(1)
        .filter(|value| value.len() >= 8)
        .and_then(|value| NaiveDate::parse_from_str(&value[..8], "%Y%m%d").ok())
        .map(|value| value.to_string());
    Some((code, parent_date.or(file_date)?))
}

fn archive_page_number(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("page");
    let tail = stem.rsplit('_').next().unwrap_or(stem);
    let page = tail
        .char_indices()
        .find(|(_, character)| character.is_ascii_alphabetic())
        .map(|(index, _)| &tail[index..])
        .unwrap_or(tail);
    sanitize_segment(page)
}

fn finish_batch_if_terminal(connection: &Connection, batch_id: &str) -> Result<(), String> {
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM newspaper_jobs
         WHERE batch_id = ?1 AND status IN ('queued', 'active', 'optimizing')",
            params![batch_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if remaining == 0 {
        let warnings: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM newspaper_jobs
             WHERE batch_id = ?1 AND status IN ('partial', 'failed', 'unavailable')",
                params![batch_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        connection.execute(
            "UPDATE newspaper_batches SET status = ?2, completed_at = ?3, updated_at = ?3 WHERE id = ?1",
            params![
                batch_id,
                if warnings > 0 { "completed_with_warnings" } else { "completed" },
                Utc::now().timestamp()
            ],
        ).map_err(|error| error.to_string())?;
    }
    Ok(())
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}
impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::newspaper::models::DateMode;
    use tempfile::tempdir;

    fn request(destination: &Path, date: &str) -> CreateNewspaperBatchRequest {
        CreateNewspaperBatchRequest {
            edition_codes: vec!["NY".to_string()],
            date_mode: DateMode::Single,
            start_date: date.to_string(),
            end_date: None,
            destination: destination.to_string_lossy().into_owned(),
            scheduled_at: None,
            delay_seconds: 15,
            optimize_images: true,
            optimization_profile: "webp_high".to_string(),
            optimization_quality: 92,
            keep_original_jpg: false,
        }
    }

    #[test]
    fn request_validation_rejects_invalid_delay_profile_and_quality() {
        let mut request = CreateNewspaperBatchRequest {
            edition_codes: vec!["NY".to_string()],
            date_mode: DateMode::Single,
            start_date: "2026-07-24".to_string(),
            end_date: None,
            destination: "C:/papers".to_string(),
            scheduled_at: None,
            delay_seconds: 3_601,
            optimize_images: true,
            optimization_profile: "webp_high".to_string(),
            optimization_quality: 92,
            keep_original_jpg: false,
        };
        assert!(validate_request(&request).is_err());
        request.delay_seconds = 15;
        request.optimization_profile = "lossless".to_string();
        assert!(validate_request(&request).is_err());
        request.optimization_profile = "webp_high".to_string();
        request.optimization_quality = 25;
        assert!(validate_request(&request).is_ok());
        request.optimization_quality = 24;
        assert!(validate_request(&request).is_err());
    }

    #[test]
    fn one_failed_page_does_not_retain_successfully_converted_sources() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let destination = directory.path().join("papers");
        let mut batch_request = request(&destination, "2026-07-24");
        batch_request.optimization_quality = 25;
        let job = create_batch(&mut connection, batch_request)
            .unwrap()
            .jobs
            .remove(0);
        std::fs::create_dir_all(&job.output_dir).unwrap();

        let valid_path = Path::new(&job.output_dir).join("A01.jpg");
        let image = image::ImageBuffer::from_fn(480, 640, |x, y| {
            let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
            image::Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        });
        image
            .save_with_format(&valid_path, image::ImageFormat::Jpeg)
            .unwrap();
        let invalid_path = Path::new(&job.output_dir).join("A02.jpg");
        std::fs::write(&invalid_path, b"not an image").unwrap();

        connection
            .execute(
                "UPDATE newspaper_jobs
                 SET status = 'completed', page_count = 2, completed_count = 2
                 WHERE id = ?1",
                params![job.id],
            )
            .unwrap();
        for (id, page, path) in [
            ("valid-page", "A01", &valid_path),
            ("invalid-page", "A02", &invalid_path),
        ] {
            let bytes = std::fs::metadata(path).unwrap().len();
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                     (id, job_id, page_number, source_url, original_path, status,
                      original_bytes, final_bytes, checksum, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'test://page', ?4, 'completed',
                             ?5, ?5, 'checksum', 1, 1)",
                    params![id, job.id, page, path.to_string_lossy(), bytes],
                )
                .unwrap();
        }
        drop(connection);

        optimize_completed_pages(&db_path, &job).unwrap();

        assert!(valid_path.with_extension("webp").exists());
        assert!(
            !valid_path.exists(),
            "a warning on another page must not retain a successfully converted source"
        );
        assert!(invalid_path.exists());
        let connection = Connection::open(&db_path).unwrap();
        let warning: Option<String> = connection
            .query_row(
                "SELECT warning FROM newspaper_jobs WHERE id = ?1",
                params![job.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(warning.is_some());
    }

    #[test]
    fn list_catalog_reads_seeded_regular_editions() {
        let directory = tempdir().unwrap();
        let connection =
            crate::cache::open_or_initialize(&directory.path().join("test.db")).unwrap();
        let catalog = list_catalog_records(&connection).unwrap();
        assert_eq!(catalog.len(), 13);
        assert_eq!(
            catalog
                .iter()
                .filter(|item| item.kind == EditionKind::Daily)
                .count(),
            10
        );
    }

    #[test]
    fn archive_identity_uses_parent_date_or_filename_date() {
        assert_eq!(
            archive_identity(
                "NY_20260724_NY20260724A01.jpg",
                Some(Path::new("C:/archive/2026-07-24"))
            ),
            Some(("NY".to_string(), "2026-07-24".to_string()))
        );
        assert_eq!(
            archive_identity(
                "LA_20260723_LA20260723A01.jpg",
                Some(Path::new("C:/archive"))
            ),
            Some(("LA".to_string(), "2026-07-23".to_string()))
        );
    }

    #[test]
    fn duplicate_batch_request_skips_existing_job_instead_of_failing() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let destination = directory.path().join("papers");

        let first = create_batch(&mut connection, request(&destination, "2026-07-24")).unwrap();
        let second = create_batch(&mut connection, request(&destination, "2026-07-24")).unwrap();

        assert_eq!(first.jobs.len(), 1);
        assert!(second.jobs.is_empty());
        assert_eq!(second.skipped_count, 1);
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM newspaper_jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn queue_controls_are_persisted_and_removal_keeps_a_history_event() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let destination = directory.path().join("papers");
        let first = create_batch(&mut connection, request(&destination, "2026-07-21"))
            .unwrap()
            .jobs
            .remove(0);
        let second = create_batch(&mut connection, request(&destination, "2026-07-22"))
            .unwrap()
            .jobs
            .remove(0);
        let third = create_batch(&mut connection, request(&destination, "2026-07-23"))
            .unwrap()
            .jobs
            .remove(0);

        let reordered = vec![third.id.clone(), first.id.clone(), second.id.clone()];
        reorder_jobs(&mut connection, &reordered, 100).unwrap();
        let persisted_order = connection
            .prepare("SELECT id FROM newspaper_jobs ORDER BY queue_position")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(persisted_order, reordered);

        assert_eq!(
            set_job_pause(&connection, &first.id, true, 101).unwrap(),
            "queued"
        );
        let paused: bool = connection
            .query_row(
                "SELECT paused FROM newspaper_jobs WHERE id = ?1",
                params![first.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(paused);

        let (_, previous_status) = dismiss_job(&mut connection, &second.id, 102).unwrap();
        assert_eq!(previous_status, "queued");
        let dismissed: (String, bool) = connection
            .query_row(
                "SELECT status, dismissed FROM newspaper_jobs WHERE id = ?1",
                params![second.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(dismissed, ("cancelled".to_string(), true));
        let event_message: String = connection
            .query_row(
                "SELECT message FROM newspaper_events
                 WHERE job_id = ?1 AND event_type = 'queue.dismissed'",
                params![second.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(event_message.contains("left on disk"));
    }

    #[test]
    fn last_seven_days_batch_creates_all_seven_daily_jobs() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let destination = directory.path().join("papers");
        let mut batch_request = request(&destination, "2000-01-01");
        batch_request.date_mode = DateMode::Last7Days;

        let response = create_batch(&mut connection, batch_request).unwrap();
        let today = Local::now().date_naive();
        let expected_start = (today - chrono::Duration::days(6)).to_string();
        let expected_end = today.to_string();
        let dates = response
            .jobs
            .iter()
            .map(|job| job.publication_date.clone())
            .collect::<Vec<_>>();

        assert_eq!(response.jobs.len(), 7);
        assert_eq!(dates.first(), Some(&expected_start));
        assert_eq!(dates.last(), Some(&expected_end));
        assert_eq!(response.skipped_count, 0);
    }

    #[test]
    fn release_retry_is_persisted_for_thirty_minutes() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let today = Local::now().date_naive().to_string();
        let mut job = create_batch(
            &mut connection,
            request(&directory.path().join("papers"), &today),
        )
        .unwrap()
        .jobs
        .remove(0);
        let before = Utc::now().timestamp();

        schedule_release_retry(&db_path, &mut job, "Not released.").unwrap();

        let persisted: (String, i64, i64) = connection
            .query_row(
                "SELECT status, retry_at, retry_count FROM newspaper_jobs WHERE id = ?1",
                params![job.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(persisted.0, "queued");
        assert!((before + 1_800..=before + 1_802).contains(&persisted.1));
        assert_eq!(persisted.2, 1);
        assert_eq!(job.status, "awaiting_release");
    }

    #[test]
    fn reading_progress_resumes_last_page_without_regressing_furthest_page() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let job = create_batch(
            &mut connection,
            request(&directory.path().join("papers"), "2026-07-24"),
        )
        .unwrap()
        .jobs
        .remove(0);
        for index in 0..3 {
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                     (id, job_id, page_number, source_url, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'test://page', 'completed', 1, 1)",
                    params![
                        format!("reading-page-{index}"),
                        job.id,
                        format!("A0{}", index + 1)
                    ],
                )
                .unwrap();
        }

        let forward =
            reader_service::save_progress(&connection, &job.id, "reading-page-2", 10).unwrap();
        assert_eq!(forward.last_page_index, 2);
        assert_eq!(forward.furthest_page_index, 2);

        let backward =
            reader_service::save_progress(&connection, &job.id, "reading-page-0", 11).unwrap();
        assert_eq!(backward.last_page_id, "reading-page-0");
        assert_eq!(backward.last_page_index, 0);
        assert_eq!(backward.furthest_page_index, 2);
        assert_eq!(
            reader_service::list_progress(&connection).unwrap(),
            vec![backward]
        );
    }

    #[test]
    fn reader_manifest_stays_non_blocking_while_background_backfill_enriches_dimensions() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let job = create_batch(
            &mut connection,
            request(&directory.path().join("papers"), "2026-07-24"),
        )
        .unwrap()
        .jobs
        .remove(0);
        std::fs::create_dir_all(&job.output_dir).unwrap();
        let page_path = Path::new(&job.output_dir).join("A01.jpg");
        image::RgbImage::from_pixel(320, 480, image::Rgb([40, 80, 120]))
            .save_with_format(&page_path, image::ImageFormat::Jpeg)
            .unwrap();
        let bytes = std::fs::metadata(&page_path).unwrap().len();
        connection
            .execute(
                "UPDATE newspaper_jobs
                 SET status = 'completed', page_count = 1, completed_count = 1
                 WHERE id = ?1",
                params![job.id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, status,
                  original_bytes, final_bytes, checksum, created_at, updated_at)
                 VALUES ('legacy-reader-page', ?1, 'A01', 'test://page', ?2,
                         'completed', ?3, ?3, 'checksum', 1, 1)",
                params![job.id, page_path.to_string_lossy(), bytes],
            )
            .unwrap();
        drop(connection);

        let manifest = reader_service::manifest(&db_path, &job.id).unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].pixel_width, None);
        assert_eq!(manifest[0].pixel_height, None);

        let connection = Connection::open(&db_path).unwrap();
        let before: (Option<u32>, Option<u32>) = connection
            .query_row(
                "SELECT pixel_width, pixel_height FROM newspaper_pages
                 WHERE id = 'legacy-reader-page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(before, (None, None));
        drop(connection);

        let candidates = page_metadata::missing_candidates(&db_path).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(page_metadata::backfill(&db_path, &candidates).unwrap(), 1);
        let connection = Connection::open(&db_path).unwrap();
        let after: (Option<u32>, Option<u32>) = connection
            .query_row(
                "SELECT pixel_width, pixel_height FROM newspaper_pages
                 WHERE id = 'legacy-reader-page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(after, (Some(320), Some(480)));
    }

    #[test]
    fn progress_rollup_updates_before_job_finalization() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let job = create_batch(
            &mut connection,
            request(&directory.path().join("papers"), "2026-07-24"),
        )
        .unwrap()
        .jobs
        .remove(0);
        connection
            .execute(
                "UPDATE newspaper_jobs SET page_count = 3 WHERE id = ?1",
                params![job.id],
            )
            .unwrap();
        for (index, status) in ["completed", "completed", "failed"].iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                     (id, job_id, page_number, source_url, status, original_bytes,
                      final_bytes, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'test://page', ?4, 100, 80, 1, 1)",
                    params![
                        format!("page-{index}"),
                        job.id,
                        format!("A0{}", index + 1),
                        status
                    ],
                )
                .unwrap();
        }

        refresh_job_progress(&connection, &job.id).unwrap();

        let progress: (i64, i64, i64, i64) = connection
            .query_row(
                "SELECT completed_count, failed_count, original_bytes, final_bytes
                 FROM newspaper_jobs WHERE id = ?1",
                params![job.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(progress, (2, 1, 300, 240));
    }

    #[test]
    fn due_daily_schedule_materializes_only_once_per_local_date() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let connection = crate::cache::open_or_initialize(&db_path).unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_schedules
                 (id, enabled, cron_time, destination, edition_codes_json, delay_seconds,
                  optimize_images, optimization_profile, keep_original_jpg, created_at, updated_at)
                 VALUES ('schedule-1', 1, '00:00', ?1, '[\"NY\"]', 15, 1,
                         'webp_high', 0, 1, 1)",
                params![directory.path().join("papers").to_string_lossy()],
            )
            .unwrap();
        drop(connection);

        materialize_due_schedules(&db_path).unwrap();
        materialize_due_schedules(&db_path).unwrap();

        let connection = Connection::open(&db_path).unwrap();
        let job_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM newspaper_jobs", [], |row| row.get(0))
            .unwrap();
        let last_run: String = connection
            .query_row(
                "SELECT last_run_date FROM newspaper_schedules WHERE id = 'schedule-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(job_count, 1);
        assert_eq!(last_run, Local::now().date_naive().to_string());
    }

    #[test]
    fn repair_renames_legacy_php_image_and_runs_optimization() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let destination = directory.path().join("papers");
        let job = create_batch(&mut connection, request(&destination, "2026-07-24"))
            .unwrap()
            .jobs
            .remove(0);
        std::fs::create_dir_all(&job.output_dir).unwrap();
        let legacy_path = Path::new(&job.output_dir).join("A01.php");
        let image = image::ImageBuffer::from_fn(320, 480, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
        });
        image
            .save_with_format(&legacy_path, image::ImageFormat::Jpeg)
            .unwrap();
        let bytes = std::fs::metadata(&legacy_path).unwrap().len();
        connection
            .execute(
                "UPDATE newspaper_jobs
                 SET status = 'completed', page_count = 1, completed_count = 1
                 WHERE id = ?1",
                params![job.id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, status,
                  original_bytes, final_bytes, checksum, created_at, updated_at)
                 VALUES ('legacy-page', ?1, 'A01', 'test://page', ?2, 'completed',
                         ?3, ?3, 'checksum', 1, 1)",
                params![job.id, legacy_path.to_string_lossy(), bytes],
            )
            .unwrap();
        drop(connection);

        let result = repair_library_files(&db_path).unwrap();

        assert_eq!(result.renamed_files, 1);
        assert_eq!(result.optimized_jobs, 1);
        assert!(!legacy_path.exists());
        let connection = Connection::open(&db_path).unwrap();
        let paths: (String, Option<String>) = connection
            .query_row(
                "SELECT original_path, optimized_path FROM newspaper_pages
                 WHERE id = 'legacy-page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(paths.0.ends_with(".jpg"));
        assert!(
            Path::new(paths.1.as_deref().unwrap_or(&paths.0)).exists(),
            "the repaired display image should exist"
        );
    }

    #[test]
    fn repair_removes_only_valid_redundant_jpeg_sources() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let destination = directory.path().join("papers");
        let mut batch_request = request(&destination, "2026-07-24");
        batch_request.optimization_quality = 25;
        let job = create_batch(&mut connection, batch_request)
            .unwrap()
            .jobs
            .remove(0);
        std::fs::create_dir_all(&job.output_dir).unwrap();

        let redundant_source = Path::new(&job.output_dir).join("A01.jpg");
        let image = image::ImageBuffer::from_fn(480, 640, |x, y| {
            let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
            image::Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        });
        image
            .save_with_format(&redundant_source, image::ImageFormat::Jpeg)
            .unwrap();
        let redundant_webp = match optimize_page(&redundant_source, 25).unwrap() {
            OptimizationOutcome::Replaced { path, .. } => path,
            OptimizationOutcome::KeptOriginal { .. } => {
                panic!("test image should be smaller as WebP")
            }
        };
        let fallback_source = Path::new(&job.output_dir).join("A02.jpg");
        image
            .save_with_format(&fallback_source, image::ImageFormat::Jpeg)
            .unwrap();
        connection
            .execute(
                "UPDATE newspaper_jobs
                 SET status = 'completed', page_count = 2, completed_count = 2
                 WHERE id = ?1",
                params![job.id],
            )
            .unwrap();
        for (id, page, source, optimized) in [
            (
                "redundant-source-page",
                "A01",
                &redundant_source,
                &redundant_webp,
            ),
            (
                "fallback-source-page",
                "A02",
                &fallback_source,
                &fallback_source,
            ),
        ] {
            let original_bytes = std::fs::metadata(source).unwrap().len();
            let final_bytes = std::fs::metadata(optimized).unwrap().len();
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                     (id, job_id, page_number, source_url, original_path,
                      optimized_path, status, original_bytes, final_bytes,
                      checksum, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'test://page', ?4, ?5, 'completed',
                             ?6, ?7, 'checksum', 1, 1)",
                    params![
                        id,
                        job.id,
                        page,
                        source.to_string_lossy(),
                        optimized.to_string_lossy(),
                        original_bytes,
                        final_bytes
                    ],
                )
                .unwrap();
        }
        drop(connection);

        let result = repair_library_files(&db_path).unwrap();

        assert_eq!(result.removed_source_files, 1);
        assert!(!redundant_source.exists());
        assert!(redundant_webp.exists());
        assert!(
            fallback_source.exists(),
            "an active JPG fallback must never be removed"
        );
    }

    #[tokio::test]
    async fn optimization_queue_runs_after_download_completion_and_is_resumable() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let mut connection = crate::cache::open_or_initialize(&db_path).unwrap();
        let destination = directory.path().join("papers");
        let job = create_batch(&mut connection, request(&destination, "2026-07-24"))
            .unwrap()
            .jobs
            .remove(0);
        std::fs::create_dir_all(&job.output_dir).unwrap();
        let original_path = Path::new(&job.output_dir).join("A01.jpg");
        let image = image::ImageBuffer::from_fn(480, 640, |x, y| {
            let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
            image::Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        });
        image
            .save_with_format(&original_path, image::ImageFormat::Jpeg)
            .unwrap();
        let bytes = std::fs::metadata(&original_path).unwrap().len();
        connection
            .execute(
                "UPDATE newspaper_jobs
                 SET status = 'completed', page_count = 1, completed_count = 1
                 WHERE id = ?1",
                params![job.id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, status,
                  original_bytes, final_bytes, checksum, created_at, updated_at)
                 VALUES ('queue-page', ?1, 'A01', 'test://page', ?2, 'completed',
                         ?3, ?3, 'checksum', 1, 1)",
                params![job.id, original_path.to_string_lossy(), bytes],
            )
            .unwrap();
        drop(connection);

        let first = process_optimization_queue(&db_path).await.unwrap();
        let second = process_optimization_queue(&db_path).await.unwrap();

        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
        let connection = Connection::open(&db_path).unwrap();
        let display_path: String = connection
            .query_row(
                "SELECT optimized_path FROM newspaper_pages WHERE id = 'queue-page'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(Path::new(&display_path).exists());
    }
}
