//! Bounded, pure validation of FFprobe's JSON media report.
//!
//! The verifier intentionally accepts only the report text and an expected
//! stream/duration policy.  It does not open the media path or launch
//! FFprobe; those concerns remain with the workflow and safe-output owners.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// Maximum FFprobe JSON input accepted by this parser.
pub const MAX_FFPROBE_JSON_BYTES: usize = 1024 * 1024;
/// Maximum nesting depth checked before serde parses the JSON.
pub const MAX_FFPROBE_JSON_DEPTH: usize = 32;
/// Maximum number of JSON values retained by the parsed report.
pub const MAX_FFPROBE_JSON_VALUES: usize = 4096;
/// Maximum fields in one FFprobe JSON object.
pub const MAX_FFPROBE_OBJECT_FIELDS: usize = 256;
/// Maximum entries in one FFprobe JSON array.
pub const MAX_FFPROBE_ARRAY_ITEMS: usize = 256;
/// Maximum stream records in one FFprobe report.
pub const MAX_FFPROBE_STREAMS: usize = 32;
/// Maximum bytes in one projected codec or container token.
pub const MAX_MEDIA_IDENTITY_BYTES: usize = 64;
/// Maximum bytes in the complete projected container identity.
pub const MAX_MEDIA_CONTAINER_BYTES: usize = 128;

/// The stream contract requested by the media-producing operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaMode {
    /// The artifact must contain at least one video and one audio stream.
    VideoAndAudio,
    /// The artifact must contain at least one video stream. Additional audio
    /// streams are permitted because a video-only download may still be
    /// delivered in a normal playable container.
    VideoOnly,
}

/// A safe, provider-owned projection of the FFprobe report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedMedia {
    pub container: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub video_streams: usize,
    pub audio_streams: usize,
    pub duration_ms: Option<u64>,
    pub warnings: Vec<MediaCompatibilityWarning>,
}

/// A warning projection containing only bounded, syntax-safe identities.
///
/// The warning deliberately has no provider-supplied free-form message. A
/// caller can map `code` to localized UI text without exposing FFprobe data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCompatibilityWarning {
    pub code: &'static str,
    pub container: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

pub const PLAYBACK_COMPATIBILITY_WARNING: &str = "PLAYBACK_COMPATIBILITY_WARNING";

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum MediaVerificationError {
    #[error("ffprobe JSON exceeds the input size limit")]
    InputTooLarge,
    #[error("ffprobe JSON is malformed")]
    MalformedJson,
    #[error("ffprobe JSON exceeds the nesting limit")]
    TooDeep,
    #[error("ffprobe JSON exceeds the value-count limit")]
    TooManyValues,
    #[error("ffprobe JSON object exceeds the field-count limit")]
    TooManyObjectFields,
    #[error("ffprobe JSON array exceeds the item-count limit")]
    TooManyArrayItems,
    #[error("ffprobe report root is not an object")]
    RootNotObject,
    #[error("ffprobe report is missing a stream array")]
    MissingStreams,
    #[error("ffprobe report contains too many streams")]
    TooManyStreams,
    #[error("ffprobe stream record is malformed")]
    MalformedStream,
    #[error("ffprobe report is missing a format object")]
    MissingFormat,
    #[error("ffprobe report is missing a readable container identity")]
    MissingContainer,
    #[error("ffprobe report contains an unsafe codec or container identity")]
    UnsafeIdentity,
    #[error("ffprobe report is missing an expected video stream")]
    MissingVideoStream,
    #[error("ffprobe report is missing an expected audio stream")]
    MissingAudioStream,
    #[error("ffprobe duration is invalid")]
    InvalidDuration,
    #[error("ffprobe report is missing the expected duration")]
    MissingDuration,
    #[error("expected source duration is invalid")]
    InvalidExpectedDuration,
    #[error("ffmpeg progress report did not reach progress=end")]
    IncompleteProgress,
    #[error("ffmpeg copied no video frames")]
    NoFramesCopied,
}

