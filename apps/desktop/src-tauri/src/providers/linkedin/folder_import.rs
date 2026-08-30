use crate::app::database::{
    get_job, get_setting, insert_job, upsert_artifact, upsert_course_cache_entry,
    upsert_setting_json, ArtifactRecord, CourseCacheEntry, JobRecord,
};
use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriteError, DatabaseWriter};
use crate::app::safe_output_filesystem::{validate_output_root, ValidatedOutputRoot};
use regex::Regex;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct LinkedInOutputRoot {
    validated: ValidatedOutputRoot,
    display_path: String,
}

impl LinkedInOutputRoot {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == "./" {
            return Err("Choose a download folder before continuing.".to_string());
        }
        let path = PathBuf::from(trimmed);
        if !path.is_absolute() {
            return Err("LinkedIn download folders must be absolute paths.".to_string());
        }
        let validated = validate_output_root(path.as_path()).map_err(map_safe_output_error)?;
        let display_path = validated
            .path()
            .canonicalize()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .to_string();
        Ok(Self {
            validated,
            display_path,
        })
    }

    pub fn as_path(&self) -> &Path {
        self.validated.path()
    }

    pub fn display_path(&self) -> &str {
        &self.display_path
    }
}

#[derive(Debug, Clone)]
struct SanitizedCourseFolder {
    name: String,
}

#[derive(Debug, Clone)]
struct LocalCourseSlug {
    inner: String,
}

impl LocalCourseSlug {
    fn from_folder(folder: &SanitizedCourseFolder) -> Self {
        Self {
            inner: format!("local:{}", folder.name),
        }
    }

    fn as_str(&self) -> &str {
        &self.inner
    }
}

#[derive(Debug, Clone)]
struct RecoveredJobId {
    inner: String,
}

impl RecoveredJobId {
    fn from_canonical_course_dir(canonical: &Path) -> Self {
        let normalized = canonical.to_string_lossy().to_ascii_lowercase();
        let digest = Sha256::digest(normalized.as_bytes());
        let hex = digest
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<String>();
        Self {
            inner: format!("recovered-{}", hex.chars().take(32).collect::<String>()),
        }
    }

    fn as_str(&self) -> &str {
        &self.inner
    }
}

#[derive(Debug, Clone)]
enum DiscoveredKind {
    Video,
    Subtitle,
    Quiz,
    StudyGuide,
}

#[derive(Debug, Clone)]
struct DiscoveredArtifact {
    relative: PathBuf,
    kind: DiscoveredKind,
    size_bytes: Option<i64>,
}

#[derive(Debug, Clone)]
struct LinkedInCourseDir {
    folder: SanitizedCourseFolder,
    absolute: PathBuf,
    title: String,
    mtime_unix: i64,
    artifacts: Vec<DiscoveredArtifact>,
}

#[derive(Debug, Clone)]
enum ScanDecision {
    OutputRoot {
        root: LinkedInOutputRoot,
    },
    SingleCourse {
        parent: LinkedInOutputRoot,
        course: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportCounts {
    pub imported: usize,
    pub skipped: usize,
    pub already_known: usize,
}

pub fn commit(writer: &DatabaseWriter, raw_path: &str, now: i64) -> Result<(String, ImportCounts), String> {
    let decision = resolve_scan_decision(raw_path)?;
    let (root, courses, skipped) = scan_for_decision(&decision)?;
    let output_dir = root.display_path().to_string();
    writer
        .execute(write_context("commit_linkedin_destination"), move |connection| {
            merge_download_preferences(connection, &output_dir, now).map_err(import_error_as_db)?;
            let counts =
                import_courses(connection, &root, &courses, skipped, now, false).map_err(import_error_as_db)?;
            Ok((output_dir, counts))
        })
        .map_err(map_database_write_error)
}

pub fn recover_into(writer: &DatabaseWriter, output_dir: &str, now: i64) -> Result<ImportCounts, String> {
    let root = LinkedInOutputRoot::parse(output_dir)?;
    let (courses, skipped) = scan_output_root(&root)?;
    writer
        .execute(write_context("recover_linkedin_folder"), move |connection| {
            import_courses(connection, &root, &courses, skipped, now, false).map_err(import_error_as_db)
        })
        .map_err(map_database_write_error)
}

fn write_context(operation: &'static str) -> DatabaseWriteContext {
    DatabaseWriteContext {
        operation,
        provider: DatabaseProvider::Linkedin,
        workflow_id: None,
    }
}

fn map_safe_output_error(error: crate::app::safe_output_filesystem::SafeOutputError) -> String {
    error.to_string()
}

fn map_database_write_error(error: DatabaseWriteError) -> String {
    error.to_string()
}

fn import_error_as_db(error: String) -> DatabaseWriteError {
    DatabaseWriteError::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(1),
        Some(error),
    ))
}

