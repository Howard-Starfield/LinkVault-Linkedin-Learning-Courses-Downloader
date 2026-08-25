//! In-process YouTube discovery/run handles.
//!
//! Scan plans and live snapshots stay here. Durable run persistence belongs to
//! `WorkflowRuntime`; this type is not a scheduler.

use crate::app::managed_process::{
    DiscoveryOperation, TransientCounts, TransientCurrentItem, TransientError,
    TransientItemOutcomeSnapshot, TransientItemPhase, TransientItemState, TransientProgress,
    TransientRunControl, TransientRunSnapshot, TransientRunState, TransientRuntimeError,
    TransientWorkItem, EVENT_NAME,
};
use crate::providers::youtube::error::YouTubeError;
use crate::providers::youtube::executor::{YouTubeExecutor, YouTubeExecutorContext};
use crate::providers::youtube::models::{StartYouTubeDownloadRequest, YouTubeStartReceipt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub type EventSink = Arc<dyn Fn(&TransientRunSnapshot) + Send + Sync + 'static>;

const MAX_SUBMISSION_LEDGER: usize = 1024;

#[derive(Clone, Debug)]
struct SubmissionLedgerEntry {
    fingerprint: String,
    receipt: YouTubeStartReceipt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YoutubeDurableRequest {
    pub schema_version: u32,
    pub start: StartYouTubeDownloadRequest,
    pub work_items: Vec<TransientWorkItem>,
    pub context: YouTubeExecutorContext,
    pub plan_fingerprint: String,
    pub client_submission_id: String,
}

struct DiscoverySlot {
    operation_id: String,
    cancel: Arc<AtomicBool>,
}

struct ActiveDownload {
    run_id: String,
    expected_revision: u64,
    control: Arc<TransientRunControl>,
    snapshot: TransientRunSnapshot,
}

pub struct YouTubeLiveHandle {
    used_operations: Mutex<HashSet<String>>,
    discovery: Mutex<Option<DiscoverySlot>>,
    active: Mutex<Option<ActiveDownload>>,
    most_recent: Mutex<Option<TransientRunSnapshot>>,
    sink: Mutex<Option<EventSink>>,
    shutting_down: AtomicBool,
    submissions: Mutex<HashMap<String, SubmissionLedgerEntry>>,
    submission_order: Mutex<VecDeque<String>>,
}

impl Default for YouTubeLiveHandle {
    fn default() -> Self {
        Self {
            used_operations: Mutex::new(HashSet::new()),
            discovery: Mutex::new(None),
            active: Mutex::new(None),
            most_recent: Mutex::new(None),
            sink: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            submissions: Mutex::new(HashMap::new()),
            submission_order: Mutex::new(VecDeque::new()),
        }
    }
}

impl YouTubeLiveHandle {
    pub fn bind_app(&self, app: AppHandle) {
        let sink: EventSink = Arc::new(move |snapshot: &TransientRunSnapshot| {
            let _ = app.emit(EVENT_NAME, snapshot);
        });
        *self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    #[cfg(test)]
    pub(crate) fn set_test_sink(&self, sink: EventSink) {
        *self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    /// Process-lifetime submission ledger lookup (ADR / FR: capped, no eviction).
    pub fn lookup_submission(
        &self,
        client_submission_id: &str,
        fingerprint: &str,
    ) -> Result<Option<YouTubeStartReceipt>, TransientRuntimeError> {
        let submissions = self
            .submissions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match submissions.get(client_submission_id) {
            None => Ok(None),
            Some(entry) if entry.fingerprint == fingerprint => Ok(Some(entry.receipt.clone())),
            Some(_) => Err(TransientRuntimeError::SubmissionConflict),
        }
    }

    pub fn ensure_submission_capacity(
        &self,
        client_submission_id: &str,
    ) -> Result<(), TransientRuntimeError> {
        let submissions = self
            .submissions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if submissions.contains_key(client_submission_id) {
            return Ok(());
        }
        if submissions.len() >= MAX_SUBMISSION_LEDGER {
            return Err(TransientRuntimeError::SubmissionCapacity);
        }
        Ok(())
    }

    pub fn record_submission(
        &self,
        client_submission_id: String,
        fingerprint: String,
        receipt: YouTubeStartReceipt,
    ) -> Result<(), TransientRuntimeError> {
        let mut submissions = self
            .submissions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut order = self
            .submission_order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = submissions.get(&client_submission_id) {
            if existing.fingerprint != fingerprint {
                return Err(TransientRuntimeError::SubmissionConflict);
            }
            // Identical replay already handled by lookup; keep first receipt.
            return Ok(());
        }
        if submissions.len() >= MAX_SUBMISSION_LEDGER {
            return Err(TransientRuntimeError::SubmissionCapacity);
        }
        order.push_back(client_submission_id.clone());
        submissions.insert(
            client_submission_id,
            SubmissionLedgerEntry {
                fingerprint,
                receipt,
            },
        );
        Ok(())
    }

    pub fn begin_discovery(
        &self,
        operation_id: String,
    ) -> Result<DiscoveryGuard<'_>, TransientRuntimeError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(TransientRuntimeError::ShuttingDown);
        }
        let mut used = self
            .used_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !used.insert(operation_id.clone()) {
            return Err(TransientRuntimeError::OperationIdReused);
        }
        let mut discovery = self
            .discovery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if discovery.is_some()
            || self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some()
        {
            used.remove(&operation_id);
            return Err(TransientRuntimeError::Busy);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *discovery = Some(DiscoverySlot {
            operation_id: operation_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        Ok(DiscoveryGuard {
            live: self,
            operation: DiscoveryOperation::with_cancel(cancel),
            operation_id,
        })
    }

    pub fn cancel_discovery(&self, operation_id: &str) -> Result<(), TransientRuntimeError> {
        let discovery = self
            .discovery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(slot) = discovery.as_ref() else {
            return Err(TransientRuntimeError::DiscoveryNotFound);
        };
        if slot.operation_id != operation_id {
            return Err(TransientRuntimeError::DiscoveryNotFound);
        }
        slot.cancel.store(true, Ordering::Release);
        Ok(())
    }

    pub fn attach_run(
        &self,
        run_id: String,
        client_submission_id: String,
        plan_fingerprint: String,
        work_items: &[TransientWorkItem],
    ) -> Result<Arc<TransientRunControl>, TransientRuntimeError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(TransientRuntimeError::ShuttingDown);
        }
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.is_some() {
            return Err(TransientRuntimeError::RunAlreadyActive);
        }
        let control = Arc::new(TransientRunControl::default());
        let snapshot = initial_snapshot(
            run_id.clone(),
            client_submission_id,
            plan_fingerprint,
            work_items,
        );
        *active = Some(ActiveDownload {
            run_id,
            expected_revision: 1,
            control: Arc::clone(&control),
            snapshot,
        });
        if let Some(current) = active.as_ref() {
            self.emit(&current.snapshot);
        }
        Ok(control)
    }

    /// Clear the live slot for `run_id` after a failed durable submit (admit rollback).
    pub fn clear_run(&self, run_id: &str) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|current| current.run_id == run_id)
        {
            *active = None;
        }
    }

    pub fn has_active_run(&self, run_id: &str) -> bool {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active
            .as_ref()
            .is_some_and(|current| current.run_id == run_id)
    }

    pub fn snapshot(
        &self,
        run_id: Option<&str>,
    ) -> Result<Option<TransientRunSnapshot>, TransientRuntimeError> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(current) = active.as_ref() {
            return match run_id {
                None => Ok(Some(current.snapshot.clone())),
                Some(wanted) if current.run_id == wanted => Ok(Some(current.snapshot.clone())),
                Some(_) => Err(TransientRuntimeError::RunNotFound),
            };
        }
        let recent = self
            .most_recent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match (run_id, recent.as_ref()) {
            (None, snapshot) => Ok(snapshot.cloned()),
            (Some(wanted), Some(snapshot)) if snapshot.run_id == wanted => {
                Ok(Some(snapshot.clone()))
            }
            (Some(_), Some(_)) => Err(TransientRuntimeError::RunNotFound),
            (Some(_), None) => Ok(None),
        }
    }

    pub fn pause(
        &self,
        run_id: &str,
        expected_revision: u64,
    ) -> Result<TransientRunSnapshot, TransientRuntimeError> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = active.as_mut().ok_or(TransientRuntimeError::RunNotFound)?;
        if current.run_id != run_id {
            return Err(TransientRuntimeError::RunNotFound);
        }
        if current.expected_revision != expected_revision {
            return Err(TransientRuntimeError::StaleRevision);
        }
        if current.snapshot.state.is_terminal() {
            return Err(TransientRuntimeError::InvalidTransition);
        }
        current.control.request_pause();
        current.snapshot.state = TransientRunState::PauseRequested;
        current.snapshot.revision += 1;
        current.expected_revision = current.snapshot.revision;
        let snapshot = current.snapshot.clone();
        self.emit(&snapshot);
        Ok(snapshot)
    }

    pub fn resume(
        &self,
        run_id: &str,
        expected_revision: u64,
    ) -> Result<TransientRunSnapshot, TransientRuntimeError> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = active.as_mut().ok_or(TransientRuntimeError::RunNotFound)?;
        if current.run_id != run_id {
            return Err(TransientRuntimeError::RunNotFound);
        }
        if current.expected_revision != expected_revision {
            return Err(TransientRuntimeError::StaleRevision);
        }
        if current.snapshot.state.is_terminal() {
            return Err(TransientRuntimeError::InvalidTransition);
        }
        current.control.withdraw_pause();
        current.snapshot.state = TransientRunState::Running;
        current.snapshot.revision += 1;
        current.expected_revision = current.snapshot.revision;
        let snapshot = current.snapshot.clone();
        self.emit(&snapshot);
        Ok(snapshot)
    }

    pub fn cancel(&self, run_id: &str) -> Result<TransientRunSnapshot, TransientRuntimeError> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = active.as_mut().ok_or(TransientRuntimeError::RunNotFound)?;
        if current.run_id != run_id {
            return Err(TransientRuntimeError::RunNotFound);
        }
        current.control.request_cancel();
        if !current.snapshot.state.is_terminal() {
            current.snapshot.state = TransientRunState::Cancelling;
            current.snapshot.revision += 1;
            current.expected_revision = current.snapshot.revision;
        }
        let snapshot = current.snapshot.clone();
        self.emit(&snapshot);
        Ok(snapshot)
    }

    pub fn control_for(&self, run_id: &str) -> Option<Arc<TransientRunControl>> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active
            .as_ref()
            .and_then(|current| (current.run_id == run_id).then(|| Arc::clone(&current.control)))
    }

    pub fn publish_snapshot(&self, snapshot: TransientRunSnapshot) {
        let terminal = snapshot.state.is_terminal();
        let control_to_retry = {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let control = if terminal {
                active.as_ref().and_then(|current| {
                    (current.run_id == snapshot.run_id).then(|| Arc::clone(&current.control))
                })
            } else {
                None
            };
            if let Some(current) = active.as_mut() {
                if current.run_id == snapshot.run_id {
                    current.expected_revision = snapshot.revision;
                    current.snapshot = snapshot.clone();
                }
            }
            if terminal {
                *active = None;
            }
            control
        };
        if let Some(control) = control_to_retry {
            let _ = control.retry_cleanup_verifiers();
        }
        *self
            .most_recent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot.clone());
        self.emit(&snapshot);
    }

    fn clear_discovery(&self, operation_id: &str) {
        let mut discovery = self
            .discovery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if discovery
            .as_ref()
            .is_some_and(|slot| slot.operation_id == operation_id)
        {
            *discovery = None;
        }
    }

    fn emit(&self, snapshot: &TransientRunSnapshot) {
        if let Some(sink) = self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            sink(snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::youtube::models::YouTubeStartReceiptState;

    fn sample_receipt(submission_id: &str, run_id: &str) -> YouTubeStartReceipt {
        YouTubeStartReceipt {
            client_submission_id: submission_id.to_string(),
            run_id: run_id.to_string(),
            revision: 1,
            scan_plan_id: "plan-1".to_string(),
            plan_fingerprint: "plan-fp".to_string(),
            state: YouTubeStartReceiptState::Running,
        }
    }

    #[test]
    fn submission_ledger_first_start_is_not_replayed_lookup() {
        let live = YouTubeLiveHandle::default();
        assert!(live
            .lookup_submission("sub-first", "fp-1")
            .unwrap()
            .is_none());
        live.record_submission(
            "sub-first".to_string(),
            "fp-1".to_string(),
            sample_receipt("sub-first", "run-1"),
        )
        .unwrap();
        let replay = live.lookup_submission("sub-first", "fp-1").unwrap();
        assert_eq!(replay.unwrap().run_id, "run-1");
    }

    #[test]
    fn submission_ledger_identical_replay_returns_receipt() {
        let live = YouTubeLiveHandle::default();
        let receipt = sample_receipt("sub-replay", "run-2");
        live.record_submission(
            "sub-replay".to_string(),
            "fp-same".to_string(),
            receipt.clone(),
        )
        .unwrap();
        let again = live
            .lookup_submission("sub-replay", "fp-same")
            .unwrap()
            .expect("replay receipt");
        assert_eq!(again, receipt);
    }

    #[test]
    fn submission_ledger_fingerprint_mismatch_is_conflict() {
        let live = YouTubeLiveHandle::default();
        live.record_submission(
            "sub-conflict".to_string(),
            "fp-a".to_string(),
            sample_receipt("sub-conflict", "run-3"),
        )
        .unwrap();
        let error = live
            .lookup_submission("sub-conflict", "fp-b")
            .expect_err("mismatch must conflict");
        assert_eq!(error, TransientRuntimeError::SubmissionConflict);
    }

    #[test]
    fn submission_ledger_caps_at_1024_without_eviction() {
        let live = YouTubeLiveHandle::default();
        for index in 0..MAX_SUBMISSION_LEDGER {
            live.record_submission(
                format!("sub-{index}"),
                format!("fp-{index}"),
                sample_receipt(&format!("sub-{index}"), &format!("run-{index}")),
            )
            .unwrap();
        }
        let error = live
            .ensure_submission_capacity("sub-overflow")
            .expect_err("capacity must exhaust");
        assert_eq!(error, TransientRuntimeError::SubmissionCapacity);
        // No eviction: first entry still present.
        assert!(live.lookup_submission("sub-0", "fp-0").unwrap().is_some());
    }

    #[test]
    fn admit_under_drain_lock_attaches_then_submits() {
        use crate::app::database::initialize_database;
        use crate::app::database_diagnostics::DatabaseDiagnostics;
        use crate::app::database_writer::DatabaseWriter;
        use crate::workflow::WorkflowRuntime;
        use tempfile::tempdir;

        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let writer = DatabaseWriter::start(db_path, DatabaseDiagnostics::default()).unwrap();
        let runtime = WorkflowRuntime::new(writer);
        let live = YouTubeLiveHandle::default();
        let run_id = "yt-admit-1".to_string();
        let items = vec![TransientWorkItem {
            occurrence_id: "occ-1".to_string(),
            artifact_fingerprint: "af-1".to_string(),
            video_id: "vid-1".to_string(),
            ordinal: 1,
            title: "t".to_string(),
            source_url: "https://www.youtube.com/watch?v=vid-1".to_string(),
        }];
        runtime.with_drain_lock(|| {
            live.attach_run(
                run_id.clone(),
                "sub-admit-1".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .unwrap();
            runtime
                .submit_youtube_download(
                    run_id.clone(),
                    "vid-1".to_string(),
                    "{}".to_string(),
                    ".".to_string(),
                    90,
                )
                .unwrap();
        });
        assert!(live.has_active_run(&run_id));
        let run = runtime.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, crate::workflow::domain::state::RunState::Queued);
    }

    #[test]
    fn submit_failure_clears_live_and_leaves_no_active_slot() {
        use crate::app::database::initialize_database;
        use crate::app::database_diagnostics::DatabaseDiagnostics;
        use crate::app::database_writer::DatabaseWriter;
        use crate::workflow::WorkflowRuntime;
        use tempfile::tempdir;

        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let writer = DatabaseWriter::start(db_path, DatabaseDiagnostics::default()).unwrap();
        let runtime = WorkflowRuntime::new(writer);
        let live = YouTubeLiveHandle::default();
        let run_id = "yt-admit-dup".to_string();
        runtime
            .submit_youtube_download(
                run_id.clone(),
                "vid-1".to_string(),
                "{}".to_string(),
                ".".to_string(),
                91,
            )
            .unwrap();
        let items = vec![TransientWorkItem {
            occurrence_id: "occ-1".to_string(),
            artifact_fingerprint: "af-1".to_string(),
            video_id: "vid-1".to_string(),
            ordinal: 1,
            title: "t".to_string(),
            source_url: "https://www.youtube.com/watch?v=vid-1".to_string(),
        }];
        let submit_error = runtime.with_drain_lock(|| {
            live.attach_run(
                run_id.clone(),
                "sub-dup".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .unwrap();
            let error = runtime
                .submit_youtube_download(
                    run_id.clone(),
                    "vid-1".to_string(),
                    "{}".to_string(),
                    ".".to_string(),
                    92,
                )
                .unwrap_err();
            live.clear_run(&run_id);
            error
        });
        assert!(!live.has_active_run(&run_id));
        assert!(!submit_error.to_string().is_empty());
    }
}

pub struct DiscoveryGuard<'a> {
    live: &'a YouTubeLiveHandle,
    pub operation: DiscoveryOperation,
    operation_id: String,
}

impl Drop for DiscoveryGuard<'_> {
    fn drop(&mut self) {
        let _ = self.operation.retry_cleanup_verifiers();
        self.live.clear_discovery(&self.operation_id);
    }
}

