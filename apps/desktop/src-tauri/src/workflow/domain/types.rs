//! Identity and record types for durable workflow runs.

use super::errors::WorkflowError;
use super::state::{RunState, StepState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowType(String);

impl WorkflowType {
    pub fn synthetic() -> Self {
        Self("synthetic".to_string())
    }

    pub fn coursera_download() -> Self {
        Self("coursera_download".to_string())
    }

    pub fn linkedin_download() -> Self {
        Self("linkedin_download".to_string())
    }

    pub fn youtube_download() -> Self {
        Self("youtube_download".to_string())
    }

    pub fn newspaper_download() -> Self {
        Self("newspaper_download".to_string())
    }

    pub fn from_owned(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepType(String);

impl StepType {
    pub fn synthetic_execute() -> Self {
        Self("synthetic_execute".to_string())
    }

    pub fn coursera_execute() -> Self {
        Self("coursera_execute".to_string())
    }

    pub fn linkedin_execute() -> Self {
        Self("linkedin_execute".to_string())
    }

    pub fn youtube_execute() -> Self {
        Self("youtube_execute".to_string())
    }

    pub fn newspaper_execute() -> Self {
        Self("newspaper_execute".to_string())
    }

    pub fn from_owned(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkflowRun {
    pub id: String,
    pub workflow_type: WorkflowType,
    pub provider: String,
    pub legacy_origin: Option<String>,
    pub legacy_id: Option<String>,
    pub request_json: String,
    pub output_root: String,
    pub created_at: i64,
    pub ready_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkflowStep {
    pub id: String,
    pub step_key: String,
    pub step_type: StepType,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub id: String,
    pub workflow_type: WorkflowType,
    pub provider: String,
    pub state: RunState,
    pub legacy_origin: Option<String>,
    pub legacy_id: Option<String>,
    pub request_json: String,
    pub output_root: String,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub id: String,
    pub run_id: String,
    pub step_key: String,
    pub step_type: StepType,
    pub state: StepState,
    pub attempt: i64,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowEventRecord {
    pub id: i64,
    pub run_id: String,
    pub step_id: Option<String>,
    pub sequence: i64,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: i64,
}

impl RunRecord {
    pub fn require_state(value: &str) -> Result<RunState, WorkflowError> {
        RunState::parse(value)
    }
}
