//! WorkflowRuntime owns the kernel supervisor. Provider executors register at setup.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::app::database_writer::DatabaseWriter;
use crate::workflow::application::repository_service::WorkflowRepositoryService;
use crate::workflow::domain::errors::WorkflowError;
use crate::workflow::domain::state::{RunState, StepState};
use crate::workflow::domain::types::{NewWorkflowRun, NewWorkflowStep, StepType, WorkflowType};
use crate::workflow::ports::executor::{ExecutorOutcome, StepExecutor};

#[derive(Clone)]
pub struct WorkflowRuntime {
    inner: Arc<WorkflowRuntimeInner>,
}

struct WorkflowRuntimeInner {
    service: WorkflowRepositoryService,
    shutdown: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
    executors: Mutex<Vec<Arc<dyn StepExecutor>>>,
    drain_lock: Mutex<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainOutcome {
    pub processed: bool,
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
}

impl DrainOutcome {
    fn idle() -> Self {
        Self {
            processed: false,
            completed: 0,
            failed: 0,
            cancelled: 0,
        }
    }
}

impl WorkflowRuntime {
    pub fn new(writer: DatabaseWriter) -> Self {
        Self {
            inner: Arc::new(WorkflowRuntimeInner {
                service: WorkflowRepositoryService::new(writer),
                shutdown: Arc::new(AtomicBool::new(false)),
                join: Mutex::new(None),
                executors: Mutex::new(Vec::new()),
                drain_lock: Mutex::new(()),
            }),
        }
    }

    pub fn register_executor(&self, executor: Arc<dyn StepExecutor>) {
        self.inner
            .executors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(executor);
    }

    pub fn start_supervisor(&self) -> Result<(), WorkflowError> {
        let mut join = self
            .inner
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if join.is_some() {
            return Ok(());
        }
        let runtime = self.clone();
        let shutdown = Arc::clone(&self.inner.shutdown);
        let handle = thread::Builder::new()
            .name("linkvault-workflow-supervisor".to_string())
            .spawn(move || {
                while !shutdown.load(Ordering::SeqCst) {
                    let now = chrono::Utc::now().timestamp();
                    let _ = runtime.reclaim_expired_leases(30 * 60, now);
                    let _ = runtime.drain_once();
                    thread::sleep(Duration::from_millis(500));
                }
            })
            .map_err(|error| WorkflowError::Writer(error.to_string()))?;
        *join = Some(handle);
        Ok(())
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self
            .inner
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = handle.join();
        }
    }

    pub fn submit_synthetic(
        &self,
        request_json: &str,
        created_at: i64,
    ) -> Result<String, WorkflowError> {
        submit_synthetic(&self.inner.service, request_json, created_at)
    }

    pub fn submit_coursera_download(
        &self,
        run_id: String,
        class_name: String,
        request_json: String,
        output_root: String,
        created_at: i64,
    ) -> Result<String, WorkflowError> {
        let step_id = format!("{run_id}-execute");
        self.inner.service.insert_run_with_steps_and_event(
            NewWorkflowRun {
                id: run_id.clone(),
                workflow_type: WorkflowType::coursera_download(),
                provider: "coursera".to_string(),
                legacy_origin: None,
                legacy_id: None,
                request_json,
                output_root,
                created_at,
                ready_at: None,
            },
            vec![NewWorkflowStep {
                id: step_id,
                step_key: class_name,
                step_type: StepType::coursera_execute(),
                created_at,
            }],
            "submitted",
            "{}".to_string(),
        )?;
        Ok(run_id)
    }

    pub fn submit_linkedin_download(
        &self,
        run_id: String,
        course_slug: String,
        request_json: String,
        output_root: String,
        created_at: i64,
        ready_at: Option<i64>,
    ) -> Result<String, WorkflowError> {
        let step_id = format!("{run_id}-execute");
        self.inner.service.insert_run_with_steps_and_event(
            NewWorkflowRun {
                id: run_id.clone(),
                workflow_type: WorkflowType::linkedin_download(),
                provider: "linkedin".to_string(),
                legacy_origin: None,
                legacy_id: None,
                request_json,
                output_root,
                created_at,
                ready_at,
            },
            vec![NewWorkflowStep {
                id: step_id,
                step_key: course_slug,
                step_type: StepType::linkedin_execute(),
                created_at,
            }],
            "submitted",
            "{}".to_string(),
        )?;
        Ok(run_id)
    }

