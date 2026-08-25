//! Durable-kernel adapter around the provider-owned YouTube item executor.

use crate::app::managed_process::{TransientError, TransientExecutor, TransientItemState};
use crate::app::safe_output_filesystem::validate_output_root;
use crate::providers::youtube::live::{
    apply_item_outcome, apply_item_progress, executor_from_request, finish_snapshot,
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
            Err(error) => return ExecutorOutcome::failed(error.to_string()),
        };
        let output_root = match validate_output_root(PathBuf::from(&run.output_root).as_path()) {
            Ok(root) => root,
            Err(error) => return ExecutorOutcome::failed(error.to_string()),
        };
        let executor = match executor_from_request(output_root, &request) {
            Ok(executor) => executor,
            Err(error) => return ExecutorOutcome::failed(error.to_string()),
        };
        let mut snapshot = match self.live.snapshot(Some(&run.id)) {
            Ok(Some(snapshot)) => snapshot,
            _ => crate::providers::youtube::live::initial_snapshot(
                run.id.clone(),
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

            let live = Arc::clone(&self.live);
            let mut item_snapshot = snapshot.clone();
            let work_item = item.clone();
            let outcome =
                TransientExecutor::execute(executor.as_ref(), item, &control, &mut |update| {
                    apply_item_progress(
                        &mut item_snapshot,
                        &work_item,
                        update.phase,
                        update.bytes_completed,
                        update.bytes_total,
                        update.fraction,
                    );
                    live.publish_snapshot(item_snapshot.clone());
                });
            snapshot = item_snapshot;

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
                    finish_snapshot(
                        &mut snapshot,
                        control.is_cancelled(),
                        Some(TransientError {
                            code: code.clone(),
                            message: message.clone(),
                        }),
                    );
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
    use crate::workflow::domain::state::{RunState, StepState};
    use crate::workflow::domain::types::{StepType, WorkflowType};
    use std::sync::atomic::{AtomicBool, Ordering};

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
}
