//! Newspaper step executor. Download work stays provider-owned.

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use rusqlite::params;

use crate::workflow::domain::types::{RunRecord, StepRecord};
use crate::workflow::ports::executor::{ExecutorOutcome, StepExecutor};

use super::client::NewspaperClient;
use super::models::NewspaperJob;
use super::projection::NewspaperWorkflowRequest;
use super::queue_service;
use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};

pub struct NewspaperDownloadExecutor {
    pub db_path: PathBuf,
    pub writer: DatabaseWriter,
    pub cancellation: Arc<AtomicBool>,
    pub on_changed: Arc<dyn Fn(&str) + Send + Sync>,
}

impl StepExecutor for NewspaperDownloadExecutor {
    fn workflow_type(&self) -> &'static str {
        "newspaper_download"
    }

    fn execute(&self, run: &RunRecord, _step: &StepRecord) -> ExecutorOutcome {
        let request = match serde_json::from_str::<NewspaperWorkflowRequest>(&run.request_json) {
            Ok(request) => request,
            Err(_) if self.cancellation.load(Ordering::SeqCst) => {
                return ExecutorOutcome::cancelled("Newspaper download was cancelled".to_string())
            }
            Err(error) => return ExecutorOutcome::failed(error.to_string()),
        };
        (self.on_changed)(&run.id);
        let outcome = match download_newspaper_run(
            &self.db_path,
            &self.writer,
            run,
            &request,
            &self.cancellation,
        ) {
            Ok(status) => match status.as_str() {
                // Download finished. `optimizing` means pages are on disk and the
                // image pass is next; treat it as success so the workflow run
                // does not fail before per-edition optimize can start.
                "completed" | "partial" | "optimizing" => {
                    ExecutorOutcome::succeeded(serde_json::json!({ "status": status }).to_string())
                }
                "queued" | "awaiting_release" => ExecutorOutcome {
                    succeeded: true,
                    cancelled: false,
                    warning: true,
                    retryable: false,
                    error_message: Some(format!(
                        "Newspaper download returned to the queue ({status})"
                    )),
                    payload_json: serde_json::json!({ "status": status }).to_string(),
                },
                "unavailable" => {
                    ExecutorOutcome::failed("Edition has not been released yet.".to_string())
                }
                "cancelled" => {
                    ExecutorOutcome::cancelled("Newspaper download was cancelled".to_string())
                }
                other => ExecutorOutcome::failed(format!(
                    "Newspaper download finished with status {other}"
                )),
            },
            Err(error) => {
                if self.cancellation.load(Ordering::SeqCst) {
                    ExecutorOutcome::cancelled(error)
                } else {
                    ExecutorOutcome::failed(error)
                }
            }
        };
        (self.on_changed)(&run.id);
        outcome
    }
}

fn download_newspaper_run(
    db_path: &std::path::Path,
    writer: &DatabaseWriter,
    run: &RunRecord,
    request: &NewspaperWorkflowRequest,
    cancelled: &Arc<AtomicBool>,
) -> Result<String, String> {
    let ready = materialize_job(writer, run, request)?;
    let job = NewspaperJob {
        id: run.id.clone(),
        batch_id: request.batch_id.clone(),
        edition_code: request.edition_code.clone(),
        edition_name: request.edition_name.clone(),
        publication_date: request.publication_date.clone(),
        status: "queued".to_string(),
        output_dir: run.output_root.clone(),
        page_count: 0,
        completed_count: 0,
        failed_count: 0,
        retry_at: None,
        retry_count: 0,
        warning: None,
        queue_position: request.queue_position,
        paused: false,
        dismissed: false,
        created_at: run.created_at,
        updated_at: run.updated_at,
        completed_at: None,
    };
    if !ready || cancelled.load(Ordering::SeqCst) {
        let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
        return match super::job_repository::find(&connection, &job.id)? {
            Some(_) => queue_service::apply_interrupted_job_state_on_writer(writer, &job)
                .map(str::to_string),
            None => Ok("cancelled".to_string()),
        };
    }
    let client = NewspaperClient::new().map_err(|error| error.to_string())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let db_path = db_path.to_path_buf();
    let cancelled = Arc::clone(cancelled);
    let finished = rt.block_on(queue_service::process_job(
        &db_path, writer, &client, job, &cancelled,
    ))?;
    Ok(finished.status)
}

