//! Notebook extractor.

#![allow(dead_code)] // Phase 5 — wired by Phase 8

use crate::coursera::error::CourseraResult;
use crate::coursera::extractors::{ExtractionContext, ResourceLink};
use crate::coursera::syllabus::ItemV2;

/// Extract notebook files.
pub async fn extract(
    _ctx: &ExtractionContext<'_>,
    _item: &ItemV2,
) -> CourseraResult<Vec<ResourceLink>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coursera::client;
    use crate::coursera::config::CourseraOptions;
    use crate::coursera::syllabus::ItemV2;
    use serde_json::json;

    #[tokio::test]
    async fn notebook_returns_empty_in_v1() {
        let client = client::build_client().unwrap();
        let opts = CourseraOptions::default();
        let ctx = ExtractionContext::new(&client, &opts);
        let item = ItemV2 {
            id: "i1".to_string(),
            type_name: "notebook".to_string(),
            name: "Notebook".to_string(),
            slug: "notebook".to_string(),
            asset_id: None,
            raw: json!({}),
        };
        let links = super::extract(&ctx, &item).await.unwrap();
        assert!(links.is_empty());
    }
}
