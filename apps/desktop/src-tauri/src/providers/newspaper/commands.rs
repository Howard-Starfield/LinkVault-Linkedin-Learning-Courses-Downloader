//! Stable Tauri command facade for the newspaper subsystem.

use std::{
    path::Path,
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::Utc;
use tauri::{Emitter, Manager, State};

use super::{
    archive_service, batch_service, catalog_service, job_service, library_events, library_service,
    models::{
        CreateNewspaperBatchRequest, CreateNewspaperBatchResponse, CreateNewspaperScheduleRequest,
        NewspaperActivitySnapshot, NewspaperBootstrap, NewspaperEdition, NewspaperJob,
        NewspaperLibraryPage, NewspaperPage, NewspaperReadingProgress, NewspaperSchedule,
        OptimizationRunOptions, OptimizationRuntimeStatus, RepairNewspaperLibraryResult,
    },
    optimization_service, overview_service, page_metadata, queue_service, reader_service,
    schedule_service,
    thumbnails::{EnsureThumbnailResult, ThumbnailCoordinator},
};

pub use super::state::NewspaperState;

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
    overview_service::bootstrap(state.db_path())
}

#[tauri::command]
pub fn list_newspaper_catalog(
    state: State<'_, NewspaperState>,
) -> Result<Vec<NewspaperEdition>, String> {
    catalog_service::list(state.db_path())
}

#[tauri::command]
pub async fn refresh_newspaper_catalog(
    state: State<'_, NewspaperState>,
) -> Result<Vec<NewspaperEdition>, String> {
    catalog_service::refresh(state.db_path()).await
}

#[tauri::command]
pub fn create_newspaper_batch(
    state: State<'_, NewspaperState>,
    request: CreateNewspaperBatchRequest,
) -> Result<CreateNewspaperBatchResponse, String> {
    batch_service::create(state.db_path(), request)
}

#[tauri::command]
pub fn create_newspaper_schedule(
    state: State<'_, NewspaperState>,
    request: CreateNewspaperScheduleRequest,
) -> Result<NewspaperSchedule, String> {
    schedule_service::create(state.db_path(), request)
}

#[tauri::command]
pub fn toggle_newspaper_schedule(
    state: State<'_, NewspaperState>,
    schedule_id: String,
    enabled: bool,
) -> Result<(), String> {
    schedule_service::toggle(state.db_path(), &schedule_id, enabled)
}

#[tauri::command]
pub fn delete_newspaper_schedule(
    state: State<'_, NewspaperState>,
    schedule_id: String,
) -> Result<(), String> {
    schedule_service::delete(state.db_path(), &schedule_id)
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
    let result = match schedule_service::materialize_due(state.db_path()) {
        Ok(()) => queue_service::process_queue(state.db_path(), &state.cancelled).await,
        Err(error) => Err(error),
    };
    state.running.store(false, Ordering::SeqCst);
    if let Ok(jobs) = &result {
        library_events::emit(&app, &state, jobs);
    }
    result
}

#[tauri::command]
pub async fn process_newspaper_optimization_queue(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    options: Option<OptimizationRunOptions>,
) -> Result<Vec<NewspaperJob>, String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    state.cancelled.store(false, Ordering::SeqCst);
    let last_emit = Arc::new(Mutex::new(
        Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now),
    ));
    let progress_app = app.clone();
    let reporter = Arc::new(move |runtime: OptimizationRuntimeStatus| {
        let newspaper_state = progress_app.state::<NewspaperState>();
        newspaper_state.set_optimization_runtime(runtime.clone());
        let mut emitted_at = last_emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if emitted_at.elapsed() >= Duration::from_millis(200) || runtime.active_workers == 0 {
            *emitted_at = Instant::now();
            let revision = newspaper_state.invalidate_progress();
            let _ = progress_app.emit(
                "newspaper://optimization-progress",
                serde_json::json!({ "revision": revision, "runtime": runtime }),
            );
        }
    });
    let result = optimization_service::process_queue_with_options(
        state.db_path(),
        options.unwrap_or_default(),
        state.cancelled.clone(),
        reporter,
    )
    .await;
    state.set_optimization_runtime(OptimizationRuntimeStatus::default());
    let progress_revision = state.invalidate_progress();
    let _ = app.emit(
        "newspaper://optimization-progress",
        serde_json::json!({ "revision": progress_revision, "runtime": state.optimization_runtime() }),
    );
    state.running.store(false, Ordering::SeqCst);
    if let Ok(jobs) = &result {
        library_events::emit(&app, &state, jobs);
    }
    result
}

#[tauri::command]
pub fn pause_newspaper_batch(
    state: State<'_, NewspaperState>,
    batch_id: String,
    paused: bool,
) -> Result<(), String> {
    batch_service::pause(state.db_path(), &batch_id, paused)?;
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
    batch_service::cancel(state.db_path(), &batch_id)?;
    state.cancelled.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn retry_newspaper_job(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<usize, String> {
    job_service::retry(state.db_path(), &job_id)
}

#[tauri::command]
pub fn set_newspaper_job_pause(
    state: State<'_, NewspaperState>,
    job_id: String,
    paused: bool,
) -> Result<(), String> {
    let status = job_service::set_pause_for_job(state.db_path(), &job_id, paused)?;
    if paused && matches!(status.as_str(), "active" | "optimizing") {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub fn reorder_newspaper_jobs(
    state: State<'_, NewspaperState>,
    job_ids: Vec<String>,
) -> Result<(), String> {
    job_service::reorder_for_jobs(state.db_path(), &job_ids)
}

#[tauri::command]
pub fn remove_newspaper_job(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<(), String> {
    let status = job_service::delete(state.db_path(), &job_id)?;
    if matches!(status.as_str(), "active" | "optimizing") {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub fn list_newspaper_library(
    state: State<'_, NewspaperState>,
    query: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<NewspaperJob>, String> {
    library_service::list_legacy(state.db_path(), query, offset, limit)
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
    library_service::validate_query(&query, &kind, &status, limit)?;
    let db_path = state.db_path.clone();
    let revision = state.library_revision();
    tauri::async_runtime::spawn_blocking(move || {
        library_service::query_page(&db_path, &query, &kind, &status, offset, limit, revision)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn get_newspaper_activity_snapshot(
    state: State<'_, NewspaperState>,
) -> Result<NewspaperActivitySnapshot, String> {
    let db_path = state.db_path.clone();
    let revision = state.progress_revision();
    let runtime = state.optimization_runtime();
    tauri::async_runtime::spawn_blocking(move || {
        overview_service::activity(&db_path, revision, runtime)
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
    let connection =
        crate::cache::open_runtime(state.db_path()).map_err(|error| error.to_string())?;
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
    let result = tauri::async_runtime::spawn_blocking(move || {
        archive_service::import(&db_path, Path::new(&path))
    })
    .await
    .map_err(|error| error.to_string())?;
    if result.is_ok() {
        library_events::after_archive_change(&app, &state)?;
    }
    result
}

#[tauri::command]
pub async fn repair_newspaper_library(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
) -> Result<RepairNewspaperLibraryResult, String> {
    let db_path = state.db_path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || archive_service::repair(&db_path))
        .await
        .map_err(|error| error.to_string())?;
    if result.is_ok() {
        library_events::after_archive_change(&app, &state)?;
    }
    result
}