    pub fn list_linkedin_runs(
        &self,
        limit: i64,
    ) -> Result<Vec<crate::workflow::domain::types::RunRecord>, WorkflowError> {
        self.inner.service.list_runs_by_workflow_type(
            WorkflowType::linkedin_download().as_str().to_string(),
            limit,
        )
    }

    pub fn reconcile_linkedin_after_restart(
        &self,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        self.inner.service.fail_running_runs(
            WorkflowType::linkedin_download().as_str().to_string(),
            "Interrupted by an application restart".to_string(),
            updated_at,
        )
    }

    pub fn delete_linkedin_runs(&self) -> Result<usize, WorkflowError> {
        self.inner
            .service
            .delete_runs_by_workflow_type(WorkflowType::linkedin_download().as_str().to_string())
    }

    pub fn delete_terminal_linkedin_runs(&self) -> Result<usize, WorkflowError> {
        self.inner
            .service
            .delete_terminal_runs(WorkflowType::linkedin_download().as_str().to_string())
    }

    pub fn submit_newspaper_download(
        &self,
        run_id: String,
        step_key: String,
        request_json: String,
        output_root: String,
        created_at: i64,
        ready_at: Option<i64>,
    ) -> Result<String, WorkflowError> {
        let step_id = format!("{run_id}-execute");
        self.inner.service.insert_run_with_steps_and_event(
            NewWorkflowRun {
                id: run_id.clone(),
                workflow_type: WorkflowType::newspaper_download(),
                provider: "newspaper".to_string(),
                legacy_origin: None,
                legacy_id: None,
                request_json,
                output_root,
                created_at,
                ready_at,
            },
            vec![NewWorkflowStep {
                id: step_id,
                step_key,
                step_type: StepType::newspaper_execute(),
                created_at,
            }],
            "submitted",
            "{}".to_string(),
        )?;
        Ok(run_id)
    }

    pub fn list_newspaper_runs(
        &self,
        limit: i64,
    ) -> Result<Vec<crate::workflow::domain::types::RunRecord>, WorkflowError> {
        self.inner.service.list_runs_by_workflow_type(
            WorkflowType::newspaper_download().as_str().to_string(),
            limit,
        )
    }

    pub fn reconcile_newspaper_after_restart(
        &self,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        self.inner.service.fail_running_runs(
            WorkflowType::newspaper_download().as_str().to_string(),
            "Interrupted by an application restart".to_string(),
            updated_at,
        )
    }

    pub fn delete_newspaper_runs(&self) -> Result<usize, WorkflowError> {
        self.inner
            .service
            .delete_runs_by_workflow_type(WorkflowType::newspaper_download().as_str().to_string())
    }

    pub fn delete_terminal_newspaper_runs(&self) -> Result<usize, WorkflowError> {
        self.inner
            .service
            .delete_terminal_runs(WorkflowType::newspaper_download().as_str().to_string())
    }

    pub fn submit_youtube_download(
        &self,
        run_id: String,
        video_id: String,
        request_json: String,
        output_root: String,
        created_at: i64,
    ) -> Result<String, WorkflowError> {
        let step_id = format!("{run_id}-execute");
        self.inner.service.insert_run_with_steps_and_event(
            NewWorkflowRun {
                id: run_id.clone(),
                workflow_type: WorkflowType::youtube_download(),
                provider: "youtube".to_string(),
                legacy_origin: None,
                legacy_id: None,
                request_json,
                output_root,
                created_at,
                ready_at: None,
            },
            vec![NewWorkflowStep {
                id: step_id,
                step_key: video_id,
                step_type: StepType::youtube_execute(),
                created_at,
            }],
            "submitted",
            "{}".to_string(),
        )?;
        Ok(run_id)
    }

