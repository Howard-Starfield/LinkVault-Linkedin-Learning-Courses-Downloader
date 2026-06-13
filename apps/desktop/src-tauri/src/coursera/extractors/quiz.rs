//! Quiz / exam extractor.

#![allow(dead_code)] // Phase 5 — wired by Phase 8

use crate::coursera::define::INSTRUCTIONS_HTML_MATHJAX_URL;
use crate::coursera::error::{CourseraError, CourseraResult};
use crate::coursera::extractors::{ExtractionContext, HtmlArtifact};
use crate::coursera::syllabus::ItemV2;

const QUIZ_HTML_TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<title>{title}</title>
<script type="text/javascript" async
  src="{mathjax}?config=TeX-AMS-MML_HTMLorMML"></script>
<style>
body {{ font-family: system-ui, sans-serif; padding: 2rem 3rem; max-width: 920px; margin: auto; }}
h1 {{ border-bottom: 1px solid #ccc; padding-bottom: 0.4rem; }}
</style>
</head>
<body>
<h1>{title}</h1>
<p>Quiz content fetched at runtime. Open this file in your browser.</p>
</body>
</html>"#;

/// Extract a quiz item. Returns a `HtmlArtifact` with the rendered HTML
/// body. The orchestrator writes it to disk and offers to open it.
pub async fn extract(_ctx: &ExtractionContext<'_>, item: &ItemV2) -> CourseraResult<HtmlArtifact> {
    if !_ctx.options.download_quizzes {
        return Err(CourseraError::Other(
            "download_quizzes is disabled".to_string(),
        ));
    }
    let html = render_html(&item.name);
    Ok(HtmlArtifact {
        filename: format!("{}.html", item.slug),
        html,
    })
}

/// Extract an exam item.
pub async fn extract_exam(
    _ctx: &ExtractionContext<'_>,
    item: &ItemV2,
) -> CourseraResult<HtmlArtifact> {
    if !_ctx.options.download_quizzes {
        return Err(CourseraError::Other(
            "download_quizzes is disabled".to_string(),
        ));
    }
    let html = render_html(&item.name);
    Ok(HtmlArtifact {
        filename: format!("{}.html", item.slug),
        html,
    })
}

fn render_html(title: &str) -> String {
    QUIZ_HTML_TEMPLATE
        .replace("{title}", title)
        .replace("{mathjax}", INSTRUCTIONS_HTML_MATHJAX_URL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coursera::client;
    use crate::coursera::config::CourseraOptions;
    use crate::coursera::syllabus::ItemV2;
    use serde_json::json;

    fn fixture_item() -> ItemV2 {
        ItemV2 {
            id: "i1".to_string(),
            type_name: "quiz".to_string(),
            name: "Quiz 1".to_string(),
            slug: "quiz-1".to_string(),
            asset_id: None,
            raw: json!({}),
        }
    }

    #[tokio::test]
    async fn extract_returns_html_when_quizzes_enabled() {
        let client = client::build_client().unwrap();
        let mut opts = CourseraOptions::default();
        opts.download_quizzes = true;
        let ctx = ExtractionContext::new(&client, &opts);
        let item = fixture_item();
        let html = extract(&ctx, &item).await.unwrap();
        assert_eq!(html.filename, "quiz-1.html");
        assert!(html.html.contains("Quiz 1"));
        assert!(html.html.contains("MathJax.js"));
    }

    #[tokio::test]
    async fn extract_errors_when_quizzes_disabled() {
        let client = client::build_client().unwrap();
        let opts = CourseraOptions::default();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = fixture_item();
        let result = extract(&ctx, &item).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn extract_exam_returns_html_when_enabled() {
        let client = client::build_client().unwrap();
        let mut opts = CourseraOptions::default();
        opts.download_quizzes = true;
        let ctx = ExtractionContext::new(&client, &opts);
        let item = ItemV2 {
            id: "i1".to_string(),
            type_name: "exam".to_string(),
            name: "Final Exam".to_string(),
            slug: "final-exam".to_string(),
            asset_id: None,
            raw: json!({}),
        };
        let html = extract_exam(&ctx, &item).await.unwrap();
        assert!(html.html.contains("Final Exam"));
    }

    #[test]
    fn render_html_includes_mathjax_url() {
        let html = render_html("My Quiz");
        assert!(html.contains(INSTRUCTIONS_HTML_MATHJAX_URL));
    }
}
