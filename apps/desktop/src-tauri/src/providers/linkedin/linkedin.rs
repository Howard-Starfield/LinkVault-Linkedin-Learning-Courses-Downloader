use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub(crate) const RESERVED_LEARNING_PREFIXES: &[&str] = &["search", "me", "login", "browse", "in"];
pub(crate) const HUB_LEARNING_PREFIXES: &[&str] = &["paths", "topics"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CourseUrl {
    pub original: String,
    pub normalized_url: String,
    pub slug: String,
    pub quiz_urls: Vec<String>,
    pub assessment_urns: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CourseUrlError {
    #[error("no LinkedIn Learning course URLs were provided")]
    Empty,
    #[error("line {line}: expected a linkedin.com/learning course URL")]
    NotLinkedInLearning { line: usize },
    #[error("line {line}: missing course slug")]
    MissingSlug { line: usize },
    #[error("line {line}: could not parse URL")]
    InvalidUrl { line: usize },
    #[error(
        "line {line}: '{segment}' is a LinkedIn Learning listing or account page, not a course"
    )]
    ReservedSegment { line: usize, segment: String },
}

#[cfg(test)]
pub fn parse_course_urls(input: &str) -> Result<Vec<CourseUrl>, CourseUrlError> {
    let mut courses = Vec::new();

    for (index, raw_line) in input.lines().enumerate() {
        let line = index + 1;
        let candidates = course_url_candidates(raw_line);
        if candidates.is_empty() {
            if raw_line.trim().is_empty() {
                continue;
            }
            return Err(CourseUrlError::NotLinkedInLearning { line });
        }

        for candidate in candidates {
            courses.push(parse_course_url(&candidate, line)?);
        }
    }

    if courses.is_empty() {
        return Err(CourseUrlError::Empty);
    }

    Ok(courses)
}

pub(crate) fn course_url_candidates(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let parts = trimmed.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 1 {
        return vec![trim_course_url_token(parts[0]).to_string()];
    }

    parts
        .into_iter()
        .map(trim_course_url_token)
        .filter(|part| part.to_ascii_lowercase().contains("linkedin.com/learning/"))
        .map(ToString::to_string)
        .collect()
}

fn trim_course_url_token(token: &str) -> &str {
    token.trim_matches(|character: char| {
        character.is_ascii_whitespace()
            || matches!(
                character,
                '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ','
            )
    })
}

pub(crate) fn parse_course_url(value: &str, line: usize) -> Result<CourseUrl, CourseUrlError> {
    let parsed = parse_learning_url(value, line)?;
    if is_reserved_or_hub_prefix(&parsed.first_segment) {
        return Err(CourseUrlError::ReservedSegment {
            line,
            segment: parsed.first_segment,
        });
    }

    let remaining_segments = parsed.remaining_segments;
    let quiz_urls = extract_quiz_urls(&parsed.url, &parsed.first_segment, &remaining_segments);
    let assessment_urns = extract_assessment_urns(&remaining_segments);

    Ok(CourseUrl {
        original: value.to_string(),
        normalized_url: format!("https://www.linkedin.com/learning/{}", parsed.first_segment),
        slug: parsed.first_segment,
        quiz_urls,
        assessment_urns,
    })
}

pub(crate) struct ParsedLearningUrl {
    pub url: Url,
    pub first_segment: String,
    pub remaining_segments: Vec<String>,
}

pub(crate) fn parse_learning_url(
    value: &str,
    line: usize,
) -> Result<ParsedLearningUrl, CourseUrlError> {
    let with_protocol = if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };

    let url = Url::parse(&with_protocol).map_err(|_| CourseUrlError::InvalidUrl { line })?;
    let host = url
        .host_str()
        .ok_or(CourseUrlError::NotLinkedInLearning { line })?;
    let is_linkedin = host == "linkedin.com" || host.ends_with(".linkedin.com");
    if !is_linkedin {
        return Err(CourseUrlError::NotLinkedInLearning { line });
    }

    let mut segments = url
        .path_segments()
        .ok_or(CourseUrlError::MissingSlug { line })?;
    let learning = segments.next();
    if learning != Some("learning") {
        return Err(CourseUrlError::NotLinkedInLearning { line });
    }

    let first_segment = segments
        .next()
        .filter(|segment| !segment.trim().is_empty())
        .ok_or(CourseUrlError::MissingSlug { line })?
        .to_string();

    let remaining_segments = segments
        .filter(|segment| !segment.trim().is_empty())
        .map(ToString::to_string)
        .collect();

    Ok(ParsedLearningUrl {
        url,
        first_segment,
        remaining_segments,
    })
}

pub(crate) fn is_reserved_learning_prefix(segment: &str) -> bool {
    matches_prefix_list(segment, RESERVED_LEARNING_PREFIXES)
}

pub(crate) fn is_reserved_or_hub_prefix(segment: &str) -> bool {
    is_reserved_learning_prefix(segment) || matches_prefix_list(segment, HUB_LEARNING_PREFIXES)
}

fn matches_prefix_list(segment: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|prefix| segment.eq_ignore_ascii_case(prefix))
}