/// Parse and validate one complete FFprobe JSON report.
///
/// `source_duration_seconds` is the trusted duration from the discovery
/// source. When present, FFprobe must report a duration too; the two values
/// are not compared because container metadata can differ by rounding.
pub fn verify_ffprobe_json(
    input: &str,
    mode: MediaMode,
    source_duration_seconds: Option<f64>,
) -> Result<VerifiedMedia, MediaVerificationError> {
    validate_expected_duration(source_duration_seconds)?;
    if input.len() > MAX_FFPROBE_JSON_BYTES {
        return Err(MediaVerificationError::InputTooLarge);
    }
    scan_json_limits(input)?;
    let value: Value =
        serde_json::from_str(input).map_err(|_| MediaVerificationError::MalformedJson)?;
    enforce_value_limits(&value, 0, &mut ValueCounts::default())?;
    let root = value
        .as_object()
        .ok_or(MediaVerificationError::RootNotObject)?;
    let format = root
        .get("format")
        .and_then(Value::as_object)
        .ok_or(MediaVerificationError::MissingFormat)?;
    let container = read_container(format)?;
    let streams = root
        .get("streams")
        .and_then(Value::as_array)
        .ok_or(MediaVerificationError::MissingStreams)?;
    if streams.len() > MAX_FFPROBE_STREAMS {
        return Err(MediaVerificationError::TooManyStreams);
    }

    let mut video_streams = 0;
    let mut audio_streams = 0;
    let mut video_codec = None;
    let mut audio_codec = None;
    for stream in streams {
        let stream = stream
            .as_object()
            .ok_or(MediaVerificationError::MalformedStream)?;
        let codec_type = stream
            .get("codec_type")
            .and_then(Value::as_str)
            .ok_or(MediaVerificationError::MalformedStream)?;
        let codec = read_optional_identity(stream.get("codec_name"))?;
        if stream.contains_key("duration") {
            read_optional_duration(stream.get("duration"))?;
        }
        match codec_type {
            "video" => {
                video_streams += 1;
                if video_codec.is_none() {
                    video_codec = codec;
                }
            }
            "audio" => {
                audio_streams += 1;
                if audio_codec.is_none() {
                    audio_codec = codec;
                }
            }
            _ => {}
        }
    }

    if video_streams == 0 {
        return Err(MediaVerificationError::MissingVideoStream);
    }
    if matches!(mode, MediaMode::VideoAndAudio) && audio_streams == 0 {
        return Err(MediaVerificationError::MissingAudioStream);
    }

    let duration = if format.contains_key("duration") {
        read_optional_duration(format.get("duration"))?
    } else {
        None
    };
    if source_duration_seconds.is_some() && duration.is_none() {
        return Err(MediaVerificationError::MissingDuration);
    }
    let warnings = if needs_compatibility_warning(&container, &video_codec, &audio_codec, mode) {
        vec![MediaCompatibilityWarning {
            code: PLAYBACK_COMPATIBILITY_WARNING,
            container: container.clone(),
            video_codec: video_codec.clone(),
            audio_codec: audio_codec.clone(),
        }]
    } else {
        Vec::new()
    };
    Ok(VerifiedMedia {
        container,
        video_codec,
        audio_codec,
        video_streams,
        audio_streams,
        duration_ms: duration,
        warnings,
    })
}

/// FFmpeg output captured for [`verify_ffmpeg_copy_probe`].
pub struct FfmpegCopyProbeOutput<'a> {
    /// stdout from `-progress pipe:1`: FFmpeg-generated `key=value` lines only.
    pub progress: &'a str,
    /// stderr at `-v info`; used only for the container line and best-effort codecs.
    pub log: &'a str,
    /// stderr exceeded the retained prefix; codecs are then not inferred at all.
    pub log_truncated: bool,
    /// The exact `-i` argument, as passed to FFmpeg.
    pub input_path: &'a str,
}

