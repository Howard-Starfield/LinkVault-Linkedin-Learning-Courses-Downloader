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
}
