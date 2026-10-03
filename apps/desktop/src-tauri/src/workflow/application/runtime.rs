//! WorkflowRuntime owns the kernel supervisor. Provider executors register at setup.

use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
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

/// Provider reconciliation registered at composition. Return the next UTC
/// deadline; callbacks must offload long work through the owned task API.
pub trait SupervisorHook: Send + Sync {
    fn reconcile(&self, runtime: &WorkflowRuntime, now: i64) -> Result<Option<i64>, WorkflowError>;

    /// Called after supervisor dispatch stops, before owned workers are joined.
    fn shutdown(&self) {}
}

const SUPERVISOR_SAFETY_INTERVAL: Duration = Duration::from_secs(60);
const SUPERVISOR_ERROR_RETRY_SECS: i64 = 5;

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
    executing_run_ids: Mutex<HashSet<String>>,
    hooks: Mutex<Vec<Arc<dyn SupervisorHook>>>,
    wake_generation: Mutex<u64>,
    wake_condition: Condvar,
    /// Lifecycle lock registers every admitted worker before shutdown joins it.
    workers: Mutex<Vec<JoinHandle<()>>>,
    task_keys: Mutex<HashSet<&'static str>>,
}

struct ClaimedStep {
    run: RunRecord,
    step: StepRecord,
    executor: Option<Arc<dyn StepExecutor>>,
}

struct ExecuteGuard {
    inner: Arc<WorkflowRuntimeInner>,
    workflow_type: String,
    run_id: Option<String>,
}

impl Drop for ExecuteGuard {
    fn drop(&mut self) {
        self.inner
            .executing_types
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.workflow_type);
        if let Some(run_id) = &self.run_id {
            self.inner
                .executing_run_ids
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(run_id);
            notify_wake(&self.inner);
        }
    }
}

struct SupervisorTaskGuard {
    inner: Arc<WorkflowRuntimeInner>,
    key: &'static str,
}

impl Drop for SupervisorTaskGuard {
    fn drop(&mut self) {
        self.inner
            .task_keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(self.key);
        notify_wake(&self.inner);
    }
}

