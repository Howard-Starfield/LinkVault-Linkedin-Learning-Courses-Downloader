//! Tauri command surface for the Coursera tab.
//!
//! All commands return `Result<T, String>` per Tauri 2 conventions.
//! Each public function is registered in `lib.rs`'s `invoke_handler!`
//! macro. They are the only entry points the React side can call.

#![allow(dead_code)] // Phase 10 — wired at end of phase

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::cache::clear_coursera_provider_data;
use crate::cache::CourseraResetCounts;
use crate::coursera::auth;
use crate::coursera::config::{parse_class_input, SavedCourseraPreferences, StartCourseraRequest};
use crate::coursera::coursera_token_store;
use crate::coursera::job::{self, CourseraJob, PersistedCourseraEvent};
use crate::coursera::projection::{
    self, merge_coursera_jobs, workflow_event_to_coursera_event, workflow_run_to_coursera_job,
    CourseraWorkflowRequest,
};
use crate::coursera::syllabus::ModulesV1;
use crate::workflow::application::runtime::WorkflowRuntime;
use crate::workflow::domain::state::RunState;

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
        Self::with_cancellation(db_path, Arc::new(AtomicBool::new(false)))
    }

    pub fn with_cancellation(db_path: PathBuf, cancellation: Arc<AtomicBool>) -> Self {
        Self {
            db_path,
            cancellation,
        }
    }

    fn connection(&self) -> Result<Connection, String> {
        crate::cache::open_runtime(&self.db_path).map_err(|e| e.to_string())
    }

    fn data_dir(&self) -> Result<PathBuf, String> {
        self.db_path
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| "database path has no parent directory".to_string())
    }

    fn db_path(&self) -> PathBuf {
        self.db_path.clone()
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
    runtime: State<'_, WorkflowRuntime>,
) -> Result<CourseraBootstrapState, String> {
    let connection = state.connection()?;
    load_coursera_bootstrap_state(
        &connection,
        &runtime,
        has_saved_coursera_token_for_dir(&state.data_dir()?)?,
    )
}

