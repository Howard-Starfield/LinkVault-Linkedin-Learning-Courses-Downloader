//! Bounded transcript metadata inspection for one YouTube occurrence.
//!
//! The helper returns provider metadata that may contain signed subtitle
//! URLs.  This module intentionally projects that response to language,
//! source, display metadata, and extension names only.  URLs, format IDs,
//! and every other provider-private field remain outside the plan and IPC
//! contracts.

use crate::providers::youtube::error::YouTubeInternalError;
use crate::providers::youtube::helper::{invocation, output_error, MAX_RECORD_STDOUT_BYTES};
use crate::providers::youtube::models::{
    PlannedYouTubeTranscriptInspection, YouTubeTranscriptInspectionContext,
    YouTubeTranscriptSource, YouTubeTranscriptTrack,
};
use crate::providers::youtube::scan::PlannedYouTubeItem;
use crate::workflow::transient::managed_process::{run, ManagedProcessOutput};
use crate::workflow::transient::DiscoveryOperation;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

/// Maximum number of language tracks retained for one occurrence.
pub const MAX_TRANSCRIPT_TRACKS: usize = 256;
/// Maximum number of extension names retained for one language/source track.
pub const MAX_TRANSCRIPT_FORMATS_PER_TRACK: usize = 32;
/// Maximum bytes accepted for one normalized language tag.
pub const MAX_TRANSCRIPT_LANGUAGE_BYTES: usize = 128;
/// Maximum bytes retained from a provider display-language label.
pub const MAX_TRANSCRIPT_DISPLAY_LANGUAGE_BYTES: usize = 256;
const MAX_FORMAT_BYTES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptInspectionResult {
    pub occurrence_id: String,
    pub video_id: String,
    pub tracks: Vec<YouTubeTranscriptTrack>,
}

/// Inspect one already-planned occurrence through the workflow-owned helper
/// boundary.  The source URL comes only from the immutable scan plan; callers
/// cannot supply an arbitrary URL here.
pub(crate) fn inspect_transcripts(
    item: &PlannedYouTubeItem,
    operation: &DiscoveryOperation,
) -> Result<TranscriptInspectionResult, YouTubeInternalError> {
    if operation.cancellation_requested() {
        return Err(YouTubeInternalError::InvalidRequest(
            "transcript inspection was cancelled".to_string(),
        ));
    }
    let output = run(
        invocation(inspection_args(item), MAX_RECORD_STDOUT_BYTES),
        None,
        operation.cancellation_flag().as_deref(),
    )
    .map_err(|error| YouTubeInternalError::Helper(error.to_string()))?;
    ensure_inspection_output(operation, &output)?;
    let tracks = parse_transcript_tracks(&item.public.occurrence_id, &output.stdout)?;
    Ok(TranscriptInspectionResult {
        occurrence_id: item.public.occurrence_id.clone(),
        video_id: item.public.video_id.clone(),
        tracks,
    })
}

/// Convert an inspection result into the plan-owned context used by later
/// commands.  The context binds the tracks to both the immutable source
/// snapshot and the normalized item metadata.
pub(crate) fn into_plan_inspection(
    result: TranscriptInspectionResult,
    source_snapshot_digest: &str,
    metadata_digest: &str,
) -> PlannedYouTubeTranscriptInspection {
    PlannedYouTubeTranscriptInspection {
        context: YouTubeTranscriptInspectionContext {
            source_snapshot_digest: source_snapshot_digest.to_string(),
            occurrence_id: result.occurrence_id,
            video_id: result.video_id,
            metadata_digest: metadata_digest.to_string(),
        },
        tracks: result.tracks,
    }
}

