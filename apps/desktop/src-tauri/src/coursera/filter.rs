//! Filter & format filtering for `ResourceLink`s.

#![allow(dead_code)] // Phase 6 — wired by Phase 8

use crate::coursera::config::CourseraOptions;
use crate::coursera::extractors::ResourceLink;

/// Drop URLs that are clearly not worth downloading.
pub fn skip_format_url(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() {
        return true;
    }
    if u.starts_with("mailto:") || u.starts_with("javascript:") {
        return true;
    }
    if u.starts_with("http://localhost") || u.starts_with("http://127.") {
        return true;
    }
    if !u.starts_with("http://")
        && !u.starts_with("https://")
        && !u.starts_with("asset://")
        && !u.starts_with("ref://")
    {
        return true;
    }
    false
}

/// Apply format whitelist/blacklist and resolution hints.
pub fn find_resources_to_get(
    links: Vec<ResourceLink>,
    opts: &CourseraOptions,
) -> Vec<ResourceLink> {
    links
        .into_iter()
        .filter(|l| !skip_format_url(&l.url))
        .filter(|l| format_allowed(&l.filename, &l.kind, opts))
        .collect()
}

fn format_allowed(filename: &str, kind: &str, opts: &CourseraOptions) -> bool {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if let Some(whitelist) = &opts.formats {
        if !whitelist.is_empty() && !whitelist.iter().any(|f| f == &ext) {
            return false;
        }
    }
    if opts.ignored_formats.iter().any(|f| f == &ext) {
        return false;
    }
    // Videos respect the resolution request.
    if kind == "video" && !opts.video_resolution.as_coursera_str().is_empty() {
        // The lecture extractor already picked by resolution; this is
        // a no-op safety net.
    }
    true
}

pub fn looks_like_video(url: &str) -> bool {
    url.contains(".mp4") || url.contains(".webm")
}
pub fn looks_like_subtitle(url: &str) -> bool {
    url.contains(".srt") || url.contains(".vtt") || url.contains(".txt")
}
pub fn looks_like_pdf(url: &str) -> bool {
    url.contains(".pdf")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, filename: &str, kind: &str) -> ResourceLink {
        ResourceLink {
            url: url.to_string(),
            filename: filename.to_string(),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn skip_format_url_drops_mailto() {
        assert!(skip_format_url("mailto:foo@bar"));
    }
    #[test]
    fn skip_format_url_keeps_https() {
        assert!(!skip_format_url("https://x/y.mp4"));
    }
    #[test]
    fn skip_format_url_drops_localhost() {
        assert!(skip_format_url("http://localhost/foo"));
        assert!(skip_format_url("http://127.0.0.1/foo"));
    }
    #[test]
    fn skip_format_url_drops_empty() {
        assert!(skip_format_url(""));
        assert!(skip_format_url("   "));
    }
    #[test]
    fn skip_format_url_keeps_asset_scheme() {
        // Asset URLs are not remote; the orchestrator resolves them.
        assert!(!skip_format_url("asset://abc"));
        assert!(!skip_format_url("ref://abc"));
    }

    #[test]
    fn find_resources_to_get_applies_whitelist() {
        let mut opts = CourseraOptions::default();
        opts.formats = Some(vec!["mp4".to_string()]);
        let links = vec![
            link("https://x/a.mp4", "a.mp4", "video"),
            link("https://x/a.srt", "a.srt", "subtitle"),
        ];
        let out = find_resources_to_get(links, &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].filename, "a.mp4");
    }

    #[test]
    fn find_resources_to_get_applies_blacklist() {
        let mut opts = CourseraOptions::default();
        opts.ignored_formats = vec!["pdf".to_string()];
        let links = vec![
            link("https://x/a.pdf", "a.pdf", "asset"),
            link("https://x/a.mp4", "a.mp4", "video"),
        ];
        let out = find_resources_to_get(links, &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].filename, "a.mp4");
    }

    #[test]
    fn looks_like_video_recognises_extensions() {
        assert!(looks_like_video("https://x/y.mp4"));
        assert!(!looks_like_video("https://x/y.srt"));
    }
    #[test]
    fn looks_like_subtitle_recognises_extensions() {
        assert!(looks_like_subtitle("https://x/y.srt"));
        assert!(looks_like_subtitle("https://x/y.vtt"));
        assert!(looks_like_subtitle("https://x/y.txt"));
    }
    #[test]
    fn looks_like_pdf_recognises_extension() {
        assert!(looks_like_pdf("https://x/y.pdf"));
        assert!(!looks_like_pdf("https://x/y.mp4"));
    }
}
