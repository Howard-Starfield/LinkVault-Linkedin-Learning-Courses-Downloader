//! Provider-owned step execution. The kernel claims work; adapters run outside
//! the database writer.

use crate::workflow::domain::types::{RunRecord, StepRecord};

pub trait StepExecutor: Send + Sync {
    fn workflow_type(&self) -> &'static str;

    fn execute(&self, run: &RunRecord, step: &StepRecord) -> ExecutorOutcome;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorOutcome {
    pub succeeded: bool,
    pub cancelled: bool,
    pub warning: bool,
    pub retryable: bool,
    /// Pause the run for the user instead of failing it (resumed attempts).
    pub pause_for_user: bool,
    pub pause_code: Option<String>,
    /// Last live revision published before the pause, so the durable write
    /// (or its retry by the runtime) never lowers it.
    pub pause_revision: u64,
    pub error_message: Option<String>,
    pub payload_json: String,
}

impl ExecutorOutcome {
    pub fn succeeded(payload_json: String) -> Self {
        Self {
            succeeded: true,
            cancelled: false,
            warning: false,
            retryable: false,
            pause_for_user: false,
            pause_code: None,
            pause_revision: 0,
            error_message: None,
            payload_json,
        }
    }

    pub fn failed(message: String) -> Self {
        Self {
            succeeded: false,
            cancelled: false,
            warning: false,
            retryable: false,
            pause_for_user: false,
            pause_code: None,
            pause_revision: 0,
            error_message: Some(message),
            payload_json: "{}".to_string(),
        }
    }

    pub fn retryable_failure(message: String) -> Self {
        Self {
            succeeded: false,
            cancelled: false,
            warning: false,
            retryable: true,
            pause_for_user: false,
            pause_code: None,
            pause_revision: 0,
            error_message: Some(message),
            payload_json: "{}".to_string(),
        }
    }

    pub fn cancelled(message: String) -> Self {
        Self {
            succeeded: false,
            cancelled: true,
            warning: false,
            retryable: false,
            pause_for_user: false,
            pause_code: None,
            pause_revision: 0,
            error_message: Some(message),
            payload_json: "{}".to_string(),
        }
    }

    /// The attempt failed but the run can be continued by the user: the kernel
    /// pauses it (keeping its resume state) rather than failing it.
    pub fn paused_for_user(message: String, code: Option<String>) -> Self {
        Self {
            succeeded: false,
            cancelled: false,
            warning: false,
            retryable: false,
            pause_for_user: true,
            pause_code: code,
            pause_revision: 0,
            error_message: Some(message),
            payload_json: "{}".to_string(),
        }
    }
}
