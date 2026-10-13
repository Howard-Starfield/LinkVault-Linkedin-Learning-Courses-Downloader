//! Durable-kernel adapter around the provider-owned YouTube item executor.

use crate::app::managed_process::{
    TransientError, TransientExecutor, TransientItemState, TransientRunControl,
};
use crate::app::safe_output_filesystem::validate_output_root;
use crate::providers::youtube::live::{
    apply_item_failure, apply_item_outcome, executor_from_request, finish_snapshot,
    YouTubeLiveHandle, YoutubeDurableRequest,
};
use crate::workflow::domain::types::{RunRecord, StepRecord};
use crate::workflow::ports::executor::{ExecutorOutcome, StepExecutor};
use std::path::PathBuf;
use std::sync::Arc;

pub struct YoutubeDownloadExecutor {
    pub live: Arc<YouTubeLiveHandle>,
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
        let request = match serde_json::from_str::<YoutubeDurableRequest>(&run.request_json) {
            Ok(request) => request,
            Err(error) => {
                return self.fail_before_start(&run.id, "REQUEST_INVALID", error.to_string())
            }
        };
        let output_root = match validate_output_root(PathBuf::from(&run.output_root).as_path()) {
            Ok(root) => root,
            Err(error) => {
                return self.fail_before_start(&run.id, "OUTPUT_ROOT_INVALID", error.to_string())
            }
        };
        let executor = match executor_from_request(output_root, &request) {
            Ok(executor) => executor,
            Err(error) => return self.fail_before_start(&run.id, &error.code, error.message),
        };
        self.run_items(&run.id, &request, &control, executor.as_ref())
    }
}

impl YoutubeDownloadExecutor {
    /// Fail the run before any item started.
    ///
    /// The live slot is only released by a terminal publish, so a bare
    /// `ExecutorOutcome::failed` would leave the run "active" and make every
    /// later start fail with `RunAlreadyActive`.
    fn fail_before_start(&self, run_id: &str, code: &str, message: String) -> ExecutorOutcome {
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
        finish_snapshot(&mut snapshot, false, Some(error));
        self.live.publish_snapshot(snapshot);
        ExecutorOutcome::failed(failure)
    }

    /// Drive every work item through `executor`, publishing snapshots as it goes.
    fn run_items(
        &self,
        run_id: &str,
        request: &YoutubeDurableRequest,
        control: &TransientRunControl,
        executor: &dyn TransientExecutor,
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
                finish_snapshot(&mut snapshot, true, None);
                self.live.publish_snapshot(snapshot);
                return ExecutorOutcome::cancelled("YouTube download was cancelled".to_string());
            }
            if control.pause_requested() {
                let _ = control.mark_paused_if_requested();
                control.wait_for_resume();
            }
            if control.is_cancelled() {
                finish_snapshot(&mut snapshot, true, None);
                self.live.publish_snapshot(snapshot);
                return ExecutorOutcome::cancelled("YouTube download was cancelled".to_string());
            }

            // Progress ticks mutate the live snapshot in place so a pause or
            // cancel requested meanwhile is not overwritten; re-read it after.
            let outcome = TransientExecutor::execute(executor, item, control, &mut |update| {
                self.live.publish_item_progress(run_id, item, update);
            });
            if let Ok(Some(current)) = self.live.snapshot(Some(run_id)) {
                snapshot = current;
            }

            match outcome {
                Ok(outcome) => {
                    apply_item_outcome(&mut snapshot, item, &outcome);
                    self.live.publish_snapshot(snapshot.clone());
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
                        finish_snapshot(
                            &mut snapshot,
                            matches!(outcome.state, TransientItemState::Cancelled),
                            outcome.error,
                        );
                        self.live.publish_snapshot(snapshot);
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
                    if !control.is_cancelled() {
                        apply_item_failure(&mut snapshot, item, &error);
                    }
                    finish_snapshot(&mut snapshot, control.is_cancelled(), Some(error));
                    self.live.publish_snapshot(snapshot);
                    if control.is_cancelled() {
                        return ExecutorOutcome::cancelled(message);
                    }
                    return ExecutorOutcome::failed(format!("{code}: {message}"));
                }
            }
        }

        finish_snapshot(&mut snapshot, false, None);
        let warning = snapshot.counts.completed_with_warnings > 0;
        let payload = serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string());
        self.live.publish_snapshot(snapshot);
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
        };

        let outcome =
            kernel.fail_before_start("yt-other-run", "REQUEST_INVALID", "bad".to_string());

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
}
