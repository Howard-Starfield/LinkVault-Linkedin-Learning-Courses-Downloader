use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{NaiveDate, Utc};
use image::{codecs::jpeg::JpegEncoder, GenericImageView};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use tauri::State;

use super::{
    client::{FetchError, NewspaperClient},
    downloader::{download_validated_page, validate_existing_page},
    manifest,
    models::{
        expand_dates, CreateNewspaperBatchRequest, CreateNewspaperBatchResponse, EditionKind,
        NewspaperBatch, NewspaperBootstrap, NewspaperEdition, NewspaperJob, NewspaperPage,
        PublicationSchedule,
    },
    optimizer::{optimize_page, OptimizationOutcome},
    storage,
};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct NewspaperState {
    db_path: PathBuf,
    cancelled: Arc<AtomicBool>,
    running: AtomicBool,
}

impl NewspaperState {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            cancelled: Arc::new(AtomicBool::new(false)),
            running: AtomicBool::new(false),
        }
    }
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
    validate_request(&request)?;
    let start = parse_date(&request.start_date)?;
    let end = request.end_date.as_deref().map(parse_date).transpose()?;
    let dates = expand_dates(request.date_mode, start, end).map_err(|error| error.to_string())?;
    let catalog = list_catalog_records(&connection)?;
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
    transaction
        .execute(
            "INSERT INTO newspaper_batches
            (id, status, destination, scheduled_at, delay_minutes, optimize_images,
             optimization_profile, keep_original_jpg, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                batch_id,
                batch_status,
                request.destination,
                scheduled,
                request.delay_minutes,
                request.optimize_images,
                request.optimization_profile,
                request.keep_original_jpg,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;

    let mut created = Vec::new();
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
                     status, output_dir, created_at, updated_at)
                    VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?7)",
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
                        now,
                    ],
                )
                .map_err(|error| error.to_string())?;
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
                warning: None,
                updated_at: now,
            });
        }
    }
    if created.is_empty() {
        return Err("The selected editions do not publish on the chosen dates.".to_string());
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let batch = NewspaperBatch {
        id: batch_id,
        status: batch_status.to_string(),
        destination: request.destination,
        scheduled_at: scheduled,
        delay_minutes: request.delay_minutes,
        optimize_images: request.optimize_images,
        optimization_profile: request.optimization_profile,
        keep_original_jpg: request.keep_original_jpg,
        created_at: now,
        updated_at: now,
    };
    Ok(CreateNewspaperBatchResponse {
        batch,
        jobs: created,
    })
}

#[tauri::command]
pub async fn process_newspaper_queue(
    state: State<'_, NewspaperState>,
) -> Result<Vec<NewspaperJob>, String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    state.cancelled.store(false, Ordering::SeqCst);
    let result = process_queue(&state.db_path, &state.cancelled).await;
    state.running.store(false, Ordering::SeqCst);
    result
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
                    j.failed_count, j.warning, j.updated_at
             FROM newspaper_jobs j
             JOIN newspaper_editions e ON e.code = j.edition_code
                 AND e.publication_date = j.edition_publication_date
             WHERE j.status IN ('completed', 'partial')
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
pub fn get_newspaper_reader_manifest(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<Vec<NewspaperPage>, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT id, job_id, page_number, section_name, source_url,
                    COALESCE(optimized_path, original_path), status, final_bytes, checksum, error
             FROM newspaper_pages WHERE job_id = ?1 ORDER BY page_number",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params![job_id], |row| {
            Ok(NewspaperPage {
                id: row.get(0)?,
                job_id: row.get(1)?,
                page_number: row.get(2)?,
                section_name: row.get(3)?,
                source_url: row.get(4)?,
                display_path: row.get(5)?,
                status: row.get(6)?,
                final_bytes: row.get(7)?,
                checksum: row.get(8)?,
                error: row.get(9)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

#[tauri::command]
pub fn get_newspaper_preview(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<String, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let path = page_path_for_job(&connection, &job_id)?;
    let image = image::open(&path).map_err(|error| error.to_string())?;
    let (width, height) = image.dimensions();
    let crop_height = (height * 32 / 100).max(1);
    let cropped = image.crop_imm(0, 0, width, crop_height);
    let resized = if cropped.width() > 720 {
        cropped.resize(720, u32::MAX, image::imageops::FilterType::Lanczos3)
    } else {
        cropped
    };
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 82)
        .encode_image(&resized)
        .map_err(|error| error.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)))
}

