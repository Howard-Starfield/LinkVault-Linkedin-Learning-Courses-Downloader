//! Stable Tauri command facade for the newspaper subsystem.

use std::{
    path::Path,
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::Utc;
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use super::{
    archive_service, batch_service, catalog_service,
    clipping_draft_service::{
        CheckpointClippingNoteRequest, ClaimClippingNoteRecoveryRequest, ClippingNoteCheckpointAck,
        ClippingNoteRecoveryResponse, DiscardClippingNoteRecoveryRequest,
        LoadClippingNoteRecoveryRequest,
    },
    clipping_models::{
        ClippingErrorCode, ClippingRootSummary, CreateNewspaperClippingFailure,
        CreateNewspaperClippingRequest, CreateNewspaperClippingResponse,
        DeleteNewspaperClippingRequest, DeleteNewspaperClippingResponse,
        EnsureNewspaperClippingThumbnailResponse, GetNewspaperClippingsPageRequest,
        NewspaperClippingDetail, NewspaperClippingsPage, ReconnectNewspaperSnapshotRootResult,
        SearchNewspaperClippingsPage, SearchNewspaperClippingsRequest,
        SearchPossibleNewspaperClippingsRequest, SearchPossibleNewspaperClippingsResponse,
        UpdateNewspaperClippingRequest,
    },
    clipping_service::ClippingService,
    job_service, library_events, library_recovery, library_service,
    models::{
        CreateNewspaperBatchRequest, CreateNewspaperBatchResponse, CreateNewspaperScheduleRequest,
        NewspaperActivitySnapshot, NewspaperBootstrap, NewspaperEdition, NewspaperJob,
        NewspaperLibraryPage, NewspaperPage, NewspaperReadingProgress, NewspaperSchedule,
        OptimizationRunOptions, OptimizationRuntimeStatus, RecoverNewspaperLibraryResult,
        RepairNewspaperLibraryResult,
    },
    optimization_service, overview_service, page_metadata, queue_service, reader_service,
    schedule_service,
    thumbnails::{EnsureThumbnailResult, ThumbnailCoordinator},
};
use crate::cache::{clear_newspaper_provider_data, NewspaperResetCounts};
use crate::workflow::application::runtime::WorkflowRuntime;

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
    runtime: State<'_, WorkflowRuntime>,
) -> Result<NewspaperBootstrap, String> {
    overview_service::bootstrap(state.db_path(), Some(&runtime))
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
    runtime: State<'_, WorkflowRuntime>,
    request: CreateNewspaperBatchRequest,
) -> Result<CreateNewspaperBatchResponse, String> {
    batch_service::create(state.db_path(), &runtime, request)
}

