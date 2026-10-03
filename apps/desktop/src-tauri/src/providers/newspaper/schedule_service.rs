//! Recurring schedule persistence and bounded, atomic catch-up reconciliation.

use super::{
    catalog_service,
    models::{
        CreateNewspaperScheduleRequest, DateMode, EditionKind, NewspaperEdition, NewspaperSchedule,
    },
    naming,
    projection::NewspaperWorkflowRequest,
};
use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriteError, DatabaseWriter};
use crate::workflow::domain::types::{NewWorkflowRun, NewWorkflowStep, StepType, WorkflowType};
use crate::workflow::infrastructure::sqlite_repository::SqliteWorkflowRepository;
use chrono::{Days, Local, NaiveTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
enum ScheduleError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Validation(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct PublicationKey {
    edition_code: String,
    publication_date: String,
}

#[derive(Default, Serialize, Deserialize)]
struct ReconciliationCheckpoint {
    initiated: Vec<PublicationKey>,
}

struct Candidate {
    edition: NewspaperEdition,
    date: String,
    completed_marker: bool,
}
struct SchedulePlan {
    schedule_id: String,
    configuration: String,
    candidates: Vec<Candidate>,
    first_date: String,
    last_date: String,
}

fn checkpoint_key(schedule_id: &str) -> String {
    format!("schedule-reconciliation:{schedule_id}")
}

fn write<T: Send + 'static>(
    writer: &DatabaseWriter,
    operation: &'static str,
    work: impl FnOnce(&mut Connection) -> Result<T, ScheduleError> + Send + 'static,
) -> Result<T, String> {
    writer
        .execute(
            DatabaseWriteContext {
                operation,
                provider: DatabaseProvider::Newspaper,
                workflow_id: None,
            },
            move |connection| match work(connection) {
                Ok(value) => Ok(Ok(value)),
                Err(ScheduleError::Sqlite(error)) => Err(DatabaseWriteError::Sqlite(error)),
                Err(error) => Ok(Err(error.to_string())),
            },
        )
        .map_err(|error| error.to_string())?
}

pub(super) fn create(
    writer: &DatabaseWriter,
    request: CreateNewspaperScheduleRequest,
) -> Result<NewspaperSchedule, String> {
    validate_request(&request)?;
    write(writer, "create_newspaper_schedule", move |connection| {
        let catalog =
            catalog_service::list_with_connection(connection).map_err(ScheduleError::Validation)?;
        if !request.edition_codes.iter().any(|selected| {
            catalog
                .iter()
                .any(|edition| naming::edition_key(edition) == *selected)
        }) {
            return Err(ScheduleError::Validation(
                "Select at least one supported newspaper edition.".to_string(),
            ));
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
        connection.execute("INSERT INTO newspaper_schedules
            (id, enabled, cron_time, destination, edition_codes_json, date_mode, delay_seconds,
             optimize_images, optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at)
            VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)", params![
            schedule.id, schedule.cron_time, schedule.destination, serde_json::to_string(&schedule.edition_codes)?,
            schedule.date_mode.as_str(), schedule.delay_seconds, schedule.optimize_images,
            schedule.optimization_profile, schedule.optimization_quality, schedule.keep_original_jpg, now,
        ])?;
        Ok(schedule)
    })
}

pub(super) fn toggle(
    writer: &DatabaseWriter,
    schedule_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let schedule_id = schedule_id.to_string();
    write(writer, "toggle_newspaper_schedule", move |connection| {
        connection.execute(
            "UPDATE newspaper_schedules SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![schedule_id, enabled, Utc::now().timestamp()],
        )?;
        Ok(())
    })
}

