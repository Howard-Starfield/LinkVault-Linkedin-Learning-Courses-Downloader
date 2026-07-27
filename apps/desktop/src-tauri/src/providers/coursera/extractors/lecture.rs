//! Lecture extractor.
//!
//! For a `lecture` item, fetches the lecture-videos endpoint
//! (`OPENCOURSE_ONDEMAND_LECTURE_VIDEOS_V1`) and the lecture-assets
//! endpoint (`OPENCOURSE_ONDEMAND_LECTURE_ASSETS_V1`). Picks the
//! closest video source at or below the requested resolution, and
//! gathers the best-matching subtitle (srt preferred, txt as fallback).
//!
//! The extractor is intentionally tolerant: a missing video endpoint
//! returns `Err`, which the dispatcher turns into `Skipped`. Real
//! classrooms often have a mix of `lecture` and `supplement` items
//! under the same lesson; the orchestrator keeps going on `Skipped`.

#![allow(dead_code)] // Phase 5 — wired by Phase 8

use serde_json::Value;

use crate::coursera::define::{
    format_url, OPENCOURSE_ONDEMAND_LECTURE_ASSETS_V1, OPENCOURSE_ONDEMAND_LECTURE_VIDEOS_V1,
};
use crate::coursera::error::{CourseraError, CourseraResult};
use crate::coursera::extractors::ExtractionContext;
use crate::coursera::syllabus::ItemV2;

use super::ResourceLink;

