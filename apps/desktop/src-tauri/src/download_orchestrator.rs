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
    fetch_course_with_selected_video_details, Course, CourseApiClient, CourseAssessment,
    CourseFetchError,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueueStopReason {
    kind: &'static str,
    message: &'static str,
    status: Option<u16>,
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
            let stop_reason = queue_stop_reason_for_orchestration_error(&error);
            transition_job_status(
                connection,
                &queued_job.id,
                "failed",
                timestamp,
                Some(
                    stop_reason
                        .map(|reason| reason.message)
                        .unwrap_or("Course metadata fetch or artifact planning failed."),
                ),
            )?;
            if let Some(reason) = stop_reason {
                append_queue_stop_event(connection, &queued_job.id, reason, timestamp)?;
            }
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
    process_next_queued_job_and_download_artifacts_with_quiz_assessments(
        connection,
        course_client,
        artifact_client,
        cancellation,
        timestamp,
        Vec::new(),
    )
}

pub fn process_next_queued_job_and_download_artifacts_with_quiz_assessments(
    connection: &Connection,
    course_client: &mut impl CourseApiClient,
    artifact_client: &mut impl ArtifactHttpClient,
    cancellation: &impl CancellationFlag,
    timestamp: i64,
    quiz_assessments: Vec<CourseAssessment>,
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

    let planned_job = match plan_active_job_downloads(
        connection,
        course_client,
        &queued_job,
        timestamp,
        quiz_assessments,
    ) {
        Ok(planned_job) => planned_job,
        Err(error) => {
            let stop_reason = queue_stop_reason_for_orchestration_error(&error);
            transition_job_status(
                connection,
                &queued_job.id,
                "failed",
                timestamp,
                Some(
                    stop_reason
                        .map(|reason| reason.message)
                        .unwrap_or("Course metadata fetch or artifact planning failed."),
                ),
            )?;
            if let Some(reason) = stop_reason {
                append_queue_stop_event(connection, &queued_job.id, reason, timestamp)?;
            }
            return Err(error);
        }
    };

    let summary = match download_artifacts_for_active_job(
        connection,
        artifact_client,
        cancellation,
        &queued_job.id,
        &planned_job.downloads,
        timestamp,
    ) {
        Ok(summary) => summary,
        Err(error) => {
            if let Some(reason) = queue_stop_reason_for_artifact_error(&error) {
                append_queue_stop_event(connection, &queued_job.id, reason, timestamp)?;
            }
            return Err(DownloadOrchestrationError::Artifact(error));
        }
    };

    Ok(Some(summary))
}

fn append_queue_stop_event(
    connection: &Connection,
    job_id: &str,
    reason: QueueStopReason,
    timestamp: i64,
) -> Result<(), CacheError> {
    append_job_event(
        connection,
        &NewJobEvent {
            job_id: job_id.to_string(),
            event_type: "queue.guardrail.stop".to_string(),
            message: reason.message.to_string(),
            payload_json: Some(
                serde_json::json!({
                    "kind": reason.kind,
                    "status": reason.status,
                    "remainingJobs": "left_queued_for_manual_resume"
                })
                .to_string(),
            ),
            created_at: timestamp,
        },
    )?;
    Ok(())
}

fn queue_stop_reason_for_orchestration_error(
    error: &DownloadOrchestrationError,
) -> Option<QueueStopReason> {
    match error {
        DownloadOrchestrationError::Course(error) => queue_stop_reason_for_course_error(error),
        DownloadOrchestrationError::Artifact(error) => queue_stop_reason_for_artifact_error(error),
        _ => None,
    }
}

fn queue_stop_reason_for_course_error(error: &CourseFetchError) -> Option<QueueStopReason> {
    match error {
        CourseFetchError::Parse(crate::course::CourseParseError::ExpiredToken) => {
            Some(queue_stop_reason("auth", "Queue stopped: LinkedIn session expired. Refresh the saved session before resuming.", None))
        }
        CourseFetchError::Http { status } => queue_stop_reason_for_status(*status),
        CourseFetchError::Api(message) => queue_stop_reason_for_api_message(message),
        _ => None,
    }
}

fn queue_stop_reason_for_artifact_error(error: &ArtifactDownloadError) -> Option<QueueStopReason> {
    match error {
        ArtifactDownloadError::Http { status, .. } => queue_stop_reason_for_status(*status),
        ArtifactDownloadError::AttemptsFailed { attempts } => attempts
            .iter()
            .filter_map(|attempt| attempt.status)
            .find_map(queue_stop_reason_for_status),
        _ => None,
    }
}

