//! Validation and serialized-write boundary for clipping note recovery drafts.

use std::path::PathBuf;

use crate::app::database::open_runtime;
use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};

pub use super::clipping_draft_models::{
    CheckpointClippingNoteRequest, ClaimClippingNoteRecoveryRequest, ClippingNoteCheckpointAck,
    ClippingNoteRecoveryResponse, ClippingNoteRecoveryStatus, DiscardClippingNoteRecoveryRequest,
    LoadClippingNoteRecoveryRequest, RecoveredClippingNoteDraft,
};
use super::clipping_draft_repository::{
    self as repository, DraftCheckpoint, DraftCheckpointAck, DraftIdentityOutcome,
    DraftLoadOutcome, DraftWriteOutcome,
};
use super::clipping_models::{
    validate_clipping_id, ClippingError, ClippingErrorCode, ClippingNoteCheckpointIdentityRequest,
};
use super::clipping_service::ClippingService;

pub const RECOVERY_TITLE_MAX_BYTES: usize = 4 * 1024;
pub const RECOVERY_MARKDOWN_MAX_BYTES: usize = 4 * 1024 * 1024;
impl ClippingNoteCheckpointIdentityRequest {
    pub fn validated(&self) -> Result<DraftCheckpointAck, ClippingError> {
        validate_identity(&self.writer_session_id, self.writer_sequence)?;
        Ok(DraftCheckpointAck {
            writer_session_id: self.writer_session_id.clone(),
            writer_sequence: self.writer_sequence,
        })
    }
}
#[derive(Clone)]
pub struct ClippingDraftService {
    db_path: PathBuf,
    writer: DatabaseWriter,
}

impl ClippingDraftService {
    pub fn new(db_path: PathBuf, writer: DatabaseWriter) -> Self {
        Self { db_path, writer }
    }

    pub fn checkpoint(
        &self,
        request: CheckpointClippingNoteRequest,
        now: i64,
    ) -> Result<ClippingNoteCheckpointAck, ClippingError> {
        validate_request(&request)?;
        let ack = request_ack(&request);
        let outcome = self
            .writer
            .execute(context("clipping_note_checkpoint"), move |connection| {
                repository::checkpoint(
                    connection,
                    &DraftCheckpoint {
                        clipping_id: request.clipping_id,
                        base_revision: request.base_revision,
                        writer_session_id: request.writer_session_id,
                        writer_sequence: request.writer_sequence,
                        title: request.title,
                        markdown: request.markdown,
                        updated_at: now,
                    },
                )
                .map_err(Into::into)
            })
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
        match outcome {
            DraftWriteOutcome::Stored | DraftWriteOutcome::Idempotent => Ok(ack),
            DraftWriteOutcome::StaleSequence => {
                Err(recovery_error(ClippingErrorCode::RecoveryStaleSequence))
            }
            DraftWriteOutcome::WriterConflict => {
                Err(recovery_error(ClippingErrorCode::RecoveryWriterConflict))
            }
            DraftWriteOutcome::NotFound => Err(recovery_error(ClippingErrorCode::NotFound)),
            DraftWriteOutcome::NotEditable => Err(recovery_error(ClippingErrorCode::NotEditable)),
        }
    }

    pub fn load(
        &self,
        request: &LoadClippingNoteRecoveryRequest,
    ) -> Result<ClippingNoteRecoveryResponse, ClippingError> {
        validate_document_id(&request.clipping_id)?;
        let connection = open_runtime(&self.db_path)
            .map_err(|_| recovery_error(ClippingErrorCode::DatabaseReadFailed))?;
        classify(
            repository::load(&connection, &request.clipping_id)
                .map_err(|_| recovery_error(ClippingErrorCode::DatabaseReadFailed))?,
        )
    }

