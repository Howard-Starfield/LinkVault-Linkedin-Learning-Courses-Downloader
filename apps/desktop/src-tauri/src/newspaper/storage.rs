use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Result};
use thiserror::Error;

use super::catalog;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS newspaper_editions (
    code TEXT NOT NULL,
    publication_date TEXT NOT NULL DEFAULT '',
    name_zh TEXT NOT NULL,
    name_en TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('daily', 'weekly', 'special')),
    schedule TEXT NOT NULL CHECK (schedule IN ('daily', 'weekly_sunday', 'ad_hoc')),
    source_url TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    discovered INTEGER NOT NULL DEFAULT 0,
    discovered_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (code, publication_date)
);

CREATE TABLE IF NOT EXISTS newspaper_batches (
    id TEXT PRIMARY KEY NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'scheduled', 'active', 'paused', 'completed', 'completed_with_warnings', 'failed', 'cancelled')),
    destination TEXT NOT NULL,
    scheduled_at INTEGER,
    delay_minutes INTEGER NOT NULL CHECK (delay_minutes BETWEEN 0 AND 1440),
    optimize_images INTEGER NOT NULL,
    optimization_profile TEXT NOT NULL CHECK (optimization_profile IN ('webp_high', 'webp_balanced')),
    keep_original_jpg INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);

CREATE TABLE IF NOT EXISTS newspaper_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    batch_id TEXT NOT NULL,
    edition_code TEXT NOT NULL,
    edition_publication_date TEXT NOT NULL DEFAULT '',
    publication_date TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'active', 'optimizing', 'completed', 'partial', 'unavailable', 'failed', 'cancelled')),
    output_dir TEXT NOT NULL,
    page_count INTEGER NOT NULL DEFAULT 0 CHECK (page_count >= 0),
    completed_count INTEGER NOT NULL DEFAULT 0 CHECK (completed_count >= 0),
    failed_count INTEGER NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    original_bytes INTEGER NOT NULL DEFAULT 0 CHECK (original_bytes >= 0),
    final_bytes INTEGER NOT NULL DEFAULT 0 CHECK (final_bytes >= 0),
    warning TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    FOREIGN KEY (batch_id) REFERENCES newspaper_batches(id) ON DELETE CASCADE,
    FOREIGN KEY (edition_code, edition_publication_date)
        REFERENCES newspaper_editions(code, publication_date),
    UNIQUE (edition_code, publication_date, output_dir)
);

CREATE TABLE IF NOT EXISTS newspaper_pages (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL,
    page_number TEXT NOT NULL,
    section_name TEXT,
    source_url TEXT NOT NULL,
    original_path TEXT,
    optimized_path TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'downloading', 'downloaded', 'optimizing', 'completed', 'failed', 'cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    original_bytes INTEGER,
    final_bytes INTEGER,
    checksum TEXT,
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (job_id) REFERENCES newspaper_jobs(id) ON DELETE CASCADE,
    UNIQUE (job_id, page_number)
);

CREATE TABLE IF NOT EXISTS newspaper_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT,
    job_id TEXT,
    event_type TEXT NOT NULL,
    message TEXT NOT NULL,
    payload_json TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (batch_id) REFERENCES newspaper_batches(id) ON DELETE CASCADE,
    FOREIGN KEY (job_id) REFERENCES newspaper_jobs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS newspaper_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_newspaper_batches_status_due
    ON newspaper_batches(status, scheduled_at);
CREATE INDEX IF NOT EXISTS idx_newspaper_jobs_batch_status
    ON newspaper_jobs(batch_id, status);
CREATE INDEX IF NOT EXISTS idx_newspaper_jobs_library
    ON newspaper_jobs(publication_date DESC, status);
CREATE INDEX IF NOT EXISTS idx_newspaper_pages_job_status
    ON newspaper_pages(job_id, status);
"#;

pub fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(SCHEMA)?;
    seed_built_in_catalog(connection, 0)
}

pub fn seed_built_in_catalog(connection: &Connection, updated_at: i64) -> Result<()> {
    for edition in catalog::built_in_catalog() {
        connection.execute(
            r#"
            INSERT INTO newspaper_editions (
                code, publication_date, name_zh, name_en, kind, schedule,
                source_url, active, discovered, updated_at
            )
            VALUES (?1, '', ?2, ?3, ?4, ?5, ?6, 1, 0, ?7)
            ON CONFLICT(code, publication_date) DO UPDATE SET
                name_zh = excluded.name_zh,
                name_en = excluded.name_en,
                kind = excluded.kind,
                schedule = excluded.schedule,
                source_url = excluded.source_url,
                active = 1,
                updated_at = excluded.updated_at
            "#,
            params![
                edition.code,
                edition.name_zh,
                edition.name_en,
                edition.kind.as_str(),
                edition.schedule.as_str(),
                edition.source_url,
                updated_at,
            ],
        )?;
    }
    Ok(())
}

