use crate::artifact_downloader::{ArtifactHttpClient, CancellationFlag};
use crate::auth::{
    select_first_valid_browser_token, validate_li_at_with_client, BrowserSource,
    ReqwestLinkedInHomeClient, ValidatedLinkedInSession,
};
use crate::browser_cookies::{
    chromium_user_data_path_for_source, read_li_at_candidates, BrowserCookieRoots,
    ChromiumCookieDecoder,
};
use crate::cache::{
    append_job_event, clear_failed_jobs, clear_job_schedule, clear_linkedin_provider_data,
    get_course_cache_entry, get_job, get_setting, list_artifacts_for_job, list_download_history,
    list_job_events, list_jobs_by_status, list_ready_queued_jobs, list_recent_jobs, open_runtime,
    remove_completed_download_job, remove_download_job, set_all_download_jobs_paused,
    set_download_job_paused, upsert_setting_json, DownloadHistoryEntry, JobRecord, NewJobEvent,
    ProviderResetCounts,
};
use crate::course::CourseApiClient;
use crate::download_orchestrator::process_next_queued_job_and_download_artifacts_with_quiz_assessments;
use crate::linkedin::{parse_course_urls, CourseUrl};
use crate::live_clients::AuthenticatedLinkedInClient;
use crate::quality::{fallback_order, VideoQuality};
use crate::quiz_hints::{quiz_hints_from_json, quiz_hints_json, QuizHints};
use crate::shell::open_folder_in_explorer;
use crate::token_store;
use crate::workflow::application::runtime::{DrainOutcome, WorkflowRuntime};
use crate::workflow::domain::state::RunState;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct LinkVaultState {
    db_path: PathBuf,
    token_path: PathBuf,
    download_cancellation: Arc<AtomicBool>,
    download_paused: Arc<AtomicBool>,
    session_token: Arc<Mutex<Option<String>>>,
}

impl LinkVaultState {
    #[cfg(test)]
    pub fn new(db_path: PathBuf) -> Self {
        Self::with_shared_flags(
            db_path,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
        )
    }

    pub fn with_shared_flags(
        db_path: PathBuf,
        download_cancellation: Arc<AtomicBool>,
        download_paused: Arc<AtomicBool>,
        session_token: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let token_path = db_path.with_file_name("linkvault.li_at.dpapi");
        Self {
            db_path,
            token_path,
            download_cancellation,
            download_paused,
            session_token,
        }
    }

    fn connection(&self) -> Result<Connection, String> {
        open_runtime(&self.db_path).map_err(|error| error.to_string())
    }

    fn reset_download_cancellation(&self) -> DownloadCancellation {
        self.download_cancellation.store(false, Ordering::SeqCst);
        self.download_paused.store(false, Ordering::SeqCst);
        self.download_cancellation()
    }

    fn request_download_cancellation(&self) {
        self.download_cancellation.store(true, Ordering::SeqCst);
    }

    fn download_cancellation(&self) -> DownloadCancellation {
        DownloadCancellation {
            cancelled: Arc::clone(&self.download_cancellation),
            paused: Arc::clone(&self.download_paused),
        }
    }

    fn set_download_paused(&self, paused: bool) {
        self.download_paused.store(paused, Ordering::SeqCst);
    }

    fn is_download_paused(&self) -> bool {
        self.download_paused.load(Ordering::SeqCst)
    }

    fn session_token_slot(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.session_token)
    }

    #[cfg(test)]
    fn is_download_cancellation_requested(&self) -> bool {
        self.download_cancellation.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn token_path(&self) -> &std::path::Path {
        &self.token_path
    }
}

#[derive(Clone)]
struct DownloadCancellation {
    cancelled: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl CancellationFlag for DownloadCancellation {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Serialize)]