fn queue_stop_reason_for_api_message(message: &str) -> Option<QueueStopReason> {
    extract_http_status(message).and_then(queue_stop_reason_for_status)
}

fn extract_http_status(message: &str) -> Option<u16> {
    message
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| part.len() == 3)
        .filter_map(|part| part.parse::<u16>().ok())
        .find(|status| (100..=599).contains(status))
}

fn queue_stop_reason_for_status(status: u16) -> Option<QueueStopReason> {
    match status {
        401 | 403 => Some(queue_stop_reason(
            "auth",
            "Queue stopped: LinkedIn rejected the saved session. Refresh the saved session before resuming.",
            Some(status),
        )),
        429 => Some(queue_stop_reason(
            "rate_limit",
            "Queue stopped: LinkedIn is rate limiting requests. Wait before resuming the queue.",
            Some(status),
        )),
        500..=599 => Some(queue_stop_reason(
            "server",
            "Queue stopped: LinkedIn returned repeated server errors. Wait before resuming the queue.",
            Some(status),
        )),
        _ => None,
    }
}

fn queue_stop_reason(
    kind: &'static str,
    message: &'static str,
    status: Option<u16>,
) -> QueueStopReason {
    QueueStopReason {
        kind,
        message,
        status,
    }
}

fn plan_active_job(
    connection: &Connection,
    client: &mut impl CourseApiClient,
    job: &JobRecord,
    timestamp: i64,
) -> Result<ProcessedQueuedJob, DownloadOrchestrationError> {
    let planned_job = plan_active_job_downloads(connection, client, job, timestamp, Vec::new())?;
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
    quiz_assessments: Vec<CourseAssessment>,
) -> Result<PlannedActiveJob, DownloadOrchestrationError> {
    let selected_quality = parse_selected_quality(&job.selected_quality)?;
    let mut course = fetch_course_with_selected_video_details(
        client,
        &job.course_slug,
        selected_quality,
        job.download_videos,
        job.download_exercises,
        job.download_subtitles,
        job.download_quizzes,
    )?;
    let merged_quiz_assessments = merge_browser_quiz_assessments(&mut course, quiz_assessments);
    let quiz_assessment_count = course.assessments.len();
    let quiz_markdown_count = course
        .assessments
        .iter()
        .filter(|assessment| assessment.quiz_markdown.is_some())
        .count();
    let safe_payload = serde_json::to_string(&CachedCoursePayload::from(&course))?;

    upsert_course_cache_entry(
        connection,
        &CourseCacheEntry {
            course_slug: course.slug.clone(),
            source_url: if job.source_url.trim().is_empty() {
                linkedin_course_url(&course.slug)
            } else {
                job.source_url.clone()
            },
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
                    "quizAssessmentCount": quiz_assessment_count,
                    "quizMarkdownCount": quiz_markdown_count,
                    "browserQuizAssessmentCount": merged_quiz_assessments,
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
                    "downloadQuizzes": job.download_quizzes,
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

pub fn merge_browser_quiz_assessments(
    course: &mut Course,
    quiz_assessments: Vec<CourseAssessment>,
) -> usize {
    let mut merged = 0;
    for mut incoming in quiz_assessments {
        if incoming
            .quiz_markdown
            .as_deref()
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            continue;
        }

        if let Some(existing) = course
            .assessments
            .iter_mut()
            .find(|existing| same_assessment(existing, &incoming))
        {
            if existing.quiz_markdown.is_none() {
                existing.quiz_markdown = incoming.quiz_markdown.take();
                merged += 1;
            }
            continue;
        }

        course.assessments.push(incoming);
        merged += 1;
    }
    merged
}

fn same_assessment(left: &CourseAssessment, right: &CourseAssessment) -> bool {
    left.entity_urn
        .as_deref()
        .zip(right.entity_urn.as_deref())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        || left
            .tracking_urn
            .as_deref()
            .zip(right.tracking_urn.as_deref())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
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
    let mut text_downloads = Vec::new();
    let mut video_downloads = Vec::new();
    let mut exercise_downloads = Vec::new();
    let mut study_videos = Vec::new();
    let mut study_assessments = Vec::new();
    let course_dir = safe_file_name(&course.title);

    let mut video_artifact_index = 0;
    for (chapter_index, chapter) in course.chapters.iter().enumerate() {
        let chapter_dir = format!(
            "{:02} - {}",
            chapter_index + 1,
            safe_file_name(&chapter.title)
        );
        for (video_index, video) in chapter.videos.iter().enumerate() {
            let video_name = format!(
                "{:02} - {}",
                video_index + 1,
                safe_file_name(video.title.as_deref().unwrap_or(&video.slug))
            );
            if job.download_videos {
                if let Some(download_url) = &video.download_url {
                    let video_path = planned_path(
                        &job.output_dir,
                        &[&course_dir, &chapter_dir, &format!("{video_name}.mp4")],
                    );
                    video_downloads.push(PlannedArtifactDownload {
                        artifact: ArtifactRecord {
                            id: format!("artifact-{}-video-{video_artifact_index}", job.id),
                            job_id: job.id.clone(),
                            artifact_type: "video".to_string(),
                            path: video_path.clone(),
                            status: "pending".to_string(),
                            size_bytes: None,
                            created_at: timestamp,
                            updated_at: timestamp,
                        },
                        source: ArtifactDownloadSource::Url(download_url.clone()),
                    });
                    ensure_study_video(
                        &mut study_videos,
                        chapter_index,
                        video_artifact_index,
                        &chapter.title,
                        video.title.as_deref().unwrap_or(&video.slug),
                    );
                }
            }
            if job.download_subtitles {
                if let Some(transcript_srt) = &video.transcript_srt {
                    text_downloads.push(PlannedArtifactDownload {
                        artifact: ArtifactRecord {
                            id: format!("artifact-{}-subtitle-{video_artifact_index}", job.id),
                            job_id: job.id.clone(),
                            artifact_type: "subtitle".to_string(),
                            path: planned_path(
                                &job.output_dir,
                                &[&course_dir, &chapter_dir, &format!("{video_name}.srt")],
                            ),
                            status: "pending".to_string(),
                            size_bytes: None,
                            created_at: timestamp,
                            updated_at: timestamp,
                        },
                        source: ArtifactDownloadSource::Text(transcript_srt.clone()),
                    });
                    ensure_study_video(
                        &mut study_videos,
                        chapter_index,
                        video_artifact_index,
                        &chapter.title,
                        video.title.as_deref().unwrap_or(&video.slug),
                    )
                    .transcript_paragraphs = transcript_srt_to_paragraphs(transcript_srt);
                }
            }
            if job.download_quizzes {
                if let Some(quiz_markdown) = &video.quiz_markdown {
                    let quiz_file_name = format!("{video_name}.quiz.md");
                    text_downloads.push(PlannedArtifactDownload {
                        artifact: ArtifactRecord {
                            id: format!("artifact-{}-quiz-{video_artifact_index}", job.id),
                            job_id: job.id.clone(),
                            artifact_type: "quiz".to_string(),
                            path: planned_path(
                                &job.output_dir,
                                &[&course_dir, &chapter_dir, &quiz_file_name],
                            ),
                            status: "pending".to_string(),
                            size_bytes: None,
                            created_at: timestamp,
                            updated_at: timestamp,
                        },
                        source: ArtifactDownloadSource::Text(quiz_markdown.clone()),
                    });
                    ensure_study_video(
                        &mut study_videos,
                        chapter_index,
                        video_artifact_index,
                        &chapter.title,
                        video.title.as_deref().unwrap_or(&video.slug),
                    )
                    .quiz_markdown = Some(quiz_markdown.clone());
                }
            }
            video_artifact_index += 1;
        }
    }

    if job.download_quizzes {
        for (assessment_index, assessment) in course.assessments.iter().enumerate() {
            if let Some(quiz_markdown) = &assessment.quiz_markdown {
                let artifact_segments =
                    assessment_quiz_path_segments(course, assessment, assessment_index);
                let artifact_segment_refs = artifact_segments
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                text_downloads.push(PlannedArtifactDownload {
                    artifact: ArtifactRecord {
                        id: format!("artifact-{}-assessment-{assessment_index}", job.id),
                        job_id: job.id.clone(),
                        artifact_type: "quiz".to_string(),
                        path: planned_path(&job.output_dir, &artifact_segment_refs),
                        status: "pending".to_string(),
                        size_bytes: None,
                        created_at: timestamp,
                        updated_at: timestamp,
                    },
                    source: ArtifactDownloadSource::Text(quiz_markdown.clone()),
                });
                study_assessments.push(StudyGuideQuiz {
                    title: assessment.title.clone(),
                    markdown: quiz_markdown.clone(),
                });
            }
        }
    }

    if let Some(study_markdown) = format_study_guide(course, &study_videos, &study_assessments) {
        text_downloads.push(PlannedArtifactDownload {
            artifact: ArtifactRecord {
                id: format!("artifact-{}-study-guide", job.id),
                job_id: job.id.clone(),
                artifact_type: "study_guide".to_string(),
                path: planned_path(&job.output_dir, &[&course_dir, "Study.md"]),
                status: "pending".to_string(),
                size_bytes: None,
                created_at: timestamp,
                updated_at: timestamp,
            },
            source: ArtifactDownloadSource::Text(study_markdown),
        });
    }

    if job.download_exercises {
        for (exercise_index, exercise_file) in course.exercise_files.iter().enumerate() {
            let file_name = safe_file_name(&exercise_file.file_name);
            exercise_downloads.push(PlannedArtifactDownload {
                artifact: ArtifactRecord {
                    id: format!("artifact-{}-exercise-{exercise_index}", job.id),
                    job_id: job.id.clone(),
                    artifact_type: exercise_artifact_type(&file_name).to_string(),
                    path: planned_path(&job.output_dir, &[&course_dir, &file_name]),
                    status: "pending".to_string(),
                    size_bytes: None,
                    created_at: timestamp,
                    updated_at: timestamp,
                },
                source: ArtifactDownloadSource::Urls(exercise_download_urls(exercise_file)),
            });
        }
    }

    let mut downloads =
        Vec::with_capacity(text_downloads.len() + video_downloads.len() + exercise_downloads.len());
    downloads.extend(text_downloads);
    downloads.extend(video_downloads);
    downloads.extend(exercise_downloads);
    downloads
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StudyGuideVideo {
    chapter_index: usize,
    video_index: usize,
    chapter_title: String,
    video_title: String,
    transcript_paragraphs: Vec<String>,
    quiz_markdown: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StudyGuideQuiz {
    title: String,
    markdown: String,
}

fn ensure_study_video<'a>(
    videos: &'a mut Vec<StudyGuideVideo>,
    chapter_index: usize,
    video_index: usize,
    chapter_title: &str,
    video_title: &str,
) -> &'a mut StudyGuideVideo {
    if let Some(index) = videos
        .iter()
        .position(|video| video.chapter_index == chapter_index && video.video_index == video_index)
    {
        return &mut videos[index];
    }

    videos.push(StudyGuideVideo {
        chapter_index,
        video_index,
        chapter_title: chapter_title.to_string(),
        video_title: video_title.to_string(),
        transcript_paragraphs: Vec::new(),
        quiz_markdown: None,
    });
    videos.last_mut().expect("study video was just pushed")
}

fn assessment_quiz_path_segments(
    course: &Course,
    assessment: &CourseAssessment,
    assessment_index: usize,
) -> Vec<String> {
    let file_name = format!(
        "{:02} - {}.quiz.md",
        assessment_index + 1,
        safe_file_name(&assessment.title)
    );
    let chapter_dir = matching_assessment_chapter_dir(course, assessment);
    let mut artifact_segments = vec![safe_file_name(&course.title)];
    if let Some(chapter_dir) = chapter_dir {
        artifact_segments.push(chapter_dir);
    }
    artifact_segments.push(file_name);
    artifact_segments
}

fn matching_assessment_chapter_dir(
    course: &Course,
    assessment: &CourseAssessment,
) -> Option<String> {
    let assessment_title = assessment.title.to_ascii_lowercase();
    course
        .chapters
        .iter()
        .enumerate()
        .find(|(_, chapter)| {
            let chapter_title = chapter.title.to_ascii_lowercase();
            assessment_title == chapter_title
                || assessment_title.starts_with(&format!("{chapter_title} -"))
        })
        .map(|(index, chapter)| format!("{:02} - {}", index + 1, safe_file_name(&chapter.title)))
}

fn format_study_guide(
    course: &Course,
    videos: &[StudyGuideVideo],
    assessments: &[StudyGuideQuiz],
) -> Option<String> {
    let has_study_content = !assessments.is_empty()
        || videos
            .iter()
            .any(|video| !video.transcript_paragraphs.is_empty() || video.quiz_markdown.is_some());
    if !has_study_content {
        return None;
    }

    let mut markdown = String::new();
    markdown.push_str("# ");
    markdown.push_str(&course.title);
    markdown.push_str("\n\n");
    markdown.push_str("## Chapter Quizzes\n\n");

    let mut quiz_index = 1;
    for assessment in assessments {
        append_study_quiz(
            &mut markdown,
            quiz_index,
            &assessment.title,
            &assessment.markdown,
        );
        quiz_index += 1;
    }
    for video in ordered_study_videos(videos)
        .into_iter()
        .filter(|video| video.quiz_markdown.is_some())
    {
        append_study_quiz(
            &mut markdown,
            quiz_index,
            &video.video_title,
            video.quiz_markdown.as_deref().unwrap_or_default(),
        );
        quiz_index += 1;
    }
    if quiz_index == 1 {
        markdown.push_str("No quiz files were available for this course.\n\n");
    }

    markdown.push_str("## Transcripts\n\n");
    let ordered_videos = ordered_study_videos(videos);
    for (chapter_index, chapter) in course.chapters.iter().enumerate() {
        markdown.push_str("### ");
        markdown.push_str(&(chapter_index + 1).to_string());
        markdown.push_str(". ");
        markdown.push_str(&chapter.title);
        markdown.push_str("\n\n");

        let chapter_videos = ordered_videos
            .iter()
            .filter(|video| {
                video.chapter_index == chapter_index && !video.transcript_paragraphs.is_empty()
            })
            .collect::<Vec<_>>();
        if chapter_videos.is_empty() {
            markdown.push_str("No transcripts were available for this chapter.\n\n");
            continue;
        }

        for video in chapter_videos {
            markdown.push_str("#### ");
            markdown.push_str(&video.video_title);
            markdown.push_str("\n\n");
            for paragraph in &video.transcript_paragraphs {
                markdown.push_str(paragraph);
                markdown.push_str("\n\n");
            }
        }
    }

    Some(markdown)
}

fn ordered_study_videos(videos: &[StudyGuideVideo]) -> Vec<&StudyGuideVideo> {
    let mut ordered = videos.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|video| (video.chapter_index, video.video_index));
    ordered
}

fn append_study_quiz(markdown: &mut String, quiz_index: usize, title: &str, quiz_markdown: &str) {
    markdown.push_str("### ");
    markdown.push_str(&quiz_index.to_string());
    markdown.push_str(". ");
    markdown.push_str(title.trim());
    markdown.push_str("\n\n");
    let quiz_body = quiz_questions_section(quiz_markdown)
        .unwrap_or_else(|| quiz_markdown_without_title(quiz_markdown))
        .trim();
    if !quiz_body.is_empty() {
        markdown.push_str(quiz_body);
        markdown.push_str("\n\n");
    }
}

fn quiz_questions_section(markdown: &str) -> Option<&str> {
    markdown
        .find("## Questions")
        .or_else(|| markdown.find("## Extracted Questions"))
        .map(|index| &markdown[index..])
}

fn quiz_markdown_without_title(markdown: &str) -> &str {
    let trimmed = markdown.trim_start();
    if !trimmed.starts_with("# ") {
        return trimmed;
    }

    trimmed
        .find("\n\n")
        .map(|index| &trimmed[index + 2..])
        .unwrap_or_default()
}

fn transcript_srt_to_paragraphs(srt: &str) -> Vec<String> {
    let text = srt
        .lines()
        .filter_map(transcript_caption_line)
        .collect::<Vec<_>>()
        .join(" ");
    split_transcript_paragraphs(&normalize_transcript_spacing(&text))
}

fn transcript_caption_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.chars().all(|ch| ch.is_ascii_digit())
        || trimmed.contains("-->")
    {
        return None;
    }

    Some(trimmed.to_string())
}

