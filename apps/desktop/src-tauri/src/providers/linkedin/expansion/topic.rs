use regex::Regex;
use serde_json::Value;
use url::Url;

use crate::linkedin::is_reserved_or_hub_prefix;

// Topic listings mix Course, Video, and LEARNING_PATH cards. Keep paths only.
// Prefer JSON entities typed LEARNING_PATH, then /learning/paths/{slug} in HTML
// or JSON. Follow an explicit next URL when the body includes one. Do not
// synthesize guest ?start= paging or GraphQL queryIds.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicPage {
    pub path_slugs: Vec<String>,
    pub has_more: bool,
    pub next: Option<String>,
}

pub fn parse_topic_listing(body: &str) -> TopicPage {
    let trimmed = body.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return topic_page_from_json(&value, trimmed);
        }
    }

    let mut path_slugs = harvest_path_slugs(body);
    for script in json_script_bodies(body) {
        if let Ok(value) = serde_json::from_str::<Value>(script) {
            let page = topic_page_from_json(&value, script);
            merge_slugs(&mut path_slugs, page.path_slugs);
            if page.next.is_some() || page.has_more {
                return TopicPage {
                    path_slugs,
                    has_more: page.has_more,
                    next: page.next,
                };
            }
        }
    }

    TopicPage {
        path_slugs,
        has_more: false,
        next: None,
    }
}

fn topic_page_from_json(value: &Value, raw: &str) -> TopicPage {
    let mut path_slugs = Vec::new();
    collect_learning_path_slugs(value, &mut path_slugs);
    merge_slugs(&mut path_slugs, harvest_path_slugs(raw));
    let next = explicit_next_url(value);
    TopicPage {
        has_more: next.is_some(),
        next,
        path_slugs,
    }
}

fn collect_learning_path_slugs(value: &Value, path_slugs: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_learning_path_slugs(item, path_slugs);
            }
        }
        Value::Object(map) => {
            if is_learning_path_entity(value) {
                if let Some(slug) = path_slug_from_object(map) {
                    push_slug(path_slugs, slug);
                }
            }
            for nested in map.values() {
                collect_learning_path_slugs(nested, path_slugs);
            }
        }
        _ => {}
    }
}

fn is_learning_path_entity(value: &Value) -> bool {
    ["entityType", "type", "$type", "contentType"]
        .iter()
        .any(|key| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(|type_name| type_name.to_ascii_uppercase().contains("LEARNING_PATH"))
                .unwrap_or(false)
        })
}

fn path_slug_from_object(map: &serde_json::Map<String, Value>) -> Option<String> {
    for key in ["url", "canonicalUrl", "publicUrl", "trackingUrl"] {
        if let Some(url) = map.get(key).and_then(Value::as_str) {
            if let Some(slug) = path_slug_from_href(url) {
                return Some(slug);
            }
        }
    }
    map.get("slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|slug| !slug.is_empty() && !is_reserved_or_hub_prefix(slug))
        .map(ToString::to_string)
}

fn explicit_next_url(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => items.iter().find_map(explicit_next_url),
        Value::Object(map) => {
            for key in ["next", "nextPageUrl", "nextUrl"] {
                if let Some(url) = map.get(key).and_then(Value::as_str) {
                    if let Some(safe) = linkedin_next_url(url) {
                        return Some(safe);
                    }
                }
            }
            if let Some(paging) = map.get("paging") {
                if let Some(url) = explicit_next_url(paging) {
                    return Some(url);
                }
            }
            map.values().find_map(explicit_next_url)
        }
        _ => None,
    }
}

fn linkedin_next_url(candidate: &str) -> Option<String> {
    let with_protocol = if candidate.starts_with("http://") || candidate.starts_with("https://") {
        candidate.to_string()
    } else if candidate.starts_with('/') {
        format!("https://www.linkedin.com{candidate}")
    } else {
        return None;
    };
    let url = Url::parse(&with_protocol).ok()?;
    let host = url.host_str()?;
    if host == "linkedin.com" || host.ends_with(".linkedin.com") {
        Some(with_protocol)
    } else {
        None
    }
}

