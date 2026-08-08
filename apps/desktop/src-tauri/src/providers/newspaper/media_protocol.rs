use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use rusqlite::{params, Connection, OptionalExtension};
use tauri::http::{
    header::{ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG},
    Request, Response, StatusCode,
};

const CACHE_SCHEMA_VERSION: i64 = 1;

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};

use super::clipping_assets::ClippingAssetLayout;
use super::clipping_models::{validate_clipping_id, ClippingErrorCode};
use super::clipping_repository;

pub fn handle_request(
    db_path: &Path,
    cache_root: &Path,
    clipping_layout: &ClippingAssetLayout,
    writer: &DatabaseWriter,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    match resolve_media(db_path, cache_root, clipping_layout, writer, request) {
        Ok(media) => response(
            StatusCode::OK,
            media.bytes,
            Some(&media.mime_type),
            Some(&media.etag),
        ),
        Err(MediaError::BadRequest) => response(
            StatusCode::BAD_REQUEST,
            b"Invalid newspaper media request.".to_vec(),
            Some("text/plain; charset=utf-8"),
            None,
        ),
        Err(MediaError::NotFound) => response(
            StatusCode::NOT_FOUND,
            b"Newspaper media is unavailable.".to_vec(),
            Some("text/plain; charset=utf-8"),
            None,
        ),
        Err(MediaError::Unsupported) => response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            b"Unsupported newspaper media type.".to_vec(),
            Some("text/plain; charset=utf-8"),
            None,
        ),
        Err(MediaError::Internal) => response(
            StatusCode::INTERNAL_SERVER_ERROR,
            b"Newspaper media could not be loaded.".to_vec(),
            Some("text/plain; charset=utf-8"),
            None,
        ),
    }
}

struct ResolvedMedia {
    bytes: Vec<u8>,
    mime_type: String,
    etag: String,
}

enum MediaError {
    BadRequest,
    NotFound,
    Unsupported,
    Internal,
}

fn resolve_media(
    db_path: &Path,
    cache_root: &Path,
    clipping_layout: &ClippingAssetLayout,
    writer: &DatabaseWriter,
    request: &Request<Vec<u8>>,
) -> Result<ResolvedMedia, MediaError> {
    let segments = request
        .uri()
        .path()
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if segments.len() != 2 {
        return Err(MediaError::BadRequest);
    }
    let clipping_route = matches!(segments[0], "clipping" | "clipping-thumbnail");
    if (clipping_route && !validate_clipping_id(segments[1]))
        || (!clipping_route && !valid_id(segments[1]))
    {
        return Err(MediaError::BadRequest);
    }
    let version = request
        .uri()
        .query()
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())
        })
        .filter(|value| !value.is_empty())
        .ok_or(MediaError::BadRequest)?;
    let connection = crate::cache::open_runtime(db_path).map_err(|_| MediaError::Internal)?;

    if segments[0] == "clipping" {
        return resolve_clipping(&connection, clipping_layout, writer, segments[1], &version);
    }
    if segments[0] == "clipping-thumbnail" {
        return resolve_clipping_thumbnail(&connection, clipping_layout, segments[1], &version);
    }

    let (path, mime_type, etag) = match segments[0] {
        "page" => resolve_page(&connection, segments[1], &version)?,
        "thumbnail" => resolve_thumbnail(&connection, cache_root, segments[1], &version)?,
        _ => return Err(MediaError::BadRequest),
    };
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| MediaError::NotFound)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
        return Err(MediaError::NotFound);
    }
    let bytes = std::fs::read(&path).map_err(|_| MediaError::Internal)?;
    if bytes.is_empty() {
        return Err(MediaError::NotFound);
    }
    Ok(ResolvedMedia {
        bytes,
        mime_type,
        etag,
    })
}

