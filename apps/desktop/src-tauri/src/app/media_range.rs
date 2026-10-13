//! Provider-neutral HTTP Range serving for custom media URI schemes.
//!
//! Providers resolve an opaque id to a validated [`ResolvedFile`] on their own
//! side and hand it to [`serve_file`]. This module owns only the generic part:
//! `Range` parsing, 2 MiB chunking, MIME selection, and response headers. It
//! never looks at databases, manifests, or provider-specific path rules.

use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use tauri::http::{
    header::{
        ACCEPT_RANGES, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE,
        CONTENT_TYPE, RANGE,
    },
    HeaderValue, Request, Response, StatusCode,
};

/// Largest body returned for one request. WebView media elements ask again
/// for the rest, so open-ended ranges never load a whole video into memory.
pub const MAX_MEDIA_CHUNK_BYTES: u64 = 2 * 1024 * 1024;

/// A file the provider has already validated (inside its root, not a link,
/// non-empty). `len` is the length the provider measured.
pub struct ResolvedFile {
    pub path: PathBuf,
    pub len: u64,
    pub mime_type: &'static str,
}

/// Per-provider response wording and caching.
#[derive(Clone, Copy)]
pub struct ServeOptions {
    /// Body for a 500 when the file cannot be read.
    pub internal_error_body: &'static [u8],
    /// `Cache-Control` for successful media responses.
    pub cache_control: &'static str,
}

enum ByteRange {
    Full,
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}

pub fn clamp_range_end(start: u64, end: u64, max_chunk: u64) -> u64 {
    start.saturating_add(max_chunk.saturating_sub(1)).min(end)
}

pub fn serve_file(
    request: &Request<Vec<u8>>,
    file: ResolvedFile,
    options: ServeOptions,
) -> Response<Vec<u8>> {
    serve_file_chunked(request, file, MAX_MEDIA_CHUNK_BYTES, options)
}

pub fn serve_file_chunked(
    request: &Request<Vec<u8>>,
    file: ResolvedFile,
    max_chunk: u64,
    options: ServeOptions,
) -> Response<Vec<u8>> {
    match parse_byte_range(request.headers().get(RANGE), file.len) {
        ByteRange::Full => {
            if file.len == 0 {
                return text_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    options.internal_error_body.to_vec(),
                );
            }
            let end = clamp_range_end(0, file.len.saturating_sub(1), max_chunk);
            serve_file_span(
                &file,
                0,
                end,
                file.len,
                end >= file.len.saturating_sub(1),
                options,
            )
        }
        ByteRange::Partial { start, end } => {
            let end = clamp_range_end(start, end, max_chunk);
            serve_file_span(&file, start, end, file.len, false, options)
        }
        ByteRange::Unsatisfiable => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(CONTENT_LENGTH, "0")
            .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(ACCEPT_RANGES, "bytes")
            .header(CONTENT_RANGE, format!("bytes */{}", file.len))
            .header(CACHE_CONTROL, "no-store")
            .body(Vec::new())
            .unwrap_or_else(|_| fallback_internal()),
    }
}

fn serve_file_span(
    file: &ResolvedFile,
    start: u64,
    end: u64,
    total: u64,
    whole_file: bool,
    options: ServeOptions,
) -> Response<Vec<u8>> {
    match read_file_range(&file.path, start, end) {
        Ok(bytes) if whole_file => media_response(
            StatusCode::OK,
            bytes,
            file.mime_type,
            None,
            total,
            options.cache_control,
        ),
        Ok(bytes) => media_response(
            StatusCode::PARTIAL_CONTENT,
            bytes,
            file.mime_type,
            Some((start, end)),
            total,
            options.cache_control,
        ),
        Err(()) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            options.internal_error_body.to_vec(),
        ),
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

fn read_file_range(path: &Path, start: u64, end: u64) -> Result<Vec<u8>, ()> {
    let len = end.saturating_sub(start).saturating_add(1);
    let len_usize = usize::try_from(len).map_err(|_| ())?;
    let mut file = fs::File::open(path).map_err(|_| ())?;
    file.seek(SeekFrom::Start(start)).map_err(|_| ())?;
    let mut buf = vec![0_u8; len_usize];
    file.read_exact(&mut buf).map_err(|_| ())?;
    Ok(buf)
}

pub fn mime_for_path(path: &str) -> &'static str {
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
    cache_control: &str,
) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(CONTENT_TYPE, mime_type)
        .header(ACCEPT_RANGES, "bytes")
        .header(CACHE_CONTROL, cache_control);
    if let Some((start, end)) = range {
        builder = builder.header(CONTENT_RANGE, format!("bytes {start}-{end}/{total}"));
    }
    builder.body(body).unwrap_or_else(|_| fallback_internal())
}

pub fn text_response(status: StatusCode, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| fallback_internal())
}