/// Arguments for the FFmpeg-only media check. Stream presence is enforced by
/// mandatory `-map`s and demux integrity by `-xerror`; both fail the process,
/// so neither depends on text that media metadata can influence.
pub fn ffmpeg_copy_probe_args(input_path: &str, mode: MediaMode) -> Vec<String> {
    let audio_map = match mode {
        MediaMode::VideoAndAudio => "0:a:0",
        MediaMode::VideoOnly => "0:a:0?",
    };
    [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-v",
        "info",
        "-xerror",
        "-i",
        input_path,
        "-map",
        "0:v:0",
        "-map",
        audio_map,
        "-map_metadata",
        "-1",
        "-map_metadata:s:v",
        "-1",
        "-map_metadata:s:a",
        "-1",
        "-map_chapters",
        "-1",
        "-c",
        "copy",
        "-progress",
        "pipe:1",
        "-fflags",
        "+bitexact",
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect()
}

/// Validate a successful (exit 0) FFmpeg copy probe. Callers must already have
/// rejected non-zero exit, timeout, cancellation, and stdout truncation.
pub fn verify_ffmpeg_copy_probe(
    output: &FfmpegCopyProbeOutput<'_>,
    mode: MediaMode,
    source_duration_seconds: Option<f64>,
) -> Result<VerifiedMedia, MediaVerificationError> {
    validate_expected_duration(source_duration_seconds)?;
    if output.progress.len() > MAX_FFPROBE_JSON_BYTES || output.log.len() > MAX_FFPROBE_JSON_BYTES {
        return Err(MediaVerificationError::InputTooLarge);
    }
    // `source_duration_seconds` is validated for shape only: FFmpeg may report
    // `out_time_us=N/A` for a successful stream copy, so duration is a projection.
    let progress = read_progress(output.progress)?;
    if progress.frames == 0 {
        return Err(MediaVerificationError::NoFramesCopied);
    }
    let duration = progress
        .out_time_us
        .filter(|us| *us > 0)
        .map(|us| us / 1000);
    let container = input_container(output.log, output.input_path)?;
    let (video_codec, audio_codec) = if output.log_truncated {
        (None, None)
    } else {
        output_codecs(output.log).unwrap_or((None, None))
    };
    // Presence is proven by the mandatory maps, not by the parsed codecs.
    let audio_streams = match mode {
        MediaMode::VideoAndAudio => 1,
        MediaMode::VideoOnly => usize::from(audio_codec.is_some()),
    };
    let codecs_known =
        video_codec.is_some() && (audio_codec.is_some() || matches!(mode, MediaMode::VideoOnly));
    let warnings = if !codecs_known
        || needs_compatibility_warning(&container, &video_codec, &audio_codec, mode)
    {
        vec![MediaCompatibilityWarning {
            code: PLAYBACK_COMPATIBILITY_WARNING,
            container: container.clone(),
            video_codec: video_codec.clone(),
            audio_codec: audio_codec.clone(),
        }]
    } else {
        Vec::new()
    };
    Ok(VerifiedMedia {
        container,
        video_codec,
        audio_codec,
        video_streams: 1,
        audio_streams,
        duration_ms: duration,
        warnings,
    })
}

struct ProbeProgress {
    frames: u64,
    out_time_us: Option<u64>,
}

/// Reads the last value of each key; FFmpeg repeats the block every period
/// and the final block (ending `progress=end`) holds the totals.
fn read_progress(progress: &str) -> Result<ProbeProgress, MediaVerificationError> {
    let mut frames = 0;
    let mut out_time_us = None;
    let mut ended = false;
    for line in progress.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        match key {
            "frame" => frames = value.trim().parse::<u64>().unwrap_or(0),
            "out_time_us" => out_time_us = value.trim().parse::<u64>().ok(),
            "progress" => ended = value.trim() == "end",
            _ => {}
        }
    }
    if !ended {
        return Err(MediaVerificationError::IncompleteProgress);
    }
    Ok(ProbeProgress {
        frames,
        out_time_us,
    })
}

fn input_container(log: &str, input_path: &str) -> Result<String, MediaVerificationError> {
    let suffix = format!(", from '{input_path}':");
    log.lines()
        .find(|line| line.starts_with("Input #0, "))
        .and_then(|line| line.strip_prefix("Input #0, "))
        .and_then(|rest| rest.strip_suffix(suffix.as_str()))
        .ok_or(MediaVerificationError::MissingContainer)
        .and_then(container_identity)
}

/// Best-effort codecs from the metadata-stripped final `Output #0` section.
/// `None` when the section is missing or holds anything except one video and
/// at most one audio stream line, so callers fall back to a warning.
fn output_codecs(log: &str) -> Option<(Option<String>, Option<String>)> {
    let lines: Vec<&str> = log.lines().collect();
    let start = lines
        .iter()
        .rposition(|line| *line == "Output #0, null, to 'pipe:':")?;
    let (mut video, mut audio) = (None, None);
    for line in &lines[start + 1..] {
        let Some(body) = line.strip_prefix("  ") else {
            break;
        };
        // FFmpeg prints sub-blocks (side data, display matrix, HDR mastering
        // data) four spaces deep under the stream line they belong to.
        if body.starts_with("  ") && (video.is_some() || audio.is_some()) {
            continue;
        }
        let rest = body.strip_prefix("Stream #0:")?;
        let (index, described) = rest.split_once(": ")?;
        if index.is_empty() || !index.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        let (kind, details) = described.split_once(": ")?;
        let codec = normalize_identity(details.split([' ', ',']).next()?).ok()?;
        let slot = match kind {
            "Video" => &mut video,
            "Audio" => &mut audio,
            _ => return None,
        };
        if slot.replace(codec).is_some() {
            return None;
        }
    }
    video.as_ref()?;
    Some((video, audio))
}