pub fn initial_snapshot(
    run_id: String,
    client_submission_id: String,
    plan_fingerprint: String,
    work_items: &[TransientWorkItem],
) -> TransientRunSnapshot {
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
    TransientRunSnapshot {
        schema_version: 1,
        run_id,
        revision: 1,
        state: TransientRunState::Running,
        item: work_items.first().map(|item| TransientCurrentItem {
            occurrence_id: item.occurrence_id.clone(),
            artifact_fingerprint: item.artifact_fingerprint.clone(),
            video_id: item.video_id.clone(),
            ordinal: item.ordinal,
            title: item.title.clone(),
            state: TransientItemState::Pending,
            phase: TransientItemPhase::Waiting,
        }),
        progress: TransientProgress::default(),
        counts: TransientCounts {
            completed: 0,
            completed_with_warnings: 0,
            selected: work_items.len() as u32,
            failed: 0,
            skipped: 0,
            cancelled: 0,
        },
        warnings: Vec::new(),
        error: None,
        client_submission_id,
        plan_fingerprint,
        items,
    }
}

pub fn apply_item_progress(
    snapshot: &mut TransientRunSnapshot,
    item: &TransientWorkItem,
    phase: TransientItemPhase,
    bytes_completed: Option<u64>,
    bytes_total: Option<u64>,
    fraction: Option<f64>,
) {
    snapshot.revision += 1;
    snapshot.item = Some(TransientCurrentItem {
        occurrence_id: item.occurrence_id.clone(),
        artifact_fingerprint: item.artifact_fingerprint.clone(),
        video_id: item.video_id.clone(),
        ordinal: item.ordinal,
        title: item.title.clone(),
        state: TransientItemState::Running,
        phase: phase.clone(),
    });
    snapshot.progress = TransientProgress {
        bytes_completed,
        bytes_total,
        fraction,
    };
    if let Some(entry) = snapshot
        .items
        .iter_mut()
        .find(|entry| entry.occurrence_id == item.occurrence_id)
    {
        entry.state = TransientItemState::Running;
        entry.phase = phase;
    }
}

