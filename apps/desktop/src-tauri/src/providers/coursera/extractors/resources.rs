//! "Resources" tab extractor.

#![allow(dead_code)] // Phase 5 — wired by Phase 8

use crate::coursera::define::{format_url, OPENCOURSE_ONDEMAND_REFERENCES_V1};
use crate::coursera::error::{CourseraError, CourseraResult};
use crate::coursera::extractors::{ExtractionContext, ResourceLink};

/// Fetch the Resources tab for a course.
pub async fn fetch_for_course(
    ctx: &ExtractionContext<'_>,
    course_id: &str,
) -> CourseraResult<Vec<ResourceLink>> {
    let url = format_url(
        OPENCOURSE_ONDEMAND_REFERENCES_V1,
        &[("course_id", course_id)],
    );
    let value: serde_json::Value = crate::coursera::client::get_json(ctx.client, &url).await?;
    let mut out = Vec::new();
    let elements = value
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            CourseraError::Other("references endpoint missing 'elements'".to_string())
        })?;
    for el in elements {
        let name = el
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("resource");
        let short_id = el.get("shortId").and_then(|v| v.as_str()).unwrap_or("");
        if short_id.is_empty() {
            continue;
        }
        out.push(ResourceLink {
            url: format!("ref://{}", short_id),
            filename: format!("{}.bin", name),
            kind: "resource".to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        // The actual endpoint call is exercised by the integration
        // test; this is a smoke to keep the linker honest.
    }
}
