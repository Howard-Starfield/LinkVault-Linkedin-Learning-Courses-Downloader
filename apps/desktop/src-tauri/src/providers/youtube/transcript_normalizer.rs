//! Deterministic, non-HTML WebVTT normalization for YouTube transcript artifacts.
//!
//! The caller owns the raw VTT artifact. This module only validates and parses
//! a bounded UTF-8 copy, then returns a versioned projection that can be
//! serialized into the staged transcript JSON artifact.

use crate::providers::youtube::models::YouTubeTranscriptSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const NORMALIZED_TRANSCRIPT_SCHEMA_VERSION: u32 = 1;
pub const TRANSCRIPT_PROVIDER: &str = "youtube";
pub const MAX_VTT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptMetadata {
    pub video_id: String,
    pub language_tag: String,
    pub source: YouTubeTranscriptSource,
    pub source_track_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedTranscript {
    pub schema_version: u32,
    pub provider: String,
    pub video_id: String,
    pub language_tag: String,
    pub source: YouTubeTranscriptSource,
    pub source_track_key: String,
    pub source_vtt_sha256: String,
    pub cues: Vec<TranscriptCue>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TranscriptNormalizationError {
    #[error("raw VTT is too large: {size_bytes} bytes exceeds {max_bytes} bytes")]
    InputTooLarge { size_bytes: usize, max_bytes: usize },
    #[error("raw VTT is not valid UTF-8")]
    InvalidUtf8,
    #[error("raw VTT is missing the WEBVTT header")]
    MissingHeader,
    #[error("VTT structure is malformed: {message}")]
    MalformedStructure { message: String },
    #[error("VTT cue timing line is malformed: {line}")]
    MalformedTiming { line: String },
    #[error("VTT timestamp is malformed: {timestamp}")]
    MalformedTimestamp { timestamp: String },
    #[error("VTT timestamp overflows milliseconds: {timestamp}")]
    TimestampOverflow { timestamp: String },
    #[error("VTT cue has invalid bounds: start {start_ms} ms, end {end_ms} ms")]
    InvalidCueBounds { start_ms: u64, end_ms: u64 },
    #[error("transcript metadata field is empty: {field}")]
    EmptyMetadata { field: &'static str },
    #[error("normalized transcript JSON serialization failed: {message}")]
    Serialization { message: String },
}

/// Normalizes a raw WebVTT artifact without mutating the raw bytes.
pub fn normalize_vtt(
    raw_vtt: &[u8],
    metadata: TranscriptMetadata,
) -> Result<NormalizedTranscript, TranscriptNormalizationError> {
    if raw_vtt.len() > MAX_VTT_BYTES {
        return Err(TranscriptNormalizationError::InputTooLarge {
            size_bytes: raw_vtt.len(),
            max_bytes: MAX_VTT_BYTES,
        });
    }
    validate_metadata(&metadata)?;

    let source_vtt_sha256 = sha256_hex(raw_vtt);
    let source =
        std::str::from_utf8(raw_vtt).map_err(|_| TranscriptNormalizationError::InvalidUtf8)?;
    let normalized_lines = normalize_line_endings(source);
    let cues = parse_vtt(&normalized_lines)?;

    Ok(NormalizedTranscript {
        schema_version: NORMALIZED_TRANSCRIPT_SCHEMA_VERSION,
        provider: TRANSCRIPT_PROVIDER.to_string(),
        video_id: metadata.video_id,
        language_tag: metadata.language_tag,
        source: metadata.source,
        source_track_key: metadata.source_track_key,
        source_vtt_sha256,
        cues,
    })
}

/// Serializes the normalized projection with stable struct field order.
pub fn normalize_vtt_json(
    raw_vtt: &[u8],
    metadata: TranscriptMetadata,
) -> Result<Vec<u8>, TranscriptNormalizationError> {
    let normalized = normalize_vtt(raw_vtt, metadata)?;
    serde_json::to_vec(&normalized).map_err(|error| TranscriptNormalizationError::Serialization {
        message: error.to_string(),
    })
}

fn validate_metadata(metadata: &TranscriptMetadata) -> Result<(), TranscriptNormalizationError> {
    if metadata.video_id.trim().is_empty() {
        return Err(TranscriptNormalizationError::EmptyMetadata { field: "videoId" });
    }
    if metadata.language_tag.trim().is_empty() {
        return Err(TranscriptNormalizationError::EmptyMetadata {
            field: "languageTag",
        });
    }
    if metadata.source_track_key.trim().is_empty() {
        return Err(TranscriptNormalizationError::EmptyMetadata {
            field: "sourceTrackKey",
        });
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_line_endings(source: &str) -> String {
    let mut normalized = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

fn parse_vtt(source: &str) -> Result<Vec<TranscriptCue>, TranscriptNormalizationError> {
    let mut lines: Vec<&str> = source.split('\n').collect();
    if let Some(first_line) = lines.first_mut() {
        if let Some(without_bom) = first_line.strip_prefix('\u{feff}') {
            *first_line = without_bom;
        }
    }
    let Some(header) = lines.first() else {
        return Err(TranscriptNormalizationError::MissingHeader);
    };
    if !is_webvtt_header(header) {
        return Err(TranscriptNormalizationError::MissingHeader);
    }

    let mut index = 1;
    skip_header_metadata(&lines, &mut index);
    let mut cues = Vec::new();

    while index < lines.len() {
        if lines[index].trim().is_empty() {
            index += 1;
            continue;
        }
        if is_ignored_block_start(lines[index]) {
            skip_block(&lines, &mut index);
            continue;
        }

        let timing_index = if lines[index].contains("-->") {
            index
        } else if lines
            .get(index + 1)
            .is_some_and(|line| line.contains("-->"))
        {
            index + 1
        } else {
            return Err(TranscriptNormalizationError::MalformedStructure {
                message: format!("expected cue timing near {}", preview(lines[index])),
            });
        };

        let timing_line = lines[timing_index];
        let (start_ms, end_ms) = parse_timing_line(timing_line)?;
        index = timing_index + 1;
        let text_start = index;
        while index < lines.len() && !lines[index].trim().is_empty() {
            index += 1;
        }
        let text = lines[text_start..index]
            .iter()
            .map(|line| normalize_cue_text(line))
            .collect::<Vec<_>>()
            .join("\n");
        cues.push(TranscriptCue {
            start_ms,
            end_ms,
            text,
        });
    }

    Ok(cues)
}

fn is_webvtt_header(line: &str) -> bool {
    line == "WEBVTT"
        || line
            .strip_prefix("WEBVTT")
            .is_some_and(|suffix| suffix.starts_with(' ') || suffix.starts_with('\t'))
}

fn skip_header_metadata(lines: &[&str], index: &mut usize) {
    while *index < lines.len() && lines[*index].trim().is_empty() {
        *index += 1;
    }
    while *index < lines.len() && !lines[*index].trim().is_empty() {
        if is_ignored_block_start(lines[*index])
            || lines[*index].contains("-->")
            || lines
                .get(*index + 1)
                .is_some_and(|line| line.contains("-->"))
        {
            break;
        }
        if !lines[*index].contains(':') {
            break;
        }
        *index += 1;
    }
    while *index < lines.len() && lines[*index].trim().is_empty() {
        *index += 1;
    }
}

fn is_ignored_block_start(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed == "STYLE"
        || trimmed == "REGION"
        || trimmed == "NOTE"
        || trimmed.starts_with("NOTE ")
        || trimmed.starts_with("NOTE\t")
}

fn skip_block(lines: &[&str], index: &mut usize) {
    *index += 1;
    while *index < lines.len() && !lines[*index].trim().is_empty() {
        *index += 1;
    }
    while *index < lines.len() && lines[*index].trim().is_empty() {
        *index += 1;
    }
}

fn parse_timing_line(line: &str) -> Result<(u64, u64), TranscriptNormalizationError> {
    if line.matches("-->").count() != 1 {
        return Err(TranscriptNormalizationError::MalformedTiming {
            line: preview(line),
        });
    }
    let Some((start, end_and_settings)) = line.split_once("-->") else {
        return Err(TranscriptNormalizationError::MalformedTiming {
            line: preview(line),
        });
    };
    let start = start.trim();
    let end = end_and_settings
        .split_whitespace()
        .next()
        .unwrap_or_default();
    if start.is_empty() || end.is_empty() {
        return Err(TranscriptNormalizationError::MalformedTiming {
            line: preview(line),
        });
    }
    let start_ms = parse_timestamp(start)?;
    let end_ms = parse_timestamp(end)?;
    if end_ms <= start_ms {
        return Err(TranscriptNormalizationError::InvalidCueBounds { start_ms, end_ms });
    }
    Ok((start_ms, end_ms))
}

fn parse_timestamp(timestamp: &str) -> Result<u64, TranscriptNormalizationError> {
    let malformed = || TranscriptNormalizationError::MalformedTimestamp {
        timestamp: preview(timestamp),
    };
    let overflow = || TranscriptNormalizationError::TimestampOverflow {
        timestamp: preview(timestamp),
    };
    let (whole, millis) = timestamp.split_once('.').ok_or_else(malformed)?;
    if millis.len() != 3 || !millis.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(malformed());
    }
    let components: Vec<&str> = whole.split(':').collect();
    if components.len() != 2 && components.len() != 3 {
        return Err(malformed());
    }
    let seconds_part = components.last().copied().unwrap_or_default();
    let minutes_part = components
        .get(components.len() - 2)
        .copied()
        .unwrap_or_default();
    if seconds_part.len() != 2
        || minutes_part.len() < 2
        || !seconds_part.bytes().all(|byte| byte.is_ascii_digit())
        || !minutes_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(malformed());
    }
    let minutes = minutes_part.parse::<u64>().map_err(|_| overflow())?;
    let seconds = seconds_part.parse::<u64>().map_err(|_| overflow())?;
    let milliseconds = millis.parse::<u64>().map_err(|_| overflow())?;
    if seconds >= 60 || (components.len() == 3 && minutes >= 60) {
        return Err(malformed());
    }
    let hours = if components.len() == 3 {
        let hours_part = components[0];
        if hours_part.len() < 2 || !hours_part.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(malformed());
        }
        hours_part.parse::<u64>().map_err(|_| overflow())?
    } else {
        0
    };
    hours
        .checked_mul(3_600_000)
        .and_then(|value| value.checked_add(minutes.checked_mul(60_000)?))
        .and_then(|value| value.checked_add(seconds.checked_mul(1_000)?))
        .and_then(|value| value.checked_add(milliseconds))
        .ok_or_else(overflow)
}

fn normalize_cue_text(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    let mut remainder = line;
    while let Some(open_index) = remainder.find('<') {
        text.push_str(&remainder[..open_index]);
        let after_open = &remainder[open_index + 1..];
        let Some(close_offset) = after_open.find('>') else {
            text.push_str(&remainder[open_index..]);
            break;
        };
        remainder = &after_open[close_offset + 1..];
    }
    if !remainder.is_empty() && !remainder.contains('<') {
        text.push_str(remainder);
    }
    decode_entities(&text)
}

fn decode_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(ampersand_index) = remainder.find('&') {
        output.push_str(&remainder[..ampersand_index]);
        let after_ampersand = &remainder[ampersand_index + 1..];
        let Some(semicolon_index) = after_ampersand.find(';') else {
            output.push_str(&remainder[ampersand_index..]);
            break;
        };
        let entity = &after_ampersand[..semicolon_index];
        if entity.len() <= 32 {
            if let Some(decoded) = decode_entity(entity) {
                output.push_str(&decoded);
                remainder = &after_ampersand[semicolon_index + 1..];
                continue;
            }
        }
        output.push('&');
        remainder = after_ampersand;
    }
    if !remainder.is_empty() && !remainder.contains('&') {
        output.push_str(remainder);
    }
    output
}

fn decode_entity(entity: &str) -> Option<String> {
    let decoded = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "lrm" => '\u{200e}',
        "rlm" => '\u{200f}',
        "nbsp" => '\u{00a0}',
        _ => {
            let numeric = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"));
            if let Some(hex) = numeric {
                let code_point = u32::from_str_radix(hex, 16).ok()?;
                return char::from_u32(code_point).map(|character| character.to_string());
            }
            let decimal = entity.strip_prefix('#')?;
            let code_point = decimal.parse::<u32>().ok()?;
            return char::from_u32(code_point).map(|character| character.to_string());
        }
    };
    Some(decoded.to_string())
}

fn preview(value: &str) -> String {
    const MAX_PREVIEW_BYTES: usize = 256;
    if value.len() <= MAX_PREVIEW_BYTES {
        return value.to_string();
    }
    let mut end = MAX_PREVIEW_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> TranscriptMetadata {
        TranscriptMetadata {
            video_id: "video-123".to_string(),
            language_tag: "en".to_string(),
            source: YouTubeTranscriptSource::Uploader,
            source_track_key: "track-en-uploader".to_string(),
        }
    }

    #[test]
    fn normalizes_fixture_without_deduplicating_rolling_caption_text() {
        let raw = concat!(
            "WEBVTT - fixture\r\n",
            "Kind: captions\r\n",
            "\r\n",
            "NOTE generated by fixture\r\n",
            "this block is omitted\r\n",
            "\r\n",
            "STYLE\r\n",
            "::cue { color: red; }\r\n",
            "\r\n",
            "cue-1\r\n",
            "00:00:01.000 --> 00:00:03.500 align:start position:0%\r\n",
            "<b>Hello</b> &amp; &lt;world&gt;\r\n",
            "same rolling text\r\n",
            "\r\n",
            "00:00:03.500 --> 00:00:04.000\r\n",
            "same rolling text\r\n",
        );
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).expect("fixture should parse");
        assert_eq!(normalized.schema_version, 1);
        assert_eq!(normalized.provider, "youtube");
        assert_eq!(normalized.cues.len(), 2);
        assert_eq!(normalized.cues[0].start_ms, 1_000);
        assert_eq!(normalized.cues[0].end_ms, 3_500);
        assert_eq!(
            normalized.cues[0].text,
            "Hello & <world>\nsame rolling text"
        );
        assert_eq!(normalized.cues[1].text, "same rolling text");
        assert_eq!(normalized.source_vtt_sha256, sha256_hex(raw.as_bytes()));
    }

    #[test]
    fn normalizes_bom_and_lone_cr_line_endings() {
        let raw = "\u{feff}WEBVTT\r\n\r\n00:00:00.000 --> 00:00:01.000\rhello\rworld";
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).expect("fixture should parse");
        assert_eq!(normalized.cues[0].text, "hello\nworld");
    }

    #[test]
    fn strips_markup_and_decodes_supported_entities_without_html_evaluation() {
        let raw = concat!(
            "WEBVTT\n\n",
            "00:00:00.000 --> 00:00:01.000\n",
            "<script>alert(1)</script><i>safe</i> &quot; &apos; &#39; &#x22; &lrm; &rlm; &nbsp;\n",
        );
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).expect("fixture should parse");
        assert_eq!(
            normalized.cues[0].text,
            "alert(1)safe \" ' ' \" \u{200e} \u{200f} \u{00a0}"
        );
    }

    #[test]
    fn decodes_entities_once_and_keeps_unknown_entities() {
        let raw = concat!(
            "WEBVTT\n\n",
            "00:00:00.000 --> 00:00:01.000\n",
            "&amp;lt; &unknown; &#x110000;\n",
        );
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).expect("fixture should parse");
        assert_eq!(normalized.cues[0].text, "&lt; &unknown; &#x110000;");
    }

    #[test]
    fn rejects_invalid_utf8_and_oversized_raw_vtt() {
        let invalid = [b'W', b'E', b'B', b'V', b'T', b'T', b'\n', 0xff];
        assert_eq!(
            normalize_vtt(&invalid, metadata()),
            Err(TranscriptNormalizationError::InvalidUtf8)
        );
        let oversized = vec![b'x'; MAX_VTT_BYTES + 1];
        assert_eq!(
            normalize_vtt(&oversized, metadata()),
            Err(TranscriptNormalizationError::InputTooLarge {
                size_bytes: MAX_VTT_BYTES + 1,
                max_bytes: MAX_VTT_BYTES,
            })
        );
    }

    #[test]
    fn rejects_malformed_timestamps_and_invalid_bounds() {
        for timestamp in ["00:00:01", "00:60.000", "0:00:00.000", "00:00:00.00"] {
            let raw = format!("WEBVTT\n\n{timestamp} --> 00:00:02.000\nx\n");
            assert!(matches!(
                normalize_vtt(raw.as_bytes(), metadata()),
                Err(TranscriptNormalizationError::MalformedTimestamp { .. })
            ));
        }
        let raw = b"WEBVTT\n\n00:00:02.000 --> 00:00:01.000\nx\n";
        assert!(matches!(
            normalize_vtt(raw, metadata()),
            Err(TranscriptNormalizationError::InvalidCueBounds { .. })
        ));
        let overflow = format!(
            "WEBVTT\n\n{}:00:00.000 --> 99:00:00.000\nx\n",
            "999999999999999999999999999999999999999999999999999999999999"
        );
        assert!(matches!(
            normalize_vtt(overflow.as_bytes(), metadata()),
            Err(TranscriptNormalizationError::TimestampOverflow { .. })
        ));
    }

    #[test]
    fn emits_deterministic_versioned_json() {
        let raw = b"WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nhello\n";
        let first = normalize_vtt_json(raw, metadata()).expect("JSON should serialize");
        let second = normalize_vtt_json(raw, metadata()).expect("JSON should serialize");
        assert_eq!(first, second);
        let json = String::from_utf8(first).expect("serde JSON is UTF-8");
        assert!(json.contains("\"schemaVersion\":1"));
        assert!(json.contains("\"provider\":\"youtube\""));
        assert!(json.contains("\"startMs\":0"));
    }
}