fn harvest_path_slugs(body: &str) -> Vec<String> {
    let Some(regex) = path_href_regex() else {
        return Vec::new();
    };
    let mut slugs = Vec::new();
    for capture in regex.captures_iter(body) {
        if let Some(slug) = capture.get(1).map(|matched| matched.as_str()) {
            push_slug(&mut slugs, slug.to_string());
        }
    }
    slugs
}

fn path_href_regex() -> Option<Regex> {
    Regex::new(r#"(?i)(?:https?://(?:www\.)?linkedin\.com)?/learning/paths/([A-Za-z0-9_-]+)"#).ok()
}

fn json_script_bodies(html: &str) -> Vec<&str> {
    let Some(regex) = json_script_regex() else {
        return Vec::new();
    };
    regex
        .captures_iter(html)
        .filter_map(|capture| capture.get(1).map(|matched| matched.as_str().trim()))
        .collect()
}

fn json_script_regex() -> Option<Regex> {
    Regex::new(r#"(?is)<script[^>]*type=["']application/(?:ld\+json|json)["'][^>]*>(.*?)</script>"#)
        .ok()
}

fn path_slug_from_href(href: &str) -> Option<String> {
    let normalized = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with('/') {
        format!("https://www.linkedin.com{href}")
    } else {
        return None;
    };
    let url = Url::parse(&normalized).ok()?;
    let mut segments = url.path_segments()?;
    if segments.next()? != "learning" {
        return None;
    }
    if segments.next()? != "paths" {
        return None;
    }
    let slug = segments.next()?.trim();
    if slug.is_empty() || is_reserved_or_hub_prefix(slug) {
        None
    } else {
        Some(slug.to_string())
    }
}

fn push_slug(slugs: &mut Vec<String>, slug: String) {
    if !slug.is_empty() && !slugs.iter().any(|existing| existing == &slug) {
        slugs.push(slug);
    }
}

fn merge_slugs(target: &mut Vec<String>, incoming: Vec<String>) {
    for slug in incoming {
        push_slug(target, slug);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_topic_listing;

    const MIXED_TOPIC_JSON: &str =
        include_str!("fixtures/topic-professional-certificates-mixed.json");

    #[test]
    fn mixed_course_video_and_path_keeps_paths_only() {
        let page = parse_topic_listing(MIXED_TOPIC_JSON);
        assert_eq!(
            page.path_slugs,
            vec![
                "career-essentials-in-github-professional-certificate",
                "career-essentials-in-software-development-by-microsoft-and-linkedin",
            ]
        );
        assert!(!page.path_slugs.iter().any(|slug| slug == "topics"));
        assert!(!page.has_more);
        assert!(page.next.is_none());
    }

    #[test]
    fn html_listing_keeps_path_hrefs_and_ignores_course_and_video() {
        let html = r#"
            <a href="https://www.linkedin.com/learning/practical-github-actions">Course</a>
            <a href="/learning/practical-github-actions/welcome">Video</a>
            <a href="https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate">Path</a>
            <a href="/learning/topics/professional-certificates">Topic</a>
        "#;
        let page = parse_topic_listing(html);
        assert_eq!(
            page.path_slugs,
            vec!["career-essentials-in-github-professional-certificate"]
        );
    }

    #[test]
    fn explicit_next_url_sets_has_more() {
        let body = r#"{
            "elements": [],
            "paging": {
                "next": "https://www.linkedin.com/learning-api/search?entityType=LEARNING_PATH&start=12"
            }
        }"#;
        let page = parse_topic_listing(body);
        assert!(page.has_more);
        assert_eq!(
            page.next.as_deref(),
            Some("https://www.linkedin.com/learning-api/search?entityType=LEARNING_PATH&start=12")
        );
    }
}