pub fn reconcile_after_restart(connection: &Connection, updated_at: i64) -> Result<usize> {
    let jobs = connection.execute(
        "UPDATE newspaper_jobs SET status = 'queued', updated_at = ?1 WHERE status IN ('active', 'optimizing')",
        params![updated_at],
    )?;
    connection.execute(
        "UPDATE newspaper_pages SET status = 'pending', updated_at = ?1 WHERE status IN ('downloading', 'optimizing')",
        params![updated_at],
    )?;
    connection.execute(
        "UPDATE newspaper_batches SET status = 'queued', updated_at = ?1 WHERE status = 'active'",
        params![updated_at],
    )?;
    Ok(jobs)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobCompletion {
    pub status: String,
    pub page_count: i64,
    pub completed_count: i64,
    pub failed_count: i64,
}

#[derive(Debug, Error)]
pub enum FinalizeError {
    #[error("newspaper job not found: {0}")]
    JobNotFound(String),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn finalize_job(
    connection: &Connection,
    job_id: &str,
    updated_at: i64,
) -> std::result::Result<JobCompletion, FinalizeError> {
    let job = connection
        .query_row(
            "SELECT output_dir, page_count FROM newspaper_jobs WHERE id = ?1",
            params![job_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or_else(|| FinalizeError::JobNotFound(job_id.to_string()))?;
    let (completed_count, failed_count): (i64, i64) = connection.query_row(
        "SELECT
            SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END),
            SUM(CASE WHEN status IN ('failed', 'cancelled') THEN 1 ELSE 0 END)
         FROM newspaper_pages WHERE job_id = ?1",
        params![job_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let page_count = job.1;
    let status = if page_count > 0 && completed_count == page_count && failed_count == 0 {
        "completed"
    } else if completed_count > 0 {
        "partial"
    } else {
        "failed"
    };
    let marker = Path::new(&job.0).join(".complete");

    if status == "completed" {
        std::fs::create_dir_all(&job.0)?;
        let part = Path::new(&job.0).join(".complete.part");
        std::fs::write(&part, b"validated")?;
        std::fs::rename(part, &marker)?;
    } else if marker.exists() {
        std::fs::remove_file(&marker)?;
    }

    connection.execute(
        "UPDATE newspaper_jobs
         SET status = ?2, completed_count = ?3, failed_count = ?4,
             updated_at = ?5, completed_at = CASE WHEN ?2 IN ('completed', 'partial') THEN ?5 ELSE NULL END
         WHERE id = ?1",
        params![job_id, status, completed_count, failed_count, updated_at],
    )?;

    Ok(JobCompletion {
        status: status.to_string(),
        page_count,
        completed_count,
        failed_count,
    })
}

pub fn retry_missing_pages(
    connection: &Connection,
    job_id: &str,
    updated_at: i64,
) -> Result<usize> {
    let changed = connection.execute(
        "UPDATE newspaper_pages
         SET status = 'pending', error = NULL, updated_at = ?2
         WHERE job_id = ?1 AND status IN ('failed', 'cancelled')",
        params![job_id, updated_at],
    )?;
    if changed > 0 {
        connection.execute(
            "UPDATE newspaper_jobs
             SET status = 'queued', failed_count = 0, completed_at = NULL, updated_at = ?2
             WHERE id = ?1 AND status IN ('partial', 'failed', 'cancelled')",
            params![job_id, updated_at],
        )?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn initialized() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn schema_keeps_newspaper_tables_provider_prefixed() {
        let connection = initialized();
        let names = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'newspaper_%' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();

        assert_eq!(
            names,
            vec![
                "newspaper_batches",
                "newspaper_editions",
                "newspaper_events",
                "newspaper_jobs",
                "newspaper_pages",
                "newspaper_settings",
            ]
        );
    }

    #[test]
    fn initialization_seeds_the_verified_regular_catalog() {
        let connection = initialized();
        let counts: (i64, i64) = connection
            .query_row(
                "SELECT
                    SUM(CASE WHEN kind = 'daily' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN kind = 'weekly' THEN 1 ELSE 0 END)
                 FROM newspaper_editions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (10, 3));
    }

    #[test]
    fn special_editions_can_share_a_code_when_dates_differ() {
        let connection = initialized();
        for (date, title) in [
            ("2026-02-17", "馬年春節專刊"),
            ("2026-03-01", "2026報稅新攻略"),
        ] {
            connection
                .execute(
                    "INSERT INTO newspaper_editions
                    (code, publication_date, name_zh, name_en, kind, schedule, source_url, updated_at)
                    VALUES ('EA', ?1, ?2, 'Special publication', 'special', 'ad_hoc', ?3, 1)",
                    params![date, title, format!("https://ep.worldjournal.com/EA/{date}")],
                )
                .unwrap();
        }
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM newspaper_editions WHERE code = 'EA'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn restart_requeues_active_work_but_preserves_completed_pages() {
        let connection = initialized();
        connection.execute(
            "INSERT INTO newspaper_batches
            (id, status, destination, delay_minutes, optimize_images, optimization_profile, keep_original_jpg, created_at, updated_at)
            VALUES ('batch-1', 'active', 'C:/papers', 5, 1, 'webp_high', 0, 1, 1)",
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO newspaper_jobs
            (id, batch_id, edition_code, publication_date, status, output_dir, created_at, updated_at)
            VALUES ('job-1', 'batch-1', 'NY', '2026-07-24', 'active', 'C:/papers/NY/2026-07-24', 1, 1)",
            [],
        ).unwrap();
        for (id, page, status) in [
            ("page-1", "A01", "completed"),
            ("page-2", "A02", "downloading"),
        ] {
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                (id, job_id, page_number, source_url, status, created_at, updated_at)
                VALUES (?1, 'job-1', ?2, ?3, ?4, 1, 1)",
                    params![
                        id,
                        page,
                        format!("https://ep.worldjournal.com/{page}.jpg"),
                        status
                    ],
                )
                .unwrap();
        }

        assert_eq!(reconcile_after_restart(&connection, 10).unwrap(), 1);
        let job_status: String = connection
            .query_row(
                "SELECT status FROM newspaper_jobs WHERE id = 'job-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let page_statuses = connection
            .prepare("SELECT status FROM newspaper_pages ORDER BY page_number")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(job_status, "queued");
        assert_eq!(page_statuses, vec!["completed", "pending"]);
    }

    fn insert_job_with_pages(connection: &Connection, output_dir: &Path, statuses: &[&str]) {
        connection.execute(
            "INSERT INTO newspaper_batches
            (id, status, destination, delay_minutes, optimize_images, optimization_profile, keep_original_jpg, created_at, updated_at)
            VALUES ('finalize-batch', 'active', ?1, 0, 0, 'webp_high', 1, 1, 1)",
            params![output_dir.to_string_lossy()],
        ).unwrap();
        connection.execute(
            "INSERT INTO newspaper_jobs
            (id, batch_id, edition_code, publication_date, status, output_dir, page_count, created_at, updated_at)
            VALUES ('finalize-job', 'finalize-batch', 'NY', '2026-07-24', 'active', ?1, ?2, 1, 1)",
            params![output_dir.to_string_lossy(), statuses.len() as i64],
        ).unwrap();
        for (index, status) in statuses.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                (id, job_id, page_number, source_url, status, created_at, updated_at)
                VALUES (?1, 'finalize-job', ?2, ?3, ?4, 1, 1)",
                    params![
                        format!("finalize-page-{index}"),
                        format!("A{:02}", index + 1),
                        format!("https://ep.worldjournal.com/{index}.jpg"),
                        status,
                    ],
                )
                .unwrap();
        }
    }

    #[test]
    fn partial_job_never_receives_a_complete_marker() {
        let connection = initialized();
        let directory = tempdir().unwrap();
        insert_job_with_pages(&connection, directory.path(), &["completed", "failed"]);

        let result = finalize_job(&connection, "finalize-job", 10).unwrap();

        assert_eq!(result.status, "partial");
        assert_eq!(result.completed_count, 1);
        assert_eq!(result.failed_count, 1);
        assert!(!directory.path().join(".complete").exists());
    }

    #[test]
    fn complete_marker_is_written_only_when_every_required_page_completed() {
        let connection = initialized();
        let directory = tempdir().unwrap();
        insert_job_with_pages(&connection, directory.path(), &["completed", "completed"]);

        let result = finalize_job(&connection, "finalize-job", 10).unwrap();

        assert_eq!(result.status, "completed");
        assert!(directory.path().join(".complete").exists());
        assert!(!directory.path().join(".complete.part").exists());
    }

    #[test]
    fn retry_missing_pages_preserves_completed_pages() {
        let connection = initialized();
        let directory = tempdir().unwrap();
        insert_job_with_pages(&connection, directory.path(), &["completed", "failed"]);
        connection
            .execute(
                "UPDATE newspaper_jobs SET status = 'partial' WHERE id = 'finalize-job'",
                [],
            )
            .unwrap();

        assert_eq!(
            retry_missing_pages(&connection, "finalize-job", 20).unwrap(),
            1
        );
        let statuses = connection
            .prepare("SELECT status FROM newspaper_pages ORDER BY page_number")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(statuses, vec!["completed", "pending"]);
    }
}
