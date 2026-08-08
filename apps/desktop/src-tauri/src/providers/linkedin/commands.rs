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
    get_course_cache_entry, get_job, get_setting, insert_job, list_artifacts_for_job,
    list_download_history, list_job_events, list_jobs_by_status, list_ready_queued_jobs,
    list_recent_jobs, open_runtime, remove_completed_download_job, remove_download_job,
    retry_failed_job, set_all_download_jobs_paused, set_download_job_paused, upsert_setting_json,
    DownloadHistoryEntry, JobRecord, NewJobEvent, ProviderResetCounts,
};
use crate::course::CourseApiClient;
use crate::download_orchestrator::process_next_queued_job_and_download_artifacts_with_quiz_assessments;
use crate::linkedin::{parse_course_urls, CourseUrl};
use crate::live_clients::AuthenticatedLinkedInClient;
use crate::quality::{fallback_order, VideoQuality};
use crate::quiz_hints::{quiz_hints_from_json, quiz_hints_json, QuizHints};
use crate::token_store;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct LinkVaultState {
    db_path: PathBuf,
    token_path: PathBuf,
    download_cancellation: Arc<AtomicBool>,
    download_paused: Arc<AtomicBool>,
}

impl LinkVaultState {
    pub fn new(db_path: PathBuf) -> Self {
        let token_path = db_path.with_file_name("linkvault.li_at.dpapi");
        Self {
            db_path,
            token_path,
            download_cancellation: Arc::new(AtomicBool::new(false)),
            download_paused: Arc::new(AtomicBool::new(false)),
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
    window_hours: u32,
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
pub fn bootstrap_state(state: tauri::State<'_, LinkVaultState>) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
    .map_err(|error| error.to_string())
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
    request: StartDownloadRequest,
) -> Result<StartDownloadResponse, String> {
    let connection = state.connection()?;
    queue_download_jobs(&connection, request, now_unix_timestamp())
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

    let connection = state.connection()?;
    persist_download_preferences(&connection, &preferences, now_unix_timestamp())?;
    Ok(preferences)
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
    job_id: String,
    paused: bool,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    let job = set_download_job_paused(&connection, &job_id, paused, now_unix_timestamp())
        .map_err(|error| error.to_string())?;
    if job.status == "active" {
        state.set_download_paused(paused);
    }
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
}

#[tauri::command]
pub fn set_all_downloads_paused(
    state: tauri::State<'_, LinkVaultState>,
    paused: bool,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    set_all_download_jobs_paused(&connection, paused, now_unix_timestamp())
        .map_err(|error| error.to_string())?;
    state.set_download_paused(paused);
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
}

#[tauri::command]
pub fn reset_linkedin_database(
    state: tauri::State<'_, LinkVaultState>,
) -> Result<ProviderResetCounts, String> {
    // The UI is expected to call set_all_downloads_paused(true) first so the
    // worker unwinds at a safe boundary. We still defensively re-arm the
    // flags here so a stale in-flight request can't keep writing after the
    // wipe commits.
    state.set_download_paused(true);
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
    job_id: String,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    retry_failed_job(&connection, &job_id, now_unix_timestamp())
        .map_err(|error| error.to_string())?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_failed_download_jobs(
    state: tauri::State<'_, LinkVaultState>,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    clear_failed_jobs(&connection).map_err(|error| error.to_string())?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_download_queue_item(
    state: tauri::State<'_, LinkVaultState>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    remove_download_job(&connection, &job_id).map_err(|error| error.to_string())?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_completed_download(
    state: tauri::State<'_, LinkVaultState>,
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

    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn download_scheduled_job_now(
    state: tauri::State<'_, LinkVaultState>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    clear_job_schedule(&connection, &job_id, now_unix_timestamp())
        .map_err(|error| error.to_string())?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    load_bootstrap_state(
        &connection,
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
    )
    .map_err(|error| error.to_string())
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
    open_folder_in_file_explorer(&folder)?;
    Ok(OpenDownloadFolderResponse {
        path: folder.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub async fn process_next_queued_download_with_saved_token(
    app: tauri::AppHandle,
    state: tauri::State<'_, LinkVaultState>,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let db_path = state.db_path.clone();
    let token_path = state.token_path.clone();
    let cancellation = state.reset_download_cancellation();
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
    request: ProcessQueuedBatchRequest,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let db_path = state.db_path.clone();
    let token_path = state.token_path.clone();
    let cancellation = state.reset_download_cancellation();
    tauri::async_runtime::spawn_blocking(move || {
        let token = token_store::load_token(&token_path).map_err(|error| error.to_string())?;
        let mut home_client =
            ReqwestLinkedInHomeClient::new().map_err(|error| error.to_string())?;
        let session = validate_li_at_with_client(&token, &mut home_client)
            .map_err(|error| error.to_string())?;
        process_queued_download_batch_with_validated_token(
            db_path,
            token,
            session,
            request.delay_seconds,
            now_unix_timestamp(),
            cancellation,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn process_next_queued_download_from_browser_source(
    app: tauri::AppHandle,
    state: tauri::State<'_, LinkVaultState>,
    source: BrowserSource,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let db_path = state.db_path.clone();
    let cancellation = state.reset_download_cancellation();
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
            "LinkVault refused to delete files outside the saved download folder.".to_string()
        })?;
        let mut components = relative.components();
        let first = match components.next() {
            Some(Component::Normal(value)) => value,
            _ => {
                return Err(
                    "LinkVault could not identify a safe course folder to delete.".to_string(),
                )
            }
        };
        if components.any(|component| !matches!(component, Component::Normal(_))) {
            return Err(
                "LinkVault refused to delete a course folder containing an unsafe path."
                    .to_string(),
            );
        }
        course_folders.insert(output_dir.join(first));
    }

    if course_folders.len() != 1 {
        return Err(
            "LinkVault could not identify one safe course folder for this completed download."
                .to_string(),
        );
    }
    let course_folder = course_folders.into_iter().next().expect("one folder");
    if course_folder == output_dir {
        return Err("LinkVault will never delete the selected download root.".to_string());
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
            "LinkVault refused to delete files outside the saved download root.".to_string(),
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

fn open_folder_in_file_explorer(folder: &Path) -> Result<(), String> {
    if !folder.is_dir() {
        return Err(format!(
            "Folder does not exist yet: {}",
            folder.to_string_lossy()
        ));
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(folder)
            .spawn()
            .map_err(|error| format!("Failed to open File Explorer: {error}"))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new("open")
            .arg(folder)
            .spawn()
            .or_else(|_| Command::new("xdg-open").arg(folder).spawn())
            .map_err(|error| format!("Failed to open folder: {error}"))?;
    }

    Ok(())
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

    let mut markdown = String::from("# LinkVault Download History\n\n");
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
    let scheduled_times =
        scheduled_download_times(request.schedule.as_ref(), &courses, created_at)?;

    let mut jobs = Vec::with_capacity(courses.len());
    for (index, course) in courses.iter().enumerate() {
        let scheduled_at = scheduled_times[index];
        let job_id = unique_job_id(connection, created_at, &course.slug, index)?;
        insert_job(
            connection,
            &JobRecord {
                id: job_id.clone(),
                course_slug: course.slug.clone(),
                source_url: course.normalized_url.clone(),
                status: "queued".to_string(),
                selected_quality: request.selected_quality.clone(),
                download_videos: request.download_videos,
                download_exercises: request.download_exercises,
                download_subtitles: request.download_subtitles,
                download_quizzes: request.download_quizzes,
                quiz_hints_json: course_quiz_hints_json(course),
                output_dir: request.output_dir.clone(),
                paused: false,
                scheduled_at,
                created_at,
                updated_at: created_at,
            },
        )
        .map_err(|error| error.to_string())?;
        append_job_event(
            connection,
            &NewJobEvent {
                job_id: job_id.clone(),
                event_type: if scheduled_at.is_some() {
                    "job.scheduled"
                } else {
                    "job.queued"
                }
                .to_string(),
                message: if let Some(run_at) = scheduled_at {
                    format!(
                        "Scheduled LinkedIn Learning course {} for {}.",
                        course.slug,
                        format_unix_timestamp_utc(run_at)
                    )
                } else {
                    format!("Queued LinkedIn Learning course: {}", course.slug)
                },
                payload_json: Some(
                    serde_json::json!({
                        "sourceUrl": course.normalized_url,
                        "quizUrls": course.quiz_urls,
                        "assessmentUrns": course.assessment_urns,
                        "delaySeconds": request.delay_seconds,
                        "scheduledAt": scheduled_at,
                    })
                    .to_string(),
                ),
                created_at,
            },
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
    connection: &Connection,
    created_at: i64,
    course_slug: &str,
    index: usize,
) -> Result<String, String> {
    let base = format!(
        "job-{created_at}-{}-{index}",
        sanitize_identifier_fragment(course_slug)
    );
    if get_job(connection, &base)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Ok(base);
    }

    for suffix in 2..=10_000 {
        let candidate = format!("{base}-{suffix}");
        if get_job(connection, &candidate)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(candidate);
        }
    }
    Err("Could not allocate a unique download job identifier.".to_string())
}

fn scheduled_download_times(
    schedule: Option<&DownloadScheduleRequest>,
    courses: &[CourseUrl],
    created_at: i64,
) -> Result<Vec<Option<i64>>, String> {
    let Some(schedule) = schedule else {
        return Ok(vec![None; courses.len()]);
    };
    if schedule.window_hours == 0 || schedule.window_hours > 168 {
        return Err("Schedule window must be between 1 and 168 hours.".to_string());
    }
    if schedule.min_wait_minutes == 0 || schedule.min_wait_minutes > 1_440 {
        return Err("Minimum random wait must be between 1 and 1440 minutes.".to_string());
    }
    if schedule.max_wait_minutes < schedule.min_wait_minutes || schedule.max_wait_minutes > 1_440 {
        return Err(
            "Maximum random wait must be at least the minimum and no more than 1440 minutes."
                .to_string(),
        );
    }

    let window_minutes = u64::from(schedule.window_hours) * 60;
    let minimum_required = u64::from(schedule.min_wait_minutes) * courses.len() as u64;
    if minimum_required > window_minutes {
        return Err(format!(
            "The schedule needs at least {} hours for {} courses at a {} minute minimum wait.",
            (minimum_required + 59) / 60,
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
        Self {
            output_dir: request.output_dir.clone(),
            selected_quality: request.selected_quality.clone(),
            delay_seconds: request.delay_seconds,
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

fn load_bootstrap_state(
    connection: &Connection,
    has_saved_token: bool,
    download_history_file_path: &Path,
) -> Result<BootstrapState, String> {
    let saved_download_preferences = get_setting(connection, "download.preferences")
        .map_err(|error| error.to_string())?
        .and_then(|setting| serde_json::from_str(&setting.value_json).ok());
    let recent_jobs = bootstrap_jobs(connection).map_err(|error| error.to_string())?;
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
        let cached_course = get_course_cache_entry(connection, &job.course_slug)
            .ok()
            .flatten();
        let thumbnail_url = cached_course
            .as_ref()
            .and_then(|entry| cached_course_thumbnail_url(&entry.payload_json));
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
            paused: job.paused,
            scheduled_at: job.scheduled_at,
            created_at: job.created_at,
            updated_at: job.updated_at,
            artifact_counts,
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
        get_setting, initialize, list_job_events, list_jobs_by_status, transition_job_status,
        upsert_artifact, ArtifactRecord,
    };
    use crate::course::CourseFetchError;

    fn initialized_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn queue_download_jobs_persists_safe_settings_jobs_and_events() {
        let connection = initialized_connection();

        let response = queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls: "linkedin.com/learning/sample-course\nhttps://www.linkedin.com/learning/second-course?trk=share".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 2,
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
        let queued_jobs = list_jobs_by_status(&connection, "queued").unwrap();
        let first_events = list_job_events(&connection, &response.jobs[0].id).unwrap();

        assert_eq!(response.jobs.len(), 2);
        assert_eq!(response.jobs[0].id, "job-1700000000-sample-course-0");
        assert_eq!(response.jobs[0].course_slug, "sample-course");
        assert_eq!(response.jobs[1].course_slug, "second-course");
        assert_eq!(setting.key, "download.preferences");
        assert!(setting.value_json.contains(r#""outputDir":"C:/downloads""#));
        assert!(setting.value_json.contains(r#""selectedQuality":"1080""#));
        assert!(!setting.value_json.to_ascii_lowercase().contains("li_at"));
        assert!(!setting.value_json.to_ascii_lowercase().contains("token"));
        assert_eq!(queued_jobs.len(), 2);
        assert_eq!(
            queued_jobs[0].source_url,
            "https://www.linkedin.com/learning/sample-course"
        );
        assert_eq!(queued_jobs[0].selected_quality, "1080");
        assert!(queued_jobs[0].download_videos);
        assert!(queued_jobs[0].download_exercises);
        assert!(!queued_jobs[0].download_subtitles);
        assert!(queued_jobs[0].download_quizzes);
        assert_eq!(first_events.len(), 1);
        assert_eq!(first_events[0].event_type, "job.queued");
        assert!(first_events[0].message.contains("sample-course"));
    }

    #[test]
    fn queue_download_jobs_persists_randomized_schedule_inside_window() {
        let connection = initialized_connection();
        let created_at = 1_700_000_000;

        let response = queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/first-course\nhttps://www.linkedin.com/learning/second-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: Some(DownloadScheduleRequest {
                    window_hours: 2,
                    min_wait_minutes: 10,
                    max_wait_minutes: 30,
                }),
            },
            created_at,
        )
        .unwrap();

        let jobs = list_jobs_by_status(&connection, "queued").unwrap();
        assert_eq!(response.jobs.len(), 2);
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|job| job.scheduled_at.is_some()));
        assert!(jobs[0].scheduled_at.unwrap() >= created_at + 10 * 60);
        assert!(jobs[1].scheduled_at.unwrap() > jobs[0].scheduled_at.unwrap());
        assert!(jobs[1].scheduled_at.unwrap() <= created_at + 2 * 60 * 60);
        assert!(list_ready_queued_jobs(&connection, created_at + 9 * 60)
            .unwrap()
            .is_empty());
        let events = list_job_events(&connection, &jobs[0].id).unwrap();
        assert_eq!(events[0].event_type, "job.scheduled");
        assert!(events[0]
            .payload_json
            .as_deref()
            .unwrap()
            .contains("scheduledAt"));
    }

    #[test]
    fn queue_download_jobs_rejects_schedule_window_shorter_than_minimum_waits() {
        let connection = initialized_connection();
        let course_urls = (0..5)
            .map(|index| format!("https://www.linkedin.com/learning/course-{index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let error = queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls,
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: Some(DownloadScheduleRequest {
                    window_hours: 1,
                    min_wait_minutes: 15,
                    max_wait_minutes: 30,
                }),
            },
            100,
        )
        .unwrap_err();

        assert!(error.contains("at least 2 hours"));
        assert!(list_jobs_by_status(&connection, "queued")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn queue_download_jobs_persists_direct_quiz_hints() {
        let connection = initialized_connection();

        queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course/quiz/urn:li:learningApiAssessment:69813586?resume=false&u=52983649&trk=ignored".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
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

        let queued_job = list_jobs_by_status(&connection, "queued")
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let hints = quiz_hints_from_json(&queued_job.quiz_hints_json);

        assert_eq!(
            queued_job.source_url,
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
        let connection = initialized_connection();

        let error = queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
                output_dir: " ".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
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
        let connection = initialized_connection();
        let response = queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "720".to_string(),
                delay_seconds: 5,
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
        transition_job_status(
            &connection,
            &response.jobs[0].id,
            "active",
            1_700_000_010,
            Some("Started metadata fetch."),
        )
        .unwrap();
        transition_job_status(
            &connection,
            &response.jobs[0].id,
            "failed",
            1_700_000_020,
            Some("Metadata fetch failed."),
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
            true,
            Path::new("C:/downloads/download-history.md"),
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
        assert_eq!(bootstrap.persisted_jobs[0].status, "failed");
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.total, 5);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.completed, 3);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.failed, 1);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.cancelled, 1);
        assert_eq!(bootstrap.persisted_jobs[0].artifact_counts.video_total, 2);
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
        assert_eq!(bootstrap.recent_events.len(), 3);
        assert_eq!(bootstrap.recent_events[0].event_type, "job.failed");
        assert_eq!(bootstrap.recent_events[0].message, "Metadata fetch failed.");
        assert_eq!(bootstrap.recent_events[0].job_id, response.jobs[0].id);
        assert_eq!(
            bootstrap.persisted_jobs[0].source_url,
            "https://www.linkedin.com/learning/sample-course"
        );
    }

    #[test]
    fn bootstrap_state_keeps_large_pending_queue_uncapped() {
        let connection = initialized_connection();
        let course_urls = (0..105)
            .map(|index| format!("https://www.linkedin.com/learning/course-{index:03}"))
            .collect::<Vec<_>>()
            .join("\n");

        queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls,
                output_dir: "C:/downloads".to_string(),
                selected_quality: "720".to_string(),
                delay_seconds: 0,
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
            true,
            Path::new("C:/downloads/download-history.md"),
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
        let connection = initialized_connection();
        let scheduled_response = queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/scheduled-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                schedule: Some(DownloadScheduleRequest {
                    window_hours: 2,
                    min_wait_minutes: 30,
                    max_wait_minutes: 30,
                }),
            },
            100,
        )
        .unwrap();
        let immediate_response = queue_download_jobs(
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/immediate-course".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "1080".to_string(),
                delay_seconds: 0,
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

        let ready = list_ready_queued_jobs(&connection, 100).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, immediate_response.jobs[0].id);

        let scheduled = get_job(&connection, &scheduled_response.jobs[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(scheduled.status, "queued");
        assert_eq!(scheduled.scheduled_at, Some(100 + 30 * 60));
    }

    #[test]
    fn duplicate_course_requests_in_the_same_second_keep_distinct_jobs() {
        let connection = initialized_connection();
        let request = || StartDownloadRequest {
            course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
            output_dir: "C:/downloads".to_string(),
            selected_quality: "1080".to_string(),
            delay_seconds: 0,
            browser_source: "Chrome".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            schedule: None,
        };

        let first = queue_download_jobs(&connection, request(), 100).unwrap();
        let second = queue_download_jobs(&connection, request(), 100).unwrap();

        assert_ne!(first.jobs[0].id, second.jobs[0].id);
        assert_eq!(list_jobs_by_status(&connection, "queued").unwrap().len(), 2);
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

        assert!(markdown.contains("# LinkVault Download History"));
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