fn validate_expected_duration(value: Option<f64>) -> Result<(), MediaVerificationError> {
    if let Some(seconds) = value {
        if !seconds.is_finite() || seconds < 0.0 || seconds > (u64::MAX as f64 / 1000.0) {
            return Err(MediaVerificationError::InvalidExpectedDuration);
        }
    }
    Ok(())
}

fn read_container(format: &Map<String, Value>) -> Result<String, MediaVerificationError> {
    let raw = format
        .get("format_name")
        .and_then(Value::as_str)
        .ok_or(MediaVerificationError::MissingContainer)?;
    container_identity(raw)
}

fn container_identity(raw: &str) -> Result<String, MediaVerificationError> {
    if raw.trim().len() > MAX_MEDIA_CONTAINER_BYTES {
        return Err(MediaVerificationError::UnsafeIdentity);
    }
    let mut tokens = Vec::new();
    for token in raw.split(',') {
        let token = normalize_identity(token)?;
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    if tokens.is_empty() {
        return Err(MediaVerificationError::MissingContainer);
    }
    let container = tokens.join(",");
    if container.len() > MAX_MEDIA_CONTAINER_BYTES {
        return Err(MediaVerificationError::UnsafeIdentity);
    }
    Ok(container)
}

fn read_optional_identity(value: Option<&Value>) -> Result<Option<String>, MediaVerificationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let raw = value
        .as_str()
        .ok_or(MediaVerificationError::UnsafeIdentity)?;
    Ok(Some(normalize_identity(raw)?))
}

fn normalize_identity(raw: &str) -> Result<String, MediaVerificationError> {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > MAX_MEDIA_IDENTITY_BYTES
        || !value.is_ascii()
        || value.chars().any(|character| {
            character.is_control()
                || !(character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.' | '+'))
        })
    {
        return Err(MediaVerificationError::UnsafeIdentity);
    }
    Ok(value.to_ascii_lowercase())
}

fn read_optional_duration(value: Option<&Value>) -> Result<Option<u64>, MediaVerificationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let seconds = match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or(MediaVerificationError::InvalidDuration)?,
        Value::String(string) => string
            .trim()
            .parse::<f64>()
            .map_err(|_| MediaVerificationError::InvalidDuration)?,
        _ => return Err(MediaVerificationError::InvalidDuration),
    };
    if !seconds.is_finite() || seconds < 0.0 || seconds > (u64::MAX as f64 / 1000.0) {
        return Err(MediaVerificationError::InvalidDuration);
    }
    let milliseconds = (seconds * 1000.0).round();
    if !milliseconds.is_finite() || milliseconds < 0.0 || milliseconds > u64::MAX as f64 {
        return Err(MediaVerificationError::InvalidDuration);
    }
    Ok(Some(milliseconds as u64))
}

fn needs_compatibility_warning(
    container: &str,
    video_codec: &Option<String>,
    audio_codec: &Option<String>,
    mode: MediaMode,
) -> bool {
    let preferred_container = container
        .split(',')
        .any(|token| matches!(token, "mp4" | "mov" | "m4a" | "3gp" | "3g2" | "mj2"));
    let preferred_video = video_codec
        .as_deref()
        .map(is_preferred_video_codec)
        .unwrap_or(false);
    let preferred_audio = if matches!(mode, MediaMode::VideoOnly) && audio_codec.is_none() {
        true
    } else {
        audio_codec
            .as_deref()
            .map(is_preferred_audio_codec)
            .unwrap_or(false)
    };
    !preferred_container || !preferred_video || !preferred_audio
}

fn is_preferred_video_codec(codec: &str) -> bool {
    matches!(codec, "h264" | "avc1" | "avc3")
}

fn is_preferred_audio_codec(codec: &str) -> bool {
    matches!(codec, "aac" | "mp4a")
}

#[derive(Default)]
struct ValueCounts {
    values: usize,
}

