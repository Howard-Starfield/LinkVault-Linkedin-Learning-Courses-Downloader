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

pub struct NewspaperDownloadExecutor {
    pub db_path: PathBuf,
    pub cancellation: Arc<AtomicBool>,
}

impl StepExecutor for NewspaperDownloadExecutor {
    fn workflow_type(&self) -> &'static str {
        "newspaper_download"
    }

    fn execute(&self, run: &RunRecord, _step: &StepRecord) -> ExecutorOutcome {
        if self.cancellation.load(Ordering::SeqCst) {
            return ExecutorOutcome::cancelled("Newspaper download was cancelled".to_string());
        }
        let request = match serde_json::from_str::<NewspaperWorkflowRequest>(&run.request_json) {
            Ok(request) => request,
            Err(error) => return ExecutorOutcome::failed(error.to_string()),
        };
        match download_newspaper_run(&self.db_path, run, &request, &self.cancellation) {
            Ok(status) => match status.as_str() {
                "completed" | "partial" => {
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
        }
    }
}

fn download_newspaper_run(
    db_path: &std::path::Path,
    run: &RunRecord,
    request: &NewspaperWorkflowRequest,
    cancelled: &Arc<AtomicBool>,
) -> Result<String, String> {
    materialize_job(db_path, run, request)?;
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
    let client = NewspaperClient::new().map_err(|error| error.to_string())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let db_path = db_path.to_path_buf();
    let cancelled = Arc::clone(cancelled);
    let finished = rt.block_on(queue_service::process_job(
        &db_path, &client, job, &cancelled,
    ))?;
    Ok(finished.status)
}

fn materialize_job(
    db_path: &std::path::Path,
    run: &RunRecord,
    request: &NewspaperWorkflowRequest,
) -> Result<(), String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
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
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::domain::state::{RunState, StepState};
    use crate::workflow::domain::types::{StepType, WorkflowType};

    #[test]
    fn cancelled_flag_fails_closed_without_network() {
        let executor = NewspaperDownloadExecutor {
            db_path: PathBuf::from("missing.sqlite3"),
            cancellation: Arc::new(AtomicBool::new(true)),
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
}