    pub fn list_youtube_runs(
        &self,
        limit: i64,
    ) -> Result<Vec<crate::workflow::domain::types::RunRecord>, WorkflowError> {
        self.inner.service.list_runs_by_workflow_type(
            WorkflowType::youtube_download().as_str().to_string(),
            limit,
        )
    }

    pub fn reconcile_youtube_after_restart(&self, updated_at: i64) -> Result<usize, WorkflowError> {
        // YouTube-only: fail queued/running/cancelling/(paused|retry_wait). Do not
        // widen shared fail_running_runs used by other providers.
        self.inner.service.fail_nonterminal_runs(
            WorkflowType::youtube_download().as_str().to_string(),
            "Interrupted by an application restart".to_string(),
            updated_at,
        )
    }

    pub fn list_coursera_runs(
        &self,
        limit: i64,
    ) -> Result<Vec<crate::workflow::domain::types::RunRecord>, WorkflowError> {
        self.inner.service.list_runs_by_workflow_type(
            WorkflowType::coursera_download().as_str().to_string(),
            limit,
        )
    }

    pub fn get_run(
        &self,
        id: String,
    ) -> Result<Option<crate::workflow::domain::types::RunRecord>, WorkflowError> {
        self.inner.service.get_run(id)
    }

    pub fn list_events(
        &self,
        run_id: String,
    ) -> Result<Vec<crate::workflow::domain::types::WorkflowEventRecord>, WorkflowError> {
        self.inner.service.list_events(run_id)
    }

    pub fn reconcile_coursera_after_restart(
        &self,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        self.inner.service.fail_running_runs(
            WorkflowType::coursera_download().as_str().to_string(),
            "Interrupted by an application restart".to_string(),
            updated_at,
        )
    }

    pub fn delete_coursera_runs(&self) -> Result<usize, WorkflowError> {
        self.inner
            .service
            .delete_runs_by_workflow_type(WorkflowType::coursera_download().as_str().to_string())
    }

    pub fn delete_terminal_coursera_runs(&self) -> Result<usize, WorkflowError> {
        self.inner
            .service
            .delete_terminal_runs(WorkflowType::coursera_download().as_str().to_string())
    }

    pub fn delete_run_if_terminal(&self, id: String) -> Result<bool, WorkflowError> {
        self.inner.service.delete_run_if_terminal(id)
    }

    pub fn cancel_run(&self, id: String, updated_at: i64) -> Result<(), WorkflowError> {
        let run = self
            .inner
            .service
            .get_run(id.clone())?
            .ok_or_else(|| WorkflowError::RunNotFound(id.clone()))?;
        match run.state {
            RunState::Cancelling => {
                // Already cooperative-cancel in progress; idempotent.
                return Ok(());
            }
            RunState::Running => {
                // Prefer Cancelling while the executor may still own work.
                self.inner.service.transition_run(
                    id,
                    RunState::Cancelling,
                    Some("cancel requested by user".to_string()),
                    "run_cancelling",
                    "{}".to_string(),
                    updated_at,
                )?;
                return Ok(());
            }
            RunState::Queued | RunState::Paused | RunState::RetryWait => {}
            other => {
                return Err(WorkflowError::IllegalRunTransition {
                    from: other.as_str().to_string(),
                    to: RunState::Cancelled.as_str().to_string(),
                })
            }
        }
        let steps = self.inner.service.list_steps_for_run(id.clone())?;
        for step in steps {
            if matches!(
                step.state,
                StepState::Pending | StepState::Ready | StepState::Running | StepState::RetryWait
            ) {
                self.inner.service.transition_step(
                    step.id,
                    StepState::Cancelled,
                    Some("cancelled by user".to_string()),
                    "step_cancelled",
                    "{}".to_string(),
                    updated_at,
                )?;
            }
        }
        self.inner.service.transition_run(
            id,
            RunState::Cancelled,
            Some("cancelled by user".to_string()),
            "run_cancelled",
            "{}".to_string(),
            updated_at,
        )?;
        Ok(())
    }