fn resolve_scan_decision(raw_path: &str) -> Result<ScanDecision, String> {
    let path = PathBuf::from(raw_path.trim());
    if classify_course_dir(&path)?.is_some() {
        let parent = path
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_default();
        let parent = LinkedInOutputRoot::parse(&parent)?;
        return Ok(ScanDecision::SingleCourse {
            parent,
            course: path,
        });
    }
    Ok(ScanDecision::OutputRoot {
        root: LinkedInOutputRoot::parse(raw_path)?,
    })
}

fn scan_for_decision(
    decision: &ScanDecision,
) -> Result<(LinkedInOutputRoot, Vec<LinkedInCourseDir>, usize), String> {
    match decision {
        ScanDecision::OutputRoot { root } => {
            let (courses, skipped) = scan_output_root(root)?;
            Ok((root.clone(), courses, skipped))
        }
        ScanDecision::SingleCourse { parent, course } => {
            let course_dir = classify_course_dir(course)?
                .ok_or_else(|| "The selected folder does not look like a LinkedIn course.".to_string())?;
            Ok((parent.clone(), vec![course_dir], 0))
        }
    }
}

fn scan_output_root(root: &LinkedInOutputRoot) -> Result<(Vec<LinkedInCourseDir>, usize), String> {
    let mut courses = Vec::new();
    let mut skipped = 0;
    for entry in fs::read_dir(root.as_path()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
            continue;
        }
        if let Some(course) = classify_course_dir(&path)? {
            courses.push(course);
        } else {
            skipped += 1;
        }
    }
    Ok((courses, skipped))
}

fn import_courses(
    connection: &mut Connection,
    root: &LinkedInOutputRoot,
    courses: &[LinkedInCourseDir],
    skipped: usize,
    now: i64,
    update_preferences: bool,
) -> Result<ImportCounts, String> {
    let preferences_json = if update_preferences {
        merge_download_preferences(connection, root.display_path(), now)?
    } else {
        get_setting(connection, "download.preferences")
            .map_err(|error| error.to_string())?
            .map(|setting| setting.value_json)
            .unwrap_or_default()
    };
    let selected_quality = if preferences_json.trim().is_empty() {
        "720".to_string()
    } else {
        selected_quality_from_preferences_json(&preferences_json)
    };
    let mut imported = 0;
    let mut already_known = 0;

    connection
        .execute("BEGIN IMMEDIATE", [])
        .map_err(|error| error.to_string())?;
    let import_result = (|| {
        for course in courses {
            let job_id = RecoveredJobId::from_canonical_course_dir(&course.absolute);
            let slug = LocalCourseSlug::from_folder(&course.folder);
            if job_exists(connection, job_id.as_str(), root.display_path(), slug.as_str())? {
                already_known += 1;
                continue;
            }
            insert_recovered_course(
                connection,
                root,
                course,
                job_id.as_str(),
                slug.as_str(),
                &selected_quality,
                now,
            )?;
            imported += 1;
        }
        Ok(ImportCounts {
            imported,
            skipped,
            already_known,
        })
    })();
    if let Err(error) = import_result {
        let _ = connection.execute("ROLLBACK", []);
        return Err(error);
    }
    connection
        .execute("COMMIT", [])
        .map_err(|error| error.to_string())?;
    import_result
}

fn merge_download_preferences(
    connection: &mut Connection,
    output_dir: &str,
    now: i64,
) -> Result<String, String> {
    let existing = get_setting(connection, "download.preferences")
        .map_err(|error| error.to_string())?
        .map(|setting| setting.value_json);
    let merged = match existing {
        Some(json) => merge_download_preferences_json(&json, output_dir)?,
        None => default_preferences_json(output_dir),
    };
    upsert_setting_json(connection, "download.preferences", &merged, now)
        .map_err(|error| error.to_string())?;
    Ok(merged)
}

fn merge_download_preferences_json(existing_json: &str, output_dir: &str) -> Result<String, String> {
    let mut value = serde_json::from_str::<serde_json::Value>(existing_json)
        .unwrap_or_else(|_| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "outputDir".to_string(),
            serde_json::Value::String(output_dir.to_string()),
        );
    }
    serde_json::to_string(&value).map_err(|error| error.to_string())
}