/// Thin Phase 2 adapter: all source resolution, filesystem work, staging,
/// idempotency, and persistence ownership remain in `ClippingService`.
#[tauri::command]
pub async fn create_newspaper_clipping(
    app: tauri::AppHandle,
    service: State<'_, ClippingService>,
    request: CreateNewspaperClippingRequest,
) -> Result<CreateNewspaperClippingResponse, CreateNewspaperClippingFailure> {
    let operation_id = request.operation_id.clone();
    let service = service.inner().clone();
    match tauri::async_runtime::spawn_blocking(move || {
        service.create_newspaper_clipping(request, Utc::now().timestamp())
    })
    .await
    {
        Ok(Ok(response)) => {
            let _ = app.emit(
                "newspaper://clipping-invalidated",
                serde_json::json!({ "clippingId": response.clipping_id, "revision": response.revision }),
            );
            Ok(response)
        }
        Ok(Err(error)) => Err(CreateNewspaperClippingFailure::from_code(
            operation_id,
            error.code,
        )),
        Err(_) => Err(CreateNewspaperClippingFailure::from_code(
            operation_id,
            ClippingErrorCode::ServiceUnavailable,
        )),
    }
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
    if schedule_service::delete(state.db_path(), &schedule_id)? {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub async fn process_newspaper_queue(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    runtime: State<'_, WorkflowRuntime>,
) -> Result<Vec<NewspaperJob>, String> {
    if state.download_running.swap(true, Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    state.cancelled.store(false, Ordering::SeqCst);
    let db_path = state.db_path().to_path_buf();
    let cancelled = Arc::clone(&state.cancelled);
    let runtime = (*runtime).clone();
    let result = match tauri::async_runtime::spawn_blocking({
        let db_path = db_path.clone();
        let cancelled = Arc::clone(&cancelled);
        let runtime = runtime.clone();
        move || {
            schedule_service::materialize_due(&db_path, Some(&runtime))?;
            loop {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }
                let outcome = runtime
                    .drain_type("newspaper_download")
                    .map_err(|error| error.to_string())?;
                if !outcome.processed {
                    break;
                }
            }
            Ok(())
        }
    })
    .await
    .map_err(|error| error.to_string())?
    {
        Ok(()) => queue_service::process_queue(&db_path, &cancelled, &app).await,
        Err(error) => Err(error),
    };
    state.download_running.store(false, Ordering::SeqCst);
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
    run_optimization_pass(&app, &state, options.unwrap_or_default()).await
}

/// Runs a single pass of the optimization queue. Used by both the
/// `process_newspaper_optimization_queue` Tauri command and the per-edition
/// trigger that fires inside the download worker the moment a job reaches a
/// terminal download status. Returns the refreshed list of jobs.
///
/// The `optimization_running` flag is shared between these callers so a
/// manual "Optimize now" and a per-edition auto-trigger never overlap, but
/// the optimization is free to run while the download queue is still
/// processing the next edition.
pub(super) async fn run_optimization_pass(
    app: &tauri::AppHandle,
    state: &NewspaperState,
    options: OptimizationRunOptions,
) -> Result<Vec<NewspaperJob>, String> {
    if state.optimization_running.swap(true, Ordering::SeqCst) {
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
        options,
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
    state.optimization_running.store(false, Ordering::SeqCst);
    if let Ok(jobs) = &result {
        library_events::emit(app, state, jobs);
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
    runtime: State<'_, WorkflowRuntime>,
    batch_id: String,
) -> Result<(), String> {
    batch_service::cancel(state.db_path(), &batch_id)?;
    cancel_newspaper_workflow_runs(&runtime, &batch_id)?;
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
pub fn set_all_newspaper_jobs_paused(
    state: State<'_, NewspaperState>,
    paused: bool,
) -> Result<Vec<String>, String> {
    let mut connection =
        crate::cache::open_runtime(state.db_path()).map_err(|error| error.to_string())?;
    let outcome = job_service::set_all_paused(&mut connection, paused, Utc::now().timestamp())?;
    if outcome.triggered_cancel {
        state.cancelled.store(true, Ordering::SeqCst);
    } else if !paused {
        // Resume must clear the cooperative cancellation flag so a follow-up
        // process_newspaper_queue invocation is guaranteed to re-arm the
        // worker, even if the in-flight worker has already unwound.
        state.cancelled.store(false, Ordering::SeqCst);
    }
    Ok(outcome.updated)
}

#[tauri::command]
pub fn reset_newspaper_database(
    state: State<'_, NewspaperState>,
    runtime: State<'_, WorkflowRuntime>,
    app: tauri::AppHandle,
) -> Result<NewspaperResetCounts, String> {
    // The UI is expected to call set_all_newspaper_jobs_paused(true) first
    // so the worker unwinds at a safe boundary. Defensive re-arm of every
    // in-memory flag here keeps a stale request from writing after the wipe
    // commits and lets the next process_newspaper_queue invocation start from
    // a clean slate.
    state.cancelled.store(true, Ordering::SeqCst);
    state.download_running.store(false, Ordering::SeqCst);
    state.optimization_running.store(false, Ordering::SeqCst);
    state
        .dimension_backfill_running
        .store(false, Ordering::SeqCst);
    state.set_optimization_runtime(OptimizationRuntimeStatus::default());
    runtime
        .delete_newspaper_runs()
        .map_err(|error| error.to_string())?;

    let connection =
        crate::cache::open_runtime(state.db_path()).map_err(|error| error.to_string())?;
    let counts = clear_newspaper_provider_data(&connection).map_err(|error| error.to_string())?;

    // Wipe the on-disk thumbnail cache (canonicalize + starts_with safety
    // pattern, same as remove_cached_thumbnail). Failure here is not fatal —
    // the DB is already wiped and stale thumbnails will be re-validated on
    // next access — but we surface it to the caller for transparency.
    let thumbnail_wipe_warning = job_service::clear_thumbnail_cache(state.db_path())
        .err()
        .map(|error| error.to_string());

    // Reset the cooperative flags and bump the cache-busting revisions so
    // the UI refreshes after the wipe.
    state.cancelled.store(false, Ordering::SeqCst);
    let library_revision = state.invalidate_library();
    let progress_revision = state.invalidate_progress();
    let _ = app.emit(
        "newspaper://library-invalidated",
        serde_json::json!({
            "reason": "reset",
            "libraryRevision": library_revision,
            "progressRevision": progress_revision,
            "thumbnailWarning": thumbnail_wipe_warning,
        }),
    );
    let _ = app.emit(
        "newspaper://clipping-invalidated",
        serde_json::json!({ "reason": "source_changed" }),
    );
    Ok(counts)
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
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    runtime: State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<(), String> {
    let status = match job_service::delete(state.db_path(), &job_id) {
        Ok(status) => status,
        Err(error) => {
            if runtime
                .get_run(job_id.clone())
                .map_err(|error| error.to_string())?
                .is_some()
            {
                cancel_or_delete_newspaper_run(&runtime, &job_id)?;
                let _ = app.emit(
                    "newspaper://clipping-invalidated",
                    serde_json::json!({ "reason": "source_changed", "jobId": job_id }),
                );
                return Ok(());
            }
            return Err(error);
        }
    };
    cancel_or_delete_newspaper_run(&runtime, &job_id)?;
    if matches!(status.as_str(), "active" | "optimizing") {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    let _ = app.emit(
        "newspaper://clipping-invalidated",
        serde_json::json!({ "reason": "source_changed", "jobId": job_id }),
    );
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
pub async fn get_newspaper_library_item(
    state: State<'_, NewspaperState>,
    job_id: String,
) -> Result<super::models::NewspaperLibraryItem, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || library_service::query_item(&db_path, &job_id))
        .await
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?
}

#[tauri::command]
pub async fn get_newspaper_activity_snapshot(
    state: State<'_, NewspaperState>,
    runtime: State<'_, WorkflowRuntime>,
) -> Result<NewspaperActivitySnapshot, String> {
    let db_path = state.db_path.clone();
    let revision = state.progress_revision();
    let optimization_runtime = state.optimization_runtime();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        overview_service::activity(&db_path, revision, optimization_runtime, Some(&runtime))
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
pub async fn search_newspaper_clippings(
    state: State<'_, ClippingService>,
    request: SearchNewspaperClippingsRequest,
) -> Result<SearchNewspaperClippingsPage, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .search(request)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_READ_FAILED".to_string())?
}

#[tauri::command]
pub async fn get_newspaper_clippings_page(
    state: State<'_, ClippingService>,
    request: GetNewspaperClippingsPageRequest,
) -> Result<NewspaperClippingsPage, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .list_page(request)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_READ_FAILED".to_string())?
}

#[tauri::command]
pub async fn get_newspaper_clipping(
    state: State<'_, ClippingService>,
    clipping_id: String,
) -> Result<NewspaperClippingDetail, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .detail_response(&clipping_id)
            .and_then(|detail| {
                detail.ok_or_else(|| {
                    super::clipping_models::ClippingError::new(ClippingErrorCode::NotFound)
                })
            })
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_READ_FAILED".to_string())?
}

#[tauri::command]
pub async fn update_newspaper_clipping(
    app: tauri::AppHandle,
    state: State<'_, ClippingService>,
    request: UpdateNewspaperClippingRequest,
) -> Result<NewspaperClippingDetail, String> {
    let clipping_id = request.clipping_id.clone();
    let checkpoint = request
        .checkpoint
        .map(|identity| identity.validated())
        .transpose()
        .map_err(|error| error.as_safe_string())?;
    let service = state.inner().clone();
    let detail = tauri::async_runtime::spawn_blocking(move || {
        service
            .update_note_response(
                &request.clipping_id,
                request.expected_revision,
                &request.title,
                &request.note_markdown,
                checkpoint,
                Utc::now().timestamp(),
            )
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_WRITE_FAILED".to_string())??;
    let _ = app.emit(
        "newspaper://clipping-invalidated",
        serde_json::json!({ "clippingId": clipping_id, "revision": detail.revision }),
    );
    Ok(detail)
}

#[tauri::command]
pub async fn delete_newspaper_clipping(
    app: tauri::AppHandle,
    state: State<'_, ClippingService>,
    request: DeleteNewspaperClippingRequest,
) -> Result<DeleteNewspaperClippingResponse, String> {
    let clipping_id = request.clipping_id.clone();
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .delete(&request.clipping_id, request.expected_revision)
            .map_err(|error| match error.code {
                ClippingErrorCode::RevisionConflict => {
                    "CLIPPING_DELETE_REVISION_CONFLICT".to_string()
                }
                _ => error.as_safe_string(),
            })
    })
    .await
    .map_err(|_| "CLIPPING_DELETE_FAILED".to_string())??;
    let _ = app.emit(
        "newspaper://clipping-invalidated",
        serde_json::json!({ "clippingId": clipping_id, "reason": "deleted" }),
    );
    Ok(DeleteNewspaperClippingResponse {
        clipping_id,
        deleted: true,
    })
}

#[tauri::command]
pub async fn recover_newspaper_clipping_asset(
    app: tauri::AppHandle,
    state: State<'_, ClippingService>,
    clipping_id: String,
) -> Result<NewspaperClippingDetail, String> {
    let event_id = clipping_id.clone();
    let service = state.inner().clone();
    let detail = tauri::async_runtime::spawn_blocking(move || {
        service
            .recover_asset(&clipping_id)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_ASSET_RECOVERY_FAILED".to_string())??;
    let _ = app.emit(
        "newspaper://clipping-invalidated",
        serde_json::json!({ "clippingId": event_id, "reason": "asset_recovered" }),
    );
    Ok(detail)
}

#[tauri::command]
pub async fn checkpoint_newspaper_clipping_note(
    state: State<'_, ClippingService>,
    request: CheckpointClippingNoteRequest,
) -> Result<ClippingNoteCheckpointAck, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .draft_service()
            .checkpoint(request, Utc::now().timestamp())
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_WRITE_FAILED".to_string())?
}

#[tauri::command]
pub async fn load_newspaper_clipping_note_recovery(
    state: State<'_, ClippingService>,
    request: LoadClippingNoteRecoveryRequest,
) -> Result<ClippingNoteRecoveryResponse, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .draft_service()
            .load(&request)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_READ_FAILED".to_string())?
}

#[tauri::command]
pub async fn claim_newspaper_clipping_note_recovery(
    state: State<'_, ClippingService>,
    request: ClaimClippingNoteRecoveryRequest,
) -> Result<ClippingNoteRecoveryResponse, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .draft_service()
            .claim(request)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_WRITE_FAILED".to_string())?
}

#[tauri::command]
pub async fn discard_newspaper_clipping_note_recovery(
    state: State<'_, ClippingService>,
    request: DiscardClippingNoteRecoveryRequest,
) -> Result<(), String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .draft_service()
            .discard(request)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_WRITE_FAILED".to_string())?
}

#[tauri::command]
pub async fn ensure_newspaper_clipping_thumbnail(
    state: State<'_, ClippingService>,
    clipping_id: String,
) -> Result<EnsureNewspaperClippingThumbnailResponse, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .ensure_thumbnail(&clipping_id)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_ASSET_ROOT_UNAVAILABLE".to_string())?
}

#[tauri::command]
pub async fn search_possible_newspaper_clippings(
    state: State<'_, ClippingService>,
    request: SearchPossibleNewspaperClippingsRequest,
) -> Result<SearchPossibleNewspaperClippingsResponse, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .search_possible(request)
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_DATABASE_READ_FAILED".to_string())?
}