    pub fn with_drain_lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self
            .inner
            .drain_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f()
    }

    pub fn reclaim_expired_leases(
        &self,
        lease_ttl_secs: i64,
        now: i64,
    ) -> Result<usize, WorkflowError> {
        self.inner.service.fail_expired_running_runs(
            "Workflow lease expired".to_string(),
            now,
            now.saturating_sub(lease_ttl_secs),
        )
    }

    pub fn drain_once(&self) -> Result<DrainOutcome, WorkflowError> {
        let _guard = self
            .inner
            .drain_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        drain_once(&self.inner.service, &self.inner.executors, None)
    }

    pub fn drain_type(&self, workflow_type: &str) -> Result<DrainOutcome, WorkflowError> {
        let _guard = self
            .inner
            .drain_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        drain_once(
            &self.inner.service,
            &self.inner.executors,
            Some(workflow_type),
        )
    }
}

impl Drop for WorkflowRuntime {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.shutdown();
        }
    }
}

fn submit_synthetic(
    service: &WorkflowRepositoryService,
    request_json: &str,
    created_at: i64,
) -> Result<String, WorkflowError> {
    let run_id = format!("synthetic-run-{created_at}");
    let step_id = format!("synthetic-step-{created_at}");
    service.insert_run_with_steps_and_event(
        NewWorkflowRun {
            id: run_id.clone(),
            workflow_type: WorkflowType::synthetic(),
            provider: "workflow".to_string(),
            legacy_origin: None,
            legacy_id: None,
            request_json: request_json.to_string(),
            output_root: ".".to_string(),
            created_at,
            ready_at: None,
        },
        vec![NewWorkflowStep {
            id: step_id,
            step_key: "execute".to_string(),
            step_type: StepType::synthetic_execute(),
            created_at,
        }],
        "submitted",
        "{}".to_string(),
    )?;
    Ok(run_id)
}

fn drain_once(
    service: &WorkflowRepositoryService,
    executors: &Mutex<Vec<Arc<dyn StepExecutor>>>,
    only_type: Option<&str>,
) -> Result<DrainOutcome, WorkflowError> {
    let now = chrono::Utc::now().timestamp();
    let registered = executors
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for executor in registered {
        if only_type.is_some_and(|wanted| executor.workflow_type() != wanted) {
            continue;
        }
        if let Some(step) =
            service.claim_next_ready_step(executor.workflow_type().to_string(), now)?
        {
            let run = service
                .get_run(step.run_id.clone())?
                .ok_or_else(|| WorkflowError::RunNotFound(step.run_id.clone()))?;
            let outcome = executor.execute(&run, &step);
            let retried = apply_executor_outcome(
                service,
                &run.id,
                &step.id,
                step.attempt,
                outcome.clone(),
                now,
            )?;
            if retried {
                return Ok(DrainOutcome {
                    processed: true,
                    completed: 0,
                    failed: 0,
                    cancelled: 0,
                });
            }
            return drain_outcome_after_apply(service, &run.id, &outcome);
        }
    }
    if only_type.is_some_and(|wanted| wanted != WorkflowType::synthetic().as_str()) {
        return Ok(DrainOutcome::idle());
    }
    drain_synthetic(service, now)
}

fn drain_synthetic(
    service: &WorkflowRepositoryService,
    now: i64,
) -> Result<DrainOutcome, WorkflowError> {
    let Some(step) =
        service.claim_next_ready_step(WorkflowType::synthetic().as_str().to_string(), now)?
    else {
        return Ok(DrainOutcome::idle());
    };
    let run = service
        .get_run(step.run_id.clone())?
        .ok_or_else(|| WorkflowError::RunNotFound(step.run_id.clone()))?;
    let outcome = if request_disk_full(&run.request_json) {
        ExecutorOutcome::failed("disk is full".to_string())
    } else if request_should_retry(&run.request_json) {
        ExecutorOutcome::retryable_failure("synthetic retry requested".to_string())
    } else if request_should_fail(&run.request_json) {
        ExecutorOutcome::failed("synthetic failure requested".to_string())
    } else {
        ExecutorOutcome::succeeded("{}".to_string())
    };
    let retried = apply_executor_outcome(
        service,
        &run.id,
        &step.id,
        step.attempt,
        outcome.clone(),
        now,
    )?;
    if retried {
        return Ok(DrainOutcome {
            processed: true,
            completed: 0,
            failed: 0,
            cancelled: 0,
        });
    }
    drain_outcome_after_apply(service, &run.id, &outcome)
}

