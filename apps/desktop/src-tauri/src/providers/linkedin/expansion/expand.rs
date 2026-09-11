use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::course::CourseApiClient;
use crate::linkedin::CourseUrl;

use super::path::parse_path_html;
use super::topic::parse_topic_listing;
use super::{ExpandedCourseCatalog, ExpansionError, ExpansionSummary, LearningUrlRef};

pub fn expand_learning_urls(
    client: &mut (impl CourseApiClient + ?Sized),
    refs: &[LearningUrlRef],
) -> Result<ExpandedCourseCatalog, ExpansionError> {
    let mut courses = BTreeMap::new();
    let mut failed_paths = Vec::new();
    let mut attempted_paths = BTreeSet::new();

    for learning_ref in refs {
        match learning_ref {
            LearningUrlRef::Course(course) => {
                courses.insert(course.slug.clone(), course.clone());
            }
            LearningUrlRef::Path { path_slug, .. } => {
                expand_path(
                    client,
                    path_slug,
                    &mut courses,
                    &mut failed_paths,
                    &mut attempted_paths,
                );
            }
            LearningUrlRef::Topic { topic_slug, .. } => {
                expand_topic(
                    client,
                    topic_slug,
                    &mut courses,
                    &mut failed_paths,
                    &mut attempted_paths,
                )?;
            }
        }
    }

    if courses.is_empty() {
        return Err(ExpansionError::EmptyCatalog);
    }

    let unique_course_count = courses.len();
    Ok(ExpandedCourseCatalog {
        courses: courses.into_values().collect(),
        summary: ExpansionSummary {
            paste_ref_count: refs.len(),
            path_count: attempted_paths.len(),
            unique_course_count,
            failed_paths,
        },
    })
}

fn expand_path(
    client: &mut (impl CourseApiClient + ?Sized),
    path_slug: &str,
    courses: &mut BTreeMap<String, CourseUrl>,
    failed_paths: &mut Vec<String>,
    attempted_paths: &mut BTreeSet<String>,
) {
    if !attempted_paths.insert(path_slug.to_string()) {
        return;
    }
    let url = format!("https://www.linkedin.com/learning/paths/{path_slug}");
    match fetch_path_courses(client, &url) {
        Ok(path_courses) => {
            for course in path_courses {
                courses.entry(course.slug.clone()).or_insert(course);
            }
        }
        Err(_) => failed_paths.push(path_slug.to_string()),
    }
}

fn fetch_path_courses(
    client: &mut (impl CourseApiClient + ?Sized),
    url: &str,
) -> Result<Vec<CourseUrl>, String> {
    let html = client.get(url).map_err(|error| error.to_string())?;
    parse_path_html(&html).map_err(|error| error.to_string())
}

