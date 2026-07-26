//! Durable page-level optimization task state and crash reconciliation.

use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use rusqlite::{
    params, Connection, OptionalExtension, Result, Transaction, TransactionBehavior,
};

pub(super) const MAX_ATTEMPTS: u32 = 3;
const LEASE_SECONDS: i64 = 120;

#[derive(Debug, Clone)]
pub(super) struct ClaimedTask {
    pub page_id: String,
    pub source_path: PathBuf,
    pub attempts: u32,
    lease_owner: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureDisposition {
    RetryScheduled,
    Failed,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReconcileStats {
    pub reset_leases: usize,
    pub adopted_outputs: usize,
    pub removed_parts: usize,
    pub removed_originals: usize,
}

pub(super) fn ensure_all(connection: &Connection, updated_at: i64) -> Result<usize> {
    let job_ids = connection
        .prepare(
            "SELECT DISTINCT p.job_id
             FROM newspaper_pages p
             JOIN newspaper_jobs j ON j.id = p.job_id
             JOIN newspaper_batches b ON b.id = j.batch_id
             WHERE b.optimize_images = 1
               AND p.status = 'completed'
               AND p.original_path IS NOT NULL",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    let mut inserted = 0;
    for job_id in job_ids {
        inserted += ensure_for_job(connection, &job_id, updated_at)?;
    }
    Ok(inserted)
}

pub(super) fn ensure_for_job(
    connection: &Connection,
    job_id: &str,
    updated_at: i64,
) -> Result<usize> {
    let pages = connection
        .prepare(
            "SELECT id, original_path, optimized_path, checksum, original_bytes, final_bytes
             FROM newspaper_pages
             WHERE job_id = ?1
               AND status = 'completed'
               AND original_path IS NOT NULL",
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
    let mut inserted = 0;

    for (page_id, source, optimized, checksum, original_bytes, final_bytes) in pages {
        let source_path = PathBuf::from(&source);
        let output_path = intended_output_path(&source_path);
        let (source_size, source_modified_at) = source_identity(&source_path);
        let desired_status = match optimized.as_deref() {
            Some(path) if paths_equal(Path::new(path), &source_path) => "kept_original",
            Some(_) => "succeeded",
            None => "pending",
        };
        let existing = connection
            .query_row(
                "SELECT status, source_checksum
                 FROM newspaper_optimization_tasks WHERE page_id = ?1",
                params![page_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;

        if let Some((existing_status, existing_checksum)) = existing {
            let source_changed = existing_checksum != checksum;
            let reset_failed = existing_status == "failed" && source_changed && optimized.is_none();
            let terminal_page = desired_status != "pending";
            connection.execute(
                "UPDATE newspaper_optimization_tasks
                 SET job_id = ?2,
                     status = CASE
                         WHEN ?3 = 1 THEN ?4
                         WHEN ?5 = 1 THEN 'pending'
                         ELSE status
                     END,
                     attempts = CASE WHEN ?5 = 1 THEN 0 ELSE attempts END,
                     lease_owner = CASE WHEN ?3 = 1 OR ?5 = 1 THEN NULL ELSE lease_owner END,
                     lease_expires_at = CASE WHEN ?3 = 1 OR ?5 = 1 THEN NULL ELSE lease_expires_at END,
                     retry_at = CASE WHEN ?3 = 1 OR ?5 = 1 THEN NULL ELSE retry_at END,
                     completed_at = CASE WHEN ?3 = 1 THEN ?6 WHEN ?5 = 1 THEN NULL ELSE completed_at END,
                     source_path = ?7,
                     source_size = ?8,
                     source_modified_at = ?9,
                     source_checksum = ?10,
                     output_path = ?11,
                     source_bytes = COALESCE(?12, source_bytes),
                     output_bytes = CASE WHEN ?3 = 1 THEN COALESCE(?13, output_bytes) ELSE output_bytes END,
                     updated_at = ?6
                 WHERE page_id = ?1",
                params![
                    page_id,
                    job_id,
                    terminal_page,
                    desired_status,
                    reset_failed,
                    updated_at,
                    source,
                    source_size,
                    source_modified_at,
                    checksum,
                    optimized.as_deref().unwrap_or_else(|| output_path.to_str().unwrap_or(&source)),
                    original_bytes,
                    final_bytes,
                ],
            )?;
            continue;
        }

        connection.execute(
            "INSERT INTO newspaper_optimization_tasks
             (page_id, job_id, status, attempts, source_path, source_size,
              source_modified_at, source_checksum, output_path, source_bytes,
              output_bytes, completed_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     CASE WHEN ?3 IN ('succeeded', 'kept_original') THEN ?11 END, ?11)",
            params![
                page_id,
                job_id,
                desired_status,
                source,
                source_size,
                source_modified_at,
                checksum,
                optimized
                    .as_deref()
                    .unwrap_or_else(|| output_path.to_str().unwrap_or(&source)),
                original_bytes,
                final_bytes,
                updated_at,
            ],
        )?;
        inserted += 1;
    }
    Ok(inserted)
}

pub(super) fn reconcile(connection: &Connection, updated_at: i64) -> Result<ReconcileStats> {
    let reset_leases = connection.execute(
        "UPDATE newspaper_optimization_tasks
         SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
             started_at = NULL, retry_at = NULL,
             last_error = CASE
                 WHEN last_error IS NULL THEN 'Recovered after an interrupted optimization.'
                 ELSE last_error
             END,
             error_kind = 'interrupted', updated_at = ?1
         WHERE status = 'running'
           AND COALESCE(lease_expires_at, 0) <= ?1",
        params![updated_at],
    )?;
    let mut stats = ReconcileStats {
        reset_leases,
        ..ReconcileStats::default()
    };

    let tasks = connection
        .prepare(
            "SELECT t.page_id, t.status, t.source_path, t.output_path,
                    t.source_size, t.source_modified_at,
                    p.optimized_path, p.original_bytes,
                    b.keep_original_jpg
             FROM newspaper_optimization_tasks t
             JOIN newspaper_pages p ON p.id = t.page_id
             JOIN newspaper_jobs j ON j.id = t.job_id
             JOIN newspaper_batches b ON b.id = j.batch_id
             WHERE t.status IN ('pending', 'succeeded')
                OR (t.status = 'running' AND COALESCE(t.lease_expires_at, 0) <= ?1)",
        )?
        .query_map(params![updated_at], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, bool>(8)?,
            ))
        })?
        .collect::<Result<Vec<_>>>()?;

    for (
        page_id,
        status,
        source,
        output,
        source_size,
        source_modified_at,
        committed_output,
        original_bytes,
        keep_original,
    ) in tasks
    {
        let source_path = PathBuf::from(&source);
        let output_path = output
            .map(PathBuf::from)
            .unwrap_or_else(|| intended_output_path(&source_path));

        if status == "pending" {
            let part = part_path(&output_path);
            if part.exists() && std::fs::remove_file(&part).is_ok() {
                stats.removed_parts += 1;
            }
            if let Some(committed) = committed_output {
                let terminal = if paths_equal(Path::new(&committed), &source_path) {
                    "kept_original"
                } else {
                    "succeeded"
                };
                connection.execute(
                    "UPDATE newspaper_optimization_tasks
                     SET status = ?2, output_path = ?3, lease_owner = NULL,
                         lease_expires_at = NULL, retry_at = NULL,
                         completed_at = ?4, last_error = NULL, error_kind = NULL,
                         updated_at = ?4
                     WHERE page_id = ?1",
                    params![page_id, terminal, committed, updated_at],
                )?;
                if terminal == "succeeded"
                    && cleanup_original(
                        connection,
                        &page_id,
                        &source_path,
                        Path::new(&committed),
                        keep_original,
                        updated_at,
                    )?
                {
                    stats.removed_originals += 1;
                }
                continue;
            }

            if valid_orphan_output(&source_path, &output_path, source_size, source_modified_at) {
                let output_bytes = std::fs::metadata(&output_path)
                    .map(|value| value.len() as i64)
                    .unwrap_or(0);
                commit_replaced_by_id(
                    connection,
                    &page_id,
                    &output_path,
                    original_bytes,
                    output_bytes,
                    0,
                    updated_at,
                    None,
                )?;
                stats.adopted_outputs += 1;
                if cleanup_original(
                    connection,
                    &page_id,
                    &source_path,
                    &output_path,
                    keep_original,
                    updated_at,
                )? {
                    stats.removed_originals += 1;
                }
            }
        } else if status == "succeeded"
            && cleanup_original(
                connection,
                &page_id,
                &source_path,
                &output_path,
                keep_original,
                updated_at,
            )?
        {
            stats.removed_originals += 1;
        }
    }
    Ok(stats)
}

pub(super) fn claim_next(
    connection: &mut Connection,
    job_id: &str,
    lease_owner: &str,
    now: i64,
) -> Result<Option<ClaimedTask>> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let candidate = transaction
        .query_row(
            "SELECT t.page_id, t.source_path, t.attempts
             FROM newspaper_optimization_tasks t
             JOIN newspaper_pages p ON p.id = t.page_id
             JOIN newspaper_jobs j ON j.id = t.job_id
             WHERE t.job_id = ?1
               AND t.status = 'pending'
               AND t.attempts < ?2
               AND (t.retry_at IS NULL OR t.retry_at <= ?3)
               AND p.status = 'completed'
               AND p.optimized_path IS NULL
               AND j.paused = 0
               AND j.dismissed = 0
             ORDER BY p.page_number
             LIMIT 1",
            params![job_id, MAX_ATTEMPTS, now],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((page_id, source_path, attempts)) = candidate else {
        transaction.commit()?;
        return Ok(None);
    };
    let attempts = attempts.saturating_add(1);
    let updated = transaction.execute(
        "UPDATE newspaper_optimization_tasks
         SET status = 'running', attempts = ?2, lease_owner = ?3,
             lease_expires_at = ?4, retry_at = NULL, started_at = ?5,
             completed_at = NULL, updated_at = ?5
         WHERE page_id = ?1 AND status = 'pending'",
        params![
            page_id,
            attempts,
            lease_owner,
            now.saturating_add(LEASE_SECONDS),
            now
        ],
    )?;
    if updated != 1 {
        transaction.commit()?;
        return Ok(None);
    }
    transaction.commit()?;
    Ok(Some(ClaimedTask {
        page_id,
        source_path: PathBuf::from(source_path),
        attempts,
        lease_owner: lease_owner.to_string(),
    }))
}

pub(super) fn complete_replaced(
    connection: &mut Connection,
    task: &ClaimedTask,
    output_path: &Path,
    output_bytes: u64,
    elapsed_ms: u64,
    updated_at: i64,
) -> Result<()> {
    commit_replaced_by_id(
        connection,
        &task.page_id,
        output_path,
        None,
        output_bytes as i64,
        elapsed_ms as i64,
        updated_at,
        Some(&task.lease_owner),
    )
}

pub(super) fn complete_kept_original(
    connection: &mut Connection,
    task: &ClaimedTask,
    bytes: u64,
    elapsed_ms: u64,
    updated_at: i64,
) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_task_owner(&transaction, task)?;
    transaction.execute(
        "UPDATE newspaper_pages
         SET optimized_path = original_path, final_bytes = ?2, updated_at = ?3
         WHERE id = ?1",
        params![task.page_id, bytes, updated_at],
    )?;
    transaction.execute(
        "UPDATE newspaper_optimization_tasks
         SET status = 'kept_original', output_path = source_path,
             output_bytes = ?2, elapsed_ms = ?3, lease_owner = NULL,
             lease_expires_at = NULL, retry_at = NULL, completed_at = ?4,
             last_error = NULL, error_kind = NULL, updated_at = ?4
         WHERE page_id = ?1",
        params![task.page_id, bytes, elapsed_ms, updated_at],
    )?;
    transaction.commit()
}

pub(super) fn complete_failure(
    connection: &mut Connection,
    task: &ClaimedTask,
    message: &str,
    error_kind: &str,
    retryable: bool,
    elapsed_ms: u64,
    updated_at: i64,
) -> Result<FailureDisposition> {
    if retryable && task.attempts < MAX_ATTEMPTS {
        let retry_at = updated_at.saturating_add(i64::from(task.attempts) * 15);
        let updated = connection.execute(
            "UPDATE newspaper_optimization_tasks
             SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
                 retry_at = ?2, elapsed_ms = ?3, last_error = ?4,
                 error_kind = ?5, updated_at = ?6
             WHERE page_id = ?1 AND status = 'running' AND lease_owner = ?7",
            params![
                task.page_id,
                retry_at,
                elapsed_ms,
                message,
                error_kind,
                updated_at,
                task.lease_owner,
            ],
        )?;
        if updated != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        return Ok(FailureDisposition::RetryScheduled);
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_task_owner(&transaction, task)?;
    transaction.execute(
        "UPDATE newspaper_pages
         SET optimized_path = original_path,
             final_bytes = COALESCE(original_bytes, final_bytes),
             error = ?2, updated_at = ?3
         WHERE id = ?1",
        params![task.page_id, message, updated_at],
    )?;
    transaction.execute(
        "UPDATE newspaper_optimization_tasks
         SET status = 'failed', output_path = NULL, elapsed_ms = ?2,
             lease_owner = NULL, lease_expires_at = NULL, retry_at = NULL,
             completed_at = ?3, last_error = ?4, error_kind = ?5,
             updated_at = ?3
         WHERE page_id = ?1",
        params![task.page_id, elapsed_ms, updated_at, message, error_kind],
    )?;
    transaction.commit()?;
    Ok(FailureDisposition::Failed)
}

pub(super) fn cleanup_completed_source(
    connection: &Connection,
    task: &ClaimedTask,
    output_path: &Path,
    keep_original: bool,
    updated_at: i64,
) -> Result<Option<String>> {
    if keep_original
        || paths_equal(&task.source_path, output_path)
        || !task.source_path.exists()
        || !output_path.is_file()
    {
        return Ok(None);
    }
    match std::fs::remove_file(&task.source_path) {
        Ok(()) => Ok(None),
        Err(error) => {
            connection.execute(
                "UPDATE newspaper_optimization_tasks
                 SET last_error = ?2, error_kind = 'cleanup', updated_at = ?3
                 WHERE page_id = ?1",
                params![task.page_id, error.to_string(), updated_at],
            )?;
            Ok(Some(format!(
                "Could not remove original {}: {error}",
                task.source_path.display()
            )))
        }
    }
}

fn commit_replaced_by_id(
    connection: &Connection,
    page_id: &str,
    output_path: &Path,
    source_bytes: impl Into<Option<i64>>,
    output_bytes: i64,
    elapsed_ms: i64,
    updated_at: i64,
    expected_lease_owner: Option<&str>,
) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    if let Some(lease_owner) = expected_lease_owner {
        let owned: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM newspaper_optimization_tasks
             WHERE page_id = ?1 AND status = 'running' AND lease_owner = ?2",
            params![page_id, lease_owner],
            |row| row.get(0),
        )?;
        if owned != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
    }
    transaction.execute(
        "UPDATE newspaper_pages
         SET optimized_path = ?2, final_bytes = ?3,
             media_version = media_version + 1, error = NULL, updated_at = ?4
         WHERE id = ?1",
        params![
            page_id,
            output_path.to_string_lossy(),
            output_bytes,
            updated_at
        ],
    )?;
    transaction.execute(
        "UPDATE newspaper_optimization_tasks
         SET status = 'succeeded', output_path = ?2,
             source_bytes = COALESCE(?3, source_bytes),
             output_bytes = ?4, elapsed_ms = ?5,
             lease_owner = NULL, lease_expires_at = NULL, retry_at = NULL,
             completed_at = ?6, last_error = NULL, error_kind = NULL,
             updated_at = ?6
         WHERE page_id = ?1",
        params![
            page_id,
            output_path.to_string_lossy(),
            source_bytes.into(),
            output_bytes,
            elapsed_ms,
            updated_at
        ],
    )?;
    transaction.commit()
}

fn verify_task_owner(transaction: &Transaction<'_>, task: &ClaimedTask) -> Result<()> {
    let owned: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM newspaper_optimization_tasks
         WHERE page_id = ?1 AND status = 'running' AND lease_owner = ?2",
        params![task.page_id, task.lease_owner],
        |row| row.get(0),
    )?;
    if owned == 1 {
        Ok(())
    } else {
        Err(rusqlite::Error::QueryReturnedNoRows)
    }
}

