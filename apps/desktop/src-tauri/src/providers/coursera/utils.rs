//! Small helpers used across the `coursera/` module.
//!
//! All functions are pure (no I/O) and synchronous. Phase 1 port; no
//! futures, no async, no Tauri surface.

use std::path::Path;

/// Strip Windows-forbidden characters and control bytes from a filename.
///
/// - `unrestricted = false` (default): also strip non-ASCII characters,
///   so the filename is safe on any locale.
/// - `unrestricted = true`: keep non-ASCII characters (CJK, Cyrillic, etc.).
///
/// The result is truncated to 200 characters to keep within common
/// filesystem limits; the truncation is applied **after** cleaning and
/// preserves no extension (callers are expected to add an extension).
#[allow(dead_code)] // wired in by Phase 6 (formatting)
pub fn clean_filename(name: &str, unrestricted: bool) -> String {
    const MAX_LEN: usize = 200;

    let mut out = String::with_capacity(name.len().min(MAX_LEN));

    for ch in name.chars() {
        let to_underscore = ch == ' '
            || is_forbidden_char(ch)
            || (!unrestricted && !ch.is_ascii_alphanumeric() && !is_filename_safe_punct(ch));
        if to_underscore {
            out.push('_');
        } else {
            out.push(ch);
        }
    }

    // Collapse runs of underscores (and runs of whitespace that were just
    // mapped to single underscores).
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_underscore = false;
    for ch in out.chars() {
        if ch == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(ch);
            prev_underscore = false;
        }
    }

    // Trim leading underscores and dots (Windows hates them at the start).
    // Trailing underscores are also trimmed (no orphan `_` from a trailing
    // forbidden char). Trailing dots are kept — the file extension matters
    // and is added by the caller.
    let trimmed_start = collapsed.trim_start_matches(|c: char| c == '_' || c == '.');
    let trimmed = trimmed_start.trim_end_matches('_').to_string();

    // Truncate to MAX_LEN, breaking on a char boundary.
    if trimmed.len() <= MAX_LEN {
        trimmed
    } else {
        let mut idx = MAX_LEN;
        while !trimmed.is_char_boundary(idx) {
            idx -= 1;
        }
        trimmed[..idx].to_string()
    }
}

#[allow(dead_code)] // private helper for clean_filename
fn is_forbidden_char(ch: char) -> bool {
    // Windows + cross-platform forbidden filename characters.
    matches!(
        ch,
        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\x1f'
    )
}

#[allow(dead_code)] // private helper for clean_filename
fn is_filename_safe_punct(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '-' | '_' | '.' | '(' | ')' | '[' | ']' | ',' | '&' | '+' | '='
    )
}

/// Drop obviously-bad URLs (mailto:, localhost, empty).
#[allow(dead_code)] // wired in by Phase 6 (filter)
pub fn clean_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("mailto:") {
        return None;
    }
    if trimmed.starts_with("http://localhost") || trimmed.starts_with("http://127.") {
        return None;
    }
    Some(trimmed.to_string())
}

/// Recursive mkdir (`mkdir -p`). Returns `Ok` if the directory already
/// exists, otherwise creates it (and any missing parents).
#[allow(dead_code)] // wired in by Phase 6 (format) and Phase 8 (orchestrator)
pub fn mkdir_p(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        if path.is_dir() {
            return Ok(());
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("path exists and is not a directory: {}", path.display()),
        ));
    }
    std::fs::create_dir_all(path)
}

