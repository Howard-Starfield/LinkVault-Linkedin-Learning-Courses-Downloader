//! Recurring schedule persistence, validation, and due materialization.

use std::path::Path;

use chrono::{Local, NaiveTime, Utc};
use rusqlite::{params, Connection};

use super::{
    batch_service, catalog_service,
    models::{
        CreateNewspaperBatchRequest, CreateNewspaperScheduleRequest, DateMode, NewspaperSchedule,
    },
    naming,
};

pub(super) fn create(
    db_path: &Path,
    request: CreateNewspaperScheduleRequest,
) -> Result<NewspaperSchedule, String> {
    validate_request(&request)?;
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let catalog = catalog_service::list_with_connection(&connection)?;
    if !request.edition_codes.iter().any(|selected| {
        catalog
            .iter()
            .any(|edition| naming::edition_key(edition) == *selected)
    }) {
        return Err("Select at least one supported newspaper edition.".to_string());
    }
    let now = Utc::now().timestamp();
    let schedule = NewspaperSchedule {
        id: naming::unique_id("newspaper-schedule"),
        enabled: true,
        cron_time: request.cron_time,
        destination: request.destination,
        edition_codes: request.edition_codes,
        date_mode: request.date_mode,
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
             (id, enabled, cron_time, destination, edition_codes_json, date_mode, delay_seconds,
              optimize_images, optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            params![
                schedule.id,
                schedule.cron_time,
                schedule.destination,
                serde_json::to_string(&schedule.edition_codes).map_err(|error| error.to_string())?,
                schedule.date_mode.as_str(),
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

pub(super) fn toggle(db_path: &Path, schedule_id: &str, enabled: bool) -> Result<(), String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE newspaper_schedules SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![schedule_id, enabled, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn delete(db_path: &Path, schedule_id: &str) -> Result<bool, String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    crate::cache::delete_newspaper_schedule_and_cancel_owned_work(
        &mut connection,
        schedule_id,
        Utc::now().timestamp(),
    )
    .map_err(|error| error.to_string())
}

pub(super) fn list(connection: &Connection) -> Result<Vec<NewspaperSchedule>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, enabled, cron_time, destination, edition_codes_json, delay_seconds,
                    date_mode, optimize_images, optimization_profile, optimization_quality,
                    keep_original_jpg, last_run_date, last_error, created_at, updated_at
             FROM newspaper_schedules ORDER BY created_at DESC",
        )
        .map_err(|error| error.to_string())?;
    let schedules = statement
        .query_map([], |row| {
            let edition_codes_json: String = row.get(4)?;
            let date_mode: String = row.get(6)?;
            Ok(NewspaperSchedule {
                id: row.get(0)?,
                enabled: row.get(1)?,
                cron_time: row.get(2)?,
                destination: row.get(3)?,
                edition_codes: serde_json::from_str(&edition_codes_json).unwrap_or_default(),
                date_mode: DateMode::from_persisted(&date_mode).unwrap_or(DateMode::Single),
                delay_seconds: row.get(5)?,
                optimize_images: row.get(7)?,
                optimization_profile: row.get(8)?,
                optimization_quality: row.get(9)?,
                keep_original_jpg: row.get(10)?,
                last_run_date: row.get(11)?,
                last_error: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(schedules)
}

pub(super) fn validate_request(request: &CreateNewspaperScheduleRequest) -> Result<(), String> {
    if request.destination.trim().is_empty() {
        return Err("Choose a newspaper download folder.".to_string());
    }
    if request.edition_codes.is_empty() {
        return Err("Select at least one newspaper edition.".to_string());
    }
    if request.date_mode == DateMode::Custom {
        return Err(
            "Daily schedules support Single date or Last 7 days. Use Download now for a custom range."
                .to_string(),
        );
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

pub(super) fn materialize_due(db_path: &Path) -> Result<(), String> {
    let now_local = Local::now();
    let today = now_local.date_naive().to_string();
    let schedules = {
        let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
        list(&connection)?
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
            date_mode: schedule.date_mode,
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
            let mut connection =
                crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
            batch_service::create_for_schedule_with_connection(
                &mut connection,
                request,
                &schedule.id,
            )
        };
        let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
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
