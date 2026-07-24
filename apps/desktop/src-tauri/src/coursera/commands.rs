//! Tauri command surface for the Coursera tab.
//!
//! All commands return `Result<T, String>` per Tauri 2 conventions.
//! Each public function is registered in `lib.rs`'s `invoke_handler!`
//! macro. They are the only entry points the React side can call.

#![allow(dead_code)] // Phase 10 — wired at end of phase

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::coursera::auth;
use crate::coursera::config::{
    parse_class_input, CourseraOptions, SavedCourseraPreferences, StartCourseraRequest,
};
use crate::coursera::coursera_token_store;
use crate::coursera::downloader::NativeDownloader;
#[allow(unused_imports)]
use crate::coursera::error::CourseraResult;
use crate::coursera::job::{self, CourseraJob, PersistedCourseraEvent};
use crate::coursera::orchestrator::{CourseEvent, CourseSummary, CourseraDownloader};
use crate::coursera::syllabus::ModulesV1;

const COURSERA_PREFS_KEY: &str = "coursera.preferences";

/// Per-app Coursera state. Kept separate from the LinkedIn-side
/// `LinkVaultState` so the two never share locks.
#[derive(Default)]
pub struct CourseraState {
    db_path: PathBuf,
    pub cancellation: Arc<AtomicBool>,
}

