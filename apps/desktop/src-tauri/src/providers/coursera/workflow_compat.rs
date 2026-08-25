//! Compatibility mapping between Coursera job strings and workflow states.
//!
//! New Coursera submissions use `workflow_runs`; this module is the status
//! translation table for bootstrap projections and history.

use crate::workflow::domain::state::RunState;
use crate::workflow::domain::types::WorkflowType;

pub fn coursera_status_to_run_state(status: &str) -> Option<RunState> {
    match status.to_ascii_lowercase().as_str() {
        "queued" => Some(RunState::Queued),
        "active" => Some(RunState::Running),
        "completed" => Some(RunState::Succeeded),
        "failed" => Some(RunState::Failed),
        "cancelled" => Some(RunState::Cancelled),
        _ => None,
    }
}

pub fn run_state_to_coursera_status(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "Queued",
        RunState::Running | RunState::RetryWait | RunState::Cancelling => "Active",
        RunState::Paused => "Queued",
        RunState::Succeeded | RunState::SucceededWithWarnings => "Completed",
        RunState::Failed => "Failed",
        RunState::Cancelled => "Cancelled",
    }
}

pub fn coursera_workflow_type() -> WorkflowType {
    WorkflowType::coursera_download()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_terminal_coursera_statuses() {
        for status in ["Queued", "Active", "Completed", "Failed", "Cancelled"] {
            let state = coursera_status_to_run_state(status).expect(status);
            assert_eq!(
                coursera_status_to_run_state(run_state_to_coursera_status(state)),
                Some(state)
            );
        }
        assert_eq!(coursera_workflow_type().as_str(), "coursera_download");
    }
}
