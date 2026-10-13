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
/// Largest raw VTT accepted.  Twelve hours of word-timed rolling auto-captions
/// is roughly 8-10 MB and dense speech can pass 16 MiB; 64 MiB leaves ample room
/// while the whole file is still read into memory once.
pub const MAX_VTT_BYTES: usize = 64 * 1024 * 1024;
/// Largest normalized transcript JSON written or reused.  Enforced by
/// [`normalize_vtt_json`], which both the fresh write and the reuse check call,
/// so what is written fresh can always be reused.  Equal to the VTT cap: the
/// JSON drops the markup but adds a few dozen bytes per cue.
pub const MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES: usize = 64 * 1024 * 1024;

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
    #[error("normalized transcript is too large: {size_bytes} bytes exceeds {max_bytes} bytes")]
    OutputTooLarge { size_bytes: usize, max_bytes: usize },
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
    normalize_vtt_json_within(raw_vtt, metadata, MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES)
}

fn normalize_vtt_json_within(
    raw_vtt: &[u8],
    metadata: TranscriptMetadata,
    max_json_bytes: usize,
) -> Result<Vec<u8>, TranscriptNormalizationError> {
    let normalized = normalize_vtt(raw_vtt, metadata)?;
    let json = serde_json::to_vec(&normalized).map_err(|error| {
        TranscriptNormalizationError::Serialization {
            message: error.to_string(),
        }
    })?;
    if json.len() > max_json_bytes {
        return Err(TranscriptNormalizationError::OutputTooLarge {
            size_bytes: json.len(),
            max_bytes: max_json_bytes,
        });
    }
    Ok(json)
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
        while index < lines.len() && !ends_cue_text(&lines, index) {
            index += 1;
        }
        let text = join_cue_text(&lines[text_start..index]);
        cues.push(TranscriptCue {
            start_ms,
            end_ms,
            text,
        });
    }

    Ok(cues)
}

/// Per the WebVTT spec only a truly empty line ends cue text. YouTube
/// auto-captions put whitespace-only lines inside cues, so those stay part of
/// the text unless the next line starts a new cue (a timing line, or a cue
/// identifier followed by a timing line) or a NOTE, STYLE or REGION block.
fn ends_cue_text(lines: &[&str], index: usize) -> bool {
    let line = lines[index];
    if line.is_empty() {
        return true;
    }
    if !line.trim().is_empty() {
        return false;
    }
    match lines.get(index + 1) {
        Some(next) if next.contains("-->") || is_ignored_block_start(next) => true,
        Some(next) if !next.trim().is_empty() => lines
            .get(index + 2)
            .is_some_and(|after| after.contains("-->")),
        _ => false,
    }
}

/// Normalizes each cue text line and joins them, dropping blank lines at
/// either end (whitespace-only lines normalize to empty text).
fn join_cue_text(cue_lines: &[&str]) -> String {
    let normalized: Vec<String> = cue_lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                normalize_cue_text(line)
            }
        })
        .collect();
    let is_blank = |line: &String| line.trim().is_empty();
    let Some(first) = normalized.iter().position(|line| !is_blank(line)) else {
        return String::new();
    };
    let last = normalized
        .iter()
        .rposition(|line| !is_blank(line))
        .unwrap_or(first);
    normalized[first..=last].join("\n")
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

const MAX_ENTITY_BYTES: usize = 32;

