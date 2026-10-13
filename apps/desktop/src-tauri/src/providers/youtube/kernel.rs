//! Durable-kernel adapter around the provider-owned YouTube item executor.

use crate::app::managed_process::{
    TransientError, TransientExecutor, TransientItemState, TransientRunControl,
    TransientRunSnapshot,
};
use crate::app::safe_output_filesystem::validate_output_root;
use crate::providers::youtube::resume::{discard_run_staging, pause_reason, ChangeNotifier};
use crate::providers::youtube::live::{
    apply_item_failure, apply_item_outcome, executor_from_request, finish_snapshot,
    YouTubeLiveHandle, YoutubeDurableRequest,
};
use crate::workflow::domain::types::{RunRecord, StepRecord};
use crate::workflow::ports::executor::{ExecutorOutcome, StepExecutor};
use crate::workflow::WorkflowRuntime;
use std::path::PathBuf;
use std::sync::Arc;

pub struct YoutubeDownloadExecutor {
    pub live: Arc<YouTubeLiveHandle>,
    /// Durable resume bookkeeping. `None` disables resume behaviour (tests).
    pub resume: Option<Arc<dyn ResumeLedger>>,
    /// Tells the UI the interrupted-runs list changed.
    pub notify_resume_changed: Option<ChangeNotifier>,
}

/// What the kernel needs from the durable resume record, kept as a port so
/// the item loop can be tested without a database.
pub trait ResumeLedger: Send + Sync {
    /// The run's current attempt was admitted by a resume (automatic or
    /// Continue). Such an attempt pauses on failure instead of failing.
    fn is_resumed_attempt(&self, run_id: &str) -> bool;

    /// Remember the size of the item being downloaded for the disk check of a
    /// later resume.
    fn record_expected_bytes(&self, run_id: &str, item_key: &str, bytes: u64);

    /// Durably pause the running run for the user. Called before the live
    /// snapshot is published, so the UI never shows a pause that is not
    /// committed.
    fn pause_for_user(&self, run_id: &str, reason: &str, code: &str) -> bool;
}

impl ResumeLedger for WorkflowRuntime {
    fn is_resumed_attempt(&self, run_id: &str) -> bool {
        self.get_resume(run_id.to_string())
            .ok()
            .flatten()
            .is_some_and(|record| {
                record.state == crate::workflow::domain::resume::ResumeState::Resuming
                    && record.auto_resume_used
            })
    }

    fn pause_for_user(&self, run_id: &str, reason: &str, code: &str) -> bool {
        self.pause_running_for_user(run_id.to_string(), reason.to_string(), code.to_string())
            .unwrap_or(false)
    }

    fn record_expected_bytes(&self, run_id: &str, item_key: &str, bytes: u64) {
        let _ = WorkflowRuntime::record_expected_bytes(
            self,
            run_id.to_string(),
            item_key.to_string(),
            bytes,
            chrono::Utc::now().timestamp(),
        );
    }
}

impl StepExecutor for YoutubeDownloadExecutor {
    fn workflow_type(&self) -> &'static str {
        "youtube_download"
    }

    fn execute(&self, run: &RunRecord, _step: &StepRecord) -> ExecutorOutcome {
        // Fail closed: never invent a default TransientRunControl. Missing live
        // control is a control-plane invariant breach, not user cancellation.
        let Some(control) = self.live.control_for(&run.id) else {
            return ExecutorOutcome::failed(format!(
                "missing live control for YouTube run {}",
                run.id
            ));
        };
        let resumed = self
            .resume
            .as_ref()
            .is_some_and(|ledger| ledger.is_resumed_attempt(&run.id));
        let request = match serde_json::from_str::<YoutubeDurableRequest>(&run.request_json) {
            Ok(request) => request,
            Err(error) => {
                return self.fail_before_start(
                    &run.id,
                    "REQUEST_INVALID",
                    error.to_string(),
                    resumed,
                )
            }
        };
        let output_root = match validate_output_root(PathBuf::from(&run.output_root).as_path()) {
            Ok(root) => root,
            Err(error) => {
                return self.fail_before_start(
                    &run.id,
                    "OUTPUT_ROOT_INVALID",
                    error.to_string(),
                    resumed,
                )
            }
        };
        let staging_root = output_root.clone();
        let executor = match executor_from_request(output_root, &request) {
            Ok(executor) => executor,
            Err(error) => {
                return self.fail_before_start(&run.id, &error.code, error.message, resumed)
            }
        };
        let outcome =
            self.run_items_with(&run.id, &request, &control, executor.as_ref(), resumed);
        if !outcome.succeeded && !outcome.pause_for_user {
            // Cancelled or failed for good: nothing will resume from the
            // partial files, so remove them now.
            discard_run_staging(&staging_root, &request.work_items);
        }
        outcome
    }
}