fn normalize_transcript_spacing(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn split_transcript_paragraphs(text: &str) -> Vec<String> {
    const TARGET_PARAGRAPH_CHARS: usize = 560;
    const MAX_PARAGRAPH_CHARS: usize = 900;

    let mut paragraphs = Vec::new();
    let mut current = String::new();

    for sentence in split_transcript_sentences(text) {
        if current.is_empty() {
            current.push_str(&sentence);
            continue;
        }

        let projected_len = current.len() + 1 + sentence.len();
        if current.len() >= TARGET_PARAGRAPH_CHARS || projected_len > MAX_PARAGRAPH_CHARS {
            paragraphs.push(current);
            current = sentence;
        } else {
            current.push(' ');
            current.push_str(&sentence);
        }
    }

    if !current.trim().is_empty() {
        paragraphs.push(current);
    }

    paragraphs
}

fn split_transcript_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let chars = text.char_indices().collect::<Vec<_>>();

    for (position, (index, ch)) in chars.iter().enumerate() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }

        let next_is_boundary = chars
            .get(position + 1)
            .map(|(_, next)| next.is_whitespace() || matches!(next, '"' | '\'' | ')' | ']'))
            .unwrap_or(true);
        if !next_is_boundary {
            continue;
        }

        let end = index + ch.len_utf8();
        let sentence = text[start..end].trim();
        if !sentence.is_empty() {
            sentences.push(sentence.to_string());
        }
        start = end;
    }

    let remaining = text[start..].trim();
    if !remaining.is_empty() {
        sentences.push(remaining.to_string());
    }

    sentences
}

