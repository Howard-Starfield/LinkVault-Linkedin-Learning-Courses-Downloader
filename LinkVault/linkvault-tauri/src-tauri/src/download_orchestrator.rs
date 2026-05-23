use crate::artifact_downloader::{
    download_artifacts_for_active_job, ArtifactDownloadError, ArtifactDownloadSource,
    ArtifactDownloadSummary, ArtifactHttpClient, CancellationFlag, PlannedArtifactDownload,
};
use crate::cache::{
    append_job_event, get_job, list_jobs_by_status, transition_job_status, upsert_artifact,
    upsert_course_cache_entry, ArtifactRecord, CacheError, CourseCacheEntry, JobRecord,
    NewJobEvent,
};
use crate::course::{
    fetch_course_with_selected_video_details, Course, CourseApiClient, CourseFetchError,
};
use crate::quality::VideoQuality;
use rusqlite::Connection;
use serde::Serialize;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedQueuedJob {
    pub job_id: String,
    pub course_slug: String,
    pub planned_artifacts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedActiveJob {
    active_job: JobRecord,
    downloads: Vec<PlannedArtifactDownload>,
}

#[derive(Debug, Error)]
pub enum DownloadOrchestrationError {
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error(transparent)]
    Artifact(#[from] ArtifactDownloadError),
    #[error(transparent)]
    Course(#[from] CourseFetchError),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error("unsupported selected quality: {0}")]
    InvalidQuality(String),
}

pub fn process_next_queued_job(
    connection: &Connection,
    client: &mut impl CourseApiClient,
    timestamp: i64,
) -> Result<Option<ProcessedQueuedJob>, DownloadOrchestrationError> {
    let Some(queued_job) = list_jobs_by_status(connection, "queued")?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };

    transition_job_status(
        connection,
        &queued_job.id,
        "active",
        timestamp,
        Some("Started course metadata fetch."),
    )?;

    match plan_active_job(connection, client, &queued_job, timestamp) {
        Ok(processed) => Ok(Some(processed)),
        Err(error) => {
            transition_job_status(
                connection,
                &queued_job.id,
                "failed",
                timestamp,
                Some("Course metadata fetch or artifact planning failed."),
            )?;
            Err(error)
        }
    }
}

pub fn process_next_queued_job_and_download_artifacts(
    connection: &Connection,
    course_client: &mut impl CourseApiClient,
    artifact_client: &mut impl ArtifactHttpClient,
    cancellation: &impl CancellationFlag,
    timestamp: i64,
) -> Result<Option<ArtifactDownloadSummary>, DownloadOrchestrationError> {
    let Some(queued_job) = list_jobs_by_status(connection, "queued")?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };

    transition_job_status(
        connection,
        &queued_job.id,
        "active",
        timestamp,
        Some("Started course metadata fetch."),
    )?;

    if cancellation.is_cancelled() {
        transition_job_status(
            connection,
            &queued_job.id,
            "cancelled",
            timestamp,
            Some("Download was cancelled before course metadata fetch."),
        )?;
        return Ok(Some(ArtifactDownloadSummary {
            completed: 0,
            failed: 0,
            cancelled: 0,
        }));
    }

    let planned_job =
        match plan_active_job_downloads(connection, course_client, &queued_job, timestamp) {
            Ok(planned_job) => planned_job,
            Err(error) => {
                transition_job_status(
                    connection,
                    &queued_job.id,
                    "failed",
                    timestamp,
                    Some("Course metadata fetch or artifact planning failed."),
                )?;
                return Err(error);
            }
        };

    let summary = download_artifacts_for_active_job(
        connection,
        artifact_client,
        cancellation,
        &queued_job.id,
        &planned_job.downloads,
        timestamp,
    )?;

    Ok(Some(summary))
}

