//! WorkflowRuntime owns the kernel supervisor. Provider executors register at setup.

use std::collections::HashSet;
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
use crate::workflow::domain::types::{
    NewWorkflowRun, NewWorkflowStep, RunRecord, StepRecord, StepType, WorkflowType,
};
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
    /// Serializes claim + apply only. Long `executor.execute` work runs outside.
    drain_lock: Mutex<()>,
    /// One in-flight execute per workflow type so providers can overlap without
    /// double-running the same provider (shared cancel flags, rate limits).
    executing_types: Mutex<HashSet<String>>,
}

struct ClaimedStep {
    run: RunRecord,
    step: StepRecord,
    executor: Option<Arc<dyn StepExecutor>>,
}

struct ExecuteGuard {
    inner: Arc<WorkflowRuntimeInner>,
    workflow_type: String,
}

impl Drop for ExecuteGuard {
    fn drop(&mut self) {
        self.inner
            .executing_types
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.workflow_type);
    }
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
                executing_types: Mutex::new(HashSet::new()),
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

    /// Persist pause for LinkedIn runs that have not started yet.
    /// Queued → Paused so the supervisor will not claim them. Resume returns
    /// Paused → Queued. In-flight Running work stays Running; the caller owns
    /// the cooperative atomic flag.
    pub fn set_linkedin_run_paused(
        &self,
        id: String,
        paused: bool,
        updated_at: i64,
    ) -> Result<(), WorkflowError> {
        let run = self
            .inner
            .service
            .get_run(id.clone())?
            .ok_or_else(|| WorkflowError::RunNotFound(id.clone()))?;
        if run.workflow_type.as_str() != WorkflowType::linkedin_download().as_str() {
            return Err(WorkflowError::RunNotFound(id));
        }
        match (paused, run.state) {
            (true, RunState::Queued) => {
                self.inner.service.transition_run(
                    id,
                    RunState::Paused,
                    None,
                    "run_paused",
                    "{}".to_string(),
                    updated_at,
                )?;
            }
            (false, RunState::Paused) => {
                self.inner.service.transition_run(
                    id,
                    RunState::Queued,
                    None,
                    "run_resumed",
                    "{}".to_string(),
                    updated_at,
                )?;
            }
            (true, RunState::Paused)
            | (false, RunState::Queued)
            | (_, RunState::Running)
            | (_, RunState::Cancelling) => {}
            _ => {}
        }
        Ok(())
    }

    pub fn set_all_queued_linkedin_runs_paused(
        &self,
        paused: bool,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        let runs = self.list_linkedin_runs(250)?;
        let mut changed = 0;
        for run in runs {
            let before = run.state;
            self.set_linkedin_run_paused(run.id.clone(), paused, updated_at)?;
            if let Some(after) = self.get_run(run.id)? {
                if after.state != before {
                    changed += 1;
                }
            }
        }
        Ok(changed)
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

    /// Cancel a run and delete it once terminal. For in-flight Running work this
    /// advances Cancelling → Cancelled immediately so Active-tab removal can
    /// clear the row without waiting for the executor drain.
    pub fn cancel_and_delete_run(
        &self,
        id: String,
        updated_at: i64,
    ) -> Result<bool, WorkflowError> {
        let _ = self.cancel_run(id.clone(), updated_at);
        if let Some(run) = self.get_run(id.clone())? {
            if run.state == RunState::Cancelling {
                let steps = self.inner.service.list_steps_for_run(id.clone())?;
                for step in steps {
                    if matches!(
                        step.state,
                        StepState::Pending
                            | StepState::Ready
                            | StepState::Running
                            | StepState::RetryWait
                    ) {
                        self.inner.service.transition_step(
                            step.id,
                            StepState::Cancelled,
                            Some("cancelled and removed by user".to_string()),
                            "step_cancelled",
                            "{}".to_string(),
                            updated_at,
                        )?;
                    }
                }
                self.inner.service.transition_run(
                    id.clone(),
                    RunState::Cancelled,
                    Some("cancelled and removed by user".to_string()),
                    "run_cancelled",
                    "{}".to_string(),
                    updated_at,
                )?;
            }
        }
        self.delete_run_if_terminal(id)
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
        drain_pipeline(self, None)
    }

    pub fn drain_type(&self, workflow_type: &str) -> Result<DrainOutcome, WorkflowError> {
        drain_pipeline(self, Some(workflow_type))
    }

    fn try_begin_execute(&self, workflow_type: &str) -> Option<ExecuteGuard> {
        let mut busy = self
            .inner
            .executing_types
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !busy.insert(workflow_type.to_string()) {
            return None;
        }
        Some(ExecuteGuard {
            inner: Arc::clone(&self.inner),
            workflow_type: workflow_type.to_string(),
        })
    }
}

