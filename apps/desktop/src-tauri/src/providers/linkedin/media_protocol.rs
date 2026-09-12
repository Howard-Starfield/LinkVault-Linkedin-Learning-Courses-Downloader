//! LinkedIn video bytes for `linkedin-media://v/{artifact_id}`.
//!
//! Registered beside newspaper-media. Does not import newspaper code.

use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use rusqlite::{params, OptionalExtension};
use tauri::http::{
    header::{
        ACCEPT_RANGES, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE,
        CONTENT_TYPE, RANGE,
    },
    HeaderValue, Request, Response, StatusCode,
};

pub fn handle_request(db_path: &Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    match resolve_media(db_path, request) {
        Ok(media) => serve_media(request, media),
        Err(MediaError::BadRequest) => text_response(
            StatusCode::BAD_REQUEST,
            b"Invalid LinkedIn media request.".to_vec(),
        ),
        Err(MediaError::NotFound) => text_response(
            StatusCode::NOT_FOUND,
            b"LinkedIn media is unavailable.".to_vec(),
        ),
        Err(MediaError::Internal) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            b"LinkedIn media could not be loaded.".to_vec(),
        ),
    }
}

struct ResolvedMedia {
    path: PathBuf,
    len: u64,
    mime_type: &'static str,
}

enum ByteRange {
    Full,
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}

enum MediaError {
    BadRequest,
    NotFound,
    Internal,
}

fn resolve_media(db_path: &Path, request: &Request<Vec<u8>>) -> Result<ResolvedMedia, MediaError> {
    let artifact_id = parse_artifact_id(request)?;
    let connection = crate::cache::open_runtime(db_path).map_err(|_| MediaError::Internal)?;
    let record: Option<(String, String)> = connection
        .query_row(
            "SELECT a.path, j.output_dir
             FROM artifacts a
             INNER JOIN jobs j ON j.id = a.job_id
             WHERE a.id = ?1",
            params![artifact_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| MediaError::Internal)?;
    let Some((artifact_path, output_dir)) = record else {
        return Err(MediaError::NotFound);
    };
    let canonical_root = Path::new(&output_dir)
        .canonicalize()
        .map_err(|_| MediaError::NotFound)?;
    let canonical_path = Path::new(&artifact_path)
        .canonicalize()
        .map_err(|_| MediaError::NotFound)?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(MediaError::NotFound);
    }
    let metadata = fs::symlink_metadata(&canonical_path).map_err(|_| MediaError::NotFound)?;
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(MediaError::NotFound);
    }
    Ok(ResolvedMedia {
        path: canonical_path,
        len: metadata.len(),
        mime_type: mime_for_path(&artifact_path),
    })
}

fn parse_artifact_id(request: &Request<Vec<u8>>) -> Result<String, MediaError> {
    let uri = request.uri();
    let host = uri.host().unwrap_or("");
    let segments = uri
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments
        .iter()
        .any(|segment| *segment == "." || *segment == "..")
    {
        return Err(MediaError::BadRequest);
    }
    let artifact_id = if host == "v" && segments.len() == 1 {
        segments[0]
    } else if segments.len() == 2 && segments[0] == "v" {
        segments[1]
    } else {
        return Err(MediaError::BadRequest);
    };
    if artifact_id.contains("..")
        || artifact_id.contains('/')
        || artifact_id.contains('\\')
        || !artifact_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        return Err(MediaError::BadRequest);
    }
    Ok(artifact_id.to_string())
}

fn serve_media(request: &Request<Vec<u8>>, media: ResolvedMedia) -> Response<Vec<u8>> {
    match parse_byte_range(request.headers().get(RANGE), media.len) {
        ByteRange::Full => match read_file_range(&media.path, 0, media.len.saturating_sub(1)) {
            Ok(bytes) => media_response(StatusCode::OK, bytes, media.mime_type, None, media.len),
            Err(_) => text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                b"LinkedIn media could not be loaded.".to_vec(),
            ),
        },
        ByteRange::Partial { start, end } => match read_file_range(&media.path, start, end) {
            Ok(bytes) => media_response(
                StatusCode::PARTIAL_CONTENT,
                bytes,
                media.mime_type,
                Some((start, end)),
                media.len,
            ),
            Err(_) => text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                b"LinkedIn media could not be loaded.".to_vec(),
            ),
        },
        ByteRange::Unsatisfiable => {
            let builder = Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(CONTENT_LENGTH, "0")
                .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(ACCEPT_RANGES, "bytes")
                .header(CONTENT_RANGE, format!("bytes */{}", media.len))
                .header(CACHE_CONTROL, "no-store");
            builder
                .body(Vec::new())
                .unwrap_or_else(|_| fallback_internal())
        }
    }
}