fn plan_active_job(
    connection: &Connection,
    client: &mut impl CourseApiClient,
    job: &JobRecord,
    timestamp: i64,
) -> Result<ProcessedQueuedJob, DownloadOrchestrationError> {
    let planned_job = plan_active_job_downloads(connection, client, job, timestamp)?;
    let active_job = planned_job.active_job;

    Ok(ProcessedQueuedJob {
        job_id: active_job.id,
        course_slug: active_job.course_slug,
        planned_artifacts: planned_job.downloads.len(),
    })
}

fn plan_active_job_downloads(
    connection: &Connection,
    client: &mut impl CourseApiClient,
    job: &JobRecord,
    timestamp: i64,
) -> Result<PlannedActiveJob, DownloadOrchestrationError> {
    let selected_quality = parse_selected_quality(&job.selected_quality)?;
    let course = fetch_course_with_selected_video_details(
        client,
        &job.course_slug,
        selected_quality,
        job.download_videos,
        job.download_subtitles,
    )?;
    let safe_payload = serde_json::to_string(&CachedCoursePayload::from(&course))?;

    upsert_course_cache_entry(
        connection,
        &CourseCacheEntry {
            course_slug: course.slug.clone(),
            source_url: linkedin_course_url(&course.slug),
            title: Some(course.title.clone()),
            payload_json: safe_payload,
            fetched_at: timestamp,
        },
    )?;
    append_job_event(
        connection,
        &NewJobEvent {
            job_id: job.id.clone(),
            event_type: "course.metadata.cached".to_string(),
            message: format!("Cached course metadata for {}.", course.title),
            payload_json: Some(
                serde_json::json!({
                    "courseSlug": course.slug,
                    "chapterCount": course.chapters.len(),
                    "exerciseFileCount": course.exercise_files.len(),
                })
                .to_string(),
            ),
            created_at: timestamp,
        },
    )?;

    let downloads = build_initial_artifact_downloads(job, &course, timestamp);
    for download in &downloads {
        upsert_artifact(connection, &download.artifact)?;
    }
    append_job_event(
        connection,
        &NewJobEvent {
            job_id: job.id.clone(),
            event_type: "artifacts.planned".to_string(),
            message: format!("Planned {} download artifacts.", downloads.len()),
            payload_json: Some(
                serde_json::json!({
                    "plannedArtifacts": downloads.len(),
                    "downloadVideos": job.download_videos,
                    "downloadExercises": job.download_exercises,
                    "downloadSubtitles": job.download_subtitles,
                })
                .to_string(),
            ),
            created_at: timestamp,
        },
    )?;

    let active_job =
        get_job(connection, &job.id)?.ok_or_else(|| CacheError::JobNotFound(job.id.clone()))?;

    Ok(PlannedActiveJob {
        active_job,
        downloads,
    })
}

fn parse_selected_quality(value: &str) -> Result<VideoQuality, DownloadOrchestrationError> {
    match value.trim() {
        "1080" | "1080p" | "1080p (Best available)" => Ok(VideoQuality::P1080),
        "720" | "720p" => Ok(VideoQuality::P720),
        "540" | "540p" => Ok(VideoQuality::P540),
        "360" | "360p" => Ok(VideoQuality::P360),
        other => Err(DownloadOrchestrationError::InvalidQuality(
            other.to_string(),
        )),
    }
}

