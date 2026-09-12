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
        collect_listing_courses(html, &mut courses);
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
                let mut found = false;
                if let Some(item) = map.get("item") {
                    if let Some(course) = course_from_json_value(item) {
                        courses.push(course);
                        found = true;
                    } else {
                        let before = courses.len();
                        walk_json_ld(item, courses, title);
                        found = courses.len() > before;
                    }
                }
                if !found {
                    if let Some(course) = course_from_json_value(value) {
                        courses.push(course);
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
            for key in ["url", "canonicalUrl", "publicUrl", "trackingUrl"] {
                if let Some(url) = map.get(key).and_then(Value::as_str) {
                    if let Some(course) = course_from_href(url) {
                        return Some(course);
                    }
                }
            }
            None
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

fn collect_listing_courses(html: &str, courses: &mut Vec<CourseUrl>) {
    let page = super::topic::parse_topic_listing(html);
    courses.extend(page.courses);
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

    #[test]
    fn bpr_guid_encoded_courses_yield_path_members() {
        let html = r#"
            <html>
              <code id="bpr-guid-240">{&quot;included&quot;:[{&quot;entityType&quot;:&quot;COURSE&quot;,&quot;slug&quot;:&quot;practical-negotiation-techniques&quot;},{&quot;entityType&quot;:&quot;COURSE&quot;,&quot;slug&quot;:&quot;strategic-negotiation&quot;},{&quot;entityType&quot;:&quot;VIDEO&quot;,&quot;slug&quot;:&quot;welcome&quot;}]}</code>
            </html>
        "#;
        let parsed = parse_path_document(html).unwrap();
        let slugs: Vec<&str> = parsed
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(
            slugs,
            vec!["practical-negotiation-techniques", "strategic-negotiation"]
        );
    }

    #[test]
    fn json_ld_list_item_without_course_type_still_yields_course_urls() {
        let html = r#"
            <script type="application/ld+json">
            {"@type":"ItemList","name":"Negotiation Professional Certificate","itemListElement":[
              {"@type":"ListItem","item":{"url":"https://www.linkedin.com/learning/practical-negotiation-techniques"}},
              {"@type":"ListItem","url":"https://www.linkedin.com/learning/strategic-negotiation"},
              {"@type":"ListItem","item":{"url":"https://www.linkedin.com/learning/paths/negotiation-professional-certificate-by-american-negotiation-institute"}},
              {"@type":"ListItem","item":{"url":"https://www.linkedin.com/learning/topics/professional-certificates"}}
            ]}
            </script>
        "#;
        let parsed = parse_path_document(html).unwrap();
        let slugs: Vec<&str> = parsed
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(
            slugs,
            vec!["practical-negotiation-techniques", "strategic-negotiation"]
        );
        assert_eq!(
            parsed.title.as_deref(),
            Some("Negotiation Professional Certificate")
        );
    }

    #[test]
    fn json_script_course_entities_yield_courses_without_ld_json() {
        let html = r#"
            <html>
              <script type="application/json">
              {"included":[
                {"entityType":"COURSE","slug":"practical-negotiation-techniques","url":"https://www.linkedin.com/learning/practical-negotiation-techniques"},
                {"entityType":"COURSE","slug":"strategic-negotiation","canonicalUrl":"https://www.linkedin.com/learning/strategic-negotiation"},
                {"entityType":"LEARNING_PATH","slug":"negotiation-professional-certificate-by-american-negotiation-institute","url":"https://www.linkedin.com/learning/paths/negotiation-professional-certificate-by-american-negotiation-institute"},
                {"entityType":"VIDEO","slug":"welcome","url":"https://www.linkedin.com/learning/practical-negotiation-techniques/welcome"}
              ]}
              </script>
            </html>
        "#;
        let parsed = parse_path_document(html).unwrap();
        let slugs: Vec<&str> = parsed
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(
            slugs,
            vec!["practical-negotiation-techniques", "strategic-negotiation"]
        );
        assert!(!slugs.contains(&"welcome"));
        assert!(!slugs
            .contains(&"negotiation-professional-certificate-by-american-negotiation-institute"));
        assert!(!slugs.contains(&"paths"));
        assert!(!slugs.contains(&"topics"));
    }

    #[test]
    fn query_string_course_hrefs_are_harvested_without_json_ld() {
        let html = r#"
            <a href="https://www.linkedin.com/learning/practical-negotiation-techniques?u=123">Course</a>
            <a href="/learning/paths/negotiation-professional-certificate-by-american-negotiation-institute">Path</a>
            <a href="/learning/topics/professional-certificates">Topic</a>
        "#;
        let parsed = parse_path_document(html).unwrap();
        let slugs: Vec<&str> = parsed
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(slugs, vec!["practical-negotiation-techniques"]);
    }
}