pub struct BootstrapState {
    default_resolution: VideoQuality,
    browser_sources: Vec<&'static str>,
    stores_plaintext_tokens_in_sqlite: bool,
    has_saved_token: bool,
    saved_download_preferences: Option<SavedDownloadPreferences>,
    persisted_jobs: Vec<PersistedDownloadJob>,
    recent_events: Vec<PersistedJobEvent>,
    download_history: Vec<DownloadHistoryEntry>,
    download_history_file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDownloadPreferences {
    output_dir: String,
    selected_quality: String,
    delay_seconds: u32,
    #[serde(default = "default_video_wait_min_seconds")]
    video_wait_min_seconds: u32,
    #[serde(default = "default_video_wait_max_seconds")]
    video_wait_max_seconds: u32,
    browser_source: String,
    download_videos: bool,
    download_exercises: bool,
    download_subtitles: bool,
    #[serde(default = "default_download_quizzes")]
    download_quizzes: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartDownloadRequest {
    course_urls: String,
    output_dir: String,
    selected_quality: String,
    delay_seconds: u32,
    #[serde(default = "default_video_wait_min_seconds")]
    video_wait_min_seconds: u32,
    #[serde(default = "default_video_wait_max_seconds")]
    video_wait_max_seconds: u32,
    browser_source: String,
    download_videos: bool,
    download_exercises: bool,
    download_subtitles: bool,
    #[serde(default = "default_download_quizzes")]
    download_quizzes: bool,
    #[serde(default)]
    schedule: Option<DownloadScheduleRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadScheduleRequest {
    window_minutes: u32,
    min_wait_minutes: u32,
    max_wait_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueuedDownloadJob {
    id: String,
    course_slug: String,
    source_url: String,
    status: String,
    thumbnail_url: Option<String>,
    scheduled_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistedDownloadJob {
    id: String,
    course_slug: String,
    source_url: String,
    status: String,
    title: Option<String>,
    thumbnail_url: Option<String>,
    selected_quality: String,
    output_dir: String,
    paused: bool,
    scheduled_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
    artifact_counts: ArtifactProgressCounts,
    video_artifacts: Vec<PersistedDownloadArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistedDownloadArtifact {
    id: String,
    display_name: String,
    status: String,
    size_bytes: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ArtifactProgressCounts {
    total: usize,
    completed: usize,
    failed: usize,
    cancelled: usize,
    active: usize,
    pending: usize,
    skipped: usize,
    video_total: usize,
    video_completed: usize,
    subtitle_total: usize,
    subtitle_completed: usize,
    quiz_total: usize,
    quiz_completed: usize,
    study_guide_total: usize,
    study_guide_completed: usize,
    exercise_total: usize,
    exercise_completed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistedJobEvent {
    id: i64,
    job_id: String,
    event_type: String,
    message: String,
    payload_json: Option<String>,
    created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartDownloadResponse {
    jobs: Vec<QueuedDownloadJob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessQueuedDownloadResponse {
    processed: bool,
    completed_artifacts: usize,
    failed_artifacts: usize,
    cancelled_artifacts: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessQueuedBatchRequest {
    delay_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelDownloadResponse {
    cancellation_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SavedTokenStatus {
    has_saved_token: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenDownloadFolderResponse {
    path: String,
}

#[tauri::command]
pub fn bootstrap_state(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub fn parse_linkedin_course_urls(input: String) -> Result<Vec<CourseUrl>, String> {
    parse_course_urls(&input).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn quality_fallback_order(selected: VideoQuality) -> Vec<VideoQuality> {
    fallback_order(selected)
}

#[tauri::command]
pub fn start_download_jobs(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    request: StartDownloadRequest,
) -> Result<StartDownloadResponse, String> {
    let connection = state.connection()?;
    queue_download_jobs(&runtime, &connection, request, now_unix_timestamp())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_download_preferences(
    state: tauri::State<'_, LinkVaultState>,
    preferences: SavedDownloadPreferences,
) -> Result<SavedDownloadPreferences, String> {
    if preferences.output_dir.trim().is_empty() {
        return Err("Choose a download folder before saving settings.".to_string());
    }
    let (video_wait_min_seconds, video_wait_max_seconds) =
        crate::artifact_downloader::normalize_video_wait_bounds(
            preferences.video_wait_min_seconds,
            preferences.video_wait_max_seconds,
        );
    let preferences = SavedDownloadPreferences {
        video_wait_min_seconds,
        video_wait_max_seconds,
        ..preferences
    };
    crate::artifact_downloader::set_live_video_wait_bounds(
        preferences.video_wait_min_seconds,
        preferences.video_wait_max_seconds,
    );

    let connection = state.connection()?;
    persist_download_preferences(&connection, &preferences, now_unix_timestamp())?;
    Ok(preferences)
}

#[tauri::command]
pub fn set_linkedin_video_wait_bounds(
    state: tauri::State<'_, LinkVaultState>,
    min_seconds: u32,
    max_seconds: u32,
) -> Result<(u32, u32), String> {
    let (min_seconds, max_seconds) =
        crate::artifact_downloader::normalize_video_wait_bounds(min_seconds, max_seconds);
    crate::artifact_downloader::set_live_video_wait_bounds(min_seconds, max_seconds);

    if let Ok(connection) = state.connection() {
        if let Ok(Some(setting)) = get_setting(&connection, "download.preferences") {
            if let Ok(mut preferences) =
                serde_json::from_str::<SavedDownloadPreferences>(&setting.value_json)
            {
                preferences.video_wait_min_seconds = min_seconds;
                preferences.video_wait_max_seconds = max_seconds;
                let _ = persist_download_preferences(
                    &connection,
                    &preferences,
                    now_unix_timestamp(),
                );
            }
        }
    }

    Ok((min_seconds, max_seconds))
}

#[tauri::command]
pub fn cancel_active_download(
    state: tauri::State<'_, LinkVaultState>,
) -> Result<CancelDownloadResponse, String> {
    state.request_download_cancellation();
    state.set_download_paused(false);
    Ok(CancelDownloadResponse {
        cancellation_requested: true,
    })
}

#[tauri::command]
pub fn set_download_job_pause(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
    paused: bool,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    match set_download_job_paused(&connection, &job_id, paused, now_unix_timestamp()) {
        Ok(job) => {
            if job.status == "active" {
                state.set_download_paused(paused);
            }
        }
        Err(_) => {
            if let Some(run) = runtime.get_run(job_id).map_err(|error| error.to_string())? {
                match run.state {
                    RunState::Running | RunState::Cancelling => {
                        // Cooperative pause for the in-flight LinkedIn executor. The
                        // run stays Running so the job remains on the Active tab;
                        // bootstrap overlays this flag onto projected jobs.
                        state.set_download_paused(paused);
                    }
                    RunState::Queued | RunState::Paused | RunState::RetryWait => {
                        // Queued workflow runs are not in the legacy jobs table.
                        // Keep the atomic flag clear for idle queue pauses so a
                        // later active download is not accidentally frozen.
                        if !paused {
                            state.set_download_paused(false);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub fn set_all_downloads_paused(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    paused: bool,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    set_all_download_jobs_paused(&connection, paused, now_unix_timestamp())
        .map_err(|error| error.to_string())?;
    state.set_download_paused(paused);
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub fn reset_linkedin_database(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<ProviderResetCounts, String> {
    // The UI is expected to call set_all_downloads_paused(true) first so the
    // worker unwinds at a safe boundary. We still defensively re-arm the
    // flags here so a stale in-flight request can't keep writing after the
    // wipe commits.
    state.set_download_paused(true);
    runtime
        .delete_linkedin_runs()
        .map_err(|error| error.to_string())?;
    let connection = state.connection()?;
    let counts = clear_linkedin_provider_data(&connection).map_err(|error| error.to_string())?;
    // Regenerate the history markdown so the next read sees a valid empty
    // document instead of rows that no longer exist in the database.
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    state.reset_download_cancellation();
    Ok(counts)
}

#[tauri::command]
pub fn retry_failed_download_job(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    retry_failed_download_job_inner(&runtime, &connection, job_id, now_unix_timestamp())?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub fn clear_failed_download_jobs(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<BootstrapState, String> {
    runtime
        .delete_terminal_linkedin_runs()
        .map_err(|error| error.to_string())?;
    let connection = state.connection()?;
    clear_failed_jobs(&connection).map_err(|error| error.to_string())?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub fn remove_download_queue_item(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let now = now_unix_timestamp();
    let mut removed_workflow = false;
    if let Some(run) = runtime
        .get_run(job_id.clone())
        .map_err(|error| error.to_string())?
    {
        if matches!(
            run.state,
            RunState::Running | RunState::Cancelling | RunState::Queued | RunState::Paused | RunState::RetryWait
        ) {
            if matches!(run.state, RunState::Running | RunState::Cancelling) {
                state.request_download_cancellation();
                state.set_download_paused(false);
            }
            removed_workflow = runtime
                .cancel_and_delete_run(job_id.clone(), now)
                .map_err(|error| error.to_string())?;
        } else {
            removed_workflow = runtime
                .delete_run_if_terminal(job_id.clone())
                .map_err(|error| error.to_string())?;
        }
    }
    let connection = state.connection()?;
    match remove_download_job(&connection, &job_id) {
        Ok(job) => {
            if job.status == "active" {
                state.request_download_cancellation();
                state.set_download_paused(false);
            }
        }
        Err(_error) if removed_workflow => {}
        Err(error) => return Err(error.to_string()),
    }
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub fn delete_completed_download(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    let job = get_job(&connection, &job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Download job was not found.".to_string())?;
    if job.status != "completed" {
        return Err("Only completed downloads can delete their course files.".to_string());
    }

    let artifacts =
        list_artifacts_for_job(&connection, &job.id).map_err(|error| error.to_string())?;
    delete_completed_download_files(&job, &artifacts)?;
    remove_completed_download_job(&connection, &job.id).map_err(|error| error.to_string())?;
    let _ = runtime.delete_run_if_terminal(job_id);

    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub fn download_scheduled_job_now(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    let now = now_unix_timestamp();
    if let Some(run) = runtime
        .get_run(job_id.clone())
        .map_err(|error| error.to_string())?
    {
        let mut request: super::projection::LinkedInWorkflowRequest =
            serde_json::from_str(&run.request_json).map_err(|error| error.to_string())?;
        request.scheduled_at = None;
        runtime
            .cancel_run(job_id.clone(), now)
            .map_err(|error| error.to_string())?;
        let _ = runtime.delete_run_if_terminal(job_id.clone());
        runtime
            .submit_linkedin_download(
                job_id,
                request.course_slug.clone(),
                serde_json::to_string(&request).map_err(|error| error.to_string())?,
                run.output_root,
                now,
                None,
            )
            .map_err(|error| error.to_string())?;
    } else {
        clear_job_schedule(&connection, &job_id, now).map_err(|error| error.to_string())?;
    }
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )
}

#[tauri::command]
pub async fn save_li_at_token(
    state: tauri::State<'_, LinkVaultState>,
    token: String,
) -> Result<SavedTokenStatus, String> {
    let token_path = state.token_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut client = ReqwestLinkedInHomeClient::new().map_err(|error| error.to_string())?;
        validate_li_at_with_client(&token, &mut client).map_err(|error| error.to_string())?;
        token_store::save_token(&token_path, &token).map_err(|error| error.to_string())?;
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(SavedTokenStatus {
        has_saved_token: true,
    })
}

#[tauri::command]
pub fn clear_saved_li_at_token(
    state: tauri::State<'_, LinkVaultState>,
) -> Result<SavedTokenStatus, String> {
    token_store::clear_token(&state.token_path).map_err(|error| error.to_string())?;
    Ok(SavedTokenStatus {
        has_saved_token: false,
    })
}

#[tauri::command]
pub fn open_download_folder(
    state: tauri::State<'_, LinkVaultState>,
    job_id: String,
) -> Result<OpenDownloadFolderResponse, String> {
    let connection = state.connection()?;
    let job = crate::cache::get_job(&connection, &job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Download job was not found.".to_string())?;
    let artifacts =
        list_artifacts_for_job(&connection, &job.id).map_err(|error| error.to_string())?;
    let folder = download_folder_for_job(&job, &artifacts);
    open_folder_in_explorer(&folder)?;
    Ok(OpenDownloadFolderResponse {
        path: folder.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub async fn process_next_queued_download_with_saved_token(
    app: tauri::AppHandle,
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let db_path = state.db_path.clone();
    let token_path = state.token_path.clone();
    let cancellation = state.reset_download_cancellation();
    let runtime = (*runtime).clone();
    let drained = tauri::async_runtime::spawn_blocking({
        let runtime = runtime.clone();
        move || {
            runtime
                .drain_type("linkedin_download")
                .map_err(|error| error.to_string())
        }
    })
    .await
    .map_err(|error| error.to_string())??;
    if drained.processed {
        return Ok(drain_outcome_to_process_response(drained));
    }
    let token_and_session = tauri::async_runtime::spawn_blocking(move || {
        let token = token_store::load_token(&token_path).map_err(|error| error.to_string())?;
        let mut home_client =
            ReqwestLinkedInHomeClient::new().map_err(|error| error.to_string())?;
        let session = validate_li_at_with_client(&token, &mut home_client)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((token, session))
    })
    .await
    .map_err(|error| error.to_string())??;

    let (token, session) = token_and_session;
    let quiz_assessments =
        extract_quizzes_for_next_job(app, db_path.clone(), session.clone(), now_unix_timestamp())
            .await;
    tauri::async_runtime::spawn_blocking(move || {
        process_next_queued_download_with_validated_token(
            db_path,
            token,
            session,
            now_unix_timestamp(),
            cancellation,
            quiz_assessments,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn process_queued_download_batch_with_saved_token(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    request: ProcessQueuedBatchRequest,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let db_path = state.db_path.clone();
    let token_path = state.token_path.clone();
    let cancellation = state.reset_download_cancellation();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut combined = ProcessQueuedDownloadResponse {
            processed: false,
            completed_artifacts: 0,
            failed_artifacts: 0,
            cancelled_artifacts: 0,
        };
        loop {
            if cancellation.is_cancelled() {
                return Ok(combined);
            }
            let outcome = runtime
                .drain_type("linkedin_download")
                .map_err(|error| error.to_string())?;
            if !outcome.processed {
                break;
            }
            merge_process_response(&mut combined, &drain_outcome_to_process_response(outcome));
            if combined.cancelled_artifacts > 0 || cancellation.is_cancelled() {
                return Ok(combined);
            }
            sleep_between_queued_courses(request.delay_seconds, &cancellation);
        }
        let token = token_store::load_token(&token_path).map_err(|error| error.to_string())?;
        let mut home_client =
            ReqwestLinkedInHomeClient::new().map_err(|error| error.to_string())?;
        let session = validate_li_at_with_client(&token, &mut home_client)
            .map_err(|error| error.to_string())?;
        let legacy = process_queued_download_batch_with_validated_token(
            db_path,
            token,
            session,
            request.delay_seconds,
            now_unix_timestamp(),
            cancellation,
        )?;
        merge_process_response(&mut combined, &legacy);
        Ok(combined)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn process_next_queued_download_from_browser_source(
    app: tauri::AppHandle,
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    source: BrowserSource,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let db_path = state.db_path.clone();
    let cancellation = state.reset_download_cancellation();
    let session_token = state.session_token_slot();
    let runtime = (*runtime).clone();
    let token_and_session = tauri::async_runtime::spawn_blocking(move || {
        let roots = BrowserCookieRoots::from_env();
        let decoder = chromium_user_data_path_for_source(source, &roots)
            .map(|path| ChromiumCookieDecoder::from_user_data_path(&path))
            .unwrap_or_else(ChromiumCookieDecoder::disabled);
        let candidates =
            read_li_at_candidates(source, &roots, &decoder).map_err(|error| error.to_string())?;
        let mut home_client =
            ReqwestLinkedInHomeClient::new().map_err(|error| error.to_string())?;
        let (candidate, session) = select_first_valid_browser_token(&candidates, &mut home_client)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((candidate.value, session))
    })
    .await
    .map_err(|error| error.to_string())??;

    let (token, session) = token_and_session;
    {
        let mut slot = session_token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(token.clone());
    }
    let drained = tauri::async_runtime::spawn_blocking({
        let runtime = runtime.clone();
        move || {
            runtime
                .drain_type("linkedin_download")
                .map_err(|error| error.to_string())
        }
    })
    .await
    .map_err(|error| error.to_string());
    {
        let mut slot = session_token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = None;
    }
    let drained = drained??;
    if drained.processed {
        return Ok(drain_outcome_to_process_response(drained));
    }
    let quiz_assessments =
        extract_quizzes_for_next_job(app, db_path.clone(), session.clone(), now_unix_timestamp())
            .await;
    tauri::async_runtime::spawn_blocking(move || {
        process_next_queued_download_with_validated_token(
            db_path,
            token,
            session,
            now_unix_timestamp(),
            cancellation,
            quiz_assessments,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn process_queued_download_batch_with_validated_token(
    db_path: PathBuf,
    token: String,
    session: ValidatedLinkedInSession,
    delay_seconds: u32,
    timestamp: i64,
    cancellation: DownloadCancellation,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let connection = open_runtime(&db_path).map_err(|error| error.to_string())?;
    let mut course_client =
        AuthenticatedLinkedInClient::new(&token, &session).map_err(|error| error.to_string())?;
    let mut artifact_client = course_client.clone();

    let response = process_queued_download_batch_with_clients(
        &connection,
        &mut course_client,
        &mut artifact_client,
        timestamp,
        delay_seconds,
        &cancellation,
    )?;
    let _ = sync_download_history_file(&connection, &download_history_file_path_for_db(&db_path));
    Ok(response)
}

fn process_next_queued_download_with_validated_token(
    db_path: PathBuf,
    token: String,
    session: ValidatedLinkedInSession,
    timestamp: i64,
    cancellation: DownloadCancellation,
    quiz_assessments: Vec<crate::course::CourseAssessment>,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let connection = open_runtime(&db_path).map_err(|error| error.to_string())?;
    let mut course_client =
        AuthenticatedLinkedInClient::new(&token, &session).map_err(|error| error.to_string())?;
    let mut artifact_client = course_client.clone();

    let response = process_next_queued_download_with_clients(
        &connection,
        &mut course_client,
        &mut artifact_client,
        timestamp,
        &cancellation,
        quiz_assessments,
    )?;
    let _ = sync_download_history_file(&connection, &download_history_file_path_for_db(&db_path));
    Ok(response)
}

fn process_queued_download_batch_with_clients(
    connection: &Connection,
    course_client: &mut impl CourseApiClient,
    artifact_client: &mut impl ArtifactHttpClient,
    timestamp: i64,
    delay_seconds: u32,
    cancellation: &impl CancellationFlag,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let mut combined = ProcessQueuedDownloadResponse {
        processed: false,
        completed_artifacts: 0,
        failed_artifacts: 0,
        cancelled_artifacts: 0,
    };

    loop {
        if cancellation.is_cancelled() {
            return Ok(combined);
        }

        let quiz_assessments = record_quiz_metadata_discovery_for_next_job(connection, timestamp);
        let response = process_next_queued_download_with_clients(
            connection,
            course_client,
            artifact_client,
            timestamp,
            cancellation,
            quiz_assessments,
        )?;
        merge_process_response(&mut combined, &response);

        if !response.processed || response.cancelled_artifacts > 0 || cancellation.is_cancelled() {
            return Ok(combined);
        }

        let has_remaining_queued_jobs = list_ready_queued_jobs(connection, timestamp)
            .map_err(|error| error.to_string())?
            .into_iter()
            .next()
            .is_some();
        if !has_remaining_queued_jobs {
            return Ok(combined);
        }

        sleep_between_queued_courses(delay_seconds, cancellation);
    }
}

fn process_next_queued_download_with_clients(
    connection: &Connection,
    course_client: &mut impl CourseApiClient,
    artifact_client: &mut impl ArtifactHttpClient,
    timestamp: i64,
    cancellation: &impl CancellationFlag,
    quiz_assessments: Vec<crate::course::CourseAssessment>,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let summary = process_next_queued_job_and_download_artifacts_with_quiz_assessments(
        connection,
        course_client,
        artifact_client,
        cancellation,
        timestamp,
        quiz_assessments,
    )
    .map_err(|error| error.to_string())?;

    Ok(match summary {
        Some(summary) => ProcessQueuedDownloadResponse {
            processed: true,
            completed_artifacts: summary.completed,
            failed_artifacts: summary.failed,
            cancelled_artifacts: summary.cancelled,
        },
        None => ProcessQueuedDownloadResponse {
            processed: false,
            completed_artifacts: 0,
            failed_artifacts: 0,
            cancelled_artifacts: 0,
        },
    })
}

fn merge_process_response(
    combined: &mut ProcessQueuedDownloadResponse,
    response: &ProcessQueuedDownloadResponse,
) {
    combined.processed |= response.processed;
    combined.completed_artifacts += response.completed_artifacts;
    combined.failed_artifacts += response.failed_artifacts;
    combined.cancelled_artifacts += response.cancelled_artifacts;
}

fn sleep_between_queued_courses(delay_seconds: u32, cancellation: &impl CancellationFlag) {
    for _ in 0..delay_seconds {
        cancellation.wait_if_paused();
        if cancellation.is_cancelled() {
            return;
        }
        thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn download_folder_for_job(job: &JobRecord, artifacts: &[crate::cache::ArtifactRecord]) -> PathBuf {
    let output_dir = PathBuf::from(job.output_dir.trim());
    for artifact in artifacts {
        let artifact_path = PathBuf::from(artifact.path.trim());
        if let Some(course_folder) = course_folder_from_artifact_path(&output_dir, &artifact_path) {
            if course_folder.is_dir() {
                return course_folder;
            }
        }
    }
    output_dir
}

fn delete_completed_download_files(
    job: &JobRecord,
    artifacts: &[crate::cache::ArtifactRecord],
) -> Result<Option<PathBuf>, String> {
    if job.status != "completed" {
        return Err("Only completed downloads can delete their course files.".to_string());
    }

    let output_dir = PathBuf::from(job.output_dir.trim());
    if output_dir.as_os_str().is_empty() {
        return Err("The completed download does not have a saved output folder.".to_string());
    }
    if artifacts.is_empty() {
        return Ok(None);
    }

    let mut course_folders = HashSet::new();
    for artifact in artifacts {
        let artifact_path = PathBuf::from(artifact.path.trim());
        let relative = artifact_path.strip_prefix(&output_dir).map_err(|_| {
            "LinkedVault refused to delete files outside the saved download folder.".to_string()
        })?;
        let mut components = relative.components();
        let first = match components.next() {
            Some(Component::Normal(value)) => value,
            _ => {
                return Err(
                    "LinkedVault could not identify a safe course folder to delete.".to_string(),
                )
            }
        };
        if components.any(|component| !matches!(component, Component::Normal(_))) {
            return Err(
                "LinkedVault refused to delete a course folder containing an unsafe path."
                    .to_string(),
            );
        }
        course_folders.insert(output_dir.join(first));
    }

    if course_folders.len() != 1 {
        return Err(
            "LinkedVault could not identify one safe course folder for this completed download."
                .to_string(),
        );
    }
    let course_folder = course_folders.into_iter().next().expect("one folder");
    if course_folder == output_dir {
        return Err("LinkedVault will never delete the selected download root.".to_string());
    }
    if !course_folder.exists() {
        return Ok(Some(course_folder));
    }
    if !course_folder.is_dir() {
        return Err("The saved course path is not a folder; no files were deleted.".to_string());
    }

    let canonical_output = fs::canonicalize(&output_dir)
        .map_err(|error| format!("Could not verify the saved download root: {error}"))?;
    let canonical_course = fs::canonicalize(&course_folder)
        .map_err(|error| format!("Could not verify the completed course folder: {error}"))?;
    if canonical_course == canonical_output || !canonical_course.starts_with(&canonical_output) {
        return Err(
            "LinkedVault refused to delete files outside the saved download root.".to_string(),
        );
    }

    fs::remove_dir_all(&course_folder)
        .map_err(|error| format!("Could not delete the completed course folder: {error}"))?;
    Ok(Some(course_folder))
}

fn course_folder_from_artifact_path(output_dir: &Path, artifact_path: &Path) -> Option<PathBuf> {
    let relative = artifact_path.strip_prefix(output_dir).ok()?;
    let first = relative
        .components()
        .find_map(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })?;
    Some(output_dir.join(first))
}

fn download_history_file_path_for_db(db_path: &Path) -> PathBuf {
    db_path.with_file_name("download-history.md")
}

fn sync_download_history_file(connection: &Connection, path: &Path) -> Result<(), String> {
    let entries = list_download_history(connection).map_err(|error| error.to_string())?;
    write_download_history_file(path, &entries).map_err(|error| error.to_string())
}

fn write_download_history_file(
    path: &Path,
    entries: &[DownloadHistoryEntry],
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut markdown = String::from("# LinkedVault Download History\n\n");
    markdown.push_str("| Date downloaded | Course | URL |\n");
    markdown.push_str("| --- | --- | --- |\n");
    for entry in entries {
        markdown.push_str(&format!(
            "| {} | {} | {} |\n",
            format_unix_timestamp_utc(entry.completed_at),
            escape_markdown_table_cell(&entry.course_title),
            escape_markdown_table_cell(&history_source_url(entry))
        ));
    }
    fs::write(path, markdown)
}

fn history_source_url(entry: &DownloadHistoryEntry) -> String {
    if entry.source_url.trim().is_empty() {
        format!("https://www.linkedin.com/learning/{}", entry.course_slug)
    } else {
        entry.source_url.clone()
    }
}

fn escape_markdown_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn format_unix_timestamp_utc(timestamp: i64) -> String {
    let timestamp = timestamp.max(0);
    let days = timestamp / 86_400;
    let seconds = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

async fn extract_quizzes_for_next_job(
    _app: tauri::AppHandle,
    db_path: PathBuf,
    _session: ValidatedLinkedInSession,
    timestamp: i64,
) -> Vec<crate::course::CourseAssessment> {
    let Ok(connection) = open_runtime(&db_path) else {
        return Vec::new();
    };
    record_quiz_metadata_discovery_for_next_job(&connection, timestamp)
}

fn record_quiz_metadata_discovery_for_next_job(
    connection: &Connection,
    timestamp: i64,
) -> Vec<crate::course::CourseAssessment> {
    let Ok(Some(job)) =
        list_ready_queued_jobs(&connection, timestamp).map(|jobs| jobs.into_iter().next())
    else {
        return Vec::new();
    };
    if !job.download_quizzes {
        return Vec::new();
    }

    let hints = quiz_hints_from_json(&job.quiz_hints_json);
    let _ = append_job_event(
        &connection,
        &NewJobEvent {
            job_id: job.id,
            event_type: "quiz.metadata_discovery".to_string(),
            message: "Quiz extraction will use authenticated LinkedIn course metadata.".to_string(),
            payload_json: Some(
                serde_json::json!({
                    "courseSlug": job.course_slug,
                    "hintQuizUrls": hints.quiz_urls.len(),
                    "hintAssessmentUrns": hints.assessment_urns.len(),
                    "source": "learning-api detailedCourses assessments field",
                })
                .to_string(),
            ),
            created_at: timestamp,
        },
    );
    Vec::new()
}

fn queue_download_jobs(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    request: StartDownloadRequest,
    created_at: i64,
) -> Result<StartDownloadResponse, String> {
    let courses = parse_course_urls(&request.course_urls).map_err(|error| error.to_string())?;
    if courses.is_empty() {
        return Err("Paste at least one LinkedIn Learning course URL.".to_string());
    }
    if request.output_dir.trim().is_empty() {
        return Err("Choose a download folder before starting.".to_string());
    }

    persist_download_preferences(
        connection,
        &SavedDownloadPreferences::from(&request),
        created_at,
    )?;
    crate::artifact_downloader::set_live_video_wait_bounds(
        request.video_wait_min_seconds,
        request.video_wait_max_seconds,
    );
    let scheduled_times =
        scheduled_download_times(request.schedule.as_ref(), &courses, created_at)?;

    let mut jobs = Vec::with_capacity(courses.len());
    for (index, course) in courses.iter().enumerate() {
        let scheduled_at = scheduled_times[index];
        let job_id = unique_job_id(runtime, connection, created_at, &course.slug, index)?;
        let workflow_request = super::projection::LinkedInWorkflowRequest {
            schema_version: 1,
            course_slug: course.slug.clone(),
            source_url: course.normalized_url.clone(),
            selected_quality: request.selected_quality.clone(),
            download_videos: request.download_videos,
            download_exercises: request.download_exercises,
            download_subtitles: request.download_subtitles,
            download_quizzes: request.download_quizzes,
            quiz_hints_json: course_quiz_hints_json(course),
            scheduled_at,
        };
        runtime
            .submit_linkedin_download(
                job_id.clone(),
                course.slug.clone(),
                serde_json::to_string(&workflow_request).map_err(|error| error.to_string())?,
                request.output_dir.clone(),
                created_at,
                scheduled_at,
            )
            .map_err(|error| error.to_string())?;

        jobs.push(QueuedDownloadJob {
            id: job_id,
            course_slug: course.slug.clone(),
            source_url: course.normalized_url.clone(),
            status: "queued".to_string(),
            thumbnail_url: None,
            scheduled_at,
        });
    }

    Ok(StartDownloadResponse { jobs })
}

fn unique_job_id(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    created_at: i64,
    course_slug: &str,
    index: usize,
) -> Result<String, String> {
    let base = format!(
        "job-{created_at}-{}-{index}",
        sanitize_identifier_fragment(course_slug)
    );
    if job_id_is_free(runtime, connection, &base)? {
        return Ok(base);
    }

    for suffix in 2..=10_000 {
        let candidate = format!("{base}-{suffix}");
        if job_id_is_free(runtime, connection, &candidate)? {
            return Ok(candidate);
        }
    }
    Err("Could not allocate a unique download job identifier.".to_string())
}

fn job_id_is_free(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    id: &str,
) -> Result<bool, String> {
    if runtime
        .get_run(id.to_string())
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(false);
    }
    Ok(get_job(connection, id)
        .map_err(|error| error.to_string())?
        .is_none())
}

fn retry_failed_download_job_inner(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    job_id: String,
    now: i64,
) -> Result<(), String> {
    if let Some(run) = runtime
        .get_run(job_id.clone())
        .map_err(|error| error.to_string())?
    {
        if !matches!(run.state, RunState::Failed | RunState::Cancelled) {
            return Err("Download job was not found or is no longer failed.".to_string());
        }
        let mut request: super::projection::LinkedInWorkflowRequest =
            serde_json::from_str(&run.request_json).map_err(|error| error.to_string())?;
        request.scheduled_at = None;
        let new_id = unique_job_id(runtime, connection, now, &request.course_slug, 0)?;
        runtime
            .submit_linkedin_download(
                new_id,
                request.course_slug.clone(),
                serde_json::to_string(&request).map_err(|error| error.to_string())?,
                run.output_root,
                now,
                None,
            )
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let Some(legacy) = get_job(connection, &job_id).map_err(|error| error.to_string())? else {
        return Err("Download job was not found or is no longer failed.".to_string());
    };
    if !matches!(
        legacy.status.to_ascii_lowercase().as_str(),
        "failed" | "cancelled"
    ) {
        return Err("Download job was not found or is no longer failed.".to_string());
    }
    let request = super::projection::LinkedInWorkflowRequest {
        schema_version: 1,
        course_slug: legacy.course_slug.clone(),
        source_url: legacy.source_url.clone(),
        selected_quality: legacy.selected_quality.clone(),
        download_videos: legacy.download_videos,
        download_exercises: legacy.download_exercises,
        download_subtitles: legacy.download_subtitles,
        download_quizzes: legacy.download_quizzes,
        quiz_hints_json: legacy.quiz_hints_json.clone(),
        scheduled_at: None,
    };
    let new_id = unique_job_id(runtime, connection, now, &legacy.course_slug, 0)?;
    runtime
        .submit_linkedin_download(
            new_id,
            legacy.course_slug,
            serde_json::to_string(&request).map_err(|error| error.to_string())?,
            legacy.output_dir,
            now,
            None,
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn drain_outcome_to_process_response(outcome: DrainOutcome) -> ProcessQueuedDownloadResponse {
    ProcessQueuedDownloadResponse {
        processed: outcome.processed,
        completed_artifacts: outcome.completed as usize,
        failed_artifacts: outcome.failed as usize,
        cancelled_artifacts: outcome.cancelled as usize,
    }
}

fn scheduled_download_times(
    schedule: Option<&DownloadScheduleRequest>,
    courses: &[CourseUrl],
    created_at: i64,
) -> Result<Vec<Option<i64>>, String> {
    let Some(schedule) = schedule else {
        return Ok(vec![None; courses.len()]);
    };
    if schedule.window_minutes == 0 || schedule.window_minutes > 10_080 {
        return Err("Schedule window must be between 1 minute and 7 days.".to_string());
    }
    if schedule.min_wait_minutes == 0 || schedule.min_wait_minutes > 10_080 {
        return Err("Minimum random wait must be between 1 minute and 7 days.".to_string());
    }
    if schedule.max_wait_minutes < schedule.min_wait_minutes || schedule.max_wait_minutes > 10_080 {
        return Err(
            "Maximum random wait must be at least the minimum and no more than 7 days.".to_string(),
        );
    }

    let window_minutes = u64::from(schedule.window_minutes);
    let minimum_required = u64::from(schedule.min_wait_minutes) * courses.len() as u64;
    if minimum_required > window_minutes {
        return Err(format!(
            "The schedule needs at least {} minutes for {} courses at a {} minute minimum wait.",
            minimum_required,
            courses.len(),
            schedule.min_wait_minutes
        ));
    }

    let mut elapsed_minutes = 0_u64;
    let mut times = Vec::with_capacity(courses.len());
    for (index, course) in courses.iter().enumerate() {
        let remaining_courses = courses.len().saturating_sub(index + 1) as u64;
        let reserved_minimum = remaining_courses * u64::from(schedule.min_wait_minutes);
        let available_for_this_wait = window_minutes
            .saturating_sub(elapsed_minutes)
            .saturating_sub(reserved_minimum);
        let max_wait = u64::from(schedule.max_wait_minutes).min(available_for_this_wait);
        let min_wait = u64::from(schedule.min_wait_minutes);
        let wait = pseudo_random_inclusive(
            schedule_seed(created_at, index, &course.slug),
            min_wait,
            max_wait.max(min_wait),
        );
        elapsed_minutes += wait;
        times.push(Some(created_at + (elapsed_minutes * 60) as i64));
    }
    Ok(times)
}

fn schedule_seed(created_at: i64, index: usize, slug: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64 ^ created_at as u64 ^ index as u64;
    for byte in slug.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn pseudo_random_inclusive(mut seed: u64, min: u64, max: u64) -> u64 {
    if max <= min {
        return min;
    }
    seed = seed.wrapping_add(0x9e3779b97f4a7c15);
    seed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    seed = (seed ^ (seed >> 27)).wrapping_mul(0x94d049bb133111eb);
    seed ^= seed >> 31;
    min + seed % (max - min + 1)
}

fn persist_download_preferences(
    connection: &Connection,
    preferences: &SavedDownloadPreferences,
    updated_at: i64,
) -> Result<(), String> {
    let settings_json = serde_json::to_string(preferences).map_err(|error| error.to_string())?;
    upsert_setting_json(
        connection,
        "download.preferences",
        &settings_json,
        updated_at,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn course_quiz_hints_json(course: &CourseUrl) -> String {
    quiz_hints_json(&QuizHints {
        quiz_urls: course.quiz_urls.clone(),
        assessment_urns: course.assessment_urns.clone(),
    })
}

impl From<&StartDownloadRequest> for SavedDownloadPreferences {
    fn from(request: &StartDownloadRequest) -> Self {
        let (video_wait_min_seconds, video_wait_max_seconds) =
            crate::artifact_downloader::normalize_video_wait_bounds(
                request.video_wait_min_seconds,
                request.video_wait_max_seconds,
            );
        Self {
            output_dir: request.output_dir.clone(),
            selected_quality: request.selected_quality.clone(),
            delay_seconds: request.delay_seconds,
            video_wait_min_seconds,
            video_wait_max_seconds,
            browser_source: request.browser_source.clone(),
            download_videos: request.download_videos,
            download_exercises: request.download_exercises,
            download_subtitles: request.download_subtitles,
            download_quizzes: request.download_quizzes,
        }
    }
}

fn default_download_quizzes() -> bool {
    true
}

fn default_video_wait_min_seconds() -> u32 {
    20
}

fn default_video_wait_max_seconds() -> u32 {
    40
}

fn load_bootstrap_state(
    connection: &Connection,
    runtime: Option<&WorkflowRuntime>,
    has_saved_token: bool,
    download_history_file_path: &Path,
    download_paused: bool,
) -> Result<BootstrapState, String> {
    let saved_download_preferences = get_setting(connection, "download.preferences")
        .map_err(|error| error.to_string())?
        .and_then(|setting| serde_json::from_str::<SavedDownloadPreferences>(&setting.value_json).ok());
    if let Some(preferences) = saved_download_preferences.as_ref() {
        crate::artifact_downloader::set_live_video_wait_bounds(
            preferences.video_wait_min_seconds,
            preferences.video_wait_max_seconds,
        );
    }
    let recent_jobs = {
        let legacy = bootstrap_jobs(connection).map_err(|error| error.to_string())?;
        if let Some(runtime) = runtime {
            let workflow_jobs = runtime
                .list_linkedin_runs(250)
                .map_err(|error| error.to_string())?
                .iter()
                .map(super::projection::job_from_run)
                .collect();
            super::projection::merge_linkedin_jobs(workflow_jobs, legacy)
        } else {
            legacy
        }
    };
    let mut recent_events = Vec::new();
    for job in &recent_jobs {
        recent_events.extend(
            list_job_events(connection, &job.id)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|event| PersistedJobEvent {
                    id: event.id,
                    job_id: event.job_id,
                    event_type: event.event_type,
                    message: event.message,
                    payload_json: event.payload_json,
                    created_at: event.created_at,
                }),
        );
    }
    recent_events.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    recent_events.truncate(20);

    let mut persisted_jobs = Vec::with_capacity(recent_jobs.len());
    for job in recent_jobs {
        let artifacts =
            list_artifacts_for_job(connection, &job.id).map_err(|error| error.to_string())?;
        let artifact_counts = summarize_artifacts(&artifacts);
        let video_artifacts = artifacts
            .iter()
            .filter(|artifact| artifact.artifact_type == "video")
            .map(|artifact| PersistedDownloadArtifact {
                id: artifact.id.clone(),
                display_name: Path::new(&artifact.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or("Course video")
                    .to_string(),
                status: artifact.status.clone(),
                size_bytes: artifact.size_bytes,
                created_at: artifact.created_at,
                updated_at: artifact.updated_at,
            })
            .collect();
        let cached_course = get_course_cache_entry(connection, &job.course_slug)
            .ok()
            .flatten();
        let thumbnail_url = cached_course
            .as_ref()
            .and_then(|entry| cached_course_thumbnail_url(&entry.payload_json));
        let paused = effective_linkedin_job_paused(&job, download_paused);
        persisted_jobs.push(PersistedDownloadJob {
            source_url: if job.source_url.trim().is_empty() {
                format!("https://www.linkedin.com/learning/{}", job.course_slug)
            } else {
                job.source_url.clone()
            },
            title: cached_course.and_then(|entry| entry.title),
            id: job.id,
            course_slug: job.course_slug,
            status: job.status,
            thumbnail_url,
            selected_quality: job.selected_quality,
            output_dir: job.output_dir,
            paused,
            scheduled_at: job.scheduled_at,
            created_at: job.created_at,
            updated_at: job.updated_at,
            artifact_counts,
            video_artifacts,
        });
    }
    let download_history = list_download_history(connection).map_err(|error| error.to_string())?;

    Ok(BootstrapState {
        default_resolution: VideoQuality::P1080,
        browser_sources: vec!["Chrome", "Edge", "Firefox"],
        stores_plaintext_tokens_in_sqlite: false,
        has_saved_token,
        saved_download_preferences,
        persisted_jobs,
        recent_events,
        download_history,
        download_history_file_path: download_history_file_path.to_string_lossy().to_string(),
    })
}

fn effective_linkedin_job_paused(job: &JobRecord, download_paused: bool) -> bool {
    job.paused || (download_paused && job.status == "active")
}

fn bootstrap_jobs(connection: &Connection) -> Result<Vec<JobRecord>, crate::cache::CacheError> {
    let mut jobs = Vec::new();
    let mut seen = HashSet::new();

    for status in ["active", "queued", "failed", "cancelled"] {
        for job in list_jobs_by_status(connection, status)? {
            seen.insert(job.id.clone());
            jobs.push(job);
        }
    }

    for job in list_recent_jobs(connection, 20)? {
        if seen.insert(job.id.clone()) {
            jobs.push(job);
        }
    }

    Ok(jobs)
}

fn cached_course_thumbnail_url(payload_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload_json)
        .ok()
        .and_then(|payload| {
            payload
                .get("thumbnail_url")
                .or_else(|| payload.get("thumbnailUrl"))
                .and_then(|value| value.as_str())
                .and_then(non_empty_string)
        })
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn summarize_artifacts(artifacts: &[crate::cache::ArtifactRecord]) -> ArtifactProgressCounts {
    let mut counts = ArtifactProgressCounts::default();
    for artifact in artifacts {
        counts.total += 1;
        match artifact.status.as_str() {
            "completed" => counts.completed += 1,
            "failed" => counts.failed += 1,
            "cancelled" => counts.cancelled += 1,
            "active" => counts.active += 1,
            "pending" => counts.pending += 1,
            "skipped" => counts.skipped += 1,
            _ => {}
        }

        match artifact.artifact_type.as_str() {
            "video" => {
                counts.video_total += 1;
                if artifact.status == "completed" {
                    counts.video_completed += 1;
                }
            }
            "subtitle" => {
                counts.subtitle_total += 1;
                if artifact.status == "completed" {
                    counts.subtitle_completed += 1;
                }
            }
            "quiz" => {
                counts.quiz_total += 1;
                if artifact.status == "completed" {
                    counts.quiz_completed += 1;
                }
            }
            "study_guide" => {
                counts.study_guide_total += 1;
                if artifact.status == "completed" {
                    counts.study_guide_completed += 1;
                }
            }
            "exercise_zip" | "exercise_file" => {
                counts.exercise_total += 1;
                if artifact.status == "completed" {
                    counts.exercise_completed += 1;
                }
            }
            _ => {}
        }
    }
    counts
}

pub(crate) fn now_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn sanitize_identifier_fragment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_downloader::{ArtifactDownloadError, ArtifactHttpResponse, NeverCancelled};
    use crate::cache::{
        get_setting, initialize, list_job_events, list_jobs_by_status, upsert_artifact,
        ArtifactRecord,
    };
    use crate::course::CourseFetchError;

    fn initialized_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
    }

    fn workflow_harness() -> (tempfile::TempDir, WorkflowRuntime, Connection) {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = crate::cache::initialize_database(&db_path).unwrap();
        drop(connection);
        let writer = crate::app::database_writer::DatabaseWriter::start(
            db_path.clone(),
            crate::app::database_diagnostics::DatabaseDiagnostics::default(),
        )
        .unwrap();
        let runtime = WorkflowRuntime::new(writer);
        let connection = crate::cache::open_runtime(&db_path).unwrap();
        (directory, runtime, connection)
    }

    #[test]
    fn queue_download_jobs_persists_safe_settings_jobs_and_events() {
        let (_dir, runtime, connection) = workflow_harness();

        let response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "linkedin.com/learning/sample-course\nhttps://www.linkedin.com/learning/second-course?trk=share".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 2,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: false,
                download_quizzes: true,
                schedule: None,
            },
            1_700_000_000,
        )
        .unwrap();

        let setting = get_setting(&connection, "download.preferences")
            .unwrap()
            .unwrap();
        let runs = runtime.list_linkedin_runs(10).unwrap();
        let sample = runs
            .iter()
            .find(|run| run.id == response.jobs[0].id)
            .unwrap();
        let request: super::super::projection::LinkedInWorkflowRequest =
            serde_json::from_str(&sample.request_json).unwrap();

        assert_eq!(response.jobs.len(), 2);
        assert_eq!(response.jobs[0].id, "job-1700000000-sample-course-0");
        assert_eq!(response.jobs[0].course_slug, "sample-course");
        assert_eq!(response.jobs[1].course_slug, "second-course");
        assert_eq!(setting.key, "download.preferences");
        assert!(setting.value_json.contains(r#""outputDir":"C:/downloads""#));
        assert!(setting.value_json.contains(r#""selectedQuality":"1080""#));
        assert!(!setting.value_json.to_ascii_lowercase().contains("li_at"));
        assert!(!setting.value_json.to_ascii_lowercase().contains("token"));
        assert!(list_jobs_by_status(&connection, "queued")
            .unwrap()
            .is_empty());
        assert_eq!(runs.len(), 2);
        assert_eq!(
            request.source_url,
            "https://www.linkedin.com/learning/sample-course"
        );
        assert_eq!(request.selected_quality, "1080");
        assert!(request.download_videos);
        assert!(request.download_exercises);
        assert!(!request.download_subtitles);
        assert!(request.download_quizzes);
        assert!(list_job_events(&connection, &response.jobs[0].id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn queue_download_jobs_persists_randomized_schedule_inside_window() {
        let (_dir, runtime, connection) = workflow_harness();
        let created_at = 1_700_000_000;

        let response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/first-course\nhttps://www.linkedin.com/learning/second-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: Some(DownloadScheduleRequest {
                    window_minutes: 120,
                    min_wait_minutes: 10,
                    max_wait_minutes: 30,
                }),
            },
            created_at,
        )
        .unwrap();

        assert_eq!(response.jobs.len(), 2);
        assert!(response.jobs.iter().all(|job| job.scheduled_at.is_some()));
        assert!(response.jobs[0].scheduled_at.unwrap() >= created_at + 10 * 60);
        assert!(response.jobs[1].scheduled_at.unwrap() > response.jobs[0].scheduled_at.unwrap());
        assert!(response.jobs[1].scheduled_at.unwrap() <= created_at + 2 * 60 * 60);
        assert!(runtime
            .list_linkedin_runs(10)
            .unwrap()
            .iter()
            .all(|run| run.state == RunState::RetryWait));
        assert!(list_jobs_by_status(&connection, "queued")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn queue_download_jobs_accepts_a_sub_hour_schedule_window() {
        let (_dir, runtime, connection) = workflow_harness();
        let created_at = 1_700_000_000;

        let response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/short-window-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "720".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: Some(DownloadScheduleRequest {
                    window_minutes: 15,
                    min_wait_minutes: 5,
                    max_wait_minutes: 15,
                }),
            },
            created_at,
        )
        .unwrap();

        let scheduled_at = response.jobs[0].scheduled_at.unwrap();
        assert!(scheduled_at >= created_at + 5 * 60);
        assert!(scheduled_at <= created_at + 15 * 60);
    }

    #[test]
    fn queue_download_jobs_rejects_schedule_window_shorter_than_minimum_waits() {
        let (_dir, runtime, connection) = workflow_harness();
        let course_urls = (0..5)
            .map(|index| format!("https://www.linkedin.com/learning/course-{index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let error = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls,
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: Some(DownloadScheduleRequest {
                    window_minutes: 60,
                    min_wait_minutes: 15,
                    max_wait_minutes: 30,
                }),
            },
            100,
        )
        .unwrap_err();

        assert!(error.contains("at least 75 minutes"));
        assert!(list_jobs_by_status(&connection, "queued")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn queue_download_jobs_persists_direct_quiz_hints() {
        let (_dir, runtime, connection) = workflow_harness();

        queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course/quiz/urn:li:learningApiAssessment:69813586?resume=false&u=52983649&trk=ignored".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: None,
            },
            100,
        )
        .unwrap();

        let run = runtime
            .list_linkedin_runs(1)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let request: super::super::projection::LinkedInWorkflowRequest =
            serde_json::from_str(&run.request_json).unwrap();
        let hints = quiz_hints_from_json(&request.quiz_hints_json);

        assert_eq!(
            request.source_url,
            "https://www.linkedin.com/learning/sample-course"
        );
        assert_eq!(
            hints.quiz_urls,
            vec![
                "https://www.linkedin.com/learning/sample-course/quiz/urn:li:learningApiAssessment:69813586?resume=false&u=52983649"
                    .to_string()
            ]
        );
        assert_eq!(
            hints.assessment_urns,
            vec!["urn:li:learningApiAssessment:69813586".to_string()]
        );
    }

    #[test]
    fn queue_download_jobs_rejects_empty_output_folder() {
        let (_dir, runtime, connection) = workflow_harness();

        let error = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
                output_dir: " ".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: None,
            },
            100,
        )
        .unwrap_err();

        assert!(error.contains("download folder"));
        assert!(list_jobs_by_status(&connection, "queued")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn bootstrap_state_loads_saved_preferences_and_persisted_jobs() {
        let (_dir, runtime, connection) = workflow_harness();
        let response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "720".to_string(),
                delay_seconds: 5,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Firefox".to_string(),
                download_videos: true,
                download_exercises: false,
                download_subtitles: true,
                download_quizzes: true,
                schedule: None,
            },
            1_700_000_000,
        )
        .unwrap();
        crate::cache::insert_job(
            &connection,
            &JobRecord {
                id: response.jobs[0].id.clone(),
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                status: "failed".to_string(),
                selected_quality: "720".to_string(),
                download_videos: true,
                download_exercises: false,
                download_subtitles: true,
                download_quizzes: true,
                quiz_hints_json: "[]".to_string(),
                output_dir: "C:/downloads".to_string(),
                paused: false,
                scheduled_at: None,
                created_at: 1_700_000_000,
                updated_at: 1_700_000_020,
            },
        )
        .unwrap();
        for (id, artifact_type, status) in [
            ("video-1", "video", "completed"),
            ("video-2", "video", "failed"),
            ("subtitle-1", "subtitle", "completed"),
            ("quiz-1", "quiz", "completed"),
            ("exercise-1", "exercise_zip", "cancelled"),
        ] {
            upsert_artifact(
                &connection,
                &ArtifactRecord {
                    id: id.to_string(),
                    job_id: response.jobs[0].id.clone(),
                    artifact_type: artifact_type.to_string(),
                    path: format!("C:/downloads/sample-course/{id}"),
                    status: status.to_string(),
                    size_bytes: None,
                    created_at: 1_700_000_012,
                    updated_at: 1_700_000_018,
                },
            )
            .unwrap();
        }

        let bootstrap = load_bootstrap_state(
            &connection,
            Some(&runtime),
            true,
            Path::new("C:/downloads/download-history.md"),
            false,
        )
        .unwrap();
        let preferences = bootstrap.saved_download_preferences.unwrap();

        assert_eq!(bootstrap.default_resolution, VideoQuality::P1080);
        assert!(!bootstrap.stores_plaintext_tokens_in_sqlite);
        assert!(bootstrap.has_saved_token);
        assert_eq!(preferences.output_dir, "C:/downloads");
        assert_eq!(preferences.selected_quality, "720");
        assert_eq!(preferences.browser_source, "Firefox");
        assert_eq!(preferences.delay_seconds, 5);
        assert!(preferences.download_videos);
        assert!(!preferences.download_exercises);
        assert!(preferences.download_subtitles);
        assert!(preferences.download_quizzes);
        assert_eq!(bootstrap.persisted_jobs.len(), 1);
        assert_eq!(bootstrap.persisted_jobs[0].id, response.jobs[0].id);
        assert_eq!(bootstrap.persisted_jobs[0].course_slug, "sample-course");
        assert_eq!(bootstrap.persisted_jobs[0].status, "queued");
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.total, 5);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.completed, 3);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.failed, 1);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.cancelled, 1);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.video_total, 2);
        assert_eq!(bootstrap.persisted_jobs[0].video_artifacts.len(), 2);
        assert_eq!(
            bootstrap.persisted_jobs[0].video_artifacts[0].display_name,
            "video-1"
        );
        assert_eq!(
            bootstrap.persisted_jobs[0].video_artifacts[1].status,
            "failed"
        );
        assert_eq!(
            bootstrap.persisted_jobs[0].artifact_counts.video_completed,
            1
        );
        assert_eq!(
            bootstrap.persisted_jobs[0].artifact_counts.subtitle_total,
            1
        );
        assert_eq!(
            bootstrap.persisted_jobs[0]
                .artifact_counts
                .subtitle_completed,
            1
        );
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.quiz_total, 1);
        assert_eq!(
            bootstrap.persisted_jobs[0].artifact_counts.quiz_completed,
            1
        );
        assert_eq!(
            bootstrap.persisted_jobs[0].artifact_counts.exercise_total,
            1
        );
        assert_eq!(
            bootstrap.persisted_jobs[0].source_url,
            "https://www.linkedin.com/learning/sample-course"
        );
    }

    #[test]
    fn bootstrap_state_keeps_large_pending_queue_uncapped() {
        let (_dir, runtime, connection) = workflow_harness();
        let course_urls = (0..105)
            .map(|index| format!("https://www.linkedin.com/learning/course-{index:03}"))
            .collect::<Vec<_>>()
            .join("\n");

        queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls,
                output_dir: "C:/downloads".to_string(),
                selected_quality: "720".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: None,
            },
            1_700_000_000,
        )
        .unwrap();

        let bootstrap = load_bootstrap_state(
            &connection,
            Some(&runtime),
            true,
            Path::new("C:/downloads/download-history.md"),
            false,
        )
        .unwrap();

        assert_eq!(bootstrap.persisted_jobs.len(), 105);
        assert_eq!(
            bootstrap
                .persisted_jobs
                .iter()
                .filter(|job| job.status == "queued")
                .count(),
            105
        );
    }

    #[test]
    fn process_next_queued_download_with_clients_reports_no_work_without_network() {
        let connection = initialized_connection();
        let mut course_client = NoopCourseClient;
        let mut artifact_client = NoopArtifactClient;

        let response = process_next_queued_download_with_clients(
            &connection,
            &mut course_client,
            &mut artifact_client,
            200,
            &NeverCancelled,
            Vec::new(),
        )
        .unwrap();

        assert_eq!(
            response,
            ProcessQueuedDownloadResponse {
                processed: false,
                completed_artifacts: 0,
                failed_artifacts: 0,
                cancelled_artifacts: 0,
            }
        );
    }

    #[test]
    fn process_queued_download_batch_with_clients_reports_no_work_without_network() {
        let connection = initialized_connection();
        let mut course_client = NoopCourseClient;
        let mut artifact_client = NoopArtifactClient;

        let response = process_queued_download_batch_with_clients(
            &connection,
            &mut course_client,
            &mut artifact_client,
            200,
            0,
            &NeverCancelled,
        )
        .unwrap();

        assert_eq!(
            response,
            ProcessQueuedDownloadResponse {
                processed: false,
                completed_artifacts: 0,
                failed_artifacts: 0,
                cancelled_artifacts: 0,
            }
        );
    }

    #[test]
    fn immediate_queue_processing_leaves_future_scheduled_jobs_untouched() {
        let (_dir, runtime, connection) = workflow_harness();
        let created_at = chrono::Utc::now().timestamp();
        let scheduled_response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/scheduled-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: Some(DownloadScheduleRequest {
                    window_minutes: 120,
                    min_wait_minutes: 30,
                    max_wait_minutes: 30,
                }),
            },
            created_at,
        )
        .unwrap();
        let immediate_response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/immediate-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: None,
            },
            created_at,
        )
        .unwrap();

        let scheduled = runtime
            .get_run(scheduled_response.jobs[0].id.clone())
            .unwrap()
            .unwrap();
        let immediate = runtime
            .get_run(immediate_response.jobs[0].id.clone())
            .unwrap()
            .unwrap();
        assert_eq!(scheduled.state, RunState::RetryWait);
        assert_eq!(immediate.state, RunState::Queued);
        assert_eq!(
            scheduled_response.jobs[0].scheduled_at,
            Some(created_at + 30 * 60)
        );
        assert!(immediate_response.jobs[0].scheduled_at.is_none());
    }

    #[test]
    fn duplicate_course_requests_in_the_same_second_keep_distinct_jobs() {
        let (_dir, runtime, connection) = workflow_harness();
        let request = || StartDownloadRequest {
            course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
            output_dir: "C:/downloads".to_string(),
            selected_quality: "1080".to_string(),
            delay_seconds: 0,
            video_wait_min_seconds: 20,
            video_wait_max_seconds: 40,
            browser_source: "Chrome".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            schedule: None,
        };

        let first = queue_download_jobs(&runtime, &connection, request(), 100).unwrap();
        let second = queue_download_jobs(&runtime, &connection, request(), 100).unwrap();

        assert_ne!(first.jobs[0].id, second.jobs[0].id);
        assert_eq!(runtime.list_linkedin_runs(10).unwrap().len(), 2);
        assert!(list_jobs_by_status(&connection, "queued")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn download_folder_for_job_prefers_course_folder_from_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let course_dir = temp.path().join("Sample Course");
        std::fs::create_dir_all(&course_dir).unwrap();
        let job = JobRecord {
            id: "job-1".to_string(),
            course_slug: "sample-course".to_string(),
            source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
            status: "completed".to_string(),
            selected_quality: "720".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            quiz_hints_json: "[]".to_string(),
            output_dir: temp.path().to_string_lossy().to_string(),
            paused: false,
            scheduled_at: None,
            created_at: 100,
            updated_at: 200,
        };
        let artifacts = vec![crate::cache::ArtifactRecord {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            artifact_type: "video".to_string(),
            path: course_dir
                .join("01 - Intro")
                .join("01 - Welcome.mp4")
                .to_string_lossy()
                .to_string(),
            status: "completed".to_string(),
            size_bytes: Some(10),
            created_at: 100,
            updated_at: 200,
        }];

        assert_eq!(download_folder_for_job(&job, &artifacts), course_dir);
    }

    #[test]
    fn delete_completed_download_files_removes_only_the_course_folder() {
        let temp = tempfile::tempdir().unwrap();
        let course_dir = temp.path().join("Sample Course");
        let chapter_dir = course_dir.join("01 - Intro");
        let sibling_dir = temp.path().join("Keep Me");
        std::fs::create_dir_all(&chapter_dir).unwrap();
        std::fs::create_dir_all(&sibling_dir).unwrap();
        let artifact_path = chapter_dir.join("01 - Welcome.mp4");
        std::fs::write(&artifact_path, b"video").unwrap();
        std::fs::write(sibling_dir.join("notes.txt"), b"keep").unwrap();
        let job = JobRecord {
            id: "job-1".to_string(),
            course_slug: "sample-course".to_string(),
            source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
            status: "completed".to_string(),
            selected_quality: "720".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            quiz_hints_json: "[]".to_string(),
            output_dir: temp.path().to_string_lossy().to_string(),
            paused: false,
            scheduled_at: None,
            created_at: 100,
            updated_at: 200,
        };
        let artifacts = vec![crate::cache::ArtifactRecord {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            artifact_type: "video".to_string(),
            path: artifact_path.to_string_lossy().to_string(),
            status: "completed".to_string(),
            size_bytes: Some(5),
            created_at: 100,
            updated_at: 200,
        }];

        let deleted = delete_completed_download_files(&job, &artifacts).unwrap();

        assert_eq!(deleted, Some(course_dir.clone()));
        assert!(!course_dir.exists());
        assert!(temp.path().is_dir());
        assert!(sibling_dir.join("notes.txt").is_file());
    }

    #[test]
    fn delete_completed_download_files_rejects_artifacts_outside_the_output_root() {
        let output = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("do-not-delete.mp4");
        std::fs::write(&outside_file, b"video").unwrap();
        let job = JobRecord {
            id: "job-1".to_string(),
            course_slug: "sample-course".to_string(),
            source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
            status: "completed".to_string(),
            selected_quality: "720".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            quiz_hints_json: "[]".to_string(),
            output_dir: output.path().to_string_lossy().to_string(),
            paused: false,
            scheduled_at: None,
            created_at: 100,
            updated_at: 200,
        };
        let artifacts = vec![crate::cache::ArtifactRecord {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            artifact_type: "video".to_string(),
            path: outside_file.to_string_lossy().to_string(),
            status: "completed".to_string(),
            size_bytes: Some(5),
            created_at: 100,
            updated_at: 200,
        }];

        let error = delete_completed_download_files(&job, &artifacts).unwrap_err();

        assert!(error.contains("outside the saved download folder"));
        assert!(outside_file.is_file());
        assert!(output.path().is_dir());
    }

    #[test]
    fn delete_completed_download_files_allows_record_cleanup_when_no_artifacts_exist() {
        let output = tempfile::tempdir().unwrap();
        let job = JobRecord {
            id: "job-1".to_string(),
            course_slug: "sample-course".to_string(),
            source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
            status: "completed".to_string(),
            selected_quality: "720".to_string(),
            download_videos: false,
            download_exercises: false,
            download_subtitles: false,
            download_quizzes: false,
            quiz_hints_json: "[]".to_string(),
            output_dir: output.path().to_string_lossy().to_string(),
            paused: false,
            scheduled_at: None,
            created_at: 100,
            updated_at: 200,
        };

        let deleted = delete_completed_download_files(&job, &[]).unwrap();

        assert_eq!(deleted, None);
        assert!(output.path().is_dir());
    }

    #[test]
    fn active_workflow_pause_flag_overlays_projected_jobs() {
        let active = JobRecord {
            id: "job-active".to_string(),
            course_slug: "active-course".to_string(),
            source_url: "https://www.linkedin.com/learning/active-course".to_string(),
            status: "active".to_string(),
            selected_quality: "720".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            quiz_hints_json: "[]".to_string(),
            output_dir: ".".to_string(),
            paused: false,
            scheduled_at: None,
            created_at: 1,
            updated_at: 1,
        };
        let queued = JobRecord {
            status: "queued".to_string(),
            ..active.clone()
        };
        assert!(!effective_linkedin_job_paused(&active, false));
        assert!(effective_linkedin_job_paused(&active, true));
        assert!(!effective_linkedin_job_paused(&queued, true));
        assert!(effective_linkedin_job_paused(
            &JobRecord {
                paused: true,
                ..queued
            },
            false
        ));
    }

    #[test]
    fn download_history_file_is_user_readable_markdown() {
        let temp = tempfile::tempdir().unwrap();
        let history_path = temp.path().join("download-history.md");
        write_download_history_file(
            &history_path,
            &[DownloadHistoryEntry {
                job_id: "job-1".to_string(),
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                course_title: "Sample | Course".to_string(),
                output_dir: "C:/downloads".to_string(),
                completed_at: 1_700_000_000,
            }],
        )
        .unwrap();

        let markdown = std::fs::read_to_string(history_path).unwrap();

        assert!(markdown.contains("# LinkedVault Download History"));
        assert!(markdown.contains("2023-11-14 22:13 UTC"));
        assert!(markdown.contains("Sample \\| Course"));
        assert!(markdown.contains("https://www.linkedin.com/learning/sample-course"));
    }

    #[test]
    fn linkvault_state_records_and_resets_download_cancellation_requests() {
        let state = LinkVaultState::new("linkvault-test.sqlite3".into());
        assert_eq!(
            state
                .token_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some("linkvault.li_at.dpapi")
        );

        state.request_download_cancellation();
        state.set_download_paused(true);
        assert!(state.is_download_cancellation_requested());
        assert!(state.download_cancellation().is_cancelled());
        assert!(state.download_cancellation().is_paused());

        let cancellation = state.reset_download_cancellation();
        assert!(!state.is_download_cancellation_requested());
        assert!(!cancellation.is_cancelled());
        assert!(!cancellation.is_paused());
    }

    struct NoopCourseClient;

    impl CourseApiClient for NoopCourseClient {
        fn get(&mut self, url: &str) -> Result<String, CourseFetchError> {
            panic!("course client should not be called without queued jobs: {url}");
        }
    }

    struct NoopArtifactClient;

    impl ArtifactHttpClient for NoopArtifactClient {
        fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError> {
            panic!("artifact client should not be called without queued jobs: {url}");
        }
    }
}