fn resolve_clipping(
    connection: &Connection,
    layout: &ClippingAssetLayout,
    writer: &DatabaseWriter,
    clipping_id: &str,
    requested_version: &str,
) -> Result<ResolvedMedia, MediaError> {
    let record = connection
        .query_row(
            "SELECT asset_relative_path, asset_version, asset_state, asset_byte_count,
                    asset_pixel_width, asset_pixel_height, asset_checksum_sha256
             FROM newspaper_clippings WHERE id = ?1",
            params![clipping_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|_| MediaError::Internal)?
        .ok_or(MediaError::NotFound)?;
    if record.2 != "ready" || requested_version != record.1.to_string() {
        return Err(MediaError::NotFound);
    }
    let expected_relative = ClippingAssetLayout::canonical_relative_path(clipping_id)
        .map_err(|_| MediaError::BadRequest)?;
    if record.0 != expected_relative
        || ClippingAssetLayout::validate_relative_path(&record.0).is_err()
    {
        return Err(MediaError::NotFound);
    }
    if let Err(error) =
        layout.verify_canonical(clipping_id, record.3, record.4, record.5, &record.6)
    {
        let safe_code = match error.code {
            ClippingErrorCode::AssetChecksumMismatch => "CLIPPING_ASSET_CHECKSUM_MISMATCH",
            _ => "CLIPPING_ASSET_MISSING",
        };
        let id = clipping_id.to_string();
        let code = safe_code.to_string();
        let _ = writer.execute(
            DatabaseWriteContext {
                operation: "clipping_media_mark_missing",
                provider: DatabaseProvider::Newspaper,
                workflow_id: None,
            },
            move |db| {
                clipping_repository::mark_missing_from_ready(
                    db,
                    &id,
                    &code,
                    chrono::Utc::now().timestamp(),
                )
                .map_err(Into::into)
            },
        );
        return Err(MediaError::NotFound);
    }
    let (bytes, mime) = layout
        .read_canonical_for_protocol(clipping_id)
        .map_err(|_| MediaError::NotFound)?;
    Ok(ResolvedMedia {
        bytes,
        mime_type: mime.to_string(),
        etag: format!("clipping-{clipping_id}-{}", record.1),
    })
}

fn resolve_clipping_thumbnail(
    connection: &Connection,
    layout: &ClippingAssetLayout,
    clipping_id: &str,
    requested_version: &str,
) -> Result<ResolvedMedia, MediaError> {
    let record = connection
        .query_row(
            "SELECT asset_version, asset_state FROM newspaper_clippings WHERE id = ?1",
            params![clipping_id],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|_| MediaError::Internal)?
        .ok_or(MediaError::NotFound)?;
    let expected = format!("{}-{}", record.0, CACHE_SCHEMA_VERSION);
    if record.1 != "ready" || requested_version != expected {
        return Err(MediaError::NotFound);
    }
    let (bytes, mime) = layout
        .read_thumbnail_for_protocol(clipping_id)
        .map_err(|_| MediaError::NotFound)?;
    Ok(ResolvedMedia {
        bytes,
        mime_type: mime.to_string(),
        etag: format!("clipping-thumbnail-{clipping_id}-{expected}"),
    })
}

fn resolve_page(
    connection: &Connection,
    page_id: &str,
    requested_version: &str,
) -> Result<(PathBuf, String, String), MediaError> {
    let record = connection
        .query_row(
            "SELECT COALESCE(optimized_path, original_path), media_version
             FROM newspaper_pages
             WHERE id = ?1 AND status = 'completed'
               AND COALESCE(optimized_path, original_path) IS NOT NULL",
            params![page_id],
            |row| {
                Ok((
                    PathBuf::from(row.get::<_, String>(0)?),
                    row.get::<_, i64>(1)?,
                ))
            },
        )
        .optional()
        .map_err(|_| MediaError::Internal)?
        .ok_or(MediaError::NotFound)?;
    if record.1.to_string() != requested_version {
        return Err(MediaError::NotFound);
    }
    let mime = mime_for_path(&record.0)?;
    Ok((
        record.0,
        mime.to_string(),
        format!("page-{page_id}-{}", record.1),
    ))
}

fn resolve_thumbnail(
    connection: &Connection,
    cache_root: &Path,
    job_id: &str,
    requested_version: &str,
) -> Result<(PathBuf, String, String), MediaError> {
    let record = connection
        .query_row(
            "SELECT cache_path, source_media_version, cache_schema_version, mime_type
             FROM newspaper_thumbnail_cache WHERE job_id = ?1",
            params![job_id],
            |row| {
                Ok((
                    PathBuf::from(row.get::<_, String>(0)?),
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|_| MediaError::Internal)?
        .ok_or(MediaError::NotFound)?;
    let expected_version = format!("{}-{}", record.1, record.2);
    if record.2 != CACHE_SCHEMA_VERSION || expected_version != requested_version {
        return Err(MediaError::NotFound);
    }
    if record.3 != "image/webp" {
        return Err(MediaError::Unsupported);
    }
    let canonical_root = cache_root
        .canonicalize()
        .map_err(|_| MediaError::NotFound)?;
    let canonical_path = record.0.canonicalize().map_err(|_| MediaError::NotFound)?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(MediaError::NotFound);
    }
    Ok((
        canonical_path,
        record.3,
        format!("thumbnail-{job_id}-{expected_version}"),
    ))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn mime_for_path(path: &Path) -> Result<&'static str, MediaError> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => Ok("image/jpeg"),
        Some("png") => Ok("image/png"),
        Some("webp") => Ok("image/webp"),
        _ => Err(MediaError::Unsupported),
    }
}

fn response(
    status: StatusCode,
    body: Vec<u8>,
    mime_type: Option<&str>,
    etag: Option<&str>,
) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if let Some(mime_type) = mime_type {
        builder = builder.header(CONTENT_TYPE, mime_type);
    }
    if status == StatusCode::OK {
        builder = builder.header(CACHE_CONTROL, "private, max-age=31536000, immutable");
    } else {
        builder = builder.header(CACHE_CONTROL, "no-store");
    }
    if let Some(etag) = etag {
        builder = builder.header(ETAG, format!("\"{etag}\""));
    }
    builder.body(body).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Vec::new())
            .expect("static media error response must be valid")
    })
}

