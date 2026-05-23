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
    pub chapters: Vec<Chapter>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExerciseFile {
    pub file_name: String,
    pub download_url: String,
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
        "https://www.linkedin.com/learning-api/detailedCourses?courseSlug={course_slug}&fields=chapters,title,exerciseFiles&addParagraphsToTranscript=true&q=slugs"
    )
}

pub fn selected_video_url(course_slug: &str, video_slug: &str, height: u16) -> String {
    format!(
        "https://www.linkedin.com/learning-api/detailedCourses?courseSlug={course_slug}&resolution=_{height}&q=slugs&fields=selectedVideo&videoSlug={video_slug}"
    )
}

pub fn course_page_url(course_slug: &str) -> String {
    format!("https://www.linkedin.com/learning/{course_slug}")
}

pub fn should_fetch_selected_video_details(
    download_videos: bool,
    download_subtitles: bool,
) -> bool {
    download_videos || download_subtitles
}

pub fn fetch_course_with_selected_video_details(
    client: &mut impl CourseApiClient,
    course_slug: &str,
    selected_quality: VideoQuality,
    download_videos: bool,
    download_subtitles: bool,
) -> Result<Course, CourseFetchError> {
    let metadata = client.get(&course_metadata_url(course_slug))?;
    let mut course = parse_course_metadata(&metadata, course_slug)?;
    let _ = refresh_exercise_file_urls(client, course_slug, &mut course);
    if !should_fetch_selected_video_details(download_videos, download_subtitles) {
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
    if course.exercise_files.is_empty() {
        return Ok(0);
    }

    let html = client.get(&course_page_url(course_slug))?;
    Ok(refresh_exercise_file_urls_from_html(course, &html))
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
            course.exercise_files[index].download_url = url;
            refreshed += 1;
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
    let ambry_url_pattern = Regex::new(r#"https?://(?:www\.)?linkedin\.com/ambry/\?[^"'<>\s\\]+"#)
        .expect("valid Ambry URL regex");

    distinct_case_insensitive(
        file_url_pattern
            .find_iter(&normalized)
            .chain(ambry_url_pattern.find_iter(&normalized))
            .map(|match_| match_.as_str().trim().to_string())
            .collect(),
    )
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

    Ok(Course {
        slug: slug.to_string(),
        title: non_empty(element.title).ok_or(CourseParseError::InvalidCourseShape)?,
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
                        })
                    })
                    .collect(),
            })
            .collect(),
        exercise_files: element
            .exercise_files
            .into_iter()
            .filter_map(|file| {
                Some(ExerciseFile {
                    file_name: non_empty(file.name)?,
                    download_url: non_empty(file.url)?,
                })
            })
            .collect(),
    })
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

    Ok(CourseVideo {
        slug: slug.to_string(),
        title: non_empty(selected.title),
        duration_seconds: Some(selected.duration_in_seconds),
        download_url: selected.url.and_then(|url| non_empty(url.progressive_url)),
        transcript_srt,
    })
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
}

#[derive(Debug, Deserialize)]
struct CourseMetadataVideo {
    slug: String,
}

#[derive(Debug, Deserialize)]
struct CourseMetadataExerciseFile {
    name: String,
    url: String,
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
        assert_eq!(course.exercise_files[0].file_name, "exercise.zip");
        assert_eq!(course.chapters[0].title, "Getting started");
        assert_eq!(course.chapters[0].videos[0].slug, "welcome");
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
        assert!(!should_fetch_selected_video_details(false, false));
        assert!(should_fetch_selected_video_details(true, false));
        assert!(should_fetch_selected_video_details(false, true));
    }

    #[test]
    fn builds_linkedin_course_api_urls() {
        assert_eq!(
            course_metadata_url("sample-course"),
            "https://www.linkedin.com/learning-api/detailedCourses?courseSlug=sample-course&fields=chapters,title,exerciseFiles&addParagraphsToTranscript=true&q=slugs"
        );
        assert_eq!(
            course_page_url("sample-course"),
            "https://www.linkedin.com/learning/sample-course"
        );
        assert_eq!(
            selected_video_url("sample-course", "welcome", 720),
            "https://www.linkedin.com/learning-api/detailedCourses?courseSlug=sample-course&resolution=_720&q=slugs&fields=selectedVideo&videoSlug=welcome"
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
        )
        .unwrap();

        assert_eq!(
            course.chapters[0].videos[0].download_url.as_deref(),
            Some("https://cdn.example.test/welcome.mp4")
        );
        assert_eq!(
            course.exercise_files[0].download_url,
            "https://www.linkedin.com/ambry/?x-li-ambry-ep=EXERCISE&download=true"
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
        responses: VecDeque<(&'static str, &'static str)>,
        requested: Vec<String>,
    }

    impl FakeCourseApiClient {
        fn new(responses: Vec<(&'static str, &'static str)>) -> Self {
            Self {
                responses: responses.into(),
                requested: Vec::new(),
            }
        }
    }

    impl CourseApiClient for FakeCourseApiClient {
        fn get(&mut self, url: &str) -> Result<String, CourseFetchError> {
            self.requested.push(url.to_string());
            let Some((expected_url_part, body)) = self.responses.front().copied() else {
                return Err(CourseFetchError::Api(format!("unexpected request: {url}")));
            };
            if !url.contains(expected_url_part) {
                return Err(CourseFetchError::Api(format!(
                    "expected URL containing {expected_url_part}, got {url}"
                )));
            }
            self.responses.pop_front();
            Ok(body.to_string())
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

    fn sample_course_with_exercises(exercises: Vec<(&str, &str)>) -> Course {
        Course {
            slug: "sample-course".to_string(),
            title: "Sample Course".to_string(),
            chapters: Vec::new(),
            exercise_files: exercises
                .into_iter()
                .map(|(file_name, download_url)| ExerciseFile {
                    file_name: file_name.to_string(),
                    download_url: download_url.to_string(),
                })
                .collect(),
        }
    }
}
