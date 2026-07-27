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
    delay_seconds INTEGER NOT NULL DEFAULT 15 CHECK (delay_seconds BETWEEN 0 AND 3600),
    optimize_images INTEGER NOT NULL,
    optimization_profile TEXT NOT NULL CHECK (optimization_profile IN ('webp_high', 'webp_balanced')),
    optimization_quality INTEGER NOT NULL DEFAULT 86 CHECK (optimization_quality BETWEEN 25 AND 95),
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
    retry_at INTEGER,
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    warning TEXT,
    queue_position INTEGER NOT NULL DEFAULT 0,
    paused INTEGER NOT NULL DEFAULT 0,
    dismissed INTEGER NOT NULL DEFAULT 0,
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
    pixel_width INTEGER CHECK (pixel_width IS NULL OR pixel_width > 0),
    pixel_height INTEGER CHECK (pixel_height IS NULL OR pixel_height > 0),
    media_version INTEGER NOT NULL DEFAULT 1 CHECK (media_version > 0),
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (job_id) REFERENCES newspaper_jobs(id) ON DELETE CASCADE,
    UNIQUE (job_id, page_number)
);

CREATE TABLE IF NOT EXISTS newspaper_optimization_tasks (
    page_id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'kept_original', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    lease_owner TEXT,
    lease_expires_at INTEGER,
    retry_at INTEGER,
    started_at INTEGER,
    completed_at INTEGER,
    source_path TEXT NOT NULL,
    source_size INTEGER,
    source_modified_at INTEGER,
    source_checksum TEXT,
    output_path TEXT,
    source_bytes INTEGER,
    output_bytes INTEGER,
    elapsed_ms INTEGER,
    last_error TEXT,
    error_kind TEXT,
    recovered INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (page_id) REFERENCES newspaper_pages(id) ON DELETE CASCADE,
    FOREIGN KEY (job_id) REFERENCES newspaper_jobs(id) ON DELETE CASCADE
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

CREATE TABLE IF NOT EXISTS newspaper_schedules (
    id TEXT PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    cron_time TEXT NOT NULL,
    destination TEXT NOT NULL,
    edition_codes_json TEXT NOT NULL,
    delay_seconds INTEGER NOT NULL DEFAULT 15 CHECK (delay_seconds BETWEEN 0 AND 3600),
    optimize_images INTEGER NOT NULL,
    optimization_profile TEXT NOT NULL CHECK (optimization_profile IN ('webp_high', 'webp_balanced')),
    optimization_quality INTEGER NOT NULL DEFAULT 86 CHECK (optimization_quality BETWEEN 25 AND 95),
    keep_original_jpg INTEGER NOT NULL,
    last_run_date TEXT,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS newspaper_reading_progress (
    job_id TEXT PRIMARY KEY NOT NULL,
    last_page_id TEXT NOT NULL,
    last_page_index INTEGER NOT NULL DEFAULT 0 CHECK (last_page_index >= 0),
    furthest_page_index INTEGER NOT NULL DEFAULT 0 CHECK (furthest_page_index >= 0),
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (job_id) REFERENCES newspaper_jobs(id) ON DELETE CASCADE,
    FOREIGN KEY (last_page_id) REFERENCES newspaper_pages(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS newspaper_read_pages (
    job_id TEXT NOT NULL,
    page_id TEXT NOT NULL,
    page_index INTEGER NOT NULL CHECK (page_index >= 0),
    viewed_at INTEGER NOT NULL,
    PRIMARY KEY (job_id, page_id),
    FOREIGN KEY (job_id) REFERENCES newspaper_jobs(id) ON DELETE CASCADE,
    FOREIGN KEY (page_id) REFERENCES newspaper_pages(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS newspaper_thumbnail_cache (
    job_id TEXT PRIMARY KEY NOT NULL,
    source_page_id TEXT NOT NULL,
    source_media_version INTEGER NOT NULL CHECK (source_media_version > 0),
    cache_schema_version INTEGER NOT NULL CHECK (cache_schema_version > 0),
    cache_path TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    pixel_width INTEGER NOT NULL CHECK (pixel_width > 0),
    pixel_height INTEGER NOT NULL CHECK (pixel_height > 0),
    byte_count INTEGER NOT NULL CHECK (byte_count > 0),
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (job_id) REFERENCES newspaper_jobs(id) ON DELETE CASCADE,
    FOREIGN KEY (source_page_id) REFERENCES newspaper_pages(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_newspaper_batches_status_due
    ON newspaper_batches(status, scheduled_at);
CREATE INDEX IF NOT EXISTS idx_newspaper_jobs_batch_status
    ON newspaper_jobs(batch_id, status);
CREATE INDEX IF NOT EXISTS idx_newspaper_jobs_library
    ON newspaper_jobs(publication_date DESC, status);
CREATE INDEX IF NOT EXISTS idx_newspaper_pages_job_status
    ON newspaper_pages(job_id, status);
CREATE INDEX IF NOT EXISTS idx_newspaper_optimization_tasks_queue
    ON newspaper_optimization_tasks(job_id, status, retry_at, lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_newspaper_schedules_enabled_time
    ON newspaper_schedules(enabled, cron_time);
CREATE INDEX IF NOT EXISTS idx_newspaper_reading_progress_updated
    ON newspaper_reading_progress(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_newspaper_thumbnail_source
    ON newspaper_thumbnail_cache(source_page_id);
"#;

pub fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(SCHEMA)?;
    connection.execute(
        "INSERT OR IGNORE INTO newspaper_read_pages (job_id, page_id, page_index, viewed_at)
         SELECT p.job_id, p.last_page_id, p.last_page_index, p.updated_at
         FROM newspaper_reading_progress p
         JOIN newspaper_pages page ON page.id = p.last_page_id
         WHERE page.job_id = p.job_id AND page.status = 'completed'",
        [],
    )?;
    migrate_add_column(
        connection,
        "newspaper_batches",
        "delay_seconds",
        "ALTER TABLE newspaper_batches ADD COLUMN delay_seconds INTEGER NOT NULL DEFAULT 15 CHECK (delay_seconds BETWEEN 0 AND 3600)",
    )?;
    migrate_optimization_quality(connection, "newspaper_batches")?;
    migrate_optimization_quality(connection, "newspaper_schedules")?;
    migrate_add_column(
        connection,
        "newspaper_jobs",
        "retry_at",
        "ALTER TABLE newspaper_jobs ADD COLUMN retry_at INTEGER",
    )?;
    migrate_add_column(
        connection,
        "newspaper_jobs",
        "retry_count",
        "ALTER TABLE newspaper_jobs ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0)",
    )?;
    migrate_add_column(
        connection,
        "newspaper_jobs",
        "queue_position",
        "ALTER TABLE newspaper_jobs ADD COLUMN queue_position INTEGER NOT NULL DEFAULT 0",
    )?;
    migrate_add_column(
        connection,
        "newspaper_jobs",
        "paused",
        "ALTER TABLE newspaper_jobs ADD COLUMN paused INTEGER NOT NULL DEFAULT 0",
    )?;
    migrate_add_column(
        connection,
        "newspaper_jobs",
        "dismissed",
        "ALTER TABLE newspaper_jobs ADD COLUMN dismissed INTEGER NOT NULL DEFAULT 0",
    )?;
    migrate_add_column(
        connection,
        "newspaper_pages",
        "pixel_width",
        "ALTER TABLE newspaper_pages ADD COLUMN pixel_width INTEGER CHECK (pixel_width IS NULL OR pixel_width > 0)",
    )?;
    migrate_add_column(
        connection,
        "newspaper_pages",
        "pixel_height",
        "ALTER TABLE newspaper_pages ADD COLUMN pixel_height INTEGER CHECK (pixel_height IS NULL OR pixel_height > 0)",
    )?;
    migrate_add_column(
        connection,
        "newspaper_pages",
        "media_version",
        "ALTER TABLE newspaper_pages ADD COLUMN media_version INTEGER NOT NULL DEFAULT 1 CHECK (media_version > 0)",
    )?;
    migrate_add_column(
        connection,
        "newspaper_optimization_tasks",
        "recovered",
        "ALTER TABLE newspaper_optimization_tasks ADD COLUMN recovered INTEGER NOT NULL DEFAULT 0",
    )?;
    connection.execute(
        "UPDATE newspaper_jobs
         SET queue_position = created_at
         WHERE queue_position = 0",
        [],
    )?;
    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_newspaper_jobs_retry_due
         ON newspaper_jobs(status, retry_at)",
        [],
    )?;
    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_newspaper_jobs_queue
         ON newspaper_jobs(dismissed, paused, status, queue_position)",
        [],
    )?;
    seed_built_in_catalog(connection, 0)?;
    super::optimization_tasks::ensure_all(connection, 0)?;
    Ok(())
}

fn migrate_add_column(
    connection: &Connection,
    table: &str,
    column: &str,
    statement: &str,
) -> Result<()> {
    let exists = connection
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        connection.execute(statement, [])?;
    }
    Ok(())
}

fn migrate_optimization_quality(connection: &Connection, table: &str) -> Result<()> {
    let exists = connection
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "optimization_quality");
    if !exists {
        let statement = match table {
            "newspaper_batches" | "newspaper_schedules" => format!(
                "ALTER TABLE {table} ADD COLUMN optimization_quality INTEGER NOT NULL DEFAULT 86 CHECK (optimization_quality BETWEEN 25 AND 95)"
            ),
            _ => return Ok(()),
        };
        connection.execute(&statement, [])?;
        connection.execute(
            &format!(
                "UPDATE {table} SET optimization_quality = CASE optimization_profile WHEN 'webp_high' THEN 92 ELSE 86 END"
            ),
            [],
        )?;
        return Ok(());
    }

    let schema: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![table],
        |row| row.get(0),
    )?;
    if !schema.contains("optimization_quality BETWEEN 55 AND 95") {
        return Ok(());
    }
    rebuild_optimization_quality_constraint(connection, table)
}

fn rebuild_optimization_quality_constraint(connection: &Connection, table: &str) -> Result<()> {
    let foreign_keys_enabled: bool =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    if foreign_keys_enabled {
        connection.pragma_update(None, "foreign_keys", false)?;
    }
    connection.pragma_update(None, "legacy_alter_table", true)?;

    let migration = match table {
        "newspaper_batches" => connection.execute_batch(
            r#"
            BEGIN IMMEDIATE;
            ALTER TABLE newspaper_batches RENAME TO newspaper_batches_quality_legacy;
            CREATE TABLE newspaper_batches (
                id TEXT PRIMARY KEY NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('queued', 'scheduled', 'active', 'paused', 'completed', 'completed_with_warnings', 'failed', 'cancelled')),
                destination TEXT NOT NULL,
                scheduled_at INTEGER,
                delay_minutes INTEGER NOT NULL CHECK (delay_minutes BETWEEN 0 AND 1440),
                delay_seconds INTEGER NOT NULL DEFAULT 15 CHECK (delay_seconds BETWEEN 0 AND 3600),
                optimize_images INTEGER NOT NULL,
                optimization_profile TEXT NOT NULL CHECK (optimization_profile IN ('webp_high', 'webp_balanced')),
                optimization_quality INTEGER NOT NULL DEFAULT 86 CHECK (optimization_quality BETWEEN 25 AND 95),
                keep_original_jpg INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                completed_at INTEGER
            );
            INSERT INTO newspaper_batches
                (id, status, destination, scheduled_at, delay_minutes, delay_seconds,
                 optimize_images, optimization_profile, optimization_quality,
                 keep_original_jpg, created_at, updated_at, completed_at)
            SELECT id, status, destination, scheduled_at, delay_minutes, delay_seconds,
                   optimize_images, optimization_profile, optimization_quality,
                   keep_original_jpg, created_at, updated_at, completed_at
            FROM newspaper_batches_quality_legacy;
            DROP TABLE newspaper_batches_quality_legacy;
            CREATE INDEX IF NOT EXISTS idx_newspaper_batches_status_due
                ON newspaper_batches(status, scheduled_at);
            COMMIT;
            "#,
        ),
        "newspaper_schedules" => connection.execute_batch(
            r#"
            BEGIN IMMEDIATE;
            ALTER TABLE newspaper_schedules RENAME TO newspaper_schedules_quality_legacy;
            CREATE TABLE newspaper_schedules (
                id TEXT PRIMARY KEY NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                cron_time TEXT NOT NULL,
                destination TEXT NOT NULL,
                edition_codes_json TEXT NOT NULL,
                delay_seconds INTEGER NOT NULL DEFAULT 15 CHECK (delay_seconds BETWEEN 0 AND 3600),
                optimize_images INTEGER NOT NULL,
                optimization_profile TEXT NOT NULL CHECK (optimization_profile IN ('webp_high', 'webp_balanced')),
                optimization_quality INTEGER NOT NULL DEFAULT 86 CHECK (optimization_quality BETWEEN 25 AND 95),
                keep_original_jpg INTEGER NOT NULL,
                last_run_date TEXT,
                last_error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO newspaper_schedules
                (id, enabled, cron_time, destination, edition_codes_json, delay_seconds,
                 optimize_images, optimization_profile, optimization_quality,
                 keep_original_jpg, last_run_date, last_error, created_at, updated_at)
            SELECT id, enabled, cron_time, destination, edition_codes_json, delay_seconds,
                   optimize_images, optimization_profile, optimization_quality,
                   keep_original_jpg, last_run_date, last_error, created_at, updated_at
            FROM newspaper_schedules_quality_legacy;
            DROP TABLE newspaper_schedules_quality_legacy;
            CREATE INDEX IF NOT EXISTS idx_newspaper_schedules_enabled_time
                ON newspaper_schedules(enabled, cron_time);
            COMMIT;
            "#,
        ),
        _ => Ok(()),
    };
    if migration.is_err() {
        let _ = connection.execute_batch("ROLLBACK");
    }
    connection.pragma_update(None, "legacy_alter_table", false)?;
    if foreign_keys_enabled {
        connection.pragma_update(None, "foreign_keys", true)?;
    }
    migration
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

pub fn reconcile_after_restart(
    connection: &Connection,
    updated_at: i64,
) -> std::result::Result<usize, FinalizeError> {
    connection.execute(
        "UPDATE newspaper_jobs
         SET status = 'cancelled', updated_at = ?1
         WHERE dismissed = 1 AND status IN ('queued', 'active', 'optimizing')",
        params![updated_at],
    )?;
    connection.execute(
        "UPDATE newspaper_pages
         SET status = 'cancelled', updated_at = ?1
         WHERE status IN ('pending', 'downloading', 'optimizing')
           AND job_id IN (SELECT id FROM newspaper_jobs WHERE dismissed = 1)",
        params![updated_at],
    )?;
    let jobs = connection.execute(
        "UPDATE newspaper_jobs
         SET status = 'queued', updated_at = ?1
         WHERE dismissed = 0 AND status IN ('active', 'optimizing')",
        params![updated_at],
    )?;
    connection.execute(
        "UPDATE newspaper_pages
         SET status = 'pending', updated_at = ?1
         WHERE status IN ('downloading', 'optimizing')
           AND job_id IN (SELECT id FROM newspaper_jobs WHERE dismissed = 0)",
        params![updated_at],
    )?;
    connection.execute(
        "UPDATE newspaper_batches SET status = 'queued', updated_at = ?1 WHERE status = 'active'",
        params![updated_at],
    )?;
    super::optimization_tasks::reconcile(connection, updated_at)?;
    let candidates = connection
        .prepare(
            "SELECT id FROM newspaper_jobs
             WHERE dismissed = 0
               AND page_count > 0
               AND status IN ('queued', 'completed', 'partial')",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    let mut recovered = 0;
    for job_id in candidates {
        let completion = finalize_job(connection, &job_id, updated_at)?;
        if completion.status == "queued" {
            recovered += 1;
        }
    }
    Ok(jobs + recovered)
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
            "SELECT j.output_dir, j.page_count, j.batch_id, b.optimize_images
             FROM newspaper_jobs j
             JOIN newspaper_batches b ON b.id = j.batch_id
             WHERE j.id = ?1",
            params![job_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| FinalizeError::JobNotFound(job_id.to_string()))?;
    let page_records = connection
        .prepare(
            "SELECT id, status, original_path, optimized_path, original_bytes, final_bytes
             FROM newspaper_pages WHERE job_id = ?1",
        )?
        .query_map(params![job_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>>>()?;

    let mut missing_files = 0_i64;
    for (page_id, status, original_path, optimized_path, original_bytes, final_bytes) in
        page_records
    {
        if status != "completed"
            || recorded_page_is_valid(
                original_path.as_deref(),
                optimized_path.as_deref(),
                original_bytes,
                final_bytes,
            )
        {
            continue;
        }
        connection.execute(
            "UPDATE newspaper_pages
             SET status = 'pending', optimized_path = NULL,
                 error = 'Downloaded file is missing or incomplete; queued for recovery.',
                 updated_at = ?2
             WHERE id = ?1",
            params![page_id, updated_at],
        )?;
        missing_files += 1;
    }

    let (completed_count, failed_count, pending_count, optimization_pending): (i64, i64, i64, i64) =
        connection.query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status IN ('failed', 'cancelled') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status IN ('pending', 'downloading', 'downloaded', 'optimizing') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'completed' AND optimized_path IS NULL THEN 1 ELSE 0 END), 0)
             FROM newspaper_pages WHERE job_id = ?1",
            params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    let page_count = job.1;
    let status = if missing_files > 0 || pending_count > 0 {
        "queued"
    } else if page_count > 0 && completed_count == page_count && failed_count == 0 {
        if job.3 && optimization_pending > 0 {
            "optimizing"
        } else {
            "completed"
        }
    } else if completed_count > 0 {
        "partial"
    } else {
        "failed"
    };
    let marker = Path::new(&job.0).join(".complete");
    let marker_part = Path::new(&job.0).join(".complete.part");

    if status == "completed" {
        std::fs::create_dir_all(&job.0)?;
        std::fs::write(
            &marker_part,
            format!("validated-pages={page_count}").as_bytes(),
        )?;
        std::fs::rename(&marker_part, &marker)?;
    } else {
        if marker.exists() {
            std::fs::remove_file(&marker)?;
        }
        if marker_part.exists() {
            std::fs::remove_file(&marker_part)?;
        }
    }

    connection.execute(
        "UPDATE newspaper_jobs
         SET status = ?2, completed_count = ?3, failed_count = ?4,
             updated_at = ?5, completed_at = CASE WHEN ?2 IN ('completed', 'partial') THEN ?5 ELSE NULL END
         WHERE id = ?1",
        params![job_id, status, completed_count, failed_count, updated_at],
    )?;

    if status == "queued" {
        connection.execute(
            "UPDATE newspaper_batches
             SET status = 'queued', completed_at = NULL, updated_at = ?2
             WHERE id = ?1 AND status NOT IN ('paused', 'cancelled')",
            params![job.2, updated_at],
        )?;
    } else if status == "optimizing" {
        connection.execute(
            "UPDATE newspaper_batches
             SET status = 'active', completed_at = NULL, updated_at = ?2
             WHERE id = ?1 AND status NOT IN ('paused', 'cancelled')",
            params![job.2, updated_at],
        )?;
    }

    Ok(JobCompletion {
        status: status.to_string(),
        page_count,
        completed_count,
        failed_count,
    })
}

fn recorded_page_is_valid(
    original_path: Option<&str>,
    optimized_path: Option<&str>,
    original_bytes: Option<i64>,
    final_bytes: Option<i64>,
) -> bool {
    let (path, expected_bytes) = match optimized_path.filter(|value| !value.is_empty()) {
        Some(path) => (Some(path), final_bytes.or(original_bytes)),
        None => (
            original_path.filter(|value| !value.is_empty()),
            original_bytes.or(final_bytes),
        ),
    };
    let Some(path) = path else {
        return false;
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    expected_bytes
        .filter(|value| *value > 0)
        .map(|value| metadata.len() == value as u64)
        .unwrap_or(true)
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
                "newspaper_optimization_tasks",
                "newspaper_pages",
                "newspaper_read_pages",
                "newspaper_reading_progress",
                "newspaper_schedules",
                "newspaper_settings",
                "newspaper_thumbnail_cache",
            ]
        );
    }

    #[test]
    fn initialization_adds_recovery_marker_to_phase_one_task_ledgers() {
        let connection = initialized();
        connection
            .execute("DROP TABLE newspaper_optimization_tasks", [])
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE newspaper_optimization_tasks (
                    page_id TEXT PRIMARY KEY NOT NULL,
                    job_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    lease_owner TEXT,
                    lease_expires_at INTEGER,
                    retry_at INTEGER,
                    started_at INTEGER,
                    completed_at INTEGER,
                    source_path TEXT NOT NULL,
                    source_size INTEGER,
                    source_modified_at INTEGER,
                    source_checksum TEXT,
                    output_path TEXT,
                    source_bytes INTEGER,
                    output_bytes INTEGER,
                    elapsed_ms INTEGER,
                    last_error TEXT,
                    error_kind TEXT,
                    updated_at INTEGER NOT NULL
                );",
            )
            .unwrap();

        initialize(&connection).unwrap();

        let columns = connection
            .prepare("PRAGMA table_info(newspaper_optimization_tasks)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(columns.contains(&"recovered".to_string()));
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
    fn initialization_migrates_v020_newspaper_columns_in_place() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE newspaper_editions (
                    code TEXT NOT NULL, publication_date TEXT NOT NULL DEFAULT '',
                    name_zh TEXT NOT NULL, name_en TEXT NOT NULL, kind TEXT NOT NULL,
                    schedule TEXT NOT NULL, source_url TEXT NOT NULL,
                    active INTEGER NOT NULL DEFAULT 1, discovered INTEGER NOT NULL DEFAULT 0,
                    discovered_at INTEGER, updated_at INTEGER NOT NULL,
                    PRIMARY KEY (code, publication_date)
                );
                CREATE TABLE newspaper_batches (
                    id TEXT PRIMARY KEY NOT NULL, status TEXT NOT NULL, destination TEXT NOT NULL,
                    scheduled_at INTEGER, delay_minutes INTEGER NOT NULL,
                    optimize_images INTEGER NOT NULL, optimization_profile TEXT NOT NULL,
                    keep_original_jpg INTEGER NOT NULL, created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL, completed_at INTEGER
                );
                CREATE TABLE newspaper_jobs (
                    id TEXT PRIMARY KEY NOT NULL, batch_id TEXT NOT NULL,
                    edition_code TEXT NOT NULL, edition_publication_date TEXT NOT NULL DEFAULT '',
                    publication_date TEXT NOT NULL, status TEXT NOT NULL, output_dir TEXT NOT NULL,
                    page_count INTEGER NOT NULL DEFAULT 0,
                    completed_count INTEGER NOT NULL DEFAULT 0,
                    failed_count INTEGER NOT NULL DEFAULT 0,
                    original_bytes INTEGER NOT NULL DEFAULT 0,
                    final_bytes INTEGER NOT NULL DEFAULT 0, warning TEXT,
                    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, completed_at INTEGER,
                    UNIQUE (edition_code, publication_date, output_dir)
                );
                "#,
            )
            .unwrap();

        initialize(&connection).unwrap();

        let batch_columns = connection
            .prepare("PRAGMA table_info(newspaper_batches)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let job_columns = connection
            .prepare("PRAGMA table_info(newspaper_jobs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(batch_columns.contains(&"delay_seconds".to_string()));
        assert!(batch_columns.contains(&"optimization_quality".to_string()));
        assert!(job_columns.contains(&"retry_at".to_string()));
        assert!(job_columns.contains(&"retry_count".to_string()));
        assert!(job_columns.contains(&"queue_position".to_string()));
        assert!(job_columns.contains(&"paused".to_string()));
        assert!(job_columns.contains(&"dismissed".to_string()));

        let queue_index: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_newspaper_jobs_queue'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queue_index, 1);
    }

    #[test]
    fn initialization_expands_existing_quality_constraints_without_losing_data() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE newspaper_batches (
                    id TEXT PRIMARY KEY NOT NULL,
                    status TEXT NOT NULL,
                    destination TEXT NOT NULL,
                    scheduled_at INTEGER,
                    delay_minutes INTEGER NOT NULL,
                    delay_seconds INTEGER NOT NULL DEFAULT 15,
                    optimize_images INTEGER NOT NULL,
                    optimization_profile TEXT NOT NULL,
                    optimization_quality INTEGER NOT NULL DEFAULT 86
                        CHECK (optimization_quality BETWEEN 55 AND 95),
                    keep_original_jpg INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    completed_at INTEGER
                );
                CREATE TABLE newspaper_schedules (
                    id TEXT PRIMARY KEY NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    cron_time TEXT NOT NULL,
                    destination TEXT NOT NULL,
                    edition_codes_json TEXT NOT NULL,
                    delay_seconds INTEGER NOT NULL DEFAULT 15,
                    optimize_images INTEGER NOT NULL,
                    optimization_profile TEXT NOT NULL,
                    optimization_quality INTEGER NOT NULL DEFAULT 86
                        CHECK (optimization_quality BETWEEN 55 AND 95),
                    keep_original_jpg INTEGER NOT NULL,
                    last_run_date TEXT,
                    last_error TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO newspaper_batches
                    (id, status, destination, delay_minutes, optimize_images,
                     optimization_profile, optimization_quality, keep_original_jpg,
                     created_at, updated_at)
                VALUES ('legacy-batch', 'completed', 'C:/papers', 0, 1,
                        'webp_balanced', 74, 0, 1, 1);
                INSERT INTO newspaper_schedules
                    (id, cron_time, destination, edition_codes_json, optimize_images,
                     optimization_profile, optimization_quality, keep_original_jpg,
                     created_at, updated_at)
                VALUES ('legacy-schedule', '07:00', 'C:/papers', '["NY"]', 1,
                        'webp_balanced', 74, 0, 1, 1);
                "#,
            )
            .unwrap();

        initialize(&connection).unwrap();

        connection
            .execute(
                "UPDATE newspaper_batches SET optimization_quality = 25
                 WHERE id = 'legacy-batch'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE newspaper_schedules SET optimization_quality = 25
                 WHERE id = 'legacy-schedule'",
                [],
            )
            .unwrap();
        let values: (i64, i64) = connection
            .query_row(
                "SELECT
                    (SELECT optimization_quality FROM newspaper_batches
                     WHERE id = 'legacy-batch'),
                    (SELECT optimization_quality FROM newspaper_schedules
                     WHERE id = 'legacy-schedule')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(values, (25, 25));
        let foreign_key_violations: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_violations, 0);
        let batch_parent: String = connection
            .query_row(
                "SELECT \"table\" FROM pragma_foreign_key_list('newspaper_jobs')
                 WHERE \"from\" = 'batch_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(batch_parent, "newspaper_batches");
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

    #[test]
    fn restart_preserves_pauses_and_keeps_dismissed_work_cancelled() {
        let connection = initialized();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, optimize_images,
                  optimization_profile, keep_original_jpg, created_at, updated_at)
                 VALUES ('control-batch', 'active', 'C:/papers', 0, 0,
                         'webp_high', 1, 1, 1)",
                [],
            )
            .unwrap();
        for (job_id, paused, dismissed) in
            [("paused-job", true, false), ("dismissed-job", false, true)]
        {
            connection
                .execute(
                    "INSERT INTO newspaper_jobs
                     (id, batch_id, edition_code, publication_date, status, output_dir,
                      paused, dismissed, created_at, updated_at)
                     VALUES (?1, 'control-batch', 'NY', '2026-07-24', 'active', ?2,
                             ?3, ?4, 1, 1)",
                    params![job_id, format!("C:/papers/{job_id}"), paused, dismissed],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                     (id, job_id, page_number, source_url, status, created_at, updated_at)
                     VALUES (?1, ?2, 'A01', 'https://ep.worldjournal.com/A01.jpg',
                             'downloading', 1, 1)",
                    params![format!("{job_id}-page"), job_id],
                )
                .unwrap();
        }

        reconcile_after_restart(&connection, 10).unwrap();

        let paused_state: (String, bool) = connection
            .query_row(
                "SELECT status, paused FROM newspaper_jobs WHERE id = 'paused-job'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let dismissed_state: (String, bool) = connection
            .query_row(
                "SELECT status, dismissed FROM newspaper_jobs WHERE id = 'dismissed-job'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let page_states = connection
            .prepare("SELECT job_id, status FROM newspaper_pages ORDER BY job_id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();

        assert_eq!(paused_state, ("queued".to_string(), true));
        assert_eq!(dismissed_state, ("cancelled".to_string(), true));
        assert_eq!(
            page_states,
            vec![
                ("dismissed-job".to_string(), "cancelled".to_string()),
                ("paused-job".to_string(), "pending".to_string())
            ]
        );
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
            let page_number = format!("A{:02}", index + 1);
            let original_path = output_dir.join(format!("{page_number}.jpg"));
            let payload = format!("validated-page-{index}").into_bytes();
            if *status == "completed" {
                std::fs::write(&original_path, &payload).unwrap();
            }
            connection
                .execute(
                    "INSERT INTO newspaper_pages
                (id, job_id, page_number, source_url, original_path, status,
                 original_bytes, final_bytes, created_at, updated_at)
                VALUES (?1, 'finalize-job', ?2, ?3, ?4, ?5, ?6, ?6, 1, 1)",
                    params![
                        format!("finalize-page-{index}"),
                        page_number,
                        format!("https://ep.worldjournal.com/{index}.jpg"),
                        original_path.to_string_lossy(),
                        status,
                        payload.len() as i64,
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
    fn completed_database_rows_without_files_are_requeued_and_never_marked_complete() {
        let connection = initialized();
        let directory = tempdir().unwrap();
        insert_job_with_pages(&connection, directory.path(), &["completed"]);
        std::fs::remove_file(directory.path().join("A01.jpg")).unwrap();
        std::fs::write(directory.path().join(".complete"), b"stale").unwrap();

        let result = finalize_job(&connection, "finalize-job", 10).unwrap();

        assert_eq!(result.status, "queued");
        assert_eq!(result.completed_count, 0);
        assert!(!directory.path().join(".complete").exists());
        let page_status: String = connection
            .query_row(
                "SELECT status FROM newspaper_pages WHERE job_id = 'finalize-job'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(page_status, "pending");
    }

    #[test]
    fn optimization_must_finish_before_complete_marker_is_written() {
        let connection = initialized();
        let directory = tempdir().unwrap();
        insert_job_with_pages(&connection, directory.path(), &["completed"]);
        connection
            .execute(
                "UPDATE newspaper_batches SET optimize_images = 1 WHERE id = 'finalize-batch'",
                [],
            )
            .unwrap();

        let downloaded = finalize_job(&connection, "finalize-job", 10).unwrap();
        assert_eq!(downloaded.status, "optimizing");
        assert!(!directory.path().join(".complete").exists());

        connection
            .execute(
                "UPDATE newspaper_pages SET optimized_path = original_path
                 WHERE job_id = 'finalize-job'",
                [],
            )
            .unwrap();
        let optimized = finalize_job(&connection, "finalize-job", 11).unwrap();
        assert_eq!(optimized.status, "completed");
        assert!(directory.path().join(".complete").exists());
    }

    #[test]
    fn restart_reconciliation_removes_stale_marker_and_recovers_missing_files() {
        let connection = initialized();
        let directory = tempdir().unwrap();
        insert_job_with_pages(&connection, directory.path(), &["completed"]);
        std::fs::remove_file(directory.path().join("A01.jpg")).unwrap();
        std::fs::write(directory.path().join(".complete"), b"stale").unwrap();
        connection
            .execute(
                "UPDATE newspaper_jobs SET status = 'completed' WHERE id = 'finalize-job'",
                [],
            )
            .unwrap();

        assert_eq!(reconcile_after_restart(&connection, 20).unwrap(), 1);
        assert!(!directory.path().join(".complete").exists());
        let status: String = connection
            .query_row(
                "SELECT status FROM newspaper_jobs WHERE id = 'finalize-job'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "queued");
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

    #[test]
    fn initialization_adds_reader_geometry_and_thumbnail_cache_contract() {
        let connection = initialized();
        let page_columns = connection
            .prepare("PRAGMA table_info(newspaper_pages)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();

        assert!(page_columns.contains(&"pixel_width".to_string()));
        assert!(page_columns.contains(&"pixel_height".to_string()));
        assert!(page_columns.contains(&"media_version".to_string()));

        let thumbnail_columns = connection
            .prepare("PRAGMA table_info(newspaper_thumbnail_cache)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            thumbnail_columns,
            vec![
                "job_id",
                "source_page_id",
                "source_media_version",
                "cache_schema_version",
                "cache_path",
                "mime_type",
                "pixel_width",
                "pixel_height",
                "byte_count",
                "updated_at",
            ]
        );
    }
}
