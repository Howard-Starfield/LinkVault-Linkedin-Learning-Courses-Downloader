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
            error_message: Some(message),
            payload_json: "{}".to_string(),
        }
    }
}