pub(super) fn materialize_job(
    writer: &DatabaseWriter,
    run: &RunRecord,
    request: &NewspaperWorkflowRequest,
) -> Result<bool, String> {
    let run = run.clone();
    let request = request.clone();
    writer
        .execute(
            DatabaseWriteContext {
                operation: "newspaper_materialize_job",
                provider: DatabaseProvider::Newspaper,
                workflow_id: Some(run.id.clone()),
            },
            move |connection| {
                let eligible: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM newspaper_batches b WHERE b.id = ?1
             AND b.status IN ('queued', 'scheduled', 'active')
             AND NOT EXISTS(SELECT 1 FROM newspaper_jobs j WHERE j.id = ?2
                 AND (j.paused = 1 OR j.dismissed = 1 OR j.status = 'cancelled')))",
                    params![request.batch_id, run.id],
                    |row| row.get(0),
                )?;
                let exists: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM newspaper_batches WHERE id = ?1)",
                    [&request.batch_id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Ok(false);
                }
                connection.execute(
                    "INSERT INTO newspaper_jobs
            (id, batch_id, edition_code, edition_publication_date, publication_date,
             status, output_dir, queue_position, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?8, ?8)
            ON CONFLICT(id) DO NOTHING",
                    params![
                        run.id,
                        request.batch_id,
                        request.edition_code,
                        request.edition_publication_date,
                        request.publication_date,
                        run.output_root,
                        request.queue_position,
                        run.created_at,
                    ],
                )?;
                Ok(eligible)
            },
        )
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::domain::state::{RunState, StepState};
    use crate::workflow::domain::types::{StepType, WorkflowType};

    #[test]
    fn cancelled_flag_fails_closed_without_network() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.sqlite3");
        crate::cache::initialize_database(&path).unwrap();
        let writer = DatabaseWriter::start(
            path,
            crate::app::database_diagnostics::DatabaseDiagnostics::default(),
        )
        .unwrap();
        let executor = NewspaperDownloadExecutor {
            db_path: PathBuf::from("missing.sqlite3"),
            writer,
            cancellation: Arc::new(AtomicBool::new(true)),
            on_changed: Arc::new(|_| {}),
        };
        let run = RunRecord {
            id: "newspaper-job-1".to_string(),
            workflow_type: WorkflowType::newspaper_download(),
            provider: "newspaper".to_string(),
            state: RunState::Running,
            legacy_origin: None,
            legacy_id: None,
            request_json: "{}".to_string(),
            output_root: ".".to_string(),
            error_message: None,
            created_at: 1,
            updated_at: 1,
            completed_at: None,
        };
        let step = StepRecord {
            id: "step".to_string(),
            run_id: run.id.clone(),
            step_key: "NY".to_string(),
            step_type: StepType::newspaper_execute(),
            state: StepState::Running,
            attempt: 1,
            error_message: None,
            created_at: 1,
            updated_at: 1,
        };
        let outcome = executor.execute(&run, &step);
        assert!(outcome.cancelled);
        assert!(!outcome.succeeded);
    }

    #[test]
    fn paused_unmaterialized_batch_and_kernel_owned_job_never_enter_legacy_queue() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.sqlite3");
        let (connection, _) = crate::cache::initialize_database(&path).unwrap();
        let writer = DatabaseWriter::start(
            path,
            crate::app::database_diagnostics::DatabaseDiagnostics::default(),
        )
        .unwrap();
        let runtime = crate::workflow::application::runtime::WorkflowRuntime::new(writer.clone());
        let response = super::super::batch_service::create(
            &writer,
            super::super::models::CreateNewspaperBatchRequest {
                edition_codes: vec!["NY".to_string()],
                date_mode: super::super::models::DateMode::Single,
                start_date: "2026-10-01".to_string(),
                end_date: None,
                destination: directory
                    .path()
                    .join("papers")
                    .to_string_lossy()
                    .into_owned(),
                scheduled_at: None,
                delay_seconds: 0,
                optimize_images: false,
                optimization_profile: "webp_high".to_string(),
                optimization_quality: 92,
                keep_original_jpg: true,
            },
        )
        .unwrap();
        let job = &response.jobs[0];
        let run = runtime.get_run(job.id.clone()).unwrap().unwrap();
        let request: NewspaperWorkflowRequest = serde_json::from_str(&run.request_json).unwrap();
        connection
            .execute(
                "UPDATE newspaper_batches SET status = 'paused' WHERE id = ?1",
                [&job.batch_id],
            )
            .unwrap();
        assert!(!materialize_job(&writer, &run, &request).unwrap());
        let executor = NewspaperDownloadExecutor {
            db_path: connection.path().map(PathBuf::from).unwrap(),
            writer: writer.clone(),
            cancellation: Arc::new(AtomicBool::new(true)),
            on_changed: Arc::new(|_| {}),
        };
        let step = StepRecord {
            id: "step".to_string(),
            run_id: run.id.clone(),
            step_key: "NY".to_string(),
            step_type: StepType::newspaper_execute(),
            state: StepState::Running,
            attempt: 1,
            error_message: None,
            created_at: 1,
            updated_at: 1,
        };
        let outcome = executor.execute(&run, &step);
        assert!(outcome.succeeded && outcome.warning && !outcome.cancelled);
        assert!(!queue_service::activate_queued_job(
            &writer,
            &job.id,
            &job.batch_id,
            chrono::Utc::now().timestamp()
        )
        .unwrap());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM newspaper_jobs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        connection
            .execute(
                "UPDATE newspaper_batches SET status = 'queued' WHERE id = ?1",
                [&job.batch_id],
            )
            .unwrap();
        assert!(materialize_job(&writer, &run, &request).unwrap());
        // The executor has inserted a queued compatibility row but has not
        // activated it yet. The legacy queue must not claim this same job.
        assert!(queue_service::next_due_job(&connection).unwrap().is_none());
        connection
            .execute(
                "UPDATE workflow_runs SET state = 'succeeded_with_warnings' WHERE id = ?1",
                [&job.id],
            )
            .unwrap();
        assert_eq!(
            queue_service::next_due_job(&connection)
                .unwrap()
                .unwrap()
                .0
                .id,
            job.id
        );
        connection
            .execute(
                "UPDATE newspaper_jobs SET paused = 1, status = 'cancelled' WHERE id = ?1",
                [&job.id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE newspaper_batches SET status = 'cancelled' WHERE id = ?1",
                [&job.batch_id],
            )
            .unwrap();
        assert_eq!(
            queue_service::apply_interrupted_job_state(
                directory.path().join("test.sqlite3").as_path(),
                job
            )
            .unwrap(),
            "cancelled"
        );
        writer.shutdown().unwrap();
    }

    #[test]
    fn optimizing_status_is_treated_as_download_success() {
        let source = include_str!("executor.rs");
        let match_arm = source
            .split("Ok(status) => match status.as_str()")
            .nth(1)
            .and_then(|rest| rest.split("\"unavailable\"").next())
            .unwrap_or_default();
        assert!(
            match_arm.contains("\"optimizing\""),
            "workflow executor must treat optimizing as success so per-edition optimize can run"
        );
        assert!(
            match_arm.contains("ExecutorOutcome::succeeded"),
            "optimizing must map to succeeded, not failed"
        );
    }
}
