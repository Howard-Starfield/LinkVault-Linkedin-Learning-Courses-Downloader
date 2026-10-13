//! Video and caption bytes for `youtube-media://{media|subs}/{runId}/{occurrenceId}`.
//!
//! URLs carry opaque ids only. Every request re-resolves the file from durable
//! state (run output root, work item, item manifest) through
//! `playback::locate_item_files`; nothing here accepts a path. Range and
//! response mechanics are the shared `app::media_range` module.

use std::path::Path;

use tauri::http::{Request, Response, StatusCode};

use crate::app::media_range::{
    bytes_response, serve_file, text_response, ResolvedFile, ServeOptions,
};
use crate::providers::youtube::playback::{
    is_valid_id, load_run, locate_item_files, mime_for_media, subtitles_body, MediaKind,
    PlaybackError,
};

const SERVE_OPTIONS: ServeOptions = ServeOptions {
    internal_error_body: b"YouTube media could not be loaded.",
    // Ids are stable but the files behind them can be replaced, so media is
    // always revalidated rather than cached as immutable.
    cache_control: "private, no-cache",
};

pub fn handle_request(db_path: &Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let (kind, run_id, occurrence_id) = match parse_request(request) {
        Ok(parsed) => parsed,
        Err(()) => {
            return text_response(
                StatusCode::BAD_REQUEST,
                b"Invalid YouTube media request.".to_vec(),
            )
        }
    };
    let not_found = || {
        text_response(
            StatusCode::NOT_FOUND,
            b"YouTube media is unavailable.".to_vec(),
        )
    };
    let Ok(connection) = crate::cache::open_runtime(db_path) else {
        return text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            b"YouTube media could not be loaded.".to_vec(),
        );
    };
    let run = match load_run(&connection, &run_id) {
        Ok(run) => run,
        Err(PlaybackError::Database) => {
            return text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                b"YouTube media could not be loaded.".to_vec(),
            )
        }
        Err(_) => return not_found(),
    };
    drop(connection);
    let Ok(item) = run.item(&occurrence_id) else {
        return not_found();
    };
    let Ok(files) = locate_item_files(&run.output_root, item) else {
        return not_found();
    };
    match kind {
        MediaKind::Media => {
            let Some(media) = files.media else {
                return not_found();
            };
            let mime_type = mime_for_media(&media.name);
            serve_file(
                request,
                ResolvedFile {
                    path: media.path,
                    len: media.len,
                    mime_type,
                },
                SERVE_OPTIONS,
            )
        }
        MediaKind::Subs => match subtitles_body(&files) {
            Some(body) => bytes_response("text/vtt", body),
            None => not_found(),
        },
    }
}

