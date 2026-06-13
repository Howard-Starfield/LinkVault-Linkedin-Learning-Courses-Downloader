//! All typed configuration for the Coursera tab.
//!
//! Every option the Python `coursera-dl` CLI accepts is represented here.
//! Defaults match the Python tool. All types that cross the Tauri boundary
//! use `#[serde(rename_all = "camelCase")]` so the React side gets
//! idiomatic JSON.

// Phase 2: every public symbol in this file is consumed by Phase 3+ but
// not by the lib build yet. The blanket allow here is the same pattern
// used in `utils.rs` and `error.rs`. When each symbol gets its first
// non-test caller, the inner `#[allow(dead_code)]` annotations below
// should be removed.
#![allow(dead_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::coursera::error::{CourseraError, CourseraResult};

// ---------------------------------------------------------------------------
// Video resolution
// ---------------------------------------------------------------------------

/// Video resolution requested by the user. Coursera serves 360p / 540p / 720p
/// on the on-demand platform; the orchestrator will pick the best available
/// ≤ requested, falling back to the next lower if the exact match is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoResolution {
    #[serde(rename = "360p")]
    R360p,
    #[serde(rename = "540p")]
    R540p,
    #[serde(rename = "720p")]
    R720p,
}

impl VideoResolution {
    pub fn as_coursera_str(self) -> &'static str {
        match self {
            VideoResolution::R360p => "360p",
            VideoResolution::R540p => "540p",
            VideoResolution::R720p => "720p",
        }
    }
}

impl Default for VideoResolution {
    fn default() -> Self {
        // Mirrors the Python tool's default of 540p.
        VideoResolution::R540p
    }
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// How the user is authenticated. Stored in a `SavedCourseraPreferences`
/// variant only as the *type*; the actual CAUTH is never put in any
/// struct that crosses the Tauri boundary.
#[allow(dead_code)] // wired in by Phase 10 (Tauri commands)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(dead_code)] // variants are added in Phase 3/10
pub enum AuthMethod {
    /// A CAUTH cookie value pasted from the user's browser devtools.
    Cauth { cauth: String },
    /// Email + password, used to log in once and obtain a CAUTH.
    EmailPassword { email: String, password: String },
    /// Reuse the CAUTH stored in the DPAPI file (no secret in this struct).
    SavedToken,
}

// ---------------------------------------------------------------------------
// Course slug parsing
// ---------------------------------------------------------------------------

/// One parsed class. The frontend textarea may contain raw slugs ("ml-005")
/// or full URLs ("https://www.coursera.org/learn/ml-005?foo=bar"). Both
/// are accepted and normalized here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCourseraClass {
    pub original: String,
    pub slug: String,
    pub normalized_url: String,
}

/// Parse a single line of user input. Returns `Ok(None)` for blank lines.
pub fn parse_one_class(input: &str) -> CourseraResult<Option<ParsedCourseraClass>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let slug = if let Some(rest) = trimmed.strip_prefix("http://") {
        extract_slug_from_url(rest)
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        extract_slug_from_url(rest)
    } else {
        // Treat the whole trimmed string as a slug. Validate: only
        // [a-z0-9-].
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(CourseraError::InvalidArgument(format!(
                "invalid course slug: {}",
                trimmed
            )));
        }
        trimmed.to_string()
    };

    let normalized_url = format!("https://www.coursera.org/learn/{}", slug);
    Ok(Some(ParsedCourseraClass {
        original: trimmed.to_string(),
        slug,
        normalized_url,
    }))
}

fn extract_slug_from_url(rest: &str) -> String {
    // Find `/learn/<slug>` and return the slug, stripping query/fragment/trailing slash.
    let lower = rest.to_ascii_lowercase();
    if let Some(idx) = lower.find("/learn/") {
        let after = &rest[idx + "/learn/".len()..];
        // Slug ends at the next `/`, `?`, or `#`.
        let end = after
            .find(|c: char| c == '/' || c == '?' || c == '#')
            .unwrap_or(after.len());
        after[..end].to_string()
    } else {
        // Fallback: treat the first path segment as the slug.
        let end = rest
            .find(|c: char| c == '/' || c == '?' || c == '#')
            .unwrap_or(rest.len());
        rest[..end].to_string()
    }
}