impl YoutubeDownloadExecutor {
    /// Fail the run before any item started.
    ///
    /// The live slot is only released by a terminal publish, so a bare
    /// `ExecutorOutcome::failed` would leave the run "active" and make every
    /// later start fail with `RunAlreadyActive`.
    fn fail_before_start(
        &self,
        run_id: &str,
        code: &str,
        message: String,
        resumed: bool,
    ) -> ExecutorOutcome {
        let error = TransientError {
            code: code.to_string(),
            message,
        };
        let failure = format!("{}: {}", error.code, error.message);
        let mut snapshot = match self.live.snapshot(Some(run_id)) {
            Ok(Some(snapshot)) => snapshot,
            _ => crate::providers::youtube::live::initial_snapshot(
                run_id.to_string(),
                String::new(),
                String::new(),
                &[],
            ),
        };
        if resumed {
            // A resumed attempt that cannot even start (folder gone, drive
            // unplugged) goes back to the user; it is never a terminal failure.
            return self.pause_for_user(run_id, &mut snapshot, &error);
        }
        self.commit(run_id, &mut snapshot, |live| {
            finish_snapshot(live, false, Some(error.clone()))
        });
        ExecutorOutcome::failed(failure)
    }

    /// Apply `change` to the live snapshot in place, so a pause or cancel that
    /// lands meanwhile is kept. If the run no longer owns the live slot the
    /// change is applied to `fallback` and published instead. Returns the
    /// resulting snapshot.
    fn commit(
        &self,
        run_id: &str,
        fallback: &mut TransientRunSnapshot,
        mut change: impl FnMut(&mut TransientRunSnapshot),
    ) -> TransientRunSnapshot {
        if let Some(committed) = self.live.update_snapshot(run_id, &mut change) {
            *fallback = committed.clone();
            return committed;
        }
        change(fallback);
        self.live.publish_snapshot(fallback.clone());
        fallback.clone()
    }

    /// Drive every work item through `executor`, publishing snapshots as it goes.
    #[cfg(test)]
    fn run_items(
        &self,
        run_id: &str,
        request: &YoutubeDurableRequest,
        control: &TransientRunControl,
        executor: &dyn TransientExecutor,
    ) -> ExecutorOutcome {
        self.run_items_with(run_id, request, control, executor, false)
    }

    /// End a resumed attempt that failed: the live view becomes a paused
    /// snapshot carrying the reason (no item is marked failed) and the run is
    /// handed back to the user instead of failing.
    fn pause_for_user(
        &self,
        run_id: &str,
        snapshot: &mut TransientRunSnapshot,
        error: &TransientError,
    ) -> ExecutorOutcome {
        let reason = pause_reason(&error.code, &error.message);
        let shown = TransientError {
            code: error.code.clone(),
            message: reason.clone(),
        };
        // Commit first, publish second: the panel must never show a pause
        // the database does not have yet.
        if let Some(ledger) = &self.resume {
            ledger.pause_for_user(run_id, &reason, &error.code);
        }
        if self.live.suspend_run(run_id, Some(shown.clone())).is_none() {
            snapshot.state = crate::app::managed_process::TransientRunState::Paused;
            snapshot.error = Some(shown);
            snapshot.revision += 1;
            self.live.publish_snapshot(snapshot.clone());
        }
        if let Some(notify) = &self.notify_resume_changed {
            notify();
        }
        ExecutorOutcome::paused_for_user(reason, Some(error.code.clone()))
    }

