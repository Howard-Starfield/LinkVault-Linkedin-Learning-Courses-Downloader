//! Per-content-type extractors and a `dispatch` entry point.
//!
//! Each `ItemV2` from the syllabus is routed to exactly one extractor
//! based on its `type_name`. The extractor returns a `DispatchResult`:
//! either a flat list of `ResourceLink`s (downloadable URLs), a
//! `QuizHtml` / `ExamHtml` pair (written to disk and opened in the
//! default browser), or `Skipped` for types we do not handle in v1.
//!
//! Isolation note: this module is owned by `coursera/`. It uses the
//! `coursera::client` HTTP helpers and the `coursera::syllabus::ItemV2`
//! type, never any LinkedIn-side module.

#![allow(dead_code)] // Phase 5 — public symbols wired by Phase 8

use reqwest::Client;
use serde::Serialize;

use crate::coursera::config::CourseraOptions;
use crate::coursera::syllabus::ItemV2;

pub mod lecture;
pub mod notebook;
pub mod programming;
pub mod quiz;
pub mod resources;
pub mod supplement;

/// A downloadable resource: a remote URL plus a target filename.
/// `kind` is a free-form tag (`"video"`, `"subtitle"`, `"pdf"`, ...)
/// used by the filter and the orchestrator's counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceLink {
    pub url: String,
    pub filename: String,
    pub kind: String,
}

/// Static HTML body to write to disk for a quiz / exam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HtmlArtifact {
    pub filename: String,
    pub html: String,
}

/// What the extractor pipeline produces for one `ItemV2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispatchResult {
    /// A list of downloadable resources.
    Links(Vec<ResourceLink>),
    /// A quiz page (filename + HTML body).
    QuizHtml(HtmlArtifact),
    /// An exam page (filename + HTML body).
    ExamHtml(HtmlArtifact),
    /// The item was not handled in v1; `reason` is human-readable.
    Skipped { reason: String },
}

/// Context threaded through the extractors. Holding a `&Client` and
/// `&CourseraOptions` keeps the dispatch signature short.
pub struct ExtractionContext<'a> {
    pub client: &'a Client,
    pub options: &'a CourseraOptions,
}

impl<'a> ExtractionContext<'a> {
    pub fn new(client: &'a Client, options: &'a CourseraOptions) -> Self {
        Self { client, options }
    }
}

/// Dispatch an `ItemV2` to the right extractor. The pattern match is
/// deliberately exhaustive on the well-known type names; anything else
/// returns `Skipped` so the orchestrator can move on.
pub async fn dispatch(ctx: &ExtractionContext<'_>, item: &ItemV2) -> DispatchResult {
    match item.type_name.as_str() {
        "lecture" => match lecture::extract(ctx, item).await {
            Ok(links) => DispatchResult::Links(links),
            Err(e) => DispatchResult::Skipped {
                reason: format!("lecture extractor failed: {}", e),
            },
        },
        "supplement" | "read" | "reading" | "asset" => match supplement::extract(ctx, item).await {
            Ok(links) => DispatchResult::Links(links),
            Err(e) => DispatchResult::Skipped {
                reason: format!("supplement extractor failed: {}", e),
            },
        },
        "quiz" => match quiz::extract(ctx, item).await {
            Ok(html) => DispatchResult::QuizHtml(html),
            Err(e) => DispatchResult::Skipped {
                reason: format!("quiz extractor failed: {}", e),
            },
        },
        "exam" => match quiz::extract_exam(ctx, item).await {
            Ok(html) => DispatchResult::ExamHtml(html),
            Err(e) => DispatchResult::Skipped {
                reason: format!("exam extractor failed: {}", e),
            },
        },
        "gradedProgramming" | "ungradedProgramming" | "phasedPeer" | "programming" => {
            match programming::extract(ctx, item).await {
                Ok(links) => DispatchResult::Links(links),
                Err(e) => DispatchResult::Skipped {
                    reason: format!("programming extractor failed: {}", e),
                },
            }
        }
        "notebook" => match notebook::extract(ctx, item).await {
            Ok(links) => DispatchResult::Links(links),
            Err(e) => DispatchResult::Skipped {
                reason: format!("notebook extractor failed: {}", e),
            },
        },
        "peer" => DispatchResult::Skipped {
            reason: "peer review assignments are not supported in v1".to_string(),
        },
        "discussionPrompt" | "staff" | "ungradedWidget" | "plugin" | "lti" => {
            DispatchResult::Skipped {
                reason: format!("type '{}' not supported in v1", item.type_name),
            }
        }
        other => DispatchResult::Skipped {
            reason: format!("unknown type_name: {}", other),
        },
    }
}

