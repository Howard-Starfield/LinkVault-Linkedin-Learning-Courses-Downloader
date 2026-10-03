//! Stable Tauri command facade for the newspaper subsystem.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
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
    optimization_service, overview_service, page_metadata, reader_service, schedule_service,
    thumbnails::{EnsureThumbnailResult, ThumbnailCoordinator},
};
use crate::app::database_writer::DatabaseWriter;
use crate::cache::{clear_newspaper_provider_data, NewspaperResetCounts};
use crate::workflow::application::runtime::WorkflowRuntime;

pub use super::state::NewspaperState;

/// UI mutations can wait for SQLite or touch files; run them away from the
/// event loop using the application's existing shared owners.
async fn run_mutation<T: Send + 'static>(
    app: tauri::AppHandle,
    work: impl FnOnce(
            &tauri::AppHandle,
            &NewspaperState,
            &DatabaseWriter,
            &WorkflowRuntime,
        ) -> Result<T, String>
        + Send
        + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        work(
            &app,
            app.state::<NewspaperState>().inner(),
            app.state::<DatabaseWriter>().inner(),
            app.state::<WorkflowRuntime>().inner(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

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
pub async fn create_newspaper_batch(
    app: tauri::AppHandle,
    writer: State<'_, DatabaseWriter>,
    runtime: State<'_, WorkflowRuntime>,
    request: CreateNewspaperBatchRequest,
) -> Result<CreateNewspaperBatchResponse, String> {
    let writer = writer.inner().clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || batch_service::create(&writer, request))
            .await
            .map_err(|error| error.to_string())?;
    if result.is_ok() {
        super::supervisor::rearm_if_idle(&app);
        super::supervisor::invalidate_activity(&app);
        runtime.wake();
    }
    result
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
pub async fn create_newspaper_schedule(
    app: tauri::AppHandle,
    writer: State<'_, DatabaseWriter>,
    runtime: State<'_, WorkflowRuntime>,
    request: CreateNewspaperScheduleRequest,
) -> Result<NewspaperSchedule, String> {
    let writer = writer.inner().clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || schedule_service::create(&writer, request))
            .await
            .map_err(|error| error.to_string())?;
    if result.is_ok() {
        super::supervisor::rearm_if_idle(&app);
        super::supervisor::invalidate_activity(&app);
        runtime.wake();
    }
    result
}

#[tauri::command]
pub async fn toggle_newspaper_schedule(
    app: tauri::AppHandle,
    writer: State<'_, DatabaseWriter>,
    runtime: State<'_, WorkflowRuntime>,
    schedule_id: String,
    enabled: bool,
) -> Result<(), String> {
    let writer = writer.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        schedule_service::toggle(&writer, &schedule_id, enabled)
    })
    .await
    .map_err(|error| error.to_string())??;
    if enabled {
        super::supervisor::rearm_if_idle(&app);
    }
    super::supervisor::invalidate_activity(&app);
    runtime.wake();
    Ok(())
}

#[tauri::command]
pub async fn delete_newspaper_schedule(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    writer: State<'_, DatabaseWriter>,
    runtime: State<'_, WorkflowRuntime>,
    schedule_id: String,
) -> Result<(), String> {
    let writer = writer.inner().clone();
    if tauri::async_runtime::spawn_blocking(move || schedule_service::delete(&writer, &schedule_id))
        .await
        .map_err(|error| error.to_string())??
    {
        state.cancelled.store(true, Ordering::SeqCst);
    }
    super::supervisor::invalidate_activity(&app);
    runtime.wake();
    Ok(())
}

#[tauri::command]
pub async fn process_newspaper_queue(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    writer: State<'_, DatabaseWriter>,
    runtime: State<'_, WorkflowRuntime>,
) -> Result<Vec<NewspaperJob>, String> {
    super::supervisor::rearm_if_idle(&app);
    let db_path = state.db_path().to_path_buf();
    let writer = writer.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        schedule_service::materialize_due(&writer, &db_path)
    })
    .await
    .map_err(|error| error.to_string())??;
    super::supervisor::invalidate_activity(&app);
    runtime.wake();
    // Retain the command's response shape while the native supervisor owns
    // execution. Submitting work must not await an entire download queue.
    Ok(Vec::new())
}