#[tauri::command]
pub fn list_newspaper_snapshot_roots(
    state: State<'_, ClippingService>,
) -> Result<Vec<ClippingRootSummary>, String> {
    state
        .list_root_summaries()
        .map_err(|error| error.as_safe_string())
}

#[tauri::command]
pub async fn check_newspaper_snapshot_root(
    state: State<'_, ClippingService>,
    root_id: String,
) -> Result<ClippingRootSummary, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .check_root(&root_id, Utc::now().timestamp())
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_ASSET_ROOT_UNAVAILABLE".to_string())?
}

#[tauri::command]
pub async fn reconnect_newspaper_snapshot_root(
    app: tauri::AppHandle,
    state: State<'_, ClippingService>,
    root_id: String,
) -> Result<ReconnectNewspaperSnapshotRootResult, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(selection) = app
            .dialog()
            .file()
            .set_title("Recover Newspaper snapshots folder")
            .blocking_pick_folder()
        else {
            return Ok(ReconnectNewspaperSnapshotRootResult::Cancelled);
        };
        let selected = selection
            .into_path()
            .map_err(|_| "CLIPPING_ASSET_ROOT_UNAVAILABLE".to_string())?;
        service
            .reconnect_root(&root_id, &selected, Utc::now().timestamp())
            .map(|root| ReconnectNewspaperSnapshotRootResult::Connected { root })
            .map_err(|error| error.as_safe_string())
    })
    .await
    .map_err(|_| "CLIPPING_ASSET_ROOT_UNAVAILABLE".to_string())?
}

