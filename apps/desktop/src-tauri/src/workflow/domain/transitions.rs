//! Single transition matrix for workflow runs and steps.

use super::state::{RunState, StepState};

pub fn is_allowed_run_transition(from: RunState, to: RunState) -> bool {
    if from == to {
        return false;
    }
    match from {
        RunState::Queued => matches!(to, RunState::Running | RunState::Cancelled),
        RunState::Running => matches!(
            to,
            RunState::Paused
                | RunState::RetryWait
                | RunState::Cancelling
                | RunState::Succeeded
                | RunState::SucceededWithWarnings
                | RunState::Failed
                | RunState::Cancelled
        ),
        RunState::Paused => matches!(to, RunState::Running | RunState::Cancelled),
        RunState::RetryWait => matches!(to, RunState::Running | RunState::Cancelled),
        RunState::Cancelling => matches!(to, RunState::Cancelled),
        RunState::Succeeded
        | RunState::SucceededWithWarnings
        | RunState::Failed
        | RunState::Cancelled => false,
    }
}

pub fn is_allowed_step_transition(from: StepState, to: StepState) -> bool {
    if from == to {
        return false;
    }
    match from {
        StepState::Pending => matches!(to, StepState::Ready | StepState::Cancelled),
        StepState::Ready => matches!(
            to,
            StepState::Running | StepState::Skipped | StepState::Cancelled
        ),
        StepState::Running => matches!(
            to,
            StepState::RetryWait | StepState::Succeeded | StepState::Failed | StepState::Cancelled
        ),
        StepState::RetryWait => matches!(to, StepState::Running | StepState::Cancelled),
        StepState::Succeeded | StepState::Skipped | StepState::Failed | StepState::Cancelled => {
            false
        }
    }
}

pub fn validate_run_transition(
    from: RunState,
    to: RunState,
) -> Result<(), super::errors::WorkflowError> {
    if is_allowed_run_transition(from, to) {
        Ok(())
    } else {
        Err(super::errors::WorkflowError::IllegalRunTransition {
            from: from.as_str().to_string(),
            to: to.as_str().to_string(),
        })
    }
}

pub fn validate_step_transition(
    from: StepState,
    to: StepState,
) -> Result<(), super::errors::WorkflowError> {
    if is_allowed_step_transition(from, to) {
        Ok(())
    } else {
        Err(super::errors::WorkflowError::IllegalStepTransition {
            from: from.as_str().to_string(),
            to: to.as_str().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_run_can_start_or_cancel_but_not_succeed_directly() {
        assert!(is_allowed_run_transition(
            RunState::Queued,
            RunState::Running
        ));
        assert!(is_allowed_run_transition(
            RunState::Queued,
            RunState::Cancelled
        ));
        assert!(!is_allowed_run_transition(
            RunState::Queued,
            RunState::Succeeded
        ));
    }

    #[test]
    fn terminal_run_states_have_no_outbound_transitions() {
        for from in [
            RunState::Succeeded,
            RunState::SucceededWithWarnings,
            RunState::Failed,
            RunState::Cancelled,
        ] {
            for to in [
                RunState::Queued,
                RunState::Running,
                RunState::Paused,
                RunState::RetryWait,
                RunState::Cancelling,
                RunState::Succeeded,
                RunState::Failed,
                RunState::Cancelled,
            ] {
                assert!(
                    !is_allowed_run_transition(from, to),
                    "{from:?} -> {to:?} must be forbidden"
                );
            }
        }
    }

    #[test]
    fn running_step_can_succeed_or_fail() {
        assert!(is_allowed_step_transition(
            StepState::Running,
            StepState::Succeeded
        ));
        assert!(is_allowed_step_transition(
            StepState::Running,
            StepState::Failed
        ));
        assert!(!is_allowed_step_transition(
            StepState::Succeeded,
            StepState::Running
        ));
    }

    #[test]
    fn validate_run_transition_rejects_illegal_jumps() {
        let error = validate_run_transition(RunState::Queued, RunState::Succeeded).unwrap_err();
        assert!(matches!(
            error,
            super::super::errors::WorkflowError::IllegalRunTransition { .. }
        ));
    }
}