impl Drop for WorkflowRuntime {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.shutdown();
        }
    }
}

fn drain_pipeline(
    runtime: &WorkflowRuntime,
    only_type: Option<&str>,
) -> Result<DrainOutcome, WorkflowError> {
    let Some((claimed, _execute_guard)) = claim_ready_step(runtime, only_type)? else {
        return Ok(DrainOutcome::idle());
    };
    let outcome = match &claimed.executor {
        Some(executor) => executor.execute(&claimed.run, &claimed.step),
        None => synthetic_outcome(&claimed.run),
    };
    let _apply_guard = runtime
        .inner
        .drain_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    finish_claimed_step(&runtime.inner.service, &claimed, outcome)
}

fn claim_ready_step(
    runtime: &WorkflowRuntime,
    only_type: Option<&str>,
) -> Result<Option<(ClaimedStep, ExecuteGuard)>, WorkflowError> {
    let now = chrono::Utc::now().timestamp();
    let registered = runtime
        .inner
        .executors
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for executor in registered {
        if only_type.is_some_and(|wanted| executor.workflow_type() != wanted) {
            continue;
        }
        let Some(execute_guard) = runtime.try_begin_execute(executor.workflow_type()) else {
            continue;
        };
        let claimed = {
            let _claim_guard = runtime
                .inner
                .drain_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match runtime
                .inner
                .service
                .claim_next_ready_step(executor.workflow_type().to_string(), now)?
            {
                Some(step) => {
                    let run = runtime
                        .inner
                        .service
                        .get_run(step.run_id.clone())?
                        .ok_or_else(|| WorkflowError::RunNotFound(step.run_id.clone()))?;
                    Some(ClaimedStep {
                        run,
                        step,
                        executor: Some(Arc::clone(&executor)),
                    })
                }
                None => None,
            }
        };
        if let Some(claimed) = claimed {
            return Ok(Some((claimed, execute_guard)));
        }
        drop(execute_guard);
    }
    if only_type.is_some_and(|wanted| wanted != WorkflowType::synthetic().as_str()) {
        return Ok(None);
    }
    let Some(execute_guard) = runtime.try_begin_execute(WorkflowType::synthetic().as_str()) else {
        return Ok(None);
    };
    let claimed = {
        let _claim_guard = runtime
            .inner
            .drain_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match runtime
            .inner
            .service
            .claim_next_ready_step(WorkflowType::synthetic().as_str().to_string(), now)?
        {
            Some(step) => {
                let run = runtime
                    .inner
                    .service
                    .get_run(step.run_id.clone())?
                    .ok_or_else(|| WorkflowError::RunNotFound(step.run_id.clone()))?;
                Some(ClaimedStep {
                    run,
                    step,
                    executor: None,
                })
            }
            None => None,
        }
    };
    Ok(claimed.map(|claimed| (claimed, execute_guard)))
}

