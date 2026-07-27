//! Library query validation, pagination, cache checks, and row mapping.

use std::path::Path;

use rusqlite::params;

use super::{
    job_repository,
    models::{NewspaperJob, NewspaperLibraryItem, NewspaperLibraryPage},
    thumbnails,
};

pub(super) fn list_legacy(
    db_path: &Path,
    query: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<NewspaperJob>, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let bounded_limit = limit.clamp(1, 50);
    let pattern = format!("%{}%", query.unwrap_or_default());
    let mut statement = connection
        .prepare(
            "SELECT j.id, j.batch_id, j.edition_code, e.name_zh, j.publication_date,
                    j.status, j.output_dir, j.page_count, j.completed_count,
                    j.failed_count, j.retry_at, j.retry_count, j.warning,
                    j.queue_position, j.paused, j.dismissed, j.created_at,
                    j.updated_at, j.completed_at
             FROM newspaper_jobs j
             JOIN newspaper_editions e ON e.code = j.edition_code
                 AND e.publication_date = j.edition_publication_date
             WHERE j.status IN ('completed', 'partial') AND j.dismissed = 0
               AND (?1 = '%%' OR e.name_zh LIKE ?1 OR e.name_en LIKE ?1 OR j.edition_code LIKE ?1)
             ORDER BY j.publication_date DESC, j.updated_at DESC
             LIMIT ?2 OFFSET ?3",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(
            params![pattern, bounded_limit, offset],
            job_repository::row_to_job,
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

pub(super) fn validate_query(
    query: &str,
    kind: &str,
    status: &str,
    limit: u32,
) -> Result<(), String> {
    if query.chars().count() > 200
        || !matches!(kind, "all" | "daily" | "weekly" | "special")
        || !matches!(status, "all" | "completed" | "partial" | "optimizing")
        || !(1..=100).contains(&limit)
    {
        return Err("INVALID_LIBRARY_QUERY".to_string());
    }
    Ok(())
}

pub(super) fn query_page(
    db_path: &Path,
    query: &str,
    kind: &str,
    status: &str,
    offset: u32,
    limit: u32,
    revision: u64,
) -> Result<NewspaperLibraryPage, String> {
    let connection =
        crate::cache::open_runtime(db_path).map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let escaped = query
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let total = connection
        .query_row(
            "SELECT COUNT(*)
             FROM newspaper_jobs j
             JOIN newspaper_editions e ON e.code = j.edition_code
                AND e.publication_date = j.edition_publication_date
             WHERE j.status IN ('completed', 'partial', 'optimizing')
               AND j.dismissed = 0
               AND (?1 = 'all' OR e.kind = ?1)
               AND (?2 = 'all' OR j.status = ?2)
               AND (
                   ?3 = '%%'
                   OR e.name_zh LIKE ?3 ESCAPE '\\'
                   OR e.name_en LIKE ?3 ESCAPE '\\'
                   OR j.edition_code LIKE ?3 ESCAPE '\\'
                   OR j.publication_date LIKE ?3 ESCAPE '\\'
               )",
            params![kind, status, pattern],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT
                j.id, j.edition_code, e.name_zh, j.publication_date, j.status,
                j.output_dir, j.page_count, j.completed_count, j.warning, j.updated_at,
                p.last_page_id, p.last_page_index, p.furthest_page_index,
                (SELECT COUNT(*) FROM newspaper_read_pages viewed WHERE viewed.job_id = j.id),
                p.updated_at,
                fp.id, fp.media_version,
                t.cache_path, t.source_page_id, t.source_media_version,
                t.cache_schema_version, t.byte_count
             FROM newspaper_jobs j
             JOIN newspaper_editions e ON e.code = j.edition_code
                AND e.publication_date = j.edition_publication_date
             LEFT JOIN newspaper_reading_progress p ON p.job_id = j.id
             LEFT JOIN newspaper_pages fp ON fp.id = (
                 SELECT first_page.id
                 FROM newspaper_pages first_page
                 WHERE first_page.job_id = j.id
                   AND first_page.status = 'completed'
                   AND COALESCE(first_page.optimized_path, first_page.original_path) IS NOT NULL
                 ORDER BY CASE WHEN first_page.page_number = 'A01' THEN 0 ELSE 1 END,
                          first_page.page_number
                 LIMIT 1
             )
             LEFT JOIN newspaper_thumbnail_cache t ON t.job_id = j.id
             WHERE j.status IN ('completed', 'partial', 'optimizing')
               AND j.dismissed = 0
               AND (?1 = 'all' OR e.kind = ?1)
               AND (?2 = 'all' OR j.status = ?2)
               AND (
                   ?3 = '%%'
                   OR e.name_zh LIKE ?3 ESCAPE '\\'
                   OR e.name_en LIKE ?3 ESCAPE '\\'
                   OR j.edition_code LIKE ?3 ESCAPE '\\'
                   OR j.publication_date LIKE ?3 ESCAPE '\\'
               )
             ORDER BY j.publication_date DESC, j.updated_at DESC, j.id
             LIMIT ?4 OFFSET ?5",
        )
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let rows = statement
        .query_map(params![kind, status, pattern, limit, offset], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<u32>>(11)?,
                row.get::<_, Option<u32>>(12)?,
                row.get::<_, u32>(13)?,
                row.get::<_, Option<i64>>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<i64>>(16)?,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, Option<String>>(18)?,
                row.get::<_, Option<i64>>(19)?,
                row.get::<_, Option<i64>>(20)?,
                row.get::<_, Option<u64>>(21)?,
            ))
        })
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "DATABASE_UNAVAILABLE".to_string())?;
    let items = rows
        .into_iter()
        .map(
            |(
                job_id,
                edition_code,
                edition_name,
                publication_date,
                status,
                output_dir,
                page_count,
                completed_count,
                warning,
                updated_at,
                last_page_id,
                last_page_index,
                furthest_page_index,
                read_page_count,
                reading_updated_at,
                first_page_id,
                first_media_version,
                cache_path,
                cache_page_id,
                cache_media_version,
                cache_schema_version,
                cache_bytes,
            )| {
                let thumbnail_version = first_media_version
                    .zip(cache_schema_version)
                    .map(|(media, schema)| format!("{media}-{schema}"));
                let thumbnail_ready = first_page_id.is_some()
                    && first_page_id == cache_page_id
                    && first_media_version == cache_media_version
                    && cache_schema_version == Some(1)
                    && cache_path
                        .as_ref()
                        .and_then(|path| std::fs::metadata(path).ok())
                        .is_some_and(|metadata| {
                            metadata.is_file()
                                && metadata.len() > 0
                                && Some(metadata.len()) == cache_bytes
                        });
                NewspaperLibraryItem {
                    thumbnail_url: thumbnail_ready.then(|| {
                        thumbnails::thumbnail_url(
                            &job_id,
                            thumbnail_version.as_deref().unwrap_or_default(),
                        )
                    }),
                    thumbnail_version: thumbnail_ready.then_some(thumbnail_version).flatten(),
                    thumbnail_ready,
                    job_id,
                    edition_code,
                    edition_name,
                    publication_date,
                    status,
                    output_dir,
                    page_count,
                    completed_count,
                    warning,
                    updated_at,
                    last_page_id,
                    last_page_index,
                    furthest_page_index,
                    read_page_count,
                    reading_updated_at,
                }
            },
        )
        .collect();
    Ok(NewspaperLibraryPage {
        items,
        total,
        offset,
        limit,
        revision,
    })
}