pub fn request_for_url(url: &str) -> Result<Request<Vec<u8>>, String> {
    let uri = tauri::http::Uri::from_str(url).map_err(|error| error.to_string())?;
    Request::builder()
        .uri(uri)
        .body(Vec::new())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database_diagnostics::DatabaseDiagnostics;
    use crate::app::database_writer::DatabaseWriter;
    use crate::newspaper::clipping_assets::{encode_test_webp, sha256_hex};
    use crate::newspaper::clipping_models::ClippingSourceKind;
    use crate::newspaper::clipping_repository::NewClippingRecord;
    use crate::newspaper::clipping_service::ClippingService;
    use tempfile::tempdir;

    fn fixture() -> (
        tempfile::TempDir,
        PathBuf,
        PathBuf,
        ClippingAssetLayout,
        DatabaseWriter,
    ) {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let cache_root = directory.path().join("newspaper-thumbnails").join("v1");
        std::fs::create_dir_all(&cache_root).unwrap();
        let page_path = directory.path().join("A01.jpg");
        std::fs::write(&page_path, b"page-bytes").unwrap();
        let thumbnail_path = cache_root.join("job.webp");
        std::fs::write(&thumbnail_path, b"thumbnail-bytes").unwrap();
        let connection = Connection::open(&db_path).unwrap();
        crate::newspaper::storage::initialize(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, optimize_images,
                  optimization_profile, keep_original_jpg, created_at, updated_at)
                 VALUES ('batch', 'completed', ?1, 0, 0, 'webp_high', 1, 1, 1)",
                params![directory.path().to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_jobs
                 (id, batch_id, edition_code, publication_date, status, output_dir,
                  page_count, completed_count, created_at, updated_at)
                 VALUES ('job', 'batch', 'NY', '2026-07-25', 'completed', ?1, 1, 1, 1, 1)",
                params![directory.path().to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, status,
                  original_bytes, final_bytes, checksum, pixel_width, pixel_height,
                  media_version, created_at, updated_at)
                 VALUES ('page', 'job', 'A01', 'https://example.test/A01.jpg', ?1,
                         'completed', 10, 10, 'checksum', 800, 1200, 3, 1, 1)",
                params![page_path.to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_thumbnail_cache
                 (job_id, source_page_id, source_media_version, cache_schema_version,
                  cache_path, mime_type, pixel_width, pixel_height, byte_count, updated_at)
                 VALUES ('job', 'page', 3, 1, ?1, 'image/webp', 420, 176, ?2, 1)",
                params![
                    thumbnail_path.to_string_lossy(),
                    std::fs::metadata(&thumbnail_path).unwrap().len()
                ],
            )
            .unwrap();
        drop(connection);
        let clipping_layout =
            ClippingAssetLayout::new(directory.path().join("newspaper-clippings"));
        let writer =
            DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default()).unwrap();
        (directory, db_path, cache_root, clipping_layout, writer)
    }

    #[test]
    fn registered_page_and_thumbnail_receive_cacheable_responses() {
        let (_directory, db_path, cache_root, clipping_layout, writer) = fixture();
        let page = request_for_url("http://newspaper-media.localhost/page/page?v=3").unwrap();
        let page_response = handle_request(&db_path, &cache_root, &clipping_layout, &writer, &page);
        assert_eq!(page_response.status(), StatusCode::OK);
        assert_eq!(page_response.headers()[CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            page_response.headers()[CACHE_CONTROL],
            "private, max-age=31536000, immutable"
        );

        let thumbnail =
            request_for_url("http://newspaper-media.localhost/thumbnail/job?v=3-1").unwrap();
        let thumbnail_response =
            handle_request(&db_path, &cache_root, &clipping_layout, &writer, &thumbnail);
        assert_eq!(thumbnail_response.status(), StatusCode::OK);
        assert_eq!(thumbnail_response.headers()[CONTENT_TYPE], "image/webp");
    }

    #[test]
    fn malformed_unknown_and_stale_requests_are_rejected_without_paths() {
        let (directory, db_path, cache_root, clipping_layout, writer) = fixture();
        for url in [
            "http://newspaper-media.localhost/page/..%2Fsecret?v=3",
            "http://newspaper-media.localhost/page/missing?v=3",
            "http://newspaper-media.localhost/page/page?v=2",
            "http://newspaper-media.localhost/other/page?v=3",
        ] {
            let request = request_for_url(url).unwrap();
            let response =
                handle_request(&db_path, &cache_root, &clipping_layout, &writer, &request);
            assert!(!response.status().is_success());
            let body = String::from_utf8_lossy(response.body());
            assert!(!body.contains(&directory.path().to_string_lossy().to_string()));
        }
    }

    #[test]
    fn clipping_routes_require_current_versions_and_mark_corruption_missing() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let (directory, db_path, cache_root, clipping_layout, writer) = fixture();
        let bytes = encode_test_webp(24, 16);
        clipping_layout.write_staging(ID, &bytes).unwrap();
        let service =
            ClippingService::new(db_path.clone(), writer.clone(), clipping_layout.clone());
        service
            .register_staged(NewClippingRecord {
                id: ID.to_string(),
                source_job_id: None,
                source_page_id: None,
                source_media_version_snapshot: 1,
                source_kind_snapshot: ClippingSourceKind::Optimized,
                source_mime_type_snapshot: "image/webp".to_string(),
                source_checksum_snapshot: None,
                edition_code_snapshot: "NY".to_string(),
                edition_name_snapshot: "New York".to_string(),
                publication_date_snapshot: "2026-08-08".to_string(),
                page_number_snapshot: "A01".to_string(),
                source_pixel_width: 24,
                source_pixel_height: 16,
                crop_x: 0,
                crop_y: 0,
                crop_width: 24,
                crop_height: 16,
                asset_relative_path: ClippingAssetLayout::canonical_relative_path(ID).unwrap(),
                asset_byte_count: bytes.len() as u64,
                asset_checksum_sha256: sha256_hex(&bytes),
                title: "New York · 2026-08-08 · A01".to_string(),
                now: 100,
            })
            .unwrap();
        std::fs::write(clipping_layout.thumbnail_path(ID).unwrap(), &bytes).unwrap();

        for (route, expected) in [("clipping", "1"), ("clipping-thumbnail", "1-1")] {
            let request = request_for_url(&format!(
                "http://newspaper-media.localhost/{route}/{ID}?v={expected}"
            ))
            .unwrap();
            let response =
                handle_request(&db_path, &cache_root, &clipping_layout, &writer, &request);
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[CONTENT_TYPE], "image/webp");
        }
        let stale = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping/{ID}?v=2"
        ))
        .unwrap();
        assert_eq!(
            handle_request(&db_path, &cache_root, &clipping_layout, &writer, &stale).status(),
            StatusCode::NOT_FOUND
        );

        std::fs::write(clipping_layout.canonical_path(ID).unwrap(), b"corrupt").unwrap();
        let corrupt = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping/{ID}?v=1"
        ))
        .unwrap();
        let response = handle_request(&db_path, &cache_root, &clipping_layout, &writer, &corrupt);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!String::from_utf8_lossy(response.body())
            .contains(&directory.path().to_string_lossy().to_string()));
        let connection = crate::cache::open_runtime(&db_path).unwrap();
        let state: String = connection
            .query_row(
                "SELECT asset_state FROM newspaper_clippings WHERE id = ?1",
                params![ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "missing");
    }
}
