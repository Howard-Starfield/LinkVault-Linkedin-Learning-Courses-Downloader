//! LinkedIn video bytes for `linkedin-media://v/{artifact_id}`.
//!
//! Registered beside newspaper-media. Does not import newspaper code. Range
//! and MIME handling is the provider-neutral `app::media_range` module.

use std::{fs, path::Path};

use rusqlite::{params, OptionalExtension};
use tauri::http::{Request, Response, StatusCode};

use crate::app::media_range::{
    is_symlink_or_reparse, mime_for_path, serve_file, text_response, ResolvedFile, ServeOptions,
};

const SERVE_OPTIONS: ServeOptions = ServeOptions {
    internal_error_body: b"LinkedIn media could not be loaded.",
    cache_control: "private, max-age=31536000, immutable",
};

type ResolvedMedia = ResolvedFile;

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
    serve_file(request, media, SERVE_OPTIONS)
}

#[cfg(test)]
fn serve_media_chunked(
    request: &Request<Vec<u8>>,
    media: ResolvedMedia,
    max_chunk: u64,
) -> Response<Vec<u8>> {
    crate::app::media_range::serve_file_chunked(request, media, max_chunk, SERVE_OPTIONS)
}

#[cfg(test)]
fn clamp_media_range_end(start: u64, end: u64, max_chunk: u64) -> u64 {
    crate::app::media_range::clamp_range_end(start, end, max_chunk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::initialize_database;
    use rusqlite::params;
    use std::path::PathBuf;
    use tauri::http::header::{CONTENT_RANGE, CONTENT_TYPE, RANGE};

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

    #[test]
    fn serves_windows_localhost_artifact_bytes() {
        let (_directory, db_path, _video_path) = harness();
        let response = handle_request(
            &db_path,
            &request_for_url("http://linkedin-media.localhost/v/artifact-welcome"),
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body().as_slice(), b"fake-mp4-bytes-for-range");
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .map(|value| value.as_bytes()),
            Some(b"video/mp4".as_slice())
        );
    }

    #[test]
    fn serves_windows_localhost_byte_range() {
        let (_directory, db_path, _video_path) = harness();
        let request = Request::builder()
            .uri("http://linkedin-media.localhost/v/artifact-welcome")
            .header(RANGE, "bytes=0-3")
            .body(Vec::new())
            .expect("ranged media request");
        let response = handle_request(&db_path, &request);
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.body().as_slice(), b"fake");
    }

    #[test]
    fn clamp_media_range_end_caps_open_ended_reads() {
        assert_eq!(clamp_media_range_end(0, 9_999_999, 2_000_000), 1_999_999);
        assert_eq!(clamp_media_range_end(0, 100, 2_000_000), 100);
        assert_eq!(clamp_media_range_end(4, 50, 8), 11);
    }

    #[test]
    fn missing_range_on_large_file_returns_first_chunk() {
        let (_directory, _db_path, video_path) = harness();
        fs::write(&video_path, vec![7_u8; 32]).unwrap();
        let media = ResolvedMedia {
            path: video_path,
            len: 32,
            mime_type: "video/mp4",
        };
        let request = request_for_url("http://linkedin-media.localhost/v/artifact-welcome");
        let response = serve_media_chunked(&request, media, 8);
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.body().len(), 8);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            Some("bytes 0-7/32")
        );
    }
}
