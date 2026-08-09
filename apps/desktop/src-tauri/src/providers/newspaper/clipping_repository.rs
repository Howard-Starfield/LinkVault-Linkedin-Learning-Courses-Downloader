//! Provider-owned SQLite primitives for the clipping aggregate
//! (specification 02 sections 9-17, FR-DOMAIN-001).
//!
//! Every state-changing statement here executes inside a `DatabaseWriter`
//! closure submitted by the clipping service (AC-PERSIST-007). The module
//! never opens connections, owns transactions, or touches the filesystem.
//! Source-unlink helpers additionally serve the legacy reset and single-job
//! deletion transactions (FR-SOURCE-DELETE-001).

use rusqlite::{params, Connection, OptionalExtension, Result};

use super::clipping_models::{
    escape_like_pattern, ClippingAssetState, ClippingSourceKind, NewspaperClipping,
    NewspaperClippingListQuery, NewspaperClippingSort,
};

/// Registration payload for the `creating` row (CREATE-STATE-002). The
/// canonical asset must already be staged and validated before insert.
#[derive(Clone, Debug)]
pub struct NewClippingRecord {
    pub id: String,
    pub source_job_id: Option<String>,
    pub source_page_id: Option<String>,
    pub source_media_version_snapshot: i64,
    pub source_kind_snapshot: ClippingSourceKind,
    pub source_mime_type_snapshot: String,
    pub source_checksum_snapshot: Option<String>,
    pub edition_code_snapshot: String,
    pub edition_name_snapshot: String,
    pub publication_date_snapshot: String,
    pub page_number_snapshot: String,
    pub source_pixel_width: u32,
    pub source_pixel_height: u32,
    pub crop_x: u32,
    pub crop_y: u32,
    pub crop_width: u32,
    pub crop_height: u32,
    pub asset_relative_path: String,
    pub asset_byte_count: u64,
    pub asset_checksum_sha256: String,
    pub title: String,
    pub now: i64,
}

const CLIPPING_COLUMNS: &str = "id, source_job_id, source_page_id,
    source_media_version_snapshot, source_kind_snapshot, source_mime_type_snapshot,
    source_checksum_snapshot, edition_code_snapshot, edition_name_snapshot,
    publication_date_snapshot, page_number_snapshot, source_pixel_width,
    source_pixel_height, crop_x, crop_y, crop_width, crop_height,
    asset_relative_path, asset_mime_type, asset_pixel_width, asset_pixel_height,
    asset_byte_count, asset_checksum_sha256, asset_version, asset_state,
    asset_error_code, title, note_markdown, revision, created_at, updated_at";

pub fn insert_creating(connection: &Connection, record: &NewClippingRecord) -> Result<()> {
    connection.execute(
        &format!(
            "INSERT INTO newspaper_clippings ({CLIPPING_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?16, ?17, ?18, 'image/webp', ?16, ?17, ?19, ?20,
                     1, 'creating', NULL, ?21, '', 1, ?22, ?22)"
        ),
        params![
            record.id,
            record.source_job_id,
            record.source_page_id,
            record.source_media_version_snapshot,
            record.source_kind_snapshot.as_sql(),
            record.source_mime_type_snapshot,
            record.source_checksum_snapshot,
            record.edition_code_snapshot,
            record.edition_name_snapshot,
            record.publication_date_snapshot,
            record.page_number_snapshot,
            record.source_pixel_width,
            record.source_pixel_height,
            record.crop_x,
            record.crop_y,
            record.crop_width,
            record.crop_height,
            record.asset_relative_path,
            record.asset_byte_count,
            record.asset_checksum_sha256,
            record.title,
            record.now,
        ],
    )?;
    Ok(())
}

