//! Run and step states from the unified workflow migration plan.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Queued,
    Running,
    Paused,
    RetryWait,
    Cancelling,
    Succeeded,
    SucceededWithWarnings,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Ready,
    Running,
    RetryWait,
    Succeeded,
    Skipped,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::RetryWait => "retry_wait",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::SucceededWithWarnings => "succeeded_with_warnings",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::SucceededWithWarnings | Self::Failed | Self::Cancelled
        )
    }

    pub fn parse(value: &str) -> Result<Self, super::errors::WorkflowError> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "retry_wait" => Ok(Self::RetryWait),
            "cancelling" => Ok(Self::Cancelling),
            "succeeded" => Ok(Self::Succeeded),
            "succeeded_with_warnings" => Ok(Self::SucceededWithWarnings),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(super::errors::WorkflowError::UnknownRunState(
                other.to_string(),
            )),
        }
    }
}

impl StepState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::RetryWait => "retry_wait",
            Self::Succeeded => "succeeded",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Result<Self, super::errors::WorkflowError> {
        match value {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            "running" => Ok(Self::Running),
            "retry_wait" => Ok(Self::RetryWait),
            "succeeded" => Ok(Self::Succeeded),
            "skipped" => Ok(Self::Skipped),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(super::errors::WorkflowError::UnknownStepState(
                other.to_string(),
            )),
        }
    }
}