fn exercise_artifact_type(file_name: &str) -> &'static str {
    if file_name.to_ascii_lowercase().ends_with(".zip") {
        "exercise_zip"
    } else {
        "exercise_file"
    }
}

fn exercise_download_urls(exercise_file: &crate::course::ExerciseFile) -> Vec<String> {
    let mut urls = vec![exercise_file.download_url.clone()];
    for alternate in &exercise_file.alternate_download_urls {
        if !urls.iter().any(|url| url.eq_ignore_ascii_case(alternate)) {
            urls.push(alternate.clone());
        }
    }
    urls
}

fn planned_path(output_dir: &str, segments: &[&str]) -> String {
    let mut path = PathBuf::from(output_dir);
    for segment in segments {
        path.push(segment);
    }
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
    thumbnail_url: Option<&'a str>,
    chapters: Vec<CachedChapter<'a>>,
    assessments: Vec<CachedAssessment<'a>>,
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
    has_quiz: bool,
}

#[derive(Debug, Serialize)]
struct CachedExerciseFile<'a> {
    file_name: &'a str,
}

#[derive(Debug, Serialize)]
struct CachedAssessment<'a> {
    title: &'a str,
    entity_urn: Option<&'a str>,
    has_quiz: bool,
}

impl<'a> From<&'a Course> for CachedCoursePayload<'a> {
    fn from(course: &'a Course) -> Self {
        Self {
            slug: &course.slug,
            title: &course.title,
            thumbnail_url: course.thumbnail_url.as_deref(),
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
                            has_quiz: video.quiz_markdown.is_some(),
                        })
                        .collect(),
                })
                .collect(),
            assessments: course
                .assessments
                .iter()
                .map(|assessment| CachedAssessment {
                    title: &assessment.title,
                    entity_urn: assessment.entity_urn.as_deref(),
                    has_quiz: assessment.quiz_markdown.is_some(),
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
    use crate::course::{Chapter, CourseFetchError, CourseVideo, ExerciseFile};
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
        assert_eq!(processed.planned_artifacts, 4);
        assert_eq!(active_jobs.len(), 1);
        assert_eq!(cached.title.as_deref(), Some("Sample Course"));
        assert!(cached.payload_json.contains(r#""has_download_url":true"#));
        assert!(cached.payload_json.contains(r#""has_transcript":true"#));
        assert!(!cached.payload_json.contains("fresh"));
        assert!(!cached.payload_json.contains("progressiveUrl"));
        assert_eq!(artifacts.len(), 4);
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.artifact_type == "video"));
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.artifact_type == "subtitle"));
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.artifact_type == "study_guide"));
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
            vec!["job.active", "job.failed", "queue.guardrail.stop"]
        );
        assert_eq!(
            events[1].message,
            "Queue stopped: LinkedIn session expired. Refresh the saved session before resuming."
        );
        assert!(!events[2].message.contains("CSRF"));
        assert!(!events[2].message.contains("secret"));
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
                400,
                b"",
            ),
            ("https://cdn.example.test/exercise.zip", 404, b""),
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
            .join("01 - Getting started")
            .join("01 - Welcome video.mp4");
        let subtitle_path = output
            .path()
            .join("Sample Course")
            .join("01 - Getting started")
            .join("01 - Welcome video.srt");
        let study_path = output.path().join("Sample Course").join("Study.md");

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 3,
                failed: 1,
                cancelled: 0
            }
        );
        assert_eq!(completed_jobs.len(), 1);
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
        assert!(fs::read_to_string(&subtitle_path)
            .unwrap()
            .contains("Welcome."));
        let study = fs::read_to_string(study_path).unwrap();
        assert!(study.contains("# Sample Course"));
        assert!(study.contains("## Chapter Quizzes"));
        assert!(study.contains("## Transcripts"));
        assert!(study.contains("#### Welcome video"));
        assert!(study.contains("Welcome."));
        assert!(!study.contains("[Transcript]"));
        assert!(!study.contains("[Video]"));
        assert_eq!(artifacts.len(), 4);
        assert!(artifacts.iter().any(|artifact| artifact
            .path
            .ends_with("Sample Course\\01 - Getting started\\01 - Welcome video.mp4")
            || artifact
                .path
                .ends_with("Sample Course/01 - Getting started/01 - Welcome video.mp4")));
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
    fn artifact_guardrail_stops_queue_on_rate_limit_and_leaves_remaining_jobs_queued() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        let mut job = sample_job("job-1", "queued", 100);
        job.output_dir = output.path().to_string_lossy().to_string();
        insert_job(&connection, &job).unwrap();
        insert_job(&connection, &sample_job("job-2", "queued", 100)).unwrap();
        let mut course_client = FakeCourseApiClient::new(vec![
            ("fields=chapters,title,exerciseFiles", metadata_fixture()),
            (
                "https://www.linkedin.com/learning/sample-course",
                r#"https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"#,
            ),
            ("resolution=_1080", selected_video_fixture()),
        ]);
        let mut artifact_client =
            FakeArtifactClient::new(vec![("https://cdn.example.test/welcome.mp4", 429, b"")]);

        let error = process_next_queued_job_and_download_artifacts(
            &connection,
            &mut course_client,
            &mut artifact_client,
            &NeverCancelled,
            200,
        )
        .unwrap_err();
        let failed_jobs = list_jobs_by_status(&connection, "failed").unwrap();
        let queued_jobs = list_jobs_by_status(&connection, "queued").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert!(matches!(
            error,
            DownloadOrchestrationError::Artifact(ArtifactDownloadError::Http { status: 429, .. })
        ));
        assert_eq!(failed_jobs.len(), 1);
        assert_eq!(queued_jobs.len(), 1);
        assert_eq!(queued_jobs[0].id, "job-2");
        assert!(events
            .iter()
            .any(|event| event.event_type == "queue.guardrail.stop"
                && event.message.contains("rate limiting")));
        assert!(events
            .iter()
            .filter_map(|event| event.payload_json.as_deref())
            .all(|payload| !payload.contains("token=fresh") && !payload.contains("li_at")));
    }

    #[test]
    fn browser_quiz_assessments_are_merged_and_written_as_quiz_artifacts() {
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
                400,
                b"",
            ),
            ("https://cdn.example.test/exercise.zip", 404, b""),
        ]);

        let summary = process_next_queued_job_and_download_artifacts_with_quiz_assessments(
            &connection,
            &mut course_client,
            &mut artifact_client,
            &NeverCancelled,
            200,
            vec![CourseAssessment {
                title: "Chapter Quiz".to_string(),
                entity_urn: Some("urn:li:learningApiAssessment:1".to_string()),
                tracking_urn: Some("urn:li:lyndaAssessment:abc".to_string()),
                quiz_markdown: Some("# Chapter Quiz\n\n1. Question?\n   - Option\n".to_string()),
            }],
        )
        .unwrap()
        .unwrap();

        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let quiz_path = output
            .path()
            .join("Sample Course")
            .join("01 - Chapter Quiz.quiz.md");

        assert_eq!(summary.completed, 4);
        assert_eq!(summary.failed, 1);
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.artifact_type == "quiz"));
        assert!(fs::read_to_string(quiz_path).unwrap().contains("Question?"));
        let study =
            fs::read_to_string(output.path().join("Sample Course").join("Study.md")).unwrap();
        let quiz_index = study.find("## Chapter Quizzes").unwrap();
        let transcripts_index = study.find("## Transcripts").unwrap();
        assert!(quiz_index < transcripts_index);
        assert!(study.contains("Question?"));
        assert!(!study.contains("[Open quiz]"));
        assert!(!study.contains(".quiz.md"));
    }

    #[test]
    fn study_guide_is_planned_after_text_before_binary_artifacts_and_uses_course_order() {
        let job = sample_job("job-1", "queued", 100);
        let course = Course {
            slug: "sample-course".to_string(),
            title: "Sample Course".to_string(),
            thumbnail_url: None,
            chapters: vec![
                Chapter {
                    title: "First chapter".to_string(),
                    videos: vec![CourseVideo {
                        slug: "first-video".to_string(),
                        title: Some("First video".to_string()),
                        duration_seconds: Some(10),
                        download_url: Some("https://cdn.example.test/first.mp4".to_string()),
                        transcript_srt: Some(
                            "1\n00:00:00,000 --> 00:00:03,000\nFirst sentence.\n\n2\n00:00:03,000 --> 00:00:06,000\nSecond sentence.\n\n".to_string(),
                        ),
                        quiz_markdown: None,
                    }],
                },
                Chapter {
                    title: "Second chapter".to_string(),
                    videos: vec![CourseVideo {
                        slug: "second-video".to_string(),
                        title: Some("Second video".to_string()),
                        duration_seconds: Some(10),
                        download_url: Some("https://cdn.example.test/second.mp4".to_string()),
                        transcript_srt: Some(
                            "1\n00:00:00,000 --> 00:00:03,000\nThird sentence.\n\n2\n00:00:03,000 --> 00:00:06,000\nFourth sentence.\n\n".to_string(),
                        ),
                        quiz_markdown: Some(
                            "# Second Quiz\n\n## Questions\n\n1. Second question?\n".to_string(),
                        ),
                    }],
                },
            ],
            assessments: vec![CourseAssessment {
                title: "First chapter - Chapter Quiz".to_string(),
                entity_urn: Some("urn:li:learningApiAssessment:1".to_string()),
                tracking_urn: Some("urn:li:lyndaAssessment:first".to_string()),
                quiz_markdown: Some(
                    "# First Quiz\n\n## Questions\n\n1. First question?\n".to_string(),
                ),
            }],
            exercise_files: vec![ExerciseFile {
                file_name: "exercise.zip".to_string(),
                download_url: "https://cdn.example.test/exercise.zip".to_string(),
                alternate_download_urls: Vec::new(),
            }],
        };

        let downloads = build_initial_artifact_downloads(&job, &course, 200);
        let types = downloads
            .iter()
            .map(|download| download.artifact.artifact_type.as_str())
            .collect::<Vec<_>>();
        let study = downloads
            .iter()
            .find(|download| download.artifact.artifact_type == "study_guide")
            .unwrap();
        let study_markdown = match &study.source {
            ArtifactDownloadSource::Text(text) => text,
            _ => panic!("study guide must be a text artifact"),
        };

        assert_eq!(
            types,
            vec![
                "subtitle",
                "subtitle",
                "quiz",
                "quiz",
                "study_guide",
                "video",
                "video",
                "exercise_zip"
            ]
        );
        assert!(
            study.artifact.path.ends_with("Sample Course\\Study.md")
                || study.artifact.path.ends_with("Sample Course/Study.md")
        );
        assert!(
            study_markdown.find("First question?").unwrap()
                < study_markdown.find("Second question?").unwrap()
        );
        assert!(
            study_markdown.find("## Chapter Quizzes").unwrap()
                < study_markdown.find("## Transcripts").unwrap()
        );
        assert!(
            study_markdown.find("First chapter").unwrap()
                < study_markdown.find("Second chapter").unwrap()
        );
        assert!(study_markdown.contains("1. First question?"));
        assert!(!study_markdown.contains("[Open quiz]"));
        assert!(!study_markdown.contains(".quiz.md"));
        assert!(study_markdown.contains("#### First video\n\nFirst sentence. Second sentence."));
        assert!(study_markdown.contains("#### Second video\n\nThird sentence. Fourth sentence."));
        assert!(!study_markdown.contains("[Transcript]"));
        assert!(!study_markdown.contains(".srt"));
        assert!(!study_markdown.contains("[Video]"));
    }

    #[test]
    fn study_guide_is_not_planned_for_video_only_downloads() {
        let mut job = sample_job("job-1", "queued", 100);
        job.download_subtitles = false;
        job.download_quizzes = false;
        job.download_exercises = false;
        let course = Course {
            slug: "sample-course".to_string(),
            title: "Sample Course".to_string(),
            thumbnail_url: None,
            chapters: vec![Chapter {
                title: "Chapter".to_string(),
                videos: vec![CourseVideo {
                    slug: "welcome".to_string(),
                    title: Some("Welcome".to_string()),
                    duration_seconds: Some(10),
                    download_url: Some("https://cdn.example.test/welcome.mp4".to_string()),
                    transcript_srt: Some(
                        "1\n00:00:00,000 --> 00:00:10,000\nWelcome.\n\n".to_string(),
                    ),
                    quiz_markdown: Some("# Quiz\n\n## Questions\n\n1. Hidden?\n".to_string()),
                }],
            }],
            assessments: Vec::new(),
            exercise_files: Vec::new(),
        };

        let downloads = build_initial_artifact_downloads(&job, &course, 200);

        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].artifact.artifact_type, "video");
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
            source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
            status: status.to_string(),
            selected_quality: "1080".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            quiz_hints_json: "[]".to_string(),
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
