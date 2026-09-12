use regex::Regex;
use serde_json::Value;
use url::Url;

use crate::linkedin::{is_reserved_or_hub_prefix, parse_course_url, parse_learning_url, CourseUrl};

// Topic listings mix Course, Video, and LEARNING_PATH cards. Keep path and
// standalone course result URLs. Drop videos, topics, and reserved prefixes.
// Prefer JSON entities typed LEARNING_PATH or COURSE, then matching hrefs.
// Follow an explicit next URL when the body includes one. Do not synthesize
// guest ?start= paging or GraphQL queryIds.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicPage {
    pub path_slugs: Vec<String>,
    pub courses: Vec<CourseUrl>,
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
    let mut courses = harvest_course_urls(body);
    for script in json_embedded_bodies(body) {
        if let Ok(value) = serde_json::from_str::<Value>(&script) {
            let page = topic_page_from_json(&value, &script);
            merge_slugs(&mut path_slugs, page.path_slugs);
            merge_courses(&mut courses, page.courses);
            if page.next.is_some() || page.has_more {
                return TopicPage {
                    path_slugs,
                    courses,
                    has_more: page.has_more,
                    next: page.next,
                };
            }
        } else {
            merge_courses(&mut courses, harvest_course_slugs_from_entities(&script));
        }
    }

    TopicPage {
        path_slugs,
        courses,
        has_more: false,
        next: None,
    }
}

fn topic_page_from_json(value: &Value, raw: &str) -> TopicPage {
    let mut path_slugs = Vec::new();
    let mut courses = Vec::new();
    collect_listing_entities(value, &mut path_slugs, &mut courses);
    merge_slugs(&mut path_slugs, harvest_path_slugs(raw));
    merge_courses(&mut courses, harvest_course_urls(raw));
    let next = explicit_next_url(value);
    TopicPage {
        has_more: next.is_some(),
        next,
        path_slugs,
        courses,
    }
}

fn collect_listing_entities(
    value: &Value,
    path_slugs: &mut Vec<String>,
    courses: &mut Vec<CourseUrl>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_listing_entities(item, path_slugs, courses);
            }
        }
        Value::Object(map) => {
            if is_learning_path_entity(value) {
                if let Some(slug) = path_slug_from_object(map) {
                    push_slug(path_slugs, slug);
                }
            } else if is_course_entity(value) {
                if let Some(course) = course_from_object(map) {
                    push_course(courses, course);
                }
            }
            for nested in map.values() {
                collect_listing_entities(nested, path_slugs, courses);
            }
        }
        _ => {}
    }
}

fn is_learning_path_entity(value: &Value) -> bool {
    entity_type_flags(value).contains("LEARNING_PATH")
}

fn is_course_entity(value: &Value) -> bool {
    let flags = entity_type_flags(value);
    flags.contains("COURSE") && !flags.contains("LEARNING_PATH") && !flags.contains("VIDEO")
}

