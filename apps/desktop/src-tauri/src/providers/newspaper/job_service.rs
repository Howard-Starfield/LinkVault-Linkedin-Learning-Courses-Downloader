//! Job control mutations for retry, pause, reorder, and dismissal.

use std::{collections::HashSet, path::Path};

use chrono::Utc;
use rusqlite::{params, Connection};

use super::{batch_service, storage};

pub(super) fn retry(db_path: &Path, job_id: &str) -> Result<usize, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    storage::retry_missing_pages(&connection, job_id, Utc::now().timestamp())
        .map_err(|error| error.to_string())
}

pub(super) fn set_pause(
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

pub(super) fn set_pause_for_job(
    db_path: &Path,
    job_id: &str,
    paused: bool,
) -> Result<String, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    set_pause(&connection, job_id, paused, Utc::now().timestamp())
}

pub(super) fn reorder(
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

pub(super) fn reorder_for_jobs(db_path: &Path, job_ids: &[String]) -> Result<(), String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    reorder(&mut connection, job_ids, Utc::now().timestamp())
}

pub(super) fn dismiss_with_connection(
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

pub(super) fn dismiss(db_path: &Path, job_id: &str) -> Result<String, String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let now = Utc::now().timestamp();
    let (batch_id, status) = dismiss_with_connection(&mut connection, job_id, now)?;
    batch_service::finish_if_terminal(&connection, &batch_id)?;
    Ok(status)
}
