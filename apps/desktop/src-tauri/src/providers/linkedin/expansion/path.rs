use regex::Regex;
use serde_json::Value;
use thiserror::Error;

use crate::linkedin::{parse_course_url, CourseUrl};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathParseError {
    #[error("learning path HTML did not contain course URLs")]
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPath {
    pub title: Option<String>,
    pub courses: Vec<CourseUrl>,
}

pub fn parse_path_document(html: &str) -> Result<ParsedPath, PathParseError> {
    let mut courses = Vec::new();
    let title = collect_json_ld_courses(html, &mut courses);
    if courses.is_empty() {
        collect_anchor_courses(html, &mut courses);
    }
    if courses.is_empty() {
        return Err(PathParseError::Empty);
    }
    Ok(ParsedPath {
        title,
        courses: dedupe_courses(courses),
    })
}

fn collect_json_ld_courses(html: &str, courses: &mut Vec<CourseUrl>) -> Option<String> {
    let Some(script_re) = json_ld_script_regex() else {
        return None;
    };
    let mut title = None;
    for capture in script_re.captures_iter(html) {
        let Some(body) = capture.get(1).map(|matched| matched.as_str().trim()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(body) else {
            continue;
        };
        walk_json_ld(&value, courses, &mut title);
    }
    title
}

fn json_ld_script_regex() -> Option<Regex> {
    Regex::new(r#"(?is)<script[^>]*type=["']application/ld\+json["'][^>]*>(.*?)</script>"#).ok()
}

fn walk_json_ld(value: &Value, courses: &mut Vec<CourseUrl>, title: &mut Option<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                walk_json_ld(item, courses, title);
            }
        }
        Value::Object(map) => {
            if let Some(graph) = map.get("@graph") {
                walk_json_ld(graph, courses, title);
            }
            if type_contains(value, "itemlist") {
                if title.is_none() {
                    if let Some(name) = map.get("name").and_then(Value::as_str) {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() {
                            *title = Some(trimmed.to_string());
                        }
                    }
                }
                if let Some(elements) = map.get("itemListElement") {
                    walk_json_ld(elements, courses, title);
                }
            }
            if type_contains(value, "listitem") {
                if let Some(item) = map.get("item") {
                    if let Some(course) = course_from_json_value(item) {
                        courses.push(course);
                    } else {
                        walk_json_ld(item, courses, title);
                    }
                }
            } else if let Some(course) = course_from_json_value(value) {
                courses.push(course);
            }
        }
        _ => {}
    }
}

fn course_from_json_value(value: &Value) -> Option<CourseUrl> {
    match value {
        Value::String(url) => course_from_href(url),
        Value::Object(map) => {
            if !type_contains(value, "course") {
                return None;
            }
            let url = map.get("url").and_then(Value::as_str)?;
            course_from_href(url)
        }
        _ => None,
    }
}

fn type_contains(value: &Value, needle: &str) -> bool {
    match value.get("@type") {
        Some(Value::String(type_name)) => type_name.to_ascii_lowercase().contains(needle),
        Some(Value::Array(types)) => types.iter().any(|entry| {
            entry
                .as_str()
                .map(|type_name| type_name.to_ascii_lowercase().contains(needle))
                .unwrap_or(false)
        }),
        _ => false,
    }
}

fn collect_anchor_courses(html: &str, courses: &mut Vec<CourseUrl>) {
    let Some(anchor_re) = learning_anchor_regex() else {
        return;
    };
    for capture in anchor_re.captures_iter(html) {
        let Some(href) = capture.get(1).map(|matched| matched.as_str()) else {
            continue;
        };
        if let Some(course) = course_from_href(href) {
            courses.push(course);
        }
    }
}

fn learning_anchor_regex() -> Option<Regex> {
    Regex::new(
        r#"(?i)href=["']((?:https?://(?:www\.)?linkedin\.com)?/learning/[A-Za-z0-9_-]+)["']"#,
    )
    .ok()
}

fn course_from_href(href: &str) -> Option<CourseUrl> {
    let with_host = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else {
        format!("https://www.linkedin.com{href}")
    };
    parse_course_url(&with_host, 1).ok()
}

fn dedupe_courses(courses: Vec<CourseUrl>) -> Vec<CourseUrl> {
    let mut seen = std::collections::HashSet::new();
    let mut unique = Vec::new();
    for course in courses {
        if seen.insert(course.slug.clone()) {
            unique.push(course);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::{parse_path_document, PathParseError};

    const GITHUB_CERT_HTML: &str =
        include_str!("fixtures/career-essentials-in-github-professional-certificate.html");

    #[test]
    fn github_cert_json_ld_yields_literal_course_slugs() {
        let courses = parse_path_document(GITHUB_CERT_HTML).unwrap().courses;
        let slugs: Vec<&str> = courses.iter().map(|course| course.slug.as_str()).collect();
        assert_eq!(
            slugs,
            vec![
                "practical-github-actions",
                "practical-github-project-management-and-collaboration",
                "practical-github-copilot",
                "practical-github-code-search",
            ]
        );
    }

    #[test]
    fn github_cert_json_ld_name_is_path_title() {
        let parsed = parse_path_document(GITHUB_CERT_HTML).unwrap();
        assert_eq!(
            parsed.title.as_deref(),
            Some("Career Essentials in GitHub Professional Certificate")
        );
    }

    #[test]
    fn anchor_fallback_skips_reserved_prefixes() {
        let html = r#"
            <a href="https://www.linkedin.com/learning/practical-github-actions">Actions</a>
            <a href="/learning/topics/professional-certificates">Topic</a>
            <a href="/learning/paths/other-path">Path</a>
            <a href="/learning/search/foo">Search</a>
            <a href="https://www.linkedin.com/learning/practical-github-copilot">Copilot</a>
        "#;
        let courses = parse_path_document(html).unwrap().courses;
        let slugs: Vec<&str> = courses.iter().map(|course| course.slug.as_str()).collect();
        assert_eq!(
            slugs,
            vec!["practical-github-actions", "practical-github-copilot"]
        );
    }

    #[test]
    fn empty_html_is_a_path_parse_failure() {
        assert_eq!(
            parse_path_document("<html></html>").unwrap_err(),
            PathParseError::Empty
        );
    }
}