/// Parse a multi-line input. Blank lines are ignored; order is preserved;
/// duplicates are allowed (the UI de-dupes if it wants to).
pub fn parse_class_input(input: &str) -> CourseraResult<Vec<ParsedCourseraClass>> {
    let mut out = Vec::new();
    for line in input.lines() {
        if let Some(parsed) = parse_one_class(line)? {
            out.push(parsed);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Subtitle languages parser
// ---------------------------------------------------------------------------

/// Parse a `--subtitle-language`-style string. The Python tool accepts
/// comma-separated language lists with optional pipe-separated fallback chains.
///
/// Examples:
/// - `"all"` → `vec!["all"]`
/// - `"en"` → `vec!["en"]`
/// - `"en|fr"` → `vec!["en", "fr"]` (fallback chain: try en, then fr)
/// - `"en,zh-CN"` → `vec!["en", "zh-CN"]` (two separate preferred languages)
/// - `"en|fr,zh-CN|zh-TW"` → `vec!["en", "fr", "zh-CN", "zh-TW"]`
pub fn parse_subtitle_languages(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return vec!["all".to_string()];
    }
    let mut out = Vec::new();
    for group in trimmed.split(',') {
        for lang in group.split('|') {
            let l = lang.trim();
            if !l.is_empty() {
                out.push(l.to_string());
            }
        }
    }
    if out.is_empty() {
        vec!["all".to_string()]
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Format list parser
// ---------------------------------------------------------------------------

/// Parse a space-separated list of file extensions. Used for both
/// `--formats` (whitelist) and as the foundation for `--ignore-formats`.
pub fn parse_format_list(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// The full set of options the Python `coursera-dl` CLI accepts, plus
/// a few that are app-specific (output dir, parallel jobs).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseraOptions {
    /// Course slugs, in user-entered order.
    pub class_names: Vec<String>,

    /// Where to save everything. Mirrors `--path`.
    pub output_dir: PathBuf,

    /// Subtitle language preference. Mirrors `--subtitle-language`.
    pub subtitle_languages: Vec<String>,

    /// Extensions to keep (whitelist). `None` means "all". Mirrors `--formats`.
    pub formats: Option<Vec<String>>,

    /// Extensions to skip (blacklist). Mirrors `--ignore-formats`.
    pub ignored_formats: Vec<String>,

    /// Section name regex filter. Mirrors `-sf` / `--section_filter`.
    pub section_filter: Option<String>,

    /// Lecture name regex filter. Mirrors `-lf` / `--lecture_filter`.
    pub lecture_filter: Option<String>,

    /// Resource title regex filter. Mirrors `-rf` / `--resource_filter`.
    pub resource_filter: Option<String>,

    /// Requested video resolution. Mirrors `--video-resolution`.
    pub video_resolution: VideoResolution,

    /// Number of parallel download workers. Mirrors `--jobs`.
    pub jobs: u8,

    /// Sleep between courses (avoid rate limiting). Mirrors `--download-delay`.
    pub download_delay_secs: u64,

    /// Save quiz/exam questions as static HTML. Mirrors `--download-quizzes`.
    pub download_quizzes: bool,

    /// Pull Jupyter notebooks from the Coursera notebooks hub. Mirrors `--download-notebooks`.
    pub download_notebooks: bool,

    /// Save the "About this course" metadata. Mirrors `--about`.
    pub download_about: bool,

    /// Expand specialization slugs to their member course slugs. Mirrors `--specialization`.
    pub specialization: bool,

    /// HTTP `Range:` resume for incomplete files. Mirrors `--resume`.
    pub resume: bool,

    /// Re-download even if the file exists. Mirrors `-o` / `--overwrite`.
    pub overwrite: bool,

    /// Prefix section dir with the course name. Mirrors `--verbose-dirs`.
    pub verbose_dirs: bool,

    /// Filenames include section and lecture numbers. Mirrors `--combined-section-lectures-nums`.
    pub combined_section_lectures_nums: bool,

    /// Allow non-ASCII characters in filenames. Mirrors `--unrestricted-filenames`.
    pub unrestricted_filenames: bool,

    /// Reverse the order of sections. Mirrors `-r` / `--reverse`.
    pub reverse: bool,

    /// Generate M3U playlists after each section. Mirrors `-pl` / `--playlist`.
    pub playlist: bool,

    /// Parse syllabus, write JSON, then exit. Mirrors `--only-syllabus`.
    pub only_syllabus: bool,

    /// Cache the parsed syllabus for reuse. Mirrors `--cache-syllabus`.
    pub cache_syllabus: bool,

    /// Force download of every URL. Mirrors `--disable-url-skipping`.
    pub disable_url_skipping: bool,
}

/// Subset of options consumed by `get_modules()`.
#[derive(Debug, Clone)]
pub struct ModuleGetOpts {
    pub reverse: bool,
    pub unrestricted_filenames: bool,
    pub subtitle_languages: Vec<String>,
    pub video_resolution: VideoResolution,
    pub download_quizzes: bool,
    pub download_notebooks: bool,
}

impl Default for CourseraOptions {
    fn default() -> Self {
        // Defaults mirror the Python `coursera-dl` tool exactly.
        Self {
            class_names: Vec::new(),
            output_dir: PathBuf::from("."),
            subtitle_languages: parse_subtitle_languages("all"),
            formats: None,
            ignored_formats: Vec::new(),
            section_filter: None,
            lecture_filter: None,
            resource_filter: None,
            video_resolution: VideoResolution::default(),
            jobs: 1,
            download_delay_secs: 60,
            download_quizzes: false,
            download_notebooks: false,
            download_about: false,
            specialization: false,
            resume: false,
            overwrite: false,
            verbose_dirs: false,
            combined_section_lectures_nums: false,
            unrestricted_filenames: false,
            reverse: false,
            playlist: false,
            only_syllabus: false,
            cache_syllabus: false,
            disable_url_skipping: false,
        }
    }
}

impl CourseraOptions {
    /// Validate option values. Called before any work starts.
    pub fn validate(&self) -> CourseraResult<()> {
        if self.jobs == 0 {
            return Err(CourseraError::InvalidArgument(
                "jobs must be >= 1".to_string(),
            ));
        }
        // Reject obviously bad regex sources.
        for (name, src) in [
            ("section_filter", &self.section_filter),
            ("lecture_filter", &self.lecture_filter),
            ("resource_filter", &self.resource_filter),
        ] {
            if let Some(s) = src.as_deref() {
                if !s.is_empty() {
                    regex::Regex::new(s)
                        .map_err(|e| CourseraError::InvalidArgument(format!("{}: {}", name, e)))?;
                }
            }
        }
        if self.class_names.is_empty() {
            return Err(CourseraError::InvalidArgument(
                "at least one class name is required".to_string(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

/// What the UI persists across sessions. Mirrors the LinkedIn-side
/// `SavedDownloadPreferences` shape. Does NOT contain any secret material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedCourseraPreferences {
    pub output_dir: String,
    pub selected_resolution: String,
    pub formats: Vec<String>,
    pub ignored_formats: Vec<String>,
    pub subtitle_language: String,
    pub download_quizzes: bool,
    pub download_notebooks: bool,
    pub download_about: bool,
    pub resume: bool,
    pub overwrite: bool,
    pub generate_playlists: bool,
    pub section_filter: String,
    pub lecture_filter: String,
    pub resource_filter: String,
    pub jobs: u8,
    pub download_delay_seconds: u64,
}

impl Default for SavedCourseraPreferences {
    fn default() -> Self {
        let opts = CourseraOptions::default();
        Self {
            output_dir: ".".to_string(),
            selected_resolution: opts.video_resolution.as_coursera_str().to_string(),
            formats: Vec::new(),
            ignored_formats: Vec::new(),
            subtitle_language: "all".to_string(),
            download_quizzes: false,
            download_notebooks: false,
            download_about: false,
            resume: false,
            overwrite: false,
            generate_playlists: false,
            section_filter: String::new(),
            lecture_filter: String::new(),
            resource_filter: String::new(),
            jobs: 1,
            download_delay_seconds: 60,
        }
    }
}

// ---------------------------------------------------------------------------
// Start request
// ---------------------------------------------------------------------------

/// The exact shape the React side sends to `start_coursera_download_jobs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCourseraRequest {
    pub classes: Vec<String>,
    pub output_dir: String,
    #[serde(default)]
    pub force_redownload: bool,
    pub selected_resolution: String,
    pub formats: Vec<String>,
    pub ignored_formats: Vec<String>,
    pub subtitle_language: String,
    pub download_quizzes: bool,
    pub download_notebooks: bool,
    pub download_about: bool,
    pub resume: bool,
    pub overwrite: bool,
    pub generate_playlists: bool,
    pub section_filter: String,
    pub lecture_filter: String,
    pub resource_filter: String,
    pub jobs: u8,
    pub download_delay_seconds: u64,
}

impl StartCourseraRequest {
    /// Build a `CourseraOptions` from the request. Returns `Err` on bad input.
    pub fn into_options(self) -> CourseraResult<CourseraOptions> {
        let selected_resolution = match self.selected_resolution.as_str() {
            "360p" => VideoResolution::R360p,
            "540p" => VideoResolution::R540p,
            "720p" => VideoResolution::R720p,
            other => {
                return Err(CourseraError::InvalidArgument(format!(
                    "unknown video resolution: {}",
                    other
                )))
            }
        };

        let subtitle_languages = parse_subtitle_languages(&self.subtitle_language);
        let formats = if self.formats.is_empty() {
            None
        } else {
            Some(self.formats)
        };
        let section_filter = opt_string(self.section_filter);
        let lecture_filter = opt_string(self.lecture_filter);
        let resource_filter = opt_string(self.resource_filter);

        let opts = CourseraOptions {
            class_names: self.classes,
            output_dir: PathBuf::from(self.output_dir),
            subtitle_languages,
            formats,
            ignored_formats: self.ignored_formats,
            section_filter,
            lecture_filter,
            resource_filter,
            video_resolution: selected_resolution,
            jobs: self.jobs.max(1),
            download_delay_secs: self.download_delay_seconds,
            download_quizzes: self.download_quizzes,
            download_notebooks: self.download_notebooks,
            download_about: self.download_about,
            specialization: false,
            resume: self.resume,
            overwrite: self.overwrite,
            verbose_dirs: false,
            combined_section_lectures_nums: false,
            unrestricted_filenames: false,
            reverse: false,
            playlist: self.generate_playlists,
            only_syllabus: false,
            cache_syllabus: false,
            disable_url_skipping: false,
        };
        opts.validate()?;
        Ok(opts)
    }
}

fn opt_string(s: String) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
#[allow(dead_code)] // Phase 2 only — used by Phase 3+ via `pub use`
mod tests {
    use super::*;

    #[test]
    fn video_resolution_as_coursera_str() {
        assert_eq!(VideoResolution::R360p.as_coursera_str(), "360p");
        assert_eq!(VideoResolution::R540p.as_coursera_str(), "540p");
        assert_eq!(VideoResolution::R720p.as_coursera_str(), "720p");
    }

    #[test]
    fn video_resolution_serde_roundtrip() {
        for v in [
            VideoResolution::R360p,
            VideoResolution::R540p,
            VideoResolution::R720p,
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: VideoResolution = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn parse_one_class_accepts_raw_slug() {
        let p = parse_one_class("ml-005").unwrap().unwrap();
        assert_eq!(p.slug, "ml-005");
        assert_eq!(p.normalized_url, "https://www.coursera.org/learn/ml-005");
    }

    #[test]
    fn parse_one_class_accepts_full_url() {
        let p = parse_one_class("https://www.coursera.org/learn/algorithms")
            .unwrap()
            .unwrap();
        assert_eq!(p.slug, "algorithms");
        assert_eq!(
            p.normalized_url,
            "https://www.coursera.org/learn/algorithms"
        );
    }

    #[test]
    fn parse_one_class_strips_query_and_trailing_slash() {
        let p = parse_one_class("https://www.coursera.org/learn/ml-005/?foo=bar#anchor")
            .unwrap()
            .unwrap();
        assert_eq!(p.slug, "ml-005");
    }

    #[test]
    fn parse_one_class_strips_http_scheme() {
        let p = parse_one_class("http://coursera.org/learn/ml-005")
            .unwrap()
            .unwrap();
        assert_eq!(p.slug, "ml-005");
    }

    #[test]
    fn parse_one_class_returns_none_for_blank() {
        assert!(parse_one_class("").unwrap().is_none());
        assert!(parse_one_class("   ").unwrap().is_none());
    }

    #[test]
    fn parse_one_class_rejects_uppercase_slug() {
        // Slugs are lowercase by Coursera convention.
        assert!(parse_one_class("ML-005").is_err());
    }

    #[test]
    fn parse_class_input_preserves_order_and_skips_blanks() {
        let out = parse_class_input(
            "ml-005\n\n  algo-001  \nhttps://www.coursera.org/learn/crypto-002?x=y",
        )
        .unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].slug, "ml-005");
        assert_eq!(out[1].slug, "algo-001");
        assert_eq!(out[2].slug, "crypto-002");
    }

    #[test]
    fn parse_subtitle_languages_handles_all() {
        assert_eq!(parse_subtitle_languages("all"), vec!["all".to_string()]);
        assert_eq!(parse_subtitle_languages(""), vec!["all".to_string()]);
        assert_eq!(parse_subtitle_languages("   "), vec!["all".to_string()]);
    }

    #[test]
    fn parse_subtitle_languages_handles_single() {
        assert_eq!(parse_subtitle_languages("en"), vec!["en".to_string()]);
    }

    #[test]
    fn parse_subtitle_languages_handles_fallback_chain() {
        assert_eq!(
            parse_subtitle_languages("en|fr"),
            vec!["en".to_string(), "fr".to_string()]
        );
    }

    #[test]
    fn parse_subtitle_languages_handles_multiple_groups() {
        assert_eq!(
            parse_subtitle_languages("en,zh-CN"),
            vec!["en".to_string(), "zh-CN".to_string()]
        );
    }

    #[test]
    fn parse_subtitle_languages_handles_mixed() {
        assert_eq!(
            parse_subtitle_languages("en|fr,zh-CN|zh-TW"),
            vec![
                "en".to_string(),
                "fr".to_string(),
                "zh-CN".to_string(),
                "zh-TW".to_string()
            ]
        );
    }

    #[test]
    fn parse_format_list_lowercases_and_splits() {
        assert_eq!(
            parse_format_list("mp4 srt PDF"),
            vec!["mp4".to_string(), "srt".to_string(), "pdf".to_string()]
        );
    }

    #[test]
    fn parse_format_list_drops_empty_tokens() {
        assert_eq!(
            parse_format_list(" mp4   srt  "),
            vec!["mp4".to_string(), "srt".to_string()]
        );
        assert_eq!(parse_format_list(""), Vec::<String>::new());
    }

    #[test]
    fn coursera_options_default_matches_python_tool() {
        let d = CourseraOptions::default();
        assert_eq!(d.subtitle_languages, vec!["all".to_string()]);
        assert_eq!(d.jobs, 1);
        assert_eq!(d.download_delay_secs, 60);
        assert_eq!(d.video_resolution, VideoResolution::R540p);
        assert!(!d.download_quizzes);
        assert!(!d.resume);
        assert!(!d.overwrite);
        assert!(!d.playlist);
        assert!(d.class_names.is_empty());
    }

    #[test]
    fn coursera_options_validate_rejects_zero_jobs() {
        let mut o = CourseraOptions::default();
        o.jobs = 0;
        assert!(matches!(
            o.validate(),
            Err(CourseraError::InvalidArgument(_))
        ));
    }

    #[test]
    fn coursera_options_validate_rejects_bad_regex() {
        let mut o = CourseraOptions::default();
        o.class_names = vec!["ml-005".to_string()];
        o.section_filter = Some("(unclosed".to_string());
        assert!(matches!(
            o.validate(),
            Err(CourseraError::InvalidArgument(_))
        ));
    }

    #[test]
    fn coursera_options_validate_rejects_empty_class_names() {
        let o = CourseraOptions::default();
        // Default has empty class_names.
        assert!(matches!(
            o.validate(),
            Err(CourseraError::InvalidArgument(_))
        ));
    }

    #[test]
    fn coursera_options_validate_accepts_good_input() {
        let mut o = CourseraOptions::default();
        o.class_names = vec!["ml-005".to_string()];
        o.section_filter = Some("^Chapter_".to_string());
        o.lecture_filter = Some(".*intro.*".to_string());
        o.resource_filter = Some(".*\\.pdf$".to_string());
        o.validate().unwrap();
    }

    #[test]
    fn start_coursera_request_into_options() {
        let req = StartCourseraRequest {
            classes: vec!["ml-005".to_string()],
            output_dir: "C:/courses".to_string(),
            force_redownload: false,
            selected_resolution: "720p".to_string(),
            formats: vec!["mp4".to_string(), "srt".to_string()],
            ignored_formats: vec!["pdf".to_string()],
            subtitle_language: "en|fr,zh-CN".to_string(),
            download_quizzes: true,
            download_notebooks: false,
            download_about: false,
            resume: true,
            overwrite: false,
            generate_playlists: true,
            section_filter: String::new(),
            lecture_filter: String::new(),
            resource_filter: String::new(),
            jobs: 4,
            download_delay_seconds: 30,
        };
        let opts = req.into_options().unwrap();
        assert_eq!(opts.class_names, vec!["ml-005".to_string()]);
        assert_eq!(opts.video_resolution, VideoResolution::R720p);
        assert_eq!(opts.jobs, 4);
        assert_eq!(
            opts.subtitle_languages,
            vec!["en".to_string(), "fr".to_string(), "zh-CN".to_string()]
        );
        assert!(opts.download_quizzes);
        assert!(opts.resume);
        assert!(opts.playlist);
    }

    #[test]
    fn start_coursera_request_rejects_unknown_resolution() {
        let mut req = StartCourseraRequest {
            classes: vec!["ml-005".to_string()],
            output_dir: ".".to_string(),
            force_redownload: false,
            selected_resolution: "1080p".to_string(),
            formats: Vec::new(),
            ignored_formats: Vec::new(),
            subtitle_language: "all".to_string(),
            download_quizzes: false,
            download_notebooks: false,
            download_about: false,
            resume: false,
            overwrite: false,
            generate_playlists: false,
            section_filter: String::new(),
            lecture_filter: String::new(),
            resource_filter: String::new(),
            jobs: 1,
            download_delay_seconds: 60,
        };
        assert!(matches!(
            req.clone().into_options(),
            Err(CourseraError::InvalidArgument(_))
        ));
        // Jobs 0 should be clamped to 1 and then validate passes.
        req.selected_resolution = "540p".to_string();
        req.jobs = 0;
        let opts = req.into_options().unwrap();
        assert_eq!(opts.jobs, 1);
    }

    #[test]
    fn saved_preferences_default_roundtrips() {
        let s = SavedCourseraPreferences::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: SavedCourseraPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