fn ensure_inspection_output(
    operation: &DiscoveryOperation,
    output: &ManagedProcessOutput,
) -> Result<(), YouTubeInternalError> {
    if operation.cancellation_requested() || output.cancelled {
        return Err(YouTubeInternalError::InvalidRequest(
            "transcript inspection was cancelled".to_string(),
        ));
    }
    if output.timed_out {
        return Err(YouTubeInternalError::Helper(
            "transcript inspection timed out".to_string(),
        ));
    }
    if output.stdout_truncated || output.stdout.len() > MAX_RECORD_STDOUT_BYTES {
        return Err(YouTubeInternalError::Helper(
            "transcript inspection output exceeded the safety limit".to_string(),
        ));
    }
    if !output.status.success() {
        return Err(YouTubeInternalError::Helper(output_error(output)));
    }
    Ok(())
}

fn inspection_args(item: &PlannedYouTubeItem) -> Vec<String> {
    vec![
        "--ignore-config".to_string(),
        "--no-plugin-dirs".to_string(),
        "--no-update".to_string(),
        "--no-warnings".to_string(),
        "--dump-single-json".to_string(),
        "--skip-download".to_string(),
        "--no-playlist".to_string(),
        item.source_url.clone(),
    ]
}

/// Parse one complete `--dump-single-json --skip-download --no-playlist`
/// result.  The parser accepts no JSON lines after the root object and does
/// not retain any signed subtitle URL.
pub(crate) fn parse_transcript_tracks(
    occurrence_id: &str,
    stdout: &str,
) -> Result<Vec<YouTubeTranscriptTrack>, YouTubeInternalError> {
    if occurrence_id.trim().is_empty() {
        return Err(YouTubeInternalError::InvalidRequest(
            "transcript occurrence identity is empty".to_string(),
        ));
    }
    if stdout.len() > MAX_RECORD_STDOUT_BYTES {
        return Err(YouTubeInternalError::Helper(
            "transcript inspection output exceeded the safety limit".to_string(),
        ));
    }
    let value: Value = serde_json::from_str(stdout).map_err(|_| {
        YouTubeInternalError::Helper(
            "yt-dlp returned malformed transcript inspection output".to_string(),
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        YouTubeInternalError::Helper(
            "yt-dlp transcript inspection result was not an object".to_string(),
        )
    })?;

    let mut tracks = Vec::new();
    append_source_tracks(
        occurrence_id,
        object,
        "subtitles",
        YouTubeTranscriptSource::Uploader,
        &mut tracks,
    )?;
    append_source_tracks(
        occurrence_id,
        object,
        "automatic_captions",
        YouTubeTranscriptSource::Automatic,
        &mut tracks,
    )?;
    deduplicate_tracks(occurrence_id, &mut tracks);
    tracks.sort_by(track_order);
    if tracks.len() > MAX_TRANSCRIPT_TRACKS {
        return Err(YouTubeInternalError::Helper(
            "transcript inspection returned too many tracks".to_string(),
        ));
    }
    Ok(tracks)
}

fn deduplicate_tracks(occurrence_id: &str, tracks: &mut Vec<YouTubeTranscriptTrack>) {
    tracks.sort_by(|left, right| {
        source_rank(&left.source)
            .cmp(&source_rank(&right.source))
            .then_with(|| left.language_tag.cmp(&right.language_tag))
            .then_with(|| left.track_key.cmp(&right.track_key))
    });
    let mut deduplicated: Vec<YouTubeTranscriptTrack> = Vec::with_capacity(tracks.len());
    for mut track in tracks.drain(..) {
        if let Some(existing) = deduplicated.iter_mut().find(|candidate| {
            candidate.language_tag == track.language_tag && candidate.source == track.source
        }) {
            existing.formats.append(&mut track.formats);
            existing.formats.sort();
            existing.formats.dedup();
            if track.display_language < existing.display_language {
                existing.display_language = track.display_language;
            }
            existing.is_likely_translated |= track.is_likely_translated;
        } else {
            deduplicated.push(track);
        }
    }
    for track in &mut deduplicated {
        track.track_key = opaque_track_key(
            occurrence_id,
            &track.language_tag,
            &track.source,
            &track.formats,
        );
    }
    *tracks = deduplicated;
}

fn append_source_tracks(
    occurrence_id: &str,
    root: &Map<String, Value>,
    field: &str,
    source: YouTubeTranscriptSource,
    tracks: &mut Vec<YouTubeTranscriptTrack>,
) -> Result<(), YouTubeInternalError> {
    let Some(value) = root.get(field) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let languages = value.as_object().ok_or_else(|| {
        YouTubeInternalError::Helper(format!("yt-dlp {field} metadata was malformed"))
    })?;
    if languages.len() > MAX_TRANSCRIPT_TRACKS {
        return Err(YouTubeInternalError::Helper(format!(
            "yt-dlp {field} metadata exceeded the track limit"
        )));
    }
    for (raw_language, formats) in languages {
        if is_live_chat(raw_language) {
            continue;
        }
        let Some(language_tag) = normalize_language_tag(raw_language) else {
            continue;
        };
        let format_entries = formats.as_array().ok_or_else(|| {
            YouTubeInternalError::Helper(format!("yt-dlp {field} track metadata was malformed"))
        })?;
        if format_entries.len() > MAX_TRANSCRIPT_FORMATS_PER_TRACK {
            return Err(YouTubeInternalError::Helper(format!(
                "yt-dlp {field} format metadata exceeded the track limit"
            )));
        }
        let mut format_names = Vec::new();
        let mut display_names = Vec::new();
        for entry in format_entries {
            let format = entry.as_object().ok_or_else(|| {
                YouTubeInternalError::Helper(format!(
                    "yt-dlp {field} format metadata was malformed"
                ))
            })?;
            if let Some(ext) = format.get("ext").and_then(Value::as_str) {
                if let Some(normalized) = normalize_format(ext) {
                    if !format_names.contains(&normalized) {
                        format_names.push(normalized);
                    }
                }
            }
            for key in ["name", "language_name"] {
                if let Some(name) = format.get(key).and_then(Value::as_str) {
                    if let Some(display) = normalize_display_language(name) {
                        if !display_names.contains(&display) {
                            display_names.push(display);
                        }
                    }
                }
            }
        }
        if format_names.is_empty() {
            continue;
        }
        format_names.sort();
        display_names.sort();
        let display_language = display_names
            .into_iter()
            .next()
            .unwrap_or_else(|| default_display_language(&language_tag));
        let is_likely_translated = likely_translated(raw_language, &display_language);
        let track_key = opaque_track_key(occurrence_id, &language_tag, &source, &format_names);
        tracks.push(YouTubeTranscriptTrack {
            track_key,
            language_tag,
            display_language,
            source: source.clone(),
            is_likely_translated,
            formats: format_names,
        });
        if tracks.len() > MAX_TRANSCRIPT_TRACKS {
            return Err(YouTubeInternalError::Helper(
                "transcript inspection returned too many tracks".to_string(),
            ));
        }
    }
    Ok(())
}

fn track_order(left: &YouTubeTranscriptTrack, right: &YouTubeTranscriptTrack) -> Ordering {
    source_rank(&left.source)
        .cmp(&source_rank(&right.source))
        .then_with(|| left.language_tag.cmp(&right.language_tag))
        .then_with(|| left.formats.cmp(&right.formats))
        .then_with(|| left.track_key.cmp(&right.track_key))
}

fn source_rank(source: &YouTubeTranscriptSource) -> u8 {
    match source {
        YouTubeTranscriptSource::Uploader => 0,
        YouTubeTranscriptSource::Automatic => 1,
    }
}

fn is_live_chat(language: &str) -> bool {
    language.trim().eq_ignore_ascii_case("live_chat")
}

fn normalize_language_tag(raw: &str) -> Option<String> {
    let value = raw.trim().replace('_', "-");
    if value.is_empty()
        || value.len() > MAX_TRANSCRIPT_LANGUAGE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-'))
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return None;
    }
    let mut parts = value.split('-');
    let first = parts.next()?.to_ascii_lowercase();
    let mut normalized = first;
    for part in parts {
        if part.is_empty() {
            return None;
        }
        let normalized_part =
            if part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                let mut chars = part.chars();
                let first = chars.next()?.to_ascii_uppercase();
                format!("{}{}", first, chars.as_str().to_ascii_lowercase())
            } else if (part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
                || (part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_digit()))
            {
                part.to_ascii_uppercase()
            } else {
                part.to_ascii_lowercase()
            };
        normalized.push('-');
        normalized.push_str(&normalized_part);
    }
    Some(normalized)
}