    /// Drive every work item through `executor`. When `resumed`, a failure
    /// pauses the run for the user instead of failing it.
    fn run_items_with(
        &self,
        run_id: &str,
        request: &YoutubeDurableRequest,
        control: &TransientRunControl,
        executor: &dyn TransientExecutor,
        resumed: bool,
    ) -> ExecutorOutcome {
        let mut snapshot = match self.live.snapshot(Some(run_id)) {
            Ok(Some(snapshot)) => snapshot,
            _ => crate::providers::youtube::live::initial_snapshot(
                run_id.to_string(),
                request.client_submission_id.clone(),
                request.plan_fingerprint.clone(),
                &request.work_items,
            ),
        };

        for item in &request.work_items {
            if control.is_cancelled() {
                self.commit(run_id, &mut snapshot, |live| {
                    finish_snapshot(live, true, None)
                });
                return ExecutorOutcome::cancelled("YouTube download was cancelled".to_string());
            }
            if control.pause_requested() {
                let _ = control.mark_paused_if_requested();
                control.wait_for_resume();
            }
            if control.is_cancelled() {
                self.commit(run_id, &mut snapshot, |live| {
                    finish_snapshot(live, true, None)
                });
                return ExecutorOutcome::cancelled("YouTube download was cancelled".to_string());
            }

            // Progress ticks, outcomes and failures all change the live
            // snapshot in place, so a pause or cancel requested meanwhile is
            // never overwritten by a copy this loop read earlier.
            let mut last_total = 0u64;
            let outcome = TransientExecutor::execute(executor, item, control, &mut |update| {
                if let (Some(ledger), Some(total)) = (&self.resume, update.bytes_total) {
                    if total > 0 && total != last_total {
                        last_total = total;
                        ledger.record_expected_bytes(run_id, &item.occurrence_id, total);
                    }
                }
                self.live.publish_item_progress(run_id, item, update);
            });

            match outcome {
                Ok(outcome) => {
                    self.commit(run_id, &mut snapshot, |live| {
                        apply_item_outcome(live, item, &outcome)
                    });
                    if matches!(
                        outcome.state,
                        TransientItemState::Failed | TransientItemState::Cancelled
                    ) && outcome.error.is_some()
                    {
                        let message = outcome
                            .error
                            .as_ref()
                            .map(|error| error.message.clone())
                            .unwrap_or_else(|| "YouTube item failed".to_string());
                        let cancelled = matches!(outcome.state, TransientItemState::Cancelled);
                        if resumed && !cancelled && !control.is_cancelled() {
                            if let Some(error) = outcome.error.as_ref() {
                                return self.pause_for_user(run_id, &mut snapshot, error);
                            }
                        }
                        self.commit(run_id, &mut snapshot, |live| {
                            finish_snapshot(live, cancelled, outcome.error.clone())
                        });
                        if control.is_cancelled() {
                            return ExecutorOutcome::cancelled(message);
                        }
                        return ExecutorOutcome::failed(message);
                    }
                }
                Err(TransientError { code, message }) => {
                    let error = TransientError {
                        code: code.clone(),
                        message: message.clone(),
                    };
                    // A cancelled run marks in-flight items Cancelled in
                    // `finish_snapshot`; only a real failure fails the item.
                    let cancelled = control.is_cancelled();
                    if resumed && !cancelled {
                        return self.pause_for_user(run_id, &mut snapshot, &error);
                    }
                    self.commit(run_id, &mut snapshot, |live| {
                        if !cancelled {
                            apply_item_failure(live, item, &error);
                        }
                        finish_snapshot(live, cancelled, Some(error.clone()));
                    });
                    if control.is_cancelled() {
                        return ExecutorOutcome::cancelled(message);
                    }
                    return ExecutorOutcome::failed(format!("{code}: {message}"));
                }
            }
        }

        let finished = self.commit(run_id, &mut snapshot, |live| {
            finish_snapshot(live, false, None)
        });
        let warning = finished.counts.completed_with_warnings > 0;
        let payload = serde_json::to_string(&finished).unwrap_or_else(|_| "{}".to_string());
        if warning {
            let mut outcome = ExecutorOutcome::succeeded(payload);
            outcome.warning = true;
            outcome
        } else {
            ExecutorOutcome::succeeded(payload)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::managed_process::{
        TransientExecutionOutcome, TransientItemPhase, TransientProgressUpdate,
        TransientRunSnapshot, TransientRunState, TransientWorkItem,
    };
    use crate::providers::youtube::models::{StartYouTubeDownloadRequest, YouTubeDownloadMode};
    use crate::workflow::domain::state::{RunState, StepState};
    use crate::workflow::domain::types::{StepType, WorkflowType};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    fn work_item(ordinal: u32) -> TransientWorkItem {
        TransientWorkItem {
            occurrence_id: format!("occ-{ordinal}"),
            artifact_fingerprint: format!("af-{ordinal}"),
            video_id: format!("vid-{ordinal}"),
            ordinal,
            title: format!("Video {ordinal}"),
            source_url: format!("https://www.youtube.com/watch?v=vid-{ordinal}"),
        }
    }

    fn durable_request(items: Vec<TransientWorkItem>) -> YoutubeDurableRequest {
        YoutubeDurableRequest {
            schema_version: 1,
            start: StartYouTubeDownloadRequest {
                client_submission_id: "sub-kernel".to_string(),
                scan_plan_id: "plan-kernel".to_string(),
                selected_occurrence_ids: items.iter().map(|i| i.occurrence_id.clone()).collect(),
                output_dir: ".".to_string(),
                mode: YouTubeDownloadMode::VideoAndTranscript,
                max_height: None,
                preferred_language: None,
                fallback_languages: Vec::new(),
                allow_automatic_captions: false,
                continue_without_transcript: false,
            },
            work_items: items,
            context: Default::default(),
            plan_fingerprint: "plan-fp".to_string(),
            client_submission_id: "sub-kernel".to_string(),
        }
    }

    fn run_record(id: &str, request_json: String) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            workflow_type: WorkflowType::youtube_download(),
            provider: "youtube".to_string(),
            state: RunState::Running,
            legacy_origin: None,
            legacy_id: None,
            request_json,
            output_root: ".".to_string(),
            error_message: None,
            created_at: 1,
            updated_at: 1,
            completed_at: None,
        }
    }

