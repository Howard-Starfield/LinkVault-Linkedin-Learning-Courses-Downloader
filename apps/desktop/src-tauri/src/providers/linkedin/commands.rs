use super::expansion::{
    classify_learning_urls, expand_learning_urls, ClassifiedPaste, ExpansionError,
    ExpansionSummary, PathCapture, SchedulePolicy,
};
use super::path_library::{
    CatalogEntry, CoursePlayback, CourseSlug, PathLibrary, PlaybackTick, VideoProgress, VideoSlug,
};
use super::placement::OutputRoot;
use crate::app::database_writer::DatabaseWriter;
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
    list_jobs_by_status, list_ready_queued_jobs, list_recent_job_events, list_recent_jobs,
    open_runtime, remove_completed_download_job, remove_download_job, set_all_download_jobs_paused,
    set_download_job_paused, upsert_setting_json, DownloadHistoryEntry, JobRecord, NewJobEvent,
    ProviderResetCounts,
};
use crate::course::CourseApiClient;
use crate::download_orchestrator::process_next_queued_job_and_download_artifacts_with_quiz_assessments;
use crate::linkedin::CourseUrl;
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

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    #[cfg(test)]
    fn is_download_cancellation_requested(&self) -> bool {
        self.download_cancellation.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn token_path(&self) -> &std::path::Path {
        &self.token_path
    }

    /// Owned handle for async commands that must move their body into
    /// `tauri::async_runtime::spawn_blocking`.
    fn command_handles(&self) -> LinkedInCommandHandles {
        LinkedInCommandHandles {
            db_path: self.db_path.clone(),
            token_path: self.token_path.clone(),
            download_cancellation: Arc::clone(&self.download_cancellation),
            download_paused: Arc::clone(&self.download_paused),
        }
    }
}

/// Owned view of the mutable slots `LinkVaultState` shares with the download
/// executor.
///
/// `tauri::State<'_, LinkVaultState>` borrows the managed state for the life of
/// the command and so cannot be moved into the `'static` closure
/// `spawn_blocking` requires. The `Arc` slots here are the *same* allocations
/// the state holds, not copies, so a pause or cancellation requested inside the
/// closure is immediately visible to an in-flight download; the `PathBuf`s are
/// plain owned values. This is deliberate duplication of cheap handles, not a
/// second owner of any resource.
#[derive(Clone)]
struct LinkedInCommandHandles {
    db_path: PathBuf,
    token_path: PathBuf,
    download_cancellation: Arc<AtomicBool>,
    download_paused: Arc<AtomicBool>,
}

impl LinkedInCommandHandles {
    fn connection(&self) -> Result<Connection, String> {
        open_runtime(&self.db_path).map_err(|error| error.to_string())
    }

    fn has_saved_token(&self) -> bool {
        token_store::has_saved_token(&self.token_path)
    }

    fn is_download_paused(&self) -> bool {
        self.download_paused.load(Ordering::SeqCst)
    }

    fn set_download_paused(&self, paused: bool) {
        self.download_paused.store(paused, Ordering::SeqCst);
    }

    fn request_download_cancellation(&self) {
        self.download_cancellation.store(true, Ordering::SeqCst);
    }

    fn reset_download_cancellation(&self) {
        self.download_cancellation.store(false, Ordering::SeqCst);
        self.download_paused.store(false, Ordering::SeqCst);
    }

    fn history_file_path(&self) -> PathBuf {
        download_history_file_path_for_db(&self.db_path)
    }

    /// The read every queue command returns: connection plus the whole
    /// bootstrap projection. Runs inside the caller's blocking closure.
    fn bootstrap_state(&self, runtime: &WorkflowRuntime) -> Result<BootstrapState, String> {
        let connection = self.connection()?;
        load_bootstrap_state(
            &connection,
            Some(runtime),
            self.has_saved_token(),
            &self.history_file_path(),
            self.is_download_paused(),
        )
    }