fn entity_type_flags(value: &Value) -> String {
    ["entityType", "type", "$type", "contentType"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_ascii_uppercase)
        .collect::<Vec<_>>()
        .join(" ")
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

fn harvest_course_urls(body: &str) -> Vec<CourseUrl> {
    let Some(regex) = course_href_regex() else {
        return Vec::new();
    };
    let mut courses = Vec::new();
    for matched in regex.find_iter(body) {
        if !listing_href_is_course_card(body, matched.end()) {
            continue;
        }
        if let Some(course) = listing_course_from_href(matched.as_str()) {
            push_course(&mut courses, course);
        }
    }
    courses
}

fn course_href_regex() -> Option<Regex> {
    Regex::new(r"(?i)(?:https?://(?:www\.)?linkedin\.com)?/learning/[A-Za-z0-9_-]+").ok()
}

fn listing_href_is_course_card(body: &str, match_end: usize) -> bool {
    let rest = body.get(match_end..).unwrap_or("").trim_start_matches('/');
    match rest.chars().next() {
        None => true,
        Some(character) => matches!(
            character,
            '"' | '\'' | '?' | '#' | '<' | '>' | '&' | ' ' | '\n' | '\r' | '\t'
        ),
    }
}

fn course_from_object(map: &serde_json::Map<String, Value>) -> Option<CourseUrl> {
    for key in ["url", "canonicalUrl", "publicUrl", "trackingUrl"] {
        if let Some(url) = map.get(key).and_then(Value::as_str) {
            if let Some(course) = listing_course_from_href(url) {
                return Some(course);
            }
        }
    }
    let slug = map.get("slug").and_then(Value::as_str).map(str::trim)?;
    if slug.is_empty() || is_reserved_or_hub_prefix(slug) {
        return None;
    }
    listing_course_from_href(&format!("https://www.linkedin.com/learning/{slug}"))
}

fn listing_course_from_href(href: &str) -> Option<CourseUrl> {
    let normalized = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with('/') {
        format!("https://www.linkedin.com{href}")
    } else {
        return None;
    };
    let parsed = parse_learning_url(&normalized, 1).ok()?;
    if is_reserved_or_hub_prefix(&parsed.first_segment) {
        return None;
    }
    if !parsed.remaining_segments.is_empty() {
        return None;
    }
    parse_course_url(&normalized, 1).ok()
}

fn json_embedded_bodies(html: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    if let Some(regex) = json_script_regex() {
        for capture in regex.captures_iter(html) {
            if let Some(body) = capture.get(1).map(|matched| matched.as_str().trim()) {
                if !body.is_empty() {
                    bodies.push(body.to_string());
                }
            }
        }
    }
    if let Some(regex) = bpr_code_regex() {
        for capture in regex.captures_iter(html) {
            let Some(raw) = capture.get(1).map(|matched| matched.as_str().trim()) else {
                continue;
            };
            let decoded = decode_html_entities(raw);
            if !decoded.is_empty() {
                bodies.push(decoded);
            }
        }
    }
    bodies
}

fn json_script_regex() -> Option<Regex> {
    Regex::new(r#"(?is)<script[^>]*type=["']application/(?:ld\+json|json)["'][^>]*>(.*?)</script>"#)
        .ok()
}

fn bpr_code_regex() -> Option<Regex> {
    Regex::new(r#"(?is)<code[^>]*id=["']bpr-guid-[^"']+["'][^>]*>(.*?)</code>"#).ok()
}

fn decode_html_entities(body: &str) -> String {
    body.replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x22;", "\"")
        .replace("&amp;", "&")
}

fn harvest_course_slugs_from_entities(body: &str) -> Vec<CourseUrl> {
    let mut courses = Vec::new();
    let patterns = [
        r#""entityType"\s*:\s*"COURSE"[\s\S]{0,1200}"slug"\s*:\s*"([^"]+)""#,
        r#""slug"\s*:\s*"([^"]+)"[\s\S]{0,1200}"entityType"\s*:\s*"COURSE""#,
    ];
    for pattern in patterns {
        let Some(regex) = Regex::new(pattern).ok() else {
            continue;
        };
        for capture in regex.captures_iter(body) {
            let Some(slug) = capture.get(1).map(|matched| matched.as_str().trim()) else {
                continue;
            };
            if let Some(course) =
                listing_course_from_href(&format!("https://www.linkedin.com/learning/{slug}"))
            {
                push_course(&mut courses, course);
            }
        }
    }
    courses
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

fn push_course(courses: &mut Vec<CourseUrl>, course: CourseUrl) {
    if !courses.iter().any(|existing| existing.slug == course.slug) {
        courses.push(course);
    }
}

fn merge_courses(target: &mut Vec<CourseUrl>, incoming: Vec<CourseUrl>) {
    for course in incoming {
        push_course(target, course);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_topic_listing;

    const MIXED_TOPIC_JSON: &str =
        include_str!("fixtures/topic-professional-certificates-mixed.json");

    #[test]
    fn mixed_listing_keeps_paths_and_standalone_courses_and_drops_videos() {
        let page = parse_topic_listing(MIXED_TOPIC_JSON);
        assert_eq!(
            page.path_slugs,
            vec![
                "career-essentials-in-github-professional-certificate",
                "career-essentials-in-software-development-by-microsoft-and-linkedin",
            ]
        );
        let course_slugs: Vec<&str> = page
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(
            course_slugs,
            vec![
                "career-essentials-in-system-administration-by-microsoft-and-linkedin",
                "practical-github-actions",
            ]
        );
        assert!(!page.path_slugs.iter().any(|slug| slug == "topics"));
        assert!(!course_slugs.contains(&"welcome"));
        assert!(!page.has_more);
        assert!(page.next.is_none());
    }

    #[test]
    fn html_listing_keeps_path_and_course_hrefs_and_ignores_video() {
        let html = r#"
            <a href="https://www.linkedin.com/learning/career-essentials-in-system-administration-by-microsoft-and-linkedin">Course</a>
            <a href="/learning/practical-github-actions/welcome">Video</a>
            <a href="https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate">Path</a>
            <a href="/learning/topics/professional-certificates">Topic</a>
        "#;
        let page = parse_topic_listing(html);
        assert_eq!(
            page.path_slugs,
            vec!["career-essentials-in-github-professional-certificate"]
        );
        assert_eq!(
            page.courses
                .iter()
                .map(|course| course.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["career-essentials-in-system-administration-by-microsoft-and-linkedin"]
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

    #[test]
    fn bpr_guid_html_entities_yield_course_slugs() {
        let html = r#"
            <code id="bpr-guid-240">{&quot;included&quot;:[{&quot;entityType&quot;:&quot;COURSE&quot;,&quot;slug&quot;:&quot;practical-negotiation-techniques&quot;},{&quot;entityType&quot;:&quot;COURSE&quot;,&quot;slug&quot;:&quot;strategic-negotiation&quot;},{&quot;entityType&quot;:&quot;VIDEO&quot;,&quot;slug&quot;:&quot;welcome&quot;}]}</code>
        "#;
        let page = parse_topic_listing(html);
        let course_slugs: Vec<&str> = page
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(
            course_slugs,
            vec!["practical-negotiation-techniques", "strategic-negotiation"]
        );
        assert!(!course_slugs.contains(&"welcome"));
    }

    #[test]
    fn invalid_json_bpr_still_yields_course_slugs_next_to_entity_type() {
        let html = r#"
            <code id="bpr-guid-242">{&quot;data&quot;:{&quot;broken&quot; &quot;entityType&quot;:&quot;COURSE&quot;,&quot;slug&quot;:&quot;practical-negotiation-techniques&quot;,&quot;entityType&quot;:&quot;VIDEO&quot;,&quot;slug&quot;:&quot;welcome&quot;,&quot;slug&quot;:&quot;strategic-negotiation&quot;,&quot;entityType&quot;:&quot;COURSE&quot;}</code>
        "#;
        let page = parse_topic_listing(html);
        let course_slugs: Vec<&str> = page
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert!(course_slugs.contains(&"practical-negotiation-techniques"));
        assert!(course_slugs.contains(&"strategic-negotiation"));
        assert_eq!(course_slugs.len(), 2);
        assert!(!course_slugs.contains(&"welcome"));
    }
}
