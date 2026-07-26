//! SQLite reads and row mapping for newspaper jobs.

use rusqlite::{params, Connection, OptionalExtension};

use super::models::NewspaperJob;

const JOB_SELECT: &str = "
    SELECT j.id, j.batch_id, j.edition_code, e.name_zh, j.publication_date,
           j.status, j.output_dir, j.page_count, j.completed_count,
           j.failed_count, j.retry_at, j.retry_count, j.warning,
           j.queue_position, j.paused, j.dismissed, j.created_at,
           j.updated_at, j.completed_at
    FROM newspaper_jobs j
    JOIN newspaper_editions e
      ON e.code = j.edition_code
     AND e.publication_date = j.edition_publication_date";

pub(super) fn list(
    connection: &Connection,
    batch_id: Option<&str>,
) -> Result<Vec<NewspaperJob>, String> {
    let sql = format!(
        "{JOB_SELECT}
         WHERE (?1 IS NULL OR j.batch_id = ?1)
         ORDER BY j.dismissed, j.queue_position, j.created_at DESC"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params![batch_id], row_to_job)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

pub(super) fn find(connection: &Connection, job_id: &str) -> Result<Option<NewspaperJob>, String> {
    let sql = format!("{JOB_SELECT} WHERE j.id = ?1");
    connection
        .query_row(&sql, params![job_id], row_to_job)
        .optional()
        .map_err(|error| error.to_string())
}

pub(super) fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<NewspaperJob> {
    Ok(NewspaperJob {
        id: row.get(0)?,
        batch_id: row.get(1)?,
        edition_code: row.get(2)?,
        edition_name: row.get(3)?,
        publication_date: row.get(4)?,
        status: row.get(5)?,
        output_dir: row.get(6)?,
        page_count: row.get(7)?,
        completed_count: row.get(8)?,
        failed_count: row.get(9)?,
        retry_at: row.get(10)?,
        retry_count: row.get(11)?,
        warning: row.get(12)?,
        queue_position: row.get(13)?,
        paused: row.get(14)?,
        dismissed: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        completed_at: row.get(18)?,
    })
}