    /// Mirrors `LinkVaultState::is_download_cancellation_requested` so a test
    /// can prove the handle and the state observe the same atomic.
    #[cfg(test)]
    fn cancellation_requested(&self) -> bool {
        self.download_cancellation.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn token_path(&self) -> &Path {
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
    /// When true, completed DB/FS matches may be re-queued. Active/queued matches still skip.
    #[serde(default)]
    force_redownload: bool,
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
pub struct SkippedDownloadCourse {
    course_slug: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartDownloadResponse {
    jobs: Vec<QueuedDownloadJob>,
    skipped: Vec<SkippedDownloadCourse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expansion: Option<ExpansionSummary>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedInDestinationCommit {
    output_dir: String,
    imported: usize,
    skipped: usize,
    already_known: usize,
    bootstrap: BootstrapState,
}

#[tauri::command]
pub async fn bootstrap_state(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    // The whole read (SQLite open, job/artifact/event projection, DPAPI token
    // check) is blocking and used to run on the UI thread, stalling the window
    // for its whole duration on every 15-second frontend poll.
    tauri::async_runtime::spawn_blocking(move || handles.bootstrap_state(&runtime))
        .await
        .map_err(|error| error.to_string())?
}

/// Lean probe: any LinkedIn workflow/legacy job that is active or ready-queued.
/// Avoids the N+1 artifact/event load of `bootstrap_state`.
#[tauri::command]
pub fn linkedin_queue_busy(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<bool, String> {
    let connection = state.connection()?;
    linkedin_queue_is_busy(&runtime, &connection, now_unix_timestamp())
}

#[tauri::command]
pub fn parse_linkedin_course_urls(input: String) -> Result<ClassifiedPaste, String> {
    classify_learning_urls(&input).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandLearningUrlsRequest {
    input: String,
    #[serde(default)]
    browser_source: Option<String>,
}

#[tauri::command]
pub async fn expand_linkedin_learning_urls(
    state: tauri::State<'_, LinkVaultState>,
    request: ExpandLearningUrlsRequest,
) -> Result<ExpansionSummary, String> {
    let classified = classify_learning_urls(&request.input).map_err(|error| error.to_string())?;
    if classified.schedule_policy == SchedulePolicy::KnownCount {
        return Ok(ExpansionSummary {
            paste_ref_count: classified.refs.len(),
            path_count: 0,
            unique_course_count: classified.course_count as usize,
            failed_paths: Vec::new(),
        });
    }

    let token_path = state.token_path.clone();
    let browser_source = request
        .browser_source
        .unwrap_or_else(|| "Chrome".to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let (token, session) = resolve_linkedin_session(&token_path, &browser_source)?;
        let mut client = AuthenticatedLinkedInClient::new(&token, &session)
            .map_err(|error| error.to_string())?;
        let catalog = expand_learning_urls(&mut client, &classified.refs)
            .map_err(|error| error.to_string())?;
        Ok(catalog.summary)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackTickDto {
    course: String,
    video: String,
    position_ms: i64,
    duration_ms: i64,
}

#[tauri::command]
pub fn linkedin_list_catalog(
    state: tauri::State<'_, LinkVaultState>,
    writer: tauri::State<'_, DatabaseWriter>,
) -> Result<Vec<CatalogEntry>, String> {
    let connection = state.connection()?;
    PathLibrary::new(writer.inner().clone())
        .list_catalog(&connection)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn linkedin_open_course(
    state: tauri::State<'_, LinkVaultState>,
    writer: tauri::State<'_, DatabaseWriter>,
    course_slug: String,
) -> Result<CoursePlayback, String> {
    let course = CourseSlug::parse(&course_slug).map_err(|error| error.to_string())?;
    let connection = state.connection()?;
    PathLibrary::new(writer.inner().clone())
        .open_course(&connection, course)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn linkedin_save_progress(
    writer: tauri::State<'_, DatabaseWriter>,
    tick: PlaybackTickDto,
) -> Result<VideoProgress, String> {
    let playback = PlaybackTick {
        course: CourseSlug::parse(&tick.course).map_err(|error| error.to_string())?,
        video: VideoSlug::parse(&tick.video).map_err(|error| error.to_string())?,
        position_ms: tick.position_ms,
        duration_ms: tick.duration_ms,
    };
    PathLibrary::new(writer.inner().clone())
        .save_progress(playback)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn linkedin_open_course_folder(
    writer: tauri::State<'_, DatabaseWriter>,
    course_slug: String,
) -> Result<(), String> {
    let course = CourseSlug::parse(&course_slug).map_err(|error| error.to_string())?;
    PathLibrary::new(writer.inner().clone())
        .open_course_folder(course)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn linkedin_add_course_to_path(
    state: tauri::State<'_, LinkVaultState>,
    writer: tauri::State<'_, DatabaseWriter>,
    course_slug: String,
    path_slug: String,
) -> Result<Vec<CatalogEntry>, String> {
    let course = CourseSlug::parse(&course_slug).map_err(|error| error.to_string())?;
    let path =
        super::path_library::PathSlug::parse(&path_slug).map_err(|error| error.to_string())?;
    let library = PathLibrary::new(writer.inner().clone());
    library
        .add_course_to_path(course, path)
        .map_err(|error| error.to_string())?;
    let connection = state.connection()?;
    library
        .list_catalog(&connection)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn quality_fallback_order(selected: VideoQuality) -> Vec<VideoQuality> {
    fallback_order(selected)
}

#[tauri::command]
pub async fn start_download_jobs(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    writer: tauri::State<'_, DatabaseWriter>,
    request: StartDownloadRequest,
) -> Result<StartDownloadResponse, String> {
    let output_dir = request.output_dir.clone();
    let classified =
        classify_learning_urls(&request.course_urls).map_err(|error| error.to_string())?;
    let path_library = PathLibrary::new(writer.inner().clone());
    let response = if classified.schedule_policy == SchedulePolicy::Discovering {
        let db_path = state.db_path.clone();
        let token_path = state.token_path.clone();
        let browser_source = request.browser_source.clone();
        let runtime = (*runtime).clone();
        tauri::async_runtime::spawn_blocking(move || {
            let connection = open_runtime(&db_path).map_err(|error| error.to_string())?;
            let (token, session) = resolve_linkedin_session(&token_path, &browser_source)?;
            let mut client = AuthenticatedLinkedInClient::new(&token, &session)
                .map_err(|error| error.to_string())?;
            queue_download_jobs_with_expander(
                &runtime,
                &connection,
                request,
                now_unix_timestamp(),
                Some(&mut client),
                Some(&path_library),
            )
        })
        .await
        .map_err(|error| error.to_string())??
    } else {
        let connection = state.connection()?;
        queue_download_jobs_with_expander(
            &runtime,
            &connection,
            request,
            now_unix_timestamp(),
            None,
            Some(&path_library),
        )
        .map_err(|error| error.to_string())?
    };
    recover_existing_linkedin_downloads(
        writer.inner().clone(),
        state.db_path.clone(),
        output_dir,
        now_unix_timestamp(),
    )
    .await;
    Ok(response)
}

#[tauri::command]
pub async fn save_download_preferences(
    state: tauri::State<'_, LinkVaultState>,
    writer: tauri::State<'_, DatabaseWriter>,
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

    {
        let connection = state.connection()?;
        persist_download_preferences(&connection, &preferences, now_unix_timestamp())?;
    }
    recover_existing_linkedin_downloads(
        writer.inner().clone(),
        state.db_path.clone(),
        preferences.output_dir.clone(),
        now_unix_timestamp(),
    )
    .await;
    Ok(preferences)
}

async fn recover_existing_linkedin_downloads(
    writer: DatabaseWriter,
    db_path: std::path::PathBuf,
    output_dir: String,
    now: i64,
) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        super::folder_import::recover_into(&writer, &output_dir, now)
    })
    .await;
    if let Ok(connection) = crate::cache::open_runtime(&db_path) {
        let history_file_path = download_history_file_path_for_db(&db_path);
        let _ = sync_download_history_file(&connection, &history_file_path);
    }
}

#[tauri::command]
pub async fn commit_linkedin_destination(
    state: tauri::State<'_, LinkVaultState>,
    writer: tauri::State<'_, DatabaseWriter>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    path: String,
) -> Result<LinkedInDestinationCommit, String> {
    let writer = writer.inner().clone();
    let now = now_unix_timestamp();
    let (output_dir, counts) = tauri::async_runtime::spawn_blocking(move || {
        super::folder_import::commit(&writer, &path, now)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;

    let connection = state.connection()?;
    let history_file_path = download_history_file_path_for_db(&state.db_path);
    let _ = sync_download_history_file(&connection, &history_file_path);
    let bootstrap = load_bootstrap_state(
        &connection,
        Some(&runtime),
        token_store::has_saved_token(&state.token_path),
        &history_file_path,
        state.is_download_paused(),
    )?;

    Ok(LinkedInDestinationCommit {
        output_dir,
        imported: counts.imported,
        skipped: counts.skipped,
        already_known: counts.already_known,
        bootstrap,
    })
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
                let _ =
                    persist_download_preferences(&connection, &preferences, now_unix_timestamp());
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
pub async fn set_download_job_pause(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
    paused: bool,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let connection = handles.connection()?;
        match set_download_job_paused(&connection, &job_id, paused, now_unix_timestamp()) {
            Ok(job) => {
                if job.status == "active" {
                    handles.set_download_paused(paused);
                }
            }
            Err(_) => {
                let now = now_unix_timestamp();
                if let Some(run) = runtime
                    .get_run(job_id.clone())
                    .map_err(|error| error.to_string())?
                {
                    match run.state {
                        RunState::Running | RunState::Cancelling => {
                            // Cooperative pause for the in-flight LinkedIn executor. The
                            // run stays Running so the job remains on the Active tab;
                            // bootstrap overlays this flag onto projected jobs.
                            handles.set_download_paused(paused);
                        }
                        RunState::Queued | RunState::Paused | RunState::RetryWait => {
                            runtime
                                .set_linkedin_run_paused(job_id, paused, now)
                                .map_err(|error| error.to_string())?;
                            // Queued workflow runs are not in the legacy jobs
                            // table. Keep the atomic flag clear for idle queue
                            // pauses so a later active download is not
                            // accidentally frozen.
                            if !paused {
                                handles.set_download_paused(false);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        load_bootstrap_state(
            &connection,
            Some(&runtime),
            handles.has_saved_token(),
            &handles.history_file_path(),
            handles.is_download_paused(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn set_all_downloads_paused(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    paused: bool,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let now = now_unix_timestamp();
        let connection = handles.connection()?;
        set_all_download_jobs_paused(&connection, paused, now).map_err(|error| error.to_string())?;
        runtime
            .set_all_queued_linkedin_runs_paused(paused, now)
            .map_err(|error| error.to_string())?;
        handles.set_download_paused(paused);
        load_bootstrap_state(
            &connection,
            Some(&runtime),
            handles.has_saved_token(),
            &handles.history_file_path(),
            handles.is_download_paused(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn reset_linkedin_database(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<ProviderResetCounts, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    // The whole reset is blocking: a SQLite open, a full-table delete write
    // transaction and a markdown rewrite on disk. It used to run on the Tauri
    // main thread, freezing the window for its whole duration on a large
    // install.
    tauri::async_runtime::spawn_blocking(move || {
        // The UI is expected to call set_all_downloads_paused(true) first so the
        // worker unwinds at a safe boundary. We still defensively re-arm the
        // flags here so a stale in-flight request can't keep writing after the
        // wipe commits. ORDER IS LOAD-BEARING: this must stay *before*
        // `delete_linkedin_runs` and before the wipe, or a request that is
        // already unwinding is re-armed and keeps writing into the tables the
        // wipe is deleting.
        handles.set_download_paused(true);
        runtime
            .delete_linkedin_runs()
            .map_err(|error| error.to_string())?;
        let connection = handles.connection()?;
        let counts =
            clear_linkedin_provider_data(&connection).map_err(|error| error.to_string())?;
        // Regenerate the history markdown so the next read sees a valid empty
        // document instead of rows that no longer exist in the database.
        let history_file_path = handles.history_file_path();
        let _ = sync_download_history_file(&connection, &history_file_path);
        // ORDER IS LOAD-BEARING: the flags are cleared only once the wipe and
        // the history rewrite have both committed, so the next download starts
        // from a database that is actually empty.
        handles.reset_download_cancellation();
        Ok(counts)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn retry_failed_download_job(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Cancel applies to the in-flight job only. Clear so a retry is not
        // immediately re-cancelled by a sticky shared flag.
        handles.reset_download_cancellation();
        let connection = handles.connection()?;
        retry_failed_download_job_inner(&runtime, &connection, job_id, now_unix_timestamp())?;
        let history_file_path = handles.history_file_path();
        // The markdown regeneration is a filesystem write; keeping it inside
        // this closure is what keeps it off the UI thread.
        let _ = sync_download_history_file(&connection, &history_file_path);
        load_bootstrap_state(
            &connection,
            Some(&runtime),
            handles.has_saved_token(),
            &history_file_path,
            handles.is_download_paused(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn clear_failed_download_jobs(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        runtime
            .delete_terminal_linkedin_runs()
            .map_err(|error| error.to_string())?;
        let connection = handles.connection()?;
        clear_failed_jobs(&connection).map_err(|error| error.to_string())?;
        let history_file_path = handles.history_file_path();
        let _ = sync_download_history_file(&connection, &history_file_path);
        load_bootstrap_state(
            &connection,
            Some(&runtime),
            handles.has_saved_token(),
            &history_file_path,
            handles.is_download_paused(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn remove_download_queue_item(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let now = now_unix_timestamp();
        let mut removed_workflow = false;
        let mut requested_cancellation = false;
        if let Some(run) = runtime
            .get_run(job_id.clone())
            .map_err(|error| error.to_string())?
        {
            if matches!(
                run.state,
                RunState::Running
                    | RunState::Cancelling
                    | RunState::Queued
                    | RunState::Paused
                    | RunState::RetryWait
            ) {
                if matches!(run.state, RunState::Running | RunState::Cancelling) {
                    handles.request_download_cancellation();
                    handles.set_download_paused(false);
                    requested_cancellation = true;
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
        let connection = handles.connection()?;
        match remove_download_job(&connection, &job_id) {
            Ok(job) => {
                if job.status == "active" {
                    handles.request_download_cancellation();
                    handles.set_download_paused(false);
                    requested_cancellation = true;
                }
            }
            Err(_error) if removed_workflow => {}
            Err(error) => return Err(error.to_string()),
        }
        // cancel_and_delete can remove the run without an executor unwind. Clear
        // the shared flag so the next queued course is not sticky-cancelled.
        if requested_cancellation {
            handles.reset_download_cancellation();
        }
        let history_file_path = handles.history_file_path();
        let _ = sync_download_history_file(&connection, &history_file_path);
        load_bootstrap_state(
            &connection,
            Some(&runtime),
            handles.has_saved_token(),
            &history_file_path,
            handles.is_download_paused(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn delete_completed_download(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let connection = handles.connection()?;
        let job = get_job(&connection, &job_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Download job was not found.".to_string())?;
        if job.status != "completed" {
            return Err("Only completed downloads can delete their course files."
                .to_string());
        }

        let artifacts =
            list_artifacts_for_job(&connection, &job.id).map_err(|error| error.to_string())?;
        // Recursive filesystem delete of the whole course folder.
        delete_completed_download_files(&job, &artifacts)?;
        remove_completed_download_job(&connection, &job.id).map_err(|error| error.to_string())?;
        let _ = runtime.delete_run_if_terminal(job_id);

        let history_file_path = handles.history_file_path();
        let _ = sync_download_history_file(&connection, &history_file_path);
        load_bootstrap_state(
            &connection,
            Some(&runtime),
            handles.has_saved_token(),
            &history_file_path,
            handles.is_download_paused(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn download_scheduled_job_now(
    state: tauri::State<'_, LinkVaultState>,
    runtime: tauri::State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let handles = state.command_handles();
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let connection = handles.connection()?;
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
        load_bootstrap_state(
            &connection,
            Some(&runtime),
            handles.has_saved_token(),
            &handles.history_file_path(),
            handles.is_download_paused(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
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

pub(crate) fn history_source_url(entry: &DownloadHistoryEntry) -> String {
    persisted_job_source_url(&entry.course_slug, &entry.source_url)
}

fn persisted_job_source_url(course_slug: &str, source_url: &str) -> String {
    if course_slug.starts_with("local:") {
        return String::new();
    }
    if source_url.trim().is_empty() {
        format!("https://www.linkedin.com/learning/{course_slug}")
    } else {
        source_url.to_string()
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

#[cfg(test)]
fn queue_download_jobs(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    request: StartDownloadRequest,
    created_at: i64,
) -> Result<StartDownloadResponse, String> {
    queue_download_jobs_with_expander(runtime, connection, request, created_at, None, None)
}

fn queue_download_jobs_with_expander(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    request: StartDownloadRequest,
    created_at: i64,
    expander: Option<&mut dyn CourseApiClient>,
    path_library: Option<&PathLibrary>,
) -> Result<StartDownloadResponse, String> {
    let classified =
        classify_learning_urls(&request.course_urls).map_err(|error| error.to_string())?;
    let (courses, expansion, paths, standalone) = resolve_download_catalog(classified, expander)?;
    if let Some(path_library) = path_library {
        capture_expanded_catalog(path_library, &request.output_dir, paths, standalone)?;
    }
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
    let mut skipped = Vec::new();
    for (index, course) in courses.iter().enumerate() {
        if let Some(reason) = linkedin_course_dedupe_skip(
            runtime,
            connection,
            &course.slug,
            &request.output_dir,
            request.force_redownload,
        )? {
            skipped.push(SkippedDownloadCourse {
                course_slug: course.slug.clone(),
                reason,
            });
            continue;
        }

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

    Ok(StartDownloadResponse {
        jobs,
        skipped,
        expansion,
    })
}

fn resolve_download_catalog(
    classified: ClassifiedPaste,
    expander: Option<&mut dyn CourseApiClient>,
) -> Result<
    (
        Vec<CourseUrl>,
        Option<ExpansionSummary>,
        Vec<PathCapture>,
        Vec<String>,
    ),
    String,
> {
    match classified.schedule_policy {
        SchedulePolicy::KnownCount => {
            let courses = classified.course_urls();
            let standalone = courses.iter().map(|course| course.slug.clone()).collect();
            Ok((courses, None, Vec::new(), standalone))
        }
        SchedulePolicy::Discovering => {
            let client = expander.ok_or_else(|| ExpansionError::SessionRequired.to_string())?;
            let catalog = expand_learning_urls(client, &classified.refs)
                .map_err(|error| error.to_string())?;
            Ok((
                catalog.courses,
                Some(catalog.summary),
                catalog.paths,
                catalog.standalone,
            ))
        }
    }
}

fn capture_expanded_catalog(
    path_library: &PathLibrary,
    output_dir: &str,
    paths: Vec<PathCapture>,
    standalone: Vec<String>,
) -> Result<(), String> {
    let output_root = OutputRoot::parse(output_dir).map_err(|error| error.to_string())?;
    path_library
        .ingest_expansion(output_root, paths, standalone)
        .map_err(|error| error.to_string())
}

fn resolve_linkedin_session(
    token_path: &Path,
    browser_source_label: &str,
) -> Result<(String, ValidatedLinkedInSession), String> {
    if let Ok(token) = token_store::load_token(token_path) {
        if let Ok(mut home_client) = ReqwestLinkedInHomeClient::new() {
            if let Ok(session) = validate_li_at_with_client(&token, &mut home_client) {
                return Ok((token, session));
            }
        }
    }

    let source = browser_source_from_label(browser_source_label)?;
    let roots = BrowserCookieRoots::from_env();
    let decoder = chromium_user_data_path_for_source(source, &roots)
        .map(|path| ChromiumCookieDecoder::from_user_data_path(&path))
        .unwrap_or_else(ChromiumCookieDecoder::disabled);
    let candidates =
        read_li_at_candidates(source, &roots, &decoder).map_err(|error| error.to_string())?;
    let mut home_client = ReqwestLinkedInHomeClient::new().map_err(|error| error.to_string())?;
    select_first_valid_browser_token(&candidates, &mut home_client)
        .map(|(candidate, session)| (candidate.value, session))
        .map_err(|_| ExpansionError::SessionRequired.to_string())
}

fn browser_source_from_label(label: &str) -> Result<BrowserSource, String> {
    match label.trim() {
        "Chrome" => Ok(BrowserSource::Chrome),
        "Edge" => Ok(BrowserSource::Edge),
        "Firefox" => Ok(BrowserSource::Firefox),
        _ => Err(ExpansionError::SessionRequired.to_string()),
    }
}

fn linkedin_queue_is_busy(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    now: i64,
) -> Result<bool, String> {
    for run in runtime
        .list_linkedin_runs(250)
        .map_err(|error| error.to_string())?
    {
        if linkedin_projected_job_is_busy(&super::projection::job_from_run(&run), now) {
            return Ok(true);
        }
    }

    for status in ["active", "queued"] {
        for job in list_jobs_by_status(connection, status).map_err(|error| error.to_string())? {
            if linkedin_projected_job_is_busy(&job, now) {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn linkedin_projected_job_is_busy(job: &JobRecord, now: i64) -> bool {
    let status = job.status.to_ascii_lowercase();
    if status == "active" {
        return true;
    }
    status == "queued" && !job.paused && job.scheduled_at.map(|at| at <= now).unwrap_or(true)
}

fn linkedin_course_dedupe_skip(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    course_slug: &str,
    output_dir: &str,
    force_redownload: bool,
) -> Result<Option<String>, String> {
    if let Some(reason) = linkedin_db_course_conflict(runtime, connection, course_slug, output_dir)?
    {
        let allow_completed = force_redownload && reason == "already_completed";
        if !allow_completed {
            return Ok(Some(reason));
        }
    }

    if !force_redownload && linkedin_slug_course_folder_exists(output_dir, course_slug) {
        return Ok(Some("folder_exists".to_string()));
    }

    Ok(None)
}

fn linkedin_db_course_conflict(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    course_slug: &str,
    output_dir: &str,
) -> Result<Option<String>, String> {
    for run in runtime
        .list_linkedin_runs(250)
        .map_err(|error| error.to_string())?
    {
        let job = super::projection::job_from_run(&run);
        if let Some(reason) = dedupe_reason_for_job(&job, course_slug, output_dir) {
            return Ok(Some(reason));
        }
    }

    for status in ["active", "queued", "completed"] {
        for job in list_jobs_by_status(connection, status).map_err(|error| error.to_string())? {
            if let Some(reason) = dedupe_reason_for_job(&job, course_slug, output_dir) {
                return Ok(Some(reason));
            }
        }
    }

    Ok(None)
}

fn dedupe_reason_for_job(job: &JobRecord, course_slug: &str, output_dir: &str) -> Option<String> {
    if job.course_slug != course_slug || !output_dirs_match(&job.output_dir, output_dir) {
        return None;
    }
    match job.status.to_ascii_lowercase().as_str() {
        "active" => Some("already_active".to_string()),
        "queued" => Some("already_queued".to_string()),
        "completed" => Some("already_completed".to_string()),
        _ => None,
    }
}

fn output_dirs_match(left: &str, right: &str) -> bool {
    let left = left.trim().trim_end_matches(['/', '\\']);
    let right = right.trim().trim_end_matches(['/', '\\']);
    left.eq_ignore_ascii_case(right)
}

/// Best-effort FS match: a child folder named exactly as the course slug that
/// already looks like a LinkedIn course (Study.md or chapter media). Title-only
/// folders cannot be matched by slug and are covered by DB dedupe instead.
fn linkedin_slug_course_folder_exists(output_dir: &str, course_slug: &str) -> bool {
    if course_slug.trim().is_empty() {
        return false;
    }
    let folder = Path::new(output_dir).join(course_slug);
    let Ok(metadata) = fs::symlink_metadata(&folder) else {
        return false;
    };
    if !metadata.is_dir() {
        return false;
    }
    if folder.join("Study.md").is_file() {
        return true;
    }
    let Ok(entries) = fs::read_dir(&folder) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(entry_meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !entry_meta.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !name.as_bytes().first().is_some_and(|b| b.is_ascii_digit()) {
            continue;
        }
        if directory_has_linkedin_media(&path) {
            return true;
        }
    }
    false
}

fn directory_has_linkedin_media(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let file_path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&file_path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let file_name = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if file_name.ends_with(".mp4")
            || file_name.ends_with(".srt")
            || file_name.ends_with(".quiz.md")
        {
            return true;
        }
    }
    false
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
        let output_root = run.output_root.clone();
        let course_slug = request.course_slug.clone();
        let request_json = serde_json::to_string(&request).map_err(|error| error.to_string())?;
        // Drop the terminal run and any mirrored legacy row so Failed no longer
        // lists this attempt after it returns to Queue.
        runtime
            .delete_run_if_terminal(job_id.clone())
            .map_err(|error| error.to_string())?;
        let _ = remove_download_job(connection, &job_id);
        runtime
            .submit_linkedin_download(job_id, course_slug, request_json, output_root, now, None)
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
    let course_slug = legacy.course_slug.clone();
    let output_dir = legacy.output_dir.clone();
    let request_json = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    remove_download_job(connection, &job_id).map_err(|error| error.to_string())?;
    runtime
        .submit_linkedin_download(job_id, course_slug, request_json, output_dir, now, None)
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
        .and_then(|setting| {
            serde_json::from_str::<SavedDownloadPreferences>(&setting.value_json).ok()
        });
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
    // One ordered read of the newest 20 events for the whole database.
    //
    // This used to loop over every job, load every one of that job's events,
    // sort the merged vector by `created_at DESC, id DESC` and keep 20. At 500
    // jobs that deserialised 60,000 rows to return 20 and cost 74.92 ms of a
    // 155.62 ms call on a debug build.
    //
    // `list_recent_job_events` issues the same comparator in SQL against
    // `idx_job_events_recent (created_at DESC, id DESC)`, so it reads only the
    // 20 index rows it returns. The projection below is unchanged, so the
    // observable `recent_events` output is identical.
    let recent_events = list_recent_job_events(connection, 20)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|event| PersistedJobEvent {
            id: event.id,
            job_id: event.job_id,
            event_type: event.event_type,
            message: event.message,
            payload_json: event.payload_json,
            created_at: event.created_at,
        })
        .collect();

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
            source_url: persisted_job_source_url(&job.course_slug, &job.source_url),
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

pub(crate) fn bootstrap_jobs(
    connection: &Connection,
) -> Result<Vec<JobRecord>, crate::cache::CacheError> {
    let mut jobs = Vec::new();
    let mut seen = HashSet::new();

    for status in ["active", "queued", "failed", "cancelled", "completed"] {
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
    use std::collections::HashMap;

    struct ScriptedClient {
        pages: HashMap<String, Result<String, u16>>,
    }

    impl CourseApiClient for ScriptedClient {
        fn get(&mut self, url: &str) -> Result<String, CourseFetchError> {
            match self.pages.get(url) {
                Some(Ok(body)) => Ok(body.clone()),
                Some(Err(status)) => Err(CourseFetchError::Http { status: *status }),
                None => Err(CourseFetchError::Http { status: 404 }),
            }
        }
    }

    fn initialized_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
    }

    fn workflow_harness() -> (tempfile::TempDir, WorkflowRuntime, Connection) {
        let (directory, runtime, connection, _writer) = workflow_harness_with_writer();
        (directory, runtime, connection)
    }

    fn workflow_harness_with_writer() -> (
        tempfile::TempDir,
        WorkflowRuntime,
        Connection,
        crate::app::database_writer::DatabaseWriter,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = crate::cache::initialize_database(&db_path).unwrap();
        drop(connection);
        let writer = crate::app::database_writer::DatabaseWriter::start(
            db_path.clone(),
            crate::app::database_diagnostics::DatabaseDiagnostics::default(),
        )
        .unwrap();
        let runtime = WorkflowRuntime::new(writer.clone());
        let connection = crate::cache::open_runtime(&db_path).unwrap();
        (directory, runtime, connection, writer)
    }

    #[test]
    fn discovering_paste_without_expander_requires_a_session() {
        let (_dir, runtime, connection) = workflow_harness();
        let error = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate".to_string(),
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
                force_redownload: false,
            },
            1_700_000_000,
        )
        .unwrap_err();
        assert!(error.contains("session"));
    }

    #[test]
    fn queue_download_jobs_schedules_two_slots_for_an_expanded_catalog() {
        let (_dir, runtime, connection) = workflow_harness();
        let created_at = 1_700_000_000;
        let path_html = r#"
            <script type="application/ld+json">
            {"@type":"ItemList","itemListElement":[
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/practical-github-actions"}},
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/practical-github-copilot"}}
            ]}
            </script>
        "#;
        let mut client = ScriptedClient {
            pages: HashMap::from([(
                "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate"
                    .to_string(),
                Ok(path_html.to_string()),
            )]),
        };

        let response = queue_download_jobs_with_expander(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate".to_string(),
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
                    window_minutes: 120,
                    min_wait_minutes: 10,
                    max_wait_minutes: 30,
                }),
                force_redownload: false,
            },
            created_at,
            Some(&mut client),
            None,
        )
        .unwrap();

        assert_eq!(response.jobs.len(), 2);
        assert_eq!(
            response
                .expansion
                .as_ref()
                .map(|summary| summary.unique_course_count),
            Some(2)
        );
        assert!(response.jobs.iter().all(|job| job.scheduled_at.is_some()));
        assert!(response.jobs[0].scheduled_at.unwrap() >= created_at + 10 * 60);
        assert!(response.jobs[1].scheduled_at.unwrap() > response.jobs[0].scheduled_at.unwrap());
        assert_ne!(response.jobs[0].course_slug, response.jobs[1].course_slug);
    }

    #[test]
    fn pausing_expanded_path_jobs_persists_and_can_resume() {
        let (_dir, runtime, connection) = workflow_harness();
        let path_html = r#"
            <script type="application/ld+json">
            {"@type":"ItemList","itemListElement":[
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/practical-github-actions"}},
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/practical-github-copilot"}}
            ]}
            </script>
        "#;
        let mut client = ScriptedClient {
            pages: HashMap::from([(
                "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate"
                    .to_string(),
                Ok(path_html.to_string()),
            )]),
        };
        let response = queue_download_jobs_with_expander(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate".to_string(),
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
                force_redownload: false,
            },
            1_700_000_000,
            Some(&mut client),
            None,
        )
        .unwrap();
        assert_eq!(response.jobs.len(), 2);

        runtime
            .set_all_queued_linkedin_runs_paused(true, 1_700_000_100)
            .unwrap();
        let paused: Vec<_> = runtime
            .list_linkedin_runs(20)
            .unwrap()
            .into_iter()
            .map(|run| super::super::projection::job_from_run(&run))
            .collect();
        assert_eq!(paused.len(), 2);
        assert!(paused
            .iter()
            .all(|job| job.paused && job.status == "queued"));

        runtime
            .set_linkedin_run_paused(paused[0].id.clone(), false, 1_700_000_200)
            .unwrap();
        let first = runtime.get_run(paused[0].id.clone()).unwrap().unwrap();
        let second = runtime.get_run(paused[1].id.clone()).unwrap().unwrap();
        assert_eq!(first.state, RunState::Queued);
        assert_eq!(second.state, RunState::Paused);

        runtime
            .set_all_queued_linkedin_runs_paused(false, 1_700_000_300)
            .unwrap();
        let resumed: Vec<_> = runtime
            .list_linkedin_runs(20)
            .unwrap()
            .into_iter()
            .map(|run| super::super::projection::job_from_run(&run))
            .collect();
        assert!(resumed
            .iter()
            .all(|job| !job.paused && job.status == "queued"));
    }

    #[test]
    fn overlapping_path_capture_keeps_one_job_and_two_memberships() {
        let (_dir, runtime, connection, writer) = workflow_harness_with_writer();
        let path_library = PathLibrary::new(writer);
        let path_a = r#"
            <script type="application/ld+json">
            {"@type":"ItemList","name":"Path A","itemListElement":[
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/shared-course"}},
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/only-a"}}
            ]}
            </script>
        "#;
        let path_b = r#"
            <script type="application/ld+json">
            {"@type":"ItemList","name":"Path B","itemListElement":[
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/shared-course"}},
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/only-b"}}
            ]}
            </script>
        "#;
        let mut client = ScriptedClient {
            pages: HashMap::from([
                (
                    "https://www.linkedin.com/learning/paths/path-a".to_string(),
                    Ok(path_a.to_string()),
                ),
                (
                    "https://www.linkedin.com/learning/paths/path-b".to_string(),
                    Ok(path_b.to_string()),
                ),
            ]),
        };

        let response = queue_download_jobs_with_expander(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/paths/path-a\nhttps://www.linkedin.com/learning/paths/path-b".to_string(),
                output_dir: "C:/downloads".to_string(),
                selected_quality: "720".to_string(),
                delay_seconds: 0,
                video_wait_min_seconds: 20,
                video_wait_max_seconds: 40,
                browser_source: "Chrome".to_string(),
                download_videos: true,
                download_exercises: false,
                download_subtitles: false,
                download_quizzes: false,
                schedule: None,
                force_redownload: false,
            },
            1_700_000_000,
            Some(&mut client),
            Some(&path_library),
        )
        .unwrap();

        assert_eq!(response.jobs.len(), 3);
        let memberships: Vec<String> = connection
            .prepare(
                "SELECT path_slug FROM linkedin_path_membership
                 WHERE course_slug = 'shared-course' ORDER BY path_slug",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            memberships,
            vec!["path-a".to_string(), "path-b".to_string()]
        );
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
                force_redownload: false,
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
                force_redownload: false,
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
                force_redownload: false,
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
                force_redownload: false,
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
                force_redownload: false,
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
                force_redownload: false,
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
                force_redownload: false,
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
                force_redownload: false,
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

    // ---------------------------------------------------------------------
    // `recent_events` used to be built by loading every event of every job,
    // sorting the merged vector and truncating to 20. It is now a single
    // ordered, capped query. These tests pin the observable output so the
    // cheaper path cannot silently change what the UI renders.
    // ---------------------------------------------------------------------

    const EVENT_FIXTURE_JOBS: usize = 6;
    /// Deliberately few distinct timestamps so `created_at` ties are common
    /// and the `id DESC` tie-break is actually exercised.
    const EVENT_FIXTURE_TIMESTAMP_SLOTS: i64 = 4;
    const EVENT_FIXTURE_EVENTS_PER_JOB: usize = 25;

    /// Seeds events whose `created_at` values collide across jobs, so ordering
    /// by `created_at` alone is ambiguous and only the `id` tie-break resolves
    /// it.
    fn seed_tied_event_log(connection: &Connection) {
        for job_index in 0..EVENT_FIXTURE_JOBS {
            let job_id = format!("event-fixture-job-{job_index:02}");
            let created_at = 1_700_000_000 + job_index as i64;
            crate::cache::insert_job(
                connection,
                &JobRecord {
                    id: job_id.clone(),
                    course_slug: format!("event-fixture-course-{job_index:02}"),
                    source_url: format!(
                        "https://www.linkedin.com/learning/event-fixture-course-{job_index:02}"
                    ),
                    status: "completed".to_string(),
                    selected_quality: "1080".to_string(),
                    download_videos: true,
                    download_exercises: true,
                    download_subtitles: true,
                    download_quizzes: true,
                    quiz_hints_json: "[]".to_string(),
                    output_dir: "C:/downloads".to_string(),
                    paused: false,
                    scheduled_at: None,
                    created_at,
                    updated_at: created_at + 1,
                },
            )
            .unwrap();

            for event_index in 0..EVENT_FIXTURE_EVENTS_PER_JOB {
                // `created_at` only takes EVENT_FIXTURE_TIMESTAMP_SLOTS distinct
                // values, so the newest 20 events are full of ties.
                let event_created_at = 1_700_000_000
                    + (event_index % EVENT_FIXTURE_TIMESTAMP_SLOTS as usize) as i64;
                append_job_event(
                    connection,
                    &NewJobEvent {
                        job_id: job_id.clone(),
                        event_type: "artifact.started".to_string(),
                        message: format!("{job_id} step {event_index}"),
                        payload_json: Some(format!("{{\"step\":{event_index}}}")),
                        created_at: event_created_at,
                    },
                )
                .unwrap();
            }
        }
    }

    /// Recomputes `recent_events` the way the pre-optimisation loop did: one
    /// query per job, merged, sorted by `created_at DESC, id DESC`, truncated
    /// to 20. Used as the oracle for the single-query replacement.
    #[allow(dead_code)]
    fn recent_events_via_per_job_n_plus_one(connection: &Connection) -> Vec<PersistedJobEvent> {
        let jobs = bootstrap_jobs(connection).unwrap();
        let mut events: Vec<PersistedJobEvent> = Vec::new();
        for job in &jobs {
            events.extend(
                list_job_events(connection, &job.id)
                    .unwrap()
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
        events.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        events.truncate(20);
        events
    }

    #[test]
    fn bootstrap_state_recent_events_are_exactly_twenty_newest_first_with_id_tie_break() {
        let (_dir, runtime, connection) = workflow_harness();
        seed_tied_event_log(&connection);

        let bootstrap = load_bootstrap_state(
            &connection,
            Some(&runtime),
            true,
            Path::new("C:/downloads/download-history.md"),
            false,
        )
        .unwrap();

        // The dataset is 150 events, so a cap of 20 is genuinely binding.
        let total_events = connection
            .query_row("SELECT COUNT(*) FROM job_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(total_events, 150);
        assert_eq!(bootstrap.recent_events.len(), 20);

        // Newest first, and ties on `created_at` broken by the higher rowid.
        for pair in bootstrap.recent_events.windows(2) {
            let (left, right) = (&pair[0], &pair[1]);
            assert!(
                left.created_at > right.created_at
                    || (left.created_at == right.created_at && left.id > right.id),
                "recent_events must be ordered by created_at DESC then id DESC, \
                 got {left:?} before {right:?}"
            );
        }

        // The 20 returned rows are genuinely the newest ones: every event not
        // returned is strictly older than the oldest returned event.
        let oldest_returned = bootstrap
            .recent_events
            .last()
            .expect("20 events were just asserted");
        let every_event = list_recent_job_events(&connection, 1_000).unwrap();
        assert_eq!(every_event.len(), 150);
        for event in &every_event {
            let is_returned = bootstrap
                .recent_events
                .iter()
                .any(|candidate| candidate.id == event.id);
            if is_returned {
                continue;
            }
            assert!(
                event.created_at < oldest_returned.created_at
                    || (event.created_at == oldest_returned.created_at
                        && event.id < oldest_returned.id),
                "event {event:?} was dropped but still sorts ahead of the \
                 oldest returned event {oldest_returned:?}"
            );
        }
    }

    #[test]
    fn bootstrap_state_recent_events_match_the_retired_per_job_merge() {
        let (_dir, runtime, connection) = workflow_harness();
        seed_tied_event_log(&connection);

        let bootstrap = load_bootstrap_state(
            &connection,
            Some(&runtime),
            true,
            Path::new("C:/downloads/download-history.md"),
            false,
        )
        .unwrap();
        let expected = recent_events_via_per_job_n_plus_one(&connection);

        assert_eq!(
            bootstrap.recent_events, expected,
            "the single capped query must return exactly what the retired \
             per-job merge + sort + truncate returned"
        );
    }

    #[test]
    fn load_bootstrap_state_reads_recent_events_in_one_query_not_one_per_job() {
        // Deterministic guard for the event N+1. A wall-clock bound can be
        // argued away by a slow machine; this cannot.
        let production = production_source();
        let load = production
            .split("fn load_bootstrap_state(")
            .nth(1)
            .and_then(|rest| rest.split("\nfn ").next())
            .unwrap_or_default();

        assert!(
            load.contains("let recent_events = list_recent_job_events(connection, 20)"),
            "the single capped ordered query must be the initializer of the \
             `recent_events` binding that is actually returned; a bare call to \
             list_recent_job_events that is discarded or dead would pass a \
             substring check"
        );
        // ... and that binding must reach the struct that is returned, not a
        // shadowed or unused local.
        let bound_at = load
            .find("let recent_events = list_recent_job_events(connection, 20)")
            .unwrap_or(0);
        assert!(
            load[bound_at..].contains("\n        recent_events,"),
            "the recent_events binding must be the one handed to the returned \
             BootstrapState"
        );
        assert!(
            !load.contains("list_job_events"),
            "load_bootstrap_state must not read job events one job at a time; \
             at 500 jobs that deserialised 60,000 rows to keep 20"
        );
        assert!(
            !load.contains("recent_events.sort_by"),
            "the in-memory sort is dead once the query returns ordered rows"
        );
        assert!(
            !load.contains("recent_events.truncate"),
            "the query is already capped; truncating again would be redundant"
        );
    }

    /// The commands that must not run blocking work on the Tauri main thread.
    /// Every one of them used to be a synchronous `#[tauri::command]`, so the
    /// whole bootstrap projection plus a filesystem rewrite of the history
    /// markdown ran on the main thread, stalling the window on every 15-second
    /// frontend poll.
    const OFF_UI_THREAD_COMMANDS: [&str; 8] = [
        "bootstrap_state",
        "set_download_job_pause",
        "set_all_downloads_paused",
        "retry_failed_download_job",
        "clear_failed_download_jobs",
        "remove_download_queue_item",
        "delete_completed_download",
        "reset_linkedin_database",
    ];

    /// The subset of `OFF_UI_THREAD_COMMANDS` that returns a `BootstrapState`
    /// and must therefore build that projection from the connection it opened
    /// inside the closure. `reset_linkedin_database` is deliberately absent: it
    /// returns `ProviderResetCounts` and never calls `load_bootstrap_state`.
    /// The subset check below keeps the two lists from drifting apart.
    const BOOTSTRAP_PROJECTING_COMMANDS: [&str; 7] = [
        "bootstrap_state",
        "set_download_job_pause",
        "set_all_downloads_paused",
        "retry_failed_download_job",
        "clear_failed_download_jobs",
        "remove_download_queue_item",
        "delete_completed_download",
    ];

    /// The part of this file that is compiled outside `cargo test`.
    ///
    /// Splitting on the bare `#[cfg(test)]` attribute is wrong here: this file
    /// also uses that attribute on individual test-only methods above the test
    /// module, so that split truncates the "production" text to the first
    /// `impl LinkVaultState`. Anchor on the module declaration instead.
    fn production_source() -> &'static str {
        let source = include_str!("commands.rs");
        source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or(source)
    }

    /// Extracts a command's body: everything from its signature up to the next
    /// top-level `#[tauri::command]`.
    fn command_source<'a>(production: &'a str, declaration: &str) -> &'a str {
        production
            .split(declaration)
            .nth(1)
            .and_then(|rest| rest.split("#[tauri::command]").next())
            .unwrap_or_default()
    }

    #[test]
    fn linkedin_bootstrap_commands_are_async_and_leave_the_async_executor() {
        let production = production_source();
        assert!(
            production.contains("pub async fn bootstrap_state("),
            "production_source() must not be empty; the split anchor is stale"
        );
        for name in BOOTSTRAP_PROJECTING_COMMANDS {
            assert!(
                OFF_UI_THREAD_COMMANDS.contains(&name),
                "{name} returns a BootstrapState and must be in OFF_UI_THREAD_COMMANDS"
            );
        }

        for name in OFF_UI_THREAD_COMMANDS {
            let async_declaration = format!("pub async fn {name}(");
            assert!(
                production.contains(&async_declaration),
                "{name} must be declared `pub async fn`; a synchronous Tauri \
                 command runs its whole body on the main thread"
            );
            assert!(
                !production.contains(&format!("pub fn {name}(")),
                "{name} must not remain a synchronous `pub fn` command"
            );

            let body = command_source(production, &async_declaration);
            let spawn_at = body.find("tauri::async_runtime::spawn_blocking").unwrap_or_else(
                || panic!(
                    "{name} performs blocking SQLite and filesystem work and must \
                     move it into spawn_blocking"
                ),
            );

            // The invariant that actually has teeth: the SQLite open is taken
            // from the *owned* `LinkedInCommandHandles` clone, and it happens
            // after the closure is entered. A bare `!body.contains("state.
            // connection()")` is satisfied just as well by a command that
            // stopped reading the database at all, and `handles.connection()`
            // hoisted above `spawn_blocking` would pass that negative check
            // while putting a blocking open back on the async executor.
            // `bootstrap_state` delegates the open to the handle method that
            // owns it, so it is accepted as the equivalent token.
            let connection_at = body
                .find("handles.connection()")
                .into_iter()
                .chain(body.find(".bootstrap_state(&runtime)"))
                .min();
            assert!(
                matches!(connection_at, Some(connection_at) if connection_at > spawn_at),
                "{name} must open SQLite from its owned LinkedInCommandHandles \
                 clone inside spawn_blocking, never from the borrowed \
                 tauri::State and never before the closure is entered"
            );

            if BOOTSTRAP_PROJECTING_COMMANDS.contains(&name) {
                // The projection may be built directly or, for `bootstrap_state`,
                // via the `LinkedInCommandHandles` helper that owns it. Either
                // way it must happen after the closure is entered.
                let load_at = body
                    .find("load_bootstrap_state(")
                    .into_iter()
                    .chain(body.find(".bootstrap_state(&runtime)"))
                    .min()
                    .unwrap_or(0);
                assert!(
                    load_at > spawn_at,
                    "{name} must call load_bootstrap_state from inside its blocking \
                     closure, not on the async executor"
                );
            }
        }

        // `reset_linkedin_database` is the one command in the list whose
        // correctness depends on statement *order* inside the closure, and the
        // order is load-bearing. Pin it: pause re-armed before the workflow
        // wipe, cancellation cleared only after the wipe and the history
        // rewrite have both run.
        let reset = command_source(production, "pub async fn reset_linkedin_database(");
        let positions = [
            ("set_download_paused(true)", reset.find("set_download_paused(true)")),
            (
                "delete_linkedin_runs()",
                reset.find("delete_linkedin_runs()"),
            ),
            (
                "clear_linkedin_provider_data(&connection)",
                reset.find("clear_linkedin_provider_data(&connection)"),
            ),
            (
                "sync_download_history_file(&connection, ..)",
                reset.find("sync_download_history_file(&connection"),
            ),
            (
                "reset_download_cancellation()",
                reset.find("reset_download_cancellation()"),
            ),
        ];
        for (statement, position) in positions {
            assert!(
                position.is_some(),
                "reset_linkedin_database must still call {statement}"
            );
        }
        let ordered: Vec<usize> = positions
            .iter()
            .map(|(_, position)| position.expect("asserted above"))
            .collect();
        assert!(
            ordered.windows(2).all(|pair| pair[0] < pair[1]),
            "reset_linkedin_database's flag ordering is load-bearing and must be: \
             set_download_paused(true) BEFORE delete_linkedin_runs(), then the \
             provider wipe, then the history-file rewrite, and only then \
             reset_download_cancellation(). Observed offsets {ordered:?}."
        );
    }

    #[test]
    fn linkedin_bootstrap_commands_move_the_history_file_write_off_the_main_thread() {
        let production = production_source();

        // `sync_download_history_file` rewrites the markdown on disk. The five
        // commands that call it must do so from inside their blocking closure,
        // otherwise the file write is still on the UI thread.
        let writers = [
            "retry_failed_download_job",
            "clear_failed_download_jobs",
            "remove_download_queue_item",
            "delete_completed_download",
            "set_all_downloads_paused",
        ];
        for name in writers {
            let body = command_source(production, &format!("pub async fn {name}("));
            let spawn = body.find("spawn_blocking");
            let sync = body.find("sync_download_history_file");
            match (spawn, sync) {
                (Some(spawn_at), Some(sync_at)) => assert!(
                    sync_at > spawn_at,
                    "{name} must sync the history file after entering its \
                     blocking closure, not before"
                ),
                (_, None) => {}
                _ => panic!(
                    "{name} calls sync_download_history_file but never enters \
                     spawn_blocking, so the file write would run on the UI thread"
                ),
            }
        }
    }

    #[test]
    fn linkvault_state_command_handles_share_the_existing_flag_allocations() {
        // `tauri::State<'_, T>` cannot be moved into the `'static` closure
        // `spawn_blocking` needs, so the async commands take this owned handle
        // instead. It must share the state's `Arc` slots rather than copy the
        // flags, or a pause set inside the closure would be invisible to the
        // running download.
        let state = LinkVaultState::new("linkvault-test.sqlite3".into());
        let handles = state.command_handles();

        handles.set_download_paused(true);
        assert!(state.is_download_paused());

        state.request_download_cancellation();
        assert!(handles.cancellation_requested());

        handles.reset_download_cancellation();
        assert!(!state.is_download_cancellation_requested());
        assert!(!state.is_download_paused());

        assert_eq!(handles.db_path, PathBuf::from("linkvault-test.sqlite3"));
        assert_eq!(handles.token_path(), state.token_path());
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
                force_redownload: false,
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
                force_redownload: false,
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
    fn duplicate_course_requests_for_same_output_are_skipped() {
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
            force_redownload: false,
        };

        let first = queue_download_jobs(&runtime, &connection, request(), 100).unwrap();
        let second = queue_download_jobs(&runtime, &connection, request(), 100).unwrap();

        assert_eq!(first.jobs.len(), 1);
        assert!(second.jobs.is_empty());
        assert_eq!(second.skipped.len(), 1);
        assert_eq!(second.skipped[0].course_slug, "sample-course");
        assert_eq!(second.skipped[0].reason, "already_queued");
        assert_eq!(runtime.list_linkedin_runs(10).unwrap().len(), 1);
    }

    #[test]
    fn queue_download_skips_completed_legacy_job_same_slug_and_output() {
        let (_dir, runtime, connection) = workflow_harness();
        crate::cache::insert_job(
            &connection,
            &JobRecord {
                id: "legacy-completed-1".to_string(),
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                status: "completed".to_string(),
                selected_quality: "720".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                quiz_hints_json: "[]".to_string(),
                output_dir: "C:/downloads".to_string(),
                paused: false,
                scheduled_at: None,
                created_at: 50,
                updated_at: 50,
            },
        )
        .unwrap();

        let response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
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
                force_redownload: false,
            },
            100,
        )
        .unwrap();

        assert!(response.jobs.is_empty());
        assert_eq!(response.skipped.len(), 1);
        assert_eq!(response.skipped[0].reason, "already_completed");
        assert!(runtime.list_linkedin_runs(10).unwrap().is_empty());
    }

    #[test]
    fn force_redownload_allows_completed_but_still_skips_queued() {
        let (_dir, runtime, connection) = workflow_harness();
        crate::cache::insert_job(
            &connection,
            &JobRecord {
                id: "legacy-completed-2".to_string(),
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                status: "completed".to_string(),
                selected_quality: "720".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                quiz_hints_json: "[]".to_string(),
                output_dir: "C:/downloads".to_string(),
                paused: false,
                scheduled_at: None,
                created_at: 50,
                updated_at: 50,
            },
        )
        .unwrap();

        let forced = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
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
                force_redownload: true,
            },
            100,
        )
        .unwrap();
        assert_eq!(forced.jobs.len(), 1);
        assert!(forced.skipped.is_empty());

        let again = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
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
                force_redownload: true,
            },
            110,
        )
        .unwrap();
        assert!(again.jobs.is_empty());
        assert_eq!(again.skipped[0].reason, "already_queued");
    }

    #[test]
    fn linkedin_queue_is_busy_for_ready_workflow_run_and_clears_when_empty() {
        let (_dir, runtime, connection) = workflow_harness();
        assert!(!linkedin_queue_is_busy(&runtime, &connection, 1_000).unwrap());

        queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
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
                force_redownload: false,
            },
            200,
        )
        .unwrap();

        assert!(linkedin_queue_is_busy(&runtime, &connection, 1_000).unwrap());
    }

    #[test]
    fn linkedin_queue_is_busy_for_legacy_active_job() {
        let (_dir, runtime, connection) = workflow_harness();
        crate::cache::insert_job(
            &connection,
            &JobRecord {
                id: "legacy-active-1".to_string(),
                course_slug: "active-course".to_string(),
                source_url: "https://www.linkedin.com/learning/active-course".to_string(),
                status: "active".to_string(),
                selected_quality: "720".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                quiz_hints_json: "[]".to_string(),
                output_dir: "C:/downloads".to_string(),
                paused: false,
                scheduled_at: None,
                created_at: 100,
                updated_at: 100,
            },
        )
        .unwrap();

        assert!(linkedin_queue_is_busy(&runtime, &connection, 1_000).unwrap());
    }

    #[test]
    fn linkedin_slug_course_folder_with_study_md_is_detected() {
        let temp = tempfile::tempdir().unwrap();
        let course_dir = temp.path().join("sample-course");
        std::fs::create_dir_all(&course_dir).unwrap();
        std::fs::write(course_dir.join("Study.md"), "# Sample\n").unwrap();

        assert!(linkedin_slug_course_folder_exists(
            temp.path().to_str().unwrap(),
            "sample-course"
        ));
        assert!(!linkedin_slug_course_folder_exists(
            temp.path().to_str().unwrap(),
            "other-course"
        ));
    }

    #[test]
    fn retry_failed_download_removes_terminal_run_and_requeues_same_id() {
        let (_dir, runtime, connection) = workflow_harness();
        let response = queue_download_jobs(
            &runtime,
            &connection,
            StartDownloadRequest {
                course_urls: "https://www.linkedin.com/learning/sample-course".to_string(),
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
                force_redownload: false,
            },
            200,
        )
        .unwrap();
        let job_id = response.jobs[0].id.clone();
        runtime.cancel_run(job_id.clone(), 210).unwrap();
        assert_eq!(
            runtime.get_run(job_id.clone()).unwrap().unwrap().state,
            RunState::Cancelled
        );

        retry_failed_download_job_inner(&runtime, &connection, job_id.clone(), 220).unwrap();

        let runs = runtime.list_linkedin_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, job_id);
        assert_eq!(runs[0].state, RunState::Queued);
        assert!(get_job(&connection, &job_id).unwrap().is_none());
    }

    #[test]
    fn retry_legacy_failed_download_removes_failed_row_and_queues_workflow_run() {
        let (_dir, runtime, connection) = workflow_harness();
        let job_id = "legacy-failed-1".to_string();
        crate::cache::insert_job(
            &connection,
            &JobRecord {
                id: job_id.clone(),
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                status: "failed".to_string(),
                selected_quality: "720".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                quiz_hints_json: "[]".to_string(),
                output_dir: "C:/downloads".to_string(),
                paused: false,
                scheduled_at: None,
                created_at: 100,
                updated_at: 100,
            },
        )
        .unwrap();

        retry_failed_download_job_inner(&runtime, &connection, job_id.clone(), 300).unwrap();

        assert!(get_job(&connection, &job_id).unwrap().is_none());
        let run = runtime.get_run(job_id.clone()).unwrap().unwrap();
        assert_eq!(run.state, RunState::Queued);
        assert_eq!(
            runtime
                .list_linkedin_runs(10)
                .unwrap()
                .iter()
                .filter(|candidate| {
                    matches!(candidate.state, RunState::Failed | RunState::Cancelled)
                })
                .count(),
            0
        );
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

    // ---------------------------------------------------------------------
    // Scaling guard for the retired event N+1. A ratio, not a wall clock.
    //
    // The defect this pins: `load_bootstrap_state` read every `job_events` row
    // of every job, sorted the merged vector, and threw all but 20 away. Its
    // cost therefore scaled with the *total* event count, which is the axis
    // that grows without bound in production -- a long-running install accrues
    // events forever while the UI only ever renders 20.
    //
    // Why a ratio and not a millisecond budget: an absolute budget measures the
    // machine, not the code. `cargo test` runs this binary's ~800 tests in
    // parallel on a debug build, and the same call on the same dataset has been
    // observed at 17 ms on a quiet run and 78.90 ms under full-suite load. A
    // fixed ceiling is therefore a coin flip on how many other threads the OS
    // happened to be running; the previous 70 ms budget failed that way roughly
    // 1 run in 9.
    //
    // The two measurements here are taken back to back over the *same* job
    // count, so whatever multiplicative slowdown the machine applies it
    // applies to both, and only the event-dependent term survives the division.
    // Holding the job count constant is what makes the per-job artifact and
    // course-cache reads identical in both runs, so they cancel. The retired
    // loop added a term proportional to the event count and quadrupling the
    // event count quadruples that term; the replacement issues one indexed
    // query that returns 20 rows regardless of table size, so its event term is
    // a constant and the ratio is ~1.
    // ---------------------------------------------------------------------

    const BOOTSTRAP_REGRESSION_JOBS: usize = 60;
    const BOOTSTRAP_REGRESSION_ARTIFACTS_PER_JOB: usize = 40;
    const BOOTSTRAP_REGRESSION_EVENTS_PER_JOB: usize = 100;
    const BOOTSTRAP_REGRESSION_EVENTS_MULTIPLIER: usize = 4;
    const BOOTSTRAP_REGRESSION_WIDE_EVENTS_PER_JOB: usize =
        BOOTSTRAP_REGRESSION_EVENTS_PER_JOB * BOOTSTRAP_REGRESSION_EVENTS_MULTIPLIER;
    /// The measured ceiling on (cost at 4x events) / (cost at 1x events).
    ///
    /// Measured on this machine, debug build, by restoring the retired per-job
    /// merge and running this test: 1.01 with the single capped indexed query,
    /// 3.21 with the per-job merge + sort + truncate(20) loop back. 2.0 sits
    /// near the middle of that gap. A 1.0 ceiling would be a tautology (any
    /// real per-event work fails it); anything above ~3.2 would admit the N+1
    /// this test exists to reject.
    const BOOTSTRAP_REGRESSION_MAX_RATIO: f64 = 2.0;
    /// Timed passes per dataset, after one untimed warm-up pass. The fastest
    /// pass is the least-contended estimate: a descheduled thread adds time to
    /// whichever pass it lands on rather than scaling all of them, so taking a
    /// minimum keeps an unlucky stall out of the ratio.
    const BOOTSTRAP_REGRESSION_SAMPLES: usize = 3;
    /// Below this the denominator is a clock artefact rather than a
    /// measurement, and the ratio would be noise amplified.
    const BOOTSTRAP_REGRESSION_MIN_MEASURED_MS: f64 = 0.5;

    /// Seeds a fresh harness with `events_per_job` events on each of
    /// `BOOTSTRAP_REGRESSION_JOBS` jobs and returns the fastest warmed
    /// `load_bootstrap_state` pass, its projection, and the event row count.
    fn bootstrap_regression_sample(events_per_job: usize) -> (f64, BootstrapState, i64) {
        use std::time::Instant;

        let (_dir, runtime, connection) = workflow_harness();
        perf_probe_seed_dataset_with(
            &connection,
            BOOTSTRAP_REGRESSION_JOBS,
            BOOTSTRAP_REGRESSION_ARTIFACTS_PER_JOB,
            events_per_job,
        );

        let event_rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM job_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            event_rows,
            (BOOTSTRAP_REGRESSION_JOBS * events_per_job) as i64,
            "the seeder must produce exactly the requested event count"
        );

        let mut fastest_ms = f64::INFINITY;
        let mut fastest: Option<BootstrapState> = None;
        for pass in 0..=BOOTSTRAP_REGRESSION_SAMPLES {
            // The first pass warms the statement cache and the SQLite page
            // cache, so the timed passes reflect the query rather than
            // first-touch page faults.
            let started = Instant::now();
            let bootstrap = load_bootstrap_state(
                &connection,
                Some(&runtime),
                true,
                Path::new("C:/downloads/download-history.md"),
                false,
            )
            .unwrap();
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            if pass == 0 {
                continue;
            }
            if elapsed_ms < fastest_ms {
                fastest_ms = elapsed_ms;
                fastest = Some(bootstrap);
            }
        }

        (
            fastest_ms,
            fastest.expect("the sample loop runs at least one timed pass"),
            event_rows,
        )
    }

    #[test]
    fn bootstrap_state_event_read_does_not_scale_with_the_event_log() {
        let (narrow_ms, narrow, narrow_rows) =
            bootstrap_regression_sample(BOOTSTRAP_REGRESSION_EVENTS_PER_JOB);
        let (wide_ms, wide, wide_rows) =
            bootstrap_regression_sample(BOOTSTRAP_REGRESSION_WIDE_EVENTS_PER_JOB);

        // The only axis allowed to differ is the event log; the job count is
        // what the division relies on staying constant.
        assert_eq!(
            wide_rows,
            narrow_rows * BOOTSTRAP_REGRESSION_EVENTS_MULTIPLIER as i64,
            "the wide dataset must hold exactly {BOOTSTRAP_REGRESSION_EVENTS_MULTIPLIER}x the \
             event log and the same number of jobs"
        );
        // Far more events than the 20 the UI ever receives: that amplification
        // is what the retired loop could not avoid, and it is what makes a
        // 4x event log detectable at all.
        assert!(
            narrow_rows > 20 && wide_rows > 20,
            "the 20-row cap must be binding in both datasets, got {narrow_rows} and {wide_rows}"
        );

        // Both projections must still be correct; a fast-but-wrong read would
        // satisfy the ratio trivially.
        assert_eq!(narrow.recent_events.len(), 20);
        assert_eq!(wide.recent_events.len(), 20);
        assert_eq!(narrow.persisted_jobs.len(), BOOTSTRAP_REGRESSION_JOBS);
        assert_eq!(wide.persisted_jobs.len(), BOOTSTRAP_REGRESSION_JOBS);

        assert!(
            narrow_ms > BOOTSTRAP_REGRESSION_MIN_MEASURED_MS,
            "the {narrow_rows}-row measurement was {narrow_ms:.3} ms, too small to divide; \
             the fixture no longer exercises load_bootstrap_state"
        );

        let ratio = wide_ms / narrow_ms;
        // Printed unconditionally so a CI log always carries both figures, not
        // just the failure.
        println!(
            "load_bootstrap_state: {narrow_ms:.2} ms at {narrow_rows} job_events rows vs \
             {wide_ms:.2} ms at {wide_rows} rows \
             ({BOOTSTRAP_REGRESSION_EVENTS_MULTIPLIER}x the event log, \
             {BOOTSTRAP_REGRESSION_JOBS} jobs held constant) -> ratio {ratio:.2} \
             (ceiling {BOOTSTRAP_REGRESSION_MAX_RATIO:.1})"
        );

        assert!(
            ratio <= BOOTSTRAP_REGRESSION_MAX_RATIO,
            "load_bootstrap_state took {narrow_ms:.2} ms over {narrow_rows} job_events rows but \
             {wide_ms:.2} ms over {wide_rows} rows: a \
             {BOOTSTRAP_REGRESSION_EVENTS_MULTIPLIER}x larger event log cost {ratio:.2}x more \
             (ceiling {BOOTSTRAP_REGRESSION_MAX_RATIO:.1}). The per-job event N+1 is probably \
             back."
        );
    }

    // ---------------------------------------------------------------------
    // Measurement-only performance probe. NOT a fix and NOT a threshold test.
    //
    // Hypothesis under test: `load_bootstrap_state` is pathologically slow
    // because it runs an N+1 query pattern over the unindexed
    // `job_events.job_id` and `artifacts.job_id` columns, collects every
    // event, and then throws all but 20 away.
    //
    // This probe records numbers and query plans so the hypothesis can be
    // CONFIRMED or REFUTED with evidence before any remediation is written.
    // It asserts only the behaviour that is currently true.
    //
    // Run explicitly:
    //   cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
    //     perf_probe_bootstrap_state_scaling -- --ignored --nocapture
    // ---------------------------------------------------------------------

    const PERF_PROBE_ARTIFACTS_PER_JOB: usize = 40;
    const PERF_PROBE_EVENTS_PER_JOB: usize = 120;
    const PERF_PROBE_EPOCH: i64 = 1_700_000_000;

    struct PerfProbeSample {
        jobs: usize,
        job_events_rows: i64,
        artifact_rows: i64,
        course_cache_rows: i64,
        download_history_rows: usize,
        cold_ms: f64,
        warm_ms: f64,
        seed_ms: f64,
        persisted_jobs: usize,
        recent_events: usize,
    }

    fn perf_probe_count(connection: &Connection, sql: &str) -> i64 {
        connection.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    /// Prints `EXPLAIN QUERY PLAN` rows verbatim (column 3 = `detail`).
    fn perf_probe_explain(
        connection: &Connection,
        label: &str,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let details = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<std::result::Result<Vec<String>, _>>()
            .unwrap();
        println!("EXPLAIN QUERY PLAN -- {label}");
        println!("  SQL: {sql}");
        if details.is_empty() {
            println!("  <no plan rows>");
        }
        for detail in details {
            println!("  {detail}");
        }
    }

    fn perf_probe_print_index_inventory(connection: &Connection) {
        let mut statement = connection
            .prepare(
                "SELECT tbl_name || ' | ' || name || ' | ' || COALESCE(sql, '(implicit index)') \
                 FROM sqlite_master WHERE type = 'index' \
                 AND tbl_name IN ('jobs', 'job_events', 'artifacts', 'course_cache') \
                 ORDER BY tbl_name, name",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<String>, _>>()
            .unwrap();
        println!("INDEX INVENTORY (jobs / job_events / artifacts / course_cache)");
        if rows.is_empty() {
            println!("  <none>");
        }
        for row in rows {
            println!("  {row}");
        }
    }

    /// Seeds `job_count` jobs, each with `PERF_PROBE_ARTIFACTS_PER_JOB`
    /// artifacts and `PERF_PROBE_EVENTS_PER_JOB` events, using only the public
    /// `crate::cache` helpers so the dataset matches what production writes.
    fn perf_probe_seed_dataset(connection: &Connection, job_count: usize) {
        perf_probe_seed_dataset_with(
            connection,
            job_count,
            PERF_PROBE_ARTIFACTS_PER_JOB,
            PERF_PROBE_EVENTS_PER_JOB,
        );
    }

    /// The same seeder with the per-job artifact and event counts as
    /// parameters, so the wall-clock regression test can weight the dataset
    /// toward the event log without duplicating the fixture.
    fn perf_probe_seed_dataset_with(
        connection: &Connection,
        job_count: usize,
        artifacts_per_job: usize,
        events_per_job: usize,
    ) {
        let job_statuses = ["completed", "failed", "cancelled", "queued", "active"];
        let artifact_types = ["video", "subtitle", "quiz"];
        let artifact_statuses = ["completed", "completed", "pending", "failed", "cancelled"];
        let event_types = [
            "artifact.started",
            "artifact.completed",
            "artifact.failed",
            "job.progress",
            "quiz.graded",
        ];

        for index in 0..job_count {
            let job_id = format!("perf-job-{index:05}");
            let course_slug = format!("perf-course-{index:05}");
            let created_at = PERF_PROBE_EPOCH + index as i64;
            let status = job_statuses[index % job_statuses.len()];
            let source_url = format!("https://www.linkedin.com/learning/{course_slug}");

            // One transaction per job: this connection runs `synchronous = FULL`,
            // so per-statement commits would add ~1 fsync per seeded row.
            connection.execute_batch("BEGIN").unwrap();
            crate::cache::insert_job(
                connection,
                &JobRecord {
                    id: job_id.clone(),
                    course_slug: course_slug.clone(),
                    source_url: source_url.clone(),
                    status: status.to_string(),
                    selected_quality: "1080".to_string(),
                    download_videos: true,
                    download_exercises: true,
                    download_subtitles: true,
                    download_quizzes: true,
                    quiz_hints_json: "[]".to_string(),
                    output_dir: format!("C:/downloads/{course_slug}"),
                    paused: false,
                    scheduled_at: None,
                    created_at,
                    updated_at: created_at + 1,
                },
            )
            .unwrap();
            crate::cache::upsert_course_cache_entry(
                connection,
                &crate::cache::CourseCacheEntry {
                    course_slug: course_slug.clone(),
                    source_url: source_url.clone(),
                    title: Some(format!("Perf Course {index}")),
                    payload_json: format!(
                        "{{\"title\":\"Perf Course {index}\",\"thumbnail_url\":\"https://media.licdn.com/thumb-{index}.jpg\",\"modules\":[{{\"urn\":\"urn:li:lesson:{index}\"}}]}}"
                    ),
                    fetched_at: created_at,
                },
            )
            .unwrap();
            for artifact_index in 0..artifacts_per_job {
                let artifact_type = artifact_types[artifact_index % artifact_types.len()];
                let artifact_status =
                    artifact_statuses[artifact_index % artifact_statuses.len()];
                let extension = match artifact_type {
                    "video" => "mp4",
                    "subtitle" => "vtt",
                    _ => "json",
                };
                upsert_artifact(
                    connection,
                    &ArtifactRecord {
                        id: format!("{job_id}-artifact-{artifact_index:03}"),
                        job_id: job_id.clone(),
                        artifact_type: artifact_type.to_string(),
                        path: format!(
                            "C:/downloads/{course_slug}/{:02} - {artifact_type}.{extension}",
                            artifact_index + 1
                        ),
                        status: artifact_status.to_string(),
                        size_bytes: Some(1_000_000 + artifact_index as i64),
                        created_at: created_at + artifact_index as i64,
                        updated_at: created_at + artifact_index as i64 + 1,
                    },
                )
                .unwrap();
            }
            for event_index in 0..events_per_job {
                let event_type = event_types[event_index % event_types.len()];
                append_job_event(
                    connection,
                    &NewJobEvent {
                        job_id: job_id.clone(),
                        event_type: event_type.to_string(),
                        message: format!("{event_type} for {course_slug} step {event_index}"),
                        payload_json: Some(format!(
                            "{{\"step\":{event_index},\"course_slug\":\"{course_slug}\",\"bytes\":{}}}",
                            1_000_000 + event_index as i64
                        )),
                        created_at: created_at + event_index as i64,
                    },
                )
                .unwrap();
            }
            connection.execute_batch("COMMIT").unwrap();
        }
    }

    #[test]
    #[ignore = "measurement-only performance probe; run with --ignored --nocapture"]
    fn perf_probe_bootstrap_state_scaling() {
        use std::time::Instant;

        let history_path = Path::new("C:/downloads/download-history.md");
        println!("{}", "=".repeat(78));
        println!("PERF PROBE: load_bootstrap_state scaling (measurement only, no fix)");
        println!("dataset: {PERF_PROBE_ARTIFACTS_PER_JOB} artifacts + {PERF_PROBE_EVENTS_PER_JOB} events per job");
        println!("{}", "=".repeat(78));

        let mut samples: Vec<PerfProbeSample> = Vec::new();
        let mut plan_sample: Option<(i64, i64, i64, i64, i64)> = None;

        for job_count in [10_usize, 100, 500] {
            let (_dir, runtime, connection) = workflow_harness();
            let seed_started = Instant::now();
            perf_probe_seed_dataset(&connection, job_count);
            let seed_ms = seed_started.elapsed().as_secs_f64() * 1000.0;

            let job_events_rows = perf_probe_count(&connection, "SELECT COUNT(*) FROM job_events");
            let artifact_rows = perf_probe_count(&connection, "SELECT COUNT(*) FROM artifacts");
            let course_cache_rows = perf_probe_count(&connection, "SELECT COUNT(*) FROM course_cache");

            let cold_started = Instant::now();
            let bootstrap = load_bootstrap_state(
                &connection,
                Some(&runtime),
                true,
                history_path,
                false,
            )
            .unwrap();
            let cold_ms = cold_started.elapsed().as_secs_f64() * 1000.0;

            // Second pass: same work, warm OS page cache / SQLite page cache.
            let warm_started = Instant::now();
            let bootstrap_warm =
                load_bootstrap_state(&connection, Some(&runtime), true, history_path, false)
                    .unwrap();
            let warm_ms = warm_started.elapsed().as_secs_f64() * 1000.0;

            // Documented waste: every job_event row for every job is read,
            // sorted, and then all but 20 rows are discarded.
            assert_eq!(
                bootstrap.recent_events.len(),
                20,
                "load_bootstrap_state is expected to keep exactly 20 events"
            );
            assert_eq!(bootstrap_warm.recent_events.len(), 20);
            assert_eq!(
                bootstrap.persisted_jobs.len(),
                job_count,
                "every seeded job is expected to be returned to the UI"
            );
            let expected_video_artifacts_per_job = (0..PERF_PROBE_ARTIFACTS_PER_JOB)
                .filter(|artifact_index| artifact_index % 3 == 0)
                .count();
            assert_eq!(
                bootstrap
                    .persisted_jobs
                    .iter()
                    .map(|job| job.video_artifacts.len())
                    .sum::<usize>(),
                job_count * expected_video_artifacts_per_job
            );

            samples.push(PerfProbeSample {
                jobs: job_count,
                job_events_rows,
                artifact_rows,
                course_cache_rows,
                download_history_rows: bootstrap.download_history.len(),
                cold_ms,
                warm_ms,
                seed_ms,
                persisted_jobs: bootstrap.persisted_jobs.len(),
                recent_events: bootstrap.recent_events.len(),
            });

            println!("--- N = {job_count} jobs ---");
            println!("  seeded: {job_events_rows} job_events rows, {artifact_rows} artifacts rows, {course_cache_rows} course_cache rows, {seed_ms:.1} ms");
            println!("  load_bootstrap_state cold = {cold_ms:.2} ms, warm = {warm_ms:.2} ms");
            println!("  returned: persisted_jobs = {}, recent_events = {}, download_history = {}", bootstrap.persisted_jobs.len(), bootstrap.recent_events.len(), bootstrap.download_history.len());

            if job_count == 500 {
                println!();
                perf_probe_print_index_inventory(&connection);
                println!();
                perf_probe_explain(
                    &connection,
                    "hot query 1: list_job_events (N+1, called once per job)",
                    "SELECT id, job_id, event_type, message, payload_json, created_at FROM job_events WHERE job_id = ?1 ORDER BY id",
                    &[&"perf-job-00000"],
                );
                println!();
                perf_probe_explain(
                    &connection,
                    "hot query 2: list_artifacts_for_job (N+1, called once per job)",
                    "SELECT id, job_id, artifact_type, path, status, size_bytes, created_at, updated_at FROM artifacts WHERE job_id = ?1 ORDER BY created_at, id",
                    &[&"perf-job-00000"],
                );
                println!();
                perf_probe_explain(
                    &connection,
                    "hot query 3: list_jobs_by_status (called 5x by bootstrap_jobs)",
                    "SELECT id, course_slug, source_url, status, selected_quality, download_videos, download_exercises, download_subtitles, download_quizzes, quiz_hints_json, output_dir, paused, scheduled_at, created_at, updated_at FROM jobs WHERE status = ?1 ORDER BY created_at, id",
                    &[&"completed"],
                );
                println!();
                perf_probe_explain(
                    &connection,
                    "hot query 4: list_download_history (uncapped)",
                    "SELECT jobs.id, jobs.course_slug, jobs.source_url, COALESCE(NULLIF(course_cache.title, ''), jobs.course_slug), jobs.output_dir, jobs.updated_at FROM jobs LEFT JOIN course_cache ON course_cache.course_slug = jobs.course_slug WHERE jobs.status = 'completed' ORDER BY jobs.updated_at DESC, jobs.created_at DESC, jobs.id",
                    &[],
                );

                // Phase breakdown: same read-only calls the production path
                // makes, timed individually, to attribute the total cost.
                let jobs = bootstrap_jobs(&connection).unwrap();
                println!();
                println!("PHASE BREAKDOWN AT N = {} (replays load_bootstrap_state's reads)", job_count);
                // The production event read: one ordered, capped query.
                let recent_started = Instant::now();
                let recent_rows = list_recent_job_events(&connection, 20)
                    .unwrap()
                    .len();
                let recent_ms = recent_started.elapsed().as_secs_f64() * 1000.0;
                // Retired pattern, kept so the cost that was removed stays
                // visible next to the cost that replaced it.
                let events_started = Instant::now();
                let mut events_read = 0usize;
                for job in &jobs {
                    events_read += list_job_events(&connection, &job.id).unwrap().len();
                }
                let events_ms = events_started.elapsed().as_secs_f64() * 1000.0;
                let artifacts_started = Instant::now();
                let mut artifacts_read = 0usize;
                for job in &jobs {
                    artifacts_read += list_artifacts_for_job(&connection, &job.id).unwrap().len();
                }
                let artifacts_ms = artifacts_started.elapsed().as_secs_f64() * 1000.0;
                let cache_started = Instant::now();
                for job in &jobs {
                    let _ = get_course_cache_entry(&connection, &job.course_slug).unwrap();
                }
                let cache_ms = cache_started.elapsed().as_secs_f64() * 1000.0;
                let history_started = Instant::now();
                let history_rows = list_download_history(&connection).unwrap().len();
                let history_ms = history_started.elapsed().as_secs_f64() * 1000.0;
                let jobs_started = Instant::now();
                let bootstrap_jobs_rows = bootstrap_jobs(&connection).unwrap().len();
                let jobs_ms = jobs_started.elapsed().as_secs_f64() * 1000.0;
                let job_queries = jobs.len();
                println!("  bootstrap_jobs                 : {jobs_ms:>9.2} ms ({bootstrap_jobs_rows} jobs)");
                println!("  list_recent_job_events(20)     : {recent_ms:>9.2} ms ({recent_rows} rows read, 1 query)  <- production path");
                println!("  N+1 list_job_events (retired)  : {events_ms:>9.2} ms ({events_read} rows read, {job_queries} queries)");
                println!("  N+1 list_artifacts_for_job     : {artifacts_ms:>9.2} ms ({artifacts_read} rows read, {job_queries} queries)");
                println!("  N+1 get_course_cache_entry     : {cache_ms:>9.2} ms ({job_queries} queries, PK search)");
                println!("  list_download_history          : {history_ms:>9.2} ms ({history_rows} rows)");
                plan_sample = Some((
                    jobs_ms as i64,
                    recent_ms as i64,
                    artifacts_ms as i64,
                    cache_ms as i64,
                    history_ms as i64,
                ));
            }
        }

        println!();
        println!("{}", "=".repeat(78));
        println!("RESULTS");
        println!("{}", "=".repeat(78));
        println!(
            "{:>6} | {:>12} | {:>11} | {:>10} | {:>9} | {:>8} | {:>12} | {:>10} | {:>12}",
            "N",
            "job_events",
            "artifacts",
            "seed ms",
            "cold ms",
            "warm ms",
            "cold ms/job",
            "marginal",
            "marginal ms/job"
        );
        println!("{}", "-".repeat(78));
        let mut previous: Option<(usize, f64)> = None;
        for sample in &samples {
            let per_job = sample.cold_ms / sample.jobs as f64;
            let marginal = match previous {
                Some((previous_n, previous_ms)) => {
                    format!("{:.2}", (sample.cold_ms - previous_ms) / (sample.jobs - previous_n) as f64)
                }
                None => "-".to_string(),
            };
            println!(
                "{:>6} | {:>12} | {:>11} | {:>10.1} | {:>9.2} | {:>8.2} | {:>12.3} | {:>10} | {:>12.3}",
                sample.jobs,
                sample.job_events_rows,
                sample.artifact_rows,
                sample.seed_ms,
                sample.cold_ms,
                sample.warm_ms,
                per_job,
                marginal,
                sample.warm_ms / sample.jobs as f64
            );
            previous = Some((sample.jobs, sample.cold_ms));
        }
        println!("{}", "-".repeat(78));
        println!("marginal = ms of additional cold time per additional job, between consecutive sizes");
        println!("last column = warm ms per job");
        for sample in &samples {
            println!(
                "  N={:>3}: recent_events={} (kept) of {} seeded event rows read, persisted_jobs={}, download_history={}, course_cache={}",
                sample.jobs,
                sample.recent_events,
                sample.job_events_rows,
                sample.persisted_jobs,
                sample.download_history_rows,
                sample.course_cache_rows
            );
        }
        if let Some((jobs_ms, recent_ms, artifacts_ms, cache_ms, history_ms)) = plan_sample {
            println!();
            println!("PHASE BREAKDOWN (N = 500, integer ms): bootstrap_jobs={jobs_ms}, list_recent_job_events(20)={recent_ms}, list_artifacts_for_job N+1={artifacts_ms}, get_course_cache_entry N+1={cache_ms}, list_download_history={history_ms}");
        }
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