pub fn apply_item_outcome(
    snapshot: &mut TransientRunSnapshot,
    item: &TransientWorkItem,
    outcome: &crate::app::managed_process::TransientExecutionOutcome,
) {
    snapshot.revision += 1;
    if let Some(entry) = snapshot
        .items
        .iter_mut()
        .find(|entry| entry.occurrence_id == item.occurrence_id)
    {
        entry.state = outcome.state.clone();
        entry.phase = outcome.phase.clone();
        entry.warnings = outcome
            .warnings
            .iter()
            .map(|code| crate::app::managed_process::TransientWarning {
                occurrence_id: Some(item.occurrence_id.clone()),
                code: code.clone(),
                message: code.clone(),
            })
            .collect();
        entry.error = outcome.error.clone();
        entry.published_artifact_kinds = outcome.published_artifact_kinds.clone();
    }
    snapshot.counts.completed = snapshot
        .items
        .iter()
        .filter(|entry| entry.state == TransientItemState::Completed)
        .count() as u32;
    snapshot.counts.completed_with_warnings = snapshot
        .items
        .iter()
        .filter(|entry| entry.state == TransientItemState::CompletedWithWarnings)
        .count() as u32;
    snapshot.counts.failed = snapshot
        .items
        .iter()
        .filter(|entry| entry.state == TransientItemState::Failed)
        .count() as u32;
    snapshot.counts.skipped = snapshot
        .items
        .iter()
        .filter(|entry| {
            matches!(
                entry.state,
                TransientItemState::Skipped | TransientItemState::SkippedExisting
            )
        })
        .count() as u32;
    snapshot.counts.cancelled = snapshot
        .items
        .iter()
        .filter(|entry| entry.state == TransientItemState::Cancelled)
        .count() as u32;
    snapshot.item = Some(TransientCurrentItem {
        occurrence_id: item.occurrence_id.clone(),
        artifact_fingerprint: item.artifact_fingerprint.clone(),
        video_id: item.video_id.clone(),
        ordinal: item.ordinal,
        title: item.title.clone(),
        state: outcome.state.clone(),
        phase: outcome.phase.clone(),
    });
}

