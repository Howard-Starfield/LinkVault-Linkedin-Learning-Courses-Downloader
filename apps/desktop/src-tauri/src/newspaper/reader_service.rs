//! Local-first newspaper reader queries and reading-progress persistence.
//!
//! This module returns metadata and protocol URLs only. It must not decode or scan
//! newspaper page files while opening the reader.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::{
    models::{NewspaperPage, NewspaperReadingProgress},
    thumbnails,
};

pub(super) fn manifest(db_path: &Path, job_id: &str) -> Result<Vec<NewspaperPage>, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT id, job_id, page_number, section_name, source_url,
                    COALESCE(optimized_path, original_path), status, final_bytes, checksum, error,
                    pixel_width, pixel_height, media_version
             FROM newspaper_pages WHERE job_id = ?1 ORDER BY page_number",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params![job_id], |row| {
            let status = row.get::<_, String>(6)?;
            let page_id = row.get::<_, String>(0)?;
            let media_version = row.get::<_, i64>(12)?;
            Ok(NewspaperPage {
                id: page_id.clone(),
                job_id: row.get(1)?,
                page_number: row.get(2)?,
                section_name: row.get(3)?,
                source_url: row.get(4)?,
                display_path: row.get(5)?,
                status: status.clone(),
                final_bytes: row.get(7)?,
                checksum: row.get(8)?,
                error: row.get(9)?,
                canonical_index: 0,
                media_url: (status == "completed")
                    .then(|| thumbnails::page_url(&page_id, media_version)),
                pixel_width: row.get(10)?,
                pixel_height: row.get(11)?,
                media_version,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(result
        .into_iter()
        .enumerate()
        .map(|(index, mut page)| {
            page.canonical_index = index as u32;
            page
        })
        .collect())
}

pub(super) fn save_progress(
    connection: &Connection,
    job_id: &str,
    page_id: &str,
    updated_at: i64,
) -> Result<NewspaperReadingProgress, String> {
    let page_index = canonical_page_index(connection, job_id, page_id)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let viewed = transaction
        .execute(
            "INSERT OR IGNORE INTO newspaper_read_pages (job_id, page_id, page_index, viewed_at)
             SELECT ?1, ?2, ?3, ?4
             WHERE EXISTS (
                 SELECT 1 FROM newspaper_pages
                 WHERE id = ?2 AND job_id = ?1 AND status = 'completed'
             )",
            params![job_id, page_id, page_index, updated_at],
        )
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "INSERT INTO newspaper_reading_progress (
                job_id, last_page_id, last_page_index, furthest_page_index, updated_at
             )
             SELECT ?1, ?2, ?3, ?3, ?4
             WHERE EXISTS (
                 SELECT 1 FROM newspaper_pages
                 WHERE id = ?2 AND job_id = ?1 AND status = 'completed'
             )
             ON CONFLICT(job_id) DO UPDATE SET
                 last_page_id = excluded.last_page_id,
                 last_page_index = excluded.last_page_index,
                 furthest_page_index = MAX(
                     newspaper_reading_progress.furthest_page_index,
                     excluded.furthest_page_index
                 ),
                 updated_at = excluded.updated_at",
            params![job_id, page_id, page_index, updated_at],
        )
        .map_err(|error| error.to_string())?;
    if changed == 0 && viewed == 0 {
        return Err(
            "Reading progress can only be saved for a completed page in this newspaper."
                .to_string(),
        );
    }
    let progress = progress_for_job(&transaction, job_id)?
        .ok_or_else(|| "Reading progress was not saved.".to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(progress)
}

pub(super) fn list_progress(
    connection: &Connection,
) -> Result<Vec<NewspaperReadingProgress>, String> {
    let mut statement = connection
        .prepare(
            "SELECT p.job_id, p.last_page_id, p.last_page_index, p.furthest_page_index,
                    (SELECT COUNT(*) FROM newspaper_read_pages viewed WHERE viewed.job_id = p.job_id),
                    p.updated_at
             FROM newspaper_reading_progress p
             ORDER BY p.updated_at DESC",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map([], row_to_progress)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

fn canonical_page_index(
    connection: &Connection,
    job_id: &str,
    page_id: &str,
) -> Result<u32, String> {
    connection
        .query_row(
            "SELECT COUNT(*)
             FROM newspaper_pages
             WHERE job_id = ?1
               AND page_number < (
                   SELECT page_number FROM newspaper_pages
                   WHERE id = ?2 AND job_id = ?1 AND status = 'completed'
               )",
            params![job_id, page_id],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|error| error.to_string())
}

fn progress_for_job(
    connection: &Connection,
    job_id: &str,
) -> Result<Option<NewspaperReadingProgress>, String> {
    connection
        .query_row(
            "SELECT p.job_id, p.last_page_id, p.last_page_index, p.furthest_page_index,
                    (SELECT COUNT(*) FROM newspaper_read_pages viewed WHERE viewed.job_id = p.job_id),
                    p.updated_at
             FROM newspaper_reading_progress p WHERE p.job_id = ?1",
            params![job_id],
            row_to_progress,
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn row_to_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<NewspaperReadingProgress> {
    Ok(NewspaperReadingProgress {
        job_id: row.get(0)?,
        last_page_id: row.get(1)?,
        last_page_index: row.get(2)?,
        furthest_page_index: row.get(3)?,
        read_page_count: row.get(4)?,
        updated_at: row.get(5)?,
    })
}