fn normalize_format(raw: &str) -> Option<String> {
    let value = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    if value.is_empty()
        || value.len() > MAX_FORMAT_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        None
    } else {
        Some(value)
    }
}

fn normalize_display_language(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > MAX_TRANSCRIPT_DISPLAY_LANGUAGE_BYTES
        || value.contains("://")
        || value.contains(':')
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_string())
}

fn default_display_language(language_tag: &str) -> String {
    let base = language_tag.split('-').next().unwrap_or(language_tag);
    match base {
        "ar" => "Arabic",
        "de" => "German",
        "en" => "English",
        "es" => "Spanish",
        "fr" => "French",
        "hi" => "Hindi",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "nl" => "Dutch",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "vi" => "Vietnamese",
        "zh" => "Chinese",
        _ => language_tag,
    }
    .to_string()
}

fn likely_translated(language: &str, display: &str) -> bool {
    let value = format!("{} {}", language, display).to_ascii_lowercase();
    [
        "translated",
        "translation",
        "auto-translate",
        "auto_translate",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn opaque_track_key(
    occurrence_id: &str,
    language_tag: &str,
    source: &YouTubeTranscriptSource,
    formats: &[String],
) -> String {
    let source = match source {
        YouTubeTranscriptSource::Uploader => "uploader",
        YouTubeTranscriptSource::Automatic => "automatic",
    };
    let mut hasher = Sha256::new();
    hasher.update([1]);
    for value in [occurrence_id, language_tag, source] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update((formats.len() as u64).to_be_bytes());
    for format in formats {
        hasher.update((format.len() as u64).to_be_bytes());
        hasher.update(format.as_bytes());
    }
    format!("track-{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OCCURRENCE: &str = "occurrence-123";

    #[test]
    fn parses_uploader_and_automatic_tracks_without_urls() {
        let payload = r#"{
            "subtitles": {
                "en": [{"ext":"vtt","url":"https://signed.example/uploader","name":"English"}],
                "fr": [{"ext":"srv3","url":"https://signed.example/french","name":"French"}]
            },
            "automatic_captions": {
                "en": [{"ext":"vtt","url":"https://signed.example/automatic","name":"English (auto-generated)"}]
            }
        }"#;
        let tracks = parse_transcript_tracks(OCCURRENCE, payload).unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].language_tag, "en");
        assert_eq!(tracks[0].source, YouTubeTranscriptSource::Uploader);
        assert_eq!(tracks[0].formats, vec!["vtt"]);
        assert_eq!(tracks[1].language_tag, "fr");
        assert_eq!(tracks[2].source, YouTubeTranscriptSource::Automatic);
        let serialized = serde_json::to_string(&tracks).unwrap();
        assert!(!serialized.contains("signed.example"));
        assert!(!serialized.contains("url"));
    }

    #[test]
    fn excludes_live_chat_and_keeps_stable_order_and_keys() {
        let first = r#"{
            "automatic_captions": {"live_chat":[{"ext":"vtt","url":"https://signed.example/chat"}],"es":[{"ext":"ttml","url":"https://signed.example/es"}]},
            "subtitles": {"en":[{"ext":"srv3","url":"https://signed.example/en"},{"ext":"vtt","url":"https://signed.example/en2"},{"ext":"vtt","url":"https://signed.example/en3"}]}
        }"#;
        let second = r#"{
            "subtitles": {"en":[{"ext":"vtt","url":"https://signed.example/en3"},{"ext":"srv3","url":"https://signed.example/en"}]},
            "automatic_captions": {"es":[{"ext":"ttml","url":"https://signed.example/es"}],"live_chat":[{"ext":"vtt","url":"https://signed.example/chat"}]}
        }"#;
        let left = parse_transcript_tracks(OCCURRENCE, first).unwrap();
        let right = parse_transcript_tracks(OCCURRENCE, second).unwrap();
        assert_eq!(left, right);
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|track| track.language_tag != "live_chat"));
        assert!(left[0].track_key.starts_with("track-"));
    }

    #[test]
    fn translated_marker_is_informational_only() {
        let payload = r#"{"automatic_captions":{"de-translated":[{"ext":"vtt","name":"German translated","url":"https://signed.example/de"}],"en":[{"ext":"vtt","name":"English (auto-generated)","url":"https://signed.example/en"}]}}"#;
        let tracks = parse_transcript_tracks(OCCURRENCE, payload).unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(tracks[0].is_likely_translated);
        assert!(!tracks[1].is_likely_translated);
    }

    #[test]
    fn absent_or_null_caption_blocks_are_empty() {
        assert!(parse_transcript_tracks(OCCURRENCE, "{}")
            .unwrap()
            .is_empty());
        assert!(parse_transcript_tracks(
            OCCURRENCE,
            r#"{"subtitles":null,"automatic_captions":null}"#
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn malformed_and_oversized_metadata_are_rejected() {
        assert!(parse_transcript_tracks(OCCURRENCE, "not-json").is_err());
        assert!(parse_transcript_tracks(OCCURRENCE, r#"{"subtitles":[]}"#).is_err());
        assert!(
            parse_transcript_tracks(OCCURRENCE, &"x".repeat(MAX_RECORD_STDOUT_BYTES + 1)).is_err()
        );
        assert!(parse_transcript_tracks(OCCURRENCE, r#"{} trailing"#).is_err());
    }

    #[test]
    fn duplicate_formats_are_collapsed_and_keys_bind_to_occurrence() {
        let payload = r#"{"subtitles":{"en":[{"ext":"vtt","url":"https://signed.example/a"},{"ext":"VTT","url":"https://signed.example/b"},{"ext":"srv3","url":"https://signed.example/c"}]}}"#;
        let one = parse_transcript_tracks("occurrence-one", payload).unwrap();
        let two = parse_transcript_tracks("occurrence-two", payload).unwrap();
        assert_eq!(one[0].formats, vec!["srv3", "vtt"]);
        assert_ne!(one[0].track_key, two[0].track_key);

        let aliases = r#"{"subtitles":{"en":[{"ext":"vtt"}],"EN":[{"ext":"srv3"}]}}"#;
        let merged = parse_transcript_tracks(OCCURRENCE, aliases).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].formats, vec!["srv3", "vtt"]);
    }

    #[test]
    fn inspection_args_are_fixed_to_the_planned_source() {
        let item = PlannedYouTubeItem {
            public: crate::providers::youtube::models::YouTubeScanItem {
                occurrence_id: OCCURRENCE.to_string(),
                video_id: "video-123".to_string(),
                source_url: "https://www.youtube.com/watch?v=video-123".to_string(),
                title: "Title".to_string(),
                ordinal: 1,
                channel_name: None,
                channel_id: None,
                duration_seconds: None,
                thumbnail_available: false,
                availability: crate::providers::youtube::models::YouTubeAvailability::Unknown,
                metadata_digest: "metadata".to_string(),
            },
            source_url: "https://www.youtube.com/watch?v=video-123".to_string(),
            transcript_inspection: None,
        };
        let args = inspection_args(&item);
        assert!(args.contains(&"--no-playlist".to_string()));
        assert_eq!(
            args.last().map(String::as_str),
            Some(item.source_url.as_str())
        );
        assert!(!args.iter().any(|arg| arg.contains("url")));
    }
}
