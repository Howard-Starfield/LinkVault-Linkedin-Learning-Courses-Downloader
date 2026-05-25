use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;
use std::path::Path;
use thiserror::Error;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS course_cache (
    course_slug TEXT PRIMARY KEY NOT NULL,
    source_url TEXT NOT NULL,
    title TEXT,
    payload_json TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY NOT NULL,
    course_slug TEXT NOT NULL,
    source_url TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN ('queued', 'active', 'completed', 'failed', 'cancelled')),
    selected_quality TEXT NOT NULL,
    download_videos INTEGER NOT NULL,
    download_exercises INTEGER NOT NULL,
    download_subtitles INTEGER NOT NULL,
    download_quizzes INTEGER NOT NULL DEFAULT 1,
    quiz_hints_json TEXT NOT NULL DEFAULT '[]',
    output_dir TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS job_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    message TEXT NOT NULL,
    payload_json TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL,
    artifact_type TEXT NOT NULL CHECK (artifact_type IN ('video', 'subtitle', 'quiz', 'study_guide', 'exercise_zip', 'exercise_file')),
    path TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'completed', 'failed', 'cancelled', 'skipped')),
    size_bytes INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE CASCADE
);
"#;

pub fn open_or_initialize(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path)?;
    initialize(&connection)?;
    Ok(connection)
}

pub fn initialize(connection: &Connection) -> Result<()> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.execute_batch(SCHEMA)?;
    migrate_jobs_download_quizzes(connection)?;
    migrate_jobs_source_url(connection)?;
    migrate_jobs_quiz_hints(connection)?;
    migrate_artifacts_known_types(connection)?;
    Ok(())
}