fn cleanup_original(
    connection: &Connection,
    page_id: &str,
    source_path: &Path,
    output_path: &Path,
    keep_original: bool,
    updated_at: i64,
) -> Result<bool> {
    if keep_original
        || paths_equal(source_path, output_path)
        || !source_path.exists()
        || !output_path.is_file()
    {
        return Ok(false);
    }
    if let Err(error) = std::fs::remove_file(source_path) {
        connection.execute(
            "UPDATE newspaper_optimization_tasks
             SET last_error = ?2, error_kind = 'cleanup', updated_at = ?3
             WHERE page_id = ?1",
            params![page_id, error.to_string(), updated_at],
        )?;
        return Ok(false);
    }
    Ok(true)
}

fn valid_orphan_output(
    source_path: &Path,
    output_path: &Path,
    expected_source_size: Option<i64>,
    expected_source_modified_at: Option<i64>,
) -> bool {
    if paths_equal(source_path, output_path) || !source_path.is_file() || !output_path.is_file() {
        return false;
    }
    let (source_size, source_modified_at) = source_identity(source_path);
    if expected_source_size.is_some() && source_size != expected_source_size {
        return false;
    }
    if expected_source_modified_at.is_some() && source_modified_at != expected_source_modified_at {
        return false;
    }
    let Ok(source_dimensions) = image::image_dimensions(source_path) else {
        return false;
    };
    let Ok(output_dimensions) = image::image_dimensions(output_path) else {
        return false;
    };
    let Ok(output_size) = std::fs::metadata(output_path).map(|value| value.len() as i64) else {
        return false;
    };
    source_dimensions == output_dimensions
        && output_size > 0
        && source_size.is_some_and(|value| output_size < value)
}