fn apply_executor_outcome(
    service: &WorkflowRepositoryService,
    run_id: &str,
    step_id: &str,
    attempt: i64,
    outcome: ExecutorOutcome,
    now: i64,
) -> Result<bool, WorkflowError> {
    const MAX_ATTEMPTS: i64 = 3;

    // Cancel-wins: a Cancelling (or already Cancelled) run must not become
    // Succeeded/Failed/RetryWait — only Cancelled is legal from Cancelling.
    if !outcome.cancelled {
        if let Some(run) = service.get_run(run_id.to_string())? {
            if matches!(run.state, RunState::Cancelling | RunState::Cancelled) {
                return apply_cancel_terminal(
                    service,
                    run_id,
                    step_id,
                    outcome
                        .error_message
                        .unwrap_or_else(|| "cancelled by user".to_string()),
                    outcome.payload_json,
                    now,
                    run.state,
                );
            }
        }
    }

    if outcome.cancelled {
        return apply_cancel_terminal(
            service,
            run_id,
            step_id,
            outcome
                .error_message
                .unwrap_or_else(|| "cancelled by user".to_string()),
            outcome.payload_json,
            now,
            service
                .get_run(run_id.to_string())?
                .map(|run| run.state)
                .unwrap_or(RunState::Cancelling),
        );
    }
    if outcome.succeeded {
        let run_state = if outcome.warning {
            RunState::SucceededWithWarnings
        } else {
            RunState::Succeeded
        };
        service.transition_step(
            step_id.to_string(),
            StepState::Succeeded,
            None,
            "step_succeeded",
            outcome.payload_json.clone(),
            now,
        )?;
        service.transition_run(
            run_id.to_string(),
            run_state,
            None,
            "run_succeeded",
            outcome.payload_json,
            now,
        )?;
        return Ok(false);
    }
    if outcome.retryable && attempt < MAX_ATTEMPTS {
        service.transition_step(
            step_id.to_string(),
            StepState::RetryWait,
            outcome.error_message.clone(),
            "step_retry_wait",
            outcome.payload_json.clone(),
            now,
        )?;
        service.transition_run(
            run_id.to_string(),
            RunState::RetryWait,
            outcome.error_message,
            "run_retry_wait",
            outcome.payload_json,
            now,
        )?;
        return Ok(true);
    }
    service.transition_step(
        step_id.to_string(),
        StepState::Failed,
        outcome.error_message.clone(),
        "step_failed",
        outcome.payload_json.clone(),
        now,
    )?;
    service.transition_run(
        run_id.to_string(),
        RunState::Failed,
        outcome.error_message,
        "run_failed",
        outcome.payload_json,
        now,
    )?;
    Ok(false)
}

fn apply_cancel_terminal(
    service: &WorkflowRepositoryService,
    run_id: &str,
    step_id: &str,
    error_message: String,
    payload_json: String,
    now: i64,
    run_state: RunState,
) -> Result<bool, WorkflowError> {
    if let Some(step) = service
        .list_steps_for_run(run_id.to_string())?
        .into_iter()
        .find(|step| step.id == step_id)
    {
        if matches!(
            step.state,
            StepState::Pending | StepState::Ready | StepState::Running | StepState::RetryWait
        ) {
            service.transition_step(
                step_id.to_string(),
                StepState::Cancelled,
                Some(error_message.clone()),
                "step_cancelled",
                payload_json.clone(),
                now,
            )?;
        }
    }
    match run_state {
        RunState::Cancelled => Ok(false),
        RunState::Cancelling
        | RunState::Running
        | RunState::Queued
        | RunState::Paused
        | RunState::RetryWait => {
            service.transition_run(
                run_id.to_string(),
                RunState::Cancelled,
                Some(error_message),
                "run_cancelled",
                payload_json,
                now,
            )?;
            Ok(false)
        }
        other => Err(WorkflowError::IllegalRunTransition {
            from: other.as_str().to_string(),
            to: RunState::Cancelled.as_str().to_string(),
        }),
    }
}