fn build_initial_artifact_downloads(
    job: &JobRecord,
    course: &Course,
    timestamp: i64,
) -> Vec<PlannedArtifactDownload> {
    let mut downloads = Vec::new();
    let course_dir = safe_file_name(&course.title);

    for (video_index, video) in course
        .chapters
        .iter()
        .flat_map(|chapter| chapter.videos.iter())
        .enumerate()
    {
        let video_name = safe_file_name(video.title.as_deref().unwrap_or(&video.slug));
        if job.download_videos {
            if let Some(download_url) = &video.download_url {
                downloads.push(PlannedArtifactDownload {
                    artifact: ArtifactRecord {
                        id: format!("artifact-{}-video-{video_index}", job.id),
                        job_id: job.id.clone(),
                        artifact_type: "video".to_string(),
                        path: planned_path(
                            &job.output_dir,
                            &course_dir,
                            &format!("{video_name}.mp4"),
                        ),
                        status: "pending".to_string(),
                        size_bytes: None,
                        created_at: timestamp,
                        updated_at: timestamp,
                    },
                    source: ArtifactDownloadSource::Url(download_url.clone()),
                });
            }
        }
        if job.download_subtitles {
            if let Some(transcript_srt) = &video.transcript_srt {
                downloads.push(PlannedArtifactDownload {
                    artifact: ArtifactRecord {
                        id: format!("artifact-{}-subtitle-{video_index}", job.id),
                        job_id: job.id.clone(),
                        artifact_type: "subtitle".to_string(),
                        path: planned_path(
                            &job.output_dir,
                            &course_dir,
                            &format!("{video_name}.srt"),
                        ),
                        status: "pending".to_string(),
                        size_bytes: None,
                        created_at: timestamp,
                        updated_at: timestamp,
                    },
                    source: ArtifactDownloadSource::Text(transcript_srt.clone()),
                });
            }
        }
    }

    if job.download_exercises {
        for (exercise_index, exercise_file) in course.exercise_files.iter().enumerate() {
            let file_name = safe_file_name(&exercise_file.file_name);
            downloads.push(PlannedArtifactDownload {
                artifact: ArtifactRecord {
                    id: format!("artifact-{}-exercise-{exercise_index}", job.id),
                    job_id: job.id.clone(),
                    artifact_type: exercise_artifact_type(&file_name).to_string(),
                    path: planned_path(&job.output_dir, &course_dir, &file_name),
                    status: "pending".to_string(),
                    size_bytes: None,
                    created_at: timestamp,
                    updated_at: timestamp,
                },
                source: ArtifactDownloadSource::Url(exercise_file.download_url.clone()),
            });
        }
    }

    downloads
}

fn exercise_artifact_type(file_name: &str) -> &'static str {
    if file_name.to_ascii_lowercase().ends_with(".zip") {
        "exercise_zip"
    } else {
        "exercise_file"
    }
}

fn planned_path(output_dir: &str, course_dir: &str, file_name: &str) -> String {
    let mut path = PathBuf::from(output_dir);
    path.push(course_dir);
    path.push(file_name);
    path.to_string_lossy().to_string()
}

fn linkedin_course_url(course_slug: &str) -> String {
    format!("https://www.linkedin.com/learning/{course_slug}")
}

fn safe_file_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) || character.is_control()
            {
                '-'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();

    if sanitized.is_empty() {
        "download".to_string()
    } else {
        sanitized
    }
}

#[derive(Debug, Serialize)]
struct CachedCoursePayload<'a> {
    slug: &'a str,
    title: &'a str,
    chapters: Vec<CachedChapter<'a>>,
    exercise_files: Vec<CachedExerciseFile<'a>>,
}

#[derive(Debug, Serialize)]
struct CachedChapter<'a> {
    title: &'a str,
    videos: Vec<CachedVideo<'a>>,
}

#[derive(Debug, Serialize)]
struct CachedVideo<'a> {
    slug: &'a str,
    title: Option<&'a str>,
    duration_seconds: Option<u64>,
    has_download_url: bool,
    has_transcript: bool,
}

#[derive(Debug, Serialize)]
struct CachedExerciseFile<'a> {
    file_name: &'a str,
}

