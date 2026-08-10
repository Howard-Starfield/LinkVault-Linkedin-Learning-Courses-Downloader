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

use super::clipping_assets::{ClippingAssetLayout, THUMBNAIL_CACHE_SCHEMA_VERSION};
use super::clipping_models::{validate_clipping_id, ClippingErrorCode};
use super::clipping_service::ClippingService;

pub fn handle_request(
    db_path: &Path,
    cache_root: &Path,
    clipping_service: &ClippingService,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    match resolve_media(db_path, cache_root, clipping_service, request) {
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
    clipping_service: &ClippingService,
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
        return resolve_clipping(&connection, clipping_service, segments[1], &version);
    }
    if segments[0] == "clipping-thumbnail" {
        return resolve_clipping_thumbnail(&connection, clipping_service, segments[1], &version);
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
    service: &ClippingService,
    clipping_id: &str,
    requested_version: &str,
) -> Result<ResolvedMedia, MediaError> {
    let record = connection
        .query_row(
            "SELECT asset_root_id, asset_relative_path, asset_version, asset_state, asset_byte_count,
                    asset_pixel_width, asset_pixel_height, asset_checksum_sha256
             FROM newspaper_clippings WHERE id = ?1",
            params![clipping_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, u32>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|_| MediaError::Internal)?
        .ok_or(MediaError::NotFound)?;
    if record.3 != "ready" || requested_version != record.2.to_string() {
        return Err(MediaError::NotFound);
    }
    if ClippingAssetLayout::validate_relative_path_for_id(&record.1, clipping_id).is_err() {
        return Err(MediaError::NotFound);
    }
    let layout = service
        .root_layout(&record.0)
        .map_err(|_| MediaError::NotFound)?;
    let validated = layout.read_validated_canonical_at(
        clipping_id,
        &record.1,
        record.4,
        record.5,
        record.6,
        &record.7,
    );
    let (bytes, mime) = match validated {
        Ok(validated) => validated,
        Err(error) => {
            // `root_layout` may use the deliberately short verified-root
            // cache. Recheck after an I/O failure so an offline removable
            // drive does not permanently turn a healthy clipping `missing`.
            if service.verify_root_fresh_for_integrity(&record.0).is_err() {
                return Err(MediaError::NotFound);
            }
            let safe_code = match error.code {
                ClippingErrorCode::AssetChecksumMismatch => "CLIPPING_ASSET_CHECKSUM_MISMATCH",
                _ => "CLIPPING_ASSET_MISSING",
            };
            let _ = service.schedule_media_integrity_transition(
                clipping_id,
                safe_code,
                chrono::Utc::now().timestamp(),
            );
            return Err(MediaError::NotFound);
        }
    };
    Ok(ResolvedMedia {
        bytes,
        mime_type: mime.to_string(),
        etag: format!("clipping-{clipping_id}-{}", record.2),
    })
}

fn resolve_clipping_thumbnail(
    connection: &Connection,
    service: &ClippingService,
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
    let expected = format!("{}-{}", record.0, THUMBNAIL_CACHE_SCHEMA_VERSION);
    if record.1 != "ready" || requested_version != expected {
        return Err(MediaError::NotFound);
    }
    let (bytes, mime) = service
        .layout()
        .read_thumbnail_for_protocol(clipping_id, record.0)
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
    use crate::app::database_diagnostics::DatabaseProvider;
    use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};
    use crate::newspaper::clipping_assets::{
        encode_test_webp, sha256_hex, THUMBNAIL_MAX_BYTES, THUMBNAIL_MAX_HEIGHT,
        THUMBNAIL_MAX_WIDTH,
    };
    use crate::newspaper::clipping_models::ClippingSourceKind;
    use crate::newspaper::clipping_repository::NewClippingRecord;
    use crate::newspaper::clipping_service::ClippingService;
    use std::time::Duration;
    use tempfile::tempdir;

    fn fixture() -> (
        tempfile::TempDir,
        PathBuf,
        PathBuf,
        ClippingAssetLayout,
        DatabaseWriter,
        ClippingService,
        DatabaseDiagnostics,
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
        let diagnostics = DatabaseDiagnostics::default();
        let writer = DatabaseWriter::start(db_path.clone(), diagnostics.clone()).unwrap();
        let service = ClippingService::new(
            db_path.clone(),
            writer.clone(),
            clipping_layout.clone(),
            diagnostics.clone(),
        );
        (
            directory,
            db_path,
            cache_root,
            clipping_layout,
            writer,
            service,
            diagnostics,
        )
    }

    #[test]
    fn registered_page_and_thumbnail_receive_cacheable_responses() {
        let (_directory, db_path, cache_root, _clipping_layout, _writer, service, _diagnostics) =
            fixture();
        let page = request_for_url("http://newspaper-media.localhost/page/page?v=3").unwrap();
        let page_response = handle_request(&db_path, &cache_root, &service, &page);
        assert_eq!(page_response.status(), StatusCode::OK);
        assert_eq!(page_response.headers()[CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            page_response.headers()[CACHE_CONTROL],
            "private, max-age=31536000, immutable"
        );

        let thumbnail =
            request_for_url("http://newspaper-media.localhost/thumbnail/job?v=3-1").unwrap();
        let thumbnail_response = handle_request(&db_path, &cache_root, &service, &thumbnail);
        assert_eq!(thumbnail_response.status(), StatusCode::OK);
        assert_eq!(thumbnail_response.headers()[CONTENT_TYPE], "image/webp");
    }

    #[test]
    fn malformed_unknown_and_stale_requests_are_rejected_without_paths() {
        let (directory, db_path, cache_root, _clipping_layout, _writer, service, _diagnostics) =
            fixture();
        for url in [
            "http://newspaper-media.localhost/page/..%2Fsecret?v=3",
            "http://newspaper-media.localhost/page/missing?v=3",
            "http://newspaper-media.localhost/page/page?v=2",
            "http://newspaper-media.localhost/other/page?v=3",
        ] {
            let request = request_for_url(url).unwrap();
            let response = handle_request(&db_path, &cache_root, &service, &request);
            assert!(!response.status().is_success());
            let body = String::from_utf8_lossy(response.body());
            assert!(!body.contains(&directory.path().to_string_lossy().to_string()));
        }
    }

    #[test]
    fn clipping_routes_require_current_versions_and_mark_corruption_missing() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let (directory, db_path, cache_root, clipping_layout, _writer, service, _diagnostics) =
            fixture();
        let bytes = encode_test_webp(24, 16);
        clipping_layout.write_staging(ID, &bytes).unwrap();
        service
            .register_staged_legacy_fixture(NewClippingRecord {
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
                asset_root_id: crate::newspaper::storage::LEGACY_CLIPPING_ROOT_ID.to_owned(),
                asset_relative_path: ClippingAssetLayout::canonical_relative_path(ID).unwrap(),
                asset_byte_count: bytes.len() as u64,
                asset_checksum_sha256: sha256_hex(&bytes),
                title: "New York · 2026-08-08 · A01".to_string(),
                now: 100,
            })
            .unwrap();
        std::fs::write(clipping_layout.thumbnail_path(ID, 1).unwrap(), &bytes).unwrap();

        for (route, expected) in [("clipping", "1"), ("clipping-thumbnail", "1-2")] {
            let request = request_for_url(&format!(
                "http://newspaper-media.localhost/{route}/{ID}?v={expected}"
            ))
            .unwrap();
            let response = handle_request(&db_path, &cache_root, &service, &request);
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[CONTENT_TYPE], "image/webp");
        }
        let stale = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping/{ID}?v=2"
        ))
        .unwrap();
        assert_eq!(
            handle_request(&db_path, &cache_root, &service, &stale).status(),
            StatusCode::NOT_FOUND
        );

        std::fs::write(clipping_layout.canonical_path(ID).unwrap(), b"corrupt").unwrap();
        let corrupt = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping/{ID}?v=1"
        ))
        .unwrap();
        let response = handle_request(&db_path, &cache_root, &service, &corrupt);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!String::from_utf8_lossy(response.body())
            .contains(&directory.path().to_string_lossy().to_string()));
        assert!(service.wait_for_media_integrity_transitions(Duration::from_secs(2)));
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

    #[test]
    fn offline_snapshot_root_does_not_poison_ready_clipping_state() {
        const ID: &str = "11111111-1111-4111-8111-111111111111";
        let (directory, db_path, cache_root, _clipping_layout, _writer, service, _diagnostics) =
            fixture();
        let destination = directory.path().join("downloads");
        std::fs::create_dir(&destination).unwrap();
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE newspaper_batches SET destination = ?1 WHERE id = 'batch'",
                params![destination.to_string_lossy()],
            )
            .unwrap();
        let root = service.register_source_job_root("job", 100).unwrap();
        let layout = service.root_layout(&root.id).unwrap();
        let bytes = encode_test_webp(24, 16);
        layout.write_staging(ID, &bytes).unwrap();
        let relative =
            ClippingAssetLayout::snapshot_relative_path("New York", "NY", "2026-08-08", "A01", ID)
                .unwrap();
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
                asset_root_id: root.id.clone(),
                asset_relative_path: relative,
                asset_byte_count: bytes.len() as u64,
                asset_checksum_sha256: sha256_hex(&bytes),
                title: "New York - 2026-08-08 - A01".to_string(),
                now: 100,
            })
            .unwrap();

        let offline = destination.join("snapshots-offline");
        std::fs::rename(&root.locator, &offline).unwrap();
        let request = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping/{ID}?v=1"
        ))
        .unwrap();
        assert_eq!(
            handle_request(&db_path, &cache_root, &service, &request).status(),
            StatusCode::NOT_FOUND
        );
        std::thread::sleep(Duration::from_millis(50));
        let state: String = Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT asset_state FROM newspaper_clippings WHERE id = ?1",
                params![ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "ready");
        assert!(!PathBuf::from(&root.locator).exists());
    }

    #[test]
    fn corrupt_media_response_is_nonblocking_coalesced_and_eventually_marks_missing() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let (directory, db_path, cache_root, layout, writer, service, diagnostics) = fixture();
        let bytes = encode_test_webp(24, 16);
        layout.write_staging(ID, &bytes).unwrap();
        let created = service
            .register_staged_legacy_fixture(NewClippingRecord {
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
                asset_root_id: crate::newspaper::storage::LEGACY_CLIPPING_ROOT_ID.to_owned(),
                asset_relative_path: ClippingAssetLayout::canonical_relative_path(ID).unwrap(),
                asset_byte_count: bytes.len() as u64,
                asset_checksum_sha256: sha256_hex(&bytes),
                title: "New York".to_string(),
                now: 100,
            })
            .unwrap();
        service
            .update_note(
                ID,
                created.revision,
                "New York",
                "secret-note-sentinel",
                101,
            )
            .unwrap();
        std::fs::write(layout.canonical_path(ID).unwrap(), b"corrupt").unwrap();

        let (writer_entered_tx, writer_entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_writer_tx, release_writer_rx) = std::sync::mpsc::sync_channel(1);
        let occupied_writer = writer.clone();
        let blocker = std::thread::spawn(move || {
            occupied_writer.execute(
                DatabaseWriteContext {
                    operation: "test_occupy_writer_for_media",
                    provider: DatabaseProvider::Newspaper,
                    workflow_id: None,
                },
                move |_db| {
                    writer_entered_tx.send(()).unwrap();
                    release_writer_rx.recv().unwrap();
                    Ok(())
                },
            )
        });
        writer_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("writer must be deliberately occupied");

        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
        let response_service = service.clone();
        let response_db = db_path.clone();
        let response_cache = cache_root.clone();
        std::thread::spawn(move || {
            let request = request_for_url(&format!(
                "http://newspaper-media.localhost/clipping/{ID}?v=1"
            ))
            .unwrap();
            response_tx
                .send(handle_request(
                    &response_db,
                    &response_cache,
                    &response_service,
                    &request,
                ))
                .unwrap();
        });
        let response = response_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("media response must not wait for the occupied writer");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(service.pending_media_integrity_transitions(), 1);

        for _ in 0..(super::super::clipping_service::MEDIA_INTEGRITY_QUEUE_CAPACITY * 4) {
            let request = request_for_url(&format!(
                "http://newspaper-media.localhost/clipping/{ID}?v=1"
            ))
            .unwrap();
            assert_eq!(
                handle_request(&db_path, &cache_root, &service, &request).status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(service.pending_media_integrity_transitions(), 1);

        release_writer_tx.send(()).unwrap();
        blocker.join().unwrap().unwrap();
        assert!(service.wait_for_media_integrity_transitions(Duration::from_secs(2)));
        let connection = crate::cache::open_runtime(&db_path).unwrap();
        let state: String = connection
            .query_row(
                "SELECT asset_state FROM newspaper_clippings WHERE id = ?1",
                params![ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "missing");

        let diagnostics_text = format!("{:?}", diagnostics.snapshot());
        assert!(!diagnostics_text.contains("secret-note-sentinel"));
        assert!(!diagnostics_text.contains(&directory.path().to_string_lossy().to_string()));
    }

    #[test]
    fn writer_shutdown_never_hangs_corrupt_media_response() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let (_directory, db_path, cache_root, layout, writer, service, diagnostics) = fixture();
        let bytes = encode_test_webp(24, 16);
        layout.write_staging(ID, &bytes).unwrap();
        service
            .register_staged_legacy_fixture(NewClippingRecord {
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
                asset_root_id: crate::newspaper::storage::LEGACY_CLIPPING_ROOT_ID.to_owned(),
                asset_relative_path: ClippingAssetLayout::canonical_relative_path(ID).unwrap(),
                asset_byte_count: bytes.len() as u64,
                asset_checksum_sha256: sha256_hex(&bytes),
                title: "New York".to_string(),
                now: 100,
            })
            .unwrap();
        std::fs::write(layout.canonical_path(ID).unwrap(), b"corrupt").unwrap();
        writer.shutdown().unwrap();

        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
        let request = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping/{ID}?v=1"
        ))
        .unwrap();
        std::thread::spawn(move || {
            response_tx
                .send(handle_request(&db_path, &cache_root, &service, &request))
                .unwrap();
        });
        assert_eq!(
            response_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("closed writer must not block the protocol")
                .status(),
            StatusCode::NOT_FOUND
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline
            && !diagnostics
                .snapshot()
                .iter()
                .any(|event| event.operation == "clipping_media_integrity_transition")
        {
            std::thread::yield_now();
        }
        assert!(diagnostics
            .snapshot()
            .iter()
            .any(|event| event.operation == "clipping_media_integrity_transition"));
    }

    #[test]
    fn thumbnail_protocol_requires_the_canonical_asset_version_file() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let (_directory, db_path, cache_root, layout, writer, service, _diagnostics) = fixture();
        let canonical = encode_test_webp(24, 16);
        layout.write_staging(ID, &canonical).unwrap();
        service
            .register_staged_legacy_fixture(NewClippingRecord {
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
                asset_root_id: crate::newspaper::storage::LEGACY_CLIPPING_ROOT_ID.to_owned(),
                asset_relative_path: ClippingAssetLayout::canonical_relative_path(ID).unwrap(),
                asset_byte_count: canonical.len() as u64,
                asset_checksum_sha256: sha256_hex(&canonical),
                title: "New York".to_string(),
                now: 100,
            })
            .unwrap();
        let version_one = encode_test_webp(8, 8);
        let version_two = encode_test_webp(THUMBNAIL_MAX_WIDTH, THUMBNAIL_MAX_HEIGHT);
        std::fs::write(layout.thumbnail_path(ID, 1).unwrap(), &version_one).unwrap();
        let owned_id = ID.to_string();
        writer
            .execute(
                DatabaseWriteContext {
                    operation: "test_increment_clipping_asset_version",
                    provider: DatabaseProvider::Newspaper,
                    workflow_id: None,
                },
                move |db| {
                    db.execute(
                        "UPDATE newspaper_clippings SET asset_version = 2 WHERE id = ?1",
                        params![owned_id],
                    )?;
                    Ok(())
                },
            )
            .unwrap();

        let current = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping-thumbnail/{ID}?v=2-2"
        ))
        .unwrap();
        assert_eq!(
            handle_request(&db_path, &cache_root, &service, &current).status(),
            StatusCode::NOT_FOUND,
            "asset-version-1 bytes must not satisfy a version-2 request"
        );
        std::fs::write(layout.thumbnail_path(ID, 2).unwrap(), &version_two).unwrap();
        let current_response = handle_request(&db_path, &cache_root, &service, &current);
        assert_eq!(current_response.status(), StatusCode::OK);
        assert_eq!(current_response.body(), &version_two);

        let canonical_before = std::fs::read(layout.canonical_path(ID).unwrap()).unwrap();
        let detail_before = service.detail(ID).unwrap().unwrap().clipping;
        for (label, invalid) in [
            ("empty", Vec::new()),
            ("malformed", b"RIFF\0\0\0\0WEBPmalformed".to_vec()),
            (
                "width",
                encode_test_webp(THUMBNAIL_MAX_WIDTH + 1, THUMBNAIL_MAX_HEIGHT),
            ),
            (
                "height",
                encode_test_webp(THUMBNAIL_MAX_WIDTH, THUMBNAIL_MAX_HEIGHT + 1),
            ),
        ] {
            std::fs::write(layout.thumbnail_path(ID, 2).unwrap(), invalid).unwrap();
            let response = handle_request(&db_path, &cache_root, &service, &current);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{label}");
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store", "{label}");
        }
        std::fs::write(
            layout.thumbnail_path(ID, 2).unwrap(),
            vec![0u8; THUMBNAIL_MAX_BYTES as usize + 1],
        )
        .unwrap();
        let oversized = handle_request(&db_path, &cache_root, &service, &current);
        assert_eq!(oversized.status(), StatusCode::NOT_FOUND);
        assert_eq!(oversized.headers()[CACHE_CONTROL], "no-store");

        let detail_after = service.detail(ID).unwrap().unwrap().clipping;
        assert_eq!(detail_after.title, detail_before.title);
        assert_eq!(detail_after.note_markdown, detail_before.note_markdown);
        assert_eq!(detail_after.revision, detail_before.revision);
        assert_eq!(
            detail_after.asset_checksum_sha256,
            detail_before.asset_checksum_sha256
        );
        assert_eq!(
            std::fs::read(layout.canonical_path(ID).unwrap()).unwrap(),
            canonical_before
        );

        let stale = request_for_url(&format!(
            "http://newspaper-media.localhost/clipping-thumbnail/{ID}?v=1-2"
        ))
        .unwrap();
        assert_eq!(
            handle_request(&db_path, &cache_root, &service, &stale).status(),
            StatusCode::NOT_FOUND
        );
    }
}
