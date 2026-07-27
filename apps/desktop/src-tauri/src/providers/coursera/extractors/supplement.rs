//! Supplement (reading) extractor.

#![allow(dead_code)] // Phase 5 — wired by Phase 8

use serde_json::Value;

use crate::coursera::define::{format_url, OPENCOURSE_ONDEMAND_SUPPLEMENT_V1};
use crate::coursera::error::{CourseraError, CourseraResult};
use crate::coursera::extractors::ExtractionContext;
use crate::coursera::syllabus::ItemV2;

use super::ResourceLink;

/// Extract a supplement item. Returns a list of `ResourceLink`s pointing
/// at the asset URL.
pub async fn extract(
    _ctx: &ExtractionContext<'_>,
    item: &ItemV2,
) -> CourseraResult<Vec<ResourceLink>> {
    let course_id = item
        .raw
        .pointer("/contentSummary/content/definition/courseId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            CourseraError::Other(format!("supplement item {} missing courseId", item.id))
        })?;
    let element_id = item
        .raw
        .pointer("/contentSummary/content/definition/elementId")
        .and_then(|v| v.as_str())
        .unwrap_or(&item.id);

    let url = format_url(
        OPENCOURSE_ONDEMAND_SUPPLEMENT_V1,
        &[("course_id", course_id), ("element_id", element_id)],
    );

    // We currently don't act on the body — the orchestrator will
    // re-resolve the asset URL via `OPENCOURSE_ASSET_URL_V1` at write
    // time. We just verify the endpoint is reachable.
    let value: Value = crate::coursera::client::get_json(_ctx.client, &url).await?;
    let asset_id = value
        .pointer("/linked/openCourseAssets.v1/0/definition/assetId")
        .and_then(|v| v.as_str());

    let mut out = Vec::new();
    if let Some(asset_id) = asset_id {
        out.push(ResourceLink {
            url: format!("asset://{}", asset_id),
            filename: format!("{}.bin", item.slug),
            kind: "supplement".to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::coursera::client;
    use crate::coursera::config::CourseraOptions;
    use crate::coursera::syllabus::ItemV2;
    use serde_json::json;

    #[test]
    fn supplement_smoke_test_does_not_panic() {
        let _item = ItemV2 {
            id: "i1".to_string(),
            type_name: "supplement".to_string(),
            name: "Reading".to_string(),
            slug: "reading".to_string(),
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