fn expand_topic(
    client: &mut (impl CourseApiClient + ?Sized),
    topic_slug: &str,
    courses: &mut BTreeMap<String, CourseUrl>,
    failed_paths: &mut Vec<String>,
    attempted_paths: &mut BTreeSet<String>,
) -> Result<(), ExpansionError> {
    let mut url = format!("https://www.linkedin.com/learning/topics/{topic_slug}");
    let mut seen_urls = HashSet::new();
    let mut path_slugs = Vec::new();
    let mut seen_path_slugs = HashSet::new();
    let mut harvested_any = false;

    loop {
        if !seen_urls.insert(url.clone()) {
            break;
        }
        let body = client
            .get(&url)
            .map_err(|error| ExpansionError::TopicExpandFailed {
                topic_slug: topic_slug.to_string(),
                detail: error.to_string(),
            })?;
        let page = parse_topic_listing(&body);
        if !page.path_slugs.is_empty() || !page.courses.is_empty() {
            harvested_any = true;
        }
        let mut added = 0_usize;
        for slug in page.path_slugs {
            if seen_path_slugs.insert(slug.clone()) {
                path_slugs.push(slug);
                added += 1;
            }
        }
        for course in page.courses {
            if courses.contains_key(&course.slug) {
                continue;
            }
            courses.insert(course.slug.clone(), course);
            added += 1;
        }
        match (page.has_more, page.next) {
            (true, Some(next)) if added > 0 || !harvested_any => {
                url = next;
            }
            _ => break,
        }
    }

    if !harvested_any {
        return Err(ExpansionError::TopicExpandFailed {
            topic_slug: topic_slug.to_string(),
            detail: "listing did not include learning paths or courses".to_string(),
        });
    }

    for path_slug in path_slugs {
        expand_path(client, &path_slug, courses, failed_paths, attempted_paths);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::{ExpansionError, LearningUrlRef};
    use super::expand_learning_urls;
    use crate::course::{CourseApiClient, CourseFetchError};

    const GITHUB_CERT_HTML: &str =
        include_str!("fixtures/career-essentials-in-github-professional-certificate.html");
    const MIXED_TOPIC_JSON: &str =
        include_str!("fixtures/topic-professional-certificates-mixed.json");

    struct ScriptedClient {
        pages: HashMap<String, Result<String, u16>>,
    }

    impl CourseApiClient for ScriptedClient {
        fn get(&mut self, url: &str) -> Result<String, CourseFetchError> {
            match self.pages.get(url) {
                Some(Ok(body)) => Ok(body.clone()),
                Some(Err(status)) => Err(CourseFetchError::Http { status: *status }),
                None => Err(CourseFetchError::Http { status: 404 }),
            }
        }
    }

    fn path_ref(path_slug: &str) -> LearningUrlRef {
        LearningUrlRef::Path {
            original: format!("https://www.linkedin.com/learning/paths/{path_slug}"),
            normalized_url: format!("https://www.linkedin.com/learning/paths/{path_slug}"),
            path_slug: path_slug.to_string(),
        }
    }

    fn topic_ref(topic_slug: &str) -> LearningUrlRef {
        LearningUrlRef::Topic {
            original: format!("https://www.linkedin.com/learning/topics/{topic_slug}"),
            normalized_url: format!("https://www.linkedin.com/learning/topics/{topic_slug}"),
            topic_slug: topic_slug.to_string(),
        }
    }

    #[test]
    fn one_path_html_failure_still_returns_courses_from_the_other_path() {
        let mut client = ScriptedClient {
            pages: HashMap::from([
                (
                    "https://www.linkedin.com/learning/paths/broken-path".to_string(),
                    Err(500),
                ),
                (
                    "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate".to_string(),
                    Ok(GITHUB_CERT_HTML.to_string()),
                ),
            ]),
        };

        let catalog = expand_learning_urls(
            &mut client,
            &[
                path_ref("broken-path"),
                path_ref("career-essentials-in-github-professional-certificate"),
            ],
        )
        .unwrap();

        let slugs: Vec<&str> = catalog
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(
            slugs,
            vec![
                "practical-github-actions",
                "practical-github-code-search",
                "practical-github-copilot",
                "practical-github-project-management-and-collaboration",
            ]
        );
        assert_eq!(
            catalog.summary.failed_paths,
            vec!["broken-path".to_string()]
        );
        assert_eq!(catalog.summary.unique_course_count, 4);
        assert_eq!(catalog.summary.path_count, 2);
        assert_eq!(catalog.summary.paste_ref_count, 2);
    }

    #[test]
    fn overlapping_paths_dedupe_course_slugs() {
        let overlapping_html = r#"
            <script type="application/ld+json">
            {"@type":"ItemList","itemListElement":[
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/practical-github-actions"}},
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/practical-github-copilot"}}
            ]}
            </script>
        "#;
        let mut client = ScriptedClient {
            pages: HashMap::from([
                (
                    "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate".to_string(),
                    Ok(GITHUB_CERT_HTML.to_string()),
                ),
                (
                    "https://www.linkedin.com/learning/paths/overlapping-github-path".to_string(),
                    Ok(overlapping_html.to_string()),
                ),
            ]),
        };

        let catalog = expand_learning_urls(
            &mut client,
            &[
                path_ref("career-essentials-in-github-professional-certificate"),
                path_ref("overlapping-github-path"),
            ],
        )
        .unwrap();

        assert_eq!(catalog.summary.unique_course_count, 4);
        assert_eq!(catalog.courses.len(), 4);
        assert!(catalog.summary.failed_paths.is_empty());
    }

    #[test]
    fn topic_listing_expands_paths_and_keeps_standalone_courses() {
        let software_html = r#"
            <script type="application/ld+json">
            {"@type":"ItemList","itemListElement":[
              {"@type":"ListItem","item":{"@type":"Course","url":"https://www.linkedin.com/learning/practical-github-actions"}}
            ]}
            </script>
        "#;
        let mut client = ScriptedClient {
            pages: HashMap::from([
                (
                    "https://www.linkedin.com/learning/topics/professional-certificates".to_string(),
                    Ok(MIXED_TOPIC_JSON.to_string()),
                ),
                (
                    "https://www.linkedin.com/learning/paths/career-essentials-in-github-professional-certificate".to_string(),
                    Ok(GITHUB_CERT_HTML.to_string()),
                ),
                (
                    "https://www.linkedin.com/learning/paths/career-essentials-in-software-development-by-microsoft-and-linkedin".to_string(),
                    Ok(software_html.to_string()),
                ),
            ]),
        };

        let catalog =
            expand_learning_urls(&mut client, &[topic_ref("professional-certificates")]).unwrap();

        let slugs: Vec<&str> = catalog
            .courses
            .iter()
            .map(|course| course.slug.as_str())
            .collect();
        assert_eq!(catalog.summary.path_count, 2);
        assert_eq!(catalog.summary.unique_course_count, 5);
        assert!(
            slugs.contains(&"career-essentials-in-system-administration-by-microsoft-and-linkedin")
        );
        assert!(catalog
            .courses
            .iter()
            .all(|course| course.slug != "welcome" && course.slug != "topics"));
    }

    #[test]
    fn topic_listing_of_only_standalone_courses_still_expands() {
        let listing = r#"{
            "elements": [{
                "entityType": "COURSE",
                "url": "https://www.linkedin.com/learning/career-essentials-in-system-administration-by-microsoft-and-linkedin"
            }]
        }"#;
        let mut client = ScriptedClient {
            pages: HashMap::from([(
                "https://www.linkedin.com/learning/topics/professional-certificates".to_string(),
                Ok(listing.to_string()),
            )]),
        };

        let catalog =
            expand_learning_urls(&mut client, &[topic_ref("professional-certificates")]).unwrap();

        assert_eq!(catalog.summary.path_count, 0);
        assert_eq!(catalog.summary.unique_course_count, 1);
        assert_eq!(
            catalog.courses[0].slug,
            "career-essentials-in-system-administration-by-microsoft-and-linkedin"
        );
    }

    #[test]
    fn all_failed_paths_yield_empty_catalog() {
        let mut client = ScriptedClient {
            pages: HashMap::from([(
                "https://www.linkedin.com/learning/paths/broken-path".to_string(),
                Err(500),
            )]),
        };
        let error = expand_learning_urls(&mut client, &[path_ref("broken-path")]).unwrap_err();
        assert_eq!(error, ExpansionError::EmptyCatalog);
    }
}