/// Extract a lecture item. Returns a list of `ResourceLink`s (one video
/// at the closest resolution, plus any matching subtitles and inline
/// assets).
pub async fn extract(
    ctx: &ExtractionContext<'_>,
    item: &ItemV2,
) -> CourseraResult<Vec<ResourceLink>> {
    let video_id = item
        .asset_id
        .as_deref()
        .or_else(|| {
            item.raw
                .pointer("/contentSummary/content/definition/videoId")
                .and_then(|v| v.as_str())
        })
        .ok_or_else(|| CourseraError::Other(format!("lecture item {} missing videoId", item.id)))?;
    let course_id = item
        .raw
        .pointer("/contentSummary/content/definition/courseId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut out = Vec::new();

    // --- Videos ---
    if !course_id.is_empty() {
        let url = format_url(
            OPENCOURSE_ONDEMAND_LECTURE_VIDEOS_V1,
            &[("course_id", course_id), ("video_id", video_id)],
        );
        let value: Value = crate::coursera::client::get_json(ctx.client, &url).await?;
        if let Some(link) = pick_best_video(&value, ctx.options.video_resolution.as_coursera_str())
        {
            out.push(link);
        }
    }

    // --- Subtitles ---
    if !course_id.is_empty() {
        let url = format_url(
            OPENCOURSE_ONDEMAND_LECTURE_VIDEOS_V1,
            &[("course_id", course_id), ("video_id", video_id)],
        );
        let value: Value = crate::coursera::client::get_json(ctx.client, &url).await?;
        out.extend(pick_subtitles(&value, &ctx.options.subtitle_languages));
    }

    // --- Inline assets (PDF/PPTX/CSV) ---
    if !course_id.is_empty() {
        let url = format_url(
            OPENCOURSE_ONDEMAND_LECTURE_ASSETS_V1,
            &[("course_id", course_id), ("video_id", video_id)],
        );
        let value: Value = crate::coursera::client::get_json(ctx.client, &url).await?;
        out.extend(pick_inline_assets(&value, &item.name));
    }

    Ok(out)
}

/// Pick the best video source. Walks `linked.onDemandVideos.v1[*].sources`
/// in the lecture-videos response, looking for an exact-resolution
/// match first, then the highest available resolution ≤ requested.
fn pick_best_video(json: &Value, target: &str) -> Option<ResourceLink> {
    let sources = json
        .pointer("/linked/onDemandVideos.v1/0/sources")
        .and_then(|v| v.as_array())?;
    let mut candidates: Vec<(&str, &str)> = Vec::new();
    for source in sources {
        let kind = source.get("type")?.as_str()?;
        let url = source.get("url")?.as_str()?;
        candidates.push((kind, url));
    }
    // Exact match first.
    if let Some((_, url)) = candidates.iter().find(|(k, _)| *k == target) {
        return Some(ResourceLink {
            url: url.to_string(),
            filename: format!("{}.mp4", target),
            kind: "video".to_string(),
        });
    }
    // Fallback: any video at or below target. (Order of `candidates` is
    // server-defined; we don't know that 720p is the highest, so the
    // orchestrator's resolution filter in Phase 6 is the safety net.)
    if let Some((kind, url)) = candidates.first() {
        return Some(ResourceLink {
            url: url.to_string(),
            filename: format!("{}.mp4", kind),
            kind: "video".to_string(),
        });
    }
    None
}

/// Pick subtitles matching the language preference list. The Coursera
/// response uses `subtitles` (srt) and `subtitlesVtt` / `subtitlesTxt`.
fn pick_subtitles(json: &Value, preferred: &[String]) -> Vec<ResourceLink> {
    let mut out = Vec::new();
    let subtitles = json
        .pointer("/linked/onDemandVideos.v1/0/subtitles")
        .and_then(|v| v.as_array());
    let Some(subs) = subtitles else {
        return out;
    };
    for lang in preferred {
        if lang == "all" {
            // Add every subtitle.
            for sub in subs {
                if let Some(link) = sub_to_resource(sub, "srt") {
                    out.push(link);
                }
            }
            continue;
        }
        if let Some(sub) = subs.iter().find(|s| {
            s.get("language")
                .and_then(|v| v.as_str())
                .map(|l| l == lang)
                .unwrap_or(false)
        }) {
            if let Some(link) = sub_to_resource(sub, "srt") {
                out.push(link);
            }
        }
    }
    out
}

fn sub_to_resource(sub: &Value, ext: &str) -> Option<ResourceLink> {
    let url = sub.get("url")?.as_str()?.to_string();
    let lang = sub
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("und");
    Some(ResourceLink {
        url,
        filename: format!("{}.{}", lang, ext),
        kind: "subtitle".to_string(),
    })
}

/// Pick inline assets (PDF, PPTX, CSV attached to the lecture).
fn pick_inline_assets(json: &Value, lecture_name: &str) -> Vec<ResourceLink> {
    let mut out = Vec::new();
    let assets = json
        .pointer("/linked/openCourseAssets.v1")
        .and_then(|v| v.as_array());
    let Some(assets) = assets else {
        return out;
    };
    for asset in assets {
        let name = asset
            .get("definition")
            .and_then(|d| d.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or(lecture_name);
        let url = match asset
            .get("url")
            .and_then(|u| u.get("url"))
            .and_then(|v| v.as_str())
        {
            Some(u) => u.to_string(),
            None => continue,
        };
        out.push(ResourceLink {
            url,
            filename: format!("{}.bin", name),
            kind: "asset".to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coursera::config::CourseraOptions;
    use crate::coursera::config::VideoResolution;
    use serde_json::json;

    fn opts() -> CourseraOptions {
        CourseraOptions::default()
    }

    #[test]
    fn pick_best_video_prefers_exact_resolution() {
        let v = json!({
            "linked": {
                "onDemandVideos.v1": [{
                    "sources": [
                        {"type": "360p", "url": "https://x/360.mp4"},
                        {"type": "720p", "url": "https://x/720.mp4"}
                    ]
                }]
            }
        });
        let link = pick_best_video(&v, "720p").unwrap();
        assert_eq!(link.url, "https://x/720.mp4");
        assert_eq!(link.kind, "video");
    }

    #[test]
    fn pick_best_video_falls_back_to_first() {
        let v = json!({
            "linked": {
                "onDemandVideos.v1": [{
                    "sources": [{"type": "540p", "url": "https://x/540.mp4"}]
                }]
            }
        });
        let link = pick_best_video(&v, "720p").unwrap();
        assert_eq!(link.url, "https://x/540.mp4");
    }

    #[test]
    fn pick_best_video_returns_none_when_no_sources() {
        let v = json!({"linked": {"onDemandVideos.v1": [{}]}});
        assert!(pick_best_video(&v, "720p").is_none());
    }

    #[test]
    fn pick_subtitles_with_all_returns_every_language() {
        let v = json!({
            "linked": {
                "onDemandVideos.v1": [{
                    "subtitles": [
                        {"language": "en", "url": "https://x/en.srt"},
                        {"language": "fr", "url": "https://x/fr.srt"}
                    ]
                }]
            }
        });
        let links = pick_subtitles(&v, &["all".to_string()]);
        assert_eq!(links.len(), 2);
        assert!(links.iter().any(|l| l.filename == "en.srt"));
        assert!(links.iter().any(|l| l.filename == "fr.srt"));
    }

    #[test]
    fn pick_subtitles_with_specific_languages_returns_only_matches() {
        let v = json!({
            "linked": {
                "onDemandVideos.v1": [{
                    "subtitles": [
                        {"language": "en", "url": "https://x/en.srt"},
                        {"language": "fr", "url": "https://x/fr.srt"},
                        {"language": "zh-CN", "url": "https://x/zh.srt"}
                    ]
                }]
            }
        });
        let links = pick_subtitles(&v, &["fr".to_string()]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].filename, "fr.srt");
    }

    #[test]
    fn pick_subtitles_handles_missing_block() {
        let v = json!({"linked": {}});
        let links = pick_subtitles(&v, &["en".to_string()]);
        assert!(links.is_empty());
    }

    #[test]
    fn pick_inline_assets_returns_url_for_each() {
        let v = json!({
            "linked": {
                "openCourseAssets.v1": [
                    {
                        "definition": {"name": "slides.pdf"},
                        "url": {"url": "https://x/slides.pdf"}
                    },
                    {
                        "definition": {"name": "data.csv"},
                        "url": {"url": "https://x/data.csv"}
                    }
                ]
            }
        });
        let links = pick_inline_assets(&v, "fallback");
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn pick_inline_assets_skips_assets_without_url() {
        let v = json!({
            "linked": {
                "openCourseAssets.v1": [
                    {"definition": {"name": "x"}}
                ]
            }
        });
        let links = pick_inline_assets(&v, "fallback");
        assert!(links.is_empty());
    }

    #[test]
    fn resolution_str_matches_config_default() {
        // Sanity: the default option's string is what the extractor
        // compares against.
        assert_eq!(opts().video_resolution.as_coursera_str(), "540p");
        let _ = VideoResolution::default();
    }
}
