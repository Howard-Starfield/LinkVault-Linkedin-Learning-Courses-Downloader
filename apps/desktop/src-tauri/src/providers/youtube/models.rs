use crate::workflow::transient::TransientRunSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum YouTubePlaylistMode {
    Video,
    Playlist,
}

impl Default for YouTubePlaylistMode {
    fn default() -> Self {
        Self::Video
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum YouTubeDownloadMode {
    VideoAndTranscript,
    VideoOnly,
    TranscriptOnly,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum YouTubeHelperBackendStatus {
    Ready,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetYouTubeHelperStatusResponse {
    pub status: YouTubeHelperBackendStatus,
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanYouTubeSourceRequest {
    pub client_operation_id: String,
    pub url: String,
    pub playlist_mode: Option<YouTubePlaylistMode>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeScanItem {
    pub occurrence_id: String,
    pub video_id: String,
    pub source_url: String,
    pub title: String,
    pub ordinal: u32,
    pub channel_name: Option<String>,
    pub channel_id: Option<String>,
    pub duration_seconds: Option<u64>,
    pub thumbnail_available: bool,
    pub availability: YouTubeAvailability,
    pub metadata_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum YouTubeAvailability {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanYouTubeSourceResponse {
    pub scan_plan_id: String,
    pub expires_at: String,
    pub kind: YouTubePlaylistMode,
    pub title: String,
    pub source_id: String,
    pub canonical_url: String,
    pub playlist_id: Option<String>,
    pub truncated: bool,
    pub items: Vec<YouTubeScanItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InspectYouTubeTranscriptsRequest {
    pub client_operation_id: String,
    pub scan_plan_id: String,
    pub occurrence_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeTranscriptTrack {
    pub track_key: String,
    pub language_tag: String,
    pub display_language: String,
    pub source: YouTubeTranscriptSource,
    pub is_likely_translated: bool,
    pub formats: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum YouTubeTranscriptSource {
    Uploader,
    Automatic,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InspectYouTubeTranscriptsResponse {
    pub occurrences: Vec<YouTubeTranscriptOccurrence>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeTranscriptOccurrence {
    pub occurrence_id: String,
    pub video_id: String,
    pub tracks: Vec<YouTubeTranscriptTrack>,
}

/// Plan-owned transcript inspection state.  This is deliberately separate
/// from the public IPC response so a later command can prove that a selected
/// track belongs to the same immutable occurrence and source snapshot that
/// was inspected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct YouTubeTranscriptInspectionContext {
    pub source_snapshot_digest: String,
    pub occurrence_id: String,
    pub video_id: String,
    pub metadata_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedYouTubeTranscriptInspection {
    pub context: YouTubeTranscriptInspectionContext,
    pub tracks: Vec<YouTubeTranscriptTrack>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartYouTubeDownloadRequest {
    pub client_submission_id: String,
    pub scan_plan_id: String,
    pub selected_occurrence_ids: Vec<String>,
    pub output_dir: String,
    pub mode: YouTubeDownloadMode,
    pub max_height: Option<u16>,
    pub preferred_language: Option<String>,
    pub fallback_languages: Vec<String>,
    pub allow_automatic_captions: bool,
    pub continue_without_transcript: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartYouTubeDownloadResponse {
    pub client_submission_id: String,
    pub run_id: String,
    pub revision: u64,
    pub scan_plan_id: String,
    pub plan_fingerprint: String,
    pub state: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetYouTubeDownloadStateRequest {
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutateYouTubeRunRequest {
    pub run_id: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CancelYouTubeRunRequest {
    pub run_id: String,
}

pub type GetYouTubeDownloadStateResponse = Option<TransientRunSnapshot>;