fn load_coursera_bootstrap_state(
    connection: &Connection,
    runtime: &WorkflowRuntime,
    has_saved_token: bool,
) -> Result<CourseraBootstrapState, String> {
    let saved_prefs =
        match job::load_setting(connection, COURSERA_PREFS_KEY).map_err(|e| e.to_string())? {
            Some(json) => Some(serde_json::from_str(&json).map_err(|e| e.to_string())?),
            None => None,
        };
    let workflow_runs = runtime.list_coursera_runs(250).map_err(|e| e.to_string())?;
    let workflow_jobs: Vec<CourseraJob> = workflow_runs
        .iter()
        .map(workflow_run_to_coursera_job)
        .collect();
    let mut workflow_events = Vec::new();
    for run in &workflow_runs {
        let events = runtime
            .list_events(run.id.clone())
            .map_err(|e| e.to_string())?;
        workflow_events.extend(
            events
                .into_iter()
                .map(|event| workflow_event_to_coursera_event(&event)),
        );
    }
    let persisted_jobs = merge_coursera_jobs(
        workflow_jobs,
        job::list_recent_jobs(connection, 250).map_err(|e| e.to_string())?,
    );
    let mut recent_events = workflow_events;
    recent_events.extend(job::list_recent_events(connection, 100).map_err(|e| e.to_string())?);
    recent_events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    recent_events.truncate(100);
    let download_history = persisted_jobs
        .iter()
        .filter(|job| job.status.eq_ignore_ascii_case("completed"))
        .cloned()
        .map(|job| CourseraHistoryEntry {
            last_event_at: Some(job.updated_at),
            job,
        })
        .collect();
    Ok(CourseraBootstrapState {
        default_options: SavedCourseraPreferences::default(),
        has_saved_token,
        saved_prefs,
        persisted_jobs,
        recent_events,
        download_history,
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
    runtime: State<'_, WorkflowRuntime>,
    req: StartCourseraRequest,
) -> Result<Vec<CourseraJob>, String> {
    let connection = state.connection()?;
    queue_coursera_download_jobs(&runtime, &connection, req, chrono_now())
}

fn queue_coursera_download_jobs(
    runtime: &WorkflowRuntime,
    connection: &Connection,
    req: StartCourseraRequest,
    now: i64,
) -> Result<Vec<CourseraJob>, String> {
    let force_redownload = req.force_redownload;
    let opts: crate::coursera::config::CourseraOptions =
        req.into_options().map_err(|e| e.to_string())?;
    if !force_redownload {
        let workflow_runs = runtime.list_coursera_runs(500).map_err(|e| e.to_string())?;
        let legacy = job::list_completed_jobs(connection).map_err(|e| e.to_string())?;
        let completed = projection::completed_class_names(&legacy, &workflow_runs);
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
    let mut jobs = Vec::new();
    for class_name in &opts.class_names {
        let mut class_opts = opts.clone();
        class_opts.class_names = vec![class_name.clone()];
        let request = CourseraWorkflowRequest {
            schema_version: 1,
            class_name: class_name.clone(),
            force_redownload,
            options: class_opts,
        };
        let request_json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        let run_id = format!("coursera-{class_name}-{now}");
        runtime
            .submit_coursera_download(
                run_id,
                class_name.clone(),
                request_json,
                opts.output_dir.to_string_lossy().to_string(),
                now,
            )
            .map_err(|e| e.to_string())?;
        jobs.push(CourseraJob {
            id: format!("coursera-{class_name}-{now}"),
            class_name: class_name.clone(),
            status: "Queued".to_string(),
            options_json: serde_json::to_string(&opts).map_err(|e| e.to_string())?,
            output_dir: opts.output_dir.to_string_lossy().to_string(),
            created_at: now,
            updated_at: now,
            counts_json: "{}".to_string(),
        });
    }
    Ok(jobs)
}

#[tauri::command]
pub async fn process_next_queued_coursera_job(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
) -> Result<ProcessCourseraResponse, String> {
    state.cancellation.store(false, Ordering::Relaxed);
    let runtime = (*runtime).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let outcome = runtime
            .drain_type("coursera_download")
            .map_err(|e| e.to_string())?;
        Ok(ProcessCourseraResponse {
            processed: outcome.processed,
            completed_artifacts: outcome.completed,
            failed_artifacts: outcome.failed,
            cancelled_artifacts: outcome.cancelled,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn process_queued_coursera_batch(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
    max: usize,
) -> Result<ProcessCourseraResponse, String> {
    state.cancellation.store(false, Ordering::Relaxed);
    let runtime = (*runtime).clone();
    let limit = max.max(1);
    tauri::async_runtime::spawn_blocking(move || {
        let mut combined = ProcessCourseraResponse {
            processed: false,
            completed_artifacts: 0,
            failed_artifacts: 0,
            cancelled_artifacts: 0,
        };
        for _ in 0..limit {
            let outcome = runtime
                .drain_type("coursera_download")
                .map_err(|e| e.to_string())?;
            combined.processed |= outcome.processed;
            combined.completed_artifacts += outcome.completed;
            combined.failed_artifacts += outcome.failed;
            combined.cancelled_artifacts += outcome.cancelled;
            if !outcome.processed {
                break;
            }
        }
        Ok(combined)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cancel_active_coursera_download(state: State<'_, CourseraState>) -> Result<bool, String> {
    state.cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

#[tauri::command]
pub fn reset_coursera_database(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
) -> Result<CourseraResetCounts, String> {
    // The UI is expected to call cancel_active_coursera_download first so the
    // in-flight worker unwinds at a safe boundary. Defensive re-arm of the
    // cancellation flag here keeps a stale request from writing after the
    // wipe commits.
    state.cancellation.store(true, Ordering::Relaxed);
    runtime.delete_coursera_runs().map_err(|e| e.to_string())?;
    let connection = state.connection()?;
    let counts = clear_coursera_provider_data(&connection).map_err(|e| e.to_string())?;
    state.cancellation.store(false, Ordering::Relaxed);
    Ok(counts)
}

#[tauri::command]
pub fn retry_failed_coursera_job(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<CourseraJob, String> {
    retry_failed_coursera_job_inner(&runtime, Some(&state.connection()?), job_id, chrono_now())
}

fn retry_failed_coursera_job_inner(
    runtime: &WorkflowRuntime,
    connection: Option<&Connection>,
    job_id: String,
    now: i64,
) -> Result<CourseraJob, String> {
    if let Some(run) = runtime.get_run(job_id.clone()).map_err(|e| e.to_string())? {
        if !matches!(run.state, RunState::Failed | RunState::Cancelled) {
            return Err("Coursera job was not found or is no longer failed/cancelled".to_string());
        }
        let job = workflow_run_to_coursera_job(&run);
        let new_id = format!("coursera-{}-{}", job.class_name, now);
        runtime
            .submit_coursera_download(
                new_id.clone(),
                job.class_name.clone(),
                run.request_json,
                run.output_root,
                now,
            )
            .map_err(|e| e.to_string())?;
        return Ok(CourseraJob {
            id: new_id,
            class_name: job.class_name,
            status: "Queued".to_string(),
            options_json: job.options_json,
            output_dir: job.output_dir,
            created_at: now,
            updated_at: now,
            counts_json: "{}".to_string(),
        });
    }
    let Some(connection) = connection else {
        return Err("Coursera job was not found or is no longer failed/cancelled".to_string());
    };
    let Some(legacy) = job::get_job(connection, &job_id).map_err(|e| e.to_string())? else {
        return Err("Coursera job was not found or is no longer failed/cancelled".to_string());
    };
    if !matches!(
        legacy.status.to_ascii_lowercase().as_str(),
        "failed" | "cancelled"
    ) {
        return Err("Coursera job was not found or is no longer failed/cancelled".to_string());
    }
    let options: crate::coursera::config::CourseraOptions =
        serde_json::from_str(&legacy.options_json).map_err(|e| e.to_string())?;
    let request = CourseraWorkflowRequest {
        schema_version: 1,
        class_name: legacy.class_name.clone(),
        force_redownload: true,
        options,
    };
    let new_id = format!("coursera-{}-{}", legacy.class_name, now);
    runtime
        .submit_coursera_download(
            new_id.clone(),
            legacy.class_name.clone(),
            serde_json::to_string(&request).map_err(|e| e.to_string())?,
            legacy.output_dir.clone(),
            now,
        )
        .map_err(|e| e.to_string())?;
    Ok(CourseraJob {
        id: new_id,
        class_name: legacy.class_name,
        status: "Queued".to_string(),
        options_json: legacy.options_json,
        output_dir: legacy.output_dir,
        created_at: now,
        updated_at: now,
        counts_json: "{}".to_string(),
    })
}

#[tauri::command]
pub fn clear_failed_coursera_jobs(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
) -> Result<usize, String> {
    let workflow_removed = runtime
        .delete_terminal_coursera_runs()
        .map_err(|e| e.to_string())?;
    let connection = state.connection()?;
    let legacy_removed = job::clear_failed_jobs(&connection).map_err(|e| e.to_string())?;
    Ok(workflow_removed + legacy_removed)
}

#[tauri::command]
pub fn remove_failed_coursera_job(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<bool, String> {
    if runtime
        .delete_run_if_terminal(job_id.clone())
        .map_err(|e| e.to_string())?
    {
        return Ok(true);
    }
    let connection = state.connection()?;
    if !job::delete_failed_job(&connection, &job_id).map_err(|e| e.to_string())? {
        return Err("Coursera job was not found or is no longer failed/cancelled".to_string());
    }
    Ok(true)
}

#[tauri::command]
pub fn list_coursera_history(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
) -> Result<Vec<CourseraHistoryEntry>, String> {
    let bootstrap = load_coursera_bootstrap_state(&state.connection()?, &runtime, false)?;
    Ok(bootstrap.download_history)
}

#[tauri::command]
pub fn open_coursera_download_folder(
    state: State<'_, CourseraState>,
    runtime: State<'_, WorkflowRuntime>,
    job_id: String,
) -> Result<String, String> {
    if let Some(run) = runtime.get_run(job_id.clone()).map_err(|e| e.to_string())? {
        crate::shell::open_folder_in_explorer(std::path::Path::new(&run.output_root))?;
        return Ok(run.output_root);
    }
    let connection = state.connection()?;
    let jobs = job::list_recent_jobs(&connection, 500).map_err(|e| e.to_string())?;
    let Some(job) = jobs.into_iter().find(|job| job.id == job_id) else {
        return Err(format!("Coursera job not found: {}", job_id));
    };
    crate::shell::open_folder_in_explorer(std::path::Path::new(&job.output_dir))?;
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
    use std::sync::Arc;

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
        runtime.register_executor(Arc::new(
            crate::coursera::executor::CourseraDownloadExecutor {
                data_dir: directory.path().to_path_buf(),
                cancellation: Arc::new(AtomicBool::new(false)),
            },
        ));
        let connection = crate::cache::open_runtime(&db_path).unwrap();
        (directory, runtime, connection)
    }

    fn sample_request(classes: Vec<String>, force: bool) -> StartCourseraRequest {
        StartCourseraRequest {
            classes,
            output_dir: ".".to_string(),
            force_redownload: force,
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
        }
    }

    #[test]
    fn parse_coursera_class_input_handles_blank_lines() {
        let result = parse_coursera_class_input("ml-005\n\n  algo  ".to_string()).unwrap();
        let _ = result;
    }

    #[test]
    fn bootstrap_returns_a_sensible_default_state() {
        let (_dir, runtime, connection) = workflow_harness();
        let result = load_coursera_bootstrap_state(&connection, &runtime, false).unwrap();
        assert!(!result.has_saved_token);
        assert!(result.persisted_jobs.is_empty());
        assert!(result.default_options.jobs >= 1);
    }

    #[test]
    fn start_request_writes_workflow_runs_not_legacy_jobs() {
        let (_dir, runtime, connection) = workflow_harness();
        let jobs = queue_coursera_download_jobs(
            &runtime,
            &connection,
            sample_request(vec!["a".to_string(), "b".to_string()], false),
            123,
        )
        .unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].status, "Queued");
        assert!(job::list_recent_jobs(&connection, 10).unwrap().is_empty());
        assert_eq!(runtime.list_coursera_runs(10).unwrap().len(), 2);
    }

    #[test]
    fn start_request_rejects_completed_class_without_force_redownload() {
        let (_dir, runtime, connection) = workflow_harness();
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

        let err = queue_coursera_download_jobs(
            &runtime,
            &connection,
            sample_request(vec!["a".to_string()], false),
            123,
        )
        .expect_err("completed class should require confirmation");
        assert!(err.contains("already completed"));

        let jobs = queue_coursera_download_jobs(
            &runtime,
            &connection,
            sample_request(vec!["a".to_string()], true),
            124,
        )
        .unwrap();
        assert_eq!(jobs.len(), 1);
        let run = runtime.get_run(jobs[0].id.clone()).unwrap().unwrap();
        assert!(run.request_json.contains("forceRedownload\":true"));
    }

    #[test]
    fn processing_queued_job_records_missing_token_failure() {
        let (_dir, runtime, connection) = workflow_harness();
        let jobs = queue_coursera_download_jobs(
            &runtime,
            &connection,
            sample_request(vec!["ml-005".to_string()], false),
            123,
        )
        .unwrap();
        let outcome = runtime.drain_once().unwrap();
        assert!(outcome.processed);
        assert_eq!(outcome.failed, 1);
        let run = runtime.get_run(jobs[0].id.clone()).unwrap().unwrap();
        assert_eq!(run.state, RunState::Failed);
        let events = runtime.list_events(jobs[0].id.clone()).unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == "run_failed"
                && run.error_message.as_deref().unwrap_or("").contains("CAUTH")
        }));
        assert!(job::list_jobs_by_status(&connection, "Failed")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn retry_failed_workflow_run_submits_a_new_queued_run() {
        let (_dir, runtime, connection) = workflow_harness();
        let jobs = queue_coursera_download_jobs(
            &runtime,
            &connection,
            sample_request(vec!["ml-005".to_string()], false),
            123,
        )
        .unwrap();
        runtime.drain_once().unwrap();
        let retried =
            retry_failed_coursera_job_inner(&runtime, Some(&connection), jobs[0].id.clone(), 999)
                .unwrap();
        assert_ne!(retried.id, jobs[0].id);
        assert_eq!(retried.status, "Queued");
        assert_eq!(runtime.list_coursera_runs(10).unwrap().len(), 2);
        let original = runtime.get_run(jobs[0].id.clone()).unwrap().unwrap();
        assert_eq!(original.state, RunState::Failed);
        let queued = runtime.get_run(retried.id).unwrap().unwrap();
        assert_eq!(queued.state, RunState::Queued);
    }

    #[test]
    fn reset_deletes_workflow_runs() {
        let (_dir, runtime, connection) = workflow_harness();
        queue_coursera_download_jobs(
            &runtime,
            &connection,
            sample_request(vec!["ml-005".to_string()], false),
            123,
        )
        .unwrap();
        assert_eq!(runtime.delete_coursera_runs().unwrap(), 1);
        assert!(runtime.list_coursera_runs(10).unwrap().is_empty());
    }

    #[test]
    fn live_job_processing_uses_blocking_pool_not_a_nested_multi_thread_runtime() {
        let source = include_str!("commands.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(
            production.contains("tauri::async_runtime::spawn_blocking"),
            "Coursera job commands must leave the async executor via spawn_blocking"
        );
        assert!(
            include_str!("executor.rs").contains("Builder::new_current_thread"),
            "async Coursera work must use a current-thread runtime, not Runtime::new()"
        );
        assert!(
            !production.contains("tokio::runtime::Runtime::new()"),
            "per-job Runtime::new() nests a multi-thread runtime on the command path"
        );
        assert!(
            production.contains("pub async fn process_next_queued_coursera_job"),
            "process_next_queued_coursera_job must be async so Tauri does not pin the runtime"
        );
        assert!(
            production.contains("pub async fn process_queued_coursera_batch"),
            "process_queued_coursera_batch must be async so Tauri does not pin the runtime"
        );
    }
}