fn drain_outcome_after_apply(
    service: &WorkflowRepositoryService,
    run_id: &str,
    outcome: &ExecutorOutcome,
) -> Result<DrainOutcome, WorkflowError> {
    // Prefer durable state so cancel-wins over a succeeded executor payload is counted
    // as cancelled, not completed.
    if let Some(run) = service.get_run(run_id.to_string())? {
        return Ok(match run.state {
            RunState::Cancelled => DrainOutcome {
                processed: true,
                completed: 0,
                failed: 0,
                cancelled: 1,
            },
            RunState::Failed => DrainOutcome {
                processed: true,
                completed: 0,
                failed: 1,
                cancelled: 0,
            },
            RunState::Succeeded | RunState::SucceededWithWarnings => DrainOutcome {
                processed: true,
                completed: 1,
                failed: 0,
                cancelled: 0,
            },
            _ => drain_from_outcome(outcome),
        });
    }
    Ok(drain_from_outcome(outcome))
}

fn drain_from_outcome(outcome: &ExecutorOutcome) -> DrainOutcome {
    DrainOutcome {
        processed: true,
        completed: u32::from(outcome.succeeded),
        failed: u32::from(!outcome.succeeded && !outcome.cancelled),
        cancelled: u32::from(outcome.cancelled),
    }
}

fn request_should_fail(request_json: &str) -> bool {
    request_flag(request_json, "fail")
}

fn request_should_retry(request_json: &str) -> bool {
    request_flag(request_json, "retry")
}

fn request_disk_full(request_json: &str) -> bool {
    request_flag(request_json, "diskFull")
}