/// A complete in-memory body (for example generated WebVTT) with the same
/// CORS policy as media and no caching.
pub fn bytes_response(content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(CONTENT_TYPE, content_type)
        .header(CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| fallback_internal())
}

pub fn fallback_internal() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

pub fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
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

    const OPTIONS: ServeOptions = ServeOptions {
        internal_error_body: b"test failure",
        cache_control: "private, no-cache",
    };

    fn file_with(bytes: &[u8]) -> (tempfile::TempDir, ResolvedFile) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clip.mp4");
        fs::write(&path, bytes).unwrap();
        let file = ResolvedFile {
            path,
            len: bytes.len() as u64,
            mime_type: "video/mp4",
        };
        (directory, file)
    }

    fn request(range: Option<&str>) -> Request<Vec<u8>> {
        let mut builder = Request::builder().uri("http://test.localhost/x");
        if let Some(range) = range {
            builder = builder.header(RANGE, range);
        }
        builder.body(Vec::new()).unwrap()
    }

    fn content_range(response: &Response<Vec<u8>>) -> Option<&str> {
        response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
    }

    #[test]
    fn small_file_without_range_is_a_full_200() {
        let (_dir, file) = file_with(b"0123456789");
        let response = serve_file(&request(None), file, OPTIONS);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body().as_slice(), b"0123456789");
        assert!(content_range(&response).is_none());
    }

    #[test]
    fn explicit_range_returns_206_with_content_range() {
        let (_dir, file) = file_with(b"0123456789");
        let response = serve_file(&request(Some("bytes=2-5")), file, OPTIONS);
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.body().as_slice(), b"2345");
        assert_eq!(content_range(&response), Some("bytes 2-5/10"));
    }

    #[test]
    fn open_ended_and_suffix_ranges_resolve_against_the_length() {
        let (_dir, file) = file_with(b"0123456789");
        let open = serve_file(&request(Some("bytes=7-")), file, OPTIONS);
        assert_eq!(open.body().as_slice(), b"789");
        assert_eq!(content_range(&open), Some("bytes 7-9/10"));
        let (_dir, file) = file_with(b"0123456789");
        let suffix = serve_file(&request(Some("bytes=-4")), file, OPTIONS);
        assert_eq!(suffix.body().as_slice(), b"6789");
        assert_eq!(content_range(&suffix), Some("bytes 6-9/10"));
    }

    #[test]
    fn range_beyond_the_file_is_416() {
        let (_dir, file) = file_with(b"0123456789");
        let response = serve_file(&request(Some("bytes=10-20")), file, OPTIONS);
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(content_range(&response), Some("bytes */10"));
        assert!(response.body().is_empty());
    }

    #[test]
    fn malformed_or_multi_ranges_fall_back_to_a_full_response() {
        for header in ["bytes=a-b", "items=0-3", "bytes=0-1,3-4"] {
            let (_dir, file) = file_with(b"0123456789");
            let response = serve_file(&request(Some(header)), file, OPTIONS);
            assert_eq!(response.status(), StatusCode::OK, "{header}");
            assert_eq!(response.body().len(), 10, "{header}");
        }
    }

    #[test]
    fn open_ended_reads_are_clamped_to_the_chunk_size() {
        let (_dir, file) = file_with(&[7_u8; 32]);
        let response = serve_file_chunked(&request(Some("bytes=4-")), file, 8, OPTIONS);
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.body().len(), 8);
        assert_eq!(content_range(&response), Some("bytes 4-11/32"));
    }

    #[test]
    fn missing_range_on_a_large_file_returns_the_first_chunk() {
        let (_dir, file) = file_with(&[7_u8; 32]);
        let response = serve_file_chunked(&request(None), file, 8, OPTIONS);
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(content_range(&response), Some("bytes 0-7/32"));
    }

    #[test]
    fn clamp_caps_open_ended_reads() {
        assert_eq!(clamp_range_end(0, 9_999_999, 2_000_000), 1_999_999);
        assert_eq!(clamp_range_end(0, 100, 2_000_000), 100);
        assert_eq!(clamp_range_end(4, 50, 8), 11);
    }

    #[test]
    fn vanished_file_reports_the_provider_error_body() {
        let (_dir, file) = file_with(b"0123456789");
        fs::remove_file(&file.path).unwrap();
        let response = serve_file(&request(None), file, OPTIONS);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.body().as_slice(), b"test failure");
    }

    #[test]
    fn mime_follows_the_extension() {
        assert_eq!(mime_for_path("a.MP4"), "video/mp4");
        assert_eq!(mime_for_path("a.webm"), "video/webm");
        assert_eq!(mime_for_path("a.mkv"), "video/x-matroska");
        assert_eq!(mime_for_path("a.bin"), "application/octet-stream");
    }
}
