//! Coursera error type and `Result` alias.
//!
//! Mirrors the Python `coursera-dl` exception hierarchy but expressed as
//! a single `thiserror` enum. Use this everywhere; do not return `String`
//! for errors from Coursera code. Convert at the Tauri command boundary
//! via `Display` or `map_err(|e| e.to_string())`.

use std::io;

use crate::coursera::coursera_token_store::CourseraTokenStoreError;

/// All errors that can be returned from the `coursera/` module.
#[allow(dead_code)] // wired in by Phase 3+; thiserror variants are added as needed
#[derive(thiserror::Error, Debug)]
pub enum CourseraError {
    #[error("authentication failed")]
    Auth,

    #[error("class not found: {0}")]
    ClassNotFound(String),

    #[error("syllabus parse error: {0}")]
    SyllabusParse(String),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("token store error: {0}")]
    TokenStore(#[from] CourseraTokenStoreError),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("cancelled")]
    Cancelled,

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("other: {0}")]
    Other(String),
}

/// Convenience alias for fallible Coursera operations.
#[allow(dead_code)] // wired in by Phase 3+
pub type CourseraResult<T> = Result<T, CourseraError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_stable() {
        // The exact strings are part of the user-visible contract (they
        // surface in Tauri toasts and SQLite events). Lock them in.
        assert_eq!(CourseraError::Auth.to_string(), "authentication failed");
        assert_eq!(
            CourseraError::ClassNotFound("ml-005".into()).to_string(),
            "class not found: ml-005"
        );
        assert_eq!(
            CourseraError::SyllabusParse("missing modules".into()).to_string(),
            "syllabus parse error: missing modules"
        );
        assert_eq!(CourseraError::Cancelled.to_string(), "cancelled");
        assert_eq!(
            CourseraError::InvalidArgument("bad regex".into()).to_string(),
            "invalid argument: bad regex"
        );
        assert_eq!(
            CourseraError::Other("oops".into()).to_string(),
            "other: oops"
        );
    }

    #[test]
    fn from_io_error_works() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "missing");
        let err: CourseraError = io_err.into();
        assert!(matches!(err, CourseraError::Io(_)));
    }

    #[test]
    fn cancelled_does_not_carry_payload() {
        // Important: Cancelled must NOT carry a String. Phase 8's orchestrator
        // uses the discriminant to decide whether to surface a user message
        // or just silently unwind.
        let err = CourseraError::Cancelled;
        assert!(matches!(err, CourseraError::Cancelled));
    }
}