/// Convenience: list every `ResourceLink` across an entire `ModulesV1`.
/// Used by tests and the "syllabus preview" command in Phase 10.
pub fn all_dispatched_links(results: &[DispatchResult]) -> Vec<ResourceLink> {
    let mut out = Vec::new();
    for r in results {
        if let DispatchResult::Links(links) = r {
            out.extend(links.iter().cloned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coursera::config::CourseraOptions;
    use serde_json::json;

    fn dummy_ctx() -> (Client, CourseraOptions) {
        (
            crate::coursera::client::build_client().unwrap(),
            CourseraOptions::default(),
        )
    }

    fn make_item(type_name: &str) -> ItemV2 {
        ItemV2 {
            id: "i1".to_string(),
            type_name: type_name.to_string(),
            name: "Item".to_string(),
            slug: "item".to_string(),
            asset_id: None,
            raw: json!({}),
        }
    }

    #[tokio::test]
    async fn dispatch_routes_lecture_to_lecture_extractor() {
        let (client, opts) = dummy_ctx();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = make_item("lecture");
        // The lecture extractor needs a `videoId` in the raw blob;
        // without one it returns Err, which `dispatch` maps to Skipped.
        let result = dispatch(&ctx, &item).await;
        assert!(matches!(result, DispatchResult::Skipped { .. }));
    }

    #[tokio::test]
    async fn dispatch_routes_supplement_to_supplement_extractor() {
        let (client, opts) = dummy_ctx();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = make_item("supplement");
        let result = dispatch(&ctx, &item).await;
        assert!(matches!(result, DispatchResult::Skipped { .. }));
    }

    #[tokio::test]
    async fn dispatch_routes_quiz_to_quiz_extractor() {
        let (client, opts) = dummy_ctx();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = make_item("quiz");
        let result = dispatch(&ctx, &item).await;
        assert!(matches!(result, DispatchResult::Skipped { .. }));
    }

    #[tokio::test]
    async fn dispatch_routes_exam_to_exam_extractor() {
        let (client, opts) = dummy_ctx();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = make_item("exam");
        let result = dispatch(&ctx, &item).await;
        assert!(matches!(result, DispatchResult::Skipped { .. }));
    }

    #[tokio::test]
    async fn dispatch_routes_programming_to_programming_extractor() {
        let (client, opts) = dummy_ctx();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = make_item("gradedProgramming");
        let result = dispatch(&ctx, &item).await;
        assert!(matches!(result, DispatchResult::Skipped { .. }));
    }

    #[tokio::test]
    async fn dispatch_routes_unknown_type_to_skipped() {
        let (client, opts) = dummy_ctx();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = make_item("frobnicated");
        let result = dispatch(&ctx, &item).await;
        match result {
            DispatchResult::Skipped { reason } => {
                assert!(reason.contains("unknown"));
            }
            _ => panic!("expected Skipped, got {:?}", result),
        }
    }

    #[test]
    fn all_dispatched_links_collects_only_link_variants() {
        let results = vec![
            DispatchResult::Links(vec![ResourceLink {
                url: "https://x".into(),
                filename: "a.mp4".into(),
                kind: "video".into(),
            }]),
            DispatchResult::Skipped {
                reason: "n/a".into(),
            },
            DispatchResult::Links(vec![ResourceLink {
                url: "https://y".into(),
                filename: "b.srt".into(),
                kind: "subtitle".into(),
            }]),
        ];
        let flat = all_dispatched_links(&results);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].filename, "a.mp4");
        assert_eq!(flat[1].filename, "b.srt");
    }
}