/// `http://youtube-media.localhost/{kind}/{run}/{occurrence}` (Windows) or
/// `youtube-media://localhost/{kind}/{run}/{occurrence}`; the legacy
/// `youtube-media://{kind}/{run}/{occurrence}` host form is also accepted.
fn parse_request(request: &Request<Vec<u8>>) -> Result<(MediaKind, String, String), ()> {
    let uri = request.uri();
    let segments = uri
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let (kind, run_id, occurrence_id) = match segments.as_slice() {
        [kind, run_id, occurrence_id] => (*kind, *run_id, *occurrence_id),
        [run_id, occurrence_id] => (uri.host().unwrap_or(""), *run_id, *occurrence_id),
        _ => return Err(()),
    };
    let kind = MediaKind::parse(kind).ok_or(())?;
    if !is_valid_id(run_id) || !is_valid_id(occurrence_id) {
        return Err(());
    }
    Ok((kind, run_id.to_string(), occurrence_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::initialize_database;
    use crate::providers::youtube::playback::tests::{
        insert_run, published_fixture, work_item, Fixture, OCC, RUN,
    };
    use std::fs;
    use tauri::http::header::{CONTENT_RANGE, CONTENT_TYPE, RANGE};

    fn request(url: &str, range: Option<&str>) -> Request<Vec<u8>> {
        let mut builder = Request::builder().uri(url);
        if let Some(range) = range {
            builder = builder.header(RANGE, range);
        }
        builder.body(Vec::new()).expect("test request")
    }

    fn harness() -> (Fixture, std::path::PathBuf) {
        let fixture = published_fixture();
        let db_path = fixture.dir.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        insert_run(
            &connection,
            RUN,
            "youtube_download",
            &fixture.root,
            vec![fixture.item.clone(), work_item(2)],
        );
        insert_run(
            &connection,
            "run-li",
            "linkedin_download",
            &fixture.root,
            vec![fixture.item.clone()],
        );
        (fixture, db_path)
    }

    fn media_url(kind: &str, run: &str, occurrence: &str) -> String {
        format!("http://youtube-media.localhost/{kind}/{run}/{occurrence}")
    }

    #[test]
    fn serves_media_bytes_and_ranges_from_durable_state() {
        let (_fixture, db_path) = harness();
        let url = media_url("media", RUN, OCC);
        let full = handle_request(&db_path, &request(&url, None));
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.body().as_slice(), b"12345678");
        assert_eq!(
            full.headers()
                .get(CONTENT_TYPE)
                .map(|value| value.as_bytes()),
            Some(b"video/mp4".as_slice())
        );
        let ranged = handle_request(&db_path, &request(&url, Some("bytes=2-4")));
        assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(ranged.body().as_slice(), b"345");
        assert_eq!(
            ranged
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok()),
            Some("bytes 2-4/8")
        );
        let beyond = handle_request(&db_path, &request(&url, Some("bytes=8-")));
        assert_eq!(beyond.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[test]
    fn serves_generated_webvtt_as_text_vtt() {
        let (_fixture, db_path) = harness();
        let response = handle_request(&db_path, &request(&media_url("subs", RUN, OCC), None));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .map(|value| value.as_bytes()),
            Some(b"text/vtt".as_slice())
        );
        let body = String::from_utf8(response.body().clone()).unwrap();
        assert!(body.starts_with("WEBVTT\n\n"));
        assert!(body.contains("hello"));
    }

    #[test]
    fn accepts_the_non_windows_url_form() {
        let (_fixture, db_path) = harness();
        let response = handle_request(
            &db_path,
            &request(
                &format!("youtube-media://localhost/media/{RUN}/{OCC}"),
                None,
            ),
        );
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn unknown_run_unknown_occurrence_and_wrong_workflow_are_404() {
        let (_fixture, db_path) = harness();
        for url in [
            media_url("media", "missing-run", OCC),
            media_url("media", RUN, "occ-missing"),
            media_url("media", "run-li", OCC),
            // Known occurrence whose folder was never downloaded.
            media_url("media", RUN, "occ-2"),
        ] {
            let response = handle_request(&db_path, &request(&url, None));
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{url}");
        }
    }

    #[test]
    fn malformed_urls_are_400() {
        let (_fixture, db_path) = harness();
        for url in [
            "http://youtube-media.localhost/media/..%2F/occ-1".to_string(),
            "http://youtube-media.localhost/media/run-1/..".to_string(),
            "http://youtube-media.localhost/media/run-1".to_string(),
            "http://youtube-media.localhost/video/run-1/occ-1".to_string(),
            "http://youtube-media.localhost/media/run-1/occ-1/extra".to_string(),
            "http://youtube-media.localhost/".to_string(),
            "http://youtube-media.localhost/media/C:/occ-1".to_string(),
        ] {
            let response = handle_request(&db_path, &request(&url, None));
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{url}");
        }
    }

    #[test]
    fn a_file_replaced_after_download_is_not_served() {
        let (fixture, db_path) = harness();
        fs::write(
            fixture.item_dir.join("media.mp4"),
            b"tampered-with-different-size",
        )
        .unwrap();
        let response = handle_request(&db_path, &request(&media_url("media", RUN, OCC), None));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn captions_are_404_when_the_item_has_none() {
        let (fixture, db_path) = harness();
        fs::remove_file(fixture.item_dir.join("video.en.vtt")).unwrap();
        fs::remove_file(fixture.item_dir.join("video.en.transcript.json")).unwrap();
        let response = handle_request(&db_path, &request(&media_url("subs", RUN, OCC), None));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
