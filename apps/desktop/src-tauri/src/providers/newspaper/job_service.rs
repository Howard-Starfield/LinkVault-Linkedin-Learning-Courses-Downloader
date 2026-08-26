//! Job control mutations for retry, pause, reorder, and deletion.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::{batch_service, clipping_repository, storage};

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

/// Result of a bulk pause/resume mutation across the visible queue.
///
/// `triggered_cancel` is true when at least one job that was `active` or
/// `optimizing` had its `paused` flag flipped to `true`. The caller flips the
/// shared cooperative `cancelled` flag so the in-flight worker loop unwinds
/// at the next safe boundary (between jobs, between pages, between HTTP
/// requests, or after a failed page).
pub struct BulkPauseOutcome {
    pub updated: Vec<String>,
    pub triggered_cancel: bool,
}

pub(super) fn set_all_paused(
    connection: &mut Connection,
    paused: bool,
    updated_at: i64,
) -> Result<BulkPauseOutcome, String> {
    let mut targets: Vec<(String, String)> = connection
        .prepare(
            "SELECT id, status FROM newspaper_jobs
             WHERE status IN ('active', 'queued', 'optimizing')
               AND paused != ?1
               AND dismissed = 0",
        )
        .map_err(|error| error.to_string())?
        .query_map(params![paused], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    targets.sort_by(|left, right| left.0.cmp(&right.0));
    let updated: Vec<String> = targets.iter().map(|(id, _)| id.clone()).collect();
    if updated.is_empty() {
        return Ok(BulkPauseOutcome {
            updated,
            triggered_cancel: false,
        });
    }
    let triggered_cancel = paused
        && targets
            .iter()
            .any(|(_, status)| matches!(status.as_str(), "active" | "optimizing"));
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    for job_id in &updated {
        transaction
            .execute(
                "UPDATE newspaper_jobs
                 SET paused = ?1,
                     status = CASE WHEN ?1 = 1 AND status = 'active' THEN 'queued' ELSE status END,
                     updated_at = ?2
                 WHERE id = ?3",
                params![paused, updated_at, job_id],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(BulkPauseOutcome {
        updated,
        triggered_cancel,
    })
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
    // Promoting a job to the front (or any reorder) clears a future batch
    // schedule so scheduled rows never block immediate downloads the user
    // explicitly started.
    if let Some(first_job_id) = job_ids.first() {
        transaction
            .execute(
                "UPDATE newspaper_batches
                 SET scheduled_at = NULL,
                     status = CASE WHEN status = 'scheduled' THEN 'queued' ELSE status END,
                     updated_at = ?2
                 WHERE id = (SELECT batch_id FROM newspaper_jobs WHERE id = ?1)
                   AND scheduled_at IS NOT NULL
                   AND scheduled_at > ?2",
                params![first_job_id, updated_at],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

pub(super) fn reorder_for_jobs(db_path: &Path, job_ids: &[String]) -> Result<(), String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    reorder(&mut connection, job_ids, Utc::now().timestamp())
}

pub(super) fn delete_with_connection(
    connection: &mut Connection,
    job_id: &str,
) -> Result<(String, String), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let (batch_id, status, output_dir, destination): (String, String, String, String) = transaction
        .query_row(
            "SELECT j.batch_id, j.status, j.output_dir, b.destination
             FROM newspaper_jobs j
             JOIN newspaper_batches b ON b.id = j.batch_id
             WHERE j.id = ?1",
            params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| "Newspaper queue item was not found.".to_string())?;
    if matches!(status.as_str(), "active" | "optimizing") {
        return Err("Pause the active newspaper download before deleting it.".to_string());
    }
    remove_output_directory(Path::new(&destination), Path::new(&output_dir))?;
    remove_cached_thumbnail(&transaction, job_id)?;
    clipping_repository::unlink_sources_for_job(&transaction, job_id)
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM newspaper_jobs WHERE id = ?1", params![job_id])
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE newspaper_batches SET updated_at = ?2 WHERE id = ?1",
            params![batch_id, Utc::now().timestamp()],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM newspaper_batches
             WHERE id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM newspaper_jobs WHERE batch_id = newspaper_batches.id
               )",
            params![batch_id],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok((batch_id, status))
}

fn remove_cached_thumbnail(connection: &Connection, job_id: &str) -> Result<(), String> {
    let cache_path = connection
        .query_row(
            "SELECT cache_path FROM newspaper_thumbnail_cache WHERE job_id = ?1",
            params![job_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(cache_path) = cache_path.map(PathBuf::from) else {
        return Ok(());
    };
    if !cache_path.exists() {
        return Ok(());
    }
    let database_path: String = connection
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let cache_root = Path::new(&database_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("newspaper-thumbnails");
    let resolved_cache_root = std::fs::canonicalize(&cache_root)
        .map_err(|_| "The newspaper thumbnail cache could not be verified.".to_string())?;
    let resolved_cache_path = std::fs::canonicalize(&cache_path)
        .map_err(|_| "The newspaper thumbnail file could not be verified.".to_string())?;
    if !resolved_cache_path.starts_with(&resolved_cache_root) {
        return Ok(());
    }
    std::fs::remove_file(cache_path)
        .map_err(|error| format!("Could not delete the newspaper thumbnail: {error}"))
}

fn remove_output_directory(destination: &Path, output_dir: &Path) -> Result<(), String> {
    if !output_dir.exists() {
        return Ok(());
    }
    if !output_dir.is_dir() {
        return Err("The newspaper output path is not a directory.".to_string());
    }
    let resolved_destination = std::fs::canonicalize(destination)
        .map_err(|_| "The newspaper destination could not be verified.".to_string())?;
    let resolved_output = std::fs::canonicalize(output_dir)
        .map_err(|_| "The newspaper output directory could not be verified.".to_string())?;
    if resolved_output == resolved_destination
        || !resolved_output.starts_with(&resolved_destination)
    {
        return Err(
            "Refusing to delete a newspaper folder outside its configured destination.".to_string(),
        );
    }
    let targets_snapshots = resolved_output
        .strip_prefix(&resolved_destination)
        .ok()
        .and_then(|relative| relative.components().next())
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .is_some_and(|segment| {
            segment.eq_ignore_ascii_case(super::clipping_roots::SNAPSHOT_DIRECTORY_NAME)
        });
    if targets_snapshots {
        return Err("Refusing to delete the protected Newspaper snapshots folder.".to_string());
    }
    std::fs::remove_dir_all(PathBuf::from(output_dir))
        .map_err(|error| format!("Could not delete the downloaded newspaper files: {error}"))
}

pub(super) fn delete(db_path: &Path, job_id: &str) -> Result<String, String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let (batch_id, status) = delete_with_connection(&mut connection, job_id)?;
    if connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM newspaper_batches WHERE id = ?1)",
            params![batch_id],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
    {
        batch_service::finish_if_terminal(&connection, &batch_id)?;
    }
    Ok(status)
}

/// Wipe the entire newspaper thumbnail cache directory.
///
/// Mirrors the safety pattern of `remove_cached_thumbnail`: canonicalize the
/// candidate path and verify it lives under the database's parent directory
/// before recursing. The directory may not exist yet — that is treated as
/// success. The caller is responsible for pausing active downloads first.
pub fn clear_thumbnail_cache(db_path: &Path) -> Result<(), String> {
    let cache_root = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("newspaper-thumbnails");
    if !cache_root.exists() {
        return Ok(());
    }
    let resolved_db_parent = std::fs::canonicalize(
        db_path.parent().unwrap_or_else(|| Path::new(".")),
    )
    .map_err(|error| format!("The newspaper database folder could not be verified: {error}"))?;
    let resolved_cache_root = std::fs::canonicalize(&cache_root)
        .map_err(|error| format!("The newspaper thumbnail cache could not be verified: {error}"))?;
    if !resolved_cache_root.starts_with(&resolved_db_parent) {
        return Err(
            "Refusing to delete a newspaper thumbnail cache outside the database folder."
                .to_string(),
        );
    }
    std::fs::remove_dir_all(&resolved_cache_root)
        .map_err(|error| format!("Could not delete the newspaper thumbnail cache: {error}"))
}
