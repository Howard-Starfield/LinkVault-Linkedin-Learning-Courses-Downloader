//! LinkedIn step executor. Course/artifact download stays provider-owned.

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use super::projection::{job_from_run, LinkedInWorkflowRequest};
use crate::artifact_downloader::CancellationFlag;
use crate::auth::{validate_li_at_with_client, ReqwestLinkedInHomeClient};
use crate::cache::{get_job, insert_job, open_runtime};
use crate::download_orchestrator::process_prepared_job_and_download_artifacts_with_quiz_assessments;
use crate::live_clients::AuthenticatedLinkedInClient;
use crate::token_store;
use crate::workflow::domain::types::{RunRecord, StepRecord};
use crate::workflow::ports::executor::{ExecutorOutcome, StepExecutor};

pub struct LinkedInDownloadExecutor {
    pub db_path: PathBuf,
    pub token_path: PathBuf,
    pub cancellation: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub session_token: Arc<Mutex<Option<String>>>,
}

struct SharedCancellation {
    cancelled: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl CancellationFlag for SharedCancellation {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

impl StepExecutor for LinkedInDownloadExecutor {
    fn workflow_type(&self) -> &'static str {
        "linkedin_download"
    }

    fn execute(&self, run: &RunRecord, _step: &StepRecord) -> ExecutorOutcome {
        let outcome = if self.cancellation.load(Ordering::SeqCst) {
            ExecutorOutcome::cancelled("LinkedIn download was cancelled".to_string())
        } else {
            match serde_json::from_str::<LinkedInWorkflowRequest>(&run.request_json) {
                Err(error) => ExecutorOutcome::failed(error.to_string()),
                Ok(request) => match session_token(&self.session_token, &self.token_path) {
                    Err(error) => ExecutorOutcome::failed(error),
                    Ok(token) => {
                        match download_linkedin_run(
                            &self.db_path,
                            run,
                            &request,
                            &token,
                            &self.cancellation,
                            &self.paused,
                        ) {
                            Ok(()) => ExecutorOutcome::succeeded("{}".to_string()),
                            Err(error) => {
                                if self.cancellation.load(Ordering::SeqCst) {
                                    ExecutorOutcome::cancelled(error)
                                } else {
                                    ExecutorOutcome::failed(error)
                                }
                            }
                        }
                    }
                },
            }
        };
        // Cancel applies to the in-flight job only. Clear so later queue items
        // are not sticky-cancelled without a Failed retry path.
        self.cancellation.store(false, Ordering::SeqCst);
        outcome
    }
}

fn session_token(
    override_token: &Mutex<Option<String>>,
    token_path: &std::path::Path,
) -> Result<String, String> {
    if let Some(token) = override_token
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Ok(token);
    }
    token_store::load_token(token_path).map_err(|error| error.to_string())
}

