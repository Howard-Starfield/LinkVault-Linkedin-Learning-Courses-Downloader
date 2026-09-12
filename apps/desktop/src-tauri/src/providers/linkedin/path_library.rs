//! LinkedIn path membership catalog. Jobs remain download attempts.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriteError, DatabaseWriter};
use crate::app::shell::open_folder_in_explorer;
use crate::cache::get_course_cache_entry;

use super::expansion::PathCapture;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathSlug(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CourseSlug(String);

impl PathSlug {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, PathLibraryError> {
        parse_slug(value.as_ref()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CourseSlug {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, PathLibraryError> {
        parse_slug(value.as_ref()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VideoSlug(String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedinMediaUrl(String);

impl VideoSlug {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, PathLibraryError> {
        parse_slug(value.as_ref()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl LinkedinMediaUrl {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub const COMPLETION_TAIL_MS: i64 = 3000;
pub const COMPLETION_TAIL_NUMERATOR: i64 = 2;
pub const COMPLETION_TAIL_DENOMINATOR: i64 = 100;
pub const COMPLETION_TAIL_FLOOR_MS: i64 = 500;

pub fn completion_threshold_ms(duration_ms: i64) -> i64 {
    if duration_ms <= 0 {
        return COMPLETION_TAIL_FLOOR_MS;
    }
    let percent =
        duration_ms.saturating_mul(COMPLETION_TAIL_NUMERATOR) / COMPLETION_TAIL_DENOMINATOR;
    COMPLETION_TAIL_MS
        .min(percent)
        .max(COMPLETION_TAIL_FLOOR_MS)
}

pub fn completed_at_for_tick(position_ms: i64, duration_ms: i64, now: i64) -> Option<i64> {
    if duration_ms <= 0 {
        return None;
    }
    let tail = completion_threshold_ms(duration_ms);
    if position_ms >= duration_ms.saturating_sub(tail) {
        Some(now)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatusChip {
    Queued,
    Downloading,
    Completed,
    Failed,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CatalogEntry {
    #[serde(rename = "path")]
    Path(PathCatalogEntry),
    #[serde(rename = "standalone")]
    Standalone(StandaloneCatalogEntry),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathCatalogEntry {
    pub path: PathSlug,
    pub title: String,
    pub source_url: String,
    pub completed_videos: u32,
    pub total_videos: u32,
    pub updated_at: i64,
    pub courses: Vec<PathCourseSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathCourseSummary {
    pub course: CourseSlug,
    pub title: String,
    pub thumbnail_url: Option<String>,
    pub completed_videos: u32,
    pub total_videos: u32,
    pub job_status: JobStatusChip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandaloneCatalogEntry {
    pub course: CourseSlug,
    pub title: String,
    pub source_url: String,
    pub thumbnail_url: Option<String>,
    pub completed_videos: u32,
    pub total_videos: u32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoursePlayback {
    pub course: CourseSlug,
    pub title: String,
    pub chapters: Vec<ChapterPlayback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterPlayback {
    pub title: String,
    pub videos: Vec<VideoPlayback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoPlayback {
    pub video: VideoSlug,
    pub title: String,
    pub duration_ms: i64,
    pub progress: VideoProgress,
    pub media_url: Option<LinkedinMediaUrl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProgress {
    pub position_ms: i64,
    pub duration_ms: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackTick {
    pub course: CourseSlug,
    pub video: VideoSlug,
    pub position_ms: i64,
    pub duration_ms: i64,
}

fn parse_slug(value: &str) -> Result<String, PathLibraryError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(PathLibraryError::InvalidSlug {
            value: value.to_string(),
        });
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Error)]
pub enum PathLibraryError {
    #[error("invalid LinkedIn slug '{value}'")]
    InvalidSlug { value: String },
    #[error("LinkedIn course '{slug}' is not in the catalog")]
    UnknownCourse { slug: String },
    #[error("could not open the LinkedIn course folder")]
    FolderUnavailable,
    #[error(transparent)]
    Database(#[from] DatabaseWriteError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Clone)]
pub struct PathLibrary {
    writer: DatabaseWriter,
}

impl PathLibrary {
    pub fn new(writer: DatabaseWriter) -> Self {
        Self { writer }
    }

    pub fn capture_path(&self, capture: PathCapture) -> Result<(), PathLibraryError> {
        let path = PathSlug::parse(&capture.path_slug)?;
        let mut members = Vec::with_capacity(capture.members.len());
        for (position, slug) in capture.members.iter().enumerate() {
            members.push((position as i64, CourseSlug::parse(slug)?));
        }
        let title = capture.title;
        let source_url = capture.source_url;
        let now = unix_timestamp();
        self.writer
            .execute(write_context("capture_path"), move |connection| {
                upsert_path(connection, &path, &title, &source_url, &members, now)?;
                Ok(())
            })?;
        Ok(())
    }

    pub fn capture_standalone(&self, course: CourseSlug) -> Result<(), PathLibraryError> {
        let now = unix_timestamp();
        self.writer
            .execute(write_context("capture_standalone"), move |connection| {
                connection.execute(
                    "INSERT INTO linkedin_standalone_courses (course_slug, created_at)
                     VALUES (?1, ?2)
                     ON CONFLICT(course_slug) DO NOTHING",
                    params![course.as_str(), now],
                )?;
                Ok(())
            })?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn record_video_file(
        &self,
        course: CourseSlug,
        video: VideoSlug,
        artifact_id: String,
        job_id: String,
    ) -> Result<(), PathLibraryError> {
        let now = unix_timestamp();
        self.writer
            .execute(write_context("record_video_file"), move |connection| {
                record_video_file_on_connection(
                    connection,
                    course.as_str(),
                    video.as_str(),
                    &artifact_id,
                    &job_id,
                    now,
                )?;
                Ok(())
            })?;
        Ok(())
    }

    pub fn list_catalog(&self, conn: &Connection) -> Result<Vec<CatalogEntry>, PathLibraryError> {
        list_catalog_rows(conn)
    }

    pub fn open_course(
        &self,
        conn: &Connection,
        course: CourseSlug,
    ) -> Result<CoursePlayback, PathLibraryError> {
        open_course_playback(conn, &course)
    }

    pub fn save_progress(&self, tick: PlaybackTick) -> Result<VideoProgress, PathLibraryError> {
        let now = unix_timestamp();
        let completed_at = completed_at_for_tick(tick.position_ms, tick.duration_ms, now);
        let course_slug = tick.course.as_str().to_string();
        let video_slug = tick.video.as_str().to_string();
        let position_ms = tick.position_ms;
        let duration_ms = tick.duration_ms;
        let progress = self.writer.execute(
            write_context("save_progress"),
            move |connection| {
                connection.execute(
                    "INSERT INTO linkedin_video_progress (
                        course_slug, video_slug, position_ms, duration_ms, completed_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(course_slug, video_slug) DO UPDATE SET
                        position_ms = excluded.position_ms,
                        duration_ms = excluded.duration_ms,
                        completed_at = COALESCE(linkedin_video_progress.completed_at, excluded.completed_at),
                        updated_at = excluded.updated_at",
                    params![
                        course_slug.as_str(),
                        video_slug.as_str(),
                        position_ms,
                        duration_ms,
                        completed_at,
                        now
                    ],
                )?;
                let row: (i64, i64, Option<i64>) = connection.query_row(
                    "SELECT position_ms, duration_ms, completed_at
                     FROM linkedin_video_progress
                     WHERE course_slug = ?1 AND video_slug = ?2",
                    params![course_slug.as_str(), video_slug.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                Ok(VideoProgress {
                    position_ms: row.0,
                    duration_ms: row.1,
                    completed_at: row.2,
                })
            },
        )?;
        Ok(progress)
    }

    pub fn open_course_folder(&self, course: CourseSlug) -> Result<(), PathLibraryError> {
        let output_dir: Option<String> =
            self.writer
                .execute(write_context("open_course_folder"), move |connection| {
                    let output_dir: Option<String> = connection
                        .query_row(
                            "SELECT output_dir FROM jobs
                         WHERE course_slug = ?1
                         ORDER BY updated_at DESC LIMIT 1",
                            params![course.as_str()],
                            |row| row.get(0),
                        )
                        .optional()?;
                    Ok(output_dir.filter(|value| !value.trim().is_empty()))
                })?;
        let Some(output_dir) = output_dir else {
            return Err(PathLibraryError::FolderUnavailable);
        };
        open_folder_in_explorer(std::path::Path::new(&output_dir))
            .map_err(|_| PathLibraryError::FolderUnavailable)
    }
}

fn write_context(operation: &'static str) -> DatabaseWriteContext {
    DatabaseWriteContext {
        operation,
        provider: DatabaseProvider::Linkedin,
        workflow_id: None,
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn upsert_path(
    connection: &mut Connection,
    path: &PathSlug,
    title: &str,
    source_url: &str,
    members: &[(i64, CourseSlug)],
    now: i64,
) -> Result<(), DatabaseWriteError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO linkedin_learning_paths (path_slug, title, source_url, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(path_slug) DO UPDATE SET
            title = excluded.title,
            source_url = excluded.source_url,
            updated_at = excluded.updated_at",
        params![path.as_str(), title, source_url, now],
    )?;
    for (position, course) in members {
        transaction.execute(
            "INSERT INTO linkedin_path_membership (path_slug, course_slug, position, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path_slug, course_slug) DO UPDATE SET
                position = excluded.position",
            params![path.as_str(), course.as_str(), position, now],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub(crate) fn record_video_file_on_connection(
    connection: &Connection,
    course_slug: &str,
    video_slug: &str,
    artifact_id: &str,
    job_id: &str,
    now: i64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO linkedin_video_files (
            course_slug, video_slug, artifact_id, job_id, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(course_slug, video_slug) DO UPDATE SET
            artifact_id = excluded.artifact_id,
            job_id = excluded.job_id,
            updated_at = excluded.updated_at",
        params![course_slug, video_slug, artifact_id, job_id, now],
    )?;
    Ok(())
}

fn list_catalog_rows(conn: &Connection) -> Result<Vec<CatalogEntry>, PathLibraryError> {
    let statuses = course_job_statuses(conn)?;
    let mut entries = Vec::new();
    let mut listed_courses = HashSet::new();

    let mut path_stmt = conn.prepare(
        "SELECT path_slug, title, source_url, updated_at
         FROM linkedin_learning_paths
         ORDER BY updated_at DESC, path_slug",
    )?;
    let paths = path_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for (path_slug, title, source_url, updated_at) in paths {
        let members = path_members(conn, &path_slug)?;
        let mut courses = Vec::new();
        let mut completed_videos = 0_u32;
        let mut total_videos = 0_u32;
        for (course_slug, _position) in members {
            listed_courses.insert(course_slug.clone());
            let summary = course_summary(conn, &course_slug, statuses.get(&course_slug).cloned())?;
            completed_videos += summary.completed_videos;
            total_videos += summary.total_videos;
            courses.push(summary);
        }
        entries.push(CatalogEntry::Path(PathCatalogEntry {
            path: PathSlug::parse(&path_slug)?,
            title,
            source_url,
            completed_videos,
            total_videos,
            updated_at,
            courses,
        }));
    }

    let mut standalone_stmt = conn.prepare(
        "SELECT course_slug, created_at
         FROM linkedin_standalone_courses
         ORDER BY created_at DESC, course_slug",
    )?;
    let standalones = standalone_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (course_slug, created_at) in standalones {
        listed_courses.insert(course_slug.clone());
        let summary = course_summary(conn, &course_slug, statuses.get(&course_slug).cloned())?;
        let source_url = course_source_url(conn, &course_slug)
            .unwrap_or_else(|| format!("https://www.linkedin.com/learning/{course_slug}"));
        entries.push(CatalogEntry::Standalone(StandaloneCatalogEntry {
            course: CourseSlug::parse(&course_slug)?,
            title: summary.title,
            source_url,
            thumbnail_url: summary.thumbnail_url,
            completed_videos: summary.completed_videos,
            total_videos: summary.total_videos,
            updated_at: created_at,
        }));
    }
    append_historical_standalone_courses(conn, &statuses, &listed_courses, &mut entries)?;
    Ok(entries)
}

fn append_historical_standalone_courses(
    conn: &Connection,
    statuses: &HashMap<String, JobStatusChip>,
    listed_courses: &HashSet<String>,
    entries: &mut Vec<CatalogEntry>,
) -> Result<(), PathLibraryError> {
    let mut job_stmt = conn.prepare(
        "SELECT course_slug, MAX(updated_at), MAX(source_url)
         FROM jobs
         WHERE status = 'completed'
         GROUP BY course_slug
         ORDER BY MAX(updated_at) DESC, course_slug",
    )?;
    let jobs = job_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (course_slug, updated_at, source_url) in jobs {
        if listed_courses.contains(&course_slug) {
            continue;
        }
        let summary = course_summary(conn, &course_slug, statuses.get(&course_slug).cloned())?;
        let source_url = if source_url.trim().is_empty() {
            course_source_url(conn, &course_slug)
                .unwrap_or_else(|| format!("https://www.linkedin.com/learning/{course_slug}"))
        } else {
            source_url
        };
        entries.push(CatalogEntry::Standalone(StandaloneCatalogEntry {
            course: CourseSlug::parse(&course_slug)?,
            title: summary.title,
            source_url,
            thumbnail_url: summary.thumbnail_url,
            completed_videos: summary.completed_videos,
            total_videos: summary.total_videos,
            updated_at,
        }));
    }
    Ok(())
}

fn path_members(conn: &Connection, path_slug: &str) -> Result<Vec<(String, i64)>, rusqlite::Error> {
    conn.prepare(
        "SELECT course_slug, position FROM linkedin_path_membership
         WHERE path_slug = ?1 ORDER BY position, course_slug",
    )?
    .query_map(params![path_slug], |row| Ok((row.get(0)?, row.get(1)?)))?
    .collect()
}

fn course_job_statuses(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, JobStatusChip>, rusqlite::Error> {
    let mut statuses = std::collections::HashMap::new();
    let mut job_stmt =
        conn.prepare("SELECT course_slug, status, updated_at FROM jobs ORDER BY updated_at DESC")?;
    let jobs = job_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (slug, status, _updated_at) in jobs {
        statuses
            .entry(slug)
            .or_insert_with(|| job_status_chip(&status));
    }

    let mut run_stmt = conn.prepare(
        "SELECT request_json, state, updated_at FROM workflow_runs
         WHERE provider = 'linkedin' ORDER BY updated_at DESC",
    )?;
    let runs = run_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (request_json, state, _updated_at) in runs {
        if let Ok(request) =
            serde_json::from_str::<super::projection::LinkedInWorkflowRequest>(&request_json)
        {
            statuses
                .entry(request.course_slug)
                .or_insert_with(|| workflow_status_chip(&state));
        }
    }
    Ok(statuses)
}

fn job_status_chip(status: &str) -> JobStatusChip {
    match status {
        "queued" => JobStatusChip::Queued,
        "active" => JobStatusChip::Downloading,
        "completed" => JobStatusChip::Completed,
        "failed" | "cancelled" => JobStatusChip::Failed,
        _ => JobStatusChip::Absent,
    }
}

fn workflow_status_chip(state: &str) -> JobStatusChip {
    match state {
        "queued" | "paused" | "retry_wait" => JobStatusChip::Queued,
        "running" | "cancelling" => JobStatusChip::Downloading,
        "succeeded" | "succeeded_with_warnings" => JobStatusChip::Completed,
        "failed" | "cancelled" => JobStatusChip::Failed,
        _ => JobStatusChip::Absent,
    }
}

fn course_summary(
    conn: &Connection,
    course_slug: &str,
    job_status: Option<JobStatusChip>,
) -> Result<PathCourseSummary, PathLibraryError> {
    let cache = get_course_cache_entry(conn, course_slug).map_err(PathLibraryError::from_cache)?;
    let parsed = cache
        .as_ref()
        .and_then(|entry| parse_cached_course(&entry.payload_json));
    let title = cache
        .as_ref()
        .and_then(|entry| entry.title.clone())
        .or_else(|| parsed.as_ref().and_then(|course| course.title.clone()))
        .unwrap_or_else(|| course_slug.to_string());
    let thumbnail_url = parsed
        .as_ref()
        .and_then(|course| course.thumbnail_url.clone());
    let video_slugs = parsed.as_ref().map(cached_video_slugs).unwrap_or_default();
    let total_videos = video_slugs.len() as u32;
    let completed_videos = count_completed_videos(conn, course_slug, &video_slugs)?;
    Ok(PathCourseSummary {
        course: CourseSlug::parse(course_slug)?,
        title,
        thumbnail_url,
        completed_videos,
        total_videos,
        job_status: job_status.unwrap_or(JobStatusChip::Absent),
    })
}

fn course_is_known(conn: &Connection, course_slug: &str) -> Result<bool, rusqlite::Error> {
    let standalone: i64 = conn.query_row(
        "SELECT COUNT(*) FROM linkedin_standalone_courses WHERE course_slug = ?1",
        params![course_slug],
        |row| row.get(0),
    )?;
    if standalone > 0 {
        return Ok(true);
    }
    let member: i64 = conn.query_row(
        "SELECT COUNT(*) FROM linkedin_path_membership WHERE course_slug = ?1",
        params![course_slug],
        |row| row.get(0),
    )?;
    Ok(member > 0)
}

fn course_source_url(conn: &Connection, course_slug: &str) -> Option<String> {
    conn.query_row(
        "SELECT source_url FROM course_cache WHERE course_slug = ?1",
        params![course_slug],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .filter(|value| !value.trim().is_empty())
}

fn count_completed_videos(
    conn: &Connection,
    course_slug: &str,
    video_slugs: &[String],
) -> Result<u32, rusqlite::Error> {
    if video_slugs.is_empty() {
        return Ok(0);
    }
    let completed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM linkedin_video_progress
         WHERE course_slug = ?1 AND completed_at IS NOT NULL",
        params![course_slug],
        |row| row.get(0),
    )?;
    Ok(completed.min(video_slugs.len() as i64) as u32)
}

fn cached_video_slugs(course: &CachedCourse) -> Vec<String> {
    course
        .chapters
        .iter()
        .flat_map(|chapter| chapter.videos.iter().map(|video| video.slug.clone()))
        .collect()
}

fn open_course_playback(
    conn: &Connection,
    course: &CourseSlug,
) -> Result<CoursePlayback, PathLibraryError> {
    let cache =
        get_course_cache_entry(conn, course.as_str()).map_err(PathLibraryError::from_cache)?;
    if cache.is_none() && !course_is_known(conn, course.as_str())? {
        return Err(PathLibraryError::UnknownCourse {
            slug: course.as_str().to_string(),
        });
    }
    let parsed = cache
        .as_ref()
        .and_then(|entry| parse_cached_course(&entry.payload_json))
        .unwrap_or_default();
    let title = cache
        .as_ref()
        .and_then(|entry| entry.title.clone())
        .or(parsed.title.clone())
        .unwrap_or_else(|| course.as_str().to_string());
    let fallback_artifact_ids = load_fallback_video_artifact_ids(conn, course.as_str())?;
    let mut video_index = 0_usize;
    let mut chapters = Vec::new();
    for chapter in parsed.chapters {
        let mut videos = Vec::new();
        for video in chapter.videos {
            let video_slug = VideoSlug::parse(&video.slug)?;
            let progress = load_progress(conn, course.as_str(), video_slug.as_str())?;
            let artifact_id = load_video_artifact_id(conn, course.as_str(), video_slug.as_str())?
                .or_else(|| fallback_artifact_ids.get(video_index).cloned());
            video_index += 1;
            let duration_ms = video
                .duration_seconds
                .map(|seconds| (seconds as i64).saturating_mul(1000))
                .unwrap_or(progress.duration_ms);
            videos.push(VideoPlayback {
                video: video_slug,
                title: video.title.unwrap_or_else(|| video.slug.clone()),
                duration_ms,
                progress,
                media_url: artifact_id.map(mint_media_url),
            });
        }
        chapters.push(ChapterPlayback {
            title: chapter.title,
            videos,
        });
    }
    Ok(CoursePlayback {
        course: course.clone(),
        title,
        chapters,
    })
}

fn load_progress(
    conn: &Connection,
    course_slug: &str,
    video_slug: &str,
) -> Result<VideoProgress, rusqlite::Error> {
    let row = conn
        .query_row(
            "SELECT position_ms, duration_ms, completed_at
             FROM linkedin_video_progress
             WHERE course_slug = ?1 AND video_slug = ?2",
            params![course_slug, video_slug],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        Some((position_ms, duration_ms, completed_at)) => VideoProgress {
            position_ms,
            duration_ms,
            completed_at,
        },
        None => VideoProgress {
            position_ms: 0,
            duration_ms: 0,
            completed_at: None,
        },
    })
}

fn load_video_artifact_id(
    conn: &Connection,
    course_slug: &str,
    video_slug: &str,
) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT artifact_id FROM linkedin_video_files
         WHERE course_slug = ?1 AND video_slug = ?2",
        params![course_slug, video_slug],
        |row| row.get(0),
    )
    .optional()
}

fn load_fallback_video_artifact_ids(
    conn: &Connection,
    course_slug: &str,
) -> Result<Vec<String>, rusqlite::Error> {
    let job_id: Option<String> = conn
        .query_row(
            "SELECT id FROM jobs
             WHERE course_slug = ?1 AND status = 'completed'
             ORDER BY updated_at DESC LIMIT 1",
            params![course_slug],
            |row| row.get(0),
        )
        .optional()?;
    let Some(job_id) = job_id else {
        return Ok(Vec::new());
    };
    conn.prepare(
        "SELECT id FROM artifacts
         WHERE job_id = ?1 AND artifact_type = 'video' AND status = 'completed'
         ORDER BY created_at, id",
    )?
    .query_map(params![job_id], |row| row.get(0))?
    .collect()
}

fn mint_media_url(artifact_id: String) -> LinkedinMediaUrl {
    LinkedinMediaUrl(format!("linkedin-media://v/{artifact_id}"))
}

#[derive(Debug, Default, Deserialize)]
struct CachedCourse {
    title: Option<String>,
    thumbnail_url: Option<String>,
    #[serde(default)]
    chapters: Vec<CachedChapter>,
}

#[derive(Debug, Deserialize)]
struct CachedChapter {
    title: String,
    #[serde(default)]
    videos: Vec<CachedVideo>,
}

#[derive(Debug, Deserialize)]
struct CachedVideo {
    slug: String,
    title: Option<String>,
    duration_seconds: Option<u64>,
}

fn parse_cached_course(payload_json: &str) -> Option<CachedCourse> {
    serde_json::from_str(payload_json).ok()
}

impl PathLibraryError {
    fn from_cache(error: crate::cache::CacheError) -> Self {
        match error {
            crate::cache::CacheError::Sql(error) => Self::Sqlite(error),
            other => Self::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(other.to_string()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::initialize_database;
    use crate::app::database_diagnostics::DatabaseDiagnostics;
    use tempfile::TempDir;

    fn harness() -> (TempDir, PathLibrary, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let writer =
            DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default()).unwrap();
        (directory, PathLibrary::new(writer), db_path)
    }

    fn path_capture(slug: &str, title: &str, members: &[&str]) -> PathCapture {
        PathCapture {
            path_slug: slug.to_string(),
            title: title.to_string(),
            source_url: format!("https://www.linkedin.com/learning/paths/{slug}"),
            members: members.iter().map(|member| (*member).to_string()).collect(),
        }
    }

    fn open_reader(db_path: &std::path::Path) -> Connection {
        crate::cache::open_runtime(db_path).unwrap()
    }

    #[test]
    fn capture_path_twice_is_one_path_row_and_stable_membership() {
        let (_directory, library, db_path) = harness();
        let capture = path_capture(
            "github-cert",
            "GitHub Certificate",
            &["practical-github-actions", "practical-github-copilot"],
        );
        library.capture_path(capture.clone()).unwrap();
        library.capture_path(capture).unwrap();

        let connection = open_reader(&db_path);
        let path_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM linkedin_learning_paths", [], |row| {
                row.get(0)
            })
            .unwrap();
        let members: Vec<(String, i64)> = connection
            .prepare(
                "SELECT course_slug, position FROM linkedin_path_membership
                 WHERE path_slug = 'github-cert' ORDER BY position",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(path_count, 1);
        assert_eq!(
            members,
            vec![
                ("practical-github-actions".to_string(), 0),
                ("practical-github-copilot".to_string(), 1),
            ]
        );
    }

    #[test]
    fn same_course_slug_can_belong_to_two_paths() {
        let (_directory, library, db_path) = harness();
        library
            .capture_path(path_capture(
                "path-a",
                "Path A",
                &["shared-course", "only-a"],
            ))
            .unwrap();
        library
            .capture_path(path_capture(
                "path-b",
                "Path B",
                &["shared-course", "only-b"],
            ))
            .unwrap();

        let connection = open_reader(&db_path);
        let memberships: Vec<String> = connection
            .prepare(
                "SELECT path_slug FROM linkedin_path_membership
                 WHERE course_slug = 'shared-course' ORDER BY path_slug",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            memberships,
            vec!["path-a".to_string(), "path-b".to_string()]
        );
        let path_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM linkedin_learning_paths", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(path_count, 2);
    }

    #[test]
    fn capture_standalone_plus_membership_records_both() {
        let (_directory, library, db_path) = harness();
        library
            .capture_path(path_capture(
                "github-cert",
                "GitHub Certificate",
                &["practical-github-actions"],
            ))
            .unwrap();
        library
            .capture_standalone(CourseSlug::parse("practical-github-actions").unwrap())
            .unwrap();
        library
            .capture_standalone(CourseSlug::parse("practical-github-actions").unwrap())
            .unwrap();

        let connection = open_reader(&db_path);
        let standalone: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM linkedin_standalone_courses WHERE course_slug = 'practical-github-actions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let membership: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM linkedin_path_membership
                 WHERE path_slug = 'github-cert' AND course_slug = 'practical-github-actions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(standalone, 1);
        assert_eq!(membership, 1);
    }

    fn seed_course_cache(
        connection: &Connection,
        slug: &str,
        title: &str,
        videos: &[(&str, &str)],
    ) {
        let chapters = serde_json::json!([{
            "title": "Chapter 1",
            "videos": videos.iter().map(|(video_slug, video_title)| serde_json::json!({
                "slug": video_slug,
                "title": video_title,
                "duration_seconds": 60
            })).collect::<Vec<_>>()
        }]);
        connection
            .execute(
                "INSERT INTO course_cache (course_slug, source_url, title, payload_json, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, 10)",
                params![
                    slug,
                    format!("https://www.linkedin.com/learning/{slug}"),
                    title,
                    serde_json::json!({
                        "slug": slug,
                        "title": title,
                        "chapters": chapters
                    })
                    .to_string()
                ],
            )
            .unwrap();
    }

    #[test]
    fn list_catalog_omits_filesystem_paths() {
        let (_directory, library, db_path) = harness();
        library
            .capture_path(path_capture(
                "github-cert",
                "GitHub Certificate",
                &["practical-github-actions"],
            ))
            .unwrap();
        let connection = open_reader(&db_path);
        seed_course_cache(
            &connection,
            "practical-github-actions",
            "Practical GitHub Actions",
            &[("welcome", "Welcome")],
        );
        connection
            .execute(
                "INSERT INTO jobs (
                    id, course_slug, source_url, status, selected_quality,
                    download_videos, download_exercises, download_subtitles, download_quizzes,
                    quiz_hints_json, output_dir, paused, created_at, updated_at
                 ) VALUES ('job-1', 'practical-github-actions', 'https://www.linkedin.com/learning/practical-github-actions',
                    'completed', '720', 1, 1, 1, 1, '[]', 'C:/secret-downloads', 0, 10, 10)",
                [],
            )
            .unwrap();
        let catalog = library.list_catalog(&connection).unwrap();
        let json = serde_json::to_string(&catalog).unwrap();
        assert!(!json.contains("C:/secret-downloads"));
        assert!(!json.contains("output_dir"));
        assert!(!json.contains("outputDir"));
        assert!(!json.contains("job-1"));
        match &catalog[0] {
            CatalogEntry::Path(path) => {
                assert_eq!(path.courses[0].job_status, JobStatusChip::Completed);
                assert_eq!(path.courses[0].total_videos, 1);
            }
            CatalogEntry::Standalone(_) => panic!("expected path entry"),
        }
    }

    #[test]
    fn save_progress_near_end_is_sticky_after_rewind() {
        let (_directory, library, db_path) = harness();
        let course = CourseSlug::parse("practical-github-actions").unwrap();
        let video = VideoSlug::parse("welcome").unwrap();
        let completed = library
            .save_progress(PlaybackTick {
                course: course.clone(),
                video: video.clone(),
                position_ms: 59_000,
                duration_ms: 60_000,
            })
            .unwrap();
        assert!(completed.completed_at.is_some());
        let rewound = library
            .save_progress(PlaybackTick {
                course,
                video,
                position_ms: 0,
                duration_ms: 60_000,
            })
            .unwrap();
        assert_eq!(rewound.completed_at, completed.completed_at);
        assert_eq!(rewound.position_ms, 0);
        let connection = open_reader(&db_path);
        let stored: Option<i64> = connection
            .query_row(
                "SELECT completed_at FROM linkedin_video_progress
                 WHERE course_slug = 'practical-github-actions' AND video_slug = 'welcome'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, completed.completed_at);
    }

    #[test]
    fn redownload_updates_files_and_leaves_progress() {
        let (_directory, library, db_path) = harness();
        let course = CourseSlug::parse("practical-github-actions").unwrap();
        let video = VideoSlug::parse("welcome").unwrap();
        library
            .save_progress(PlaybackTick {
                course: course.clone(),
                video: video.clone(),
                position_ms: 59_000,
                duration_ms: 60_000,
            })
            .unwrap();
        library
            .record_video_file(
                course.clone(),
                video.clone(),
                "artifact-old".to_string(),
                "job-old".to_string(),
            )
            .unwrap();
        library
            .record_video_file(
                course.clone(),
                video.clone(),
                "artifact-new".to_string(),
                "job-new".to_string(),
            )
            .unwrap();
        let connection = open_reader(&db_path);
        let artifact_id: String = connection
            .query_row(
                "SELECT artifact_id FROM linkedin_video_files
                 WHERE course_slug = 'practical-github-actions' AND video_slug = 'welcome'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let completed: Option<i64> = connection
            .query_row(
                "SELECT completed_at FROM linkedin_video_progress
                 WHERE course_slug = 'practical-github-actions' AND video_slug = 'welcome'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(artifact_id, "artifact-new");
        assert!(completed.is_some());
        seed_course_cache(
            &connection,
            "practical-github-actions",
            "Practical GitHub Actions",
            &[("welcome", "Welcome")],
        );
        let playback = library.open_course(&connection, course).unwrap();
        assert_eq!(
            playback.chapters[0].videos[0]
                .media_url
                .as_ref()
                .map(LinkedinMediaUrl::as_str),
            Some("linkedin-media://v/artifact-new")
        );
    }

    #[test]
    fn open_course_mints_artifact_media_urls() {
        let (_directory, library, db_path) = harness();
        let connection = open_reader(&db_path);
        seed_course_cache(
            &connection,
            "practical-github-actions",
            "Practical GitHub Actions",
            &[("welcome", "Welcome")],
        );
        library
            .record_video_file(
                CourseSlug::parse("practical-github-actions").unwrap(),
                VideoSlug::parse("welcome").unwrap(),
                "artifact-welcome".to_string(),
                "job-1".to_string(),
            )
            .unwrap();
        let playback = library
            .open_course(
                &connection,
                CourseSlug::parse("practical-github-actions").unwrap(),
            )
            .unwrap();
        assert_eq!(
            playback.chapters[0].videos[0]
                .media_url
                .as_ref()
                .map(LinkedinMediaUrl::as_str),
            Some("linkedin-media://v/artifact-welcome")
        );
    }

    fn seed_completed_job_with_video(
        connection: &Connection,
        job_id: &str,
        course_slug: &str,
        artifact_id: &str,
    ) {
        connection
            .execute(
                "INSERT INTO jobs (
                    id, course_slug, source_url, status, selected_quality,
                    download_videos, download_exercises, download_subtitles, download_quizzes,
                    quiz_hints_json, output_dir, paused, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'completed', '720', 1, 1, 1, 1, '[]', 'C:/secret-downloads', 0, 10, 10)",
                params![
                    job_id,
                    course_slug,
                    format!("https://www.linkedin.com/learning/{course_slug}")
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO artifacts (
                    id, job_id, artifact_type, path, status, size_bytes, created_at, updated_at
                 ) VALUES (?1, ?2, 'video', 'C:/secret-downloads/welcome.mp4', 'completed', 24, 10, 10)",
                params![artifact_id, job_id],
            )
            .unwrap();
    }

    #[test]
    fn list_catalog_includes_completed_jobs_without_membership() {
        let (_directory, library, db_path) = harness();
        let connection = open_reader(&db_path);
        seed_course_cache(
            &connection,
            "excel-essential-training",
            "Excel Essential Training",
            &[("welcome", "Welcome")],
        );
        seed_completed_job_with_video(
            &connection,
            "job-legacy",
            "excel-essential-training",
            "artifact-excel",
        );
        let catalog = library.list_catalog(&connection).unwrap();
        match &catalog[..] {
            [CatalogEntry::Standalone(course)] => {
                assert_eq!(course.course.as_str(), "excel-essential-training");
                assert_eq!(course.title, "Excel Essential Training");
            }
            other => panic!("expected one standalone card, got {other:?}"),
        }
    }

    #[test]
    fn open_course_falls_back_to_completed_video_artifacts() {
        let (_directory, library, db_path) = harness();
        let connection = open_reader(&db_path);
        seed_course_cache(
            &connection,
            "excel-essential-training",
            "Excel Essential Training",
            &[("welcome", "Welcome")],
        );
        seed_completed_job_with_video(
            &connection,
            "job-legacy",
            "excel-essential-training",
            "artifact-excel",
        );
        let playback = library
            .open_course(
                &connection,
                CourseSlug::parse("excel-essential-training").unwrap(),
            )
            .unwrap();
        assert_eq!(
            playback.chapters[0].videos[0]
                .media_url
                .as_ref()
                .map(LinkedinMediaUrl::as_str),
            Some("linkedin-media://v/artifact-excel")
        );
        let json = serde_json::to_string(&playback).unwrap();
        assert!(!json.contains("C:/secret-downloads"));
    }
}
