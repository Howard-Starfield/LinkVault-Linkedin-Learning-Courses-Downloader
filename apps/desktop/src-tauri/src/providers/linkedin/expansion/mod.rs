use serde::{Deserialize, Serialize};

use super::linkedin::CourseUrl;

mod classify;
mod path;

pub use classify::classify_learning_urls;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum LearningUrlRef {
    #[serde(rename = "course")]
    Course(CourseUrl),
    #[serde(rename = "path")]
    Path {
        original: String,
        normalized_url: String,
        path_slug: String,
    },
    #[serde(rename = "topic")]
    Topic {
        original: String,
        normalized_url: String,
        topic_slug: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SchedulePolicy {
    KnownCount,
    Discovering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedPaste {
    pub refs: Vec<LearningUrlRef>,
    pub course_count: u32,
    pub path_count: u32,
    pub topic_count: u32,
    pub schedule_policy: SchedulePolicy,
}

impl ClassifiedPaste {
    pub fn from_refs(refs: Vec<LearningUrlRef>) -> Self {
        let mut course_count = 0_u32;
        let mut path_count = 0_u32;
        let mut topic_count = 0_u32;
        for learning_ref in &refs {
            match learning_ref {
                LearningUrlRef::Course(_) => course_count += 1,
                LearningUrlRef::Path { .. } => path_count += 1,
                LearningUrlRef::Topic { .. } => topic_count += 1,
            }
        }
        let schedule_policy = if path_count > 0 || topic_count > 0 {
            SchedulePolicy::Discovering
        } else {
            SchedulePolicy::KnownCount
        };
        Self {
            refs,
            course_count,
            path_count,
            topic_count,
            schedule_policy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_learning_urls, LearningUrlRef, SchedulePolicy};
    use crate::linkedin::CourseUrlError;

    #[test]
    fn topic_url_classifies_as_topic_not_slug_topics() {
        let classified = classify_learning_urls(
            "https://www.linkedin.com/learning/topics/professional-certificates",
        )
        .unwrap();

        assert_eq!(classified.topic_count, 1);
        assert_eq!(classified.path_count, 0);
        assert_eq!(classified.course_count, 0);
        assert_eq!(classified.schedule_policy, SchedulePolicy::Discovering);
        match &classified.refs[0] {
            LearningUrlRef::Topic {
                topic_slug,
                normalized_url,
                ..
            } => {
                assert_eq!(topic_slug, "professional-certificates");
                assert_eq!(
                    normalized_url,
                    "https://www.linkedin.com/learning/topics/professional-certificates"
                );
            }
            other => panic!("expected Topic, got {other:?}"),
        }
    }

    #[test]
    fn two_path_urls_keep_distinct_path_slugs() {
        let classified = classify_learning_urls(
            "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate\nhttps://www.linkedin.com/learning/paths/career-essentials-in-software-development-by-microsoft-and-linkedin",
        )
        .unwrap();

        let slugs: Vec<&str> = classified
            .refs
            .iter()
            .map(|learning_ref| match learning_ref {
                LearningUrlRef::Path { path_slug, .. } => path_slug.as_str(),
                other => panic!("expected Path, got {other:?}"),
            })
            .collect();

        assert_eq!(
            slugs,
            vec![
                "career-essentials-in-github-professional-certificate",
                "career-essentials-in-software-development-by-microsoft-and-linkedin",
            ]
        );
        assert_eq!(classified.path_count, 2);
        assert_eq!(classified.schedule_policy, SchedulePolicy::Discovering);
        assert!(!slugs.contains(&"paths"));
    }

    #[test]
    fn reserved_prefixes_error_instead_of_fake_course_slugs() {
        for prefix in ["search", "me", "login", "browse", "in"] {
            let error = classify_learning_urls(&format!(
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

    #[test]
    fn course_urls_keep_known_count_policy_and_quiz_hints() {
        let classified = classify_learning_urls(
            "https://www.linkedin.com/learning/time-management-for-customer-service-professionals/quiz/urn:li:learningApiAssessment:69813586?resume=false&u=52983649",
        )
        .unwrap();

        assert_eq!(classified.schedule_policy, SchedulePolicy::KnownCount);
        assert_eq!(classified.course_count, 1);
        match &classified.refs[0] {
            LearningUrlRef::Course(course) => {
                assert_eq!(
                    course.slug,
                    "time-management-for-customer-service-professionals"
                );
                assert_eq!(
                    course.assessment_urns,
                    vec!["urn:li:learningApiAssessment:69813586".to_string()]
                );
            }
            other => panic!("expected Course, got {other:?}"),
        }
    }

    #[test]
    fn mixed_paste_counts_each_kind() {
        let classified = classify_learning_urls(
            "https://www.linkedin.com/learning/practical-github-actions\nhttps://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate\nhttps://www.linkedin.com/learning/topics/professional-certificates",
        )
        .unwrap();

        assert_eq!(classified.course_count, 1);
        assert_eq!(classified.path_count, 1);
        assert_eq!(classified.topic_count, 1);
        assert_eq!(classified.schedule_policy, SchedulePolicy::Discovering);
    }
}
