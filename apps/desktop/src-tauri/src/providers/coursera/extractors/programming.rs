//! Programming-assignment extractor.

#![allow(dead_code)] // Phase 5 — wired by Phase 8

use serde_json::Value;

use crate::coursera::define::{format_url, OPENCOURSE_ONDEMAND_PROGRAMMING_V1};
use crate::coursera::error::{CourseraError, CourseraResult};
use crate::coursera::extractors::{ExtractionContext, ResourceLink};
use crate::coursera::syllabus::ItemV2;

/// Extract a programming assignment.
pub async fn extract(
    _ctx: &ExtractionContext<'_>,
    item: &ItemV2,
) -> CourseraResult<Vec<ResourceLink>> {
    let course_id = item
        .raw
        .pointer("/contentSummary/content/definition/courseId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            CourseraError::Other(format!("programming item {} missing courseId", item.id))
        })?;
    let element_id = item
        .raw
        .pointer("/contentSummary/content/definition/elementId")
        .and_then(|v| v.as_str())
        .unwrap_or(&item.id);

    let url = format_url(
        OPENCOURSE_ONDEMAND_PROGRAMMING_V1,
        &[("course_id", course_id), ("element_id", element_id)],
    );
    let _value: Value = crate::coursera::client::get_json(_ctx.client, &url).await?;
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use crate::coursera::client;
    use crate::coursera::config::CourseraOptions;
    use crate::coursera::syllabus::ItemV2;
    use serde_json::json;

    #[test]
    fn programming_item_smoke() {
        let _item = ItemV2 {
            id: "i1".to_string(),
            type_name: "gradedProgramming".to_string(),
            name: "Assignment 1".to_string(),
            slug: "assignment-1".to_string(),
            asset_id: None,
            raw: json!({
                "contentSummary": {
                    "content": {
                        "definition": {
                            "courseId": "COURSE",
                            "elementId": "ELEM"
                        }
                    }
                }
            }),
        };
        let _opts = CourseraOptions::default();
        let _client = client::build_client().unwrap();
    }
}