fn enforce_value_limits(
    value: &Value,
    depth: usize,
    counts: &mut ValueCounts,
) -> Result<(), MediaVerificationError> {
    if depth > MAX_FFPROBE_JSON_DEPTH {
        return Err(MediaVerificationError::TooDeep);
    }
    counts.values += 1;
    if counts.values > MAX_FFPROBE_JSON_VALUES {
        return Err(MediaVerificationError::TooManyValues);
    }
    match value {
        Value::Array(values) => {
            if values.len() > MAX_FFPROBE_ARRAY_ITEMS {
                return Err(MediaVerificationError::TooManyArrayItems);
            }
            for child in values {
                enforce_value_limits(child, depth + 1, counts)?;
            }
        }
        Value::Object(fields) => {
            if fields.len() > MAX_FFPROBE_OBJECT_FIELDS {
                return Err(MediaVerificationError::TooManyObjectFields);
            }
            for child in fields.values() {
                enforce_value_limits(child, depth + 1, counts)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

/// Scan delimiters before serde recursion so hostile nesting is rejected
/// before a deeply nested value can be allocated or parsed.
fn scan_json_limits(input: &str) -> Result<(), MediaVerificationError> {
    let bytes = input.as_bytes();
    let mut depth = 0;
    let mut containers = 0;
    let mut in_string = false;
    let mut escaped = false;
    for byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                containers += 1;
                if depth > MAX_FFPROBE_JSON_DEPTH {
                    return Err(MediaVerificationError::TooDeep);
                }
                if containers > MAX_FFPROBE_JSON_VALUES {
                    return Err(MediaVerificationError::TooManyValues);
                }
            }
            b'}' | b']' => {
                if depth == 0 {
                    return Err(MediaVerificationError::MalformedJson);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    if in_string || escaped || depth != 0 {
        return Err(MediaVerificationError::MalformedJson);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AV_REPORT: &str = r#"{
        "streams": [
            {"codec_type":"video","codec_name":"h264","duration":"12.345"},
            {"codec_type":"audio","codec_name":"aac","duration":"12.340"}
        ],
        "format": {"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"12.345"}
    }"#;

    #[test]
    fn accepts_bounded_audio_video_report_and_projects_metadata() {
        let media = verify_ffprobe_json(AV_REPORT, MediaMode::VideoAndAudio, Some(12.0)).unwrap();
        assert_eq!(media.container, "mov,mp4,m4a,3gp,3g2,mj2");
        assert_eq!(media.video_codec.as_deref(), Some("h264"));
        assert_eq!(media.audio_codec.as_deref(), Some("aac"));
        assert_eq!(media.video_streams, 1);
        assert_eq!(media.audio_streams, 1);
        assert_eq!(media.duration_ms, Some(12_345));
        assert!(media.warnings.is_empty());
    }

    #[test]
    fn video_only_requires_video_but_not_audio() {
        let report = r#"{
            "streams": [{"codec_type":"video","codec_name":"h264"}],
            "format": {"format_name":"mp4"}
        }"#;
        let media = verify_ffprobe_json(report, MediaMode::VideoOnly, None).unwrap();
        assert_eq!(media.audio_streams, 0);
        assert!(media.warnings.is_empty());
        assert!(verify_ffprobe_json(report, MediaMode::VideoAndAudio, None).is_err());
    }

    #[test]
    fn rejects_malformed_deep_and_unbounded_json() {
        assert!(matches!(
            verify_ffprobe_json("not-json", MediaMode::VideoOnly, None),
            Err(MediaVerificationError::MalformedJson)
        ));
        let nested = format!(
            "{}0{}",
            "[".repeat(MAX_FFPROBE_JSON_DEPTH + 1),
            "]".repeat(MAX_FFPROBE_JSON_DEPTH + 1)
        );
        assert!(matches!(
            verify_ffprobe_json(&nested, MediaMode::VideoOnly, None),
            Err(MediaVerificationError::TooDeep)
        ));
        assert!(matches!(
            verify_ffprobe_json(
                &"{".to_string().repeat(MAX_FFPROBE_JSON_BYTES + 1),
                MediaMode::VideoOnly,
                None
            ),
            Err(MediaVerificationError::InputTooLarge)
        ));
        let values = (0..=MAX_FFPROBE_ARRAY_ITEMS)
            .map(|_| "0")
            .collect::<Vec<_>>()
            .join(",");
        let report = format!(
            r#"{{"streams":[{}],"format":{{"format_name":"mp4"}}}}"#,
            values
        );
        assert!(matches!(
            verify_ffprobe_json(&report, MediaMode::VideoOnly, None),
            Err(MediaVerificationError::TooManyArrayItems)
        ));
        let fields = (0..=MAX_FFPROBE_OBJECT_FIELDS)
            .map(|index| format!("\"field{index}\":0"))
            .collect::<Vec<_>>()
            .join(",");
        let report = format!("{{{fields}}}");
        assert!(matches!(
            verify_ffprobe_json(&report, MediaMode::VideoOnly, None),
            Err(MediaVerificationError::TooManyObjectFields)
        ));
    }

    #[test]
    fn duration_policy_rejects_negative_nonfinite_and_missing_expected_values() {
        for duration in ["-1", "NaN", "Infinity", "-Infinity"] {
            let report = format!(
                r#"{{"streams":[{{"codec_type":"video"}}],"format":{{"format_name":"mp4","duration":"{}"}}}}"#,
                duration
            );
            assert!(matches!(
                verify_ffprobe_json(&report, MediaMode::VideoOnly, None),
                Err(MediaVerificationError::InvalidDuration)
            ));
        }
        let missing = r#"{"streams":[{"codec_type":"video"}],"format":{"format_name":"mp4"}}"#;
        assert!(verify_ffprobe_json(missing, MediaMode::VideoOnly, None).is_ok());
        assert!(matches!(
            verify_ffprobe_json(missing, MediaMode::VideoOnly, Some(5.0)),
            Err(MediaVerificationError::MissingDuration)
        ));
        assert!(matches!(
            verify_ffprobe_json(AV_REPORT, MediaMode::VideoOnly, Some(f64::NAN)),
            Err(MediaVerificationError::InvalidExpectedDuration)
        ));
    }

    #[test]
    fn unsafe_identity_is_rejected_and_fallback_warns_safely() {
        let unsafe_container = r#"{"streams":[{"codec_type":"video","codec_name":"h264"}],"format":{"format_name":"../../mp4"}}"#;
        assert!(matches!(
            verify_ffprobe_json(unsafe_container, MediaMode::VideoOnly, None),
            Err(MediaVerificationError::UnsafeIdentity)
        ));
        let unsafe_codec = r#"{"streams":[{"codec_type":"video","codec_name":"https://evil.test"}],"format":{"format_name":"mp4"}}"#;
        assert!(matches!(
            verify_ffprobe_json(unsafe_codec, MediaMode::VideoOnly, None),
            Err(MediaVerificationError::UnsafeIdentity)
        ));
        let fallback = r#"{
            "streams": [
                {"codec_type":"video","codec_name":"vp9"},
                {"codec_type":"audio","codec_name":"opus"}
            ],
            "format": {"format_name":"webm","duration":"1"}
        }"#;
        let media = verify_ffprobe_json(fallback, MediaMode::VideoAndAudio, None).unwrap();
        assert_eq!(media.warnings.len(), 1);
        assert_eq!(media.warnings[0].code, PLAYBACK_COMPATIBILITY_WARNING);
        assert_eq!(media.warnings[0].container, "webm");
        assert_eq!(media.warnings[0].video_codec.as_deref(), Some("vp9"));
        assert_eq!(media.warnings[0].audio_codec.as_deref(), Some("opus"));
        let serialized = serde_json::to_string(&media.warnings).unwrap();
        assert!(!serialized.contains("evil.test"));
    }

    const PROBE_PATH: &str = "C:\\out\\media.mp4";
    const PROGRESS_END: &str = "frame=2\nfps=0.00\nbitrate=N/A\ntotal_size=N/A\nout_time_us=400000\nout_time_ms=400000\nout_time=00:00:00.400000\nspeed= 234x\nprogress=end\n";
    const CLEAN_LOG: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'C:\\out\\media.mp4':\n  Metadata:\n    major_brand     : isom\n  Duration: 00:00:00.40, start: 0.000000, bitrate: 47 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 64x64, 5 fps (default)\n  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, mono, fltp, 1 kb/s (default)\nStream mapping:\n  Stream #0:0 -> #0:0 (copy)\n  Stream #0:1 -> #0:1 (copy)\nOutput #0, null, to 'pipe:':\n  Stream #0:0: Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 64x64, q=2-31, 5 fps (default)\n  Stream #0:1: Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, mono, fltp, 1 kb/s (default)\n[out#0/null @ 000001a034002580] video:1KiB audio:0KiB subtitle:0KiB other streams:0KiB global headers:0KiB muxing overhead: unknown\n";

    fn probe<'a>(progress: &'a str, log: &'a str) -> FfmpegCopyProbeOutput<'a> {
        FfmpegCopyProbeOutput {
            progress,
            log,
            log_truncated: false,
            input_path: PROBE_PATH,
        }
    }

    #[test]
    fn ffmpeg_copy_probe_args_enforce_streams_and_demux_errors() {
        let args = ffmpeg_copy_probe_args(PROBE_PATH, MediaMode::VideoAndAudio);
        let joined = args.join(" ");
        for required in [
            "-xerror",
            "-map 0:v:0",
            "-map 0:a:0 ",
            "-progress pipe:1",
            "-c copy",
            "-map_metadata -1",
            "-map_metadata:s:v -1",
            "-map_metadata:s:a -1",
            "-map_chapters -1",
            "-fflags +bitexact",
            "-f null -",
        ] {
            assert!(joined.contains(required), "missing {required}: {joined}");
        }
        let video_only = ffmpeg_copy_probe_args(PROBE_PATH, MediaMode::VideoOnly).join(" ");
        assert!(video_only.contains("-map 0:a:0? "));
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == PROBE_PATH).count(),
            1
        );
    }

    #[test]
    fn ffmpeg_copy_probe_projects_clean_report() {
        let media = verify_ffmpeg_copy_probe(
            &probe(PROGRESS_END, CLEAN_LOG),
            MediaMode::VideoAndAudio,
            Some(0.4),
        )
        .unwrap();
        assert_eq!(media.container, "mov,mp4,m4a,3gp,3g2,mj2");
        assert_eq!(media.video_codec.as_deref(), Some("h264"));
        assert_eq!(media.audio_codec.as_deref(), Some("aac"));
        assert_eq!((media.video_streams, media.audio_streams), (1, 1));
        assert_eq!(media.duration_ms, Some(400));
        assert!(media.warnings.is_empty());
    }

    #[test]
    fn ffmpeg_copy_probe_skips_output_stream_sub_blocks() {
        let marker = "q=2-31, 5 fps (default)\n";
        let rotated = CLEAN_LOG.replacen(
            marker,
            "q=2-31, 5 fps (default)\n    Side data:\n      Display Matrix: rotation of -90.00 degrees\n",
            1,
        );
        assert_ne!(rotated, CLEAN_LOG, "fixture edit must apply");
        let media = verify_ffmpeg_copy_probe(
            &probe(PROGRESS_END, &rotated),
            MediaMode::VideoAndAudio,
            Some(0.4),
        )
        .unwrap();
        assert_eq!(media.video_codec.as_deref(), Some("h264"));
        assert_eq!(media.audio_codec.as_deref(), Some("aac"));
        assert!(media.warnings.is_empty());
    }

    #[test]
    fn ffmpeg_copy_probe_accepts_crlf_output() {
        let log = CLEAN_LOG.replace('\n', "\r\n");
        let progress = PROGRESS_END.replace('\n', "\r\n");
        let media =
            verify_ffmpeg_copy_probe(&probe(&progress, &log), MediaMode::VideoAndAudio, Some(0.4))
                .unwrap();
        assert_eq!(media.container, "mov,mp4,m4a,3gp,3g2,mj2");
        assert_eq!(media.video_codec.as_deref(), Some("h264"));
        assert_eq!(media.audio_codec.as_deref(), Some("aac"));
        assert_eq!((media.video_streams, media.audio_streams), (1, 1));
        assert_eq!(media.duration_ms, Some(400));
        assert!(media.warnings.is_empty());
    }

    #[test]
    fn ffmpeg_copy_probe_ignores_forged_lines_in_the_input_section() {
        // Round-2 reproductions: a metadata key and a stream language value
        // injecting stream lines and even a fake Output header.
        let forged = CLEAN_LOG
            .replace(
                "    major_brand     : isom\n",
                "    major_brand     : isom\n    Stream #0:9: Audio: aac, 48000 Hz\n",
            )
            .replace(
                "  Stream #0:0[0x1](und): Video: h264",
                "  Stream #0:0[0x1](eng): Audio: aac, ): Video: vp9\nOutput #0, null, to 'pipe:':\n  Stream #0:0: Video: h264 (High)\n  Stream #0:1: Audio: aac (LC)\n  x(und): Video: h264",
            )
            .replace(
                "Output #0, null, to 'pipe:':\n  Stream #0:0: Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 64x64, q=2-31",
                "Output #0, null, to 'pipe:':\n  Stream #0:0: Video: vp9 (Profile 0), yuv420p, 64x64, q=2-31",
            );
        let media = verify_ffmpeg_copy_probe(
            &probe(PROGRESS_END, &forged),
            MediaMode::VideoAndAudio,
            Some(0.4),
        )
        .unwrap();
        assert_eq!(media.video_codec.as_deref(), Some("vp9")); // from the real (last) Output section
        assert_eq!(media.warnings.len(), 1);
    }

    #[test]
    fn ffmpeg_copy_probe_falls_back_to_a_warning_when_output_section_is_unexpected() {
        let tagged = CLEAN_LOG.replace(
            "  Stream #0:0: Video: h264",
            "  Stream #0:0(eng): Video: h264",
        );
        let media = verify_ffmpeg_copy_probe(
            &probe(PROGRESS_END, &tagged),
            MediaMode::VideoAndAudio,
            None,
        )
        .unwrap();
        assert_eq!(media.video_codec, None);
        assert_eq!(media.warnings.len(), 1);
        let with_metadata = CLEAN_LOG.replace(
            "Output #0, null, to 'pipe:':\n",
            "Output #0, null, to 'pipe:':\n  Metadata:\n    encoder         : Lavf62.6.101\n",
        );
        let media = verify_ffmpeg_copy_probe(
            &probe(PROGRESS_END, &with_metadata),
            MediaMode::VideoAndAudio,
            None,
        )
        .unwrap();
        assert_eq!(media.warnings.len(), 1);
    }

    #[test]
    fn ffmpeg_copy_probe_requires_completed_progress_with_copied_frames() {
        let unfinished = PROGRESS_END.replace("progress=end", "progress=continue");
        assert_eq!(
            verify_ffmpeg_copy_probe(
                &probe(&unfinished, CLEAN_LOG),
                MediaMode::VideoAndAudio,
                None
            ),
            Err(MediaVerificationError::IncompleteProgress)
        );
        let no_frames = PROGRESS_END.replace("frame=2", "frame=0");
        assert_eq!(
            verify_ffmpeg_copy_probe(
                &probe(&no_frames, CLEAN_LOG),
                MediaMode::VideoAndAudio,
                Some(0.4)
            ),
            Err(MediaVerificationError::NoFramesCopied)
        );
    }

    #[test]
    fn ffmpeg_copy_probe_accepts_unavailable_out_time_as_unknown_duration() {
        // Captured in review round 3: a clean MKV stream copy exits 0 with
        // `frame=2`, `out_time_us=N/A`, `progress=end`.
        let mkv_progress = "frame=2\nfps=0.00\nbitrate=N/A\ntotal_size=N/A\nout_time_us=N/A\nout_time_ms=N/A\nout_time=N/A\nspeed=N/A\nprogress=end\n";
        let media = verify_ffmpeg_copy_probe(
            &probe(mkv_progress, CLEAN_LOG),
            MediaMode::VideoAndAudio,
            Some(0.4),
        )
        .unwrap();
        assert_eq!(media.duration_ms, None);
        let zero_time = PROGRESS_END.replace("out_time_us=400000", "out_time_us=0");
        let media = verify_ffmpeg_copy_probe(
            &probe(&zero_time, CLEAN_LOG),
            MediaMode::VideoAndAudio,
            Some(0.4),
        )
        .unwrap();
        assert_eq!(media.duration_ms, None);
    }

    #[test]
    fn ffmpeg_copy_probe_ignores_codecs_when_stderr_was_truncated() {
        // The retained prefix holds a forged Output section from input
        // metadata; the real final section was cut off.
        let forged_prefix = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'C:\\out\\media.mp4':\n  Stream #0:0[0x1](eng): Audio: aac, ): Video: vp9\nOutput #0, null, to 'pipe:':\n  Stream #0:0: Video: h264 (High)\n  Stream #0:1: Audio: aac (LC)\n";
        let output = FfmpegCopyProbeOutput {
            progress: PROGRESS_END,
            log: forged_prefix,
            log_truncated: true,
            input_path: PROBE_PATH,
        };
        let media = verify_ffmpeg_copy_probe(&output, MediaMode::VideoAndAudio, None).unwrap();
        assert_eq!(media.video_codec, None);
        assert_eq!(media.warnings.len(), 1);
    }

    #[test]
    fn ffmpeg_copy_probe_requires_the_input_line_for_our_exact_path() {
        let other_path =
            CLEAN_LOG.replace("from 'C:\\out\\media.mp4':", "from 'C:\\elsewhere.mp4':");
        assert_eq!(
            verify_ffmpeg_copy_probe(
                &probe(PROGRESS_END, &other_path),
                MediaMode::VideoAndAudio,
                None
            ),
            Err(MediaVerificationError::MissingContainer)
        );
        let huge = "x".repeat(MAX_FFPROBE_JSON_BYTES + 1);
        assert_eq!(
            verify_ffmpeg_copy_probe(&probe(PROGRESS_END, &huge), MediaMode::VideoAndAudio, None),
            Err(MediaVerificationError::InputTooLarge)
        );
    }
}