    pub fn claim(
        &self,
        request: ClaimClippingNoteRecoveryRequest,
    ) -> Result<ClippingNoteRecoveryResponse, ClippingError> {
        validate_document_id(&request.clipping_id)?;
        validate_identity(
            &request.prior_writer_session_id,
            request.prior_writer_sequence,
        )?;
        validate_identity(&request.writer_session_id, 1)?;
        if request.prior_writer_session_id == request.writer_session_id {
            return Err(recovery_error(ClippingErrorCode::RecoveryInvalid));
        }
        let (outcome, loaded) = self
            .writer
            .execute(context("clipping_note_recovery_claim"), move |connection| {
                let prior = DraftCheckpointAck {
                    writer_session_id: request.prior_writer_session_id,
                    writer_sequence: request.prior_writer_sequence,
                };
                let loaded = repository::load(connection, &request.clipping_id)?;
                let eligible = matches!(
                    &loaded,
                    DraftLoadOutcome::Found {
                        canonical_revision,
                        checkpoint,
                    } if checkpoint.base_revision <= *canonical_revision
                        && validate_stored(checkpoint).is_ok()
                );
                if !eligible {
                    return Ok((None, loaded));
                }
                let outcome = repository::claim(
                    connection,
                    &request.clipping_id,
                    &prior,
                    &request.writer_session_id,
                )?;
                let loaded = if outcome == DraftIdentityOutcome::Applied {
                    repository::load(connection, &request.clipping_id)?
                } else {
                    loaded
                };
                Ok((Some(outcome), loaded))
            })
            .map_err(|_| recovery_error(ClippingErrorCode::DatabaseWriteFailed))?;
        match (outcome, loaded) {
            (Some(DraftIdentityOutcome::Applied), loaded) | (None, loaded) => classify(loaded),
            (Some(outcome), _) => Err(identity_error(outcome)),
        }
    }

    pub fn discard(
        &self,
        request: DiscardClippingNoteRecoveryRequest,
    ) -> Result<(), ClippingError> {
        validate_document_id(&request.clipping_id)?;
        validate_identity(&request.writer_session_id, request.writer_sequence)?;
        let outcome = self
            .writer
            .execute(
                context("clipping_note_recovery_discard"),
                move |connection| {
                    repository::discard(
                        connection,
                        &request.clipping_id,
                        &DraftCheckpointAck {
                            writer_session_id: request.writer_session_id,
                            writer_sequence: request.writer_sequence,
                        },
                    )
                    .map_err(Into::into)
                },
            )
            .map_err(|_| recovery_error(ClippingErrorCode::DatabaseWriteFailed))?;
        match outcome {
            DraftIdentityOutcome::Applied | DraftIdentityOutcome::NotFound => Ok(()),
            outcome => Err(identity_error(outcome)),
        }
    }
}

fn classify(outcome: DraftLoadOutcome) -> Result<ClippingNoteRecoveryResponse, ClippingError> {
    let (canonical_revision, checkpoint) = match outcome {
        DraftLoadOutcome::NotFound => return Err(recovery_error(ClippingErrorCode::NotFound)),
        DraftLoadOutcome::NotEditable => {
            return Err(recovery_error(ClippingErrorCode::NotEditable));
        }
        DraftLoadOutcome::Empty { canonical_revision } => {
            return Ok(ClippingNoteRecoveryResponse {
                status: ClippingNoteRecoveryStatus::None,
                canonical_revision,
                identity: None,
                draft: None,
            });
        }
        DraftLoadOutcome::Found {
            canonical_revision,
            checkpoint,
        } => (canonical_revision, checkpoint),
    };
    let identity = valid_checkpoint_identity(&checkpoint);
    if validate_stored(&checkpoint).is_err() || checkpoint.base_revision > canonical_revision {
        return Ok(ClippingNoteRecoveryResponse {
            status: ClippingNoteRecoveryStatus::Invalid,
            canonical_revision,
            identity,
            draft: None,
        });
    }
    let status = if checkpoint.base_revision == canonical_revision {
        ClippingNoteRecoveryStatus::Matching
    } else {
        ClippingNoteRecoveryStatus::CanonicalChanged
    };
    Ok(ClippingNoteRecoveryResponse {
        status,
        canonical_revision,
        identity,
        draft: Some(RecoveredClippingNoteDraft {
            base_revision: checkpoint.base_revision,
            title: checkpoint.title,
            markdown: checkpoint.markdown,
            updated_at: checkpoint.updated_at,
        }),
    })
}