pub fn row_state(connection: &Connection, id: &str) -> Result<Option<ClippingAssetState>> {
    connection
        .query_row(
            "SELECT asset_state FROM newspaper_clippings WHERE id = ?1",
            params![id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map(|value| value.and_then(|state| ClippingAssetState::from_sql(&state)))
}

/// Read-only source snapshot used by the Phase 2 crop service. This joins the
/// authoritative page, job, and edition projection once; callers never derive
/// provenance or filesystem locations from IPC input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CropSourceRecord {
    pub page_id: String,
    pub job_id: String,
    pub page_number: String,
    pub page_status: String,
    pub original_path: Option<String>,
    pub optimized_path: Option<String>,
    pub stored_pixel_width: Option<u32>,
    pub stored_pixel_height: Option<u32>,
    pub media_version: i64,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub output_dir: String,
}

/// Loads the complete provider-owned source projection needed for one crop.
/// The `JOIN`s intentionally require an extant job and catalog edition; a
/// missing relationship is indistinguishable from an ineligible page to the
/// crop command and is handled as a typed source failure by the service.
pub fn load_crop_source(
    connection: &Connection,
    page_id: &str,
) -> Result<Option<CropSourceRecord>> {
    connection
        .query_row(
            "SELECT p.id, p.job_id, p.page_number, p.status, p.original_path,
                    p.optimized_path, p.pixel_width, p.pixel_height, p.media_version,
                    j.edition_code,
                    COALESCE(NULLIF(e.name_zh, ''), NULLIF(e.name_en, '')),
                    j.publication_date, j.output_dir
             FROM newspaper_pages p
             JOIN newspaper_jobs j ON j.id = p.job_id
             JOIN newspaper_editions e
               ON e.code = j.edition_code
              AND e.publication_date = j.edition_publication_date
             WHERE p.id = ?1",
            params![page_id],
            |row| {
                Ok(CropSourceRecord {
                    page_id: row.get(0)?,
                    job_id: row.get(1)?,
                    page_number: row.get(2)?,
                    page_status: row.get(3)?,
                    original_path: row.get(4)?,
                    optimized_path: row.get(5)?,
                    stored_pixel_width: row.get(6)?,
                    stored_pixel_height: row.get(7)?,
                    media_version: row.get(8)?,
                    edition_code: row.get(9)?,
                    edition_name: row.get(10)?,
                    publication_date: row.get(11)?,
                    output_dir: row.get(12)?,
                })
            },
        )
        .optional()
}

fn map_clipping_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NewspaperClipping> {
    let source_kind = row.get::<_, String>(3)?;
    let asset_state = row.get::<_, String>(24)?;
    Ok(NewspaperClipping {
        id: row.get(0)?,
        source_job_id: row.get(1)?,
        source_page_id: row.get(2)?,
        source_media_version_snapshot: row.get(4)?,
        source_kind_snapshot: ClippingSourceKind::from_sql(&source_kind)
            .unwrap_or(ClippingSourceKind::Optimized),
        source_mime_type_snapshot: row.get(5)?,
        source_checksum_snapshot: row.get(6)?,
        edition_code_snapshot: row.get(7)?,
        edition_name_snapshot: row.get(8)?,
        publication_date_snapshot: row.get(9)?,
        page_number_snapshot: row.get(10)?,
        source_pixel_width: row.get(11)?,
        source_pixel_height: row.get(12)?,
        crop_x: row.get(13)?,
        crop_y: row.get(14)?,
        crop_width: row.get(15)?,
        crop_height: row.get(16)?,
        asset_relative_path: row.get(17)?,
        asset_mime_type: row.get(18)?,
        asset_pixel_width: row.get(19)?,
        asset_pixel_height: row.get(20)?,
        asset_byte_count: row.get(21)?,
        asset_checksum_sha256: row.get(22)?,
        asset_version: row.get(23)?,
        asset_state: ClippingAssetState::from_sql(&asset_state)
            .unwrap_or(ClippingAssetState::Missing),
        asset_error_code: row.get(25)?,
        title: row.get(26)?,
        note_markdown: row.get(27)?,
        revision: row.get(28)?,
        created_at: row.get(29)?,
        updated_at: row.get(30)?,
    })
}

const CLIPPING_SELECT: &str = "SELECT id, source_job_id, source_page_id,
    source_kind_snapshot, source_media_version_snapshot, source_mime_type_snapshot,
    source_checksum_snapshot, edition_code_snapshot, edition_name_snapshot,
    publication_date_snapshot, page_number_snapshot, source_pixel_width,
    source_pixel_height, crop_x, crop_y, crop_width, crop_height,
    asset_relative_path, asset_mime_type, asset_pixel_width, asset_pixel_height,
    asset_byte_count, asset_checksum_sha256, asset_version, asset_state,
    asset_error_code, title, note_markdown, revision, created_at, updated_at";

pub fn load_by_id(connection: &Connection, id: &str) -> Result<Option<NewspaperClipping>> {
    connection
        .query_row(
            &format!("{CLIPPING_SELECT} FROM newspaper_clippings WHERE id = ?1"),
            params![id],
            map_clipping_row,
        )
        .optional()
}

pub fn load_public_by_id(connection: &Connection, id: &str) -> Result<Option<NewspaperClipping>> {
    connection
        .query_row(
            &format!(
                "{CLIPPING_SELECT} FROM newspaper_clippings
                 WHERE id = ?1 AND asset_state IN ('ready', 'missing')"
            ),
            params![id],
            map_clipping_row,
        )
        .optional()
}

pub fn mark_ready_from_creating(connection: &Connection, id: &str, now: i64) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE newspaper_clippings
         SET asset_state = 'ready', asset_error_code = NULL, updated_at = ?2
         WHERE id = ?1 AND asset_state = 'creating'",
        params![id, now],
    )?;
    Ok(changed == 1)
}