#[tauri::command]
pub fn get_newspaper_page_image(
    state: State<'_, NewspaperState>,
    page_id: String,
) -> Result<String, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let path: String = connection
        .query_row(
            "SELECT COALESCE(optimized_path, original_path)
             FROM newspaper_pages WHERE id = ?1 AND status = 'completed'",
            params![page_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    let mime = match Path::new(&path)
        .extension()
        .and_then(|value| value.to_str())
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
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
    state: State<'_, NewspaperState>,
    path: String,
) -> Result<usize, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || import_archive(&db_path, Path::new(&path)))
        .await
        .map_err(|error| error.to_string())?
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
        let Some((job, delay_minutes, scheduled_at)) = next else {
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
        if has_next_due_job && delay_minutes > 0 && !cancelled.load(Ordering::SeqCst) {
            let mut remaining = u64::from(delay_minutes) * 60;
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
                "UPDATE newspaper_jobs SET status = 'active', updated_at = ?2 WHERE id = ?1",
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
            update_job_terminal(db_path, &job.id, "unavailable", None)?;
            job.status = "unavailable".to_string();
            return Ok(job);
        }
        Err(FetchError::Cancelled) => {
            update_job_terminal(db_path, &job.id, "cancelled", None)?;
            job.status = "cancelled".to_string();
            return Ok(job);
        }
        Err(error) => {
            update_job_terminal(db_path, &job.id, "failed", Some(&error.to_string()))?;
            job.status = "failed".to_string();
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
            let extension = Path::new(source_url.path())
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("jpg");
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

    for page in pages {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let source_url = manifest::resolve_page_url_with_origin(&page.pagefile, client.origin())
            .map_err(|error| error.to_string())?;
        let extension = Path::new(source_url.path())
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("jpg");
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
        match result {
            Ok(downloaded) => {
                connection.execute(
                    "UPDATE newspaper_pages SET status = 'completed', attempts = attempts + 1,
                     original_bytes = ?3, final_bytes = ?3, checksum = ?4, error = NULL, updated_at = ?5
                     WHERE job_id = ?1 AND page_number = ?2",
                    params![
                        job.id,
                        page.pageno,
                        downloaded.size_bytes,
                        downloaded.checksum_sha256,
                        Utc::now().timestamp(),
                    ],
                ).map_err(|error| error.to_string())?;
            }
            Err(error) => {
                if cancelled.load(Ordering::SeqCst) {
                    let batch_status: String = connection
                        .query_row(
                            "SELECT status FROM newspaper_batches WHERE id = ?1",
                            params![job.batch_id],
                            |row| row.get(0),
                        )
                        .map_err(|sql_error| sql_error.to_string())?;
                    let (page_status, job_status) = if batch_status == "paused" {
                        ("pending", "queued")
                    } else {
                        ("cancelled", "cancelled")
                    };
                    connection
                        .execute(
                            "UPDATE newspaper_pages SET status = ?3, error = NULL, updated_at = ?4
                             WHERE job_id = ?1 AND page_number = ?2",
                            params![job.id, page.pageno, page_status, Utc::now().timestamp()],
                        )
                        .map_err(|sql_error| sql_error.to_string())?;
                    connection
                        .execute(
                            "UPDATE newspaper_jobs SET status = ?2, updated_at = ?3 WHERE id = ?1",
                            params![job.id, job_status, Utc::now().timestamp()],
                        )
                        .map_err(|sql_error| sql_error.to_string())?;
                    job.status = job_status.to_string();
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
    }
    if cancelled.load(Ordering::SeqCst) {
        let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
        let batch_status: String = connection
            .query_row(
                "SELECT status FROM newspaper_batches WHERE id = ?1",
                params![job.batch_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let job_status = if batch_status == "paused" {
            "queued"
        } else {
            "cancelled"
        };
        connection
            .execute(
                "UPDATE newspaper_jobs SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![job.id, job_status, Utc::now().timestamp()],
            )
            .map_err(|error| error.to_string())?;
        job.status = job_status.to_string();
        return Ok(job);
    }
    let optimization_db_path = db_path.to_path_buf();
    let optimization_job = job.clone();
    tauri::async_runtime::spawn_blocking(move || {
        optimize_completed_pages(&optimization_db_path, &optimization_job)
    })
    .await
    .map_err(|error| error.to_string())??;
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

fn optimize_completed_pages(db_path: &Path, job: &NewspaperJob) -> Result<(), String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let settings: (bool, String, bool) = connection
        .query_row(
            "SELECT optimize_images, optimization_profile, keep_original_jpg
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
                 WHERE job_id = ?1 AND status = 'completed' AND original_path IS NOT NULL",
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
        match optimize_page(Path::new(&source), &settings.1) {
            Ok(OptimizationOutcome::Replaced { path, bytes }) => {
                connection
                    .execute(
                        "UPDATE newspaper_pages SET optimized_path = ?2, final_bytes = ?3,
                         updated_at = ?4 WHERE id = ?1",
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
                        "UPDATE newspaper_pages SET final_bytes = ?2, updated_at = ?3 WHERE id = ?1",
                        params![page_id, bytes, Utc::now().timestamp()],
                    )
                    .map_err(|error| error.to_string())?;
            }
            Err(error) => warnings.push(error.to_string()),
        }
    }
    if warnings.is_empty() && !settings.2 {
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
                    j.failed_count, j.warning, j.updated_at, b.delay_minutes,
                    b.scheduled_at
             FROM newspaper_jobs j
             JOIN newspaper_batches b ON b.id = j.batch_id
             JOIN newspaper_editions e ON e.code = j.edition_code
                 AND e.publication_date = j.edition_publication_date
             WHERE j.status = 'queued' AND b.status IN ('queued', 'scheduled', 'active')
               AND (b.scheduled_at IS NULL OR b.scheduled_at <= ?1)
             ORDER BY b.created_at, j.created_at LIMIT 1",
            params![now],
            |row| Ok((row_to_job(row)?, row.get(12)?, row.get(13)?)),
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
            "SELECT id, status, destination, scheduled_at, delay_minutes, optimize_images,
                optimization_profile, keep_original_jpg, created_at, updated_at
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
                delay_minutes: row.get(4)?,
                optimize_images: row.get(5)?,
                optimization_profile: row.get(6)?,
                keep_original_jpg: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

fn list_jobs(connection: &Connection, batch_id: Option<&str>) -> Result<Vec<NewspaperJob>, String> {
    let mut statement = connection
        .prepare(
            "SELECT j.id, j.batch_id, j.edition_code, e.name_zh, j.publication_date,
                j.status, j.output_dir, j.page_count, j.completed_count,
                j.failed_count, j.warning, j.updated_at
         FROM newspaper_jobs j JOIN newspaper_editions e
           ON e.code = j.edition_code AND e.publication_date = j.edition_publication_date
         WHERE (?1 IS NULL OR j.batch_id = ?1)
         ORDER BY j.created_at DESC",
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
        warning: row.get(10)?,
        updated_at: row.get(11)?,
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

fn page_path_for_job(connection: &Connection, job_id: &str) -> Result<String, String> {
    connection
        .query_row(
            "SELECT COALESCE(optimized_path, original_path)
             FROM newspaper_pages
             WHERE job_id = ?1 AND status = 'completed'
             ORDER BY CASE WHEN page_number = 'A01' THEN 0 ELSE 1 END, page_number
             LIMIT 1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn validate_request(request: &CreateNewspaperBatchRequest) -> Result<(), String> {
    if request.destination.trim().is_empty() {
        return Err("Choose a newspaper download folder.".to_string());
    }
    if request.delay_minutes > 1_440 {
        return Err("Delay must be between 0 and 1,440 minutes.".to_string());
    }
    if !matches!(
        request.optimization_profile.as_str(),
        "webp_high" | "webp_balanced"
    ) {
        return Err("Unsupported image optimization profile.".to_string());
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
            if image::load_from_memory(&bytes).is_err() {
                continue;
            }
            let page_number = archive_page_number(&file);
            valid_pages.push((
                file,
                page_number,
                bytes.len() as u64,
                format!("{:x}", Sha256::digest(&bytes)),
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
        for (file, page_number, bytes, checksum) in valid_pages {
            transaction
                .execute(
                    "INSERT INTO newspaper_pages
                    (id, job_id, page_number, source_url, original_path, status,
                     attempts, original_bytes, final_bytes, checksum, created_at, updated_at)
                    VALUES (?1, ?2, ?3, 'archive://local', ?4, 'completed', 0, ?5, ?5, ?6, ?7, ?7)",
                    params![
                        unique_id("newspaper-import-page"),
                        job_id,
                        page_number,
                        file.to_string_lossy(),
                        bytes,
                        checksum,
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

    #[test]
    fn request_validation_rejects_invalid_delay_and_profile() {
        let mut request = CreateNewspaperBatchRequest {
            edition_codes: vec!["NY".to_string()],
            date_mode: DateMode::Single,
            start_date: "2026-07-24".to_string(),
            end_date: None,
            destination: "C:/papers".to_string(),
            scheduled_at: None,
            delay_minutes: 1_441,
            optimize_images: true,
            optimization_profile: "webp_high".to_string(),
            keep_original_jpg: false,
        };
        assert!(validate_request(&request).is_err());
        request.delay_minutes = 5;
        request.optimization_profile = "lossless".to_string();
        assert!(validate_request(&request).is_err());
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
}
