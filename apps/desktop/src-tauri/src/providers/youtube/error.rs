use crate::app::managed_process::ManagedProcessError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
#[serde(rename_all = "camelCase")]
pub struct YouTubeError {
    pub code: String,
    pub message: String,
}

impl YouTubeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<crate::app::safe_output_filesystem::SafeOutputError> for YouTubeError {
    fn from(error: crate::app::safe_output_filesystem::SafeOutputError) -> Self {
        Self::new("OUTPUT_ROOT_INVALID", error.to_string())
    }
}

impl From<crate::app::managed_process::TransientRuntimeError> for YouTubeError {
    fn from(error: crate::app::managed_process::TransientRuntimeError) -> Self {
        let code = match error {
            crate::app::managed_process::TransientRuntimeError::Busy => "RUNTIME_BUSY",
            crate::app::managed_process::TransientRuntimeError::ShuttingDown => "APP_SHUTTING_DOWN",
            crate::app::managed_process::TransientRuntimeError::Quarantined => {
                "RUNTIME_QUARANTINED"
            }
            crate::app::managed_process::TransientRuntimeError::RunNotFound => "RUN_NOT_FOUND",
            crate::app::managed_process::TransientRuntimeError::StaleRevision => "STALE_REVISION",
            crate::app::managed_process::TransientRuntimeError::InvalidTransition => {
                "INVALID_STATE_TRANSITION"
            }
            crate::app::managed_process::TransientRuntimeError::DiscoveryNotFound => {
                "DISCOVERY_NOT_FOUND"
            }
            crate::app::managed_process::TransientRuntimeError::OperationIdReused => {
                "OPERATION_ID_REUSED"
            }
            crate::app::managed_process::TransientRuntimeError::SubmissionConflict => {
                "SUBMISSION_CONFLICT"
            }
            crate::app::managed_process::TransientRuntimeError::SubmissionRetired => {
                "SUBMISSION_ID_RETIRED"
            }
            crate::app::managed_process::TransientRuntimeError::SubmissionCapacity => {
                "SUBMISSION_ID_CAPACITY_EXHAUSTED"
            }
            crate::app::managed_process::TransientRuntimeError::RunAlreadyActive => {
                "RUN_ALREADY_ACTIVE"
            }
            crate::app::managed_process::TransientRuntimeError::SubmissionPending => "RUNTIME_BUSY",
            crate::app::managed_process::TransientRuntimeError::WorkerSpawnFailed => {
                "INVALID_RUN_TRANSITION"
            }
            crate::app::managed_process::TransientRuntimeError::DiscoveryCapacity => {
                "RUNTIME_CAPACITY"
            }
        };
        Self::new(code, error.to_string())
    }
}

#[derive(Debug, Error)]
pub enum YouTubeInternalError {
    #[error("{0}")]
    Public(#[from] YouTubeError),
    #[error("helper failed: {0}")]
    Helper(String),
    #[error("{0}")]
    HelperTemp(String),
    #[error("invalid source URL: {0}")]
    InvalidUrl(String),
    #[error("scan plan is not available")]
    PlanNotFound,
    #[error("scan plan has expired")]
    PlanExpired,
    #[error("scan plan no longer matches the current public source")]
    ScanPlanStale,
    #[error("selected occurrence is not part of the scan plan")]
    UnknownOccurrence,
    #[error("selected occurrences must be unique")]
    DuplicateOccurrence,
    #[error("at least one occurrence must be selected")]
    EmptySelection,
    #[error("too many selected occurrences")]
    TooManySelected,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl YouTubeInternalError {
    /// Map a failed discovery/inspection helper run. Integrity failures keep their
    /// own public code so the UI can offer a helper reinstall.
    pub(crate) fn from_helper_run(error: ManagedProcessError) -> Self {
        match error {
            ManagedProcessError::HelperTemp(reason) => Self::HelperTemp(reason),
            ManagedProcessError::Integrity(reason) => {
                Self::Public(YouTubeError::new("HELPER_INTEGRITY_FAILED", reason))
            }
            error => Self::Helper(error.to_string()),
        }
    }
}

impl From<YouTubeInternalError> for YouTubeError {
    fn from(error: YouTubeInternalError) -> Self {
        match error {
            YouTubeInternalError::Public(error) => error,
            YouTubeInternalError::Helper(message) => Self::new("HELPER_FAILED", message),
            YouTubeInternalError::HelperTemp(message) => {
                Self::new("HELPER_TEMP_ROOT_INVALID", message)
            }
            YouTubeInternalError::InvalidUrl(message) => Self::new("INVALID_URL", message),
            YouTubeInternalError::PlanNotFound => {
                Self::new("SCAN_PLAN_NOT_FOUND", "scan plan is not available")
            }
            YouTubeInternalError::PlanExpired => {
                Self::new("SCAN_PLAN_EXPIRED", "scan plan has expired")
            }
            YouTubeInternalError::ScanPlanStale => Self::new(
                "SCAN_PLAN_STALE",
                "scan plan no longer matches the current public source",
            ),
            YouTubeInternalError::UnknownOccurrence => Self::new(
                "UNKNOWN_OCCURRENCE",
                "selected occurrence is not part of the scan plan",
            ),
            YouTubeInternalError::DuplicateOccurrence => Self::new(
                "DUPLICATE_OCCURRENCE",
                "selected occurrences must be unique",
            ),
            YouTubeInternalError::EmptySelection => Self::new(
                "EMPTY_SELECTION",
                "at least one occurrence must be selected",
            ),
            YouTubeInternalError::TooManySelected => {
                Self::new("SELECTION_LIMIT", "too many selected occurrences")
            }
            YouTubeInternalError::InvalidRequest(message) => Self::new("INVALID_REQUEST", message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_code(error: ManagedProcessError) -> (String, String) {
        let public = YouTubeError::from(YouTubeInternalError::from_helper_run(error));
        (public.code, public.message)
    }

    #[test]
    fn scan_and_inspection_integrity_failures_surface_as_helper_integrity_failed() {
        let (code, message) = public_code(ManagedProcessError::Integrity(
            "the yt-dlp lock entry is unavailable".to_string(),
        ));
        assert_eq!(code, "HELPER_INTEGRITY_FAILED");
        assert_eq!(message, "the yt-dlp lock entry is unavailable");
    }

    #[test]
    fn scan_and_inspection_temp_and_start_failures_keep_their_codes() {
        let (code, _) = public_code(ManagedProcessError::HelperTemp("junction".to_string()));
        assert_eq!(code, "HELPER_TEMP_ROOT_INVALID");
        let (code, message) = public_code(ManagedProcessError::Start("blocked".to_string()));
        assert_eq!(code, "HELPER_FAILED");
        assert!(message.contains("failed to start trusted helper: blocked"));
    }
}
