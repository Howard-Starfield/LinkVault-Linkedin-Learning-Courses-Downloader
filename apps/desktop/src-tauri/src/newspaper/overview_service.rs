//! Cross-domain bootstrap and activity snapshot read models.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use super::{
    batch_service, catalog_service, job_repository,
    models::{NewspaperActivitySnapshot, NewspaperBootstrap},
    reader_service, schedule_service, storage,
};

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

pub(super) fn activity(db_path: &Path, revision: u64) -> Result<NewspaperActivitySnapshot, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let jobs = job_repository::list(&connection, None)?;
    let has_live_activity = jobs.iter().any(|job| {
        matches!(job.status.as_str(), "queued" | "active" | "optimizing") || job.retry_at.is_some()
    });
    Ok(NewspaperActivitySnapshot {
        jobs,
        batches: batch_service::list(&connection)?,
        schedules: schedule_service::list(&connection)?,
        has_live_activity,
        revision,
    })
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
