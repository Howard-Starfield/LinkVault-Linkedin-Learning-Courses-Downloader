//! Pure identity and manifest contracts for YouTube item reuse.
//!
//! This module deliberately has no filesystem, helper, network, or workflow
//! dependencies. It defines the typed projections that a later safe-output
//! integration can use for exact manifest compatibility.

use crate::providers::youtube::models::{
    YouTubeDownloadMode, YouTubeTranscriptSource, YouTubeTranscriptTrack,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use thiserror::Error;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const FORMAT_POLICY_VERSION: u32 = 1;
pub const YOUTUBE_PROVIDER: &str = "youtube";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSnapshot {
    pub source_id: String,
    pub canonical_url: String,
    pub playlist_id: Option<String>,
    pub occurrences: Vec<SourceOccurrence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceOccurrence {
    pub occurrence_id: String,
    pub video_id: String,
    pub playlist_index: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceSnapshotProjection<'a> {
    schema_version: u32,
    source_id: &'a str,
    canonical_url: &'a str,
    playlist_id: Option<&'a str>,
    occurrences: &'a [SourceOccurrence],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedTranscript {
    pub track_key: String,
    pub language_tag: String,
    pub source: YouTubeTranscriptSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactFingerprintInput {
    pub occurrence_id: String,
    pub video_id: String,
    pub mode: YouTubeDownloadMode,
    pub format_policy_version: u32,
    pub max_height: Option<u16>,
    pub selected_transcript: Option<SelectedTranscript>,
    pub helper_lock_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactFingerprintProjection<'a> {
    schema_version: u32,
    occurrence_id: &'a str,
    video_id: &'a str,
    mode: &'a YouTubeDownloadMode,
    format_policy_version: u32,
    max_height: Option<u16>,
    selected_transcript: Option<&'a SelectedTranscript>,
    helper_lock_digest: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestArtifactKind {
    Media,
    Vtt,
    TranscriptJson,
    Metadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestArtifact {
    pub kind: ManifestArtifactKind,
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStatus {
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct YouTubeArtifactManifest {
    pub schema_version: u32,
    pub provider: String,
    pub source_snapshot_digest: String,
    pub artifact_fingerprint: String,
    pub occurrence_id: String,
    pub video_id: String,
    pub playlist_id: Option<String>,
    pub playlist_index: Option<u32>,
    pub mode: YouTubeDownloadMode,
    pub format_policy_version: u32,
    pub max_height: Option<u16>,
    pub selected_transcript: Option<SelectedTranscript>,
    pub helper_lock_digest: String,
    pub artifacts: Vec<ManifestArtifact>,
    pub status: ManifestStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestProjectionInput {
    pub source_snapshot_digest: String,
    pub artifact_fingerprint: String,
    pub occurrence_id: String,
    pub video_id: String,
    pub playlist_id: Option<String>,
    pub playlist_index: Option<u32>,
    pub mode: YouTubeDownloadMode,
    pub format_policy_version: u32,
    pub max_height: Option<u16>,
    pub selected_transcript: Option<SelectedTranscript>,
    pub helper_lock_digest: String,
    pub artifacts: Vec<ManifestArtifact>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestContractError {
    #[error("{field} must be a non-empty value")]
    EmptyField { field: &'static str },
    #[error("{field} must be a lowercase SHA-256 digest")]
    InvalidDigest { field: &'static str },
    #[error("manifest artifact path is not a safe relative path: {path}")]
    InvalidArtifactPath { path: String },
    #[error("manifest artifact has zero size: {path}")]
    EmptyArtifact { path: String },
    #[error("manifest contains duplicate artifact path: {path}")]
    DuplicateArtifactPath { path: String },
    #[error("manifest selected an invalid transcript track: {track_key}")]
    InvalidTranscriptTrack { track_key: String },
    #[error("manifest artifact fingerprint does not match its identity projection")]
    ArtifactFingerprintMismatch { expected: String, actual: String },
    #[error("{mode} mode cannot carry a selected transcript")]
    ModeTranscriptInvariant { mode: &'static str },
    #[error("transcript-only mode cannot carry a video height cap")]
    TranscriptOnlyHeightInvariant,
    #[error("playlist index must be one-based: {index}")]
    InvalidPlaylistIndex { index: u32 },
    #[error("unsupported format policy version: {version}")]
    UnsupportedFormatPolicyVersion { version: u32 },
    #[error("manifest serialization failed: {message}")]
    Serialization { message: String },
    #[error("source snapshot contains duplicate occurrence ID: {occurrence_id}")]
    DuplicateOccurrence { occurrence_id: String },
}

/// Hashes the versioned canonical source identity and ordered occurrence
/// projection. Display metadata, scan-plan IDs, and run IDs are not included.
pub fn source_snapshot_digest(snapshot: &SourceSnapshot) -> Result<String, ManifestContractError> {
    require_non_empty("sourceId", &snapshot.source_id)?;
    require_non_empty("canonicalUrl", &snapshot.canonical_url)?;
    if snapshot.occurrences.is_empty() {
        return Err(ManifestContractError::EmptyField {
            field: "occurrences",
        });
    }
    let mut occurrence_ids = HashSet::with_capacity(snapshot.occurrences.len());
    for occurrence in &snapshot.occurrences {
        require_non_empty("occurrenceId", &occurrence.occurrence_id)?;
        require_non_empty("videoId", &occurrence.video_id)?;
        validate_playlist_index(occurrence.playlist_index)?;
        if !occurrence_ids.insert(&occurrence.occurrence_id) {
            return Err(ManifestContractError::DuplicateOccurrence {
                occurrence_id: occurrence.occurrence_id.clone(),
            });
        }
    }
    let projection = SourceSnapshotProjection {
        schema_version: MANIFEST_SCHEMA_VERSION,
        source_id: &snapshot.source_id,
        canonical_url: &snapshot.canonical_url,
        playlist_id: snapshot.playlist_id.as_deref(),
        occurrences: &snapshot.occurrences,
    };
    canonical_digest(&projection)
}

/// Hashes the stable item identity projection used for exact reuse.
///
/// The input intentionally has no output root, run ID, scan-plan ID, display
/// metadata, or whole-run selection/order. The helper lock digest is supplied
/// by the verified helper boundary; this function never invents one.
pub fn artifact_fingerprint(
    input: &ArtifactFingerprintInput,
) -> Result<String, ManifestContractError> {
    require_non_empty("occurrenceId", &input.occurrence_id)?;
    require_non_empty("videoId", &input.video_id)?;
    validate_format_policy_version(input.format_policy_version)?;
    validate_digest("helperLockDigest", &input.helper_lock_digest)?;
    validate_selected_transcript(input.selected_transcript.as_ref())?;
    validate_mode_invariants(
        &input.mode,
        input.selected_transcript.as_ref(),
        input.max_height,
    )?;
    let projection = ArtifactFingerprintProjection {
        schema_version: MANIFEST_SCHEMA_VERSION,
        occurrence_id: &input.occurrence_id,
        video_id: &input.video_id,
        mode: &input.mode,
        format_policy_version: input.format_policy_version,
        max_height: input.max_height,
        selected_transcript: input.selected_transcript.as_ref(),
        helper_lock_digest: &input.helper_lock_digest,
    };
    canonical_digest(&projection)
}

/// Selects one semantic transcript track for one occurrence.
///
/// Language matching is normalized only for comparison (trim, `_` to `-`,
/// ASCII lowercase) and remains exact after normalization. The caller passes
/// the parser-identified `live_chat` keys explicitly because the public track
/// model intentionally carries no subtitle URL or provider-private category.
pub fn select_transcript(
    tracks: &[YouTubeTranscriptTrack],
    preferred_language: Option<&str>,
    fallback_languages: &[String],
    allow_automatic_captions: bool,
    live_chat_track_keys: &[String],
) -> Option<SelectedTranscript> {
    let mut languages = Vec::with_capacity(1 + fallback_languages.len());
    if let Some(preferred) = preferred_language.map(normalize_language) {
        if !preferred.is_empty() {
            languages.push(preferred);
        }
    }
    for fallback in fallback_languages {
        let normalized = normalize_language(fallback);
        if !normalized.is_empty() && !languages.contains(&normalized) {
            languages.push(normalized);
        }
    }

    for language in languages {
        let mut candidates = tracks
            .iter()
            .filter(|track| normalize_language(&track.language_tag) == language)
            .filter(|track| {
                !live_chat_track_keys
                    .iter()
                    .any(|key| key == &track.track_key)
            })
            .filter(|track| {
                allow_automatic_captions || track.source == YouTubeTranscriptSource::Uploader
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| transcript_order(left, right));
        if let Some(track) = candidates.first() {
            return Some(SelectedTranscript {
                track_key: track.track_key.clone(),
                language_tag: track.language_tag.clone(),
                source: track.source.clone(),
            });
        }
    }
    None
}

/// Builds the strict verified manifest projection consumed by the later
/// handle-safe output/reuse layer.
pub fn project_manifest(
    input: ManifestProjectionInput,
) -> Result<YouTubeArtifactManifest, ManifestContractError> {
    validate_digest("sourceSnapshotDigest", &input.source_snapshot_digest)?;
    validate_digest("artifactFingerprint", &input.artifact_fingerprint)?;
    validate_digest("helperLockDigest", &input.helper_lock_digest)?;
    require_non_empty("occurrenceId", &input.occurrence_id)?;
    require_non_empty("videoId", &input.video_id)?;
    validate_format_policy_version(input.format_policy_version)?;
    validate_playlist_index(input.playlist_index)?;
    validate_selected_transcript(input.selected_transcript.as_ref())?;
    validate_mode_invariants(
        &input.mode,
        input.selected_transcript.as_ref(),
        input.max_height,
    )?;

    let expected_fingerprint = artifact_fingerprint(&ArtifactFingerprintInput {
        occurrence_id: input.occurrence_id.clone(),
        video_id: input.video_id.clone(),
        mode: input.mode.clone(),
        format_policy_version: input.format_policy_version,
        max_height: input.max_height,
        selected_transcript: input.selected_transcript.clone(),
        helper_lock_digest: input.helper_lock_digest.clone(),
    })?;
    if expected_fingerprint != input.artifact_fingerprint {
        return Err(ManifestContractError::ArtifactFingerprintMismatch {
            expected: expected_fingerprint,
            actual: input.artifact_fingerprint,
        });
    }

    let mut paths = HashSet::with_capacity(input.artifacts.len());
    for artifact in &input.artifacts {
        validate_artifact(artifact)?;
        let normalized_path = artifact.relative_path.replace('\\', "/").to_lowercase();
        if !paths.insert(normalized_path) {
            return Err(ManifestContractError::DuplicateArtifactPath {
                path: artifact.relative_path.clone(),
            });
        }
    }
    if input.artifacts.is_empty() {
        return Err(ManifestContractError::EmptyField { field: "artifacts" });
    }

    Ok(YouTubeArtifactManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        provider: YOUTUBE_PROVIDER.to_string(),
        source_snapshot_digest: input.source_snapshot_digest,
        artifact_fingerprint: input.artifact_fingerprint,
        occurrence_id: input.occurrence_id,
        video_id: input.video_id,
        playlist_id: input.playlist_id,
        playlist_index: input.playlist_index,
        mode: input.mode,
        format_policy_version: input.format_policy_version,
        max_height: input.max_height,
        selected_transcript: input.selected_transcript,
        helper_lock_digest: input.helper_lock_digest,
        artifacts: input.artifacts,
        status: ManifestStatus::Verified,
    })
}

/// Serializes a typed manifest with the same canonical key ordering used by
/// identity digests. This is useful for compatibility checks and tests; it
/// does not perform filesystem reads or artifact verification.
pub fn canonical_manifest_bytes(
    manifest: &YouTubeArtifactManifest,
) -> Result<Vec<u8>, ManifestContractError> {
    canonical_bytes(manifest)
}

fn validate_artifact(artifact: &ManifestArtifact) -> Result<(), ManifestContractError> {
    validate_relative_artifact_path(&artifact.relative_path)?;
    if artifact.size_bytes == 0 {
        return Err(ManifestContractError::EmptyArtifact {
            path: artifact.relative_path.clone(),
        });
    }
    validate_digest("artifactSha256", &artifact.sha256)
}

fn validate_selected_transcript(
    selected: Option<&SelectedTranscript>,
) -> Result<(), ManifestContractError> {
    if let Some(selected) = selected {
        require_non_empty("trackKey", &selected.track_key)?;
        require_non_empty("languageTag", &selected.language_tag)?;
        if selected.track_key.eq_ignore_ascii_case("live_chat") {
            return Err(ManifestContractError::InvalidTranscriptTrack {
                track_key: selected.track_key.clone(),
            });
        }
    }
    Ok(())
}

fn validate_mode_invariants(
    mode: &YouTubeDownloadMode,
    selected_transcript: Option<&SelectedTranscript>,
    max_height: Option<u16>,
) -> Result<(), ManifestContractError> {
    match mode {
        YouTubeDownloadMode::VideoOnly if selected_transcript.is_some() => {
            Err(ManifestContractError::ModeTranscriptInvariant { mode: "video_only" })
        }
        YouTubeDownloadMode::TranscriptOnly if max_height.is_some() => {
            Err(ManifestContractError::TranscriptOnlyHeightInvariant)
        }
        _ => Ok(()),
    }
}

fn validate_playlist_index(index: Option<u32>) -> Result<(), ManifestContractError> {
    if index == Some(0) {
        return Err(ManifestContractError::InvalidPlaylistIndex { index: 0 });
    }
    Ok(())
}

fn validate_format_policy_version(version: u32) -> Result<(), ManifestContractError> {
    if version != FORMAT_POLICY_VERSION {
        return Err(ManifestContractError::UnsupportedFormatPolicyVersion { version });
    }
    Ok(())
}

fn validate_relative_artifact_path(path: &str) -> Result<(), ManifestContractError> {
    if path.is_empty()
        || path.encode_utf16().count() > 240
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains(':')
        || path.chars().any(char::is_control)
        || path.chars().any(is_windows_forbidden_character)
    {
        return Err(ManifestContractError::InvalidArtifactPath {
            path: path.to_string(),
        });
    }
    for component in path.split(['/', '\\']) {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with('.')
            || component.ends_with(' ')
            || is_windows_reserved_name(component)
        {
            return Err(ManifestContractError::InvalidArtifactPath {
                path: path.to_string(),
            });
        }
    }
    Ok(())
}

fn is_windows_forbidden_character(character: char) -> bool {
    matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
}

fn is_windows_reserved_name(name: &str) -> bool {
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base[3..]
                .chars()
                .all(|character| character.is_ascii_digit())
            && base[3..]
                .parse::<u8>()
                .is_ok_and(|number| (1..=9).contains(&number)))
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), ManifestContractError> {
    if value.len() != 64
        || value != value.to_ascii_lowercase()
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(ManifestContractError::InvalidDigest { field });
    }
    Ok(())
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ManifestContractError> {
    if value.is_empty() {
        Err(ManifestContractError::EmptyField { field })
    } else {
        Ok(())
    }
}

fn normalize_language(value: &str) -> String {
    value.trim().replace('_', "-").to_ascii_lowercase()
}

fn transcript_order(
    left: &YouTubeTranscriptTrack,
    right: &YouTubeTranscriptTrack,
) -> std::cmp::Ordering {
    transcript_source_rank(left)
        .cmp(&transcript_source_rank(right))
        .then_with(|| left.is_likely_translated.cmp(&right.is_likely_translated))
        .then_with(|| vtt_rank(left).cmp(&vtt_rank(right)))
        .then_with(|| left.track_key.cmp(&right.track_key))
}

fn transcript_source_rank(track: &YouTubeTranscriptTrack) -> u8 {
    match track.source {
        YouTubeTranscriptSource::Uploader => 0,
        YouTubeTranscriptSource::Automatic => 1,
    }
}

fn vtt_rank(track: &YouTubeTranscriptTrack) -> u8 {
    if track
        .formats
        .iter()
        .any(|format| format.eq_ignore_ascii_case("vtt"))
    {
        0
    } else {
        1
    }
}

fn canonical_digest<T: Serialize>(value: &T) -> Result<String, ManifestContractError> {
    Ok(format!("{:x}", Sha256::digest(canonical_bytes(value)?)))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ManifestContractError> {
    let value =
        serde_json::to_value(value).map_err(|error| ManifestContractError::Serialization {
            message: error.to_string(),
        })?;
    let mut output = String::new();
    write_canonical_json(&value, &mut output)?;
    Ok(output.into_bytes())
}

fn write_canonical_json(value: &Value, output: &mut String) -> Result<(), ManifestContractError> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            let encoded = serde_json::to_string(value).map_err(|error| {
                ManifestContractError::Serialization {
                    message: error.to_string(),
                }
            })?;
            output.push_str(&encoded);
        }
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            output.push('{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let encoded_key = serde_json::to_string(key).map_err(|error| {
                    ManifestContractError::Serialization {
                        message: error.to_string(),
                    }
                })?;
                output.push_str(&encoded_key);
                output.push(':');
                write_canonical_json(value, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn digest_value(character: char) -> String {
        std::iter::repeat(character).take(64).collect()
    }

    fn source_snapshot() -> SourceSnapshot {
        SourceSnapshot {
            source_id: "playlist-1".to_string(),
            canonical_url: "https://www.youtube.com/playlist?list=playlist-1".to_string(),
            playlist_id: Some("playlist-1".to_string()),
            occurrences: vec![
                SourceOccurrence {
                    occurrence_id: "occurrence-1".to_string(),
                    video_id: "video-1".to_string(),
                    playlist_index: Some(1),
                },
                SourceOccurrence {
                    occurrence_id: "occurrence-2".to_string(),
                    video_id: "video-2".to_string(),
                    playlist_index: Some(2),
                },
            ],
        }
    }

    fn track(
        key: &str,
        language: &str,
        source: YouTubeTranscriptSource,
        translated: bool,
        formats: &[&str],
    ) -> YouTubeTranscriptTrack {
        YouTubeTranscriptTrack {
            track_key: key.to_string(),
            language_tag: language.to_string(),
            display_language: language.to_string(),
            source,
            is_likely_translated: translated,
            formats: formats.iter().map(|format| (*format).to_string()).collect(),
        }
    }

    fn manifest_input() -> ManifestProjectionInput {
        let selected_transcript = Some(SelectedTranscript {
            track_key: "track-en".to_string(),
            language_tag: "en".to_string(),
            source: YouTubeTranscriptSource::Uploader,
        });
        let helper_lock_digest = digest_value('c');
        let artifact_fingerprint = artifact_fingerprint(&ArtifactFingerprintInput {
            occurrence_id: "occurrence-1".to_string(),
            video_id: "video-1".to_string(),
            mode: YouTubeDownloadMode::VideoAndTranscript,
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: Some(1080),
            selected_transcript: selected_transcript.clone(),
            helper_lock_digest: helper_lock_digest.clone(),
        })
        .unwrap();
        ManifestProjectionInput {
            source_snapshot_digest: digest_value('a'),
            artifact_fingerprint,
            occurrence_id: "occurrence-1".to_string(),
            video_id: "video-1".to_string(),
            playlist_id: Some("playlist-1".to_string()),
            playlist_index: Some(1),
            mode: YouTubeDownloadMode::VideoAndTranscript,
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: Some(1080),
            selected_transcript,
            helper_lock_digest,
            artifacts: vec![ManifestArtifact {
                kind: ManifestArtifactKind::Vtt,
                relative_path: "video-1.en.vtt".to_string(),
                size_bytes: 4,
                sha256: digest_value('d'),
            }],
        }
    }

    #[test]
    fn source_digest_is_canonical_and_order_sensitive() {
        let snapshot = source_snapshot();
        let digest = source_snapshot_digest(&snapshot).unwrap();
        let expected_bytes = br#"{"canonicalUrl":"https://www.youtube.com/playlist?list=playlist-1","occurrences":[{"occurrenceId":"occurrence-1","playlistIndex":1,"videoId":"video-1"},{"occurrenceId":"occurrence-2","playlistIndex":2,"videoId":"video-2"}],"playlistId":"playlist-1","schemaVersion":1,"sourceId":"playlist-1"}"#;
        let expected = format!("{:x}", Sha256::digest(expected_bytes));
        assert_eq!(digest, expected);

        let mut reordered = snapshot;
        reordered.occurrences.reverse();
        assert_ne!(digest, source_snapshot_digest(&reordered).unwrap());
    }

    #[test]
    fn source_digest_rejects_duplicate_occurrences() {
        let mut snapshot = source_snapshot();
        snapshot.occurrences[1].occurrence_id = snapshot.occurrences[0].occurrence_id.clone();
        assert!(matches!(
            source_snapshot_digest(&snapshot),
            Err(ManifestContractError::DuplicateOccurrence { .. })
        ));
    }

    #[test]
    fn source_digest_rejects_zero_based_playlist_index() {
        let mut snapshot = source_snapshot();
        snapshot.occurrences[0].playlist_index = Some(0);
        assert!(matches!(
            source_snapshot_digest(&snapshot),
            Err(ManifestContractError::InvalidPlaylistIndex { index: 0 })
        ));
    }

    #[test]
    fn artifact_fingerprint_changes_only_for_identity_or_effective_options() {
        let input = ArtifactFingerprintInput {
            occurrence_id: "occurrence-1".to_string(),
            video_id: "video-1".to_string(),
            mode: YouTubeDownloadMode::VideoOnly,
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: Some(1080),
            selected_transcript: None,
            helper_lock_digest: digest_value('a'),
        };
        let baseline = artifact_fingerprint(&input).unwrap();
        let mut changed = input.clone();
        changed.max_height = Some(720);
        assert_ne!(baseline, artifact_fingerprint(&changed).unwrap());
        changed.max_height = input.max_height;
        changed.video_id = "video-2".to_string();
        assert_ne!(baseline, artifact_fingerprint(&changed).unwrap());
    }

    #[test]
    fn transcript_selection_applies_language_source_and_tie_breakers() {
        let tracks = vec![
            track(
                "automatic-en",
                "EN_us",
                YouTubeTranscriptSource::Automatic,
                false,
                &["vtt"],
            ),
            track(
                "uploader-translated",
                "en-US",
                YouTubeTranscriptSource::Uploader,
                true,
                &["xml"],
            ),
            track(
                "uploader-en",
                "en-us",
                YouTubeTranscriptSource::Uploader,
                false,
                &["vtt"],
            ),
            track(
                "live_chat",
                "en-US",
                YouTubeTranscriptSource::Uploader,
                false,
                &["vtt"],
            ),
            track(
                "fallback-ja",
                "ja",
                YouTubeTranscriptSource::Uploader,
                false,
                &["vtt"],
            ),
        ];
        let selected = select_transcript(
            &tracks,
            Some(" en_us "),
            &["ja".to_string()],
            true,
            &["live_chat".to_string()],
        )
        .unwrap();
        assert_eq!(selected.track_key, "uploader-en");
        assert_eq!(selected.language_tag, "en-us");
        assert_eq!(selected.source, YouTubeTranscriptSource::Uploader);

        let selected = select_transcript(
            &tracks,
            Some("fr"),
            &["ja".to_string()],
            false,
            &["live_chat".to_string()],
        )
        .unwrap();
        assert_eq!(selected.track_key, "fallback-ja");
    }

    #[test]
    fn transcript_selection_returns_none_when_only_automatic_is_disallowed() {
        let tracks = vec![track(
            "automatic-en",
            "en",
            YouTubeTranscriptSource::Automatic,
            false,
            &["vtt"],
        )];
        assert!(select_transcript(&tracks, Some("en"), &[], false, &[]).is_none());
    }

    #[test]
    fn manifest_projection_is_strict_and_canonical() {
        let manifest = project_manifest(manifest_input()).unwrap();
        assert_eq!(manifest.provider, YOUTUBE_PROVIDER);
        let bytes = canonical_manifest_bytes(&manifest).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\"sourceSnapshotDigest\""));
        assert!(text.contains("\"playlistIndex\":1"));
        assert!(text.contains("\"status\":\"verified\""));

        let round_trip: YouTubeArtifactManifest = serde_json::from_str(&text).unwrap();
        assert_eq!(round_trip, manifest);
        let mut unknown_field = serde_json::from_str::<Value>(&text).unwrap();
        unknown_field
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_string(), Value::Bool(true));
        assert!(serde_json::from_value::<YouTubeArtifactManifest>(unknown_field).is_err());
    }

    #[test]
    fn manifest_projection_rejects_invalid_digest_and_duplicate_path() {
        let mut input = manifest_input();
        input.helper_lock_digest = "not-a-digest".to_string();
        assert!(matches!(
            project_manifest(input),
            Err(ManifestContractError::InvalidDigest {
                field: "helperLockDigest"
            })
        ));

        let mut input = manifest_input();
        input.artifacts.push(input.artifacts[0].clone());
        assert!(matches!(
            project_manifest(input),
            Err(ManifestContractError::DuplicateArtifactPath { .. })
        ));
    }

    #[test]
    fn manifest_projection_recomputes_and_checks_artifact_fingerprint() {
        let mut input = manifest_input();
        input.artifact_fingerprint = digest_value('b');
        assert!(matches!(
            project_manifest(input),
            Err(ManifestContractError::ArtifactFingerprintMismatch { .. })
        ));
    }

    #[test]
    fn manifest_projection_enforces_mode_and_playlist_index_invariants() {
        let mut video_only = manifest_input();
        video_only.mode = YouTubeDownloadMode::VideoOnly;
        assert!(matches!(
            project_manifest(video_only),
            Err(ManifestContractError::ModeTranscriptInvariant { mode: "video_only" })
        ));

        let mut transcript_only = manifest_input();
        transcript_only.mode = YouTubeDownloadMode::TranscriptOnly;
        assert!(matches!(
            project_manifest(transcript_only),
            Err(ManifestContractError::TranscriptOnlyHeightInvariant)
        ));

        let mut zero_based = manifest_input();
        zero_based.playlist_index = Some(0);
        assert!(matches!(
            project_manifest(zero_based),
            Err(ManifestContractError::InvalidPlaylistIndex { index: 0 })
        ));
    }

    #[test]
    fn manifest_projection_applies_windows_artifact_path_rules() {
        for path in [
            "CON.txt",
            "LPT9.log",
            "trailing.",
            "trailing ",
            "nested/../escape.txt",
            "nested\\..\\escape.txt",
            "C:stream.txt",
            "angle<.txt",
            "angle>.txt",
            "quote\".txt",
            "pipe|.txt",
            "question?.txt",
            "star*.txt",
            "absolute.txt\u{0007}",
        ] {
            let mut input = manifest_input();
            input.artifacts[0].relative_path = path.to_string();
            assert!(
                matches!(
                    project_manifest(input),
                    Err(ManifestContractError::InvalidArtifactPath { .. })
                ),
                "accepted unsafe path {path:?}"
            );
        }

        let mut too_long = manifest_input();
        too_long.artifacts[0].relative_path = std::iter::repeat('a').take(241).collect();
        assert!(matches!(
            project_manifest(too_long),
            Err(ManifestContractError::InvalidArtifactPath { .. })
        ));
    }

    #[test]
    fn format_policy_version_is_v1_only() {
        let selected_transcript = Some(SelectedTranscript {
            track_key: "track-en".to_string(),
            language_tag: "en".to_string(),
            source: YouTubeTranscriptSource::Uploader,
        });
        let input = ArtifactFingerprintInput {
            occurrence_id: "occurrence-1".to_string(),
            video_id: "video-1".to_string(),
            mode: YouTubeDownloadMode::VideoAndTranscript,
            format_policy_version: FORMAT_POLICY_VERSION + 1,
            max_height: Some(1080),
            selected_transcript,
            helper_lock_digest: digest_value('a'),
        };
        assert!(matches!(
            artifact_fingerprint(&input),
            Err(ManifestContractError::UnsupportedFormatPolicyVersion { version: 2 })
        ));

        let mut manifest = manifest_input();
        manifest.format_policy_version = FORMAT_POLICY_VERSION + 1;
        assert!(matches!(
            project_manifest(manifest),
            Err(ManifestContractError::UnsupportedFormatPolicyVersion { version: 2 })
        ));
    }

    #[test]
    fn manifest_projection_rejects_case_insensitive_and_separator_aliases() {
        let mut input = manifest_input();
        input.artifacts[0].relative_path = "video-1/en.vtt".to_string();
        input.artifacts.push(ManifestArtifact {
            kind: ManifestArtifactKind::Metadata,
            relative_path: "VIDEO-1\\EN.VTT".to_string(),
            size_bytes: 5,
            sha256: digest_value('e'),
        });
        assert!(matches!(
            project_manifest(input),
            Err(ManifestContractError::DuplicateArtifactPath { .. })
        ));
    }

    #[test]
    fn canonical_serializer_sorts_nested_object_keys() {
        let mut object = Map::new();
        object.insert("z".to_string(), Value::String("last".to_string()));
        object.insert("a".to_string(), Value::String("first".to_string()));
        let value = Value::Object(object);
        assert_eq!(
            canonical_bytes(&value).unwrap(),
            br#"{"a":"first","z":"last"}"#
        );
    }
}