pub(super) fn delete(writer: &DatabaseWriter, schedule_id: &str) -> Result<bool, String> {
    let schedule_id = schedule_id.to_string();
    write(writer, "delete_newspaper_schedule", move |connection| {
        let now = Utc::now().timestamp();
        let tx = connection.transaction()?;
        let batch_ids: HashSet<String> = {
            let mut statement =
                tx.prepare("SELECT id FROM newspaper_batches WHERE schedule_id = ?1")?;
            let rows = statement.query_map([&schedule_id], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut interrupted = tx.query_row("SELECT EXISTS(SELECT 1 FROM newspaper_jobs j JOIN newspaper_batches b ON b.id = j.batch_id WHERE b.schedule_id = ?1 AND j.status IN ('active', 'optimizing'))", [&schedule_id], |row| row.get::<_, bool>(0))?;
        let runs = {
            let mut statement = tx.prepare("SELECT id, request_json, state FROM workflow_runs WHERE workflow_type = 'newspaper_download'")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, json, state) in runs {
            let request: NewspaperWorkflowRequest = serde_json::from_str(&json)?;
            if !batch_ids.contains(&request.batch_id)
                || !matches!(
                    state.as_str(),
                    "queued" | "retry_wait" | "paused" | "running" | "cancelling"
                )
            {
                continue;
            }
            let running = matches!(state.as_str(), "running" | "cancelling");
            interrupted |= running;
            let next_state = if running { "cancelling" } else { "cancelled" };
            tx.execute("UPDATE workflow_runs SET state = ?3, error_message = 'Daily schedule removed. Automatic retry stopped.', updated_at = ?2, completed_at = CASE WHEN ?4 THEN NULL ELSE ?2 END WHERE id = ?1", params![id, now, next_state, running])?;
            if !running {
                tx.execute("UPDATE workflow_steps SET state = 'cancelled', error_message = 'Daily schedule removed. Automatic retry stopped.', updated_at = ?2 WHERE run_id = ?1 AND state IN ('pending', 'ready', 'retry_wait', 'running')", params![id, now])?;
            }
            tx.execute("INSERT INTO workflow_events (run_id, step_id, sequence, event_type, payload_json, created_at) SELECT ?1, NULL, COALESCE(MAX(sequence), 0) + 1, ?3, '{}', ?2 FROM workflow_events WHERE run_id = ?1", params![id, now, if running { "run_cancelling" } else { "run_cancelled" }])?;
        }
        tx.execute("UPDATE newspaper_jobs SET status = 'cancelled', retry_at = NULL, warning = 'Daily schedule removed. Automatic retry stopped.', updated_at = ?2 WHERE batch_id IN (SELECT id FROM newspaper_batches WHERE schedule_id = ?1) AND status IN ('queued', 'active', 'optimizing')", params![schedule_id, now])?;
        tx.execute("UPDATE newspaper_batches SET status = 'cancelled', scheduled_at = NULL, completed_at = ?2, updated_at = ?2 WHERE schedule_id = ?1 AND status IN ('queued', 'scheduled', 'active', 'paused')", params![schedule_id, now])?;
        tx.execute(
            "DELETE FROM newspaper_schedules WHERE id = ?1",
            [&schedule_id],
        )?;
        tx.execute(
            "DELETE FROM newspaper_settings WHERE key = ?1",
            [checkpoint_key(&schedule_id)],
        )?;
        tx.commit()?;
        Ok(interrupted)
    })
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
            "Daily schedules support Single date or Last 7 days. Use Download for a custom range."
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

pub(super) fn materialize_due(writer: &DatabaseWriter, db_path: &Path) -> Result<usize, String> {
    materialize_due_at(writer, db_path, Utc::now().timestamp())
}

pub(super) fn materialize_due_at(
    writer: &DatabaseWriter,
    db_path: &Path,
    now: i64,
) -> Result<usize, String> {
    let local = Local
        .timestamp_opt(now, 0)
        .single()
        .ok_or_else(|| "Invalid newspaper schedule timestamp.".to_string())?;
    let today = local.date_naive();
    let first = today
        .checked_sub_days(Days::new(6))
        .ok_or_else(|| "Newspaper schedule date is out of range.".to_string())?;
    // Preflight only the bounded candidate paths. No filesystem or runtime
    // calls occur on the writer thread or while its transaction is held.
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let schedules = list(&connection)?;
    if !schedules.iter().any(|schedule| schedule.enabled) {
        return Ok(0);
    }
    let catalog = catalog_service::list_with_connection(&connection)?;
    drop(connection);
    let mut plans = Vec::new();
    let mut completed_publications = HashSet::new();
    for schedule in schedules {
        // Disabled schedules still identify known vault roots. Inventory their
        // bounded markers without interpreting a cron that cannot run.
        let last = if schedule.enabled {
            let cron = NaiveTime::parse_from_str(&schedule.cron_time, "%H:%M")
                .map_err(|_| "Invalid persisted newspaper schedule time.".to_string())?;
            if local.time() >= cron {
                today
            } else {
                today
                    .pred_opt()
                    .ok_or_else(|| "Newspaper schedule date is out of range.".to_string())?
            }
        } else {
            today
        };
        let mut candidates = Vec::new();
        for edition in catalog.iter().filter(|edition| {
            schedule
                .edition_codes
                .iter()
                .any(|key| *key == naming::edition_key(edition))
        }) {
            for offset in 0..=6 {
                let date = first
                    .checked_add_days(Days::new(offset))
                    .ok_or_else(|| "Newspaper schedule date is out of range.".to_string())?;
                if !edition.schedule.accepts(date)
                    || (edition.kind == EditionKind::Special
                        && edition.publication_date != Some(date))
                {
                    continue;
                }
                let eligible = date <= last;
                let date = date.to_string();
                let marker = candidate_output_dir(&schedule, edition, &date).join(".complete");
                let completed_marker = match std::fs::metadata(&marker) {
                    Ok(metadata) => metadata.is_file(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => {
                        return Err(format!(
                            "Could not inspect newspaper completion marker: {error}"
                        ))
                    }
                };
                if completed_marker {
                    completed_publications.insert(PublicationKey {
                        edition_code: edition.code.clone(),
                        publication_date: date.clone(),
                    });
                }
                if !schedule.enabled || !eligible {
                    continue;
                }
                candidates.push(Candidate {
                    edition: edition.clone(),
                    date,
                    completed_marker,
                });
            }
        }
        if !schedule.enabled {
            continue;
        }
        let configuration = schedule_configuration(&schedule).map_err(|error| error.to_string())?;
        let plan = SchedulePlan {
            schedule_id: schedule.id,
            configuration,
            candidates,
            first_date: first.to_string(),
            last_date: last.to_string(),
        };
        plans.push(plan);
    }
    let mut total = 0;
    for mut plan in plans {
        for candidate in &mut plan.candidates {
            candidate.completed_marker |= completed_publications.contains(&PublicationKey {
                edition_code: candidate.edition.code.clone(),
                publication_date: candidate.date.clone(),
            });
        }
        total += write(writer, "reconcile_newspaper_schedule", move |connection| {
            reconcile(connection, plan, now)
        })?;
    }
    Ok(total)
}

fn schedule_configuration(schedule: &NewspaperSchedule) -> Result<String, serde_json::Error> {
    serde_json::to_string(&(
        &schedule.cron_time,
        &schedule.destination,
        &schedule.edition_codes,
        schedule.date_mode,
        schedule.delay_seconds,
        schedule.optimize_images,
        &schedule.optimization_profile,
        schedule.optimization_quality,
        schedule.keep_original_jpg,
    ))
}

fn candidate_output_dir(
    schedule: &NewspaperSchedule,
    edition: &NewspaperEdition,
    date: &str,
) -> PathBuf {
    Path::new(&schedule.destination)
        .join(naming::sanitize_segment(&format!(
            "{} - {}",
            edition.name_zh, edition.code
        )))
        .join(date)
}

fn reconcile(
    connection: &mut Connection,
    plan: SchedulePlan,
    now: i64,
) -> Result<usize, ScheduleError> {
    let tx = connection.transaction()?;
    // Recheck enable/delete and configuration under the writer serialization.
    let Some(schedule) = list(&tx)
        .map_err(ScheduleError::Validation)?
        .into_iter()
        .find(|schedule| schedule.id == plan.schedule_id && schedule.enabled)
    else {
        return Ok(0);
    };
    if schedule_configuration(&schedule)? != plan.configuration {
        return Err(ScheduleError::Validation(
            "Newspaper schedule changed during reconciliation; retry the updated schedule."
                .to_string(),
        ));
    }
    let checkpoint_json: Option<String> = tx
        .query_row(
            "SELECT value_json FROM newspaper_settings WHERE key = ?1",
            [checkpoint_key(&schedule.id)],
            |row| row.get(0),
        )
        .optional()?;
    let mut checkpoint: ReconciliationCheckpoint = checkpoint_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_default();
    let mut initiated: HashSet<PublicationKey> = checkpoint
        .initiated
        .drain(..)
        .filter(|key| {
            key.publication_date >= plan.first_date && key.publication_date <= plan.last_date
        })
        .collect();
    {
        let mut statement = tx.prepare("SELECT edition_code, publication_date FROM newspaper_jobs WHERE publication_date >= ?1 AND publication_date <= ?2")?;
        let keys = statement.query_map(params![plan.first_date, plan.last_date], |row| {
            Ok(PublicationKey {
                edition_code: row.get(0)?,
                publication_date: row.get(1)?,
            })
        })?;
        for key in keys {
            initiated.insert(key?);
        }
    }
    let mut workflow_max_position = 0;
    {
        let mut statement = tx.prepare(
            "SELECT request_json FROM workflow_runs WHERE workflow_type = 'newspaper_download'",
        )?;
        let json_rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for json in json_rows {
            let request: NewspaperWorkflowRequest = serde_json::from_str(&json?)?;
            workflow_max_position = workflow_max_position.max(request.queue_position);
            if request.publication_date >= plan.first_date
                && request.publication_date <= plan.last_date
            {
                initiated.insert(PublicationKey {
                    edition_code: request.edition_code,
                    publication_date: request.publication_date,
                });
            }
        }
    }
    let mut next_position = tx
        .query_row(
            "SELECT COALESCE(MAX(queue_position), 0) + 1 FROM newspaper_jobs",
            [],
            |row| row.get::<_, i64>(0),
        )?
        .max(workflow_max_position.saturating_add(1));
    let batch_id = naming::unique_id("newspaper-batch");
    let mut created = 0_usize;
    let mut candidate_keys = HashSet::new();
    for candidate in plan.candidates {
        if !schedule
            .edition_codes
            .iter()
            .any(|key| *key == naming::edition_key(&candidate.edition))
        {
            continue;
        }
        let key = PublicationKey {
            edition_code: candidate.edition.code.clone(),
            publication_date: candidate.date.clone(),
        };
        candidate_keys.insert(key.clone());
        if candidate.completed_marker || initiated.contains(&key) {
            initiated.insert(key);
            continue;
        }
        if created == 0 {
            tx.execute("INSERT INTO newspaper_batches (id, schedule_id, status, destination, scheduled_at, delay_minutes, delay_seconds, optimize_images, optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at) VALUES (?1, ?2, 'queued', ?3, NULL, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?9)", params![batch_id, schedule.id, schedule.destination, schedule.delay_seconds, schedule.optimize_images, schedule.optimization_profile, schedule.optimization_quality, schedule.keep_original_jpg, now])?;
        }
        let id = naming::unique_id("newspaper-job");
        let request = NewspaperWorkflowRequest {
            schema_version: 1,
            batch_id: batch_id.clone(),
            edition_code: candidate.edition.code.clone(),
            edition_name: candidate.edition.name_zh.clone(),
            edition_publication_date: candidate
                .edition
                .publication_date
                .map(|date| date.to_string())
                .unwrap_or_default(),
            publication_date: candidate.date.clone(),
            queue_position: next_position,
            delay_seconds: schedule.delay_seconds,
            scheduled_at: None,
            optimize_images: schedule.optimize_images,
        };
        let ready_at =
            if created == 0 || schedule.delay_seconds == 0 {
                None
            } else {
                Some(now.saturating_add(
                    (created as i64).saturating_mul(i64::from(schedule.delay_seconds)),
                ))
            };
        SqliteWorkflowRepository.insert_run_with_steps_and_event_in_transaction(
            &tx,
            &NewWorkflowRun {
                id: id.clone(),
                workflow_type: WorkflowType::newspaper_download(),
                provider: "newspaper".to_string(),
                legacy_origin: None,
                legacy_id: None,
                request_json: serde_json::to_string(&request)?,
                output_root: candidate_output_dir(&schedule, &candidate.edition, &candidate.date)
                    .to_string_lossy()
                    .into_owned(),
                created_at: now,
                ready_at,
            },
            &[NewWorkflowStep {
                id: format!("{id}-execute"),
                step_key: candidate.edition.code,
                step_type: StepType::newspaper_execute(),
                created_at: now,
            }],
            "submitted",
            "{}",
        )?;
        initiated.insert(key);
        next_position = next_position.saturating_add(1);
        created += 1;
    }
    // Retain only this schedule's bounded candidate identities; explicit removal
    // of an observed run/job must not turn into an automatic retry next startup.
    checkpoint.initiated = initiated
        .into_iter()
        .filter(|key| candidate_keys.contains(key))
        .collect();
    checkpoint.initiated.sort_by(|left, right| {
        (&left.edition_code, &left.publication_date)
            .cmp(&(&right.edition_code, &right.publication_date))
    });
    let checkpoint_value = serde_json::to_string(&checkpoint)?;
    if checkpoint_json.as_deref() != Some(checkpoint_value.as_str()) {
        tx.execute("INSERT INTO newspaper_settings (key, value_json, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at", params![checkpoint_key(&schedule.id), checkpoint_value, now])?;
    }
    tx.execute("UPDATE newspaper_schedules SET last_run_date = ?2, last_error = NULL, updated_at = ?3 WHERE id = ?1 AND (last_run_date IS NOT ?2 OR last_error IS NOT NULL)", params![schedule.id, plan.last_date, now])?;
    tx.commit()?;
    Ok(created)
}

pub(super) fn next_due_at(db_path: &Path, now: i64) -> Result<Option<i64>, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let local = Local
        .timestamp_opt(now, 0)
        .single()
        .ok_or_else(|| "Invalid newspaper schedule timestamp.".to_string())?;
    let mut next = None;
    for schedule in list(&connection)?
        .into_iter()
        .filter(|schedule| schedule.enabled)
    {
        let cron = NaiveTime::parse_from_str(&schedule.cron_time, "%H:%M")
            .map_err(|_| "Invalid persisted newspaper schedule time.".to_string())?;
        for day in 0..=1 {
            let date = local
                .date_naive()
                .checked_add_days(Days::new(day))
                .ok_or_else(|| "Newspaper schedule date is out of range.".to_string())?;
            // On the spring-forward gap, admit the first valid local minute
            // after the requested time. On fall-back use the first occurrence.
            for minutes in 0..=120 {
                let naive = date.and_time(cron) + chrono::Duration::minutes(minutes);
                if let Some(due) = Local.from_local_datetime(&naive).earliest() {
                    let due = due.timestamp();
                    if due > now {
                        next = Some(next.map_or(due, |previous: i64| previous.min(due)));
                    }
                    break;
                }
            }
        }
    }
    Ok(next)
}

#[cfg(test)]
#[path = "schedule_service/tests.rs"]
mod tests;