fn download_linkedin_run(
    db_path: &std::path::Path,
    run: &RunRecord,
    request: &LinkedInWorkflowRequest,
    token: &str,
    cancelled: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut home_client = ReqwestLinkedInHomeClient::new().map_err(|error| error.to_string())?;
    let session =
        validate_li_at_with_client(token, &mut home_client).map_err(|error| error.to_string())?;
    let connection = open_runtime(db_path).map_err(|error| error.to_string())?;
    let mut job = job_from_run(run);
    job.status = "queued".to_string();
    if get_job(&connection, &job.id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        insert_job(&connection, &job).map_err(|error| error.to_string())?;
    }
    let mut client =
        AuthenticatedLinkedInClient::new(token, &session).map_err(|error| error.to_string())?;
    let mut artifact_client = client.clone();
    let cancellation = SharedCancellation {
        cancelled: Arc::clone(cancelled),
        paused: Arc::clone(paused),
    };
    let summary = process_prepared_job_and_download_artifacts_with_quiz_assessments(
        &connection,
        &mut client,
        &mut artifact_client,
        &cancellation,
        chrono::Utc::now().timestamp(),
        Vec::new(),
        job,
    )
    .map_err(|error| error.to_string())?;
    let Some(summary) = summary else {
        return Err("LinkedIn job was not processed".to_string());
    };
    if summary.cancelled > 0 {
        return Err("LinkedIn download was cancelled".to_string());
    }
    if summary.failed > 0 {
        return Err(format!(
            "LinkedIn download finished with {} failed artifact(s)",
            summary.failed
        ));
    }
    let _ = request;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::domain::state::{RunState, StepState};
    use crate::workflow::domain::types::{StepType, WorkflowType};

    #[test]
    fn missing_token_fails_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let cancellation = Arc::new(AtomicBool::new(false));
        let executor = LinkedInDownloadExecutor {
            db_path: temp.path().join("linkvault.sqlite3"),
            token_path: temp.path().join("token.bin"),
            cancellation: Arc::clone(&cancellation),
            paused: Arc::new(AtomicBool::new(false)),
            session_token: Arc::new(Mutex::new(None)),
        };
        let run = RunRecord {
            id: "job-1".to_string(),
            workflow_type: WorkflowType::linkedin_download(),
            provider: "linkedin".to_string(),
            state: RunState::Running,
            legacy_origin: None,
            legacy_id: None,
            request_json: "{\"schemaVersion\":1,\"courseSlug\":\"foo\",\"sourceUrl\":\"https://www.linkedin.com/learning/foo\",\"selectedQuality\":\"720p\",\"downloadVideos\":true,\"downloadExercises\":true,\"downloadSubtitles\":true,\"downloadQuizzes\":true,\"quizHintsJson\":\"[]\"}".to_string(),
            output_root: ".".to_string(),
            error_message: None,
            created_at: 1,
            updated_at: 1,
            completed_at: None,
        };
        let step = StepRecord {
            id: "step".to_string(),
            run_id: run.id.clone(),
            step_key: "execute".to_string(),
            step_type: StepType::linkedin_execute(),
            state: StepState::Running,
            attempt: 1,
            error_message: None,
            created_at: 1,
            updated_at: 1,
        };
        let outcome = executor.execute(&run, &step);
        assert!(!outcome.succeeded);
        assert!(!outcome.cancelled);
    }

    #[test]
    fn execute_clears_sticky_cancellation_after_cancelled_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let cancellation = Arc::new(AtomicBool::new(true));
        let executor = LinkedInDownloadExecutor {
            db_path: temp.path().join("linkvault.sqlite3"),
            token_path: temp.path().join("token.bin"),
            cancellation: Arc::clone(&cancellation),
            paused: Arc::new(AtomicBool::new(false)),
            session_token: Arc::new(Mutex::new(None)),
        };
        let run = RunRecord {
            id: "job-1".to_string(),
            workflow_type: WorkflowType::linkedin_download(),
            provider: "linkedin".to_string(),
            state: RunState::Running,
            legacy_origin: None,
            legacy_id: None,
            request_json: "{\"schemaVersion\":1,\"courseSlug\":\"foo\",\"sourceUrl\":\"https://www.linkedin.com/learning/foo\",\"selectedQuality\":\"720p\",\"downloadVideos\":true,\"downloadExercises\":true,\"downloadSubtitles\":true,\"downloadQuizzes\":true,\"quizHintsJson\":\"[]\"}".to_string(),
            output_root: ".".to_string(),
            error_message: None,
            created_at: 1,
            updated_at: 1,
            completed_at: None,
        };
        let step = StepRecord {
            id: "step".to_string(),
            run_id: run.id.clone(),
            step_key: "execute".to_string(),
            step_type: StepType::linkedin_execute(),
            state: StepState::Running,
            attempt: 1,
            error_message: None,
            created_at: 1,
            updated_at: 1,
        };

        let outcome = executor.execute(&run, &step);

        assert!(outcome.cancelled);
        assert!(!cancellation.load(Ordering::SeqCst));
    }
}
