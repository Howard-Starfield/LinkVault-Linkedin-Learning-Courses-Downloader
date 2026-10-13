//! Bounded transcript metadata inspection for one YouTube occurrence.
//!
//! The helper is asked for the two caption fields only (never the whole info
//! dict, whose `formats` grow with duration), and returns provider metadata
//! that may contain signed subtitle URLs.  This module intentionally projects that response to language,
//! source, display metadata, and extension names only.  URLs, format IDs,
//! and every other provider-private field remain outside the plan and IPC
//! contracts.

use crate::app::managed_process::{
    run, DiscoveryOperation, ManagedProcessContext, ManagedProcessOutput,
};
use crate::providers::youtube::error::YouTubeInternalError;
use crate::providers::youtube::helper::{invocation, output_error, MAX_RECORD_STDOUT_BYTES};
use crate::providers::youtube::models::{
    PlannedYouTubeTranscriptInspection, YouTubeTranscriptInspectionContext,
    YouTubeTranscriptSource, YouTubeTranscriptTrack,
};
use crate::providers::youtube::scan::PlannedYouTubeItem;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

/// Maximum number of language tracks retained for one occurrence.
///
/// YouTube offers about 160 automatic languages and a talk can add 100+
/// uploader ones, so the combined count (before same-language tracks merge)
/// can pass the former 256.  512 leaves roughly 2x headroom while staying
/// bounded: 512 tracks of at most 32 short format names is a few hundred KiB.
pub const MAX_TRANSCRIPT_TRACKS: usize = 512;
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
        ManagedProcessContext::Discovery(operation),
    )
    .map_err(YouTubeInternalError::from_helper_run)?;
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

/// yt-dlp field selection: print just the caption fields as one JSON object.
/// `--print` implies simulate and quiet; errors still reach stderr and a
/// non-zero exit.  The parser reads only these two fields.
const INSPECTION_PRINT_TEMPLATE: &str = "%(.{subtitles,automatic_captions})j";

fn inspection_args(item: &PlannedYouTubeItem) -> Vec<String> {
    vec![
        "--ignore-config".to_string(),
        "--no-plugin-dirs".to_string(),
        "--no-update".to_string(),
        "--no-warnings".to_string(),
        "--skip-download".to_string(),
        "--no-playlist".to_string(),
        "--print".to_string(),
        INSPECTION_PRINT_TEMPLATE.to_string(),
        item.source_url.clone(),
    ]
}

/// Parse one complete inspection result: the field-selected object printed by
/// [`inspection_args`], or any larger yt-dlp info dict that carries the same
/// two fields.  The parser accepts no JSON lines after the root object and does
/// not retain any signed subtitle URL.
pub(crate) fn parse_transcript_tracks(
    occurrence_id: &str,
    stdout: &str,
) -> Result<Vec<YouTubeTranscriptTrack>, YouTubeInternalError> {
    ensure_occurrence_identity(occurrence_id)?;
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
    tracks_from_metadata(occurrence_id, &value)
}