impl<'a> From<&'a Course> for CachedCoursePayload<'a> {
    fn from(course: &'a Course) -> Self {
        Self {
            slug: &course.slug,
            title: &course.title,
            chapters: course
                .chapters
                .iter()
                .map(|chapter| CachedChapter {
                    title: &chapter.title,
                    videos: chapter
                        .videos
                        .iter()
                        .map(|video| CachedVideo {
                            slug: &video.slug,
                            title: video.title.as_deref(),
                            duration_seconds: video.duration_seconds,
                            has_download_url: video.download_url.is_some(),
                            has_transcript: video.transcript_srt.is_some(),
                        })
                        .collect(),
                })
                .collect(),
            exercise_files: course
                .exercise_files
                .iter()
                .map(|exercise_file| CachedExerciseFile {
                    file_name: &exercise_file.file_name,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_downloader::{ArtifactHttpResponse, NeverCancelled};
    use crate::cache::{
        get_course_cache_entry, initialize, insert_job, list_artifacts_for_job, list_job_events,
    };
    use crate::course::CourseFetchError;
    use rusqlite::Connection;
    use std::collections::VecDeque;
    use std::fs;
    use tempfile::tempdir;

    fn initialized_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn process_next_queued_job_fetches_course_caches_metadata_and_plans_artifacts() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-1", "queued", 100)).unwrap();
        let mut client = FakeCourseApiClient::new(vec![
            ("fields=chapters,title,exerciseFiles", metadata_fixture()),
            (
                "https://www.linkedin.com/learning/sample-course",
                r#"https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"#,
            ),
            ("resolution=_1080", selected_video_fixture()),
        ]);

        let processed = process_next_queued_job(&connection, &mut client, 200)
            .unwrap()
            .unwrap();
        let active_jobs = list_jobs_by_status(&connection, "active").unwrap();
        let cached = get_course_cache_entry(&connection, "sample-course")
            .unwrap()
            .unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(processed.job_id, "job-1");
        assert_eq!(processed.course_slug, "sample-course");
        assert_eq!(processed.planned_artifacts, 3);
        assert_eq!(active_jobs.len(), 1);
        assert_eq!(cached.title.as_deref(), Some("Sample Course"));
        assert!(cached.payload_json.contains(r#""has_download_url":true"#));
        assert!(cached.payload_json.contains(r#""has_transcript":true"#));
        assert!(!cached.payload_json.contains("fresh"));
        assert!(!cached.payload_json.contains("progressiveUrl"));
        assert_eq!(artifacts.len(), 3);
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.artifact_type == "video"));
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.artifact_type == "subtitle"));
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.artifact_type == "exercise_zip"));
        assert!(artifacts
            .iter()
            .all(|artifact| artifact.status == "pending" && artifact.size_bytes.is_none()));
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["job.active", "course.metadata.cached", "artifacts.planned"]
        );
    }

    #[test]
    fn process_next_queued_job_marks_job_failed_when_metadata_fetch_fails() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-1", "queued", 100)).unwrap();
        let mut client = FakeCourseApiClient::new(vec![(
            "fields=chapters,title,exerciseFiles",
            r#"{"message":"CSRF check failed"}"#,
        )]);

        let error = process_next_queued_job(&connection, &mut client, 200).unwrap_err();
        let failed_jobs = list_jobs_by_status(&connection, "failed").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert!(matches!(
            error,
            DownloadOrchestrationError::Course(CourseFetchError::Parse(_))
        ));
        assert_eq!(failed_jobs.len(), 1);
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["job.active", "job.failed"]
        );
        assert!(list_artifacts_for_job(&connection, "job-1")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn process_next_queued_job_returns_none_when_no_queued_job_exists() {
        let connection = initialized_connection();
        let mut client = FakeCourseApiClient::new(vec![]);

        let processed = process_next_queued_job(&connection, &mut client, 200).unwrap();

        assert!(processed.is_none());
    }

    #[test]
    fn process_next_queued_job_and_download_artifacts_completes_files_and_job() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        let mut job = sample_job("job-1", "queued", 100);
        job.output_dir = output.path().to_string_lossy().to_string();
        insert_job(&connection, &job).unwrap();
        let mut course_client = FakeCourseApiClient::new(vec![
            ("fields=chapters,title,exerciseFiles", metadata_fixture()),
            (
                "https://www.linkedin.com/learning/sample-course",
                r#"https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"#,
            ),
            ("resolution=_1080", selected_video_fixture()),
        ]);
        let mut artifact_client = FakeArtifactClient::new(vec![
            ("https://cdn.example.test/welcome.mp4", 200, b"video"),
            (
                "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh",
                404,
                b"",
            ),
        ]);

        let summary = process_next_queued_job_and_download_artifacts(
            &connection,
            &mut course_client,
            &mut artifact_client,
            &NeverCancelled,
            200,
        )
        .unwrap()
        .unwrap();
        let completed_jobs = list_jobs_by_status(&connection, "completed").unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let video_path = output
            .path()
            .join("Sample Course")
            .join("Welcome video.mp4");
        let subtitle_path = output
            .path()
            .join("Sample Course")
            .join("Welcome video.srt");

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 2,
                failed: 1,
                cancelled: 0
            }
        );
        assert_eq!(completed_jobs.len(), 1);
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
        assert!(fs::read_to_string(&subtitle_path)
            .unwrap()
            .contains("Welcome."));
        assert_eq!(artifacts.len(), 3);
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.artifact_type == "exercise_zip")
                .unwrap()
                .status,
            "failed"
        );
    }

    #[test]
    fn process_next_queued_job_and_download_artifacts_cancels_before_metadata_fetch() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-1", "queued", 100)).unwrap();
        let mut course_client = FakeCourseApiClient::new(vec![]);
        let mut artifact_client = FakeArtifactClient::new(vec![]);
        let cancellation = AlwaysCancelled;

        let summary = process_next_queued_job_and_download_artifacts(
            &connection,
            &mut course_client,
            &mut artifact_client,
            &cancellation,
            200,
        )
        .unwrap()
        .unwrap();

        let cancelled_jobs = list_jobs_by_status(&connection, "cancelled").unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 0,
                failed: 0,
                cancelled: 0
            }
        );
        assert_eq!(cancelled_jobs.len(), 1);
        assert!(artifacts.is_empty());
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["job.active", "job.cancelled"]
        );
    }

    struct AlwaysCancelled;

    impl CancellationFlag for AlwaysCancelled {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    struct FakeCourseApiClient {
        responses: VecDeque<(&'static str, &'static str)>,
    }

    impl FakeCourseApiClient {
        fn new(responses: Vec<(&'static str, &'static str)>) -> Self {
            Self {
                responses: responses.into(),
            }
        }
    }

    impl CourseApiClient for FakeCourseApiClient {
        fn get(&mut self, url: &str) -> Result<String, CourseFetchError> {
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

    struct FakeArtifactClient {
        responses: VecDeque<(&'static str, u16, &'static [u8])>,
    }

    impl FakeArtifactClient {
        fn new(responses: Vec<(&'static str, u16, &'static [u8])>) -> Self {
            Self {
                responses: responses.into(),
            }
        }
    }

    impl ArtifactHttpClient for FakeArtifactClient {
        fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError> {
            let Some((expected_url, status, bytes)) = self.responses.pop_front() else {
                panic!("unexpected request: {url}");
            };
            assert_eq!(url, expected_url);
            Ok(ArtifactHttpResponse {
                status,
                bytes: bytes.to_vec(),
            })
        }
    }

    fn sample_job(id: &str, status: &str, timestamp: i64) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            course_slug: "sample-course".to_string(),
            status: status.to_string(),
            selected_quality: "1080".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            output_dir: "C:/downloads".to_string(),
            created_at: timestamp,
            updated_at: timestamp,
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

    fn selected_video_fixture() -> &'static str {
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
                            "caption": "Welcome.",
                            "transcriptStartAt": 0
                        }]
                    }
                }
            }]
        }"#
    }
}