fn validate_request(request: &CheckpointClippingNoteRequest) -> Result<(), ClippingError> {
    validate_document_id(&request.clipping_id)?;
    validate_identity(&request.writer_session_id, request.writer_sequence)?;
    if request.base_revision == 0 || request.base_revision > i64::MAX as u64 {
        return Err(recovery_error(ClippingErrorCode::RecoveryInvalid));
    }
    validate_content(&request.title, &request.markdown)
}

fn validate_stored(checkpoint: &DraftCheckpoint) -> Result<(), ClippingError> {
    validate_document_id(&checkpoint.clipping_id)?;
    validate_identity(&checkpoint.writer_session_id, checkpoint.writer_sequence)?;
    if checkpoint.base_revision == 0 || checkpoint.updated_at < 0 {
        return Err(recovery_error(ClippingErrorCode::RecoveryInvalid));
    }
    validate_content(&checkpoint.title, &checkpoint.markdown)
}

fn validate_content(title: &str, markdown: &str) -> Result<(), ClippingError> {
    if title.contains('\0') || markdown.contains('\0') {
        return Err(recovery_error(ClippingErrorCode::RecoveryInvalid));
    }
    if title.len() > RECOVERY_TITLE_MAX_BYTES || markdown.len() > RECOVERY_MARKDOWN_MAX_BYTES {
        return Err(recovery_error(ClippingErrorCode::RecoveryTooLarge));
    }
    Ok(())
}

fn validate_document_id(clipping_id: &str) -> Result<(), ClippingError> {
    if validate_clipping_id(clipping_id) {
        Ok(())
    } else {
        Err(recovery_error(ClippingErrorCode::InvalidId))
    }
}

fn validate_identity(writer_session_id: &str, writer_sequence: u64) -> Result<(), ClippingError> {
    if validate_clipping_id(writer_session_id)
        && writer_sequence > 0
        && writer_sequence <= i64::MAX as u64
    {
        Ok(())
    } else {
        Err(recovery_error(ClippingErrorCode::RecoveryInvalid))
    }
}

fn valid_checkpoint_identity(checkpoint: &DraftCheckpoint) -> Option<ClippingNoteCheckpointAck> {
    validate_identity(&checkpoint.writer_session_id, checkpoint.writer_sequence)
        .ok()
        .map(|()| ClippingNoteCheckpointAck {
            clipping_id: checkpoint.clipping_id.clone(),
            writer_session_id: checkpoint.writer_session_id.clone(),
            writer_sequence: checkpoint.writer_sequence,
        })
}

fn request_ack(request: &CheckpointClippingNoteRequest) -> ClippingNoteCheckpointAck {
    ClippingNoteCheckpointAck {
        clipping_id: request.clipping_id.clone(),
        writer_session_id: request.writer_session_id.clone(),
        writer_sequence: request.writer_sequence,
    }
}

fn identity_error(outcome: DraftIdentityOutcome) -> ClippingError {
    recovery_error(match outcome {
        DraftIdentityOutcome::WriterConflict => ClippingErrorCode::RecoveryWriterConflict,
        DraftIdentityOutcome::StaleSequence => ClippingErrorCode::RecoveryStaleSequence,
        DraftIdentityOutcome::NotFound => ClippingErrorCode::NotFound,
        DraftIdentityOutcome::Applied => ClippingErrorCode::RecoveryFailed,
    })
}

fn recovery_error(code: ClippingErrorCode) -> ClippingError {
    ClippingError::new(code)
}

fn context(operation: &'static str) -> DatabaseWriteContext {
    DatabaseWriteContext {
        operation,
        provider: DatabaseProvider::Newspaper,
        workflow_id: None,
    }
}

impl ClippingService {
    pub fn draft_service(&self) -> ClippingDraftService {
        ClippingDraftService::new(self.db_path.clone(), self.writer.clone())
    }
}