    fn step_record(run_id: &str) -> StepRecord {
        StepRecord {
            id: format!("{run_id}-step"),
            run_id: run_id.to_string(),
            step_key: "video".to_string(),
            step_type: StepType::youtube_execute(),
            state: StepState::Running,
            attempt: 1,
            error_message: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn capture_sink(live: &YouTubeLiveHandle) -> Arc<Mutex<Vec<TransientRunSnapshot>>> {
        let published = Arc::new(Mutex::new(Vec::new()));
        let sink_target = Arc::clone(&published);
        live.set_test_sink(Arc::new(move |snapshot| {
            if let Ok(mut guard) = sink_target.lock() {
                guard.push(snapshot.clone());
            }
        }));
        published
    }

    /// Fake item executor: completes the first item, then fails the others.
    struct ScriptedExecutor;

    impl TransientExecutor for ScriptedExecutor {
        fn execute(
            &self,
            item: &TransientWorkItem,
            _control: &TransientRunControl,
            progress: &mut dyn FnMut(TransientProgressUpdate),
        ) -> Result<TransientExecutionOutcome, TransientError> {
            progress(TransientProgressUpdate {
                occurrence_id: item.occurrence_id.clone(),
                phase: TransientItemPhase::Media,
                bytes_completed: None,
                bytes_total: None,
                fraction: Some(0.0),
            });
            if item.ordinal == 1 {
                return Ok(TransientExecutionOutcome::completed(vec![
                    "video".to_string()
                ]));
            }
            Err(TransientError {
                code: "TRANSCRIPT_INVALID".to_string(),
                message: "caption file was malformed".to_string(),
            })
        }
    }

    #[test]
    fn executor_error_marks_the_running_item_failed_in_the_terminal_snapshot() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = capture_sink(&live);
        let items = vec![work_item(1), work_item(2), work_item(3)];
        let control = live
            .attach_run(
                "yt-err-run".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };

        let outcome = kernel.run_items(
            "yt-err-run",
            &durable_request(items),
            &control,
            &ScriptedExecutor,
        );

        assert!(!outcome.succeeded);
        let last = published
            .lock()
            .expect("published lock")
            .last()
            .cloned()
            .expect("terminal snapshot published");
        assert_eq!(last.state, TransientRunState::Failed);
        assert_eq!(last.items[0].state, TransientItemState::Completed);
        let failed = &last.items[1];
        assert_eq!(failed.state, TransientItemState::Failed);
        assert_eq!(failed.phase, TransientItemPhase::Failed);
        let error = failed.error.as_ref().expect("item carries the error");
        assert_eq!(error.code, "TRANSCRIPT_INVALID");
        assert_eq!(error.message, "caption file was malformed");
        assert_eq!(last.items[2].state, TransientItemState::Pending);
        assert_eq!(last.counts.completed, 1);
        assert_eq!(last.counts.failed, 1);
        assert_eq!(
            last.error.as_ref().map(|e| e.code.as_str()),
            Some("TRANSCRIPT_INVALID")
        );
    }

    #[test]
    fn pre_loop_failure_publishes_terminal_snapshot_and_frees_the_active_slot() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = capture_sink(&live);
        let items = vec![work_item(1)];
        live.attach_run(
            "yt-preloop-run".to_string(),
            "sub-kernel".to_string(),
            "plan-fp".to_string(),
            &items,
        )
        .expect("attach run");
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };
        let run = run_record("yt-preloop-run", "{not valid json".to_string());