fn migrate_jobs_download_quizzes(connection: &Connection) -> Result<()> {
    let has_column = connection
        .prepare("PRAGMA table_info(jobs)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "download_quizzes");

    if !has_column {
        connection.execute(
            "ALTER TABLE jobs ADD COLUMN download_quizzes INTEGER NOT NULL DEFAULT 1",
            [],
        )?;
    }

    Ok(())
}

fn migrate_jobs_source_url(connection: &Connection) -> Result<()> {
    if !table_has_column(connection, "jobs", "source_url")? {
        connection.execute(
            "ALTER TABLE jobs ADD COLUMN source_url TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }

    Ok(())
}

fn migrate_jobs_quiz_hints(connection: &Connection) -> Result<()> {
    if !table_has_column(connection, "jobs", "quiz_hints_json")? {
        connection.execute(
            "ALTER TABLE jobs ADD COLUMN quiz_hints_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }

    Ok(())
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(connection
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?
        .iter()
        .any(|name| name == column))
}

fn migrate_artifacts_known_types(connection: &Connection) -> Result<()> {
    let create_sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'artifacts'",
        [],
        |row| row.get(0),
    )?;
    if create_sql.contains("'quiz'") && create_sql.contains("'study_guide'") {
        return Ok(());
    }

    connection.execute_batch(
        r#"
        PRAGMA foreign_keys = OFF;

        CREATE TABLE artifacts_new (
            id TEXT PRIMARY KEY NOT NULL,
            job_id TEXT NOT NULL,
            artifact_type TEXT NOT NULL CHECK (artifact_type IN ('video', 'subtitle', 'quiz', 'study_guide', 'exercise_zip', 'exercise_file')),
            path TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('pending', 'active', 'completed', 'failed', 'cancelled', 'skipped')),
            size_bytes INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE CASCADE
        );

        INSERT INTO artifacts_new (
            id,
            job_id,
            artifact_type,
            path,
            status,
            size_bytes,
            created_at,
            updated_at
        )
        SELECT
            id,
            job_id,
            artifact_type,
            path,
            status,
            size_bytes,
            created_at,
            updated_at
        FROM artifacts;

        DROP TABLE artifacts;
        ALTER TABLE artifacts_new RENAME TO artifacts;

        PRAGMA foreign_keys = ON;
        "#,
    )?;

    Ok(())
}

#[derive(Debug, Error)]
pub enum CacheError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("setting key may contain secret material and must not be stored in SQLite: {0}")]
    SecretSettingKey(String),
    #[error("invalid job transition from {from} to {to}")]
    InvalidJobTransition { from: String, to: String },
    #[error("job not found: {0}")]
    JobNotFound(String),
}

type CacheResult<T> = std::result::Result<T, CacheError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingRecord {
    pub key: String,
    pub value_json: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CourseCacheEntry {
    pub course_slug: String,
    pub source_url: String,
    pub title: Option<String>,
    pub payload_json: String,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobRecord {
    pub id: String,
    pub course_slug: String,
    pub source_url: String,
    pub status: String,
    pub selected_quality: String,
    pub download_videos: bool,
    pub download_exercises: bool,
    pub download_subtitles: bool,
    pub download_quizzes: bool,
    pub quiz_hints_json: String,
    pub output_dir: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobEventRecord {
    pub id: i64,
    pub job_id: String,
    pub event_type: String,
    pub message: String,
    pub payload_json: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewJobEvent {
    pub job_id: String,
    pub event_type: String,
    pub message: String,
    pub payload_json: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactRecord {
    pub id: String,
    pub job_id: String,
    pub artifact_type: String,
    pub path: String,
    pub status: String,
    pub size_bytes: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadHistoryEntry {
    pub job_id: String,
    pub course_slug: String,
    pub source_url: String,
    pub course_title: String,
    pub output_dir: String,
    pub completed_at: i64,
}

pub fn upsert_setting_json(
    connection: &Connection,
    key: &str,
    value_json: &str,
    updated_at: i64,
) -> CacheResult<()> {
    ensure_non_secret_setting_key(key)?;
    connection.execute(
        r#"
        INSERT INTO settings (key, value_json, updated_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(key) DO UPDATE SET
            value_json = excluded.value_json,
            updated_at = excluded.updated_at
        "#,
        params![key, value_json, updated_at],
    )?;
    Ok(())
}

pub fn get_setting(connection: &Connection, key: &str) -> CacheResult<Option<SettingRecord>> {
    ensure_non_secret_setting_key(key)?;
    let record = connection
        .query_row(
            "SELECT key, value_json, updated_at FROM settings WHERE key = ?1",
            params![key],
            |row| {
                Ok(SettingRecord {
                    key: row.get(0)?,
                    value_json: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            },
        )
        .optional()?;
    Ok(record)
}

pub fn upsert_course_cache_entry(
    connection: &Connection,
    entry: &CourseCacheEntry,
) -> CacheResult<()> {
    connection.execute(
        r#"
        INSERT INTO course_cache (course_slug, source_url, title, payload_json, fetched_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(course_slug) DO UPDATE SET
            source_url = excluded.source_url,
            title = excluded.title,
            payload_json = excluded.payload_json,
            fetched_at = excluded.fetched_at
        "#,
        params![
            &entry.course_slug,
            &entry.source_url,
            &entry.title,
            &entry.payload_json,
            entry.fetched_at
        ],
    )?;
    Ok(())
}

pub fn get_course_cache_entry(
    connection: &Connection,
    course_slug: &str,
) -> CacheResult<Option<CourseCacheEntry>> {
    let entry = connection
        .query_row(
            r#"
            SELECT course_slug, source_url, title, payload_json, fetched_at
            FROM course_cache
            WHERE course_slug = ?1
            "#,
            params![course_slug],
            |row| {
                Ok(CourseCacheEntry {
                    course_slug: row.get(0)?,
                    source_url: row.get(1)?,
                    title: row.get(2)?,
                    payload_json: row.get(3)?,
                    fetched_at: row.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(entry)
}

pub fn insert_job(connection: &Connection, job: &JobRecord) -> CacheResult<()> {
    connection.execute(
        r#"
        INSERT INTO jobs (
            id,
            course_slug,
            source_url,
            status,
            selected_quality,
            download_videos,
            download_exercises,
            download_subtitles,
            download_quizzes,
            quiz_hints_json,
            output_dir,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        "#,
        params![
            &job.id,
            &job.course_slug,
            &job.source_url,
            &job.status,
            &job.selected_quality,
            job.download_videos,
            job.download_exercises,
            job.download_subtitles,
            job.download_quizzes,
            &job.quiz_hints_json,
            &job.output_dir,
            job.created_at,
            job.updated_at
        ],
    )?;
    Ok(())
}

pub fn update_job_status(
    connection: &Connection,
    job_id: &str,
    status: &str,
    updated_at: i64,
) -> CacheResult<()> {
    transition_job_status(connection, job_id, status, updated_at, None).map(|_| ())
}

pub fn transition_job_status(
    connection: &Connection,
    job_id: &str,
    next_status: &str,
    updated_at: i64,
    event_message: Option<&str>,
) -> CacheResult<JobRecord> {
    let current =
        get_job(connection, job_id)?.ok_or_else(|| CacheError::JobNotFound(job_id.to_string()))?;
    if !is_allowed_job_transition(&current.status, next_status) {
        return Err(CacheError::InvalidJobTransition {
            from: current.status,
            to: next_status.to_string(),
        });
    }

    connection.execute(
        "UPDATE jobs SET status = ?2, updated_at = ?3 WHERE id = ?1",
        params![job_id, next_status, updated_at],
    )?;
    let updated =
        get_job(connection, job_id)?.ok_or_else(|| CacheError::JobNotFound(job_id.to_string()))?;

    if let Some(message) = event_message {
        append_job_event(
            connection,
            &NewJobEvent {
                job_id: job_id.to_string(),
                event_type: format!("job.{next_status}"),
                message: message.to_string(),
                payload_json: None,
                created_at: updated_at,
            },
        )?;
    }

    Ok(updated)
}

pub fn reconcile_active_jobs_after_restart(
    connection: &Connection,
    restarted_at: i64,
) -> CacheResult<usize> {
    let active_jobs = list_jobs_by_status(connection, "active")?;
    let mut reconciled = 0;

    for job in active_jobs {
        transition_job_status(
            connection,
            &job.id,
            "failed",
            restarted_at,
            Some("Job was active when LinkVault restarted and was marked failed for recovery."),
        )?;
        connection.execute(
            r#"
            UPDATE artifacts
            SET status = 'failed', updated_at = ?2
            WHERE job_id = ?1 AND status IN ('pending', 'active')
            "#,
            params![&job.id, restarted_at],
        )?;
        reconciled += 1;
    }

    Ok(reconciled)
}

pub fn retry_failed_job(
    connection: &Connection,
    job_id: &str,
    retried_at: i64,
) -> CacheResult<JobRecord> {
    let current =
        get_job(connection, job_id)?.ok_or_else(|| CacheError::JobNotFound(job_id.to_string()))?;
    if current.status != "failed" {
        return Err(CacheError::InvalidJobTransition {
            from: current.status,
            to: "queued".to_string(),
        });
    }

    connection.execute(
        "UPDATE jobs SET status = 'queued', updated_at = ?2 WHERE id = ?1",
        params![job_id, retried_at],
    )?;
    append_job_event(
        connection,
        &NewJobEvent {
            job_id: job_id.to_string(),
            event_type: "job.retry".to_string(),
            message: "Retry requested; completed artifacts will be reused when present."
                .to_string(),
            payload_json: None,
            created_at: retried_at,
        },
    )?;

    get_job(connection, job_id)?.ok_or_else(|| CacheError::JobNotFound(job_id.to_string()))
}

pub fn clear_failed_jobs(connection: &Connection) -> CacheResult<usize> {
    connection
        .execute(
            "DELETE FROM jobs WHERE status IN ('failed', 'cancelled')",
            [],
        )
        .map_err(CacheError::from)
}

pub fn get_job(connection: &Connection, job_id: &str) -> CacheResult<Option<JobRecord>> {
    let job = connection
        .query_row(
            r#"
            SELECT
                id,
                course_slug,
                source_url,
                status,
                selected_quality,
                download_videos,
                download_exercises,
                download_subtitles,
                download_quizzes,
                quiz_hints_json,
                output_dir,
                created_at,
                updated_at
            FROM jobs
            WHERE id = ?1
            "#,
            params![job_id],
            job_from_row,
        )
        .optional()?;
    Ok(job)
}

pub fn list_jobs_by_status(connection: &Connection, status: &str) -> CacheResult<Vec<JobRecord>> {
    let mut statement = connection.prepare(
        r#"
        SELECT
            id,
            course_slug,
            source_url,
            status,
            selected_quality,
            download_videos,
            download_exercises,
            download_subtitles,
            download_quizzes,
            quiz_hints_json,
            output_dir,
            created_at,
            updated_at
        FROM jobs
        WHERE status = ?1
        ORDER BY created_at, id
        "#,
    )?;
    let jobs = statement
        .query_map(params![status], job_from_row)?
        .collect::<Result<Vec<_>>>()?;
    Ok(jobs)
}

pub fn list_recent_jobs(connection: &Connection, limit: usize) -> CacheResult<Vec<JobRecord>> {
    let mut statement = connection.prepare(
        r#"
        SELECT
            id,
            course_slug,
            source_url,
            status,
            selected_quality,
            download_videos,
            download_exercises,
            download_subtitles,
            download_quizzes,
            quiz_hints_json,
            output_dir,
            created_at,
            updated_at
        FROM jobs
        ORDER BY updated_at DESC, created_at DESC, id
        LIMIT ?1
        "#,
    )?;
    let jobs = statement
        .query_map(params![limit as i64], job_from_row)?
        .collect::<Result<Vec<_>>>()?;
    Ok(jobs)
}

pub fn list_download_history(connection: &Connection) -> CacheResult<Vec<DownloadHistoryEntry>> {
    let mut statement = connection.prepare(
        r#"
        SELECT
            jobs.id,
            jobs.course_slug,
            jobs.source_url,
            COALESCE(NULLIF(course_cache.title, ''), jobs.course_slug),
            jobs.output_dir,
            jobs.updated_at
        FROM jobs
        LEFT JOIN course_cache ON course_cache.course_slug = jobs.course_slug
        WHERE jobs.status = 'completed'
        ORDER BY jobs.updated_at DESC, jobs.created_at DESC, jobs.id
        "#,
    )?;
    let entries = statement
        .query_map([], |row| {
            Ok(DownloadHistoryEntry {
                job_id: row.get(0)?,
                course_slug: row.get(1)?,
                source_url: row.get(2)?,
                course_title: row.get(3)?,
                output_dir: row.get(4)?,
                completed_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>>>()?;
    Ok(entries)
}

pub fn append_job_event(connection: &Connection, event: &NewJobEvent) -> CacheResult<i64> {
    connection.execute(
        r#"
        INSERT INTO job_events (job_id, event_type, message, payload_json, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            &event.job_id,
            &event.event_type,
            &event.message,
            &event.payload_json,
            event.created_at
        ],
    )?;
    Ok(connection.last_insert_rowid())
}

pub fn list_job_events(connection: &Connection, job_id: &str) -> CacheResult<Vec<JobEventRecord>> {
    let mut statement = connection.prepare(
        r#"
        SELECT id, job_id, event_type, message, payload_json, created_at
        FROM job_events
        WHERE job_id = ?1
        ORDER BY id
        "#,
    )?;
    let events = statement
        .query_map(params![job_id], |row| {
            Ok(JobEventRecord {
                id: row.get(0)?,
                job_id: row.get(1)?,
                event_type: row.get(2)?,
                message: row.get(3)?,
                payload_json: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>>>()?;
    Ok(events)
}

pub fn upsert_artifact(connection: &Connection, artifact: &ArtifactRecord) -> CacheResult<()> {
    connection.execute(
        r#"
        INSERT INTO artifacts (
            id,
            job_id,
            artifact_type,
            path,
            status,
            size_bytes,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(id) DO UPDATE SET
            job_id = excluded.job_id,
            artifact_type = excluded.artifact_type,
            path = excluded.path,
            status = CASE
                WHEN artifacts.status = 'completed' AND excluded.status = 'pending' THEN artifacts.status
                ELSE excluded.status
            END,
            size_bytes = CASE
                WHEN artifacts.status = 'completed' AND excluded.status = 'pending' THEN artifacts.size_bytes
                ELSE excluded.size_bytes
            END,
            updated_at = excluded.updated_at
        "#,
        params![
            &artifact.id,
            &artifact.job_id,
            &artifact.artifact_type,
            &artifact.path,
            &artifact.status,
            &artifact.size_bytes,
            artifact.created_at,
            artifact.updated_at
        ],
    )?;
    Ok(())
}

pub fn update_artifact_status(
    connection: &Connection,
    artifact_id: &str,
    status: &str,
    size_bytes: Option<i64>,
    updated_at: i64,
) -> CacheResult<()> {
    connection.execute(
        r#"
        UPDATE artifacts
        SET status = ?2, size_bytes = ?3, updated_at = ?4
        WHERE id = ?1
        "#,
        params![artifact_id, status, size_bytes, updated_at],
    )?;
    Ok(())
}

pub fn list_artifacts_for_job(
    connection: &Connection,
    job_id: &str,
) -> CacheResult<Vec<ArtifactRecord>> {
    let mut statement = connection.prepare(
        r#"
        SELECT id, job_id, artifact_type, path, status, size_bytes, created_at, updated_at
        FROM artifacts
        WHERE job_id = ?1
        ORDER BY created_at, id
        "#,
    )?;
    let artifacts = statement
        .query_map(params![job_id], artifact_from_row)?
        .collect::<Result<Vec<_>>>()?;
    Ok(artifacts)
}

fn job_from_row(row: &rusqlite::Row<'_>) -> Result<JobRecord> {
    Ok(JobRecord {
        id: row.get(0)?,
        course_slug: row.get(1)?,
        source_url: row.get(2)?,
        status: row.get(3)?,
        selected_quality: row.get(4)?,
        download_videos: row.get(5)?,
        download_exercises: row.get(6)?,
        download_subtitles: row.get(7)?,
        download_quizzes: row.get(8)?,
        quiz_hints_json: row.get(9)?,
        output_dir: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn artifact_from_row(row: &rusqlite::Row<'_>) -> Result<ArtifactRecord> {
    Ok(ArtifactRecord {
        id: row.get(0)?,
        job_id: row.get(1)?,
        artifact_type: row.get(2)?,
        path: row.get(3)?,
        status: row.get(4)?,
        size_bytes: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn ensure_non_secret_setting_key(key: &str) -> CacheResult<()> {
    let normalized = key.to_ascii_lowercase();
    let forbidden = ["li_at", "token", "cookie", "authorization"];
    if forbidden.iter().any(|needle| normalized.contains(needle)) {
        return Err(CacheError::SecretSettingKey(key.to_string()));
    }
    Ok(())
}

fn is_allowed_job_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("queued", "active")
            | ("queued", "cancelled")
            | ("active", "completed")
            | ("active", "failed")
            | ("active", "cancelled")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn initialized_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn creates_required_cache_tables() {
        let connection = initialized_connection();

        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();

        for table in [
            "artifacts",
            "course_cache",
            "job_events",
            "jobs",
            "settings",
        ] {
            assert!(
                names.iter().any(|name| name == table),
                "missing table {table}"
            );
        }
    }

    #[test]
    fn settings_round_trip_json_values_without_plaintext_secret_keys() {
        let connection = initialized_connection();

        upsert_setting_json(
            &connection,
            "download.preferences",
            r#"{"quality":"1080p","delayMs":500}"#,
            100,
        )
        .unwrap();
        upsert_setting_json(
            &connection,
            "download.preferences",
            r#"{"quality":"720p","delayMs":250}"#,
            200,
        )
        .unwrap();

        let setting = get_setting(&connection, "download.preferences")
            .unwrap()
            .unwrap();

        assert_eq!(setting.key, "download.preferences");
        assert_eq!(setting.value_json, r#"{"quality":"720p","delayMs":250}"#);
        assert_eq!(setting.updated_at, 200);
        assert!(matches!(
            upsert_setting_json(&connection, "linkedin.li_at", r#""secret""#, 300),
            Err(CacheError::SecretSettingKey(key)) if key == "linkedin.li_at"
        ));
        assert!(get_setting(&connection, "browser.cookie").is_err());
    }

    #[test]
    fn course_cache_upsert_round_trips_latest_payload() {
        let connection = initialized_connection();

        upsert_course_cache_entry(
            &connection,
            &CourseCacheEntry {
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                title: Some("Old title".to_string()),
                payload_json: r#"{"title":"Old title"}"#.to_string(),
                fetched_at: 100,
            },
        )
        .unwrap();
        upsert_course_cache_entry(
            &connection,
            &CourseCacheEntry {
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                title: Some("New title".to_string()),
                payload_json: r#"{"title":"New title"}"#.to_string(),
                fetched_at: 200,
            },
        )
        .unwrap();

        let cached = get_course_cache_entry(&connection, "sample-course")
            .unwrap()
            .unwrap();

        assert_eq!(cached.title.as_deref(), Some("New title"));
        assert_eq!(cached.payload_json, r#"{"title":"New title"}"#);
        assert_eq!(cached.fetched_at, 200);
    }

    #[test]
    fn jobs_can_be_inserted_listed_by_status_and_updated() {
        let connection = initialized_connection();
        let job = sample_job("job-1", "queued", 100);

        insert_job(&connection, &job).unwrap();

        assert_eq!(
            list_jobs_by_status(&connection, "queued").unwrap(),
            vec![job.clone()]
        );

        update_job_status(&connection, "job-1", "active", 200).unwrap();
        let updated = get_job(&connection, "job-1").unwrap().unwrap();

        assert_eq!(updated.status, "active");
        assert_eq!(updated.updated_at, 200);
        assert!(list_jobs_by_status(&connection, "queued")
            .unwrap()
            .is_empty());
        assert_eq!(
            list_jobs_by_status(&connection, "active").unwrap(),
            vec![updated]
        );
    }

    #[test]
    fn recent_jobs_are_ordered_by_latest_update() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-old", "queued", 100)).unwrap();
        insert_job(&connection, &sample_job("job-new", "failed", 200)).unwrap();
        insert_job(&connection, &sample_job("job-middle", "completed", 150)).unwrap();

        let recent = list_recent_jobs(&connection, 2).unwrap();

        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, "job-new");
        assert_eq!(recent[1].id, "job-middle");
    }

    #[test]
    fn download_history_lists_completed_courses_with_cached_titles() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-queued", "queued", 100)).unwrap();
        insert_job(&connection, &sample_job("job-done", "completed", 200)).unwrap();
        upsert_course_cache_entry(
            &connection,
            &CourseCacheEntry {
                course_slug: "sample-course".to_string(),
                source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
                title: Some("Sample Course".to_string()),
                payload_json: "{}".to_string(),
                fetched_at: 200,
            },
        )
        .unwrap();

        let history = list_download_history(&connection).unwrap();

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].job_id, "job-done");
        assert_eq!(history[0].course_title, "Sample Course");
        assert_eq!(history[0].completed_at, 200);
    }

    #[test]
    fn retry_failed_job_returns_job_to_queue_and_preserves_artifacts_for_resume() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-1", "failed", 100)).unwrap();
        upsert_artifact(
            &connection,
            &ArtifactRecord {
                id: "artifact-1".to_string(),
                job_id: "job-1".to_string(),
                artifact_type: "video".to_string(),
                path: "C:/downloads/sample/welcome.mp4".to_string(),
                status: "failed".to_string(),
                size_bytes: None,
                created_at: 110,
                updated_at: 120,
            },
        )
        .unwrap();

        let retried = retry_failed_job(&connection, "job-1", 200).unwrap();

        assert_eq!(retried.status, "queued");
        assert_eq!(retried.updated_at, 200);
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].status, "failed");
        let events = list_job_events(&connection, "job-1").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "job.retry");
    }

    #[test]
    fn clear_failed_jobs_removes_failed_and_cancelled_jobs_only() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("queued-job", "queued", 100)).unwrap();
        insert_job(&connection, &sample_job("failed-job", "failed", 100)).unwrap();
        insert_job(&connection, &sample_job("cancelled-job", "cancelled", 100)).unwrap();

        let removed = clear_failed_jobs(&connection).unwrap();

        assert_eq!(removed, 2);
        assert!(get_job(&connection, "failed-job").unwrap().is_none());
        assert!(get_job(&connection, "cancelled-job").unwrap().is_none());
        assert_eq!(
            get_job(&connection, "queued-job").unwrap().unwrap().status,
            "queued"
        );
    }

    #[test]
    fn job_lifecycle_allows_only_cancellation_safe_transitions() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-1", "queued", 100)).unwrap();

        let active = transition_job_status(
            &connection,
            "job-1",
            "active",
            110,
            Some("Started metadata fetch."),
        )
        .unwrap();

        assert_eq!(active.status, "active");
        assert_eq!(active.updated_at, 110);
        assert!(matches!(
            transition_job_status(&connection, "job-1", "queued", 120, None),
            Err(CacheError::InvalidJobTransition { from, to })
                if from == "active" && to == "queued"
        ));

        let cancelled = transition_job_status(
            &connection,
            "job-1",
            "cancelled",
            130,
            Some("Cancelled during video download."),
        )
        .unwrap();

        assert_eq!(cancelled.status, "cancelled");
        assert!(matches!(
            transition_job_status(&connection, "job-1", "completed", 140, None),
            Err(CacheError::InvalidJobTransition { from, to })
                if from == "cancelled" && to == "completed"
        ));

        let events = list_job_events(&connection, "job-1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "job.active");
        assert_eq!(events[0].message, "Started metadata fetch.");
        assert_eq!(events[1].event_type, "job.cancelled");
        assert_eq!(events[1].message, "Cancelled during video download.");
    }

    #[test]
    fn job_lifecycle_rejects_missing_jobs() {
        let connection = initialized_connection();

        assert!(matches!(
            transition_job_status(&connection, "missing-job", "active", 100, None),
            Err(CacheError::JobNotFound(job_id)) if job_id == "missing-job"
        ));
    }

    #[test]
    fn restart_reconciliation_marks_active_jobs_failed_and_leaves_terminal_jobs_alone() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("queued-job", "queued", 100)).unwrap();
        insert_job(&connection, &sample_job("active-job", "active", 100)).unwrap();
        insert_job(&connection, &sample_job("done-job", "completed", 100)).unwrap();
        upsert_artifact(
            &connection,
            &ArtifactRecord {
                id: "artifact-active".to_string(),
                job_id: "active-job".to_string(),
                artifact_type: "video".to_string(),
                path: "C:/downloads/sample/partial.mp4".to_string(),
                status: "active".to_string(),
                size_bytes: Some(512),
                created_at: 105,
                updated_at: 105,
            },
        )
        .unwrap();

        let reconciled = reconcile_active_jobs_after_restart(&connection, 200).unwrap();

        assert_eq!(reconciled, 1);
        let active_job = get_job(&connection, "active-job").unwrap().unwrap();
        let queued_job = get_job(&connection, "queued-job").unwrap().unwrap();
        let done_job = get_job(&connection, "done-job").unwrap().unwrap();
        let events = list_job_events(&connection, "active-job").unwrap();
        let artifacts = list_artifacts_for_job(&connection, "active-job").unwrap();

        assert_eq!(active_job.status, "failed");
        assert_eq!(active_job.updated_at, 200);
        assert_eq!(queued_job.status, "queued");
        assert_eq!(done_job.status, "completed");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "job.failed");
        assert!(events[0].message.contains("restarted"));
        assert_eq!(artifacts[0].status, "failed");
        assert_eq!(artifacts[0].size_bytes, Some(512));
        assert_eq!(artifacts[0].updated_at, 200);
        assert!(list_jobs_by_status(&connection, "active")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn job_events_are_appended_and_deleted_with_job() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-1", "active", 100)).unwrap();

        let first_id = append_job_event(
            &connection,
            &NewJobEvent {
                job_id: "job-1".to_string(),
                event_type: "metadata".to_string(),
                message: "Fetched course metadata".to_string(),
                payload_json: Some(r#"{"videos":1}"#.to_string()),
                created_at: 110,
            },
        )
        .unwrap();
        let second_id = append_job_event(
            &connection,
            &NewJobEvent {
                job_id: "job-1".to_string(),
                event_type: "download".to_string(),
                message: "Downloaded welcome video".to_string(),
                payload_json: None,
                created_at: 120,
            },
        )
        .unwrap();

        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, first_id);
        assert_eq!(events[0].payload_json.as_deref(), Some(r#"{"videos":1}"#));
        assert_eq!(events[1].id, second_id);
        assert_eq!(events[1].message, "Downloaded welcome video");

        connection
            .execute("DELETE FROM jobs WHERE id = ?1", params!["job-1"])
            .unwrap();
        assert!(list_job_events(&connection, "job-1").unwrap().is_empty());
    }

    #[test]
    fn artifacts_upsert_update_and_cascade_with_job() {
        let connection = initialized_connection();
        insert_job(&connection, &sample_job("job-1", "active", 100)).unwrap();

        upsert_artifact(
            &connection,
            &ArtifactRecord {
                id: "artifact-1".to_string(),
                job_id: "job-1".to_string(),
                artifact_type: "video".to_string(),
                path: "C:/downloads/sample/welcome.mp4".to_string(),
                status: "pending".to_string(),
                size_bytes: None,
                created_at: 110,
                updated_at: 110,
            },
        )
        .unwrap();
        update_artifact_status(&connection, "artifact-1", "completed", Some(2048), 130).unwrap();
        upsert_artifact(
            &connection,
            &ArtifactRecord {
                id: "artifact-1".to_string(),
                job_id: "job-1".to_string(),
                artifact_type: "video".to_string(),
                path: "C:/downloads/sample/welcome.mp4".to_string(),
                status: "pending".to_string(),
                size_bytes: None,
                created_at: 110,
                updated_at: 140,
            },
        )
        .unwrap();

        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].status, "completed");
        assert_eq!(artifacts[0].size_bytes, Some(2048));
        assert_eq!(artifacts[0].updated_at, 140);

        connection
            .execute("DELETE FROM jobs WHERE id = ?1", params!["job-1"])
            .unwrap();
        assert!(list_artifacts_for_job(&connection, "job-1")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn schema_does_not_create_plaintext_token_columns() {
        let forbidden = ["li_at", "token", "cookie"];
        let schema = SCHEMA.to_lowercase();

        for needle in forbidden {
            assert!(
                !schema.contains(needle),
                "schema must not contain plaintext secret column {needle}"
            );
        }
    }

    fn sample_job(id: &str, status: &str, timestamp: i64) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            course_slug: "sample-course".to_string(),
            source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
            status: status.to_string(),
            selected_quality: "1080p".to_string(),
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
}
