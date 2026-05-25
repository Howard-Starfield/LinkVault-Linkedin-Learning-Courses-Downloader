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
    append_job_event, clear_failed_jobs, get_course_cache_entry, get_setting, insert_job,
    list_artifacts_for_job, list_job_events, list_jobs_by_status, list_recent_jobs,
    open_or_initialize, retry_failed_job, upsert_setting_json, JobRecord, NewJobEvent,
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
use std::path::PathBuf;
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
}

impl LinkVaultState {
    pub fn new(db_path: PathBuf) -> Self {
        let token_path = db_path.with_file_name("linkvault.li_at.dpapi");
        Self {
            db_path,
            token_path,
            download_cancellation: Arc::new(AtomicBool::new(false)),
        }
    }

    fn connection(&self) -> Result<Connection, String> {
        open_or_initialize(&self.db_path).map_err(|error| error.to_string())
    }

    fn reset_download_cancellation(&self) -> DownloadCancellation {
        self.download_cancellation.store(false, Ordering::SeqCst);
        self.download_cancellation()
    }

    fn request_download_cancellation(&self) {
        self.download_cancellation.store(true, Ordering::SeqCst);
    }

    fn download_cancellation(&self) -> DownloadCancellation {
        DownloadCancellation {
            cancelled: Arc::clone(&self.download_cancellation),
        }
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
}

impl CancellationFlag for DownloadCancellation {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueuedDownloadJob {
    id: String,
    course_slug: String,
    source_url: String,
    status: String,
    thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistedDownloadJob {
    id: String,
    course_slug: String,
    source_url: String,
    status: String,
    thumbnail_url: Option<String>,
    selected_quality: String,
    output_dir: String,
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

#[tauri::command]
pub fn bootstrap_state(state: tauri::State<'_, LinkVaultState>) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    load_bootstrap_state(&connection, token_store::has_saved_token(&state.token_path))
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
    Ok(CancelDownloadResponse {
        cancellation_requested: true,
    })
}

#[tauri::command]
pub fn retry_failed_download_job(
    state: tauri::State<'_, LinkVaultState>,
    job_id: String,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    retry_failed_job(&connection, &job_id, now_unix_timestamp())
        .map_err(|error| error.to_string())?;
    load_bootstrap_state(&connection, token_store::has_saved_token(&state.token_path))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_failed_download_jobs(
    state: tauri::State<'_, LinkVaultState>,
) -> Result<BootstrapState, String> {
    let connection = state.connection()?;
    clear_failed_jobs(&connection).map_err(|error| error.to_string())?;
    load_bootstrap_state(&connection, token_store::has_saved_token(&state.token_path))
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
    let connection = open_or_initialize(&db_path).map_err(|error| error.to_string())?;
    let mut course_client =
        AuthenticatedLinkedInClient::new(&token, &session).map_err(|error| error.to_string())?;
    let mut artifact_client = course_client.clone();

    process_queued_download_batch_with_clients(
        &connection,
        &mut course_client,
        &mut artifact_client,
        timestamp,
        delay_seconds,
        &cancellation,
    )
}

fn process_next_queued_download_with_validated_token(
    db_path: PathBuf,
    token: String,
    session: ValidatedLinkedInSession,
    timestamp: i64,
    cancellation: DownloadCancellation,
    quiz_assessments: Vec<crate::course::CourseAssessment>,
) -> Result<ProcessQueuedDownloadResponse, String> {
    let connection = open_or_initialize(&db_path).map_err(|error| error.to_string())?;
    let mut course_client =
        AuthenticatedLinkedInClient::new(&token, &session).map_err(|error| error.to_string())?;
    let mut artifact_client = course_client.clone();

    process_next_queued_download_with_clients(
        &connection,
        &mut course_client,
        &mut artifact_client,
        timestamp,
        &cancellation,
        quiz_assessments,
    )
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

        let has_remaining_queued_jobs = list_jobs_by_status(connection, "queued")
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
        if cancellation.is_cancelled() {
            return;
        }
        thread::sleep(std::time::Duration::from_secs(1));
    }
}

async fn extract_quizzes_for_next_job(
    _app: tauri::AppHandle,
    db_path: PathBuf,
    _session: ValidatedLinkedInSession,
    timestamp: i64,
) -> Vec<crate::course::CourseAssessment> {
    let Ok(connection) = open_or_initialize(&db_path) else {
        return Vec::new();
    };
    record_quiz_metadata_discovery_for_next_job(&connection, timestamp)
}

fn record_quiz_metadata_discovery_for_next_job(
    connection: &Connection,
    timestamp: i64,
) -> Vec<crate::course::CourseAssessment> {
    let Ok(Some(job)) =
        list_jobs_by_status(&connection, "queued").map(|jobs| jobs.into_iter().next())
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

    let mut jobs = Vec::with_capacity(courses.len());
    for (index, course) in courses.iter().enumerate() {
        let job_id = format!(
            "job-{created_at}-{}-{index}",
            sanitize_identifier_fragment(&course.slug)
        );
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
                created_at,
                updated_at: created_at,
            },
        )
        .map_err(|error| error.to_string())?;
        append_job_event(
            connection,
            &NewJobEvent {
                job_id: job_id.clone(),
                event_type: "job.queued".to_string(),
                message: format!("Queued LinkedIn Learning course: {}", course.slug),
                payload_json: Some(
                    serde_json::json!({
                        "sourceUrl": course.normalized_url,
                        "quizUrls": course.quiz_urls,
                        "assessmentUrns": course.assessment_urns,
                        "delaySeconds": request.delay_seconds,
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
        });
    }

    Ok(StartDownloadResponse { jobs })
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
        let thumbnail_url = get_course_cache_entry(connection, &job.course_slug)
            .ok()
            .flatten()
            .and_then(|entry| cached_course_thumbnail_url(&entry.payload_json));
        persisted_jobs.push(PersistedDownloadJob {
            source_url: if job.source_url.trim().is_empty() {
                format!("https://www.linkedin.com/learning/{}", job.course_slug)
            } else {
                job.source_url.clone()
            },
            id: job.id,
            course_slug: job.course_slug,
            status: job.status,
            thumbnail_url,
            selected_quality: job.selected_quality,
            output_dir: job.output_dir,
            updated_at: job.updated_at,
            artifact_counts,
        });
    }

    Ok(BootstrapState {
        default_resolution: VideoQuality::P1080,
        browser_sources: vec!["Chrome", "Edge", "Firefox"],
        stores_plaintext_tokens_in_sqlite: false,
        has_saved_token,
        saved_download_preferences,
        persisted_jobs,
        recent_events,
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

        let bootstrap = load_bootstrap_state(&connection, true).unwrap();
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
            },
            1_700_000_000,
        )
        .unwrap();

        let bootstrap = load_bootstrap_state(&connection, true).unwrap();

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
        assert!(state.is_download_cancellation_requested());
        assert!(state.download_cancellation().is_cancelled());

        let cancellation = state.reset_download_cancellation();
        assert!(!state.is_download_cancellation_requested());
        assert!(!cancellation.is_cancelled());
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
