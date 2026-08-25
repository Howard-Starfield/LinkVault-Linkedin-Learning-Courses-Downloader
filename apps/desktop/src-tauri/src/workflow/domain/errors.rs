//! Domain errors for the shared workflow kernel.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("illegal run transition from {from} to {to}")]
    IllegalRunTransition { from: String, to: String },
    #[error("illegal step transition from {from} to {to}")]
    IllegalStepTransition { from: String, to: String },
    #[error("unknown run state: {0}")]
    UnknownRunState(String),
    #[error("unknown step state: {0}")]
    UnknownStepState(String),
    #[error("workflow run not found: {0}")]
    RunNotFound(String),
    #[error("workflow step not found: {0}")]
    StepNotFound(String),
    #[error("duplicate legacy origin {origin} id {id}")]
    DuplicateLegacy { origin: String, id: String },
    #[error("sqlite error: {0}")]
    Sqlite(String),
    #[error("database writer error: {0}")]
    Writer(String),
}

impl From<rusqlite::Error> for WorkflowError {
    fn from(error: rusqlite::Error) -> Self {
        let message = error.to_string();
        if message.to_ascii_lowercase().contains("unique") {
            return Self::DuplicateLegacy {
                origin: "unknown".to_string(),
                id: "unknown".to_string(),
            };
        }
        Self::Sqlite(message)
    }
}