        let outcome = kernel.execute(&run, &step_record(&run.id));

        assert!(!outcome.succeeded);
        let last = published
            .lock()
            .expect("published lock")
            .last()
            .cloned()
            .expect("terminal snapshot published");
        assert_eq!(last.state, TransientRunState::Failed);
        assert!(last.error.is_some(), "run-level error must be recorded");
        assert!(
            !live.has_active_run("yt-preloop-run"),
            "terminal publish must release the active slot"
        );
        live.attach_run(
            "yt-next-run".to_string(),
            "sub-next".to_string(),
            "plan-fp".to_string(),
            &items,
        )
        .expect("a later start must not be refused with RunAlreadyActive");
    }

    #[test]
    fn pre_loop_failure_for_an_inactive_run_leaves_the_active_run_alone() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let items = vec![work_item(1)];
        live.attach_run(
            "yt-still-active".to_string(),
            "sub-kernel".to_string(),
            "plan-fp".to_string(),
            &items,
        )
        .expect("attach run");
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };

        let outcome =
            kernel.fail_before_start("yt-other-run", "REQUEST_INVALID", "bad".to_string(), false);

        assert!(!outcome.succeeded);
        assert!(
            live.has_active_run("yt-still-active"),
            "failing another run must not release the active slot"
        );
    }

    #[test]
    fn execute_without_live_control_fails_closed_without_publish() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&published);
        live.set_test_sink(Arc::new(move |_| {
            flag.store(true, Ordering::SeqCst);
        }));
        let executor = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };
        let run = RunRecord {
            id: "yt-orphan-run".to_string(),
            workflow_type: WorkflowType::youtube_download(),
            provider: "youtube".to_string(),
            state: RunState::Running,
            legacy_origin: None,
            legacy_id: None,
            request_json: "this-must-not-be-parsed".to_string(),
            output_root: ".".to_string(),
            error_message: None,
            created_at: 1,
            updated_at: 1,
            completed_at: None,
        };
        let step = StepRecord {
            id: "yt-orphan-step".to_string(),
            run_id: run.id.clone(),
            step_key: "video".to_string(),
            step_type: StepType::youtube_execute(),
            state: StepState::Running,
            attempt: 1,
            error_message: None,
            created_at: 1,
            updated_at: 1,
        };

        let outcome = executor.execute(&run, &step);

        assert!(!outcome.succeeded);
        assert!(!outcome.cancelled);
        assert!(
            outcome
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("missing live control")),
            "expected control-plane failure, got {outcome:?}"
        );
        assert!(
            !published.load(Ordering::SeqCst),
            "missing live control must not publish a snapshot"
        );
        assert!(live.control_for(&run.id).is_none());
    }
    /// Reports progress, receives a pause request from the UI mid-item, then
    /// keeps reporting progress before it completes.
    struct PausedMidItemExecutor {
        live: Arc<YouTubeLiveHandle>,
        run_id: String,
    }

    impl TransientExecutor for PausedMidItemExecutor {
        fn execute(
            &self,
            item: &TransientWorkItem,
            _control: &TransientRunControl,
            progress: &mut dyn FnMut(TransientProgressUpdate),
        ) -> Result<TransientExecutionOutcome, TransientError> {
            let mut report = |fraction: f64| {
                progress(TransientProgressUpdate {
                    occurrence_id: item.occurrence_id.clone(),
                    phase: TransientItemPhase::Media,
                    bytes_completed: Some((fraction * 1000.0) as u64),
                    bytes_total: Some(1000),
                    fraction: Some(fraction),
                });
            };
            report(0.1);
            report(0.2);
            let seen = self
                .live
                .snapshot(Some(&self.run_id))
                .expect("live snapshot")
                .expect("active run");
            self.live
                .pause(&self.run_id, seen.revision)
                .expect("pause on the revision the UI last saw");
            report(0.3);
            report(0.4);
            Ok(TransientExecutionOutcome::completed(vec![
                "video".to_string()
            ]))
        }
    }

    #[test]
    fn progress_ticks_never_overwrite_a_pause_or_move_the_revision_backwards() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = capture_sink(&live);
        let items = vec![work_item(1)];
        let control = live
            .attach_run(
                "yt-pause-run".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };
        let executor = PausedMidItemExecutor {
            live: Arc::clone(&live),
            run_id: "yt-pause-run".to_string(),
        };

        let outcome =
            kernel.run_items("yt-pause-run", &durable_request(items), &control, &executor);

        assert!(outcome.succeeded);
        let snapshots = published.lock().expect("published lock").clone();
        let revisions = snapshots
            .iter()
            .map(|snapshot| snapshot.revision)
            .collect::<Vec<_>>();
        assert!(
            revisions.windows(2).all(|pair| pair[0] < pair[1]),
            "every published revision must be strictly greater: {revisions:?}"
        );
        let pause_at = snapshots
            .iter()
            .position(|snapshot| snapshot.state == TransientRunState::PauseRequested)
            .expect("the pause is published");
        let after_pause = &snapshots[pause_at..snapshots.len() - 1];
        assert!(
            after_pause.len() >= 3,
            "progress and the item outcome follow the pause"
        );
        assert!(
            after_pause
                .iter()
                .all(|snapshot| snapshot.state == TransientRunState::PauseRequested),
            "a progress tick or item outcome reverted the pause"
        );
        let last = snapshots.last().expect("terminal snapshot");
        assert_eq!(last.state, TransientRunState::Completed);
        assert_eq!(last.items[0].state, TransientItemState::Completed);
    }
    struct CompletesImmediately;

    impl TransientExecutor for CompletesImmediately {
        fn execute(
            &self,
            _item: &TransientWorkItem,
            _control: &TransientRunControl,
            _progress: &mut dyn FnMut(TransientProgressUpdate),
        ) -> Result<TransientExecutionOutcome, TransientError> {
            Ok(TransientExecutionOutcome::completed(vec![
                "video".to_string()
            ]))
        }
    }

    #[test]
    fn a_pause_landing_just_before_the_item_outcome_is_applied_survives() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = capture_sink(&live);
        let items = vec![work_item(1)];
        let control = live
            .attach_run(
                "yt-race-run".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        // The pause arrives in the window where the kernel used to hold a
        // copy it had read earlier and was about to publish over the top.
        let racing = Arc::clone(&live);
        live.set_before_update(Box::new(move || {
            let seen = racing
                .snapshot(Some("yt-race-run"))
                .expect("live snapshot")
                .expect("active run");
            racing
                .pause("yt-race-run", seen.revision)
                .expect("pause on the revision the UI last saw");
        }));
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };

        let outcome = kernel.run_items(
            "yt-race-run",
            &durable_request(items),
            &control,
            &CompletesImmediately,
        );

        assert!(outcome.succeeded);
        let snapshots = published.lock().expect("published lock").clone();
        let revisions = snapshots
            .iter()
            .map(|snapshot| snapshot.revision)
            .collect::<Vec<_>>();
        assert!(
            revisions.windows(2).all(|pair| pair[0] < pair[1]),
            "every published revision must be strictly greater: {revisions:?}"
        );
        let after_outcome = snapshots
            .iter()
            .find(|snapshot| {
                snapshot.items[0].state == TransientItemState::Completed
                    && !snapshot.state.is_terminal()
            })
            .expect("the item outcome is published before the run ends");
        assert_eq!(
            after_outcome.state,
            TransientRunState::PauseRequested,
            "the item outcome reverted the pause"
        );
        assert!(control.pause_requested());
    }

    struct NetworkDrops;

    impl TransientExecutor for NetworkDrops {
        fn execute(
            &self,
            item: &TransientWorkItem,
            _control: &TransientRunControl,
            progress: &mut dyn FnMut(TransientProgressUpdate),
        ) -> Result<TransientExecutionOutcome, TransientError> {
            progress(TransientProgressUpdate {
                occurrence_id: item.occurrence_id.clone(),
                phase: TransientItemPhase::Media,
                bytes_completed: Some(400),
                bytes_total: Some(1000),
                fraction: Some(0.4),
            });
            Err(TransientError {
                code: "HELPER_FAILED".to_string(),
                message: "connection reset".to_string(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingLedger {
        expected: Mutex<Vec<(String, u64)>>,
        pauses: Mutex<Vec<(String, String, String)>>,
    }

    impl ResumeLedger for RecordingLedger {
        fn is_resumed_attempt(&self, _run_id: &str) -> bool {
            true
        }

        fn record_expected_bytes(&self, _run_id: &str, item_key: &str, bytes: u64) {
            self.expected
                .lock()
                .unwrap()
                .push((item_key.to_string(), bytes));
        }

        fn pause_for_user(&self, run_id: &str, reason: &str, code: &str) -> bool {
            self.pauses
                .lock()
                .unwrap()
                .push((run_id.to_string(), reason.to_string(), code.to_string()));
            true
        }
    }

    #[test]
    fn a_failed_resumed_attempt_pauses_for_the_user_and_frees_the_slot() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = capture_sink(&live);
        let items = vec![work_item(1), work_item(2)];
        let control = live
            .attach_run(
                "yt-paused".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };

        let outcome = kernel.run_items_with(
            "yt-paused",
            &durable_request(items),
            &control,
            &NetworkDrops,
            true,
        );

        assert!(!outcome.succeeded);
        assert!(!outcome.cancelled);
        assert!(outcome.pause_for_user, "a resumed attempt pauses, never fails");
        assert_eq!(outcome.pause_code.as_deref(), Some("HELPER_FAILED"));
        assert!(!live.has_active_run("yt-paused"), "the slot must be released");
        let last = published.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.state, TransientRunState::Paused);
        assert!(last.error.is_some());
        assert_eq!(last.items[0].state, TransientItemState::Pending);
        assert_eq!(last.counts.failed, 0);
    }

    #[test]
    fn a_cancelled_resumed_attempt_is_cancelled_not_paused() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let items = vec![work_item(1)];
        let control = live
            .attach_run(
                "yt-cancel-resumed".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        control.request_cancel();
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };
        let outcome = kernel.run_items_with(
            "yt-cancel-resumed",
            &durable_request(items),
            &control,
            &NetworkDrops,
            true,
        );
        assert!(outcome.cancelled);
        assert!(!outcome.pause_for_user);
    }

    #[test]
    fn a_failed_first_attempt_still_fails_the_run() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let items = vec![work_item(1)];
        let control = live
            .attach_run(
                "yt-first".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: None,
            notify_resume_changed: None,
        };
        let outcome = kernel.run_items_with(
            "yt-first",
            &durable_request(items),
            &control,
            &NetworkDrops,
            false,
        );
        assert!(!outcome.succeeded);
        assert!(!outcome.pause_for_user);
    }

    #[test]
    fn the_item_size_is_recorded_once_for_the_next_resumes_disk_check() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let items = vec![work_item(1)];
        let control = live
            .attach_run(
                "yt-size".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        let ledger = Arc::new(RecordingLedger::default());
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: Some(ledger.clone()),
            notify_resume_changed: None,
        };
        kernel.run_items_with(
            "yt-size",
            &durable_request(items),
            &control,
            &NetworkDrops,
            true,
        );
        assert_eq!(
            ledger.expected.lock().unwrap().clone(),
            vec![("occ-1".to_string(), 1000u64)]
        );
    }

    #[test]
    fn a_resumed_attempt_whose_folder_vanished_pauses_instead_of_failing() {
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = capture_sink(&live);
        let items = vec![work_item(1)];
        live.attach_run(
            "yt-gone".to_string(),
            "sub-kernel".to_string(),
            "plan-fp".to_string(),
            &items,
        )
        .expect("attach run");
        let ledger = Arc::new(RecordingLedger::default());
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: Some(ledger.clone()),
            notify_resume_changed: None,
        };
        let mut run = run_record("yt-gone", serde_json::to_string(&durable_request(items)).unwrap());
        run.output_root = "Z:\\definitely\\not\\a\\folder".to_string();

        let outcome = kernel.execute(&run, &step_record(&run.id));

        assert!(outcome.pause_for_user, "{outcome:?}");
        assert!(!outcome.succeeded);
        assert_eq!(ledger.pauses.lock().unwrap().len(), 1);
        assert_eq!(ledger.pauses.lock().unwrap()[0].2, "OUTPUT_ROOT_INVALID");
        assert!(!live.has_active_run("yt-gone"));
        assert_eq!(
            published.lock().unwrap().last().unwrap().state,
            TransientRunState::Paused
        );
    }

    #[test]
    fn the_pause_is_committed_before_the_paused_snapshot_is_published() {
        struct OrderLedger {
            live: Arc<YouTubeLiveHandle>,
            saw_paused_published: Mutex<Option<bool>>,
            published: Arc<Mutex<Vec<TransientRunSnapshot>>>,
        }
        impl ResumeLedger for OrderLedger {
            fn is_resumed_attempt(&self, _run_id: &str) -> bool {
                true
            }
            fn record_expected_bytes(&self, _: &str, _: &str, _: u64) {}
            fn pause_for_user(&self, run_id: &str, _: &str, _: &str) -> bool {
                let already = self
                    .published
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|snapshot| snapshot.state == TransientRunState::Paused);
                *self.saw_paused_published.lock().unwrap() = Some(already);
                self.live.has_active_run(run_id)
            }
        }
        let live = Arc::new(YouTubeLiveHandle::default());
        let published = capture_sink(&live);
        let items = vec![work_item(1)];
        let control = live
            .attach_run(
                "yt-order".to_string(),
                "sub-kernel".to_string(),
                "plan-fp".to_string(),
                &items,
            )
            .expect("attach run");
        let ledger = Arc::new(OrderLedger {
            live: Arc::clone(&live),
            saw_paused_published: Mutex::new(None),
            published,
        });
        let kernel = YoutubeDownloadExecutor {
            live: Arc::clone(&live),
            resume: Some(ledger.clone()),
            notify_resume_changed: None,
        };

        kernel.run_items_with(
            "yt-order",
            &durable_request(items),
            &control,
            &NetworkDrops,
            true,
        );

        assert_eq!(
            *ledger.saw_paused_published.lock().unwrap(),
            Some(false),
            "the durable pause must be committed before the paused snapshot is emitted"
        );
    }
}