impl CourseraState {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            cancellation: Arc::new(AtomicBool::new(false)),
        }
    }

    fn connection(&self) -> Result<Connection, String> {
        crate::cache::open_or_initialize(&self.db_path).map_err(|e| e.to_string())
    }

    fn data_dir(&self) -> Result<PathBuf, String> {
        self.db_path
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| "database path has no parent directory".to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseraSessionInfo {
    pub email: String,
    pub cauth_set: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseraBootstrapState {
    pub default_options: SavedCourseraPreferences,
    pub has_saved_token: bool,
    pub saved_prefs: Option<SavedCourseraPreferences>,
    pub persisted_jobs: Vec<CourseraJob>,
    pub recent_events: Vec<PersistedCourseraEvent>,
    pub download_history: Vec<CourseraHistoryEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseraHistoryEntry {
    pub job: CourseraJob,
    pub last_event_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyllabusPreview {
    pub slug: String,
    pub module_count: usize,
    pub lesson_count: usize,
    pub total_items: usize,
    pub has_quizzes: bool,
    pub has_notebooks: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessCourseraResponse {
    pub processed: bool,
    pub completed_artifacts: u32,
    pub failed_artifacts: u32,
    pub cancelled_artifacts: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseraTokenSaveRequest {
    pub cauth: String,
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthMethodRequest {
    pub kind: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub cauth: String,
}

// ----- command implementations -----

#[tauri::command]
pub fn bootstrap_coursera_state(
    state: State<'_, CourseraState>,
) -> Result<CourseraBootstrapState, String> {
    let connection = state.connection()?;
    load_coursera_bootstrap_state(
        &connection,
        has_saved_coursera_token_for_dir(&state.data_dir()?)?,
    )
}

fn load_coursera_bootstrap_state(
    connection: &Connection,
    has_saved_token: bool,
) -> Result<CourseraBootstrapState, String> {
    let saved_prefs =
        match job::load_setting(connection, COURSERA_PREFS_KEY).map_err(|e| e.to_string())? {
            Some(json) => Some(serde_json::from_str(&json).map_err(|e| e.to_string())?),
            None => None,
        };
    let completed = job::list_completed_jobs(connection).map_err(|e| e.to_string())?;
    Ok(CourseraBootstrapState {
        default_options: SavedCourseraPreferences::default(),
        has_saved_token,
        saved_prefs,
        persisted_jobs: job::list_recent_jobs(connection, 250).map_err(|e| e.to_string())?,
        recent_events: job::list_recent_events(connection, 100).map_err(|e| e.to_string())?,
        download_history: completed
            .into_iter()
            .map(|job| CourseraHistoryEntry {
                last_event_at: Some(job.updated_at),
                job,
            })
            .collect(),
    })
}

#[tauri::command]
pub fn parse_coursera_class_input(
    input: String,
) -> Result<Vec<crate::coursera::config::ParsedCourseraClass>, String> {
    parse_class_input(&input).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn coursera_login(req: AuthMethodRequest) -> Result<CourseraSessionInfo, String> {
    let client = crate::coursera::client::build_client().map_err(|e| e.to_string())?;
    let (cauth, email) = match req.kind.as_str() {
        "email_password" => {
            let session = auth::login(&client, &req.email, &req.password)
                .await
                .map_err(|e| e.to_string())?;
            (session.cauth, session.email)
        }
        "cauth" => (req.cauth, req.email),
        "saved_token" => {
            let data_dir = crate::storage::resolve_data_dir().map_err(|e| e.to_string())?;
            let cauth =
                auth::read_cached_cauth(&data_dir).ok_or_else(|| "no saved token".to_string())?;
            let email = req.email;
            (cauth, email)
        }
        _ => return Err(format!("unknown auth method: {}", req.kind)),
    };
    let data_dir = crate::storage::resolve_data_dir().map_err(|e| e.to_string())?;
    let _ = auth::write_cached_cauth(&data_dir, &cauth);
    Ok(CourseraSessionInfo {
        email,
        cauth_set: !cauth.is_empty(),
    })
}

#[tauri::command]
pub fn save_coursera_token(req: CourseraTokenSaveRequest) -> Result<bool, String> {
    let data_dir = crate::storage::resolve_data_dir().map_err(|e| e.to_string())?;
    auth::write_cached_cauth(&data_dir, &req.cauth).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn clear_saved_coursera_token() -> Result<bool, String> {
    let data_dir = crate::storage::resolve_data_dir().map_err(|e| e.to_string())?;
    auth::clear_cache(&data_dir).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn has_saved_coursera_token() -> Result<bool, String> {
    let data_dir = crate::storage::resolve_data_dir().map_err(|e| e.to_string())?;
    has_saved_coursera_token_for_dir(&data_dir)
}

fn has_saved_coursera_token_for_dir(data_dir: &std::path::Path) -> Result<bool, String> {
    let p = coursera_token_store::default_token_path(data_dir);
    Ok(coursera_token_store::has_saved_token(&p))
}

#[tauri::command]
pub fn save_coursera_preferences(
    state: State<'_, CourseraState>,
    prefs: SavedCourseraPreferences,
) -> Result<bool, String> {
    let connection = state.connection()?;
    let json = serde_json::to_string(&prefs).map_err(|e| e.to_string())?;
    job::save_setting(&connection, COURSERA_PREFS_KEY, &json).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn load_coursera_preferences(
    state: State<'_, CourseraState>,
) -> Result<SavedCourseraPreferences, String> {
    let connection = state.connection()?;
    match job::load_setting(&connection, COURSERA_PREFS_KEY).map_err(|e| e.to_string())? {
        Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
        None => Ok(SavedCourseraPreferences::default()),
    }
}

#[tauri::command]
pub async fn start_coursera_download_jobs(
    state: State<'_, CourseraState>,
    req: StartCourseraRequest,
) -> Result<Vec<CourseraJob>, String> {
    let connection = state.connection()?;
    queue_coursera_download_jobs(&connection, req, chrono_now())
}

fn queue_coursera_download_jobs(
    connection: &Connection,
    req: StartCourseraRequest,
    now: i64,
) -> Result<Vec<CourseraJob>, String> {
    let force_redownload = req.force_redownload;
    let opts: CourseraOptions = req.into_options().map_err(|e| e.to_string())?;
    if !force_redownload {
        let completed: std::collections::HashSet<String> = job::list_completed_jobs(connection)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|job| job.class_name)
            .collect();
        let duplicates: Vec<String> = opts
            .class_names
            .iter()
            .filter(|class_name| completed.contains(*class_name))
            .cloned()
            .collect();
        if !duplicates.is_empty() {
            return Err(format!(
                "Coursera course already completed locally: {}. Confirm re-download to queue again.",
                duplicates.join(", ")
            ));
        }
    }
    let options_json = serde_json::to_string(&opts).map_err(|e| e.to_string())?;
    let jobs: Vec<CourseraJob> = opts
        .class_names
        .iter()
        .map(|c| CourseraJob {
            id: format!("coursera-{}-{}", c, now),
            class_name: c.clone(),
            status: "Queued".to_string(),
            options_json: options_json.clone(),
            output_dir: opts.output_dir.to_string_lossy().to_string(),
            created_at: now,
            updated_at: now,
            counts_json: "{}".to_string(),
        })
        .collect();
    for job in &jobs {
        job::insert_job(connection, job).map_err(|e| e.to_string())?;
        job::append_job_event(
            connection,
            &job.id,
            "queued",
            &serde_json::json!({
                "className": job.class_name,
                "message": if force_redownload {
                    "Coursera course queued for re-download"
                } else {
                    "Coursera course queued"
                },
                "forceRedownload": force_redownload
            })
            .to_string(),
            now,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(jobs)
}

#[tauri::command]
pub fn process_next_queued_coursera_job(
    state: State<'_, CourseraState>,
) -> Result<ProcessCourseraResponse, String> {
    state.cancellation.store(false, Ordering::Relaxed);
    let connection = state.connection()?;
    process_next_queued_coursera_job_with_connection(
        &connection,
        &state.data_dir()?,
        Arc::clone(&state.cancellation),
    )
}

fn process_next_queued_coursera_job_with_connection(
    connection: &Connection,
    data_dir: &std::path::Path,
    cancellation: Arc<AtomicBool>,
) -> Result<ProcessCourseraResponse, String> {
    let Some(job) = job::list_jobs_by_status(&connection, "Queued")
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    else {
        return Ok(ProcessCourseraResponse {
            processed: false,
            completed_artifacts: 0,
            failed_artifacts: 0,
            cancelled_artifacts: 0,
        });
    };
    let now = chrono_now();
    job::update_job_status(connection, &job.id, "Active", now).map_err(|e| e.to_string())?;
    append_coursera_event(
        connection,
        &job.id,
        "job_active",
        serde_json::json!({
            "className": job.class_name.clone(),
            "message": "Coursera job started"
        }),
        now,
    )?;

    match process_coursera_job_live(&job, data_dir, Arc::clone(&cancellation)) {
        Ok((summary, events)) => {
            let now = chrono_now();
            for event in events {
                persist_course_event(connection, &job.id, &event, now)?;
            }
            let status = if cancellation.load(Ordering::Relaxed) {
                "Cancelled"
            } else if summary.failed.is_empty() && summary.completed {
                "Completed"
            } else {
                "Failed"
            };
            let counts_json = serde_json::json!({
                "completed": if status == "Completed" { 1 } else { 0 },
                "failed": summary.failed.len(),
                "skipped": summary.skipped.len(),
                "cancelled": if status == "Cancelled" { 1 } else { 0 }
            })
            .to_string();
            update_job_status_and_counts(connection, &job.id, status, now, &counts_json)?;
            append_coursera_event(
                connection,
                &job.id,
                "job_finished",
                serde_json::json!({
                    "className": job.class_name.clone(),
                    "status": status,
                    "skipped": summary.skipped,
                    "failed": summary.failed
                }),
                now,
            )?;
            Ok(ProcessCourseraResponse {
                processed: true,
                completed_artifacts: if status == "Completed" { 1 } else { 0 },
                failed_artifacts: if status == "Failed" { 1 } else { 0 },
                cancelled_artifacts: if status == "Cancelled" { 1 } else { 0 },
            })
        }
        Err(error) => {
            let now = chrono_now();
            update_job_status_and_counts(
                connection,
                &job.id,
                "Failed",
                now,
                &serde_json::json!({ "completed": 0, "failed": 1, "skipped": 0, "cancelled": 0 })
                    .to_string(),
            )?;
            append_coursera_event(
                connection,
                &job.id,
                "job_failed",
                serde_json::json!({
                    "className": job.class_name.clone(),
                    "message": error
                }),
                now,
            )?;
            Ok(ProcessCourseraResponse {
                processed: true,
                completed_artifacts: 0,
                failed_artifacts: 1,
                cancelled_artifacts: 0,
            })
        }
    }
}

#[tauri::command]
pub fn process_queued_coursera_batch(
    state: State<'_, CourseraState>,
    max: usize,
) -> Result<ProcessCourseraResponse, String> {
    state.cancellation.store(false, Ordering::Relaxed);
    let connection = state.connection()?;
    let mut combined = ProcessCourseraResponse {
        processed: false,
        completed_artifacts: 0,
        failed_artifacts: 0,
        cancelled_artifacts: 0,
    };
    let limit = max.max(1);
    for _ in 0..limit {
        let response = process_next_queued_coursera_job_with_connection(
            &connection,
            &state.data_dir()?,
            Arc::clone(&state.cancellation),
        )?;
        combined.processed |= response.processed;
        combined.completed_artifacts += response.completed_artifacts;
        combined.failed_artifacts += response.failed_artifacts;
        combined.cancelled_artifacts += response.cancelled_artifacts;
        if !response.processed {
            break;
        }
    }
    Ok(combined)
}

fn process_coursera_job_live(
    job: &CourseraJob,
    data_dir: &std::path::Path,
    cancellation: Arc<AtomicBool>,
) -> Result<(CourseSummary, Vec<CourseEvent>), String> {
    let options: CourseraOptions =
        serde_json::from_str(&job.options_json).map_err(|e| e.to_string())?;
    let cauth = auth::read_cached_cauth(data_dir)
        .ok_or_else(|| "Saved Coursera CAUTH token is required before downloading.".to_string())?;
    let cookie_header = auth::make_cookie_values(&cauth)
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let session = auth::AuthSession::from_cauth(cauth, "saved-cauth")
            .await
            .map_err(|e| e.to_string())?;
        let syllabus = crate::coursera::syllabus::fetch_syllabus(&session.client, &job.class_name)
            .await
            .map_err(|e| e.to_string())?;
        let modules =
            crate::coursera::syllabus::parse_syllabus(&syllabus).map_err(|e| e.to_string())?;
        let events: Arc<Mutex<Vec<CourseEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_events = Arc::clone(&events);
        let on_event: Arc<dyn Fn(CourseEvent) + Send + Sync> = Arc::new(move |event| {
            if let Ok(mut guard) = captured_events.lock() {
                guard.push(event);
            }
        });
        let downloader =
            NativeDownloader::with_cookie_header(Some(cookie_header)).map_err(|e| e.to_string())?;
        let output_root = options.output_dir.clone();
        let coursera_downloader = CourseraDownloader {
            client: &session.client,
            options: &options,
            output_root: &output_root,
            downloader: Arc::new(downloader),
            cancellation,
            slug: &job.class_name,
            on_event: Some(on_event),
        };
        let summary = coursera_downloader
            .download_modules(modules)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<(CourseSummary, Vec<CourseEvent>), String>((
            summary,
            events
                .lock()
                .map(|events| events.clone())
                .unwrap_or_default(),
        ))
    })
}

fn persist_course_event(
    connection: &Connection,
    job_id: &str,
    event: &CourseEvent,
    created_at: i64,
) -> Result<(), String> {
    let payload = serde_json::to_value(event).map_err(|e| e.to_string())?;
    let event_type = payload
        .get("kind")
        .and_then(|kind| kind.as_str())
        .unwrap_or("event")
        .to_string();
    append_coursera_event(connection, job_id, &event_type, payload, created_at)
}

fn append_coursera_event(
    connection: &Connection,
    job_id: &str,
    event_type: &str,
    payload: serde_json::Value,
    created_at: i64,
) -> Result<(), String> {
    job::append_job_event(
        connection,
        job_id,
        event_type,
        &payload.to_string(),
        created_at,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn update_job_status_and_counts(
    connection: &Connection,
    job_id: &str,
    status: &str,
    updated_at: i64,
    counts_json: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE coursera_jobs SET status = ?1, updated_at = ?2, counts_json = ?3 WHERE id = ?4",
            rusqlite::params![status, updated_at, counts_json, job_id],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cancel_active_coursera_download(state: State<'_, CourseraState>) -> Result<bool, String> {
    state.cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

#[tauri::command]
pub fn retry_failed_coursera_job(
    state: State<'_, CourseraState>,
    job_id: String,
) -> Result<CourseraJob, String> {
    let connection = state.connection()?;
    let now = chrono_now();
    job::retry_failed_job(&connection, &job_id, now)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Coursera job was not found or is no longer failed/cancelled".to_string())
}

#[tauri::command]
pub fn clear_failed_coursera_jobs(state: State<'_, CourseraState>) -> Result<usize, String> {
    let connection = state.connection()?;
    job::clear_failed_jobs(&connection).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_failed_coursera_job(
    state: State<'_, CourseraState>,
    job_id: String,
) -> Result<bool, String> {
    let connection = state.connection()?;
    if !job::delete_failed_job(&connection, &job_id).map_err(|e| e.to_string())? {
        return Err("Coursera job was not found or is no longer failed/cancelled".to_string());
    }
    Ok(true)
}

#[tauri::command]
pub fn list_coursera_history(
    state: State<'_, CourseraState>,
) -> Result<Vec<CourseraHistoryEntry>, String> {
    let connection = state.connection()?;
    Ok(job::list_completed_jobs(&connection)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|job| CourseraHistoryEntry {
            last_event_at: Some(job.updated_at),
            job,
        })
        .collect())
}

#[tauri::command]
pub fn open_coursera_download_folder(
    state: State<'_, CourseraState>,
    job_id: String,
) -> Result<String, String> {
    let connection = state.connection()?;
    let jobs = job::list_recent_jobs(&connection, 500).map_err(|e| e.to_string())?;
    let Some(job) = jobs.into_iter().find(|job| job.id == job_id) else {
        return Err(format!("Coursera job not found: {}", job_id));
    };
    Ok(job.output_dir)
}

#[tauri::command]
pub async fn fetch_coursera_syllabus_preview(slug: String) -> Result<SyllabusPreview, String> {
    let client = crate::coursera::client::build_client().map_err(|e| e.to_string())?;
    let json = crate::coursera::syllabus::fetch_syllabus(&client, &slug)
        .await
        .map_err(|e| e.to_string())?;
    let modules: ModulesV1 =
        crate::coursera::syllabus::parse_syllabus(&json).map_err(|e| e.to_string())?;
    let lesson_count = modules.modules.iter().map(|m| m.lessons.len()).sum();
    let total_items = modules
        .modules
        .iter()
        .flat_map(|m| m.lessons.iter())
        .map(|l| l.items.len())
        .sum();
    let has_quizzes = modules
        .modules
        .iter()
        .flat_map(|m| m.lessons.iter())
        .flat_map(|l| l.items.iter())
        .any(|i| i.type_name == "quiz" || i.type_name == "exam");
    let has_notebooks = modules
        .modules
        .iter()
        .flat_map(|m| m.lessons.iter())
        .flat_map(|l| l.items.iter())
        .any(|i| i.type_name == "notebook");
    Ok(SyllabusPreview {
        slug,
        module_count: modules.modules.len(),
        lesson_count,
        total_items,
        has_quizzes,
        has_notebooks,
    })
}

fn chrono_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::cache::initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn parse_coursera_class_input_handles_blank_lines() {
        let result = parse_coursera_class_input("ml-005\n\n  algo  ".to_string()).unwrap();
        // The course parser trims and accepts 'algo' (lowercase, digits, dash).
        let _ = result;
    }

    #[test]
    fn bootstrap_returns_a_sensible_default_state() {
        let connection = fresh_connection();
        let result = load_coursera_bootstrap_state(&connection, false).unwrap();
        assert!(!result.has_saved_token);
        assert!(result.persisted_jobs.is_empty());
        assert!(result.default_options.jobs >= 1);
    }

    #[test]
    fn start_request_persists_one_job_per_class() {
        let connection = fresh_connection();
        let req = StartCourseraRequest {
            classes: vec!["a".to_string(), "b".to_string()],
            output_dir: ".".to_string(),
            force_redownload: false,
            selected_resolution: "540p".to_string(),
            formats: Vec::new(),
            ignored_formats: Vec::new(),
            subtitle_language: "all".to_string(),
            download_quizzes: false,
            download_notebooks: false,
            download_about: false,
            resume: false,
            overwrite: false,
            generate_playlists: false,
            section_filter: String::new(),
            lecture_filter: String::new(),
            resource_filter: String::new(),
            jobs: 1,
            download_delay_seconds: 60,
        };
        let jobs = queue_coursera_download_jobs(&connection, req, 123).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].status, "Queued");
        let persisted = job::list_recent_jobs(&connection, 10).unwrap();
        assert_eq!(persisted.len(), 2);
        let events = job::list_recent_events(&connection, 10).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn start_request_rejects_completed_class_without_force_redownload() {
        let connection = fresh_connection();
        let completed = CourseraJob {
            id: "completed-a".to_string(),
            class_name: "a".to_string(),
            status: "Completed".to_string(),
            options_json: "{}".to_string(),
            output_dir: ".".to_string(),
            created_at: 1,
            updated_at: 2,
            counts_json: "{}".to_string(),
        };
        job::insert_job(&connection, &completed).unwrap();

        let mut req = StartCourseraRequest {
            classes: vec!["a".to_string()],
            output_dir: ".".to_string(),
            force_redownload: false,
            selected_resolution: "540p".to_string(),
            formats: Vec::new(),
            ignored_formats: Vec::new(),
            subtitle_language: "all".to_string(),
            download_quizzes: false,
            download_notebooks: false,
            download_about: false,
            resume: false,
            overwrite: false,
            generate_playlists: false,
            section_filter: String::new(),
            lecture_filter: String::new(),
            resource_filter: String::new(),
            jobs: 1,
            download_delay_seconds: 60,
        };

        let err = queue_coursera_download_jobs(&connection, req.clone(), 123)
            .expect_err("completed class should require confirmation");
        assert!(err.contains("already completed"));

        req.force_redownload = true;
        let jobs = queue_coursera_download_jobs(&connection, req, 124).unwrap();
        assert_eq!(jobs.len(), 1);
        let events = job::list_job_events(&connection, &jobs[0].id, 10).unwrap();
        assert!(events
            .iter()
            .any(|event| event.payload_json.contains("forceRedownload\":true")));
    }

    #[test]
    fn processing_queued_job_records_missing_token_failure() {
        let connection = fresh_connection();
        let temp = tempfile::tempdir().unwrap();
        let req = StartCourseraRequest {
            classes: vec!["ml-005".to_string()],
            output_dir: ".".to_string(),
            force_redownload: false,
            selected_resolution: "540p".to_string(),
            formats: Vec::new(),
            ignored_formats: Vec::new(),
            subtitle_language: "all".to_string(),
            download_quizzes: false,
            download_notebooks: false,
            download_about: false,
            resume: false,
            overwrite: false,
            generate_playlists: false,
            section_filter: String::new(),
            lecture_filter: String::new(),
            resource_filter: String::new(),
            jobs: 1,
            download_delay_seconds: 60,
        };
        let jobs = queue_coursera_download_jobs(&connection, req, 123).unwrap();
        let response = process_next_queued_coursera_job_with_connection(
            &connection,
            temp.path(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(response.processed);
        assert_eq!(response.failed_artifacts, 1);
        let failed = job::list_jobs_by_status(&connection, "Failed").unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].id, jobs[0].id);
        let events = job::list_job_events(&connection, &jobs[0].id, 10).unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == "job_failed" && event.payload_json.contains("CAUTH")
        }));
    }
}
