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

pub fn handle_request(
    db_path: &Path,
    cache_root: &Path,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    match resolve_media(db_path, cache_root, request) {
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
    request: &Request<Vec<u8>>,
) -> Result<ResolvedMedia, MediaError> {
    let segments = request
        .uri()
        .path()
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if segments.len() != 2 || !valid_id(segments[1]) {
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
    let connection = Connection::open(db_path).map_err(|_| MediaError::Internal)?;

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
    use tempfile::tempdir;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let cache_root = directory.path().join("newspaper-thumbnails").join("v1");
        std::fs::create_dir_all(&cache_root).unwrap();
        let page_path = directory.path().join("A01.jpg");
        std::fs::write(&page_path, b"page-bytes").unwrap();
        let thumbnail_path = cache_root.join("job.webp");
        std::fs::write(&thumbnail_path, b"thumbnail-bytes").unwrap();
        let connection = Connection::open(&db_path).unwrap();
        super::super::storage::initialize(&connection).unwrap();
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
        (directory, db_path, cache_root)
    }

    #[test]
    fn registered_page_and_thumbnail_receive_cacheable_responses() {
        let (_directory, db_path, cache_root) = fixture();
        let page = request_for_url("http://newspaper-media.localhost/page/page?v=3").unwrap();
        let page_response = handle_request(&db_path, &cache_root, &page);
        assert_eq!(page_response.status(), StatusCode::OK);
        assert_eq!(page_response.headers()[CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            page_response.headers()[CACHE_CONTROL],
            "private, max-age=31536000, immutable"
        );

        let thumbnail =
            request_for_url("http://newspaper-media.localhost/thumbnail/job?v=3-1").unwrap();
        let thumbnail_response = handle_request(&db_path, &cache_root, &thumbnail);
        assert_eq!(thumbnail_response.status(), StatusCode::OK);
        assert_eq!(thumbnail_response.headers()[CONTENT_TYPE], "image/webp");
    }

    #[test]
    fn malformed_unknown_and_stale_requests_are_rejected_without_paths() {
        let (directory, db_path, cache_root) = fixture();
        for url in [
            "http://newspaper-media.localhost/page/..%2Fsecret?v=3",
            "http://newspaper-media.localhost/page/missing?v=3",
            "http://newspaper-media.localhost/page/page?v=2",
            "http://newspaper-media.localhost/other/page?v=3",
        ] {
            let request = request_for_url(url).unwrap();
            let response = handle_request(&db_path, &cache_root, &request);
            assert!(!response.status().is_success());
            let body = String::from_utf8_lossy(response.body());
            assert!(!body.contains(&directory.path().to_string_lossy().to_string()));
        }
    }
}