/// Project an already-parsed yt-dlp metadata object to transcript tracks.
/// This is the single parser behind both the dedicated inspection run
/// ([`parse_transcript_tracks`]) and the scan-time shortcut
/// ([`tracks_from_scan_metadata`]), so the two can never disagree.
pub(crate) fn tracks_from_metadata(
    occurrence_id: &str,
    value: &Value,
) -> Result<Vec<YouTubeTranscriptTrack>, YouTubeInternalError> {
    ensure_occurrence_identity(occurrence_id)?;
    let object = value.as_object().ok_or_else(|| {
        YouTubeInternalError::Helper(
            "yt-dlp transcript inspection result was not an object".to_string(),
        )
    })?;

    let mut tracks = Vec::new();
    append_source_tracks(
        occurrence_id,
        object,
        CAPTION_FIELDS[0],
        YouTubeTranscriptSource::Uploader,
        &mut tracks,
    )?;
    append_source_tracks(
        occurrence_id,
        object,
        CAPTION_FIELDS[1],
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

fn ensure_occurrence_identity(occurrence_id: &str) -> Result<(), YouTubeInternalError> {
    if occurrence_id.trim().is_empty() {
        return Err(YouTubeInternalError::InvalidRequest(
            "transcript occurrence identity is empty".to_string(),
        ));
    }
    Ok(())
}

/// Derive an occurrence's tracks from the metadata object of a scan.
///
/// Only a video-mode scan (`--dump-single-json --no-playlist`) carries the
/// caption fields; flat playlist entries carry none, so the caller must not use
/// this for them.  The scan output as a whole may be far larger than an
/// inspection result (`formats` grows with duration), and that is irrelevant
/// here: a dedicated run prints only the caption fields, so the same
/// inspection limit applies to the serialized caption fields alone.  Returns
/// `None` when those exceed it or the caption metadata is malformed, which
/// leaves the occurrence to the lazy per-item inspection fallback and its
/// original error reporting.
pub(crate) fn tracks_from_scan_metadata(
    occurrence_id: &str,
    metadata: &Value,
) -> Option<Vec<YouTubeTranscriptTrack>> {
    let object = metadata.as_object()?;
    let mut caption_bytes = 0usize;
    for field in CAPTION_FIELDS {
        if let Some(value) = object.get(field) {
            caption_bytes = caption_bytes
                .saturating_add(serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len()));
        }
    }
    if caption_bytes > MAX_RECORD_STDOUT_BYTES {
        return None;
    }
    tracks_from_metadata(occurrence_id, metadata).ok()
}

/// The only metadata fields transcript inspection reads.
const CAPTION_FIELDS: [&str; 2] = ["subtitles", "automatic_captions"];

/// Answer an inspection from the plan's stored data when it is bound to this
/// exact snapshot and item metadata, so no helper process is needed.
pub(crate) fn stored_inspection(
    item: &PlannedYouTubeItem,
    source_snapshot_digest: &str,
) -> Option<TranscriptInspectionResult> {
    let stored = item.transcript_inspection.as_ref()?;
    let context = &stored.context;
    if context.source_snapshot_digest != source_snapshot_digest
        || context.occurrence_id != item.public.occurrence_id
        || context.video_id != item.public.video_id
        || context.metadata_digest != item.public.metadata_digest
    {
        return None;
    }
    Some(TranscriptInspectionResult {
        occurrence_id: context.occurrence_id.clone(),
        video_id: context.video_id.clone(),
        tracks: stored.tracks.clone(),
    })
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
        // Field selection: only the caption fields are requested, never the whole
        // info dict (formats, fragments, thumbnails...).
        assert!(!args.contains(&"--dump-single-json".to_string()));
        assert!(args.contains(&"--skip-download".to_string()));
        let print = args.iter().position(|arg| arg == "--print").unwrap();
        assert_eq!(args[print + 1], "%(.{subtitles,automatic_captions})j");
        assert_eq!(args[print + 1], INSPECTION_PRINT_TEMPLATE);
        assert!(!args
            .iter()
            .any(|arg| arg == "--write-auto-subs" || arg == "--write-subs"));
    }

    /// Output captured from yt-dlp 2026.08.19 with the exact field-selection template, trimmed
    /// to a few languages and with signed URLs shortened.  Python-style separators and the
    /// trailing newline are as printed.
    const FIELD_SELECTED_SAMPLE: &str = r#"{"subtitles": {"en": [{"ext": "json3", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv1", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv2", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv3", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "ttml", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srt", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "vtt", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}], "de-DE": [{"ext": "json3", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "German (Germany)", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv1", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "German (Germany)", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv2", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "German (Germany)", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv3", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "German (Germany)", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "ttml", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "German (Germany)", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srt", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "German (Germany)", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "vtt", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "German (Germany)", "impersonate": true, "__yt_dlp_client": "visionos"}]}, "automatic_captions": {"en": [{"url": "https://manifest.googlevideo.com/api/manifest/hl...", "ext": "vtt", "protocol": "m3u8_native", "__yt_dlp_client": "visionos"}, {"ext": "json3", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv1", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv2", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srv3", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "ttml", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "srt", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}, {"ext": "vtt", "url": "https://www.youtube.com/api/timedtext?v=dQw4w9Wg...", "name": "English", "impersonate": true, "__yt_dlp_client": "visionos"}]}}
"#;

    #[test]
    fn parser_accepts_the_field_selected_output_and_matches_the_full_dump_shape() {
        let selected = parse_transcript_tracks(OCCURRENCE, FIELD_SELECTED_SAMPLE).unwrap();
        assert!(selected.iter().any(|track| track.language_tag == "en"
            && track.source == YouTubeTranscriptSource::Uploader));
        assert!(selected
            .iter()
            .any(|track| track.source == YouTubeTranscriptSource::Automatic));
        assert!(selected.iter().all(|track| !track.formats.is_empty()));
        // A full dump-single-json result carries the same two fields among many others.
        let value: Value = serde_json::from_str(FIELD_SELECTED_SAMPLE).unwrap();
        let mut full = serde_json::Map::new();
        full.insert("id".to_string(), Value::from("dQw4w9WgXcQ"));
        full.insert("formats".to_string(), serde_json::json!([{"format_id": "18"}]));
        full.extend(value.as_object().unwrap().clone());
        let full = serde_json::to_string(&Value::Object(full)).unwrap();
        assert_eq!(parse_transcript_tracks(OCCURRENCE, &full).unwrap(), selected);
    }

    #[test]
    fn a_video_with_many_uploader_and_automatic_languages_does_not_hit_the_track_limit() {
        // YouTube offers ~160 automatic languages; a talk can add 100+ uploader ones.
        let languages = |prefix: &str, count: usize| {
            (0..count)
                .map(|index| format!(r#""{prefix}{index:03}":[{{"ext":"vtt"}}]"#))
                .collect::<Vec<_>>()
                .join(",")
        };
        let payload = format!(
            r#"{{"subtitles":{{{}}},"automatic_captions":{{{}}}}}"#,
            languages("u", 120),
            languages("a", 170)
        );
        let tracks = parse_transcript_tracks(OCCURRENCE, &payload).unwrap();
        assert_eq!(tracks.len(), 290);
        let too_many = format!(
            r#"{{"automatic_captions":{{{}}}}}"#,
            languages("a", MAX_TRANSCRIPT_TRACKS + 1)
        );
        assert!(parse_transcript_tracks(OCCURRENCE, &too_many).is_err());
    }

    /// Live reproduction through the real managed supervisor and the installed
    /// helper pack.  Run with `LINKVAULT_DATA_DIR` pointing at the data folder
    /// that holds `media-toolchain` (for example `%TEMP%\lv-e2e`):
    /// `cargo test --lib live_inspection_sizes -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires an installed media toolchain and network access"]
    fn live_inspection_sizes() {
        use crate::app::managed_process::DiscoveryOperation;
        use crate::providers::youtube::models::{ScanYouTubeSourceRequest, YouTubePlaylistMode};
        use crate::providers::youtube::scan::scan_source;
        let urls = std::env::var("LV_LIVE_URLS")
            .unwrap_or_else(|_| {
                "https://www.youtube.com/watch?v=jNQXAC9IVRw,https://www.youtube.com/watch?v=wbPMGczOVbk"
                    .to_string()
            });
        for url in urls.split(',') {
            let operation = DiscoveryOperation::new();
            let plan = scan_source(
                &ScanYouTubeSourceRequest {
                    client_operation_id: "live-inspection-sizes".to_string(),
                    url: url.to_string(),
                    playlist_mode: Some(YouTubePlaylistMode::Video),
                },
                &operation,
            )
            .expect("scan");
            let item = plan.items[0].clone();
            eprintln!(
                "{url}: scan stored tracks = {:?}",
                item.transcript_inspection.as_ref().map(|stored| stored.tracks.len())
            );
            // The old dedicated run: the whole info dict.
            let mut full_args = inspection_args(&item);
            let print = full_args.iter().position(|arg| arg == "--print").unwrap();
            full_args.splice(print..print + 2, ["--dump-single-json".to_string()]);
            let output = run(
                invocation(full_args, MAX_RECORD_STDOUT_BYTES),
                ManagedProcessContext::Discovery(&operation),
            )
            .expect("full dump run");
            eprintln!(
                "{url}: full dump stdout.len={} truncated={} status={:?}",
                output.stdout.len(),
                output.stdout_truncated,
                output.status
            );
            let check = ensure_inspection_output(&operation, &output);
            eprintln!("{url}: ensure_inspection_output = {check:?}");
            if check.is_ok() {
                let tracks = parse_transcript_tracks(&item.public.occurrence_id, &output.stdout);
                eprintln!("{url}: parse (full dump) = {:?}", tracks.as_ref().map(Vec::len));
            }
            // The field-selected dedicated run.
            let inspected = inspect_transcripts(&item, &operation).expect("field-selected inspection");
            eprintln!("{url}: field-selected tracks = {}", inspected.tracks.len());
            if let Some(stored) = item.transcript_inspection.as_ref() {
                assert_eq!(stored.tracks, inspected.tracks, "scan-derived tracks differ");
            }
        }
    }
}
