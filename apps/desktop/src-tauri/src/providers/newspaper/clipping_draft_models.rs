//! Path-free Tauri transport models for clipping note recovery.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointClippingNoteRequest {
    pub clipping_id: String,
    pub base_revision: u64,
    pub writer_session_id: String,
    pub writer_sequence: u64,
    pub title: String,
    pub markdown: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoadClippingNoteRecoveryRequest {
    pub clipping_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimClippingNoteRecoveryRequest {
    pub clipping_id: String,
    pub prior_writer_session_id: String,
    pub prior_writer_sequence: u64,
    pub writer_session_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscardClippingNoteRecoveryRequest {
    pub clipping_id: String,
    pub writer_session_id: String,
    pub writer_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClippingNoteCheckpointAck {
    pub clipping_id: String,
    pub writer_session_id: String,
    pub writer_sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClippingNoteRecoveryStatus {
    None,
    Matching,
    CanonicalChanged,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveredClippingNoteDraft {
    pub base_revision: u64,
    pub title: String,
    pub markdown: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClippingNoteRecoveryResponse {
    pub status: ClippingNoteRecoveryStatus,
    pub canonical_revision: u64,
    pub identity: Option<ClippingNoteCheckpointAck>,
    pub draft: Option<RecoveredClippingNoteDraft>,
}