#[tauri::command]
pub async fn process_newspaper_optimization_queue(
    app: tauri::AppHandle,
    runtime: State<'_, WorkflowRuntime>,
    options: Option<OptimizationRunOptions>,
) -> Result<Vec<NewspaperJob>, String> {
    super::supervisor::rearm_if_idle(&app);
    // The reply channel belongs to this command; the shared runtime owns and
    // joins the worker even if its UI caller disappears during optimization.
    let (reply, response) = tokio::sync::oneshot::channel();
    let admitted = runtime
        .spawn_supervisor_task("newspaper_optimization", move || {
            let state = app.state::<NewspaperState>();
            let result = tauri::async_runtime::block_on(run_optimization_pass(
                &app,
                &state,
                options.unwrap_or_default(),
            ));
            let _ = reply.send(result);
        })
        .map_err(|error| error.to_string())?;
    if !admitted {
        return Ok(Vec::new());
    }
    response
        .await
        .map_err(|_| "Newspaper optimization worker stopped before replying.".to_string())?
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
    super::supervisor::rearm_if_idle(app);
    if state.cancelled.load(Ordering::SeqCst) || app.state::<WorkflowRuntime>().is_shutting_down() {
        return Ok(Vec::new());
    }
    if state.optimization_running.swap(true, Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    let previous_runtime = state.optimization_runtime();
    let reported_progress = Arc::new(AtomicBool::new(false));
    let reporter_progress = Arc::clone(&reported_progress);
    let last_emit = Arc::new(Mutex::new(
        Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now),
    ));
    let progress_app = app.clone();
    let reporter = Arc::new(move |runtime: OptimizationRuntimeStatus| {
        reporter_progress.store(true, Ordering::SeqCst);
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
    let settled_runtime = OptimizationRuntimeStatus::default();
    state.set_optimization_runtime(settled_runtime.clone());
    if optimization_pass_needs_completion_event(
        &result,
        reported_progress.load(Ordering::SeqCst),
        &previous_runtime,
        &settled_runtime,
    ) {
        let progress_revision = state.invalidate_progress();
        let _ = app.emit(
            "newspaper://optimization-progress",
            serde_json::json!({ "revision": progress_revision, "runtime": settled_runtime }),
        );
    }
    state.optimization_running.store(false, Ordering::SeqCst);
    if let Ok(jobs) = &result {
        library_events::emit(app, state, jobs);
    }
    result
}

fn optimization_pass_needs_completion_event(
    result: &Result<Vec<NewspaperJob>, String>,
    reported_progress: bool,
    previous: &OptimizationRuntimeStatus,
    settled: &OptimizationRuntimeStatus,
) -> bool {
    reported_progress
        || previous != settled
        || result.as_ref().map_or(true, |jobs| !jobs.is_empty())
}

#[tauri::command]
pub async fn pause_newspaper_batch(
    app: tauri::AppHandle,
    batch_id: String,
    paused: bool,
) -> Result<(), String> {
    run_mutation(app, move |app, state, writer, runtime| {
        batch_service::pause(writer, &batch_id, paused)?;
        for run in runtime
            .list_newspaper_runs(-1)
            .map_err(|error| error.to_string())?
        {
            if !run.state.is_terminal()
                && super::projection::job_from_run(&run).batch_id == batch_id
            {
                runtime
                    .set_run_paused(run.id, paused, Utc::now().timestamp())
                    .map_err(|error| error.to_string())?;
            }
        }
        if paused {
            state.cancelled.store(true, Ordering::SeqCst);
        } else {
            super::supervisor::rearm_if_idle(app);
        }
        super::supervisor::invalidate_activity(app);
        runtime.wake();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn cancel_newspaper_batch(app: tauri::AppHandle, batch_id: String) -> Result<(), String> {
    run_mutation(app, move |app, state, writer, runtime| {
        batch_service::cancel(writer, &batch_id)?;
        cancel_newspaper_workflow_runs(runtime, &batch_id)?;
        state.cancelled.store(true, Ordering::SeqCst);
        super::supervisor::invalidate_activity(app);
        runtime.wake();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn retry_newspaper_job(app: tauri::AppHandle, job_id: String) -> Result<usize, String> {
    run_mutation(app, move |app, state, _writer, runtime| {
        let result = job_service::retry(state.db_path(), &job_id)?;
        super::supervisor::rearm_if_idle(app);
        super::supervisor::invalidate_activity(app);
        runtime.wake();
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn set_newspaper_job_pause(
    app: tauri::AppHandle,
    job_id: String,
    paused: bool,
) -> Result<(), String> {
    run_mutation(app, move |app, state, writer, runtime| {
        let existing = runtime
            .get_run(job_id.clone())
            .map_err(|error| error.to_string())?;
        let status = match job_service::set_pause_for_job(state.db_path(), &job_id, paused) {
            Ok(status) => status,
            Err(error) => match &existing {
                Some(run) if !run.state.is_terminal() => {
                    let request =
                        serde_json::from_str::<super::projection::NewspaperWorkflowRequest>(
                            &run.request_json,
                        )
                        .map_err(|error| error.to_string())?;
                    super::executor::materialize_job(writer, run, &request)?;
                    job_service::set_pause_for_job(state.db_path(), &job_id, paused)?
                }
                _ => return Err(error),
            },
        };
        if existing.is_some() {
            runtime
                .set_run_paused(job_id, paused, Utc::now().timestamp())
                .map_err(|error| error.to_string())?;
        }
        if paused && matches!(status.as_str(), "active" | "optimizing") {
            state.cancelled.store(true, Ordering::SeqCst);
        } else if !paused {
            super::supervisor::rearm_if_idle(app);
        }
        super::supervisor::invalidate_activity(app);
        runtime.wake();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn set_all_newspaper_jobs_paused(
    app: tauri::AppHandle,
    paused: bool,
) -> Result<Vec<String>, String> {
    run_mutation(app, move |app, state, _writer, runtime| {
        let mut connection =
            crate::cache::open_runtime(state.db_path()).map_err(|error| error.to_string())?;
        let mut outcome =
            job_service::set_all_paused(&mut connection, paused, Utc::now().timestamp())?;
        for run in runtime
            .list_newspaper_runs(-1)
            .map_err(|error| error.to_string())?
        {
            if !run.state.is_terminal() {
                outcome.triggered_cancel |= paused
                    && matches!(run.state, crate::workflow::domain::state::RunState::Running);
                runtime
                    .set_run_paused(run.id.clone(), paused, Utc::now().timestamp())
                    .map_err(|error| error.to_string())?;
                if !outcome.updated.contains(&run.id) {
                    outcome.updated.push(run.id);
                }
            }
        }
        if outcome.triggered_cancel {
            state.cancelled.store(true, Ordering::SeqCst);
        } else if !paused {
            // Only re-arm after the previous worker has safely unwound.
            super::supervisor::rearm_if_idle(app);
        }
        super::supervisor::invalidate_activity(app);
        runtime.wake();
        Ok(outcome.updated)
    })
    .await
}

#[tauri::command]
pub async fn reset_newspaper_database(
    app: tauri::AppHandle,
) -> Result<NewspaperResetCounts, String> {
    run_mutation(app, move |app, state, _writer, runtime| {
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
        let counts =
            clear_newspaper_provider_data(&connection).map_err(|error| error.to_string())?;

        // Wipe the on-disk thumbnail cache (canonicalize + starts_with safety
        // pattern, same as remove_cached_thumbnail). Failure here is not fatal —
        // the DB is already wiped and stale thumbnails will be re-validated on
        // next access — but we surface it to the caller for transparency.
        let thumbnail_wipe_warning = job_service::clear_thumbnail_cache(state.db_path())
            .err()
            .map(|error| error.to_string());

        // Reset the cooperative flags and bump the cache-busting revisions so
        // the UI refreshes after the wipe.
        super::supervisor::rearm_if_idle(app);
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
    })
    .await
}

#[tauri::command]
pub async fn reorder_newspaper_jobs(
    app: tauri::AppHandle,
    job_ids: Vec<String>,
) -> Result<(), String> {
    run_mutation(app, move |app, state, _writer, runtime| {
        job_service::reorder_for_jobs(state.db_path(), &job_ids)?;
        super::supervisor::invalidate_activity(app);
        runtime.wake();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn remove_newspaper_job(app: tauri::AppHandle, job_id: String) -> Result<(), String> {
    run_mutation(app, move |app, state, _writer, runtime| {
        let status = match job_service::delete(state.db_path(), &job_id) {
            Ok(status) => status,
            Err(error) => {
                if runtime
                    .get_run(job_id.clone())
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    cancel_or_delete_newspaper_run(runtime, &job_id)?;
                    let _ = app.emit(
                        "newspaper://clipping-invalidated",
                        serde_json::json!({ "reason": "source_changed", "jobId": job_id }),
                    );
                    return Ok(());
                }
                return Err(error);
            }
        };
        cancel_or_delete_newspaper_run(runtime, &job_id)?;
        if matches!(status.as_str(), "active" | "optimizing") {
            state.cancelled.store(true, Ordering::SeqCst);
        }
        let _ = app.emit(
            "newspaper://clipping-invalidated",
            serde_json::json!({ "reason": "source_changed", "jobId": job_id }),
        );
        Ok(())
    })
    .await
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
pub async fn save_newspaper_reading_progress(
    writer: State<'_, DatabaseWriter>,
    job_id: String,
    page_id: String,
) -> Result<NewspaperReadingProgress, String> {
    let writer = writer.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        reader_service::save_progress(&writer, &job_id, &page_id, Utc::now().timestamp())
    })
    .await
    .map_err(|error| error.to_string())?
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
        .list_newspaper_runs(-1)
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
    fn empty_optimization_pass_does_not_invalidate_unchanged_activity() {
        let settled = super::OptimizationRuntimeStatus::default();
        assert!(!super::optimization_pass_needs_completion_event(
            &Ok(Vec::new()),
            false,
            &settled,
            &settled
        ));
        assert!(super::optimization_pass_needs_completion_event(
            &Ok(Vec::new()),
            true,
            &settled,
            &settled
        ));
        assert!(super::optimization_pass_needs_completion_event(
            &Err("failure".to_string()),
            false,
            &settled,
            &settled
        ));
        let previous = super::OptimizationRuntimeStatus {
            active: true,
            ..settled.clone()
        };
        assert!(super::optimization_pass_needs_completion_event(
            &Ok(Vec::new()),
            false,
            &previous,
            &settled
        ));
    }

    #[test]
    fn reading_progress_command_waits_for_the_writer_off_the_event_loop() {
        let source = include_str!("commands.rs");
        let body = source
            .split("pub async fn save_newspaper_reading_progress(")
            .nth(1)
            .expect("reading progress must be an async command")
            .split("#[tauri::command]")
            .next()
            .unwrap();
        assert!(body.contains("writer: State<'_, DatabaseWriter>"));
        assert!(body.contains("tauri::async_runtime::spawn_blocking(move ||"));
        assert!(body.contains("reader_service::save_progress(&writer"));
        assert!(!body.contains("open_runtime"));
    }

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
        assert!(process_fn.contains("runtime.wake()"));
        assert!(
            !process_fn.contains("drain_type("),
            "native supervisor must own execution"
        );
    }
}
