//! Bounded, in-memory workflow runtime for the YouTube V1 bridge.
//!
//! This module deliberately knows nothing about YouTube.  Provider adapters
//! submit typed work items and an executor implementation; this runtime owns
//! admission, revisions, pause-after-current-item, cancellation, and the
//! reconstructable snapshot emitted to the renderer.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

pub mod managed_process;

pub const EVENT_NAME: &str = "linkvault://youtube-run-changed";
pub const MAX_SELECTED_ITEMS: usize = 100;
pub const MAX_DISCOVERY_TOMBSTONES: usize = 4096;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransientRunState {
    Running,
    PauseRequested,
    Paused,
    Cancelling,
    Completed,
    CompletedWithWarnings,
    Failed,
    Cancelled,
}

impl TransientRunState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::CompletedWithWarnings | Self::Failed | Self::Cancelled
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::PauseRequested => "pause_requested",
            Self::Paused => "paused",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::CompletedWithWarnings => "completed_with_warnings",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransientItemState {
    Pending,
    Running,
    Completed,
    CompletedWithWarnings,
    Failed,
    Cancelled,
    Skipped,
    SkippedExisting,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransientItemPhase {
    Waiting,
    Transcript,
    Media,
    Merging,
    NormalizingTranscript,
    Verifying,
    Completed,
    Warning,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransientWarning {
    pub occurrence_id: Option<String>,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TransientError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransientCurrentItem {
    pub occurrence_id: String,
    pub artifact_fingerprint: String,
    pub video_id: String,
    pub ordinal: u32,
    pub title: String,
    pub state: TransientItemState,
    pub phase: TransientItemPhase,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransientProgress {
    pub bytes_completed: Option<u64>,
    pub bytes_total: Option<u64>,
    pub fraction: Option<f64>,
}

impl Default for TransientProgress {
    fn default() -> Self {
        Self {
            bytes_completed: None,
            bytes_total: None,
            fraction: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransientCounts {
    pub completed: u32,
    pub completed_with_warnings: u32,
    pub selected: u32,
    pub failed: u32,
    pub skipped: u32,
    pub cancelled: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransientItemOutcomeSnapshot {
    pub occurrence_id: String,
    pub artifact_fingerprint: String,
    pub video_id: String,
    pub ordinal: u32,
    pub title: String,
    pub state: TransientItemState,
    pub phase: TransientItemPhase,
    pub warnings: Vec<String>,
    pub error: Option<TransientError>,
    pub published_artifact_kinds: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransientRunSnapshot {
    pub schema_version: u32,
    pub run_id: String,
    pub revision: u64,
    pub state: TransientRunState,
    pub item: Option<TransientCurrentItem>,
    pub progress: TransientProgress,
    pub counts: TransientCounts,
    pub warnings: Vec<TransientWarning>,
    pub error: Option<TransientError>,
    pub client_submission_id: String,
    pub plan_fingerprint: String,
    pub items: Vec<TransientItemOutcomeSnapshot>,
}

#[derive(Clone, Debug)]
pub struct TransientWorkItem {
    pub occurrence_id: String,
    pub artifact_fingerprint: String,
    pub video_id: String,
    pub ordinal: u32,
    pub title: String,
    pub source_url: String,
}

#[derive(Clone, Debug)]
pub struct TransientProgressUpdate {
    pub occurrence_id: String,
    pub phase: TransientItemPhase,
    pub bytes_completed: Option<u64>,
    pub bytes_total: Option<u64>,
    pub fraction: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct TransientExecutionOutcome {
    pub state: TransientItemState,
    pub phase: TransientItemPhase,
    pub warnings: Vec<String>,
    pub error: Option<TransientError>,
    pub published_artifact_kinds: Vec<String>,
}

impl TransientExecutionOutcome {
    pub fn completed(artifacts: Vec<String>) -> Self {
        Self {
            state: TransientItemState::Completed,
            phase: TransientItemPhase::Completed,
            warnings: Vec::new(),
            error: None,
            published_artifact_kinds: artifacts,
        }
    }

    pub fn warning(code: impl Into<String>, artifacts: Vec<String>) -> Self {
        Self {
            state: TransientItemState::CompletedWithWarnings,
            phase: TransientItemPhase::Warning,
            warnings: vec![code.into()],
            error: None,
            published_artifact_kinds: artifacts,
        }
    }
}

pub trait TransientExecutor: Send + Sync {
    fn execute(
        &self,
        item: &TransientWorkItem,
        control: &TransientRunControl,
        progress: &mut dyn FnMut(TransientProgressUpdate),
    ) -> Result<TransientExecutionOutcome, TransientError>;
}

#[derive(Debug)]
pub struct TransientRunControl {
    cancelled: AtomicBool,
    pause_requested: AtomicBool,
    paused: Mutex<bool>,
    wake: Condvar,
}

impl Default for TransientRunControl {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            pause_requested: AtomicBool::new(false),
            paused: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
}

impl TransientRunControl {
    pub fn request_cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn request_pause(&self) {
        self.pause_requested.store(true, Ordering::Release);
    }

    pub fn withdraw_pause(&self) {
        self.pause_requested.store(false, Ordering::Release);
        self.resume();
    }

    pub fn pause_requested(&self) -> bool {
        self.pause_requested.load(Ordering::Acquire)
    }

    fn mark_paused(&self) {
        let mut paused = self
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *paused = true;
    }

    fn resume(&self) {
        self.pause_requested.store(false, Ordering::Release);
        let mut paused = self
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *paused = false;
        self.wake.notify_all();
    }

    fn wait_for_resume(&self) {
        let mut paused = self
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *paused && !self.is_cancelled() {
            paused = self
                .wake
                .wait(paused)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

pub type EventSink = Arc<dyn Fn(&TransientRunSnapshot) + Send + Sync + 'static>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransientRuntimeError {
    #[error("transient workflow is busy")]
    Busy,
    #[error("transient workflow is shutting down")]
    ShuttingDown,
    #[error("transient workflow is quarantined")]
    Quarantined,
    #[error("run was not found")]
    RunNotFound,
    #[error("run revision is stale")]
    StaleRevision,
    #[error("invalid run state transition")]
    InvalidTransition,
    #[error("discovery operation was not found")]
    DiscoveryNotFound,
    #[error("discovery operation id was already used")]
    OperationIdReused,
    #[error("client submission id was already used for different input")]
    SubmissionConflict,
    #[error("transient discovery capacity is exhausted")]
    DiscoveryCapacity,
}

#[derive(Debug)]
enum AdmissionState {
    Idle,
    Discovering {
        operation_id: String,
        cancel: Arc<AtomicBool>,
    },
    Running,
    ShuttingDown,
}

#[derive(Debug)]
struct Admission {
    state: Mutex<AdmissionState>,
    discovery_tombstones: Mutex<HashSet<String>>,
}

impl Default for Admission {
    fn default() -> Self {
        Self {
            state: Mutex::new(AdmissionState::Idle),
            discovery_tombstones: Mutex::new(HashSet::new()),
        }
    }
}

struct DiscoveryGuard {
    admission: Arc<Admission>,
    operation_id: String,
    shutting_down: Arc<AtomicBool>,
    shutdown_signal: Arc<ShutdownSignal>,
}

impl DiscoveryGuard {
    fn cancellation_requested(&self) -> bool {
        let state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        matches!(
            &*state,
            AdmissionState::Discovering { operation_id, cancel }
                if operation_id == &self.operation_id && cancel.load(Ordering::Acquire)
        )
    }
}

impl Drop for DiscoveryGuard {
    fn drop(&mut self) {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(&*state, AdmissionState::Discovering { operation_id, .. } if operation_id == &self.operation_id)
        {
            *state = if self.shutting_down.load(Ordering::Acquire) {
                AdmissionState::ShuttingDown
            } else {
                AdmissionState::Idle
            };
        }
        drop(state);
        self.shutdown_signal.notify();
    }
}

#[derive(Debug, Default)]
struct ShutdownSignal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl ShutdownSignal {
    fn notify(&self) {
        let mut generation = self
            .generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *generation = generation.saturating_add(1);
        self.changed.notify_all();
    }
}

struct ActiveRun {
    run_id: String,
    control: Arc<TransientRunControl>,
    record: Arc<Mutex<RunRecord>>,
}

#[derive(Debug)]
struct RunRecord {
    snapshot: TransientRunSnapshot,
    work_items: Vec<TransientWorkItem>,
}

#[derive(Clone)]
pub struct TransientWorkflowRuntime {
    inner: Arc<TransientRuntimeInner>,
}

struct TransientRuntimeInner {
    admission: Arc<Admission>,
    active: Mutex<Option<ActiveRun>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    most_recent: Mutex<Option<TransientRunSnapshot>>,
    terminal_snapshots: Mutex<HashMap<String, TransientRunSnapshot>>,
    event_sink: Option<EventSink>,
    shutting_down: Arc<AtomicBool>,
    shutdown_signal: Arc<ShutdownSignal>,
    submission_receipts: Mutex<SubmissionReplayCache>,
    #[cfg(test)]
    fail_next_worker_spawn: AtomicBool,
}

#[derive(Debug, Default)]
struct SubmissionReplayCache {
    receipts: HashMap<String, TransientSubmissionReceipt>,
}

#[derive(Clone, Debug)]
pub struct TransientSubmissionReceipt {
    pub client_submission_id: String,
    pub request_fingerprint: String,
    pub plan_fingerprint: String,
    pub run_id: String,
    pub revision: u64,
    pub scan_plan_id: String,
}

/// Composition-root-owned state for the non-durable transient workflow.
/// Provider adapters receive this state as a separate Tauri dependency; they
/// do not own the runtime or submission replay registry.
#[derive(Clone)]
pub struct TransientWorkflowState {
    runtime: TransientWorkflowRuntime,
}

impl TransientWorkflowState {
    pub fn new(event_sink: Option<EventSink>) -> Self {
        Self {
            runtime: TransientWorkflowRuntime::new(event_sink),
        }
    }

    pub fn runtime(&self) -> &TransientWorkflowRuntime {
        &self.runtime
    }

    pub fn shutdown(&self) {
        self.runtime.shutdown();
    }

    pub fn shutdown_and_wait(&self, timeout: Duration) -> bool {
        self.runtime.shutdown_and_wait(timeout)
    }
}

impl TransientWorkflowRuntime {
    pub fn new(event_sink: Option<EventSink>) -> Self {
        Self {
            inner: Arc::new(TransientRuntimeInner {
                admission: Arc::new(Admission::default()),
                active: Mutex::new(None),
                worker: Mutex::new(None),
                most_recent: Mutex::new(None),
                terminal_snapshots: Mutex::new(HashMap::new()),
                event_sink,
                shutting_down: Arc::new(AtomicBool::new(false)),
                shutdown_signal: Arc::new(ShutdownSignal::default()),
                submission_receipts: Mutex::new(SubmissionReplayCache::default()),
                #[cfg(test)]
                fail_next_worker_spawn: AtomicBool::new(false),
            }),
        }
    }

    pub fn find_submission(
        &self,
        client_submission_id: &str,
        request_fingerprint: &str,
    ) -> Result<Option<TransientSubmissionReceipt>, TransientRuntimeError> {
        let receipts = self
            .inner
            .submission_receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(receipt) = receipts.receipts.get(client_submission_id) {
            if receipt.request_fingerprint == request_fingerprint {
                return Ok(Some(receipt.clone()));
            }
            return Err(TransientRuntimeError::SubmissionConflict);
        }
        Ok(None)
    }

    pub fn record_submission(&self, receipt: TransientSubmissionReceipt) {
        let mut receipts = self
            .inner
            .submission_receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = receipt.client_submission_id.clone();
        receipts.receipts.entry(key).or_insert(receipt);
    }

    pub fn begin_discovery(
        &self,
        operation_id: impl Into<String>,
    ) -> Result<DiscoveryOperation, TransientRuntimeError> {
        let operation_id = operation_id.into();
        if operation_id.is_empty() {
            return Err(TransientRuntimeError::OperationIdReused);
        }
        let mut tombstones = self
            .inner
            .admission
            .discovery_tombstones
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if tombstones.contains(&operation_id) {
            return Err(TransientRuntimeError::OperationIdReused);
        }
        if tombstones.len() >= MAX_DISCOVERY_TOMBSTONES {
            return Err(TransientRuntimeError::DiscoveryCapacity);
        }
        tombstones.insert(operation_id.clone());
        drop(tombstones);

        let mut state = self
            .inner
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(TransientRuntimeError::ShuttingDown);
        }
        if !matches!(&*state, AdmissionState::Idle) {
            return Err(match &*state {
                AdmissionState::ShuttingDown => TransientRuntimeError::ShuttingDown,
                _ => TransientRuntimeError::Busy,
            });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *state = AdmissionState::Discovering {
            operation_id: operation_id.clone(),
            cancel,
        };
        Ok(DiscoveryOperation {
            guard: Some(DiscoveryGuard {
                admission: Arc::clone(&self.inner.admission),
                operation_id,
                shutting_down: Arc::clone(&self.inner.shutting_down),
                shutdown_signal: Arc::clone(&self.inner.shutdown_signal),
            }),
        })
    }

    pub fn cancel_discovery(&self, operation_id: &str) -> Result<(), TransientRuntimeError> {
        let state = self
            .inner
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &*state {
            AdmissionState::Discovering {
                operation_id: active,
                cancel,
            } if active == operation_id => {
                cancel.store(true, Ordering::Release);
                Ok(())
            }
            _ => Err(TransientRuntimeError::DiscoveryNotFound),
        }
    }

    pub fn start_run(
        &self,
        run_id: String,
        client_submission_id: String,
        plan_fingerprint: String,
        work_items: Vec<TransientWorkItem>,
        executor: Arc<dyn TransientExecutor>,
    ) -> Result<TransientRunSnapshot, TransientRuntimeError> {
        if work_items.is_empty() || work_items.len() > MAX_SELECTED_ITEMS {
            return Err(TransientRuntimeError::InvalidTransition);
        }
        let mut admission = self
            .inner
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(TransientRuntimeError::ShuttingDown);
        }
        if !matches!(&*admission, AdmissionState::Idle) {
            return Err(match &*admission {
                AdmissionState::ShuttingDown => TransientRuntimeError::ShuttingDown,
                _ => TransientRuntimeError::Busy,
            });
        }
        self.reap_worker();
        let selected = work_items.len() as u32;
        let items = work_items
            .iter()
            .map(|item| TransientItemOutcomeSnapshot {
                occurrence_id: item.occurrence_id.clone(),
                artifact_fingerprint: item.artifact_fingerprint.clone(),
                video_id: item.video_id.clone(),
                ordinal: item.ordinal,
                title: item.title.clone(),
                state: TransientItemState::Pending,
                phase: TransientItemPhase::Waiting,
                warnings: Vec::new(),
                error: None,
                published_artifact_kinds: Vec::new(),
            })
            .collect::<Vec<_>>();
        let snapshot = TransientRunSnapshot {
            schema_version: 1,
            run_id: run_id.clone(),
            revision: 1,
            state: TransientRunState::Running,
            item: None,
            progress: TransientProgress::default(),
            counts: TransientCounts {
                completed: 0,
                completed_with_warnings: 0,
                selected,
                failed: 0,
                skipped: 0,
                cancelled: 0,
            },
            warnings: Vec::new(),
            error: None,
            client_submission_id,
            plan_fingerprint,
            items,
        };
        *admission = AdmissionState::Running;
        drop(admission);

        let control = Arc::new(TransientRunControl::default());
        let record = Arc::new(Mutex::new(RunRecord {
            snapshot: snapshot.clone(),
            work_items,
        }));
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = Some(ActiveRun {
            run_id: run_id.clone(),
            control: Arc::clone(&control),
            record: Arc::clone(&record),
        });
        drop(active);

        let inner = Arc::clone(&self.inner);
        let start_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = Arc::clone(&start_gate);
        #[cfg(test)]
        let force_spawn_failure = self
            .inner
            .fail_next_worker_spawn
            .swap(false, Ordering::AcqRel);
        #[cfg(not(test))]
        let force_spawn_failure = false;
        let worker = if force_spawn_failure {
            Err(std::io::Error::other("injected worker spawn failure"))
        } else {
            thread::Builder::new()
                .name("linkvault-youtube-run".to_string())
                .spawn(move || {
                    let (ready, changed) = &*worker_gate;
                    let ready = ready
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let ready = changed
                        .wait_while(ready, |ready| !*ready)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    drop(ready);
                    execute_run(inner, record, control, executor);
                })
        };
        let worker = match worker {
            Ok(worker) => worker,
            Err(_) => {
                *self
                    .inner
                    .active
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                let mut admission = self
                    .inner
                    .admission
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *admission = if self.inner.shutting_down.load(Ordering::Acquire) {
                    AdmissionState::ShuttingDown
                } else {
                    AdmissionState::Idle
                };
                return Err(TransientRuntimeError::InvalidTransition);
            }
        };
        *self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(worker);
        self.emit(&snapshot);
        let (ready, changed) = &*start_gate;
        *ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        changed.notify_one();
        Ok(snapshot)
    }

    pub fn get_state(
        &self,
        run_id: Option<&str>,
    ) -> Result<Option<TransientRunSnapshot>, TransientRuntimeError> {
        if let Some(run_id) = run_id {
            let active = self
                .inner
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(active) = active.as_ref().filter(|active| active.run_id == run_id) {
                let snapshot = active
                    .record
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .snapshot
                    .clone();
                return Ok(Some(snapshot));
            }
            let terminal_snapshots = self
                .inner
                .terminal_snapshots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            return Ok(terminal_snapshots.get(run_id).cloned());
        }
        if let Some(active) = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            return Ok(Some(
                active
                    .record
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .snapshot
                    .clone(),
            ));
        }
        Ok(self
            .inner
            .most_recent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    pub fn pause(
        &self,
        run_id: &str,
        expected_revision: u64,
    ) -> Result<TransientRunSnapshot, TransientRuntimeError> {
        let active = self.active_run(run_id)?;
        let mut record = active
            .record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ensure_revision(&record.snapshot, expected_revision)?;
        if !matches!(record.snapshot.state, TransientRunState::Running) {
            return Err(TransientRuntimeError::InvalidTransition);
        }
        active.control.request_pause();
        record.snapshot.state = TransientRunState::PauseRequested;
        let snapshot = commit_revision(&mut record.snapshot);
        drop(record);
        self.emit(&snapshot);
        Ok(snapshot)
    }

    pub fn resume(
        &self,
        run_id: &str,
        expected_revision: u64,
    ) -> Result<TransientRunSnapshot, TransientRuntimeError> {
        let active = self.active_run(run_id)?;
        let mut record = active
            .record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ensure_revision(&record.snapshot, expected_revision)?;
        if !matches!(
            record.snapshot.state,
            TransientRunState::Paused | TransientRunState::PauseRequested
        ) {
            return Err(TransientRuntimeError::InvalidTransition);
        }
        active.control.withdraw_pause();
        record.snapshot.state = TransientRunState::Running;
        let snapshot = commit_revision(&mut record.snapshot);
        drop(record);
        self.emit(&snapshot);
        Ok(snapshot)
    }

    pub fn cancel(&self, run_id: &str) -> Result<TransientRunSnapshot, TransientRuntimeError> {
        let active = self.active_run(run_id)?;
        let mut record = active
            .record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if record.snapshot.state.is_terminal() {
            return Ok(record.snapshot.clone());
        }
        active.control.request_cancel();
        if !matches!(record.snapshot.state, TransientRunState::Cancelling) {
            record.snapshot.state = TransientRunState::Cancelling;
            let snapshot = commit_revision(&mut record.snapshot);
            drop(record);
            self.emit(&snapshot);
            return Ok(snapshot);
        }
        Ok(record.snapshot.clone())
    }

    pub fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        let mut admission = self
            .inner
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &*admission {
            AdmissionState::Idle => {
                *admission = AdmissionState::ShuttingDown;
            }
            AdmissionState::Discovering { cancel, .. } => {
                cancel.store(true, Ordering::Release);
            }
            AdmissionState::Running | AdmissionState::ShuttingDown => {}
        }
        drop(admission);
        if let Some(active) = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            active.control.request_cancel();
        }
    }

    pub fn shutdown_and_wait(&self, timeout: Duration) -> bool {
        self.shutdown();
        let deadline = Instant::now() + timeout;
        loop {
            let generation = self
                .inner
                .shutdown_signal
                .generation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.is_quiescent() {
                self.reap_worker();
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let observed = *generation;
            let (generation, wait) = self
                .inner
                .shutdown_signal
                .changed
                .wait_timeout_while(generation, remaining, |current| *current == observed)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            drop(generation);
            if wait.timed_out() && !self.is_quiescent() {
                return false;
            }
        }
    }

    fn is_quiescent(&self) -> bool {
        let admission = self
            .inner
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(&*admission, AdmissionState::ShuttingDown) {
            return false;
        }
        drop(admission);
        self.inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none()
    }

    fn reap_worker(&self) {
        let worker = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    #[cfg(test)]
    fn inject_worker_spawn_failure(&self) {
        self.inner
            .fail_next_worker_spawn
            .store(true, Ordering::Release);
    }

    fn active_run(&self, run_id: &str) -> Result<ActiveRun, TransientRuntimeError> {
        let active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active = active
            .as_ref()
            .filter(|active| active.run_id == run_id)
            .ok_or(TransientRuntimeError::RunNotFound)?;
        Ok(ActiveRun {
            run_id: active.run_id.clone(),
            control: Arc::clone(&active.control),
            record: Arc::clone(&active.record),
        })
    }

    fn emit(&self, snapshot: &TransientRunSnapshot) {
        if let Some(sink) = &self.inner.event_sink {
            sink(snapshot);
        }
    }
}

pub struct DiscoveryOperation {
    guard: Option<DiscoveryGuard>,
}

impl DiscoveryOperation {
    pub fn cancellation_requested(&self) -> bool {
        self.guard
            .as_ref()
            .is_some_and(DiscoveryGuard::cancellation_requested)
    }

    pub fn cancellation_flag(&self) -> Option<Arc<AtomicBool>> {
        self.guard.as_ref().map(|guard| {
            let state = guard
                .admission
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &*state {
                AdmissionState::Discovering {
                    operation_id,
                    cancel,
                } if operation_id == &guard.operation_id => Arc::clone(cancel),
                _ => Arc::new(AtomicBool::new(false)),
            }
        })
    }
}

fn ensure_revision(
    snapshot: &TransientRunSnapshot,
    expected_revision: u64,
) -> Result<(), TransientRuntimeError> {
    if snapshot.revision != expected_revision {
        Err(TransientRuntimeError::StaleRevision)
    } else {
        Ok(())
    }
}

fn commit_revision(snapshot: &mut TransientRunSnapshot) -> TransientRunSnapshot {
    snapshot.revision = snapshot.revision.saturating_add(1);
    snapshot.clone()
}

fn execute_run(
    inner: Arc<TransientRuntimeInner>,
    record: Arc<Mutex<RunRecord>>,
    control: Arc<TransientRunControl>,
    executor: Arc<dyn TransientExecutor>,
) {
    let work_items = record
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .work_items
        .clone();
    for (index, item) in work_items.iter().enumerate() {
        if control.is_cancelled() {
            mark_remaining_cancelled(&inner, &record, index);
            finish_run(&inner, &record, &control, TransientRunState::Cancelled);
            return;
        }
        if control.pause_requested() && index > 0 {
            set_paused(&inner, &record, &control);
            if control.is_cancelled() {
                mark_remaining_cancelled(&inner, &record, index);
                finish_run(&inner, &record, &control, TransientRunState::Cancelled);
                return;
            }
        }
        set_item_running(&inner, &record, item);
        let mut progress = |update: TransientProgressUpdate| {
            update_progress(&inner, &record, update);
        };
        let result = executor.execute(item, &control, &mut progress);
        if control.is_cancelled() {
            mark_item_cancelled(&inner, &record, item);
            mark_remaining_cancelled(&inner, &record, index.saturating_add(1));
            finish_run(&inner, &record, &control, TransientRunState::Cancelled);
            return;
        }
        let committed = match result {
            Ok(outcome) => commit_item_outcome(&inner, &record, &control, item, outcome),
            Err(error) => commit_item_failure(&inner, &record, &control, item, error),
        };
        if !committed {
            mark_item_cancelled(&inner, &record, item);
            mark_remaining_cancelled(&inner, &record, index.saturating_add(1));
            finish_run(&inner, &record, &control, TransientRunState::Cancelled);
            return;
        }
        if control.pause_requested() && index + 1 < work_items.len() {
            set_paused(&inner, &record, &control);
        }
    }
    let has_warnings = record
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .snapshot
        .items
        .iter()
        .any(|item| {
            item.state == TransientItemState::CompletedWithWarnings
                || item.state == TransientItemState::Failed
        });
    finish_run(
        &inner,
        &record,
        &control,
        if has_warnings {
            TransientRunState::CompletedWithWarnings
        } else {
            TransientRunState::Completed
        },
    );
}

fn set_item_running(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    item: &TransientWorkItem,
) {
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(outcome) = record
            .snapshot
            .items
            .iter_mut()
            .find(|candidate| candidate.occurrence_id == item.occurrence_id)
        {
            outcome.state = TransientItemState::Running;
            outcome.phase = TransientItemPhase::Media;
        }
        record.snapshot.item = Some(TransientCurrentItem {
            occurrence_id: item.occurrence_id.clone(),
            artifact_fingerprint: item.artifact_fingerprint.clone(),
            video_id: item.video_id.clone(),
            ordinal: item.ordinal,
            title: item.title.clone(),
            state: TransientItemState::Running,
            phase: TransientItemPhase::Media,
        });
        record.snapshot.progress = TransientProgress::default();
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
}

fn update_progress(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    update: TransientProgressUpdate,
) {
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if record
            .snapshot
            .item
            .as_ref()
            .map_or(true, |item| item.occurrence_id != update.occurrence_id)
        {
            return;
        }
        record.snapshot.progress = TransientProgress {
            bytes_completed: update.bytes_completed,
            bytes_total: update.bytes_total,
            fraction: update.fraction,
        };
        if let Some(item) = record.snapshot.item.as_mut() {
            item.phase = update.phase.clone();
            if let Some(outcome) = record
                .snapshot
                .items
                .iter_mut()
                .find(|candidate| candidate.occurrence_id == update.occurrence_id)
            {
                outcome.phase = update.phase;
            }
        }
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
}

fn commit_item_outcome(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    control: &TransientRunControl,
    item: &TransientWorkItem,
    outcome: TransientExecutionOutcome,
) -> bool {
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if control.is_cancelled() {
            return false;
        }
        if let Some(candidate) = record
            .snapshot
            .items
            .iter_mut()
            .find(|candidate| candidate.occurrence_id == item.occurrence_id)
        {
            candidate.state = outcome.state.clone();
            candidate.phase = outcome.phase.clone();
            candidate.warnings = outcome.warnings.clone();
            candidate.error = outcome.error.clone();
            candidate.published_artifact_kinds = outcome.published_artifact_kinds.clone();
        }
        if outcome.state == TransientItemState::Completed {
            record.snapshot.counts.completed = record.snapshot.counts.completed.saturating_add(1);
        } else if outcome.state == TransientItemState::CompletedWithWarnings {
            record.snapshot.counts.completed_with_warnings = record
                .snapshot
                .counts
                .completed_with_warnings
                .saturating_add(1);
        } else if outcome.state == TransientItemState::Failed {
            record.snapshot.counts.failed = record.snapshot.counts.failed.saturating_add(1);
        }
        for code in &outcome.warnings {
            record.snapshot.warnings.push(TransientWarning {
                occurrence_id: Some(item.occurrence_id.clone()),
                code: code.clone(),
                message: code.clone(),
            });
        }
        record.snapshot.item = None;
        record.snapshot.progress = TransientProgress::default();
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
    true
}

fn commit_item_failure(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    control: &TransientRunControl,
    item: &TransientWorkItem,
    error: TransientError,
) -> bool {
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if control.is_cancelled() {
            return false;
        }
        if let Some(candidate) = record
            .snapshot
            .items
            .iter_mut()
            .find(|candidate| candidate.occurrence_id == item.occurrence_id)
        {
            candidate.state = TransientItemState::Failed;
            candidate.phase = TransientItemPhase::Failed;
            candidate.error = Some(error.clone());
        }
        record.snapshot.counts.failed = record.snapshot.counts.failed.saturating_add(1);
        record.snapshot.warnings.push(TransientWarning {
            occurrence_id: Some(item.occurrence_id.clone()),
            code: "ITEM_FAILED_CONTINUING".to_string(),
            message: error.message.clone(),
        });
        record.snapshot.item = None;
        record.snapshot.progress = TransientProgress::default();
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
    true
}

fn mark_item_cancelled(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    item: &TransientWorkItem,
) {
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(candidate) = record
            .snapshot
            .items
            .iter_mut()
            .find(|candidate| candidate.occurrence_id == item.occurrence_id)
        {
            if candidate.state == TransientItemState::Running {
                candidate.state = TransientItemState::Cancelled;
                candidate.phase = TransientItemPhase::Cancelled;
                record.snapshot.counts.cancelled =
                    record.snapshot.counts.cancelled.saturating_add(1);
            }
        }
        record.snapshot.item = None;
        record.snapshot.progress = TransientProgress::default();
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
}

fn mark_remaining_cancelled(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    start: usize,
) {
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut changed = false;
        let mut newly_cancelled = 0u32;
        for index in start..record.snapshot.items.len() {
            let candidate = &mut record.snapshot.items[index];
            if candidate.state == TransientItemState::Pending {
                candidate.state = TransientItemState::Cancelled;
                candidate.phase = TransientItemPhase::Cancelled;
                newly_cancelled = newly_cancelled.saturating_add(1);
                changed = true;
            }
        }
        record.snapshot.counts.cancelled = record
            .snapshot
            .counts
            .cancelled
            .saturating_add(newly_cancelled);
        if !changed {
            return;
        }
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
}

fn set_paused(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    control: &Arc<TransientRunControl>,
) {
    control.mark_paused();
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(
            record.snapshot.state,
            TransientRunState::PauseRequested | TransientRunState::Running
        ) {
            return;
        }
        record.snapshot.state = TransientRunState::Paused;
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
    control.wait_for_resume();
    if control.is_cancelled() {
        return;
    }
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(record.snapshot.state, TransientRunState::Paused) {
            return;
        }
        record.snapshot.state = TransientRunState::Running;
        commit_revision(&mut record.snapshot)
    };
    emit_inner(inner, &snapshot);
}

fn finish_run(
    inner: &Arc<TransientRuntimeInner>,
    record: &Arc<Mutex<RunRecord>>,
    control: &TransientRunControl,
    state: TransientRunState,
) {
    let snapshot = {
        let mut record = record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        record.snapshot.state = if control.is_cancelled() {
            TransientRunState::Cancelled
        } else {
            state
        };
        record.snapshot.item = None;
        record.snapshot.progress = TransientProgress::default();
        commit_revision(&mut record.snapshot)
    };
    *inner
        .most_recent
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot.clone());
    inner
        .terminal_snapshots
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(snapshot.run_id.clone(), snapshot.clone());
    *inner
        .active
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let mut admission = inner
        .admission
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if matches!(&*admission, AdmissionState::Running) {
        *admission = if inner.shutting_down.load(Ordering::Acquire) {
            AdmissionState::ShuttingDown
        } else {
            AdmissionState::Idle
        };
    }
    drop(admission);
    inner.shutdown_signal.notify();
    emit_inner(inner, &snapshot);
}

fn emit_inner(inner: &Arc<TransientRuntimeInner>, snapshot: &TransientRunSnapshot) {
    if let Some(sink) = &inner.event_sink {
        sink(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    struct FakeExecutor {
        calls: Arc<AtomicUsize>,
    }

    struct BlockingExecutor {
        started: Arc<AtomicBool>,
    }

    struct SuccessAfterReleaseExecutor {
        started: Arc<AtomicBool>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl TransientExecutor for FakeExecutor {
        fn execute(
            &self,
            item: &TransientWorkItem,
            control: &TransientRunControl,
            progress: &mut dyn FnMut(TransientProgressUpdate),
        ) -> Result<TransientExecutionOutcome, TransientError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            progress(TransientProgressUpdate {
                occurrence_id: item.occurrence_id.clone(),
                phase: TransientItemPhase::Media,
                bytes_completed: Some(1),
                bytes_total: Some(2),
                fraction: Some(0.5),
            });
            if control.is_cancelled() {
                return Err(TransientError {
                    code: "CANCELLED".to_string(),
                    message: "cancelled".to_string(),
                });
            }
            Ok(TransientExecutionOutcome::completed(vec![
                "media".to_string()
            ]))
        }
    }

    impl TransientExecutor for BlockingExecutor {
        fn execute(
            &self,
            _item: &TransientWorkItem,
            control: &TransientRunControl,
            _progress: &mut dyn FnMut(TransientProgressUpdate),
        ) -> Result<TransientExecutionOutcome, TransientError> {
            self.started.store(true, Ordering::Release);
            while !control.is_cancelled() {
                thread::sleep(Duration::from_millis(2));
            }
            Err(TransientError {
                code: "CANCELLED".to_string(),
                message: "cancelled".to_string(),
            })
        }
    }

    impl TransientExecutor for SuccessAfterReleaseExecutor {
        fn execute(
            &self,
            _item: &TransientWorkItem,
            _control: &TransientRunControl,
            _progress: &mut dyn FnMut(TransientProgressUpdate),
        ) -> Result<TransientExecutionOutcome, TransientError> {
            self.started.store(true, Ordering::Release);
            let (released, changed) = &*self.release;
            let released = released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let released = changed
                .wait_while(released, |released| !*released)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            drop(released);
            Ok(TransientExecutionOutcome::completed(vec![
                "media".to_string()
            ]))
        }
    }

    fn item(id: &str, ordinal: u32) -> TransientWorkItem {
        TransientWorkItem {
            occurrence_id: id.to_string(),
            artifact_fingerprint: format!("artifact-{id}"),
            video_id: format!("video-{id}"),
            ordinal,
            title: format!("Title {id}"),
            source_url: format!("https://www.youtube.com/watch?v={id}"),
        }
    }

    #[test]
    fn transition_table_and_revisions_are_monotonic() {
        let runtime = TransientWorkflowRuntime::new(None);
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(FakeExecutor {
            calls: Arc::clone(&calls),
        });
        let first = runtime
            .start_run(
                "run-1".to_string(),
                "submission-1".to_string(),
                "plan-1".to_string(),
                vec![item("one", 0)],
                executor,
            )
            .unwrap();
        assert_eq!(first.revision, 1);
        let mut last = first.revision;
        for _ in 0..50 {
            if let Some(snapshot) = runtime.get_state(Some("run-1")).unwrap() {
                last = last.max(snapshot.revision);
                if snapshot.state.is_terminal() {
                    assert_eq!(snapshot.state, TransientRunState::Completed);
                    break;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(last > 1);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(runtime.get_state(Some("missing")).unwrap().is_none());
    }

    #[test]
    fn duplicate_discovery_operation_is_rejected_and_cancel_is_scoped() {
        let runtime = TransientWorkflowRuntime::new(None);
        let operation = runtime.begin_discovery("operation-1").unwrap();
        assert!(runtime.begin_discovery("operation-1").is_err());
        runtime.cancel_discovery("operation-1").unwrap();
        assert!(operation.cancellation_requested());
        drop(operation);
        assert!(runtime.begin_discovery("operation-1").is_err());
        assert!(runtime.cancel_discovery("operation-1").is_err());
    }

    #[test]
    fn cancellation_marks_not_started_items_without_running_them() {
        let runtime = TransientWorkflowRuntime::new(None);
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(FakeExecutor {
            calls: Arc::clone(&calls),
        });
        let snapshot = runtime
            .start_run(
                "run-2".to_string(),
                "submission-2".to_string(),
                "plan-2".to_string(),
                vec![item("one", 0), item("two", 1)],
                executor,
            )
            .unwrap();
        runtime.cancel("run-2").unwrap();
        for _ in 0..100 {
            if let Some(snapshot) = runtime.get_state(Some("run-2")).unwrap() {
                if snapshot.state.is_terminal() {
                    assert_eq!(snapshot.state, TransientRunState::Cancelled);
                    assert_eq!(snapshot.counts.completed + snapshot.counts.cancelled, 2);
                    assert!(snapshot.revision > snapshot.revision.min(1));
                    break;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(calls.load(Ordering::Relaxed) <= 1);
        assert_eq!(snapshot.counts.selected, 2);
    }

    #[test]
    fn shutdown_cancels_an_active_run_and_waits_for_quiescence() {
        let runtime = TransientWorkflowRuntime::new(None);
        let started = Arc::new(AtomicBool::new(false));
        runtime
            .start_run(
                "shutdown-run".to_string(),
                "shutdown-submission".to_string(),
                "shutdown-plan".to_string(),
                vec![item("one", 0)],
                Arc::new(BlockingExecutor {
                    started: Arc::clone(&started),
                }),
            )
            .unwrap();
        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }

        assert!(runtime.shutdown_and_wait(Duration::from_secs(1)));
        let snapshot = runtime
            .get_state(Some("shutdown-run"))
            .unwrap()
            .expect("cancelled run snapshot");
        assert_eq!(snapshot.state, TransientRunState::Cancelled);
        assert_eq!(
            runtime
                .start_run(
                    "late-run".to_string(),
                    "late-submission".to_string(),
                    "late-plan".to_string(),
                    vec![item("late", 0)],
                    Arc::new(FakeExecutor {
                        calls: Arc::new(AtomicUsize::new(0)),
                    }),
                )
                .unwrap_err(),
            TransientRuntimeError::ShuttingDown
        );
    }

    #[test]
    fn shutdown_cancels_discovery_and_waits_for_its_guard() {
        let runtime = TransientWorkflowRuntime::new(None);
        let operation = runtime.begin_discovery("shutdown-discovery").unwrap();
        let waiter_runtime = runtime.clone();
        let waiter =
            thread::spawn(move || waiter_runtime.shutdown_and_wait(Duration::from_secs(1)));

        for _ in 0..100 {
            if operation.cancellation_requested() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(operation.cancellation_requested());
        drop(operation);
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn worker_spawn_failure_rolls_back_admission() {
        let runtime = TransientWorkflowRuntime::new(None);
        runtime.inject_worker_spawn_failure();
        let calls = Arc::new(AtomicUsize::new(0));
        let failed = runtime.start_run(
            "failed-spawn".to_string(),
            "failed-submission".to_string(),
            "failed-plan".to_string(),
            vec![item("failed", 0)],
            Arc::new(FakeExecutor {
                calls: Arc::clone(&calls),
            }),
        );
        assert_eq!(
            failed.unwrap_err(),
            TransientRuntimeError::InvalidTransition
        );

        runtime
            .start_run(
                "retry-spawn".to_string(),
                "retry-submission".to_string(),
                "retry-plan".to_string(),
                vec![item("retry", 0)],
                Arc::new(FakeExecutor {
                    calls: Arc::clone(&calls),
                }),
            )
            .unwrap();
        for _ in 0..100 {
            if runtime
                .get_state(Some("retry-spawn"))
                .unwrap()
                .is_some_and(|snapshot| snapshot.state.is_terminal())
            {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[test]
    fn submission_ids_remain_retired_for_the_process_lifetime() {
        let runtime = TransientWorkflowRuntime::new(None);
        for index in 0..300 {
            runtime.record_submission(TransientSubmissionReceipt {
                client_submission_id: format!("submission-{index}"),
                request_fingerprint: format!("request-{index}"),
                plan_fingerprint: format!("plan-{index}"),
                run_id: format!("run-{index}"),
                revision: 1,
                scan_plan_id: format!("scan-{index}"),
            });
        }

        let first = runtime
            .find_submission("submission-0", "request-0")
            .unwrap()
            .expect("first submission remains retired");
        assert_eq!(first.run_id, "run-0");
        assert_eq!(
            runtime
                .find_submission("submission-0", "different-request")
                .unwrap_err(),
            TransientRuntimeError::SubmissionConflict
        );
    }

    #[test]
    fn accepted_cancellation_wins_over_later_executor_success() {
        let runtime = TransientWorkflowRuntime::new(None);
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        runtime
            .start_run(
                "cancel-race".to_string(),
                "cancel-race-submission".to_string(),
                "cancel-race-plan".to_string(),
                vec![item("race", 0)],
                Arc::new(SuccessAfterReleaseExecutor {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                }),
            )
            .unwrap();
        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        let cancelling = runtime.cancel("cancel-race").unwrap();
        assert_eq!(cancelling.state, TransientRunState::Cancelling);
        let (released, changed) = &*release;
        *released
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        changed.notify_one();

        for _ in 0..100 {
            if let Some(snapshot) = runtime.get_state(Some("cancel-race")).unwrap() {
                if snapshot.state.is_terminal() {
                    assert_eq!(snapshot.state, TransientRunState::Cancelled);
                    assert_eq!(snapshot.counts.cancelled, 1);
                    assert_eq!(snapshot.counts.completed, 0);
                    return;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("cancelled terminal snapshot was not observed");
    }

    #[test]
    fn older_terminal_run_state_remains_available_for_exact_replay() {
        let runtime = TransientWorkflowRuntime::new(None);
        let calls = Arc::new(AtomicUsize::new(0));
        for run in ["first-terminal", "second-terminal"] {
            runtime
                .start_run(
                    run.to_string(),
                    format!("{run}-submission"),
                    format!("{run}-plan"),
                    vec![item(run, 0)],
                    Arc::new(FakeExecutor {
                        calls: Arc::clone(&calls),
                    }),
                )
                .unwrap();
            for _ in 0..100 {
                if runtime
                    .get_state(Some(run))
                    .unwrap()
                    .is_some_and(|snapshot| snapshot.state.is_terminal())
                {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }

        let first = runtime
            .get_state(Some("first-terminal"))
            .unwrap()
            .expect("older terminal state");
        assert_eq!(first.state, TransientRunState::Completed);
        assert_eq!(calls.load(Ordering::Acquire), 2);
    }
}