pub fn mark_missing_from_creating(
    connection: &Connection,
    id: &str,
    error_code: &str,
    now: i64,
) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE newspaper_clippings
         SET asset_state = 'missing', asset_error_code = ?2, updated_at = ?3
         WHERE id = ?1 AND asset_state = 'creating'",
        params![id, error_code, now],
    )?;
    Ok(changed == 1)
}

/// Integrity transition for rows that were `ready` but whose canonical asset
/// failed validation on access (D-028 missing-asset state). Notes, revision,
/// and provenance remain untouched except for `updated_at`.
pub fn mark_missing_from_ready(
    connection: &Connection,
    id: &str,
    error_code: &str,
    now: i64,
) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE newspaper_clippings
         SET asset_state = 'missing', asset_error_code = ?2, updated_at = ?3
         WHERE id = ?1 AND asset_state = 'ready'",
        params![id, error_code, now],
    )?;
    Ok(changed == 1)
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoteUpdateOutcome {
    Updated { clipping: NewspaperClipping },
    Unchanged { clipping: NewspaperClipping },
    NotFound,
    Conflict { current_revision: u64 },
    NotEditable,
}

/// Optimistic revision update (FR-UPDATE-001..005, D-018). Validation happens
/// before the writer submission; this primitive only applies SQL semantics.
pub fn update_note(
    connection: &Connection,
    id: &str,
    expected_revision: u64,
    title: &str,
    note_markdown: &str,
    now: i64,
) -> Result<NoteUpdateOutcome> {
    let current = connection
        .query_row(
            "SELECT asset_state, revision, title, note_markdown
             FROM newspaper_clippings WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((state, revision, stored_title, stored_note)) = current else {
        return Ok(NoteUpdateOutcome::NotFound);
    };
    let Some(state) = ClippingAssetState::from_sql(&state) else {
        return Ok(NoteUpdateOutcome::NotEditable);
    };
    if !state.is_publicly_visible() {
        return Ok(NoteUpdateOutcome::NotEditable);
    }
    if revision != expected_revision {
        return Ok(NoteUpdateOutcome::Conflict {
            current_revision: revision,
        });
    }
    if stored_title == title && stored_note == note_markdown {
        let clipping = load_by_id(connection, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        return Ok(NoteUpdateOutcome::Unchanged { clipping });
    }
    let changed = connection.execute(
        "UPDATE newspaper_clippings
         SET title = ?2,
             note_markdown = ?3,
             revision = revision + 1,
             updated_at = ?4
         WHERE id = ?1
           AND revision = ?5
           AND asset_state IN ('ready', 'missing')",
        params![id, title, note_markdown, now, expected_revision],
    )?;
    if changed == 1 {
        let clipping = load_by_id(connection, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        return Ok(NoteUpdateOutcome::Updated { clipping });
    }
    let current_revision: Option<u64> = connection
        .query_row(
            "SELECT revision FROM newspaper_clippings WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    match current_revision {
        Some(current_revision) => Ok(NoteUpdateOutcome::Conflict { current_revision }),
        None => Ok(NoteUpdateOutcome::NotFound),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteIntentOutcome {
    Marked,
    NotFound,
    Conflict { current_revision: u64 },
    NotEditable,
}

/// DELETE-STATE-001: mark a confirmed deletion intent. Only `ready` or
/// `missing` rows with a matching revision transition; every conflict aborts
/// before any filesystem mutation.
pub fn mark_delete_pending(
    connection: &Connection,
    id: &str,
    expected_revision: u64,
) -> Result<DeleteIntentOutcome> {
    let current = connection
        .query_row(
            "SELECT asset_state, revision FROM newspaper_clippings WHERE id = ?1",
            params![id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .optional()?;
    let Some((state, revision)) = current else {
        return Ok(DeleteIntentOutcome::NotFound);
    };
    let Some(state) = ClippingAssetState::from_sql(&state) else {
        return Ok(DeleteIntentOutcome::NotEditable);
    };
    if !state.is_publicly_visible() {
        return Ok(DeleteIntentOutcome::NotEditable);
    }
    if revision != expected_revision {
        return Ok(DeleteIntentOutcome::Conflict {
            current_revision: revision,
        });
    }
    let changed = connection.execute(
        "UPDATE newspaper_clippings
         SET asset_state = 'delete_pending'
         WHERE id = ?1
           AND revision = ?2
           AND asset_state IN ('ready', 'missing')",
        params![id, expected_revision],
    )?;
    if changed == 1 {
        Ok(DeleteIntentOutcome::Marked)
    } else {
        Ok(DeleteIntentOutcome::Conflict {
            current_revision: revision,
        })
    }
}

pub fn delete_if_pending(connection: &Connection, id: &str) -> Result<bool> {
    let changed = connection.execute(
        "DELETE FROM newspaper_clippings WHERE id = ?1 AND asset_state = 'delete_pending'",
        params![id],
    )?;
    Ok(changed == 1)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClippingSummary {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub page_number: String,
    pub asset_state: ClippingAssetState,
    pub asset_error_code: Option<String>,
    pub asset_version: u32,
    pub asset_pixel_width: u32,
    pub asset_pixel_height: u32,
    pub source_available: bool,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Plain-text excerpt for list rows (FR-READ-001/002). Derived from at most
/// the first 4,096 UTF-8 bytes of Markdown; markup markers are removed, HTML
/// is never executed, and whitespace is collapsed.
pub fn excerpt_from_markdown(markdown: &str) -> String {
    let mut end = markdown.len().min(4096);
    while end > 0 && !markdown.is_char_boundary(end) {
        end -= 1;
    }
    let head = &markdown[..end];
    let mut plain = String::with_capacity(head.len());
    for line in head.lines() {
        let mut trimmed = line.trim_start();
        while let Some(rest) = trimmed.strip_prefix('#') {
            trimmed = rest.trim_start();
        }
        let trimmed = trimmed.trim_start_matches("> ").trim_start_matches("- ");
        let mut chars = trimmed.chars();
        let mut cleaned = String::new();
        let mut previous_marker = false;
        while let Some(ch) = chars.next() {
            if ch == '*' || ch == '~' {
                previous_marker = true;
                continue;
            }
            if ch == '[' {
                // Keep link text, drop the URL target.
                let rest: String = chars.by_ref().take_while(|c| *c != ']').collect();
                cleaned.push_str(&rest);
                // Skip the "(...)" destination.
                let mut depth = 0usize;
                for next in chars.by_ref() {
                    if next == '(' {
                        depth += 1;
                    } else if next == ')' {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            break;
                        }
                    } else if depth == 0 {
                        break;
                    }
                }
                previous_marker = false;
                continue;
            }
            if previous_marker && ch == ' ' {
                previous_marker = false;
                continue;
            }
            previous_marker = false;
            cleaned.push(ch);
        }
        if !cleaned.trim().is_empty() {
            if !plain.is_empty() {
                plain.push(' ');
            }
            plain.push_str(cleaned.trim());
        }
    }
    let collapsed: Vec<&str> = plain.split_whitespace().collect();
    let mut result = collapsed.join(" ");
    let mut truncate_at = result.len().min(160);
    while truncate_at > 0 && !result.is_char_boundary(truncate_at) {
        truncate_at -= 1;
    }
    result.truncate(truncate_at);
    result
}

fn excerpt_from_bounded_markdown_bytes(bytes: &[u8]) -> String {
    let valid = match std::str::from_utf8(bytes) {
        Ok(markdown) => markdown,
        Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
    };
    excerpt_from_markdown(valid)
}

fn ordering_for(sort: NewspaperClippingSort) -> &'static str {
    match sort {
        NewspaperClippingSort::UpdatedDesc => "updated_at DESC, id DESC",
        NewspaperClippingSort::CreatedDesc => "created_at DESC, id DESC",
        NewspaperClippingSort::PublicationDesc => {
            "publication_date_snapshot DESC, edition_code_snapshot ASC, page_number_snapshot ASC, id ASC"
        }
        NewspaperClippingSort::TitleAsc => "title COLLATE NOCASE ASC, id ASC",
    }
}

/// Derive source availability from live joins; it is never persisted as a
/// mutable boolean (FR-SOURCE-003).
pub fn source_available(
    connection: &Connection,
    source_job_id: Option<&str>,
    source_page_id: Option<&str>,
) -> Result<bool> {
    let (Some(job_id), Some(page_id)) = (source_job_id, source_page_id) else {
        return Ok(false);
    };
    connection.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM newspaper_pages p
            JOIN newspaper_jobs j ON j.id = p.job_id
            WHERE p.id = ?1
              AND j.id = ?2
              AND p.status = 'completed'
        )",
        params![page_id, job_id],
        |row| row.get(0),
    )
}

/// Paged, searched, sorted list over publicly visible clippings
/// (FR-READ-001..003, D-019 bound escaped LIKE). Returns the page of
/// summaries plus the total visible count for deterministic pagination.
pub fn list_clippings(
    connection: &Connection,
    query: &NewspaperClippingListQuery,
) -> Result<(Vec<ClippingSummary>, u32)> {
    let has_search = !query.query.is_empty();
    let pattern = format!("%{}%", escape_like_pattern(&query.query));
    let where_sql = if has_search {
        "asset_state IN ('ready', 'missing')
         AND (title LIKE ?1 ESCAPE '\\'
              OR note_markdown LIKE ?1 ESCAPE '\\'
              OR edition_name_snapshot LIKE ?1 ESCAPE '\\'
              OR edition_code_snapshot LIKE ?1 ESCAPE '\\'
              OR publication_date_snapshot LIKE ?1 ESCAPE '\\'
              OR page_number_snapshot LIKE ?1 ESCAPE '\\')"
    } else {
        "asset_state IN ('ready', 'missing')"
    };
    let total: u32 = if has_search {
        connection.query_row(
            &format!("SELECT COUNT(*) FROM newspaper_clippings WHERE {where_sql}"),
            params![pattern],
            |row| row.get(0),
        )?
    } else {
        connection.query_row(
            &format!("SELECT COUNT(*) FROM newspaper_clippings WHERE {where_sql}"),
            [],
            |row| row.get(0),
        )?
    };
    let order = ordering_for(query.sort);
    let (limit_index, offset_index) = if has_search {
        ("?2", "?3")
    } else {
        ("?1", "?2")
    };
    let sql = format!(
        "SELECT id, title,
                COALESCE(substr(CAST(note_markdown AS BLOB), 1, 4096), X''),
                edition_code_snapshot, edition_name_snapshot,
                publication_date_snapshot, page_number_snapshot, asset_state,
                asset_error_code, asset_version, asset_pixel_width, asset_pixel_height,
                EXISTS(
                    SELECT 1
                    FROM newspaper_pages p
                    JOIN newspaper_jobs j ON j.id = p.job_id
                    WHERE p.id = newspaper_clippings.source_page_id
                      AND j.id = newspaper_clippings.source_job_id
                      AND p.status = 'completed'
                ),
                revision, created_at, updated_at
         FROM newspaper_clippings
         WHERE {where_sql}
         ORDER BY {order}
         LIMIT {limit_index} OFFSET {offset_index}"
    );
    #[allow(clippy::type_complexity)]
    let raw_rows: Vec<(
        String,
        String,
        Vec<u8>,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        u32,
        u32,
        u32,
        bool,
        u64,
        i64,
        i64,
    )> = if has_search {
        connection
            .prepare(&sql)?
            .query_map(params![pattern, query.limit, query.offset], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?
    } else {
        connection
            .prepare(&sql)?
            .query_map(params![query.limit, query.offset], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?
    };
    let mut summaries = Vec::with_capacity(raw_rows.len());
    for (
        id,
        title,
        note_markdown,
        edition_code,
        edition_name,
        publication_date,
        page_number,
        state,
        asset_error_code,
        asset_version,
        asset_pixel_width,
        asset_pixel_height,
        source_available,
        revision,
        created_at,
        updated_at,
    ) in raw_rows
    {
        summaries.push(ClippingSummary {
            id,
            title,
            excerpt: excerpt_from_bounded_markdown_bytes(&note_markdown),
            edition_code,
            edition_name,
            publication_date,
            page_number,
            asset_state: ClippingAssetState::from_sql(&state)
                .unwrap_or(ClippingAssetState::Missing),
            asset_error_code,
            asset_version,
            asset_pixel_width,
            asset_pixel_height,
            source_available,
            revision,
            created_at,
            updated_at,
        });
    }
    Ok((summaries, total))
}

/// Detail read for one public clipping plus its derived source availability
/// (specification 02 section 11). Absolute paths are never returned; media
/// URLs are derived by the command layer from the asset version.
pub struct ClippingDetail {
    pub clipping: NewspaperClipping,
    pub source_available: bool,
}

pub fn load_detail(connection: &Connection, id: &str) -> Result<Option<ClippingDetail>> {
    let Some(clipping) = load_public_by_id(connection, id)? else {
        return Ok(None);
    };
    let available = source_available(
        connection,
        clipping.source_job_id.as_deref(),
        clipping.source_page_id.as_deref(),
    )?;
    Ok(Some(ClippingDetail {
        clipping,
        source_available: available,
    }))
}

/// Recovery scan input for creating-state rows (RECOVERY-001), oldest first.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatingRecoveryTarget {
    pub id: String,
    pub asset_relative_path: String,
    pub asset_byte_count: u64,
    pub asset_pixel_width: u32,
    pub asset_pixel_height: u32,
    pub asset_checksum_sha256: String,
}

pub fn load_creating_rows(connection: &Connection) -> Result<Vec<CreatingRecoveryTarget>> {
    connection
        .prepare(
            "SELECT id, asset_relative_path, asset_byte_count, asset_pixel_width,
                    asset_pixel_height, asset_checksum_sha256
             FROM newspaper_clippings
             WHERE asset_state = 'creating'
             ORDER BY created_at ASC, id ASC",
        )?
        .query_map([], |row| {
            Ok(CreatingRecoveryTarget {
                id: row.get(0)?,
                asset_relative_path: row.get(1)?,
                asset_byte_count: row.get(2)?,
                asset_pixel_width: row.get(3)?,
                asset_pixel_height: row.get(4)?,
                asset_checksum_sha256: row.get(5)?,
            })
        })?
        .collect()
}

pub fn load_delete_pending_ids(connection: &Connection) -> Result<Vec<String>> {
    connection
        .prepare(
            "SELECT id FROM newspaper_clippings
             WHERE asset_state = 'delete_pending'
             ORDER BY updated_at ASC, id ASC",
        )?
        .query_map([], |row| row.get(0))?
        .collect()
}

pub fn load_all_ids(connection: &Connection) -> Result<Vec<String>> {
    connection
        .prepare("SELECT id FROM newspaper_clippings ORDER BY id ASC")?
        .query_map([], |row| row.get(0))?
        .collect()
}

/// Explicit source unlink for the World Journal reset transaction
/// (FR-SOURCE-DELETE-001). Runs on the reset connection so preservation is
/// deterministic even when the foreign_keys pragma is disabled. Only the
/// source references change: titles, notes, revisions, timestamps, assets,
/// provenance, and geometry are untouched (FR-SOURCE-DELETE-002).
pub fn unlink_all_sources(connection: &Connection) -> Result<usize> {
    connection.execute(
        "UPDATE newspaper_clippings
         SET source_page_id = NULL,
             source_job_id = NULL
         WHERE source_page_id IS NOT NULL
            OR source_job_id IS NOT NULL",
        [],
    )
}

/// Scoped unlink for single-job deletion (FR-SOURCE-DELETE-001). Pages
/// cascade from the job, so both references are cleared for the job's pages.
pub fn unlink_sources_for_job(connection: &Connection, job_id: &str) -> Result<usize> {
    connection.execute(
        "UPDATE newspaper_clippings
         SET source_page_id = NULL,
             source_job_id = NULL
         WHERE source_job_id = ?1
            OR source_page_id IN (
                SELECT id FROM newspaper_pages WHERE job_id = ?1
            )",
        params![job_id],
    )
}