#[tauri::command]
pub async fn open_newspaper_snapshot_root(
    state: State<'_, ClippingService>,
    root_id: String,
) -> Result<(), String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = service
            .verified_root_open_path(&root_id)
            .map_err(|error| error.as_safe_string())?;
        crate::shell::open_folder_in_explorer(&path)
            .map_err(|_| "CLIPPING_ASSET_ROOT_UNAVAILABLE".to_string())
    })
    .await
    .map_err(|_| "CLIPPING_ASSET_ROOT_UNAVAILABLE".to_string())?
}

#[tauri::command]
pub fn open_newspaper_download_folder(path: String) -> Result<(), String> {
    crate::shell::open_folder_in_explorer(Path::new(&path))
}

#[tauri::command]
pub async fn recover_newspaper_library(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    clipping: State<'_, ClippingService>,
    path: String,
) -> Result<RecoverNewspaperLibraryResult, String> {
    let db_path = state.db_path.clone();
    let clipping = clipping.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        library_recovery::recover(&db_path, &clipping, &path)
    })
    .await
    .map_err(|error| error.to_string())?;
    if result.is_ok() {
        library_events::after_archive_change(&app, &state)?;
        let _ = app.emit(
            "newspaper://clipping-invalidated",
            serde_json::json!({ "reason": "library_recovered" }),
        );
    }
    result
}