fn parse_byte_range(header: Option<&HeaderValue>, total: u64) -> ByteRange {
    let Some(value) = header.and_then(|value| value.to_str().ok()) else {
        return ByteRange::Full;
    };
    let value = value.trim();
    if value.is_empty() {
        return ByteRange::Full;
    }
    let Some(spec) = value.strip_prefix("bytes=") else {
        return ByteRange::Full;
    };
    if spec.contains(',') {
        return ByteRange::Full;
    }
    let Some((start_raw, end_raw)) = spec.split_once('-') else {
        return ByteRange::Full;
    };
    if total == 0 {
        return ByteRange::Unsatisfiable;
    }
    if start_raw.is_empty() {
        let Ok(suffix) = end_raw.parse::<u64>() else {
            return ByteRange::Full;
        };
        if suffix == 0 {
            return ByteRange::Unsatisfiable;
        }
        return ByteRange::Partial {
            start: total.saturating_sub(suffix),
            end: total - 1,
        };
    }
    let Ok(start) = start_raw.parse::<u64>() else {
        return ByteRange::Full;
    };
    if start >= total {
        return ByteRange::Unsatisfiable;
    }
    let end = if end_raw.is_empty() {
        total - 1
    } else {
        let Ok(end) = end_raw.parse::<u64>() else {
            return ByteRange::Full;
        };
        end.min(total - 1)
    };
    if start > end {
        return ByteRange::Unsatisfiable;
    }
    ByteRange::Partial { start, end }
}

fn read_file_range(path: &Path, start: u64, end: u64) -> Result<Vec<u8>, MediaError> {
    let len = end.saturating_sub(start).saturating_add(1);
    let len_usize = usize::try_from(len).map_err(|_| MediaError::Internal)?;
    let mut file = fs::File::open(path).map_err(|_| MediaError::Internal)?;
    file.seek(SeekFrom::Start(start))
        .map_err(|_| MediaError::Internal)?;
    let mut buf = vec![0_u8; len_usize];
    file.read_exact(&mut buf)
        .map_err(|_| MediaError::Internal)?;
    Ok(buf)
}

fn mime_for_path(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

fn media_response(
    status: StatusCode,
    body: Vec<u8>,
    mime_type: &str,
    range: Option<(u64, u64)>,
    total: u64,
) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(CONTENT_TYPE, mime_type)
        .header(ACCEPT_RANGES, "bytes")
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable");
    if let Some((start, end)) = range {
        builder = builder.header(CONTENT_RANGE, format!("bytes {start}-{end}/{total}"));
    }
    builder.body(body).unwrap_or_else(|_| fallback_internal())
}

fn text_response(status: StatusCode, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| fallback_internal())
}

fn fallback_internal() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::initialize_database;
    use rusqlite::params;

    fn request_for_url(url: &str) -> Request<Vec<u8>> {
        Request::builder()
            .uri(url)
            .body(Vec::new())
            .expect("test media request URI")
    }

    fn harness() -> (tempfile::TempDir, std::path::PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        let output_dir = directory.path().join("downloads");
        fs::create_dir_all(&output_dir).unwrap();
        let video_path = output_dir.join("welcome.mp4");
        fs::write(&video_path, b"fake-mp4-bytes-for-range").unwrap();
        connection
            .execute(
                "INSERT INTO jobs (
                    id, course_slug, source_url, status, selected_quality,
                    download_videos, download_exercises, download_subtitles, download_quizzes,
                    quiz_hints_json, output_dir, paused, created_at, updated_at
                 ) VALUES ('job-1', 'practical-github-actions', 'https://www.linkedin.com/learning/practical-github-actions',
                    'completed', '720', 1, 1, 1, 1, '[]', ?1, 0, 10, 10)",
                params![output_dir.to_string_lossy().to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO artifacts (
                    id, job_id, artifact_type, path, status, size_bytes, created_at, updated_at
                 ) VALUES ('artifact-welcome', 'job-1', 'video', ?1, 'completed', 24, 10, 10)",
                params![video_path.to_string_lossy().to_string()],
            )
            .unwrap();
        drop(connection);
        (directory, db_path, video_path)
    }

    #[test]
    fn rejects_parent_directory_artifact_ids() {
        let (_directory, db_path, _video_path) = harness();
        let response = handle_request(
            &db_path,
            &request_for_url("http://linkedin-media.localhost/v/../secret"),
        );
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let dotted = Request::builder()
            .uri("linkedin-media://v/..")
            .body(Vec::new());
        if let Ok(request) = dotted {
            let response = handle_request(&db_path, &request);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn rejects_unknown_artifact_id() {
        let (_directory, db_path, _video_path) = harness();
        let response = handle_request(
            &db_path,
            &request_for_url("linkedin-media://v/missing-artifact"),
        );
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn serves_known_artifact_bytes() {
        let (_directory, db_path, _video_path) = harness();
        let response = handle_request(
            &db_path,
            &request_for_url("linkedin-media://v/artifact-welcome"),
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body().as_slice(), b"fake-mp4-bytes-for-range");
    }
}
