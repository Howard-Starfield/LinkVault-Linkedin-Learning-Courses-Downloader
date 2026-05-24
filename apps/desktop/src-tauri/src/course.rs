use crate::quality::{fallback_order, VideoQuality};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Course {
    pub slug: String,
    pub title: String,
    pub thumbnail_url: Option<String>,
    pub chapters: Vec<Chapter>,
    pub assessments: Vec<CourseAssessment>,
    pub exercise_files: Vec<ExerciseFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chapter {
    pub title: String,
    pub videos: Vec<CourseVideo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CourseVideo {
    pub slug: String,
    pub title: Option<String>,
    pub duration_seconds: Option<u64>,
    pub download_url: Option<String>,
    pub transcript_srt: Option<String>,
    pub quiz_markdown: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExerciseFile {
    pub file_name: String,
    pub download_url: String,
    pub alternate_download_urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CourseAssessment {
    pub title: String,
    pub entity_urn: Option<String>,
    pub tracking_urn: Option<String>,
    pub quiz_markdown: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CourseParseError {
    #[error("LinkedIn token is expired. Please use a new one.")]
    ExpiredToken,
    #[error("LinkedIn course metadata shape changed")]
    InvalidCourseShape,
    #[error("LinkedIn selected video metadata shape changed")]
    InvalidSelectedVideoShape,
}

#[derive(Debug, Error)]
pub enum CourseFetchError {
    #[error("LinkedIn course API request failed: {0}")]
    Api(String),
    #[error("LinkedIn course API returned HTTP status {status}")]
    Http { status: u16 },
    #[error(transparent)]
    Parse(#[from] CourseParseError),
    #[error("LinkedIn did not return a downloadable video for {video_slug}")]
    NoDownloadableVideo { video_slug: String },
}

pub trait CourseApiClient {
    fn get(&mut self, url: &str) -> Result<String, CourseFetchError>;
}

pub fn course_metadata_url(course_slug: &str) -> String {
    format!(
        "https://www.linkedin.com/learning-api/detailedCourses?courseSlug={course_slug}&fields=chapters,title,exerciseFiles,assessments&addParagraphsToTranscript=true&q=slugs"
    )
}

pub fn selected_video_url(course_slug: &str, video_slug: &str, height: u16) -> String {
    format!(
        "https://www.linkedin.com/learning-api/detailedCourses?courseSlug={course_slug}&resolution=_{height}&q=slugs&fields=selectedVideo&videoSlug={video_slug}"
    )
}

pub fn detailed_assessment_url(tracking_urn: &str) -> String {
    format!(
        "https://www.linkedin.com/learning-api/detailedAssessments/{}",
        tracking_urn.trim().replace(':', "%3A")
    )
}

pub fn course_page_url(course_slug: &str) -> String {
    format!("https://www.linkedin.com/learning/{course_slug}")
}

pub fn should_fetch_selected_video_details(
    download_videos: bool,
    download_subtitles: bool,
    download_quizzes: bool,
) -> bool {
    download_videos || download_subtitles || download_quizzes
}

pub fn fetch_course_with_selected_video_details(
    client: &mut impl CourseApiClient,
    course_slug: &str,
    selected_quality: VideoQuality,
    download_videos: bool,
    download_subtitles: bool,
    download_quizzes: bool,
) -> Result<Course, CourseFetchError> {
    let metadata = client.get(&course_metadata_url(course_slug))?;
    let mut course = parse_course_metadata(&metadata, course_slug)?;
    let _ = refresh_exercise_file_urls(client, course_slug, &mut course);
    if download_quizzes {
        let _ = fetch_assessment_details(client, &mut course);
    }
    if !should_fetch_selected_video_details(download_videos, download_subtitles, download_quizzes) {
        return Ok(course);
    }

    for chapter in &mut course.chapters {
        for video in &mut chapter.videos {
            *video = fetch_selected_video_with_fallback(
                client,
                course_slug,
                &video.slug,
                selected_quality,
            )?;
        }
    }

    Ok(course)
}

pub fn fetch_selected_video_with_fallback(
    client: &mut impl CourseApiClient,
    course_slug: &str,
    video_slug: &str,
    selected_quality: VideoQuality,
) -> Result<CourseVideo, CourseFetchError> {
    for quality in fallback_order(selected_quality) {
        let response = client.get(&selected_video_url(
            course_slug,
            video_slug,
            quality_height(quality),
        ))?;
        let video = parse_selected_video(&response, video_slug)?;
        if video
            .download_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
        {
            if selected_quality == VideoQuality::P1080 && quality == VideoQuality::P1080 {
                if video
                    .download_url
                    .as_deref()
                    .and_then(infer_video_height_from_url)
                    .is_some_and(|height| height < 720)
                {
                    continue;
                }
            }
            return Ok(video);
        }
    }

    Err(CourseFetchError::NoDownloadableVideo {
        video_slug: video_slug.to_string(),
    })
}

pub fn refresh_exercise_file_urls(
    client: &mut impl CourseApiClient,
    course_slug: &str,
    course: &mut Course,
) -> Result<usize, CourseFetchError> {
    let html = client.get(&course_page_url(course_slug))?;
    if course.thumbnail_url.is_none() {
        course.thumbnail_url = extract_course_thumbnail_url_from_html(&html);
    }
    if course.assessments.is_empty() {
        course.assessments = extract_course_assessments_from_html(&html);
    }
    if course.exercise_files.is_empty() {
        return Ok(0);
    }
    Ok(refresh_exercise_file_urls_from_html(course, &html))
}

pub fn fetch_assessment_details(
    client: &mut impl CourseApiClient,
    course: &mut Course,
) -> Result<usize, CourseFetchError> {
    let mut fetched = 0;
    for assessment in &mut course.assessments {
        if assessment.quiz_markdown.is_some() {
            continue;
        }
        let Some(tracking_urn) = assessment
            .tracking_urn
            .as_deref()
            .filter(|urn| urn.starts_with("urn:li:lyndaAssessment:"))
        else {
            continue;
        };
        let Ok(json) = client.get(&detailed_assessment_url(tracking_urn)) else {
            continue;
        };
        if let Some(markdown) = parse_detailed_assessment_markdown(&json) {
            assessment.quiz_markdown = Some(markdown);
            fetched += 1;
        }
    }
    Ok(fetched)
}

pub fn refresh_exercise_file_urls_from_html(course: &mut Course, course_page_html: &str) -> usize {
    if course.exercise_files.is_empty() {
        return 0;
    }

    let urls = extract_exercise_file_urls_from_html(course_page_html);
    if urls.is_empty() {
        return 0;
    }

    let mut refreshed = 0;
    let mut matched_urls = HashSet::new();
    let mut unmatched_indices = Vec::new();

    for (index, exercise_file) in course.exercise_files.iter_mut().enumerate() {
        if let Some(refreshed_url) = find_exercise_file_url_by_name(&urls, &exercise_file.file_name)
        {
            if exercise_file.download_url != refreshed_url {
                push_distinct_url(
                    &mut exercise_file.alternate_download_urls,
                    exercise_file.download_url.clone(),
                );
                exercise_file.download_url = refreshed_url.clone();
            }
            matched_urls.insert(refreshed_url.to_lowercase());
            refreshed += 1;
        } else {
            unmatched_indices.push(index);
        }
    }

    let unmatched_urls = urls
        .into_iter()
        .filter(|url| !matched_urls.contains(&url.to_lowercase()))
        .collect::<Vec<_>>();

    if !unmatched_indices.is_empty() && unmatched_indices.len() == unmatched_urls.len() {
        for (index, url) in unmatched_indices.into_iter().zip(unmatched_urls) {
            if should_keep_existing_exercise_url(&course.exercise_files[index], &url) {
                push_distinct_url(
                    &mut course.exercise_files[index].alternate_download_urls,
                    url,
                );
                continue;
            }
            let current_url = course.exercise_files[index].download_url.clone();
            push_distinct_url(
                &mut course.exercise_files[index].alternate_download_urls,
                current_url,
            );
            refreshed += 1;
            course.exercise_files[index].download_url = url;
        }
    }

    refreshed
}

pub fn extract_exercise_file_urls_from_html(html: &str) -> Vec<String> {
    if html.trim().is_empty() {
        return Vec::new();
    }

    let normalized = normalize_linkedin_escaped_html(html);
    let file_url_pattern = Regex::new(
        r#"https?://[^"'<>\s\\]+(?:\.(?:zip|pdf|rar|7z|tar|gz|docx?|xlsx?|pptx?))(?:\?[^"'<>\s\\]*)?"#,
    )
    .expect("valid file URL regex");
    let ambry_url_pattern =
        Regex::new(r#"(?:https?://(?:www\.)?linkedin\.com)?/ambry/\?[^"'<>\s\\]+"#)
            .expect("valid Ambry URL regex");

    distinct_case_insensitive(
        file_url_pattern
            .find_iter(&normalized)
            .chain(ambry_url_pattern.find_iter(&normalized))
            .filter_map(|match_| normalize_exercise_artifact_url(match_.as_str().trim()))
            .collect(),
    )
}

fn normalize_exercise_artifact_url(url: &str) -> Option<String> {
    let absolute_url = if url.starts_with("/ambry/") {
        format!("https://www.linkedin.com{url}")
    } else {
        url.to_string()
    };

    let Ok(parsed) = Url::parse(&absolute_url) else {
        return Some(url.to_string());
    };
    let is_ambry = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("www.linkedin.com") || host.eq_ignore_ascii_case("linkedin.com")
    }) && parsed.path().eq_ignore_ascii_case("/ambry/");

    if is_ambry {
        let has_non_empty_endpoint = parsed.query_pairs().any(|(key, value)| {
            key.eq_ignore_ascii_case("x-li-ambry-ep") && !value.trim().is_empty()
        });
        if !has_non_empty_endpoint {
            return None;
        }
    }

    Some(parsed.to_string())
}

fn find_exercise_file_url_by_name(urls: &[String], file_name: &str) -> Option<String> {
    if file_name.trim().is_empty() {
        return None;
    }

    urls.iter()
        .find(|url| {
            get_url_file_name(url)
                .as_deref()
                .is_some_and(|url_file_name| url_file_name.eq_ignore_ascii_case(file_name))
        })
        .cloned()
}

fn should_keep_existing_exercise_url(exercise_file: &ExerciseFile, replacement_url: &str) -> bool {
    get_url_file_name(&exercise_file.download_url)
        .as_deref()
        .is_some_and(|url_file_name| url_file_name.eq_ignore_ascii_case(&exercise_file.file_name))
        && get_url_file_name(replacement_url).is_none()
}

fn push_distinct_url(urls: &mut Vec<String>, url: String) {
    if !url.trim().is_empty()
        && !urls
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&url))
    {
        urls.push(url);
    }
}

fn get_url_file_name(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    parsed
        .path_segments()?
        .next_back()
        .filter(|segment| !segment.trim().is_empty())
        .map(ToString::to_string)
}

fn normalize_linkedin_escaped_html(html: &str) -> String {
    decode_common_html_entities(
        &decode_common_html_entities(html)
            .replace("\\u002F", "/")
            .replace("\\u002f", "/")
            .replace("\\/", "/")
            .replace("\\u0026amp;", "&")
            .replace("\\u0026", "&")
            .replace("\\u003D", "=")
            .replace("\\u003d", "="),
    )
}

fn decode_common_html_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x22;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#61;", "=")
        .replace("&#x3D;", "=")
        .replace("&#x3d;", "=")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn distinct_case_insensitive(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut distinct = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_lowercase()) {
            distinct.push(trimmed.to_string());
        }
    }
    distinct
}

pub fn parse_course_metadata(json: &str, slug: &str) -> Result<Course, CourseParseError> {
    if json.contains("CSRF check failed") {
        return Err(CourseParseError::ExpiredToken);
    }

    let response: CourseMetadataResponse =
        serde_json::from_str(json).map_err(|_| CourseParseError::InvalidCourseShape)?;
    let element = response
        .elements
        .into_iter()
        .next()
        .ok_or(CourseParseError::InvalidCourseShape)?;

    let assessments = extract_chapter_assessments_from_metadata(&element.chapters);

    Ok(Course {
        slug: slug.to_string(),
        title: non_empty(element.title).ok_or(CourseParseError::InvalidCourseShape)?,
        thumbnail_url: extract_course_thumbnail_url_from_json(json),
        chapters: element
            .chapters
            .into_iter()
            .map(|chapter| Chapter {
                title: chapter.title,
                videos: chapter
                    .videos
                    .into_iter()
                    .filter_map(|video| {
                        non_empty(video.slug).map(|slug| CourseVideo {
                            slug,
                            title: None,
                            duration_seconds: None,
                            download_url: None,
                            transcript_srt: None,
                            quiz_markdown: None,
                        })
                    })
                    .collect(),
            })
            .collect(),
        assessments,
        exercise_files: element
            .exercise_files
            .into_iter()
            .filter_map(|file| {
                Some(ExerciseFile {
                    file_name: non_empty(file.name)?,
                    download_url: non_empty(file.url)?,
                    alternate_download_urls: Vec::new(),
                })
            })
            .collect(),
    })
}

fn extract_chapter_assessments_from_metadata(
    chapters: &[CourseMetadataChapter],
) -> Vec<CourseAssessment> {
    let mut seen = HashSet::new();
    let mut assessments = Vec::new();

    for chapter in chapters {
        let Some(assessment) = &chapter.assessment else {
            continue;
        };
        if !is_quiz_assessment(assessment) {
            continue;
        }
        if !seen.insert(assessment.urn.to_ascii_lowercase()) {
            continue;
        }

        assessments.push(CourseAssessment {
            title: assessment_title_for_chapter(&chapter.title, &assessment.title),
            entity_urn: assessment
                .status
                .as_ref()
                .and_then(|status| extract_learning_api_assessment_urn(&status.caching_key)),
            tracking_urn: non_empty(assessment.urn.clone()),
            quiz_markdown: None,
        });
    }

    assessments
}

fn is_quiz_assessment(assessment: &CourseMetadataAssessment) -> bool {
    assessment.assessment_type.eq_ignore_ascii_case("QUIZ")
        || assessment.assessment_type_v2.eq_ignore_ascii_case("QUIZ")
}

fn assessment_title_for_chapter(chapter_title: &str, assessment_title: &str) -> String {
    let chapter_title = chapter_title.trim();
    let assessment_title = assessment_title.trim();
    match (
        chapter_title.is_empty(),
        assessment_title.is_empty(),
        assessment_title.eq_ignore_ascii_case(chapter_title),
    ) {
        (_, false, true) => assessment_title.to_string(),
        (false, false, false) => format!("{chapter_title} - {assessment_title}"),
        (false, true, _) => chapter_title.to_string(),
        (true, false, _) => assessment_title.to_string(),
        (true, true, _) => "Chapter Quiz".to_string(),
    }
}

fn extract_learning_api_assessment_urn(value: &str) -> Option<String> {
    let pattern = Regex::new(r"urn:li:learningApiAssessment:[0-9A-Za-z_-]+")
        .expect("valid learning assessment urn regex");
    pattern
        .find(value)
        .map(|match_| match_.as_str().to_string())
}

pub fn extract_course_assessments_from_html(html: &str) -> Vec<CourseAssessment> {
    let normalized = normalize_linkedin_escaped_html(html);
    if !normalized.contains("lyndaAssessment") && !normalized.contains("learningApiAssessment") {
        return Vec::new();
    }

    let object_pattern =
        Regex::new(r#"\{[^{}]*(?:"trackingUrn"|"entityUrn")[^{}]*\}"#).expect("valid regex");
    let tracking_pattern =
        Regex::new(r#""trackingUrn"\s*:\s*"(urn:li:lyndaAssessment:[^"]+)""#).expect("valid regex");
    let entity_pattern = Regex::new(r#""entityUrn"\s*:\s*"(urn:li:learningApiAssessment:[^"]+)""#)
        .expect("valid regex");
    let title_pattern = Regex::new(r#""title"\s*:\s*"([^"]+)""#).expect("valid regex");

    let mut assessments = Vec::new();
    let mut seen = HashSet::new();
    for match_ in object_pattern.find_iter(&normalized) {
        let object = match_.as_str();
        let Some(tracking_urn) = tracking_pattern
            .captures(object)
            .and_then(|captures| captures.get(1))
            .map(|match_| match_.as_str().to_string())
        else {
            continue;
        };
        if !seen.insert(tracking_urn.clone()) {
            continue;
        }
        let entity_urn = entity_pattern
            .captures(object)
            .and_then(|captures| captures.get(1))
            .map(|match_| match_.as_str().to_string());
        let title = title_pattern
            .captures(object)
            .and_then(|captures| captures.get(1))
            .map(|match_| decode_json_string_fragment(match_.as_str()))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Chapter Quiz".to_string());

        assessments.push(CourseAssessment {
            title,
            entity_urn,
            tracking_urn: Some(tracking_urn),
            quiz_markdown: None,
        });
    }

    for entity_urn in extract_quiz_entity_urns_from_html(&normalized) {
        if !seen.insert(entity_urn.clone()) {
            continue;
        }
        assessments.push(CourseAssessment {
            title: "Chapter Quiz".to_string(),
            entity_urn: Some(entity_urn),
            tracking_urn: None,
            quiz_markdown: None,
        });
    }

    assessments
}

fn extract_quiz_entity_urns_from_html(html: &str) -> Vec<String> {
    let quiz_url_pattern =
        Regex::new(r#"/learning/[^"'\\\s]+/quiz/(urn(?::|%3A)li(?::|%3A)learningApiAssessment(?::|%3A)[^"'?\\\s<]+)"#)
            .expect("valid regex");
    let mut seen = HashSet::new();
    quiz_url_pattern
        .captures_iter(html)
        .filter_map(|captures| captures.get(1))
        .map(|match_| match_.as_str().replace("%3A", ":").replace("%3a", ":"))
        .filter(|urn| seen.insert(urn.clone()))
        .collect()
}

pub fn parse_detailed_assessment_markdown(json: &str) -> Option<String> {
    if json.contains("CSRF check failed") {
        return None;
    }

    let response: DetailedAssessmentResponse = serde_json::from_str(json).ok()?;
    let data = response.into_data();
    let questions = data.questions;
    if questions.is_empty() {
        return None;
    }

    let mut markdown = String::new();
    markdown.push_str("# ");
    markdown.push_str(
        data.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or("Chapter Quiz"),
    );
    markdown.push_str("\n\n");
    markdown.push_str("- Source assessment: `");
    markdown.push_str(&data.urn);
    markdown.push_str("`\n");
    markdown.push_str("- Format: LinkedIn Learning assessment questions and options\n\n");
    markdown.push_str("## Questions\n\n");

    for (index, question) in questions.iter().enumerate() {
        let question_text = non_empty(question.display_text.clone())
            .or_else(|| attributed_text_plain(&question.display_content_text))
            .unwrap_or_else(|| format!("Question {}", index + 1));
        markdown.push_str(&(index + 1).to_string());
        markdown.push_str(". ");
        markdown.push_str(&question_text);
        markdown.push_str("\n");

        for option in &question.options {
            let option_text = non_empty(option.label.clone())
                .or_else(|| attributed_text_plain(&option.content_label));
            if let Some(option_text) = option_text {
                markdown.push_str("   - ");
                markdown.push_str(&option_text);
                markdown.push('\n');
            }
        }
        markdown.push('\n');
    }

    Some(markdown)
}

fn attributed_text_plain(values: &[AttributedText]) -> Option<String> {
    let text = values
        .iter()
        .map(|value| value.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    non_empty(text)
}

fn decode_json_string_fragment(value: &str) -> String {
    value
        .replace(r#"\""#, "\"")
        .replace(r#"\u0027"#, "'")
        .replace(r#"\u003d"#, "=")
        .replace(r#"\u003D"#, "=")
}

pub fn extract_course_thumbnail_url_from_html(html: &str) -> Option<String> {
    let normalized = normalize_linkedin_escaped_html(html);
    let meta_pattern = Regex::new(
        r#"(?is)<meta[^>]+(?:property|name)=["'](?:og:image|twitter:image)["'][^>]+content=["']([^"']+)["']"#,
    )
    .expect("valid thumbnail metadata regex");
    meta_pattern
        .captures(&normalized)
        .and_then(|captures| captures.get(1))
        .map(|match_| match_.as_str())
        .and_then(normalize_thumbnail_url)
}

fn extract_course_thumbnail_url_from_json(json: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(json).ok()?;
    let first_element = value.get("elements")?.as_array()?.first()?;
    find_thumbnail_url_in_value(first_element, false, 0)
}

fn find_thumbnail_url_in_value(
    value: &serde_json::Value,
    image_context: bool,
    depth: usize,
) -> Option<String> {
    if depth > 8 {
        return None;
    }

    match value {
        serde_json::Value::String(text) => {
            if image_context {
                normalize_thumbnail_url(text)
            } else {
                None
            }
        }
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_thumbnail_url_in_value(value, image_context, depth + 1)),
        serde_json::Value::Object(object) => {
            if let (Some(root_url), Some(artifacts)) = (
                object.get("rootUrl").and_then(|value| value.as_str()),
                object.get("artifacts").and_then(|value| value.as_array()),
            ) {
                if let Some(segment) = artifacts.iter().find_map(|artifact| {
                    artifact
                        .get("fileIdentifyingUrlPathSegment")
                        .and_then(|value| value.as_str())
                }) {
                    if let Some(url) = normalize_thumbnail_url(&format!("{root_url}{segment}")) {
                        return Some(url);
                    }
                }
            }

            object.iter().find_map(|(key, value)| {
                let lower_key = key.to_ascii_lowercase();
                let next_context = image_context
                    || lower_key.contains("image")
                    || lower_key.contains("thumbnail")
                    || lower_key.contains("cover");
                find_thumbnail_url_in_value(value, next_context, depth + 1)
            })
        }
        _ => None,
    }
}

fn normalize_thumbnail_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("media.licdn.com")
        || lower.contains("licdn.com/dms/image")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

pub fn parse_selected_video(json: &str, slug: &str) -> Result<CourseVideo, CourseParseError> {
    if json.contains("CSRF check failed") {
        return Err(CourseParseError::ExpiredToken);
    }

    let response: SelectedVideoResponse =
        serde_json::from_str(json).map_err(|_| CourseParseError::InvalidSelectedVideoShape)?;
    let selected = response
        .elements
        .into_iter()
        .next()
        .and_then(|element| element.selected_video)
        .ok_or(CourseParseError::InvalidSelectedVideoShape)?;

    let transcript_srt = selected
        .transcript
        .as_ref()
        .map(|transcript| format_transcript_srt(&transcript.lines, selected.duration_in_seconds));
    let title = non_empty(selected.title);
    let quiz_markdown = selected.transcript.as_ref().and_then(|transcript| {
        format_quiz_markdown(title.as_deref().unwrap_or(slug), slug, &transcript.lines)
    });

    Ok(CourseVideo {
        slug: slug.to_string(),
        title,
        duration_seconds: Some(selected.duration_in_seconds),
        download_url: selected.url.and_then(|url| non_empty(url.progressive_url)),
        transcript_srt,
        quiz_markdown,
    })
}

fn format_quiz_markdown(title: &str, slug: &str, lines: &[TranscriptLine]) -> Option<String> {
    if !is_likely_quiz_video(title, slug, lines) {
        return None;
    }

    let text = transcript_plain_text(lines);
    let questions = extract_quiz_question_segments(&text);
    if questions.is_empty() {
        return None;
    }

    let mut markdown = String::new();
    markdown.push_str("# ");
    markdown.push_str(title.trim());
    markdown.push_str("\n\n");
    markdown.push_str("- Source video slug: `");
    markdown.push_str(slug.trim());
    markdown.push_str("`\n");
    markdown.push_str("- Format: transcript-derived quiz question notes\n\n");
    markdown.push_str("## Extracted Questions\n\n");

    for (index, question) in questions.iter().enumerate() {
        markdown.push_str(&(index + 1).to_string());
        markdown.push_str(". ");
        markdown.push_str(question);
        markdown.push_str("\n\n");
    }

    Some(markdown)
}

fn is_likely_quiz_video(title: &str, slug: &str, lines: &[TranscriptLine]) -> bool {
    let haystack = format!(
        "{} {} {}",
        title.to_ascii_lowercase(),
        slug.to_ascii_lowercase(),
        lines
            .iter()
            .map(|line| line.caption.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    );

    haystack.contains("quiz")
        || haystack.contains("assessment")
        || haystack.contains("question one")
        || haystack.contains("first question")
        || haystack.contains("next question")
}

fn transcript_plain_text(lines: &[TranscriptLine]) -> String {
    normalize_whitespace(
        &lines
            .iter()
            .map(|line| line.caption.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn extract_quiz_question_segments(text: &str) -> Vec<String> {
    let marker_pattern = Regex::new(
        r"(?i)\b(?:question\s+(?:one|two|three|four|five|six|seven|eight|nine|ten|\d+)|(?:the\s+)?(?:first|next|second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth)\s+question)\b",
    )
    .expect("valid quiz marker regex");
    let markers = marker_pattern
        .find_iter(text)
        .map(|match_| match_.start())
        .collect::<Vec<_>>();
    if markers.is_empty() {
        return Vec::new();
    }

    let mut questions = Vec::new();
    for (index, start) in markers.iter().enumerate() {
        let end = markers
            .get(index + 1)
            .copied()
            .unwrap_or_else(|| text.len());
        let segment = normalize_whitespace(&text[*start..end]);
        let segment = trim_quiz_segment_tail(&segment);
        if segment.contains('?') || segment.to_ascii_lowercase().contains(" options ") {
            questions.push(segment);
        }
    }

    questions
}

fn trim_quiz_segment_tail(segment: &str) -> String {
    let lower = segment.to_ascii_lowercase();
    let cut_points = [
        " thank you",
        " thanks for",
        " this concludes",
        " that concludes",
        " i hope",
    ];
    let end = cut_points
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min()
        .unwrap_or(segment.len());
    segment[..end].trim().to_string()
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_transcript_srt(lines: &[TranscriptLine], duration_seconds: u64) -> String {
    let mut srt = String::new();
    for (index, line) in lines.iter().enumerate() {
        let end_ms = lines
            .get(index + 1)
            .map(|next| next.starts_at)
            .unwrap_or(duration_seconds.saturating_mul(1000));
        srt.push_str(&(index + 1).to_string());
        srt.push('\n');
        srt.push_str(&format!(
            "{} --> {}\n",
            format_srt_time(line.starts_at),
            format_srt_time(end_ms)
        ));
        srt.push_str(&line.caption);
        srt.push_str("\n\n");
    }
    srt
}

fn format_srt_time(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

fn quality_height(quality: VideoQuality) -> u16 {
    match quality {
        VideoQuality::P1080 => 1080,
        VideoQuality::P720 => 720,
        VideoQuality::P540 => 540,
        VideoQuality::P360 => 360,
    }
}

fn infer_video_height_from_url(url: &str) -> Option<u16> {
    let dimensions = Regex::new(r"(?i)(?:^|[^0-9])[0-9]{3,4}x(1080|720|640|540|360)(?:[^0-9]|$)")
        .expect("valid video dimensions regex");
    if let Some(height) = dimensions
        .captures(url)
        .and_then(|captures| captures.get(1))
        .and_then(|height| height.as_str().parse::<u16>().ok())
    {
        return Some(height);
    }

    let height_token = Regex::new(r"(?i)(?:^|[^0-9])(1080|720|640|540|360)p(?:[^0-9]|$)")
        .expect("valid video height token regex");
    height_token
        .captures(url)
        .and_then(|captures| captures.get(1))
        .and_then(|height| height.as_str().parse::<u16>().ok())
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct CourseMetadataResponse {
    elements: Vec<CourseMetadataElement>,
}

#[derive(Debug, Deserialize)]
struct CourseMetadataElement {
    title: String,
    #[serde(default)]
    chapters: Vec<CourseMetadataChapter>,
    #[serde(default, rename = "exerciseFiles")]
    exercise_files: Vec<CourseMetadataExerciseFile>,
}

#[derive(Debug, Deserialize)]
struct CourseMetadataChapter {
    title: String,
    #[serde(default)]
    videos: Vec<CourseMetadataVideo>,
    assessment: Option<CourseMetadataAssessment>,
}

#[derive(Debug, Deserialize)]
struct CourseMetadataVideo {
    slug: String,
}

#[derive(Debug, Deserialize)]
struct CourseMetadataAssessment {
    urn: String,
    title: String,
    #[serde(default, rename = "type")]
    assessment_type: String,
    #[serde(default, rename = "typeV2")]
    assessment_type_v2: String,
    status: Option<CourseMetadataAssessmentStatus>,
}

#[derive(Debug, Deserialize)]
struct CourseMetadataAssessmentStatus {
    #[serde(rename = "cachingKey")]
    caching_key: String,
}

#[derive(Debug, Deserialize)]
struct CourseMetadataExerciseFile {
    name: String,
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DetailedAssessmentResponse {
    Wrapped { data: DetailedAssessmentData },
    Bare(DetailedAssessmentData),
}

impl DetailedAssessmentResponse {
    fn into_data(self) -> DetailedAssessmentData {
        match self {
            Self::Wrapped { data } => data,
            Self::Bare(data) => data,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DetailedAssessmentData {
    urn: String,
    title: Option<String>,
    #[serde(default)]
    questions: Vec<DetailedAssessmentQuestion>,
}

#[derive(Debug, Deserialize)]
struct DetailedAssessmentQuestion {
    #[serde(default, rename = "displayText")]
    display_text: String,
    #[serde(default, rename = "displayContentText")]
    display_content_text: Vec<AttributedText>,
    #[serde(default)]
    options: Vec<DetailedAssessmentOption>,
}

#[derive(Debug, Deserialize)]
struct DetailedAssessmentOption {
    #[serde(default)]
    label: String,
    #[serde(default, rename = "contentLabel")]
    content_label: Vec<AttributedText>,
}

#[derive(Debug, Deserialize)]
struct AttributedText {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct SelectedVideoResponse {
    elements: Vec<SelectedVideoElement>,
}

#[derive(Debug, Deserialize)]
struct SelectedVideoElement {
    #[serde(rename = "selectedVideo")]
    selected_video: Option<SelectedVideo>,
}

#[derive(Debug, Deserialize)]
struct SelectedVideo {
    title: String,
    #[serde(rename = "durationInSeconds")]
    duration_in_seconds: u64,
    url: Option<SelectedVideoUrl>,
    transcript: Option<Transcript>,
}

#[derive(Debug, Deserialize)]
struct SelectedVideoUrl {
    #[serde(rename = "progressiveUrl")]
    progressive_url: String,
}

#[derive(Debug, Deserialize)]
struct Transcript {
    #[serde(default)]
    lines: Vec<TranscriptLine>,
}

#[derive(Debug, Deserialize)]
struct TranscriptLine {
    caption: String,
    #[serde(rename = "transcriptStartAt")]
    starts_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn parses_course_metadata_title_chapters_videos_and_exercises() {
        let course = parse_course_metadata(
            r#"{
                "elements": [{
                    "title": "Sample Course",
                    "primaryImage": {
                        "rootUrl": "https://media.licdn.com/dms/image/sample/",
                        "artifacts": [{ "fileIdentifyingUrlPathSegment": "course.jpg" }]
                    },
                    "exerciseFiles": [{
                        "name": "exercise.zip",
                        "url": "https://cdn.example.test/exercise.zip"
                    }],
                    "chapters": [{
                        "title": "Getting started",
                        "videos": [{ "slug": "welcome" }]
                    }]
                }]
            }"#,
            "sample-course",
        )
        .unwrap();

        assert_eq!(course.slug, "sample-course");
        assert_eq!(course.title, "Sample Course");
        assert_eq!(
            course.thumbnail_url.as_deref(),
            Some("https://media.licdn.com/dms/image/sample/course.jpg")
        );
        assert_eq!(course.exercise_files[0].file_name, "exercise.zip");
        assert_eq!(course.chapters[0].title, "Getting started");
        assert_eq!(course.chapters[0].videos[0].slug, "welcome");
    }

    #[test]
    fn parses_chapter_quiz_assessments_from_course_metadata() {
        let course = parse_course_metadata(
            r#"{
                "elements": [{
                    "title": "Sample Course",
                    "chapters": [{
                        "title": "Prioritizing Tasks",
                        "assessment": {
                            "urn": "urn:li:lyndaAssessment:chapter-one",
                            "totalQuestions": 5,
                            "typeV2": "QUIZ",
                            "title": "Chapter Quiz",
                            "type": "QUIZ",
                            "status": {
                                "cachingKey": "ConsistentBasicAssessmentStatus;urn:li:learningApiAssessmentStatus:(urn:li:learningApiAssessment:69813586,966398456)"
                            }
                        },
                        "videos": [{ "slug": "welcome" }]
                    }, {
                        "title": "Ignored",
                        "assessment": {
                            "urn": "urn:li:lyndaAssessment:pre",
                            "typeV2": "PRE_ASSESSMENT",
                            "title": "Pre Assessment",
                            "type": "PRE_ASSESSMENT"
                        },
                        "videos": []
                    }]
                }]
            }"#,
            "sample-course",
        )
        .unwrap();

        assert_eq!(course.assessments.len(), 1);
        assert_eq!(
            course.assessments[0].title,
            "Prioritizing Tasks - Chapter Quiz"
        );
        assert_eq!(
            course.assessments[0].tracking_urn.as_deref(),
            Some("urn:li:lyndaAssessment:chapter-one")
        );
        assert_eq!(
            course.assessments[0].entity_urn.as_deref(),
            Some("urn:li:learningApiAssessment:69813586")
        );
    }

    #[test]
    fn extracts_course_thumbnail_from_course_page_html() {
        let html = r#"<html><head><meta property="og:image" content="https://media.licdn.com/dms/image/course-cover.jpg"></head></html>"#;

        assert_eq!(
            extract_course_thumbnail_url_from_html(html).as_deref(),
            Some("https://media.licdn.com/dms/image/course-cover.jpg")
        );
    }

    #[test]
    fn extracts_assessment_urns_from_rich_course_page_html() {
        let html = r#"{
            "totalQuestions":4,
            "entityType":"QUIZ",
            "trackingUrn":"urn:li:lyndaAssessment:6785398b3450b932b5ba7018",
            "title":"Chapter Quiz",
            "entityUrn":"urn:li:learningApiAssessment:69813586"
        }"#;

        let assessments = extract_course_assessments_from_html(html);

        assert_eq!(assessments.len(), 1);
        assert_eq!(assessments[0].title, "Chapter Quiz");
        assert_eq!(
            assessments[0].tracking_urn.as_deref(),
            Some("urn:li:lyndaAssessment:6785398b3450b932b5ba7018")
        );
        assert_eq!(
            assessments[0].entity_urn.as_deref(),
            Some("urn:li:learningApiAssessment:69813586")
        );
    }

    #[test]
    fn extracts_assessment_entity_urns_from_course_quiz_links() {
        let html = r#"
            <a href="/learning/time-management-for-customer-service-professionals/quiz/urn:li:learningApiAssessment:69813586?resume=false&amp;u=52983649">Chapter Quiz</a>
            <a href="/learning/time-management-for-customer-service-professionals/quiz/urn%3Ali%3AlearningApiAssessment%3A69919176">Chapter Quiz</a>
        "#;

        let assessments = extract_course_assessments_from_html(html);

        assert_eq!(assessments.len(), 2);
        assert_eq!(
            assessments
                .iter()
                .filter_map(|assessment| assessment.entity_urn.as_deref())
                .collect::<Vec<_>>(),
            vec![
                "urn:li:learningApiAssessment:69813586",
                "urn:li:learningApiAssessment:69919176"
            ]
        );
        assert!(assessments
            .iter()
            .all(|assessment| assessment.tracking_urn.is_none()));
    }

    #[test]
    fn parses_detailed_assessment_questions_into_markdown() {
        let markdown = parse_detailed_assessment_markdown(
            r#"{
                "data": {
                    "urn": "urn:li:lyndaAssessment:abc",
                    "title": "Chapter Quiz",
                    "questions": [{
                        "displayText": "When should you use templates?",
                        "options": [{
                            "optionId": 0,
                            "label": "For common inquiries"
                        }, {
                            "optionId": 1,
                            "label": "Never"
                        }]
                    }]
                }
            }"#,
        )
        .unwrap();

        assert!(markdown.contains("# Chapter Quiz"));
        assert!(markdown.contains("1. When should you use templates?"));
        assert!(markdown.contains("- For common inquiries"));
        assert!(markdown.contains("- Never"));
    }

    #[test]
    fn parses_bare_detailed_assessment_questions_into_markdown() {
        let markdown = parse_detailed_assessment_markdown(
            r#"{
                "urn": "urn:li:lyndaAssessment:chapter-one",
                "title": "Chapter Quiz",
                "questions": [{
                    "displayText": "What should you prioritize?",
                    "options": [{ "label": "The urgent customer" }]
                }]
            }"#,
        )
        .unwrap();

        assert!(markdown.contains("# Chapter Quiz"));
        assert!(markdown.contains("urn:li:lyndaAssessment:chapter-one"));
        assert!(markdown.contains("1. What should you prioritize?"));
        assert!(markdown.contains("- The urgent customer"));
    }

    #[test]
    fn parses_selected_video_and_formats_srt_transcript() {
        let video = parse_selected_video(
            r#"{
                "elements": [{
                    "selectedVideo": {
                        "title": "Welcome video",
                        "durationInSeconds": 3,
                        "url": {
                            "progressiveUrl": "https://cdn.example.test/welcome.mp4"
                        },
                        "transcript": {
                            "lines": [{
                                "caption": "Hello there",
                                "transcriptStartAt": 0
                            }, {
                                "caption": "Welcome back",
                                "transcriptStartAt": 1500
                            }]
                        }
                    }
                }]
            }"#,
            "welcome",
        )
        .unwrap();

        assert_eq!(video.slug, "welcome");
        assert_eq!(video.title.as_deref(), Some("Welcome video"));
        assert_eq!(
            video.download_url.as_deref(),
            Some("https://cdn.example.test/welcome.mp4")
        );
        let srt = video.transcript_srt.unwrap();
        assert!(srt.contains("00:00:00,000 --> 00:00:01,500"));
        assert!(srt.contains("Hello there"));
        assert!(srt.contains("00:00:01,500 --> 00:00:03,000"));
        assert!(srt.contains("Welcome back"));
    }

    #[test]
    fn parses_quiz_video_transcript_into_markdown_questions() {
        let video = parse_selected_video(
            r#"{
                "elements": [{
                    "selectedVideo": {
                        "title": "Quiz",
                        "durationInSeconds": 120,
                        "url": {
                            "progressiveUrl": "https://cdn.example.test/quiz.mp4"
                        },
                        "transcript": {
                            "lines": [{
                                "caption": "The first question, what are the types of controls in risk management? The options are preventive, detective, and corrective.",
                                "transcriptStartAt": 0
                            }, {
                                "caption": "The next question, which risk assessment approach is mathematical? The options are qualitative and quantitative.",
                                "transcriptStartAt": 10000
                            }]
                        }
                    }
                }]
            }"#,
            "quiz",
        )
        .unwrap();

        let markdown = video.quiz_markdown.unwrap();
        assert!(markdown.contains("# Quiz"));
        assert!(markdown.contains("1. The first question"));
        assert!(markdown.contains("2. The next question"));
        assert!(markdown.contains("risk assessment approach"));
    }

    #[test]
    fn selected_video_without_download_url_is_parsed_for_resolution_fallback() {
        let video = parse_selected_video(
            r#"{
                "elements": [{
                    "selectedVideo": {
                        "title": "Welcome video",
                        "durationInSeconds": 3
                    }
                }]
            }"#,
            "welcome",
        )
        .unwrap();

        assert_eq!(video.title.as_deref(), Some("Welcome video"));
        assert_eq!(video.download_url, None);
    }

    #[test]
    fn csrf_check_failure_maps_to_expired_token_without_raw_response_dump() {
        let raw = r#"{"message":"CSRF check failed","secret":"do-not-log"}"#;

        let error = parse_course_metadata(raw, "sample-course").unwrap_err();

        assert_eq!(error, CourseParseError::ExpiredToken);
        assert!(!error.to_string().contains("do-not-log"));
    }

    #[test]
    fn invalid_course_shape_error_avoids_raw_response_body() {
        let raw = r#"{"unexpected":"unsafe raw body"}"#;

        let error = parse_course_metadata(raw, "sample-course").unwrap_err();

        assert_eq!(error, CourseParseError::InvalidCourseShape);
        assert!(!error.to_string().contains("unsafe raw body"));
    }

    #[test]
    fn selected_video_fetch_is_skipped_when_videos_and_subtitles_are_off() {
        assert!(!should_fetch_selected_video_details(false, false, false));
        assert!(should_fetch_selected_video_details(true, false, false));
        assert!(should_fetch_selected_video_details(false, true, false));
        assert!(should_fetch_selected_video_details(false, false, true));
    }

    #[test]
    fn builds_linkedin_course_api_urls() {
        assert_eq!(
            course_metadata_url("sample-course"),
            "https://www.linkedin.com/learning-api/detailedCourses?courseSlug=sample-course&fields=chapters,title,exerciseFiles,assessments&addParagraphsToTranscript=true&q=slugs"
        );
        assert_eq!(
            course_page_url("sample-course"),
            "https://www.linkedin.com/learning/sample-course"
        );
        assert_eq!(
            selected_video_url("sample-course", "welcome", 720),
            "https://www.linkedin.com/learning-api/detailedCourses?courseSlug=sample-course&resolution=_720&q=slugs&fields=selectedVideo&videoSlug=welcome"
        );
        assert_eq!(
            detailed_assessment_url("urn:li:lyndaAssessment:abc"),
            "https://www.linkedin.com/learning-api/detailedAssessments/urn%3Ali%3AlyndaAssessment%3Aabc"
        );
    }

    #[test]
    fn fetches_selected_video_with_1080_first_fallback_to_720() {
        let mut client = FakeCourseApiClient::new(vec![
            ("fields=chapters,title,exerciseFiles", metadata_fixture()),
            (
                "https://www.linkedin.com/learning/sample-course",
                r#"https://www.linkedin.com/ambry/?x-li-ambry-ep=EXERCISE&download=true"#,
            ),
            (
                "resolution=_1080",
                selected_video_fixture_without_download_url(),
            ),
            (
                "resolution=_720",
                selected_video_fixture_with_download_url(),
            ),
        ]);

        let course = fetch_course_with_selected_video_details(
            &mut client,
            "sample-course",
            VideoQuality::P1080,
            true,
            true,
            true,
        )
        .unwrap();

        assert_eq!(
            course.chapters[0].videos[0].download_url.as_deref(),
            Some("https://cdn.example.test/welcome.mp4")
        );
        assert_eq!(
            course.exercise_files[0].download_url,
            "https://cdn.example.test/exercise.zip"
        );
        assert!(client
            .requested
            .iter()
            .any(|url| url == "https://www.linkedin.com/learning/sample-course"));
        assert!(client
            .requested
            .iter()
            .any(|url| url.contains("resolution=_1080")));
        assert!(client
            .requested
            .iter()
            .any(|url| url.contains("resolution=_720")));
        assert!(!client
            .requested
            .iter()
            .any(|url| url.contains("resolution=_540")));
    }

    #[test]
    fn selected_video_1080_skips_url_that_encodes_lower_height() {
        let mut client = FakeCourseApiClient::new(vec![
            (
                "resolution=_1080",
                selected_video_fixture_with_url("https://cdn.example.test/1138x640/welcome.mp4"),
            ),
            (
                "resolution=_720",
                selected_video_fixture_with_url("https://cdn.example.test/1280x720/welcome.mp4"),
            ),
        ]);

        let video = fetch_selected_video_with_fallback(
            &mut client,
            "sample-course",
            "welcome",
            VideoQuality::P1080,
        )
        .unwrap();

        assert_eq!(
            video.download_url.as_deref(),
            Some("https://cdn.example.test/1280x720/welcome.mp4")
        );
        assert!(client
            .requested
            .iter()
            .any(|url| url.contains("resolution=_1080")));
        assert!(client
            .requested
            .iter()
            .any(|url| url.contains("resolution=_720")));
    }

    #[test]
    fn fetch_course_skips_selected_video_requests_when_videos_and_subtitles_are_disabled() {
        let mut client = FakeCourseApiClient::new(vec![
            ("fields=chapters,title,exerciseFiles", metadata_fixture()),
            (
                "https://www.linkedin.com/learning/sample-course",
                r#"https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"#,
            ),
        ]);

        let course = fetch_course_with_selected_video_details(
            &mut client,
            "sample-course",
            VideoQuality::P1080,
            false,
            false,
            false,
        )
        .unwrap();

        assert_eq!(course.title, "Sample Course");
        assert_eq!(
            course.exercise_files[0].download_url,
            "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"
        );
        assert!(!client
            .requested
            .iter()
            .any(|url| url.contains("fields=selectedVideo")));
    }

    #[test]
    fn selected_video_fallback_reports_when_no_resolution_has_download_url() {
        let mut client = FakeCourseApiClient::new(vec![
            (
                "resolution=_1080",
                selected_video_fixture_without_download_url(),
            ),
            (
                "resolution=_720",
                selected_video_fixture_without_download_url(),
            ),
            (
                "resolution=_540",
                selected_video_fixture_without_download_url(),
            ),
            (
                "resolution=_360",
                selected_video_fixture_without_download_url(),
            ),
        ]);

        let error = fetch_selected_video_with_fallback(
            &mut client,
            "sample-course",
            "welcome",
            VideoQuality::P1080,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CourseFetchError::NoDownloadableVideo { video_slug } if video_slug == "welcome"
        ));
    }

    #[test]
    fn fetch_course_surfaces_expired_token_from_metadata_response() {
        let mut client = FakeCourseApiClient::new(vec![(
            "fields=chapters,title,exerciseFiles",
            r#"{"message":"CSRF check failed"}"#,
        )]);

        let error = fetch_course_with_selected_video_details(
            &mut client,
            "sample-course",
            VideoQuality::P1080,
            true,
            true,
            true,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CourseFetchError::Parse(CourseParseError::ExpiredToken)
        ));
    }

    #[test]
    fn extracts_escaped_direct_exercise_file_url() {
        let html = r#"{"url":"https:\/\/files3.lynda.com\/secure\/courses\/123\/exercises\/exercise.zip?token=a\u0026b=c"}"#;

        let urls = extract_exercise_file_urls_from_html(html);

        assert_eq!(
            urls,
            vec!["https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=a&b=c"]
        );
    }

    #[test]
    fn extracts_escaped_ambry_exercise_file_url() {
        let html = r#"{"url":"https:\/\/www.linkedin.com\/ambry\/?x-li-ambry-ep=AQK123\u0026amp;download=true"}"#;

        let urls = extract_exercise_file_urls_from_html(html);

        assert_eq!(
            urls,
            vec!["https://www.linkedin.com/ambry/?x-li-ambry-ep=AQK123&download=true"]
        );
    }

    #[test]
    fn extracts_relative_ambry_exercise_file_url() {
        let html = r#"{"url":"\/ambry\/?x-li-ambry-ep=AQK123"}"#;

        let urls = extract_exercise_file_urls_from_html(html);

        assert_eq!(
            urls,
            vec!["https://www.linkedin.com/ambry/?x-li-ambry-ep=AQK123"]
        );
    }

    #[test]
    fn extracts_html_entity_encoded_ambry_equals_sign() {
        let html = r#"{&quot;url&quot;:&quot;https://www.linkedin.com/ambry/?x-li-ambry-ep&#61;AQK123&quot;}"#;

        let urls = extract_exercise_file_urls_from_html(html);

        assert_eq!(
            urls,
            vec!["https://www.linkedin.com/ambry/?x-li-ambry-ep=AQK123"]
        );
    }

    #[test]
    fn skips_empty_ambry_exercise_file_placeholder_url() {
        let html = r#"{"url":"https:\/\/www.linkedin.com\/ambry\/?x-li-ambry-ep=\u0026amp;download=true"}"#;

        let urls = extract_exercise_file_urls_from_html(html);

        assert!(urls.is_empty());
    }

    #[test]
    fn exercise_url_extraction_deduplicates_case_insensitively() {
        let html = r#"
            https://files.example.test/exercise.zip
            https://FILES.example.test/exercise.zip
        "#;

        let urls = extract_exercise_file_urls_from_html(html);

        assert_eq!(urls, vec!["https://files.example.test/exercise.zip"]);
    }

    #[test]
    fn refresh_exercise_urls_matches_by_file_name() {
        let mut course = sample_course_with_exercises(vec![(
            "exercise.zip",
            "https://cdn.example.test/stale.zip",
        )]);
        let html =
            r#"https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"#;

        let refreshed = refresh_exercise_file_urls_from_html(&mut course, html);

        assert_eq!(refreshed, 1);
        assert_eq!(
            course.exercise_files[0].download_url,
            "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"
        );
    }

    #[test]
    fn refresh_exercise_urls_assigns_unmatched_urls_by_order_when_counts_align() {
        let mut course = sample_course_with_exercises(vec![
            ("first.zip", "https://cdn.example.test/old-first.zip"),
            ("second.zip", "https://cdn.example.test/old-second.zip"),
        ]);
        let html = r#"
            https://www.linkedin.com/ambry/?x-li-ambry-ep=FIRST&download=true
            https://www.linkedin.com/ambry/?x-li-ambry-ep=SECOND&download=true
        "#;

        let refreshed = refresh_exercise_file_urls_from_html(&mut course, html);

        assert_eq!(refreshed, 2);
        assert_eq!(
            course.exercise_files[0].download_url,
            "https://www.linkedin.com/ambry/?x-li-ambry-ep=FIRST&download=true"
        );
        assert_eq!(
            course.exercise_files[1].download_url,
            "https://www.linkedin.com/ambry/?x-li-ambry-ep=SECOND&download=true"
        );
    }

    #[test]
    fn refresh_exercise_urls_keeps_existing_named_file_url_over_unmatched_ambry() {
        let mut course = sample_course_with_exercises(vec![(
            "exercise.zip",
            "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=metadata",
        )]);
        let html = r#"https://www.linkedin.com/ambry/?x-li-ambry-ep=ONLY&download=true"#;

        let refreshed = refresh_exercise_file_urls_from_html(&mut course, html);

        assert_eq!(refreshed, 0);
        assert_eq!(
            course.exercise_files[0].download_url,
            "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=metadata"
        );
    }

    #[test]
    fn refresh_exercise_urls_keeps_unmatched_files_when_counts_do_not_align() {
        let mut course = sample_course_with_exercises(vec![
            ("first.zip", "https://cdn.example.test/old-first.zip"),
            ("second.zip", "https://cdn.example.test/old-second.zip"),
        ]);
        let html = r#"https://www.linkedin.com/ambry/?x-li-ambry-ep=ONLY&download=true"#;

        let refreshed = refresh_exercise_file_urls_from_html(&mut course, html);

        assert_eq!(refreshed, 0);
        assert_eq!(
            course.exercise_files[0].download_url,
            "https://cdn.example.test/old-first.zip"
        );
        assert_eq!(
            course.exercise_files[1].download_url,
            "https://cdn.example.test/old-second.zip"
        );
    }

    #[test]
    fn fetch_course_continues_when_exercise_refresh_request_fails() {
        let mut client = FakeCourseApiClient::new(vec![
            ("fields=chapters,title,exerciseFiles", metadata_fixture()),
            (
                "resolution=_1080",
                selected_video_fixture_with_download_url(),
            ),
        ]);

        let course = fetch_course_with_selected_video_details(
            &mut client,
            "sample-course",
            VideoQuality::P1080,
            true,
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            course.exercise_files[0].download_url,
            "https://cdn.example.test/exercise.zip"
        );
        assert_eq!(
            course.chapters[0].videos[0].download_url.as_deref(),
            Some("https://cdn.example.test/welcome.mp4")
        );
        assert!(client
            .requested
            .iter()
            .any(|url| url == "https://www.linkedin.com/learning/sample-course"));
    }

    struct FakeCourseApiClient {
        responses: VecDeque<(&'static str, String)>,
        requested: Vec<String>,
    }

    impl FakeCourseApiClient {
        fn new(responses: Vec<(&'static str, impl Into<String>)>) -> Self {
            Self {
                responses: responses
                    .into_iter()
                    .map(|(expected_url_part, body)| (expected_url_part, body.into()))
                    .collect(),
                requested: Vec::new(),
            }
        }
    }

    impl CourseApiClient for FakeCourseApiClient {
        fn get(&mut self, url: &str) -> Result<String, CourseFetchError> {
            self.requested.push(url.to_string());
            let Some((expected_url_part, _body)) = self.responses.front() else {
                return Err(CourseFetchError::Api(format!("unexpected request: {url}")));
            };
            if !url.contains(expected_url_part) {
                return Err(CourseFetchError::Api(format!(
                    "expected URL containing {expected_url_part}, got {url}"
                )));
            }
            Ok(self.responses.pop_front().unwrap().1)
        }
    }

    fn metadata_fixture() -> &'static str {
        r#"{
            "elements": [{
                "title": "Sample Course",
                "exerciseFiles": [{
                    "name": "exercise.zip",
                    "url": "https://cdn.example.test/exercise.zip"
                }],
                "chapters": [{
                    "title": "Getting started",
                    "videos": [{ "slug": "welcome" }]
                }]
            }]
        }"#
    }

    fn selected_video_fixture_without_download_url() -> &'static str {
        r#"{
            "elements": [{
                "selectedVideo": {
                    "title": "Welcome video",
                    "durationInSeconds": 3
                }
            }]
        }"#
    }

    fn selected_video_fixture_with_download_url() -> &'static str {
        r#"{
            "elements": [{
                "selectedVideo": {
                    "title": "Welcome video",
                    "durationInSeconds": 3,
                    "url": {
                        "progressiveUrl": "https://cdn.example.test/welcome.mp4"
                    }
                }
            }]
        }"#
    }

    fn selected_video_fixture_with_url(url: &str) -> String {
        serde_json::json!({
            "elements": [{
                "selectedVideo": {
                    "title": "Welcome video",
                    "durationInSeconds": 3,
                    "url": {
                        "progressiveUrl": url
                    }
                }
            }]
        })
        .to_string()
    }

    fn sample_course_with_exercises(exercises: Vec<(&str, &str)>) -> Course {
        Course {
            slug: "sample-course".to_string(),
            title: "Sample Course".to_string(),
            thumbnail_url: None,
            chapters: Vec::new(),
            assessments: Vec::new(),
            exercise_files: exercises
                .into_iter()
                .map(|(file_name, download_url)| ExerciseFile {
                    file_name: file_name.to_string(),
                    download_url: download_url.to_string(),
                    alternate_download_urls: Vec::new(),
                })
                .collect(),
        }
    }
}
