//! Provider-agnostic helper control types.
//!
//! These types used to live beside the YouTube V1 in-memory scheduler. The
//! durable workflow kernel now owns run persistence; this module only carries
//! cancellation, pause, discovery, and cleanup-verification handles used by
//! the managed-process supervisor.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use thiserror::Error;

pub const EVENT_NAME: &str = "linkvault://youtube-run-changed";
pub const MAX_SELECTED_ITEMS: usize = 100;
const MAX_RETAINED_CLEANUP_VERIFIERS: usize = 16;

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
    pub warnings: Vec<TransientWarning>,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
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

    pub fn skipped_existing(artifacts: Vec<String>) -> Self {
        Self {
            state: TransientItemState::SkippedExisting,
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

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TransientCleanupVerificationError {
    #[error("cleanup remains unproven")]
    Unproven,
}

pub trait TransientCleanupVerifier: Send + Sync {
    fn verify_cleanup(&self) -> Result<(), TransientCleanupVerificationError>;
}

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
    #[error("client submission id is retired")]
    SubmissionRetired,
    #[error("submission capacity is exhausted")]
    SubmissionCapacity,
    #[error("a download run is already active")]
    RunAlreadyActive,
    #[error("submission admission is still in progress")]
    SubmissionPending,
    #[error("worker creation failed after Start admission")]
    WorkerSpawnFailed,
    #[error("transient discovery capacity is exhausted")]
    DiscoveryCapacity,
}

pub struct TransientRunControl {
    cancelled: AtomicBool,
    pause_requested: AtomicBool,
    cleanup_verifier_overflow: AtomicBool,
    paused: Mutex<bool>,
    wake: Condvar,
    cleanup_verifiers: Mutex<Vec<Arc<dyn TransientCleanupVerifier>>>,
}

impl std::fmt::Debug for TransientRunControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransientRunControl")
            .field("cancelled", &self.is_cancelled())
            .field("pause_requested", &self.pause_requested())
            .finish_non_exhaustive()
    }
}

impl Default for TransientRunControl {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            pause_requested: AtomicBool::new(false),
            cleanup_verifier_overflow: AtomicBool::new(false),
            paused: Mutex::new(false),
            wake: Condvar::new(),
            cleanup_verifiers: Mutex::new(Vec::new()),
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

    pub fn register_cleanup_verifier(
        &self,
        verifier: Arc<dyn TransientCleanupVerifier>,
    ) -> Result<(), TransientRuntimeError> {
        let mut verifiers = self
            .cleanup_verifiers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.cleanup_verifier_overflow.load(Ordering::Acquire)
            || verifiers.len() >= MAX_RETAINED_CLEANUP_VERIFIERS
        {
            self.cleanup_verifier_overflow
                .store(true, Ordering::Release);
            return Err(TransientRuntimeError::Quarantined);
        }
        verifiers.push(verifier);
        Ok(())
    }

    pub(crate) fn take_cleanup_verifiers(&self) -> (Vec<Arc<dyn TransientCleanupVerifier>>, bool) {
        let mut verifiers = self
            .cleanup_verifiers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            std::mem::take(&mut *verifiers),
            self.cleanup_verifier_overflow.load(Ordering::Acquire),
        )
    }

    /// Retries registered helper-temp cleanup. Failed verifiers stay registered
    /// so a later exit-equivalent retry can try again. Returns true only when
    /// every retained root is proven gone.
    pub fn retry_cleanup_verifiers(&self) -> bool {
        retry_registered_cleanup_verifiers(self.take_cleanup_verifiers(), |verifier| {
            self.register_cleanup_verifier(verifier)
        })
    }

    #[cfg(test)]
    pub(crate) fn take_cleanup_verifier(&self) -> Option<Arc<dyn TransientCleanupVerifier>> {
        self.cleanup_verifiers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
    }

    pub fn mark_paused_if_requested(&self) -> bool {
        let mut paused = self
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.pause_requested.load(Ordering::Acquire) {
            return false;
        }
        *paused = true;
        true
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

    pub fn wait_for_resume(&self) {
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

pub struct DiscoveryOperation {
    cancel: Arc<AtomicBool>,
    cleanup_verifiers: Mutex<Vec<Arc<dyn TransientCleanupVerifier>>>,
    cleanup_verifier_overflow: AtomicBool,
}

impl DiscoveryOperation {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            cleanup_verifiers: Mutex::new(Vec::new()),
            cleanup_verifier_overflow: AtomicBool::new(false),
        }
    }

    pub fn with_cancel(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            cleanup_verifiers: Mutex::new(Vec::new()),
            cleanup_verifier_overflow: AtomicBool::new(false),
        }
    }

    pub fn cancellation_requested(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn cancellation_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(Arc::clone(&self.cancel))
    }

    pub fn register_cleanup_verifier(
        &self,
        verifier: Arc<dyn TransientCleanupVerifier>,
    ) -> Result<(), TransientRuntimeError> {
        let mut verifiers = self
            .cleanup_verifiers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.cleanup_verifier_overflow.load(Ordering::Acquire)
            || verifiers.len() >= MAX_RETAINED_CLEANUP_VERIFIERS
        {
            self.cleanup_verifier_overflow
                .store(true, Ordering::Release);
            return Err(TransientRuntimeError::Quarantined);
        }
        verifiers.push(verifier);
        Ok(())
    }

    fn take_cleanup_verifiers(&self) -> (Vec<Arc<dyn TransientCleanupVerifier>>, bool) {
        let mut verifiers = self
            .cleanup_verifiers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            std::mem::take(&mut *verifiers),
            self.cleanup_verifier_overflow.load(Ordering::Acquire),
        )
    }

    pub fn retry_cleanup_verifiers(&self) -> bool {
        retry_registered_cleanup_verifiers(self.take_cleanup_verifiers(), |verifier| {
            self.register_cleanup_verifier(verifier)
        })
    }
}

impl Default for DiscoveryOperation {
    fn default() -> Self {
        Self::new()
    }
}

fn retry_registered_cleanup_verifiers(
    (verifiers, overflow): (Vec<Arc<dyn TransientCleanupVerifier>>, bool),
    mut register: impl FnMut(Arc<dyn TransientCleanupVerifier>) -> Result<(), TransientRuntimeError>,
) -> bool {
    if overflow {
        for verifier in verifiers {
            let _ = register(verifier);
        }
        return false;
    }
    let mut remaining = Vec::new();
    for verifier in verifiers {
        if verifier.verify_cleanup().is_err() {
            remaining.push(verifier);
        }
    }
    let all_ok = remaining.is_empty();
    for verifier in remaining {
        let _ = register(verifier);
    }
    all_ok
}
