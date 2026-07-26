//! Shared pure naming rules for persisted newspaper IDs, edition identities, and paths.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;

use super::models::NewspaperEdition;

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn edition_key(edition: &NewspaperEdition) -> String {
    edition
        .publication_date
        .map(|date| format!("{}@{date}", edition.code))
        .unwrap_or_else(|| edition.code.clone())
}

pub(super) fn unique_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        Utc::now().timestamp_millis(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) fn sanitize_segment(value: &str) -> String {
    value
        .chars()
        .filter(|character| !r#"\/:*?"<>|"#.contains(*character) && !character.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}
