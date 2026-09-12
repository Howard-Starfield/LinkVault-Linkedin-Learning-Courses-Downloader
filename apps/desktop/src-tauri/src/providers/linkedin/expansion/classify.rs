use super::{ClassifiedPaste, LearningUrlRef};
use crate::linkedin::{
    course_url_candidates, is_reserved_learning_prefix, parse_course_url, parse_learning_url,
    CourseUrlError,
};

pub fn classify_learning_urls(input: &str) -> Result<ClassifiedPaste, CourseUrlError> {
    let mut refs = Vec::new();

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
            refs.push(classify_learning_url(&candidate, line)?);
        }
    }

    if refs.is_empty() {
        return Err(CourseUrlError::Empty);
    }

    Ok(ClassifiedPaste::from_refs(refs))
}

fn classify_learning_url(value: &str, line: usize) -> Result<LearningUrlRef, CourseUrlError> {
    let parsed = parse_learning_url(value, line)?;
    if parsed.first_segment.eq_ignore_ascii_case("paths") {
        let path_slug = parsed
            .remaining_segments
            .first()
            .filter(|segment| !segment.trim().is_empty())
            .cloned()
            .ok_or(CourseUrlError::MissingSlug { line })?;
        return Ok(LearningUrlRef::Path {
            original: value.to_string(),
            normalized_url: format!("https://www.linkedin.com/learning/paths/{path_slug}"),
            path_slug,
        });
    }
    if parsed.first_segment.eq_ignore_ascii_case("topics") {
        let topic_slug = parsed
            .remaining_segments
            .first()
            .filter(|segment| !segment.trim().is_empty())
            .cloned()
            .ok_or(CourseUrlError::MissingSlug { line })?;
        return Ok(LearningUrlRef::Topic {
            original: value.to_string(),
            normalized_url: format!("https://www.linkedin.com/learning/topics/{topic_slug}"),
            topic_slug,
        });
    }
    if is_reserved_learning_prefix(&parsed.first_segment) {
        return Err(CourseUrlError::ReservedSegment {
            line,
            segment: parsed.first_segment,
        });
    }
    Ok(LearningUrlRef::Course(parse_course_url(value, line)?))
}
