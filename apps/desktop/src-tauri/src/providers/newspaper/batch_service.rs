//! Batch creation, listing, control, and terminal reconciliation.

use std::path::Path;

use chrono::{Local, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::{
    catalog_service,
    models::{
        expand_dates, CreateNewspaperBatchRequest, CreateNewspaperBatchResponse, DateMode,
        EditionKind, NewspaperBatch, NewspaperEdition, NewspaperJob,
    },
    naming,
};

pub(super) fn create(
    db_path: &Path,
    request: CreateNewspaperBatchRequest,
) -> Result<CreateNewspaperBatchResponse, String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    create_with_connection(&mut connection, request)
}

pub(super) fn create_with_connection(
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
    let catalog = catalog_service::list_with_connection(connection)?;
    let selected: Vec<NewspaperEdition> = catalog
        .into_iter()
        .filter(|edition| {
            request
                .edition_codes
                .iter()
                .any(|code| code == &naming::edition_key(edition))
        })
        .collect();
    if selected.is_empty() {
        return Err("Select at least one supported newspaper edition.".to_string());
    }

    let now = Utc::now().timestamp();
    let batch_id = naming::unique_id("newspaper-batch");
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
            let job_id = naming::unique_id("newspaper-job");
            let output_dir = Path::new(&request.destination)
                .join(naming::sanitize_segment(&format!(
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
                let existing: Option<(String, String, String, bool)> = transaction
                    .query_row(
                        "SELECT id, batch_id, status, dismissed FROM newspaper_jobs
                         WHERE edition_code = ?1 AND publication_date = ?2 AND output_dir = ?3",
                        params![edition.code, date_string, output_dir.to_string_lossy()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()
                    .map_err(|error| error.to_string())?;
                if let Some((
                    existing_job_id,
                    existing_batch_id,
                    existing_status,
                    existing_dismissed,
                )) = existing
                {
                    let marker_missing = !output_dir.join(".complete").is_file();
                    let should_requeue = marker_missing
                        && matches!(
                            existing_status.as_str(),
                            "completed" | "failed" | "unavailable" | "partial" | "cancelled"
                        );
                    let should_restore = existing_dismissed
                        && matches!(existing_status.as_str(), "completed" | "partial");
                    if should_requeue {
                        transaction
                            .execute(
                                "UPDATE newspaper_pages
                                 SET status = 'pending', error = NULL, updated_at = ?2
                                 WHERE job_id = ?1 AND status IN ('failed', 'cancelled')",
                                params![existing_job_id, now],
                            )
                            .map_err(|error| error.to_string())?;
                    }
                    if should_requeue || should_restore {
                        transaction
                            .execute(
                                "UPDATE newspaper_jobs
                                 SET status = CASE WHEN ?4 THEN 'queued' ELSE status END,
                                     retry_at = CASE WHEN ?4 THEN NULL ELSE retry_at END,
                                     failed_count = CASE WHEN ?4 THEN 0 ELSE failed_count END,
                                     warning = CASE WHEN ?4 THEN NULL ELSE warning END,
                                     paused = CASE WHEN ?4 THEN 0 ELSE paused END,
                                     dismissed = 0,
                                     queue_position = CASE WHEN ?4 THEN ?3 ELSE queue_position END,
                                     completed_at = CASE WHEN ?4 THEN NULL ELSE completed_at END,
                                     updated_at = ?2
                                 WHERE id = ?1",
                                params![existing_job_id, now, next_queue_position, should_requeue],
                            )
                            .map_err(|error| error.to_string())?;
                    }
                    if should_requeue {
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

pub(super) fn list(connection: &Connection) -> Result<Vec<NewspaperBatch>, String> {
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

pub(super) fn pause(db_path: &Path, batch_id: &str, paused: bool) -> Result<(), String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
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
    Ok(())
}

pub(super) fn cancel(db_path: &Path, batch_id: &str) -> Result<(), String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
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
    Ok(())
}

pub(super) fn validate_request(request: &CreateNewspaperBatchRequest) -> Result<(), String> {
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

pub(super) fn parse_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("Invalid publication date: {value}"))
}

pub(super) fn finish_if_terminal(connection: &Connection, batch_id: &str) -> Result<(), String> {
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