fn default_preferences_json(output_dir: &str) -> String {
    serde_json::json!({
        "outputDir": output_dir,
        "selectedQuality": "720",
        "delaySeconds": 0,
        "videoWaitMinSeconds": 20,
        "videoWaitMaxSeconds": 40,
        "browserSource": "Chrome",
        "downloadVideos": true,
        "downloadExercises": true,
        "downloadSubtitles": true,
        "downloadQuizzes": true,
    })
    .to_string()
}

fn selected_quality_from_preferences_json(preferences_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(preferences_json)
        .ok()
        .and_then(|value| {
            value
                .get("selectedQuality")
                .and_then(|quality| quality.as_str())
                .map(|quality| quality.to_string())
        })
        .filter(|quality| !quality.trim().is_empty())
        .unwrap_or_else(|| "720".to_string())
}

fn job_exists(
    connection: &Connection,
    job_id: &str,
    output_dir: &str,
    course_slug: &str,
) -> Result<bool, String> {
    if get_job(connection, job_id)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(true);
    }
    let exists = connection
        .query_row(
            "SELECT 1 FROM jobs WHERE output_dir = ?1 AND course_slug = ?2 LIMIT 1",
            rusqlite::params![output_dir, course_slug],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .is_some();
    Ok(exists)
}

fn insert_recovered_course(
    connection: &Connection,
    root: &LinkedInOutputRoot,
    course: &LinkedInCourseDir,
    job_id: &str,
    course_slug: &str,
    selected_quality: &str,
    now: i64,
) -> Result<(), String> {
    let timestamp = course.mtime_unix.max(0);
    let job = JobRecord {
        id: job_id.to_string(),
        course_slug: course_slug.to_string(),
        source_url: String::new(),
        status: "completed".to_string(),
        selected_quality: selected_quality.to_string(),
        download_videos: true,
        download_exercises: true,
        download_subtitles: true,
        download_quizzes: true,
        quiz_hints_json: "[]".to_string(),
        output_dir: root.display_path().to_string(),
        paused: false,
        scheduled_at: None,
        created_at: timestamp,
        updated_at: now,
    };
    insert_job(connection, &job).map_err(|error| error.to_string())?;

    upsert_course_cache_entry(
        connection,
        &CourseCacheEntry {
            course_slug: course_slug.to_string(),
            source_url: String::new(),
            title: Some(course.title.clone()),
            payload_json: serde_json::json!({
                "slug": course_slug,
                "title": course.title,
            })
            .to_string(),
            fetched_at: now,
        },
    )
    .map_err(|error| error.to_string())?;

    for (index, artifact) in course.artifacts.iter().enumerate() {
        let artifact_path = root.as_path().join(&artifact.relative);
        let artifact_type = match artifact.kind {
            DiscoveredKind::Video => "video",
            DiscoveredKind::Subtitle => "subtitle",
            DiscoveredKind::Quiz => "quiz",
            DiscoveredKind::StudyGuide => "study_guide",
        };
        upsert_artifact(
            connection,
            &ArtifactRecord {
                id: format!("{job_id}-artifact-{index}"),
                job_id: job_id.to_string(),
                artifact_type: artifact_type.to_string(),
                path: artifact_path.to_string_lossy().to_string(),
                status: "completed".to_string(),
                size_bytes: artifact.size_bytes,
                created_at: timestamp,
                updated_at: now,
            },
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn classify_course_dir(path: &Path) -> Result<Option<LinkedInCourseDir>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(None),
    };
    if is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Ok(None);
    }

    let folder_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Course folder name is missing.".to_string())?;
    if folder_name.contains('/') || folder_name.contains('\\') {
        return Ok(None);
    }

    let study_md = path.join("Study.md");
    let has_study_md = fs::metadata(&study_md)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    let chapter_dirs = list_matching_chapter_dirs(path)?;
    let has_layout = has_study_md || chapter_dirs.iter().any(|chapter| chapter.has_media);
    if !has_layout {
        return Ok(None);
    }

    let title = if has_study_md {
        title_from_study_md(&study_md).unwrap_or_else(|| folder_name.to_string())
    } else {
        folder_name.to_string()
    };
    let mtime_unix = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let folder = SanitizedCourseFolder {
        name: folder_name.to_string(),
    };
    let artifacts = discover_artifacts(path, &folder.name, has_study_md, &chapter_dirs)?;
    if artifacts.is_empty() {
        return Ok(None);
    }

    Ok(Some(LinkedInCourseDir {
        folder,
        absolute: fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        title,
        mtime_unix,
        artifacts,
    }))
}

#[derive(Debug, Clone)]
struct ChapterDir {
    name: String,
    has_media: bool,
}