pub fn finish_snapshot(
    snapshot: &mut TransientRunSnapshot,
    cancelled: bool,
    error: Option<TransientError>,
) {
    snapshot.revision += 1;
    snapshot.error = error.clone();
    snapshot.state = if cancelled {
        TransientRunState::Cancelled
    } else if error.is_some() {
        TransientRunState::Failed
    } else if snapshot.counts.completed_with_warnings > 0 || !snapshot.warnings.is_empty() {
        TransientRunState::CompletedWithWarnings
    } else {
        TransientRunState::Completed
    };
    if cancelled {
        for entry in &mut snapshot.items {
            if matches!(
                entry.state,
                TransientItemState::Pending | TransientItemState::Running
            ) {
                entry.state = TransientItemState::Cancelled;
                entry.phase = TransientItemPhase::Cancelled;
            }
        }
        snapshot.counts.cancelled = snapshot
            .items
            .iter()
            .filter(|entry| entry.state == TransientItemState::Cancelled)
            .count() as u32;
    }
}

pub fn executor_from_request(
    output_root: crate::app::safe_output_filesystem::ValidatedOutputRoot,
    request: &YoutubeDurableRequest,
) -> Result<Arc<YouTubeExecutor>, YouTubeError> {
    YouTubeExecutor::new_with_context(output_root, &request.start, request.context.clone())
}
