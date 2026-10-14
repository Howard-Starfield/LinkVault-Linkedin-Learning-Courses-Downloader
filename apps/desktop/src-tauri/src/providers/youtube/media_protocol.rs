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
                    file: media.file,
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