fn decode_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(ampersand_index) = remainder.find('&') {
        output.push_str(&remainder[..ampersand_index]);
        let after_ampersand = &remainder[ampersand_index + 1..];
        // An entity is at most 32 bytes, so look for its terminator only that far:
        // an unbounded search made a long run of '&' quadratic in the line length.
        let window = &after_ampersand.as_bytes()[..after_ampersand.len().min(MAX_ENTITY_BYTES + 1)];
        if let Some(semicolon_index) = window.iter().position(|byte| *byte == b';') {
            if let Some(decoded) = decode_entity(&after_ampersand[..semicolon_index]) {
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
    fn parses_youtube_auto_caption_cues_with_whitespace_only_lines() {
        let raw = concat!(
            "WEBVTT\n",
            "Kind: captions\n",
            "Language: en\n",
            "\n",
            "00:00:00.160 --> 00:00:02.230 align:start position:0%\n",
            " \n",
            "You<00:00:00.240><c> are</c><00:00:00.320><c> sitting</c><00:00:00.600><c> in</c><00:00:00.680><c> a</c><00:00:00.720><c> meeting</c>\n",
            "\n",
            "00:00:02.230 --> 00:00:02.240 align:start position:0%\n",
            "You are sitting in a meeting\n",
            " \n",
            "\n",
            "00:00:02.240 --> 00:00:04.990 align:start position:0%\n",
            "You are sitting in a meeting\n",
            "and<00:00:02.560><c> someone</c><00:00:02.880><c> says</c>\n",
        );
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).expect("auto captions parse");
        assert_eq!(normalized.cues.len(), 3);
        assert_eq!(
            (normalized.cues[0].start_ms, normalized.cues[0].end_ms),
            (160, 2_230)
        );
        assert_eq!(normalized.cues[0].text, "You are sitting in a meeting");
        assert_eq!(
            (normalized.cues[1].start_ms, normalized.cues[1].end_ms),
            (2_230, 2_240)
        );
        assert_eq!(normalized.cues[1].text, "You are sitting in a meeting");
        assert_eq!(
            (normalized.cues[2].start_ms, normalized.cues[2].end_ms),
            (2_240, 4_990)
        );
        assert_eq!(
            normalized.cues[2].text,
            "You are sitting in a meeting\nand someone says"
        );
    }

    #[test]
    fn whitespace_only_line_before_next_cue_terminates_the_cue() {
        let raw = concat!(
            "WEBVTT\n\n",
            "00:00:00.000 --> 00:00:01.000\n",
            "first\n",
            " \n",
            "00:00:01.000 --> 00:00:02.000\n",
            "second\n",
            "\t\n",
            "cue-3\n",
            "00:00:02.000 --> 00:00:03.000\n",
            "third\n",
        );
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).expect("fixture should parse");
        let cues: Vec<_> = normalized
            .cues
            .iter()
            .map(|cue| (cue.start_ms, cue.end_ms, cue.text.as_str()))
            .collect();
        assert_eq!(
            cues,
            vec![
                (0, 1_000, "first"),
                (1_000, 2_000, "second"),
                (2_000, 3_000, "third"),
            ]
        );
    }

    #[test]
    fn whitespace_only_line_before_note_style_or_region_block_terminates_the_cue() {
        for block in [
            "NOTE a comment\nspanning two lines",
            "STYLE\n::cue { color: red }",
            "REGION\nid:fred",
        ] {
            let raw = format!(
                "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nfirst\n \n{block}\n\n00:00:01.000 --> 00:00:02.000\nsecond\n"
            );
            let normalized =
                normalize_vtt(raw.as_bytes(), metadata()).expect("fixture should parse");
            let cues: Vec<_> = normalized
                .cues
                .iter()
                .map(|cue| (cue.start_ms, cue.end_ms, cue.text.as_str()))
                .collect();
            assert_eq!(
                cues,
                vec![(0, 1_000, "first"), (1_000, 2_000, "second")],
                "block leaked into the cue: {block}"
            );
        }
    }

    #[test]
    fn keeps_cues_whose_text_is_only_whitespace_with_empty_text() {
        let raw =
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n \n\n00:00:01.000 --> 00:00:02.000\nx\n";
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).expect("fixture should parse");
        assert_eq!(normalized.cues.len(), 2);
        assert_eq!(normalized.cues[0].text, "");
        assert_eq!(normalized.cues[1].text, "x");
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

    /// Twelve hours of YouTube-style rolling auto-captions: every 0.5 s a cue
    /// that repeats the previous line, holds a whitespace-only line, and adds a
    /// word-timed line, i.e. about 430k lines and about 10 MB.
    fn rolling_auto_caption_vtt(hours: u64) -> String {
        const WORDS: [&str; 8] = ["so", "today", "we", "are", "going", "to", "talk", "about"];
        let cue_count = hours * 3600 * 2;
        let mut vtt = String::with_capacity(cue_count as usize * 130);
        vtt.push_str("WEBVTT\nKind: captions\nLanguage: en\n\n");
        let stamp = |ms: u64| {
            format!(
                "{:02}:{:02}:{:02}.{:03}",
                ms / 3_600_000,
                ms / 60_000 % 60,
                ms / 1_000 % 60,
                ms % 1_000
            )
        };
        for cue in 0..cue_count {
            let start = cue * 500;
            let previous = WORDS[(cue as usize) % WORDS.len()];
            let word = WORDS[(cue as usize + 1) % WORDS.len()];
            vtt.push_str(&format!(
                "{} --> {} align:start position:0%\n{previous} {previous}\n \n{word}<{}><c> {word}</c><{}><c> &gt;&gt; {previous}</c>\n\n",
                stamp(start),
                stamp(start + 500),
                stamp(start + 150),
                stamp(start + 300)
            ));
        }
        vtt
    }

    #[test]
    fn twelve_hours_of_rolling_auto_captions_normalize_within_the_caps_and_in_linear_time() {
        let vtt = rolling_auto_caption_vtt(12);
        assert!(vtt.len() > 8_000_000, "fixture is {} bytes", vtt.len());
        assert!(vtt.len() < MAX_VTT_BYTES);
        let line_count = vtt.lines().count();
        assert!(line_count > 400_000, "fixture has {line_count} lines");

        let started = std::time::Instant::now();
        let json = normalize_vtt_json(vtt.as_bytes(), metadata()).expect("12 h must normalize");
        let elapsed = started.elapsed();
        eprintln!("12h fixture: {} bytes, {line_count} lines, json {} bytes, {elapsed:?}", vtt.len(), json.len());
        assert!(json.len() <= MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES);
        let normalized: NormalizedTranscript = serde_json::from_slice(&json).unwrap();
        assert_eq!(normalized.cues.len(), 12 * 3600 * 2);
        assert_eq!(normalized.cues[0].text, "so so\n\ntoday today >> so");
        assert_eq!(normalized.cues.last().unwrap().end_ms, 12 * 3_600_000);
        // Unoptimized test builds do this in a few seconds; a quadratic pass would not finish.
        assert!(elapsed < std::time::Duration::from_secs(60), "took {elapsed:?}");
    }

    #[test]
    fn a_huge_run_of_ampersands_in_one_cue_line_is_linear() {
        // 4 MiB of '&' followed by one distant ';' made the entity scan quadratic: every
        // '&' searched to the end of the line for its terminator.
        let run = format!("{};", "&".repeat(4 * 1024 * 1024));
        let raw = format!("WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n{run}\n");
        let started = std::time::Instant::now();
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).unwrap();
        assert_eq!(normalized.cues[0].text, run);
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        // Entities still decode next to the run.
        let mixed = format!("WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n{}&amp;&{}&lt;\n", "&".repeat(40), "x".repeat(40));
        let normalized = normalize_vtt(mixed.as_bytes(), metadata()).unwrap();
        assert_eq!(
            normalized.cues[0].text,
            format!("{}&&{}<", "&".repeat(40), "x".repeat(40))
        );
    }

    #[test]
    fn an_entity_is_decoded_only_within_its_32_byte_limit() {
        let at_limit = format!("#x{}41", "0".repeat(28));
        let over_limit = format!("#x{}41", "0".repeat(29));
        assert_eq!(at_limit.len(), 32);
        let raw = format!("WEBVTT

00:00:00.000 --> 00:00:01.000
&{at_limit}; &{over_limit}; &#x41;
");
        let normalized = normalize_vtt(raw.as_bytes(), metadata()).unwrap();
        assert_eq!(normalized.cues[0].text, format!("A &{over_limit}; A"));
    }

    #[test]
    fn normalized_json_over_its_cap_is_refused_with_a_dedicated_error() {
        let raw = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nhello world\n";
        let json = normalize_vtt_json(raw.as_bytes(), metadata()).unwrap();
        assert_eq!(
            normalize_vtt_json_within(raw.as_bytes(), metadata(), json.len()).unwrap(),
            json
        );
        assert_eq!(
            normalize_vtt_json_within(raw.as_bytes(), metadata(), json.len() - 1),
            Err(TranscriptNormalizationError::OutputTooLarge {
                size_bytes: json.len(),
                max_bytes: json.len() - 1,
            })
        );
    }
}