fn notify_wake(inner: &WorkflowRuntimeInner) {
    let mut generation = inner
        .wake_generation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *generation = generation.wrapping_add(1);
    inner.wake_condition.notify_all();
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
                executing_run_ids: Mutex::new(HashSet::new()),
                hooks: Mutex::new(Vec::new()),
                wake_generation: Mutex::new(0),
                wake_condition: Condvar::new(),
                workers: Mutex::new(Vec::new()),
                task_keys: Mutex::new(HashSet::new()),
            }),
        }
    }

    pub fn register_executor(&self, executor: Arc<dyn StepExecutor>) {
        self.inner
            .executors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(executor);
        self.wake();
    }

    pub fn register_supervisor_hook(&self, hook: Arc<dyn SupervisorHook>) {
        self.inner
            .hooks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(hook);
        self.wake();
    }

    /// Durable mutation callers wake after commit. A generation counter keeps a
    /// wake during reconciliation from being lost when the supervisor waits.
    pub fn wake(&self) {
        notify_wake(&self.inner);
    }

    pub fn is_shutting_down(&self) -> bool {
        self.inner.shutdown.load(Ordering::SeqCst)
    }

    pub fn is_workflow_type_executing(&self, workflow_type: &str) -> bool {
        let tasks = self
            .inner
            .task_keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let executing = self
            .inner
            .executing_types
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        tasks.contains(workflow_type) || executing.contains(workflow_type)
    }

    fn after_mutation<T>(&self, result: Result<T, WorkflowError>) -> Result<T, WorkflowError> {
        if result.is_ok() {
            self.wake();
        }
        result
    }

    pub fn set_run_paused(
        &self,
        id: String,
        paused: bool,
        updated_at: i64,
    ) -> Result<(), WorkflowError> {
        let _claim_guard = self
            .inner
            .drain_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.after_mutation(self.inner.service.set_run_paused(id, paused, updated_at))
    }

    pub fn spawn_supervisor_task(
        &self,
        key: &'static str,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<bool, WorkflowError> {
        let mut workers = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.inner.shutdown.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let mut keys = self
            .inner
            .task_keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !keys.insert(key) {
            return Ok(false);
        }
        drop(keys);
        let guard = SupervisorTaskGuard {
            inner: Arc::clone(&self.inner),
            key,
        };
        match thread::Builder::new()
            .name(format!("linkvault-workflow-{key}"))
            .spawn(move || {
                let _guard = guard;
                work();
            }) {
            Ok(worker) => {
                workers.push(worker);
                Ok(true)
            }
            Err(error) => {
                // Spawn drops its closure and the guard releases admission.
                Err(WorkflowError::Writer(error.to_string()))
            }
        }
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
                    let generation = *runtime
                        .inner
                        .wake_generation
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let now = chrono::Utc::now().timestamp();
                    let _ = runtime.reclaim_expired_leases(30 * 60, now);
                    let mut next_deadline =
                        now.saturating_add(SUPERVISOR_SAFETY_INTERVAL.as_secs() as i64);
                    let hooks = runtime
                        .inner
                        .hooks
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    for hook in hooks {
                        match hook.reconcile(&runtime, now) {
                            Ok(Some(deadline)) => next_deadline = next_deadline.min(deadline),
                            Ok(None) => {}
                            Err(_) => {
                                next_deadline = next_deadline
                                    .min(now.saturating_add(SUPERVISOR_ERROR_RETRY_SECS))
                            }
                        }
                    }
                    if runtime.dispatch_ready_steps().is_err() {
                        next_deadline =
                            next_deadline.min(now.saturating_add(SUPERVISOR_ERROR_RETRY_SECS));
                    }
                    match runtime.next_ready_deadline(now) {
                        Ok(Some(deadline)) => next_deadline = next_deadline.min(deadline),
                        Ok(None) => {}
                        Err(_) => {
                            next_deadline =
                                next_deadline.min(now.saturating_add(SUPERVISOR_ERROR_RETRY_SECS))
                        }
                    }
                    runtime.reap_finished_workers();
                    // Re-read wall time after callbacks; an overdue deadline
                    // must not acquire a new full delay after a slow callback.
                    let remaining = next_deadline
                        .saturating_sub(chrono::Utc::now().timestamp())
                        .max(1) as u64;
                    runtime.wait_for_wake(
                        generation,
                        Duration::from_secs(remaining).min(SUPERVISOR_SAFETY_INTERVAL),
                    );
                }
            })
            .map_err(|error| WorkflowError::Writer(error.to_string()))?;
        *join = Some(handle);
        Ok(())
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.wake();
        if let Some(handle) = self
            .inner
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = handle.join();
        }
        let hooks = self
            .inner
            .hooks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        for hook in hooks {
            hook.shutdown();
        }
        let workers = std::mem::take(
            &mut *self
                .inner
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for worker in workers {
            let _ = worker.join();
        }
    }

    fn wait_for_wake(&self, generation: u64, timeout: Duration) {
        let current = self
            .inner
            .wake_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _result = self
            .inner
            .wake_condition
            .wait_timeout_while(current, timeout, |current| {
                *current == generation && !self.inner.shutdown.load(Ordering::SeqCst)
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }

    fn reap_finished_workers(&self) {
        let finished = {
            let mut workers = self
                .inner
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut finished = Vec::new();
            let mut index = 0;
            while index < workers.len() {
                if workers[index].is_finished() {
                    finished.push(workers.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            finished
        };
        for worker in finished {
            let _ = worker.join();
        }
    }

    fn next_ready_deadline(&self, now: i64) -> Result<Option<i64>, WorkflowError> {
        let registered = self
            .inner
            .executors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tasks = self
            .inner
            .task_keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let busy = self
            .inner
            .executing_types
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let types = registered
            .iter()
            .map(|executor| executor.workflow_type())
            .chain(std::iter::once("synthetic"))
            .filter(|workflow_type| {
                !busy.contains(*workflow_type) && !tasks.contains(*workflow_type)
            })
            .map(str::to_string)
            .collect();
        drop(busy);
        drop(tasks);
        drop(registered);
        self.inner.service.next_ready_deadline(types, now)
    }

    fn dispatch_ready_steps(&self) -> Result<(), WorkflowError> {
        while !self.inner.shutdown.load(Ordering::SeqCst) {
            let Some((claimed, execute_guard)) = claim_ready_step(self, None)? else {
                break;
            };
            if !self.dispatch_claimed_step(claimed, execute_guard)? {
                break;
            }
        }
        Ok(())
    }

    fn dispatch_claimed_step(
        &self,
        claimed: ClaimedStep,
        execute_guard: ExecuteGuard,
    ) -> Result<bool, WorkflowError> {
        let key = claimed
            .executor
            .as_ref()
            .map_or("synthetic", |executor| executor.workflow_type());
        let runtime = self.clone();
        // Retain a record for a recoverable thread-launch failure.
        let failed_claim = ClaimedStep {
            run: claimed.run.clone(),
            step: claimed.step.clone(),
            executor: claimed.executor.clone(),
        };
        match self.spawn_supervisor_task(key, move || {
            let _execute_guard = execute_guard;
            let outcome = match &claimed.executor {
                Some(executor) => executor.execute(&claimed.run, &claimed.step),
                None => synthetic_outcome(&claimed.run),
            };
            let _apply_guard = runtime
                .inner
                .drain_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = finish_claimed_step(&runtime.inner.service, &claimed, outcome);
        }) {
            Ok(true) => Ok(true),
            Ok(false) => {
                let _apply_guard = self
                    .inner
                    .drain_lock
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                self.after_mutation(self.inner.service.release_unstarted_claim(
                    failed_claim.run.id,
                    failed_claim.step.id,
                    failed_claim.step.attempt,
                    chrono::Utc::now().timestamp(),
                ))?;
                Ok(false)
            }
            Err(error) => {
                let _apply_guard = self
                    .inner
                    .drain_lock
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                finish_claimed_step(
                    &self.inner.service,
                    &failed_claim,
                    ExecutorOutcome::failed("Workflow worker could not start".to_string()),
                )?;
                Err(error)
            }
        }
    }

    pub fn submit_synthetic(
        &self,
        request_json: &str,
        created_at: i64,
    ) -> Result<String, WorkflowError> {
        self.after_mutation(submit_synthetic(
            &self.inner.service,
            request_json,
            created_at,
        ))
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
        self.wake();
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
        self.wake();
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
        self.after_mutation(self.inner.service.fail_running_runs(
            WorkflowType::linkedin_download().as_str().to_string(),
            "Interrupted by an application restart".to_string(),
            updated_at,
        ))
    }

    pub fn delete_linkedin_runs(&self) -> Result<usize, WorkflowError> {
        self.after_mutation(
            self.inner.service.delete_runs_by_workflow_type(
                WorkflowType::linkedin_download().as_str().to_string(),
            ),
        )
    }

    pub fn delete_terminal_linkedin_runs(&self) -> Result<usize, WorkflowError> {
        self.after_mutation(
            self.inner
                .service
                .delete_terminal_runs(WorkflowType::linkedin_download().as_str().to_string()),
        )
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
        self.wake();
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
        self.after_mutation(self.inner.service.fail_running_runs(
            WorkflowType::newspaper_download().as_str().to_string(),
            "Interrupted by an application restart".to_string(),
            updated_at,
        ))
    }

    pub fn delete_newspaper_runs(&self) -> Result<usize, WorkflowError> {
        self.after_mutation(
            self.inner.service.delete_runs_by_workflow_type(
                WorkflowType::newspaper_download().as_str().to_string(),
            ),
        )
    }

    pub fn delete_terminal_newspaper_runs(&self) -> Result<usize, WorkflowError> {
        self.after_mutation(
            self.inner
                .service
                .delete_terminal_runs(WorkflowType::newspaper_download().as_str().to_string()),
        )
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
        self.wake();
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
        self.after_mutation(
            // YouTube-only: fail queued/running/cancelling/(paused|retry_wait). Do not
            // widen shared fail_running_runs used by other providers.
            self.inner.service.fail_nonterminal_runs(
                WorkflowType::youtube_download().as_str().to_string(),
                "Interrupted by an application restart".to_string(),
                updated_at,
            ),
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
        self.set_run_paused(id, paused, updated_at)
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
        self.after_mutation(self.inner.service.fail_running_runs(
            WorkflowType::coursera_download().as_str().to_string(),
            "Interrupted by an application restart".to_string(),
            updated_at,
        ))
    }

    pub fn delete_coursera_runs(&self) -> Result<usize, WorkflowError> {
        self.after_mutation(
            self.inner.service.delete_runs_by_workflow_type(
                WorkflowType::coursera_download().as_str().to_string(),
            ),
        )
    }

    pub fn delete_terminal_coursera_runs(&self) -> Result<usize, WorkflowError> {
        self.after_mutation(
            self.inner
                .service
                .delete_terminal_runs(WorkflowType::coursera_download().as_str().to_string()),
        )
    }

    pub fn delete_run_if_terminal(&self, id: String) -> Result<bool, WorkflowError> {
        self.after_mutation(self.inner.service.delete_run_if_terminal(id))
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
                self.wake();
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
        self.wake();
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
        let _claim_guard = self
            .inner
            .drain_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active: Vec<String> = self
            .inner
            .executing_run_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect();
        if active.is_empty() {
            return self.inner.service.fail_expired_running_runs(
                "Workflow lease expired".to_string(),
                now,
                now.saturating_sub(lease_ttl_secs),
            );
        }
        self.inner.service.fail_expired_running_runs_excluding(
            "Workflow lease expired".to_string(),
            now,
            now.saturating_sub(lease_ttl_secs),
            active,
        )
    }

    pub fn drain_once(&self) -> Result<DrainOutcome, WorkflowError> {
        drain_pipeline(self, None)
    }

    pub fn drain_type(&self, workflow_type: &str) -> Result<DrainOutcome, WorkflowError> {
        drain_pipeline(self, Some(workflow_type))
    }

    fn try_begin_execute(&self, workflow_type: &str) -> Option<ExecuteGuard> {
        // A worker releases its execute guard before its task guard. Avoid
        // claiming the next run during that small admission handoff window.
        let tasks = self
            .inner
            .task_keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if tasks.contains(workflow_type) {
            return None;
        }
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
            run_id: None,
        })
    }

    fn record_active_run(&self, guard: &mut ExecuteGuard, run_id: &str) {
        self.inner
            .executing_run_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(run_id.to_string());
        guard.run_id = Some(run_id.to_string());
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
        let Some(mut execute_guard) = runtime.try_begin_execute(executor.workflow_type()) else {
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
                    runtime.record_active_run(&mut execute_guard, &run.id);
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
    let Some(mut execute_guard) = runtime.try_begin_execute(WorkflowType::synthetic().as_str())
    else {
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
                runtime.record_active_run(&mut execute_guard, &run.id);
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

    struct DeadlineHook {
        calls: std::sync::mpsc::Sender<i64>,
        deadline: std::sync::atomic::AtomicI64,
    }

    impl SupervisorHook for DeadlineHook {
        fn reconcile(
            &self,
            _runtime: &WorkflowRuntime,
            now: i64,
        ) -> Result<Option<i64>, WorkflowError> {
            let _ = self.calls.send(now);
            let deadline = self.deadline.load(Ordering::SeqCst);
            Ok((deadline > 0).then_some(deadline))
        }
    }

    #[test]
    fn supervisor_sleeps_when_idle_and_wakes_before_a_future_deadline() {
        let (_dir, runtime) = runtime();
        let (sent, received) = std::sync::mpsc::channel();
        runtime.register_supervisor_hook(Arc::new(DeadlineHook {
            calls: sent,
            deadline: std::sync::atomic::AtomicI64::new(chrono::Utc::now().timestamp() + 3600),
        }));
        runtime.start_supervisor().unwrap();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        // The old 500ms loop would reconcile twice during this quiet interval.
        assert!(received.recv_timeout(Duration::from_millis(1100)).is_err());
        runtime.wake();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = std::time::Instant::now();
        runtime.shutdown();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "Shutdown must interrupt the deadline wait"
        );
        assert!(!runtime
            .spawn_supervisor_task("after_shutdown", || {})
            .unwrap());
    }

    #[test]
    fn supervisor_rechecks_a_deadline_that_is_already_missed() {
        let (_dir, runtime) = runtime();
        let (sent, received) = std::sync::mpsc::channel();
        let missed = chrono::Utc::now().timestamp() - 60;
        runtime.register_supervisor_hook(Arc::new(DeadlineHook {
            calls: sent,
            deadline: std::sync::atomic::AtomicI64::new(missed),
        }));
        runtime.start_supervisor().unwrap();
        assert!(received.recv_timeout(Duration::from_secs(2)).unwrap() >= missed);
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime.shutdown();
    }

    #[test]
    fn wake_during_reconciliation_is_not_lost() {
        struct BlockingHook {
            calls: std::sync::mpsc::Sender<()>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
            first: AtomicBool,
        }
        impl SupervisorHook for BlockingHook {
            fn reconcile(
                &self,
                _runtime: &WorkflowRuntime,
                now: i64,
            ) -> Result<Option<i64>, WorkflowError> {
                let _ = self.calls.send(());
                if self.first.swap(false, Ordering::SeqCst) {
                    self.release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap();
                }
                Ok(Some(now + 3600))
            }
        }
        let (_dir, runtime) = runtime();
        let (calls, received) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        runtime.register_supervisor_hook(Arc::new(BlockingHook {
            calls,
            release: Mutex::new(blocked),
            first: AtomicBool::new(true),
        }));
        runtime.start_supervisor().unwrap();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime.wake();
        release.send(()).unwrap();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime.shutdown();
    }

    #[test]
    fn long_executor_does_not_block_hooks_or_expire_its_local_run() {
        struct BlockingExecutor {
            started: std::sync::mpsc::Sender<()>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl StepExecutor for BlockingExecutor {
            fn workflow_type(&self) -> &'static str {
                "newspaper_download"
            }
            fn execute(&self, _run: &RunRecord, _step: &StepRecord) -> ExecutorOutcome {
                let _ = self.started.send(());
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                ExecutorOutcome::succeeded("{}".to_string())
            }
        }
        let (_dir, runtime) = runtime();
        let now = chrono::Utc::now().timestamp();
        let orphan = runtime.submit_synthetic("{}", now - 100).unwrap();
        runtime
            .inner
            .service
            .claim_next_ready_step("synthetic".to_string(), now - 100)
            .unwrap()
            .unwrap();
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        runtime.register_executor(Arc::new(BlockingExecutor {
            started,
            release: Mutex::new(release_rx),
        }));
        let (calls, calls_rx) = std::sync::mpsc::channel();
        runtime.register_supervisor_hook(Arc::new(DeadlineHook {
            calls,
            deadline: std::sync::atomic::AtomicI64::new(now + 3600),
        }));
        let run_id = runtime
            .submit_newspaper_download(
                "long-run".to_string(),
                "edition".to_string(),
                "{}".to_string(),
                String::new(),
                now,
                None,
            )
            .unwrap();
        runtime.start_supervisor().unwrap();
        calls_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(runtime.is_workflow_type_executing("newspaper_download"));
        runtime.wake();
        calls_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(runtime.reclaim_expired_leases(0, now + 2).unwrap(), 1);
        assert_eq!(
            runtime.get_run(orphan).unwrap().unwrap().state,
            RunState::Failed
        );
        assert_eq!(
            runtime.get_run(run_id.clone()).unwrap().unwrap().state,
            RunState::Running
        );
        release.send(()).unwrap();
        runtime.shutdown();
        assert!(!runtime.is_workflow_type_executing("newspaper_download"));
        assert_eq!(
            runtime.get_run(run_id).unwrap().unwrap().state,
            RunState::Succeeded
        );
    }

    #[test]
    fn owned_tasks_are_single_flight_and_shutdown_joins_them_after_hook_cancellation() {
        struct ShutdownHook {
            release: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        }
        impl SupervisorHook for ShutdownHook {
            fn reconcile(
                &self,
                _runtime: &WorkflowRuntime,
                _now: i64,
            ) -> Result<Option<i64>, WorkflowError> {
                Ok(None)
            }
            fn shutdown(&self) {
                if let Some(release) = self.release.lock().unwrap().take() {
                    let _ = release.send(());
                }
            }
        }
        let (_dir, runtime) = runtime();
        let (release, waiting) = std::sync::mpsc::channel();
        runtime.register_supervisor_hook(Arc::new(ShutdownHook {
            release: Mutex::new(Some(release)),
        }));
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = Arc::clone(&completed);
        assert!(runtime
            .spawn_supervisor_task("owned_test", move || {
                waiting.recv_timeout(Duration::from_secs(3)).unwrap();
                worker_completed.store(true, Ordering::SeqCst);
            })
            .unwrap());
        assert!(!runtime
            .spawn_supervisor_task("owned_test", || panic!("Duplicate must not run"))
            .unwrap());
        runtime.shutdown();
        assert!(completed.load(Ordering::SeqCst));
    }

    #[test]
    fn pause_resume_retains_future_workflow_deadline_and_step_consistency() {
        let (_dir, runtime) = runtime();
        let now = chrono::Utc::now().timestamp();
        let run_id = runtime
            .submit_newspaper_download(
                "future".to_string(),
                "edition".to_string(),
                "{}".to_string(),
                String::new(),
                now,
                Some(now + 3600),
            )
            .unwrap();
        let deadlines = |at| {
            runtime
                .inner
                .service
                .next_ready_deadline(vec!["newspaper_download".to_string()], at)
                .unwrap()
        };
        assert_eq!(deadlines(now), Some(now + 3600));
        runtime
            .set_run_paused(run_id.clone(), true, now + 1)
            .unwrap();
        assert_eq!(
            runtime.get_run(run_id.clone()).unwrap().unwrap().state,
            RunState::Paused
        );
        assert_eq!(deadlines(now), None);
        runtime
            .set_run_paused(run_id.clone(), false, now + 2)
            .unwrap();
        let resumed = runtime.get_run(run_id.clone()).unwrap().unwrap();
        assert_eq!(resumed.state, RunState::RetryWait);
        assert_eq!(resumed.updated_at, now + 3600);
        assert_eq!(deadlines(now + 2), Some(now + 3600));
        assert_eq!(
            deadlines(now + 3601),
            Some(now + 3601),
            "A missed workflow deadline is due immediately"
        );
        let steps = runtime.inner.service.list_steps_for_run(run_id).unwrap();
        assert_eq!(steps[0].state, StepState::RetryWait);
        assert_eq!(steps[0].updated_at, now + 3600);
        assert!(runtime
            .inner
            .service
            .claim_next_ready_step("newspaper_download".to_string(), now + 3)
            .unwrap()
            .is_none());
    }

    #[test]
    fn completed_worker_releases_admission_for_the_next_run() {
        let (_dir, runtime) = runtime();
        let now = chrono::Utc::now().timestamp();
        let first = runtime.submit_synthetic("{}", now).unwrap();
        let second = runtime.submit_synthetic("{}", now + 1).unwrap();
        runtime.start_supervisor().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if runtime.get_run(second.clone()).unwrap().unwrap().state == RunState::Succeeded {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        runtime.shutdown();
        assert_eq!(
            runtime.get_run(first).unwrap().unwrap().state,
            RunState::Succeeded
        );
        assert_eq!(
            runtime.get_run(second).unwrap().unwrap().state,
            RunState::Succeeded
        );
    }

    #[test]
    fn rejected_worker_admission_releases_the_unstarted_claim_without_an_attempt() {
        let (_dir, runtime) = runtime();
        let now = chrono::Utc::now().timestamp();
        let run_id = runtime.submit_synthetic("{}", now).unwrap();
        let (claimed, guard) = claim_ready_step(&runtime, None).unwrap().unwrap();
        // Force the real shutdown race between durable claim and admission.
        runtime.inner.shutdown.store(true, Ordering::SeqCst);
        assert!(!runtime.dispatch_claimed_step(claimed, guard).unwrap());
        let run = runtime.get_run(run_id.clone()).unwrap().unwrap();
        let steps = runtime
            .inner
            .service
            .list_steps_for_run(run_id.clone())
            .unwrap();
        assert_eq!(run.state, RunState::RetryWait);
        assert_eq!(steps[0].state, StepState::RetryWait);
        assert_eq!(steps[0].attempt, 0);
        assert!(steps[0].updated_at <= chrono::Utc::now().timestamp());
        runtime.inner.shutdown.store(false, Ordering::SeqCst);
        assert!(runtime.drain_once().unwrap().processed);
        assert_eq!(
            runtime.get_run(run_id).unwrap().unwrap().state,
            RunState::Succeeded
        );
    }

    #[test]
    fn rejected_worker_admission_does_not_overwrite_pause_or_cancel() {
        for paused in [true, false] {
            let (_dir, runtime) = runtime();
            let now = chrono::Utc::now().timestamp();
            let run_id = runtime.submit_synthetic("{}", now).unwrap();
            let (claimed, guard) = claim_ready_step(&runtime, None).unwrap().unwrap();
            if paused {
                runtime
                    .inner
                    .service
                    .transition_run(
                        run_id.clone(),
                        RunState::Paused,
                        None,
                        "run_paused",
                        "{}".to_string(),
                        now,
                    )
                    .unwrap();
            } else {
                runtime.cancel_run(run_id.clone(), now).unwrap();
            }
            runtime.inner.shutdown.store(true, Ordering::SeqCst);
            assert!(!runtime.dispatch_claimed_step(claimed, guard).unwrap());
            let run = runtime.get_run(run_id.clone()).unwrap().unwrap();
            let steps = runtime.inner.service.list_steps_for_run(run_id).unwrap();
            assert_eq!(
                run.state,
                if paused {
                    RunState::Paused
                } else {
                    RunState::Cancelled
                }
            );
            assert_eq!(
                steps[0].state,
                if paused {
                    StepState::RetryWait
                } else {
                    StepState::Cancelled
                }
            );
            assert_eq!(steps[0].attempt, 0);
        }
    }

    #[test]
    fn managed_task_admission_suppresses_an_overdue_workflow_deadline() {
        let (_dir, runtime) = runtime();
        let now = chrono::Utc::now().timestamp();
        runtime.submit_synthetic("{}", now).unwrap();
        assert_eq!(runtime.next_ready_deadline(now).unwrap(), Some(now));
        let (release, waiting) = std::sync::mpsc::channel();
        assert!(runtime
            .spawn_supervisor_task("synthetic", move || {
                waiting.recv_timeout(Duration::from_secs(3)).unwrap();
            })
            .unwrap());
        assert!(runtime.inner.executing_types.lock().unwrap().is_empty());
        assert_eq!(
            runtime.next_ready_deadline(now).unwrap(),
            None,
            "An admitted managed task must suppress claim and deadline identically"
        );
        release.send(()).unwrap();
        runtime.shutdown();
        assert_eq!(runtime.next_ready_deadline(now).unwrap(), Some(now));
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
