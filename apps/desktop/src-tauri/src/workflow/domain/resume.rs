//! Resume vocabulary for runs interrupted by a restart or a failed attempt.
//!
//! A run that is waiting to be resumed is durably `Paused` and carries one
//! `RunResumeRecord`. The record says who moves it next: the app (once,
//! `AutoPending`), the user (`NeedsUser`), or nobody because an attempt is
//! already running (`Resuming`).

use super::errors::WorkflowError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeState {
    /// Interrupted; the app resumes it once, automatically.
    AutoPending,
    /// Paused until the user presses Continue.
    NeedsUser,
    /// A resumed attempt (automatic or manual) has been admitted.
    Resuming,
}

impl ResumeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoPending => "auto_pending",
            Self::NeedsUser => "needs_user",
            Self::Resuming => "resuming",
        }
    }

    pub fn parse(value: &str) -> Result<Self, WorkflowError> {
        match value {
            "auto_pending" => Ok(Self::AutoPending),
            "needs_user" => Ok(Self::NeedsUser),
            "resuming" => Ok(Self::Resuming),
            other => Err(WorkflowError::Sqlite(format!(
                "unknown resume state: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeTrigger {
    /// The one automatic resume. Spends the budget.
    Auto,
    /// The user pressed Continue.
    Manual,
    /// Continue, valid only for the pause the user was shown (`generation`).
    /// A delayed duplicate click carries a stale generation and is refused.
    ManualAt(i64),
}

impl ResumeTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual | Self::ManualAt(_) => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResumeRecord {
    pub run_id: String,
    pub state: ResumeState,
    pub auto_resume_used: bool,
    pub reason: Option<String>,
    pub reason_code: Option<String>,
    pub expected_bytes: Option<u64>,
    pub expected_item_key: Option<String>,
    pub updated_at: i64,
    /// Bumped every time the run enters a pause, so Continue can name the
    /// pause it answers.
    pub generation: i64,
}

/// What a restart reconcile did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileSummary {
    /// Runs left paused and resumable (automatically or by the user).
    pub resumable: usize,
    /// Runs that were mid-cancel and are now finished as cancelled.
    pub cancelled: usize,
}

pub const INTERRUPTED_AGAIN_REASON: &str =
    "The app closed again during the resumed attempt. Press Continue to try once more.";