fn source_identity(path: &Path) -> (Option<i64>, Option<i64>) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (None, None);
    };
    let size = i64::try_from(metadata.len()).ok();
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_secs()).ok());
    (size, modified)
}

fn intended_output_path(source: &Path) -> PathBuf {
    source.with_extension("webp")
}

fn part_path(output: &Path) -> PathBuf {
    output.with_extension("webp.part")
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use image::{ImageBuffer, Rgb};
    use tempfile::TempDir;

    use super::*;

    struct Fixture {
        _directory: TempDir,
        connection: Connection,
        source: PathBuf,
        output: PathBuf,
    }

    fn fixture(keep_original: bool) -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("A01.jpg");
        let image = ImageBuffer::from_fn(480, 640, |x, y| {
            let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
            Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        });
        image
            .save_with_format(&source, image::ImageFormat::Jpeg)
            .unwrap();
        let source_bytes = std::fs::metadata(&source).unwrap().len();
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        crate::newspaper::storage::initialize(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_editions
                 (code, name_zh, name_en, kind, schedule, source_url, updated_at)
                 VALUES ('TEST', 'Test', 'Test', 'daily', 'daily', 'test://edition', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, delay_seconds,
                  optimize_images, optimization_profile, optimization_quality,
                  keep_original_jpg, created_at, updated_at)
                 VALUES ('batch', 'completed', ?1, 0, 0, 1, 'webp_balanced',
                         25, ?2, 1, 1)",
                params![directory.path().to_string_lossy(), keep_original],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_jobs
                 (id, batch_id, edition_code, publication_date, status, output_dir,
                  page_count, completed_count, created_at, updated_at)
                 VALUES ('job', 'batch', 'TEST', '2026-07-26', 'optimizing', ?1,
                         1, 1, 1, 1)",
                params![directory.path().to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, status,
                  original_bytes, final_bytes, checksum, created_at, updated_at)
                 VALUES ('page', 'job', 'A01', 'test://page', ?1, 'completed',
                         ?2, ?2, 'checksum', 1, 1)",
                params![source.to_string_lossy(), source_bytes],
            )
            .unwrap();
        ensure_for_job(&connection, "job", 1).unwrap();
        let output = source.with_extension("webp");
        Fixture {
            _directory: directory,
            connection,
            source,
            output,
        }
    }

    #[test]
    fn expired_lease_returns_to_pending_and_stale_part_is_removed() {
        let fixture = fixture(true);
        let part = part_path(&fixture.output);
        std::fs::write(&part, b"partial").unwrap();
        fixture
            .connection
            .execute(
                "UPDATE newspaper_optimization_tasks
                 SET status = 'running', lease_owner = 'dead-worker',
                     lease_expires_at = 5
                 WHERE page_id = 'page'",
                [],
            )
            .unwrap();

        let stats = reconcile(&fixture.connection, 10).unwrap();

        let state: (String, Option<String>, Option<i64>) = fixture
            .connection
            .query_row(
                "SELECT status, lease_owner, lease_expires_at
                 FROM newspaper_optimization_tasks WHERE page_id = 'page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, ("pending".to_string(), None, None));
        assert!(!part.exists());
        assert_eq!(stats.reset_leases, 1);
        assert_eq!(stats.removed_parts, 1);
    }

    #[test]
    fn unexpired_lease_is_not_stolen_by_restart_reconciliation() {
        let fixture = fixture(true);
        fixture
            .connection
            .execute(
                "UPDATE newspaper_optimization_tasks
                 SET status = 'running', lease_owner = 'live-worker',
                     lease_expires_at = 100
                 WHERE page_id = 'page'",
                [],
            )
            .unwrap();

        let stats = reconcile(&fixture.connection, 50).unwrap();

        let state: (String, Option<String>) = fixture
            .connection
            .query_row(
                "SELECT status, lease_owner
                 FROM newspaper_optimization_tasks WHERE page_id = 'page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("running".to_string(), Some("live-worker".to_string()))
        );
        assert_eq!(stats.reset_leases, 0);
    }

    #[test]
    fn valid_output_left_before_database_commit_is_adopted() {
        let fixture = fixture(false);
        let outcome = crate::newspaper::optimizer::optimize_page(&fixture.source, 25).unwrap();
        assert!(matches!(
            outcome,
            crate::newspaper::optimizer::OptimizationOutcome::Replaced { .. }
        ));
        fixture
            .connection
            .execute(
                "UPDATE newspaper_optimization_tasks
                 SET status = 'running', lease_owner = 'crashed-worker',
                     lease_expires_at = 5
                 WHERE page_id = 'page'",
                [],
            )
            .unwrap();

        let stats = reconcile(&fixture.connection, 10).unwrap();

        let state: (String, Option<String>) = fixture
            .connection
            .query_row(
                "SELECT t.status, p.optimized_path
                 FROM newspaper_optimization_tasks t
                 JOIN newspaper_pages p ON p.id = t.page_id
                 WHERE t.page_id = 'page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state.0, "succeeded");
        assert_eq!(state.1.as_deref(), fixture.output.to_str());
        assert!(fixture.output.is_file());
        assert!(!fixture.source.exists());
        assert_eq!(stats.adopted_outputs, 1);
        assert_eq!(stats.removed_originals, 1);
    }

    #[test]
    fn retryable_failure_waits_before_the_next_claim() {
        let mut fixture = fixture(true);
        let first = claim_next(&mut fixture.connection, "job", "worker", 10)
            .unwrap()
            .unwrap();
        assert_eq!(first.attempts, 1);
        assert_eq!(
            complete_failure(
                &mut fixture.connection,
                &first,
                "temporary error",
                "io",
                true,
                5,
                10,
            )
            .unwrap(),
            FailureDisposition::RetryScheduled
        );
        assert!(claim_next(&mut fixture.connection, "job", "worker", 20)
            .unwrap()
            .is_none());
        let second = claim_next(&mut fixture.connection, "job", "worker", 25)
            .unwrap()
            .unwrap();
        assert_eq!(second.attempts, 2);
    }

    #[test]
    fn permanent_failure_preserves_the_original_as_reader_fallback() {
        let mut fixture = fixture(true);
        let task = claim_next(&mut fixture.connection, "job", "worker", 10)
            .unwrap()
            .unwrap();
        assert_eq!(
            complete_failure(
                &mut fixture.connection,
                &task,
                "invalid image",
                "invalid_image",
                false,
                5,
                10,
            )
            .unwrap(),
            FailureDisposition::Failed
        );
        let state: (String, Option<String>, Option<String>) = fixture
            .connection
            .query_row(
                "SELECT t.status, p.optimized_path, t.last_error
                 FROM newspaper_optimization_tasks t
                 JOIN newspaper_pages p ON p.id = t.page_id
                 WHERE t.page_id = 'page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state.0, "failed");
        assert_eq!(state.1.as_deref(), fixture.source.to_str());
        assert_eq!(state.2.as_deref(), Some("invalid image"));
        assert!(fixture.source.is_file());
    }
}