#[tauri::command]
pub async fn import_existing_newspaper_archive(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    path: String,
) -> Result<usize, String> {
    let db_path = state.db_path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        archive_service::import(&db_path, Path::new(&path)).map(|counts| counts.imported)
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

fn cancel_newspaper_workflow_runs(runtime: &WorkflowRuntime, batch_id: &str) -> Result<(), String> {
    let now = Utc::now().timestamp();
    for run in runtime
        .list_newspaper_runs(1_000)
        .map_err(|error| error.to_string())?
    {
        if run.state.is_terminal() {
            continue;
        }
        let job = super::projection::job_from_run(&run);
        if job.batch_id == batch_id {
            runtime
                .cancel_run(run.id, now)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn cancel_or_delete_newspaper_run(runtime: &WorkflowRuntime, job_id: &str) -> Result<(), String> {
    if let Some(run) = runtime
        .get_run(job_id.to_string())
        .map_err(|error| error.to_string())?
    {
        if !run.state.is_terminal() {
            runtime
                .cancel_run(job_id.to_string(), Utc::now().timestamp())
                .map_err(|error| error.to_string())?;
        }
        let _ = runtime.delete_run_if_terminal(job_id.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn download_queue_command_does_not_run_sqlite_on_the_async_executor() {
        let source = include_str!("commands.rs");
        assert!(
            source.contains("pub async fn process_newspaper_queue"),
            "queue processing must stay an async command so it can yield"
        );
        let process_fn = source
            .split("pub async fn process_newspaper_queue")
            .nth(1)
            .unwrap_or_default();
        let process_fn = process_fn
            .split("#[tauri::command]")
            .next()
            .unwrap_or_default();
        assert!(
            process_fn.contains("tauri::async_runtime::spawn_blocking"),
            "materialize_due must use spawn_blocking"
        );
        assert!(
            process_fn.contains("Arc::clone(&state.cancelled)"),
            "clone cancelled before the download pass so the command does not borrow state across .await"
        );
    }
}