fn request_flag(request_json: &str, key: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(request_json)
        .ok()
        .and_then(|value| value.get(key).and_then(|flag| flag.as_bool()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::initialize_database;
    use crate::app::database_diagnostics::DatabaseDiagnostics;
    use crate::workflow::domain::state::RunState;
    use tempfile::tempdir;

    fn runtime() -> (tempfile::TempDir, WorkflowRuntime) {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let writer = DatabaseWriter::start(db_path, DatabaseDiagnostics::default()).unwrap();
        (directory, WorkflowRuntime::new(writer))
    }

    #[test]
    fn synthetic_workflow_succeeds_and_records_events() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{}", 42).unwrap();
        assert!(runtime.drain_once().unwrap().processed);
        assert!(!runtime.drain_once().unwrap().processed);
        let run = runtime
            .inner
            .service
            .get_run(run_id.clone())
            .unwrap()
            .unwrap();
        assert_eq!(run.state, RunState::Succeeded);
        let steps = runtime
            .inner
            .service
            .list_steps_for_run(run_id.clone())
            .unwrap();
        assert_eq!(
            steps[0].state,
            crate::workflow::domain::state::StepState::Succeeded
        );
        let events = runtime.inner.service.list_events(run_id).unwrap();
        assert!(events.iter().any(|event| event.event_type == "submitted"));
        assert!(events
            .iter()
            .any(|event| event.event_type == "step_claimed"));
        assert!(events
            .iter()
            .any(|event| event.event_type == "run_succeeded"));
    }

    #[test]
    fn synthetic_workflow_can_fail_on_request() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{\"fail\":true}", 43).unwrap();
        assert!(runtime.drain_once().unwrap().processed);
        let run = runtime.inner.service.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Failed);
    }

    #[test]
    fn illegal_run_transition_is_rejected() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{}", 44).unwrap();
        let error = runtime
            .inner
            .service
            .transition_run(
                run_id,
                RunState::Succeeded,
                None,
                "illegal",
                "{}".to_string(),
                99,
            )
            .unwrap_err();
        assert!(error.to_string().contains("illegal run transition"));
    }

    #[test]
    fn coursera_submit_does_not_claim_as_synthetic() {
        let (_dir, runtime) = runtime();
        let run_id = runtime
            .submit_coursera_download(
                "coursera-ml-1".to_string(),
                "ml-005".to_string(),
                "{\"className\":\"ml-005\"}".to_string(),
                ".".to_string(),
                50,
            )
            .unwrap();
        assert!(!runtime.drain_once().unwrap().processed);
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Queued);
        assert_eq!(run.workflow_type.as_str(), "coursera_download");
    }

    #[test]
    fn newspaper_submit_does_not_claim_as_synthetic() {
        let (_dir, runtime) = runtime();
        let run_id = runtime
            .submit_newspaper_download(
                "newspaper-job-1".to_string(),
                "NY".to_string(),
                "{\"schemaVersion\":1,\"batchId\":\"batch-1\",\"editionCode\":\"NY\",\"editionName\":\"World Journal\",\"editionPublicationDate\":\"\",\"publicationDate\":\"2026-07-24\",\"queuePosition\":1,\"delaySeconds\":0,\"scheduledAt\":null,\"optimizeImages\":false}".to_string(),
                ".".to_string(),
                55,
                None,
            )
            .unwrap();
        assert!(!runtime.drain_once().unwrap().processed);
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Queued);
        assert_eq!(run.workflow_type.as_str(), "newspaper_download");
    }

    #[test]
    fn coursera_restart_reconcile_fails_running_runs() {
        let (_dir, runtime) = runtime();
        let run_id = runtime
            .submit_coursera_download(
                "coursera-ml-2".to_string(),
                "ml-005".to_string(),
                "{}".to_string(),
                ".".to_string(),
                51,
            )
            .unwrap();
        runtime
            .inner
            .service
            .claim_next_ready_step("coursera_download".to_string(), 52)
            .unwrap();
        assert_eq!(runtime.reconcile_coursera_after_restart(53).unwrap(), 1);
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Failed);
    }

    #[test]
    fn delete_coursera_runs_removes_queued_work() {
        let (_dir, runtime) = runtime();
        runtime
            .submit_coursera_download(
                "coursera-ml-3".to_string(),
                "ml-005".to_string(),
                "{}".to_string(),
                ".".to_string(),
                54,
            )
            .unwrap();
        assert_eq!(runtime.delete_coursera_runs().unwrap(), 1);
        assert!(runtime.list_coursera_runs(10).unwrap().is_empty());
    }

    #[test]
    fn two_claim_calls_do_not_duplicate_the_same_ready_step() {
        let (_dir, runtime) = runtime();
        runtime
            .submit_coursera_download(
                "coursera-ml-dup".to_string(),
                "ml-005".to_string(),
                "{}".to_string(),
                ".".to_string(),
                60,
            )
            .unwrap();
        let first = runtime
            .inner
            .service
            .claim_next_ready_step("coursera_download".to_string(), 61)
            .unwrap();
        let second = runtime
            .inner
            .service
            .claim_next_ready_step("coursera_download".to_string(), 62)
            .unwrap();
        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[test]
    fn cancel_run_marks_queued_work_cancelled() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{}", 70).unwrap();
        runtime.cancel_run(run_id.clone(), 71).unwrap();
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Cancelled);
    }

    #[test]
    fn disk_full_synthetic_request_fails_immediately() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{\"diskFull\":true}", 72).unwrap();
        let outcome = runtime.drain_once().unwrap();
        assert!(outcome.processed);
        assert_eq!(outcome.failed, 1);
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Failed);
        assert!(run
            .error_message
            .unwrap_or_default()
            .contains("disk is full"));
    }

    #[test]
    fn retryable_synthetic_failure_retries_then_fails() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{\"retry\":true}", 73).unwrap();
        assert!(runtime.drain_once().unwrap().processed);
        let waiting = runtime.get_run(run_id.clone()).unwrap().unwrap();
        assert_eq!(waiting.state, RunState::RetryWait);
        assert!(runtime.drain_once().unwrap().processed);
        assert!(runtime.drain_once().unwrap().processed);
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Failed);
    }

    #[test]
    fn expired_lease_fails_running_work() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{}", 80).unwrap();
        runtime
            .inner
            .service
            .claim_next_ready_step("synthetic".to_string(), 10)
            .unwrap();
        assert_eq!(runtime.reclaim_expired_leases(5, 20).unwrap(), 1);
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Failed);
        assert!(run
            .error_message
            .unwrap_or_default()
            .contains("lease expired"));
    }

    #[test]
    fn youtube_restart_reconcile_fails_queued_and_leaves_other_providers() {
        let (_dir, runtime) = runtime();
        let youtube_id = runtime
            .submit_youtube_download(
                "yt-queued-1".to_string(),
                "vid-1".to_string(),
                "{}".to_string(),
                ".".to_string(),
                100,
            )
            .unwrap();
        let coursera_id = runtime
            .submit_coursera_download(
                "coursera-queued-1".to_string(),
                "ml-005".to_string(),
                "{}".to_string(),
                ".".to_string(),
                101,
            )
            .unwrap();
        assert_eq!(runtime.reconcile_youtube_after_restart(102).unwrap(), 1);
        let youtube = runtime.get_run(youtube_id.clone()).unwrap().unwrap();
        assert_eq!(youtube.state, RunState::Failed);
        assert!(youtube
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("restart")));
        let claimed = runtime
            .inner
            .service
            .claim_next_ready_step("youtube_download".to_string(), 103)
            .unwrap();
        assert!(
            claimed.is_none(),
            "failed youtube queued run must not be claimable"
        );
        let coursera = runtime.get_run(coursera_id).unwrap().unwrap();
        assert_eq!(coursera.state, RunState::Queued);
    }

    #[test]
    fn cancel_running_youtube_marks_cancelling_not_terminal() {
        let (_dir, runtime) = runtime();
        let run_id = runtime
            .submit_youtube_download(
                "yt-cancel-1".to_string(),
                "vid-1".to_string(),
                "{}".to_string(),
                ".".to_string(),
                110,
            )
            .unwrap();
        runtime
            .inner
            .service
            .claim_next_ready_step("youtube_download".to_string(), 111)
            .unwrap();
        runtime.cancel_run(run_id.clone(), 112).unwrap();
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Cancelling);
        // Idempotent second cancel while Cancelling.
        runtime.cancel_run(run.id.clone(), 113).unwrap();
        assert_eq!(
            runtime.get_run(run.id).unwrap().unwrap().state,
            RunState::Cancelling
        );
    }

    #[test]
    fn cancel_wins_when_executor_reports_succeeded() {
        struct CancelThenSucceedExecutor {
            runtime: WorkflowRuntime,
        }

        impl crate::workflow::ports::executor::StepExecutor for CancelThenSucceedExecutor {
            fn workflow_type(&self) -> &'static str {
                "youtube_download"
            }

            fn execute(
                &self,
                run: &crate::workflow::domain::types::RunRecord,
                _step: &crate::workflow::domain::types::StepRecord,
            ) -> ExecutorOutcome {
                self.runtime
                    .cancel_run(run.id.clone(), chrono::Utc::now().timestamp())
                    .expect("cancel during execute");
                ExecutorOutcome::succeeded("{}".to_string())
            }
        }

        let (_dir, runtime) = runtime();
        runtime.register_executor(Arc::new(CancelThenSucceedExecutor {
            runtime: runtime.clone(),
        }));
        let run_id = runtime
            .submit_youtube_download(
                "yt-cancel-win-1".to_string(),
                "vid-1".to_string(),
                "{}".to_string(),
                ".".to_string(),
                120,
            )
            .unwrap();
        let outcome = runtime.drain_type("youtube_download").unwrap();
        assert!(outcome.processed);
        assert_eq!(outcome.cancelled, 1);
        assert_eq!(outcome.completed, 0);
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, RunState::Cancelled);
    }

    #[test]
    fn cancel_run_propagates_illegal_transition_errors() {
        let (_dir, runtime) = runtime();
        let run_id = runtime.submit_synthetic("{}", 130).unwrap();
        assert!(runtime.drain_once().unwrap().processed);
        let error = runtime.cancel_run(run_id, 131).unwrap_err();
        assert!(error.to_string().contains("illegal run transition"));
    }
}
