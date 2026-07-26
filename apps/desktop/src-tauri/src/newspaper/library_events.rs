//! Tauri-facing Library invalidation and thumbnail prewarming coordination.

use std::path::Path;

use rusqlite::{params, Connection};
use tauri::{Emitter, Manager};

use super::{
    models::NewspaperJob, page_metadata, state::NewspaperState, thumbnails::ThumbnailCoordinator,
};

pub(super) fn emit(app: &tauri::AppHandle, state: &NewspaperState, jobs: &[NewspaperJob]) {
    if jobs.is_empty() {
        return;
    }
    let job_ids = jobs.iter().map(|job| job.id.clone()).collect::<Vec<_>>();
    let revision = state.invalidate_library();
    let _ = app.emit(
        "newspaper://library-invalidated",
        serde_json::json!({ "revision": revision, "jobIds": job_ids.clone() }),
    );
    prewarm(app, job_ids);
}

pub(super) fn after_archive_change(
    app: &tauri::AppHandle,
    state: &NewspaperState,
) -> Result<(), String> {
    let candidates = thumbnail_candidates(&state.db_path, 14)?;
    let revision = state.invalidate_library();
    let _ = app.emit(
        "newspaper://library-invalidated",
        serde_json::json!({ "revision": revision, "jobIds": candidates.clone() }),
    );
    prewarm(app, candidates);
    page_metadata::schedule(
        state.db_path.clone(),
        state.dimension_backfill_running.clone(),
    );
    Ok(())
}

fn prewarm(app: &tauri::AppHandle, job_ids: Vec<String>) {
    for job_id in job_ids.into_iter().take(14) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let coordinator = app.state::<ThumbnailCoordinator>();
            let _ = coordinator.ensure(job_id).await;
        });
    }
}

fn thumbnail_candidates(db_path: &Path, limit: u32) -> Result<Vec<String>, String> {
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
    let result = statement
        .query_map(params![limit], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}