fn finish_claimed_step(
    service: &WorkflowRepositoryService,
    claimed: &ClaimedStep,
    outcome: ExecutorOutcome,
) -> Result<DrainOutcome, WorkflowError> {
    let now = chrono::Utc::now().timestamp();
    let retried = apply_executor_outcome(
        service,
        &claimed.run.id,
        &claimed.step.id,
        claimed.step.attempt,
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
    drain_outcome_after_apply(service, &claimed.run.id, &outcome)
}

fn synthetic_outcome(run: &RunRecord) -> ExecutorOutcome {
    if request_disk_full(&run.request_json) {
        ExecutorOutcome::failed("disk is full".to_string())
    } else if request_should_retry(&run.request_json) {
        ExecutorOutcome::retryable_failure("synthetic retry requested".to_string())
    } else if request_should_fail(&run.request_json) {
        ExecutorOutcome::failed("synthetic failure requested".to_string())
    } else {
        ExecutorOutcome::succeeded("{}".to_string())
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
    fn different_workflow_types_execute_without_holding_drain_lock() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
        use std::time::Duration;

        struct HoldingExecutor {
            workflow_type: &'static str,
            hold_until_release: bool,
            entered: Arc<AtomicBool>,
            overlap_seen: Arc<AtomicUsize>,
            release: Arc<AtomicBool>,
            peer_entered: Arc<AtomicBool>,
        }

        impl StepExecutor for HoldingExecutor {
            fn workflow_type(&self) -> &'static str {
                self.workflow_type
            }

            fn execute(
                &self,
                _run: &crate::workflow::domain::types::RunRecord,
                _step: &crate::workflow::domain::types::StepRecord,
            ) -> ExecutorOutcome {
                self.entered.store(true, AtomicOrdering::SeqCst);
                if self.hold_until_release {
                    let deadline = std::time::Instant::now() + Duration::from_secs(2);
                    while std::time::Instant::now() < deadline {
                        if self.peer_entered.load(AtomicOrdering::SeqCst) {
                            self.overlap_seen.fetch_add(1, AtomicOrdering::SeqCst);
                            break;
                        }
                        if self.release.load(AtomicOrdering::SeqCst) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    while !self.release.load(AtomicOrdering::SeqCst) {
                        thread::sleep(Duration::from_millis(5));
                    }
                } else if self.peer_entered.load(AtomicOrdering::SeqCst) {
                    self.overlap_seen.fetch_add(1, AtomicOrdering::SeqCst);
                }
                ExecutorOutcome::succeeded("{}".to_string())
            }
        }

        let (_dir, runtime) = runtime();
        let slow_entered = Arc::new(AtomicBool::new(false));
        let fast_entered = Arc::new(AtomicBool::new(false));
        let overlap_seen = Arc::new(AtomicUsize::new(0));
        let release_slow = Arc::new(AtomicBool::new(false));

        runtime.register_executor(Arc::new(HoldingExecutor {
            workflow_type: "linkedin_download",
            hold_until_release: true,
            entered: Arc::clone(&slow_entered),
            overlap_seen: Arc::clone(&overlap_seen),
            release: Arc::clone(&release_slow),
            peer_entered: Arc::clone(&fast_entered),
        }));
        runtime.register_executor(Arc::new(HoldingExecutor {
            workflow_type: "newspaper_download",
            hold_until_release: false,
            entered: Arc::clone(&fast_entered),
            overlap_seen: Arc::clone(&overlap_seen),
            release: Arc::new(AtomicBool::new(true)),
            peer_entered: Arc::clone(&slow_entered),
        }));

        runtime
            .submit_linkedin_download(
                "li-overlap-1".to_string(),
                "course-1".to_string(),
                "{}".to_string(),
                ".".to_string(),
                200,
                None,
            )
            .unwrap();
        runtime
            .submit_newspaper_download(
                "np-overlap-1".to_string(),
                "NY".to_string(),
                "{\"schemaVersion\":1,\"batchId\":\"batch-1\",\"editionCode\":\"NY\",\"editionName\":\"World Journal\",\"editionPublicationDate\":\"\",\"publicationDate\":\"2026-07-24\",\"queuePosition\":1,\"delaySeconds\":0,\"scheduledAt\":null,\"optimizeImages\":false}".to_string(),
                ".".to_string(),
                201,
                None,
            )
            .unwrap();

        let runtime_slow = runtime.clone();
        let slow_handle =
            thread::spawn(move || runtime_slow.drain_type("linkedin_download").unwrap());

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !slow_entered.load(AtomicOrdering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "slow linkedin executor never entered"
            );
            thread::sleep(Duration::from_millis(5));
        }

        let newspaper_outcome = runtime.drain_type("newspaper_download").unwrap();
        assert!(
            newspaper_outcome.processed,
            "newspaper must claim while linkedin execute is in flight"
        );
        assert!(
            fast_entered.load(AtomicOrdering::SeqCst),
            "newspaper executor must run while linkedin still holds execute"
        );
        assert!(
            overlap_seen.load(AtomicOrdering::SeqCst) > 0,
            "providers must overlap execute outside drain_lock"
        );

        release_slow.store(true, AtomicOrdering::SeqCst);
        let linkedin_outcome = slow_handle.join().expect("linkedin drain thread");
        assert!(linkedin_outcome.processed);
    }
}