/// Best-effort UTF-8 decode with lossy fallback. Mirrors the Python
/// `decode_input` helper: log nothing, just produce a string.
#[allow(dead_code)] // wired in by Phase 3 (network wrappers) and Phase 4 (syllabus parse)
pub fn decode_input(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// True when debug logging is on. In Rust, debug assertions are the
/// only knob we have; the Python tool also respects a `--debug` flag,
/// which we surface in `config.rs` later and combine with this.
#[allow(dead_code)] // wired in by Phase 4 (syllabus dump) and Phase 8 (orchestrator)
pub fn is_debug_run() -> bool {
    cfg!(debug_assertions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_filename_strips_forbidden_chars() {
        assert_eq!(
            clean_filename("a<b>c:d\"e/f\\g|h?i*j", false),
            "a_b_c_d_e_f_g_h_i_j"
        );
    }

    #[test]
    fn clean_filename_collapses_runs() {
        assert_eq!(clean_filename("foo    bar", false), "foo_bar");
        assert_eq!(clean_filename("a///b", false), "a_b");
    }

    #[test]
    fn clean_filename_trims_leading_dots_and_trailing_underscores() {
        // Leading dots/underscores are trimmed.
        assert_eq!(clean_filename("...hello...", false), "hello...");
        assert_eq!(clean_filename("___hello___", false), "hello");
        // But not the other way around: trailing dots are kept (file
        // extension) and leading underscores in the middle are kept.
    }

    #[test]
    fn clean_filename_keeps_safe_ascii_punct() {
        // Spaces become `_`; `.mp4` is preserved (trim is conservative
        // about trailing dots so the file extension survives).
        assert_eq!(
            clean_filename("Section 1 - intro (v2).mp4", false),
            "Section_1_-_intro_(v2).mp4"
        );
    }

    #[test]
    fn clean_filename_strips_non_ascii_when_restricted() {
        // Trailing `_` from the dropped `é` is trimmed.
        assert_eq!(clean_filename("café", false), "caf");
    }

    #[test]
    fn clean_filename_keeps_non_ascii_when_unrestricted() {
        assert_eq!(clean_filename("café", true), "café");
        assert_eq!(clean_filename("日本語のタイトル", true), "日本語のタイトル");
    }

    #[test]
    fn clean_filename_truncates_to_200_chars() {
        let long = "a".repeat(500);
        let result = clean_filename(&long, false);
        assert_eq!(result.len(), 200);
    }

    #[test]
    fn clean_filename_strips_control_chars() {
        assert_eq!(clean_filename("foo\nbar\tbaz", false), "foo_bar_baz");
        assert_eq!(clean_filename("foo\x00bar", false), "foo_bar");
    }

    #[test]
    fn clean_url_drops_mailto() {
        assert_eq!(clean_url("mailto:foo@example.com"), None);
        assert_eq!(clean_url("  mailto:foo@example.com  "), None);
    }

    #[test]
    fn clean_url_drops_localhost() {
        assert_eq!(clean_url("http://localhost/foo"), None);
        assert_eq!(clean_url("http://127.0.0.1/foo"), None);
    }

    #[test]
    fn clean_url_drops_empty() {
        assert_eq!(clean_url(""), None);
        assert_eq!(clean_url("   "), None);
    }

    #[test]
    fn clean_url_keeps_normal_https() {
        assert_eq!(
            clean_url("https://example.com/file.mp4"),
            Some("https://example.com/file.mp4".to_string())
        );
    }

    #[test]
    fn mkdir_p_creates_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("a").join("b").join("c");
        assert!(!target.exists());
        mkdir_p(&target).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn mkdir_p_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("a");
        mkdir_p(&target).unwrap();
        // second call should also succeed
        mkdir_p(&target).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn mkdir_p_errors_when_path_is_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("not-a-dir");
        std::fs::write(&file_path, b"hi").unwrap();
        let target = file_path.join("subdir");
        let err = mkdir_p(&target).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn decode_input_handles_valid_utf8() {
        assert_eq!(decode_input("hello".as_bytes()), "hello");
        assert_eq!(decode_input("café".as_bytes()), "café");
    }

    #[test]
    fn decode_input_falls_back_on_invalid_bytes() {
        // 0xff is never valid UTF-8.
        let bytes = b"foo\xffbar";
        let result = decode_input(bytes);
        assert!(result.starts_with("foo"));
        assert!(result.ends_with("bar"));
        // The replacement character is U+FFFD.
        assert!(result.contains('\u{FFFD}'));
    }

    #[test]
    fn is_debug_run_returns_bool() {
        // Just exercise the symbol so the linker keeps it.
        let _ = is_debug_run();
    }

    #[test]
    fn clean_filename_handles_unicode_truncation_safely() {
        // 100 emoji = 400 bytes. Truncation must not split a codepoint.
        let long_emoji: String = "🦀".repeat(100);
        let result = clean_filename(&long_emoji, true);
        // Result is well-formed UTF-8 by construction.
        assert!(result.len() <= 200);
        // And no replacement char from a bad split.
        assert!(!result.contains('\u{FFFD}'));
    }

    #[test]
    fn clean_filename_empty_input_returns_empty() {
        assert_eq!(clean_filename("", false), "");
        assert_eq!(clean_filename("...", false), "");
        assert_eq!(clean_filename("___", false), "");
    }
}