fn extract_quiz_urls(url: &Url, slug: &str, remaining_segments: &[String]) -> Vec<String> {
    let Some(quiz_index) = remaining_segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("quiz"))
    else {
        return Vec::new();
    };

    let Some(assessment_segment) = remaining_segments.get(quiz_index + 1) else {
        return Vec::new();
    };

    let mut quiz_url =
        format!("https://www.linkedin.com/learning/{slug}/quiz/{assessment_segment}");
    if let Some(query) = normalized_quiz_query(url) {
        quiz_url.push('?');
        quiz_url.push_str(&query);
    }
    vec![quiz_url]
}

fn extract_assessment_urns(remaining_segments: &[String]) -> Vec<String> {
    remaining_segments
        .windows(2)
        .filter(|segments| segments[0].eq_ignore_ascii_case("quiz"))
        .filter_map(|segments| {
            let urn = segments[1].trim();
            if urn.starts_with("urn:li:learningApiAssessment:")
                || urn.starts_with("urn%3Ali%3AlearningApiAssessment%3A")
            {
                Some(urn.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn normalized_quiz_query(url: &Url) -> Option<String> {
    let allowed = ["resume", "u"];
    let query = url
        .query_pairs()
        .filter(|(key, value)| {
            allowed.contains(&key.as_ref()) && !key.trim().is_empty() && !value.trim().is_empty()
        })
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        None
    } else {
        Some(query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_missing_protocol_and_extracts_slug() {
        let parsed =
            parse_course_urls("www.linkedin.com/learning/service-desk-fundamentals").unwrap();

        assert_eq!(parsed[0].slug, "service-desk-fundamentals");
        assert_eq!(
            parsed[0].normalized_url,
            "https://www.linkedin.com/learning/service-desk-fundamentals"
        );
    }

    #[test]
    fn extracts_same_slug_with_trailing_slash_query_and_hash() {
        let parsed = parse_course_urls(
            "https://www.linkedin.com/learning/service-desk-fundamentals/?trk=share#lesson",
        )
        .unwrap();

        assert_eq!(parsed[0].slug, "service-desk-fundamentals");
        assert_eq!(
            parsed[0].normalized_url,
            "https://www.linkedin.com/learning/service-desk-fundamentals"
        );
        assert!(parsed[0].quiz_urls.is_empty());
    }

    #[test]
    fn preserves_direct_quiz_hint_while_normalizing_course_url() {
        let parsed = parse_course_urls(
            "https://www.linkedin.com/learning/time-management-for-customer-service-professionals/quiz/urn:li:learningApiAssessment:69813586?resume=false&u=52983649&trk=ignored",
        )
        .unwrap();

        assert_eq!(
            parsed[0].normalized_url,
            "https://www.linkedin.com/learning/time-management-for-customer-service-professionals"
        );
        assert_eq!(
            parsed[0].quiz_urls,
            vec![
                "https://www.linkedin.com/learning/time-management-for-customer-service-professionals/quiz/urn:li:learningApiAssessment:69813586?resume=false&u=52983649"
                    .to_string()
            ]
        );
        assert_eq!(
            parsed[0].assessment_urns,
            vec!["urn:li:learningApiAssessment:69813586".to_string()]
        );
    }

    #[test]
    fn rejects_embedded_or_non_learning_urls() {
        let error = parse_course_urls(
            "https://example.com/?next=https://www.linkedin.com/learning/service-desk-fundamentals",
        )
        .unwrap_err();

        assert_eq!(error, CourseUrlError::NotLinkedInLearning { line: 1 });
    }

    #[test]
    fn ignores_blank_lines_and_preserves_order() {
        let parsed = parse_course_urls(
            "\nhttps://www.linkedin.com/learning/first-course\n\nwww.linkedin.com/learning/second-course\n",
        )
        .unwrap();

        assert_eq!(
            parsed
                .iter()
                .map(|course| course.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["first-course", "second-course"]
        );
    }

    #[test]
    fn extracts_many_course_urls_from_space_separated_paste() {
        let input = (0..105)
            .map(|index| format!("https://www.linkedin.com/learning/course-{index:03}"))
            .collect::<Vec<_>>()
            .join(" ");

        let parsed = parse_course_urls(&input).unwrap();

        assert_eq!(parsed.len(), 105);
        assert_eq!(parsed[0].slug, "course-000");
        assert_eq!(parsed[104].slug, "course-104");
    }

    #[test]
    fn rejects_topic_and_path_prefixes_as_course_slugs() {
        let topic =
            parse_course_urls("https://www.linkedin.com/learning/topics/professional-certificates")
                .unwrap_err();
        assert_eq!(
            topic,
            CourseUrlError::ReservedSegment {
                line: 1,
                segment: "topics".to_string(),
            }
        );

        let path = parse_course_urls(
            "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate",
        )
        .unwrap_err();
        assert_eq!(
            path,
            CourseUrlError::ReservedSegment {
                line: 1,
                segment: "paths".to_string(),
            }
        );
    }

    #[test]
    fn rejects_reserved_account_and_search_prefixes() {
        for prefix in ["search", "me", "login", "browse", "in"] {
            let error = parse_course_urls(&format!(
                "https://www.linkedin.com/learning/{prefix}/something"
            ))
            .unwrap_err();
            assert_eq!(
                error,
                CourseUrlError::ReservedSegment {
                    line: 1,
                    segment: prefix.to_string(),
                }
            );
        }
    }
}