fn list_matching_chapter_dirs(course_path: &Path) -> Result<Vec<ChapterDir>, String> {
    let chapter_regex = chapter_dir_regex();
    let mut chapters = Vec::new();
    for entry in fs::read_dir(course_path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !chapter_regex.is_match(name) {
            continue;
        }
        let has_media = directory_has_media(&path)?;
        chapters.push(ChapterDir {
            name: name.to_string(),
            has_media,
        });
    }
    Ok(chapters)
}

fn directory_has_media(path: &Path) -> Result<bool, String> {
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_path = entry.path();
        let metadata = match fs::symlink_metadata(&file_path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
            continue;
        }
        let file_name = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if file_name.ends_with(".mp4")
            || file_name.ends_with(".srt")
            || file_name.ends_with(".quiz.md")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn discover_artifacts(
    course_path: &Path,
    folder_name: &str,
    has_study_md: bool,
    chapters: &[ChapterDir],
) -> Result<Vec<DiscoveredArtifact>, String> {
    let mut artifacts = Vec::new();
    if has_study_md {
        let study_path = course_path.join("Study.md");
        artifacts.push(DiscoveredArtifact {
            relative: PathBuf::from(folder_name).join("Study.md"),
            kind: DiscoveredKind::StudyGuide,
            size_bytes: file_size(&study_path),
        });
    }
    for chapter in chapters {
        if !chapter.has_media {
            continue;
        }
        let chapter_path = course_path.join(&chapter.name);
        for entry in fs::read_dir(&chapter_path).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_path = entry.path();
            let metadata = match fs::symlink_metadata(&file_path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
                continue;
            }
            let file_name = file_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let lower = file_name.to_ascii_lowercase();
            let kind = if lower.ends_with(".mp4") {
                DiscoveredKind::Video
            } else if lower.ends_with(".srt") {
                DiscoveredKind::Subtitle
            } else if lower.ends_with(".quiz.md") {
                DiscoveredKind::Quiz
            } else {
                continue;
            };
            artifacts.push(DiscoveredArtifact {
                relative: PathBuf::from(folder_name)
                    .join(&chapter.name)
                    .join(file_name),
                kind,
                size_bytes: file_size(&file_path),
            });
        }
    }
    Ok(artifacts)
}

fn file_size(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .map(|metadata| metadata.len() as i64)
}

fn title_from_study_md(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let title = trimmed.trim_start_matches('#').trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

fn chapter_dir_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\d{2} - .+").expect("chapter dir regex"))
}

fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{
        get_job, get_setting, initialize_database, list_artifacts_for_job, list_jobs_by_status, open_runtime,
    };
    use crate::providers::linkedin::commands::{bootstrap_jobs, history_source_url};
    use crate::workflow::application::runtime::WorkflowRuntime;

    fn import_harness() -> (tempfile::TempDir, DatabaseWriter, Connection, WorkflowRuntime) {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let writer = crate::app::database_writer::DatabaseWriter::start(
            db_path.clone(),
            crate::app::database_diagnostics::DatabaseDiagnostics::default(),
        )
        .unwrap();
        let runtime = WorkflowRuntime::new(writer.clone());
        let connection = open_runtime(&db_path).unwrap();
        (directory, writer, connection, runtime)
    }

    fn write_course_tree(root: &Path, title: &str, with_study_md: bool, with_video: bool) -> PathBuf {
        let course_dir = root.join(title);
        fs::create_dir_all(course_dir.join("01 - Intro")).unwrap();
        if with_study_md {
            fs::write(
                course_dir.join("Study.md"),
                format!("# {}\n", title),
            )
            .unwrap();
        }
        if with_video {
            fs::write(course_dir.join("01 - Intro/01 - Welcome.mp4"), b"video").unwrap();
        }
        course_dir
    }

    #[test]
    fn study_md_and_video_import_completed_job() {
        let (temp, writer, connection, _runtime) = import_harness();
        let root = temp.path().join("downloads");
        fs::create_dir_all(&root).unwrap();
        write_course_tree(&root, "Title", true, true);

        let (output_dir, counts) = commit(&writer, root.to_str().unwrap(), 1_700_000_000).unwrap();
        assert_eq!(counts.imported, 1);
        assert_eq!(counts.skipped, 0);
        assert_eq!(counts.already_known, 0);
        assert_eq!(output_dir, fs::canonicalize(&root).unwrap().to_string_lossy());

        let jobs = list_jobs_by_status(&connection, "completed").unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].course_slug, "local:Title");
        assert!(jobs[0].source_url.is_empty());
        assert_eq!(jobs[0].output_dir, output_dir);

        let job = get_job(&connection, &jobs[0].id).unwrap().unwrap();
        assert_eq!(job.status, "completed");

        let artifacts = list_artifacts_for_job(&connection, &job.id).unwrap();
        assert!(artifacts.iter().any(|artifact| artifact.artifact_type == "study_guide"));
        assert!(artifacts.iter().any(|artifact| artifact.artifact_type == "video"));
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.path.replace('\\', "/").contains("/Title/"))
        );

        let (output_dir_again, second_counts) =
            commit(&writer, root.to_str().unwrap(), 1_700_000_010).unwrap();
        assert_eq!(second_counts.imported, 0);
        assert_eq!(second_counts.already_known, 1);
        assert_eq!(output_dir_again, output_dir);
        assert_eq!(get_job(&connection, &job.id).unwrap().unwrap().id, job.id);
    }

    #[test]
    fn video_only_without_study_md_imports() {
        let (temp, writer, connection, _runtime) = import_harness();
        let root = temp.path().join("downloads");
        fs::create_dir_all(&root).unwrap();
        write_course_tree(&root, "Video Only", false, true);

        let (_, counts) = commit(&writer, root.to_str().unwrap(), 1_700_000_000).unwrap();
        assert_eq!(counts.imported, 1);
        let jobs = list_jobs_by_status(&connection, "completed").unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].course_slug, "local:Video Only");
    }

    #[test]
    fn sibling_without_layout_is_skipped() {
        let (temp, writer, connection, _runtime) = import_harness();
        let root = temp.path().join("downloads");
        fs::create_dir_all(&root).unwrap();
        write_course_tree(&root, "Title", true, true);
        fs::create_dir_all(root.join("Random Notes")).unwrap();
        fs::write(root.join("Random Notes/notes.txt"), b"notes").unwrap();

        let (_, counts) = commit(&writer, root.to_str().unwrap(), 1_700_000_000).unwrap();
        assert_eq!(counts.imported, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(list_jobs_by_status(&connection, "completed").unwrap().len(), 1);
    }

    #[test]
    fn commit_does_not_write_download_folder_or_youtube_prefs() {
        let (temp, writer, connection, _runtime) = import_harness();
        let root = temp.path().join("downloads");
        fs::create_dir_all(&root).unwrap();
        write_course_tree(&root, "Title", true, true);
        commit(&writer, root.to_str().unwrap(), 1_700_000_000).unwrap();

        assert!(get_setting(&connection, "download.folder").unwrap().is_none());
        assert!(get_setting(&connection, "youtube.preferences").unwrap().is_none());
    }

    #[test]
    fn commit_does_not_create_workflow_runs() {
        let (temp, writer, connection, runtime) = import_harness();
        let root = temp.path().join("downloads");
        fs::create_dir_all(&root).unwrap();
        write_course_tree(&root, "Title", true, true);
        commit(&writer, root.to_str().unwrap(), 1_700_000_000).unwrap();
        assert!(runtime.list_linkedin_runs(10).unwrap().is_empty());
        assert_eq!(list_jobs_by_status(&connection, "completed").unwrap().len(), 1);
    }

    #[test]
    fn bootstrap_jobs_includes_imported_completed_row() {
        let (temp, writer, connection, _runtime) = import_harness();
        let root = temp.path().join("downloads");
        fs::create_dir_all(&root).unwrap();
        write_course_tree(&root, "Title", true, true);
        commit(&writer, root.to_str().unwrap(), 1_700_000_000).unwrap();

        let bootstrapped = bootstrap_jobs(&connection).unwrap();
        assert!(
            bootstrapped
                .iter()
                .any(|job| job.status == "completed" && job.course_slug == "local:Title")
        );
    }

    #[test]
    fn history_source_url_for_local_slug_is_not_learning_url() {
        let url = history_source_url(&crate::cache::DownloadHistoryEntry {
            job_id: "recovered-test".to_string(),
            course_slug: "local:Title".to_string(),
            source_url: String::new(),
            course_title: "Title".to_string(),
            output_dir: "C:/downloads".to_string(),
            completed_at: 1,
        });
        assert!(!url.contains("linkedin.com/learning/local:"));
        assert!(url.is_empty());
    }

    #[test]
    fn history_source_url_for_learning_slug_without_url_uses_canonical_page() {
        let url = history_source_url(&crate::cache::DownloadHistoryEntry {
            job_id: "job-1".to_string(),
            course_slug: "sample-course".to_string(),
            source_url: String::new(),
            course_title: "Sample".to_string(),
            output_dir: "C:/downloads".to_string(),
            completed_at: 1,
        });
        assert_eq!(url, "https://www.linkedin.com/learning/sample-course");
    }
}
