//! In-app playback of finished YouTube downloads.
//!
//! Everything here re-derives what to play from durable state on every call:
//! `workflow_runs.output_root` and the run's recorded work items give the item
//! folder name (the same function the executor publishes under), the folder's
//! `manifest.json` names the artifacts, and each artifact is checked on disk
//! before a byte is served. URLs handed to the webview carry only opaque ids,
//! never paths, and nothing is discovered by scanning folders.

use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriteError, DatabaseWriter};
use crate::app::managed_process::TransientWorkItem;
use crate::app::media_range::is_symlink_or_reparse;
use crate::app::safe_output_filesystem::validate_output_component;
use crate::providers::youtube::commands::YouTubeState;
use crate::providers::youtube::error::YouTubeError;
use crate::providers::youtube::executor::{published_item_name, MAX_MANIFEST_BYTES};
use crate::providers::youtube::live::YoutubeDurableRequest;
use crate::providers::youtube::manifest_contract::{
    ManifestArtifact, ManifestArtifactKind, YouTubeArtifactManifest, MANIFEST_SCHEMA_VERSION,
    YOUTUBE_PROVIDER,
};
use crate::providers::youtube::models::{
    GetYouTubePlaybackProgressRequest, ListYouTubeRunVideosRequest,
    SaveYouTubePlaybackProgressRequest, YouTubePlaybackProgress, YouTubeRunVideo,
};
use crate::providers::youtube::transcript_normalizer::{
    NormalizedTranscript, MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES,
    NORMALIZED_TRANSCRIPT_SCHEMA_VERSION,
};

/// Longest subtitle document served (generated or raw). Longer transcripts are
/// cut at a cue boundary; a raw `.vtt` over the cap is not offered.
pub(super) const MAX_SUBTITLE_BYTES: usize = 16 * 1024 * 1024;
/// Longest position or duration accepted from the webview (seven days).
const MAX_PLAYBACK_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;
/// A saved position may not exceed the duration by more than this.
const POSITION_SLACK_SECONDS: f64 = 5.0;
const MAX_ID_LEN: usize = 128;
const YOUTUBE_WORKFLOW_TYPE: &str = "youtube_download";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MediaKind {
    Media,
    Subs,
}

impl MediaKind {
    fn segment(self) -> &'static str {
        match self {
            Self::Media => "media",
            Self::Subs => "subs",
        }
    }

    pub(super) fn parse(segment: &str) -> Option<Self> {
        match segment {
            "media" => Some(Self::Media),
            "subs" => Some(Self::Subs),
            _ => None,
        }
    }
}

/// Opaque playback URL. Windows/Android route custom schemes through
/// `http://<scheme>.localhost`, matching how `linkedin-media` URLs are minted.
pub(super) fn media_url(kind: MediaKind, run_id: &str, occurrence_id: &str) -> String {
    #[cfg(any(target_os = "windows", target_os = "android"))]
    {
        format!(
            "http://youtube-media.localhost/{}/{run_id}/{occurrence_id}",
            kind.segment()
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    {
        format!(
            "youtube-media://localhost/{}/{run_id}/{occurrence_id}",
            kind.segment()
        )
    }
}

/// Ids are opaque tokens minted by this app (hex digests); reject anything
/// else before it reaches SQL, a URL, or a log line.
pub(super) fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PlaybackError {
    RunNotFound,
    ItemNotFound,
    Database,
}

/// Why an item cannot be played right now. Never an error for the listing.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Unavailable {
    RootMissing,
    FolderMissing,
    Unsafe,
    ManifestMissing,
    ManifestInvalid,
    NoVideo,
    FileMissing,
    FileChanged,
}

impl Unavailable {
    pub(super) fn reason(&self) -> &'static str {
        match self {
            Self::RootMissing => "The download folder is not available.",
            Self::FolderMissing => {
                "The downloaded files were not found. They may have been moved or deleted, or this video did not finish downloading."
            }
            Self::Unsafe => "The saved files are not in a safe location, so they were not opened.",
            Self::ManifestMissing => "This video's saved record is missing.",
            Self::ManifestInvalid => "This video's saved record is damaged.",
            Self::NoVideo => "Only the transcript was saved for this video.",
            Self::FileMissing => "The video file is missing.",
            Self::FileChanged => "The video file has changed since it was downloaded.",
        }
    }
}

/// A file proven to be a regular, non-link, non-empty child of the item folder
/// whose size matches its manifest entry.
#[derive(Debug, Clone)]
pub(super) struct VerifiedFile {
    pub path: PathBuf,
    pub len: u64,
    pub name: String,
}

#[derive(Debug, Default)]
pub(super) struct ItemFiles {
    pub media: Option<VerifiedFile>,
    pub vtt: Option<VerifiedFile>,
    pub transcript_json: Option<VerifiedFile>,
}

impl ItemFiles {
    pub(super) fn has_captions(&self) -> bool {
        self.vtt.is_some() || self.transcript_json.is_some()
    }
}

pub(super) struct RunPlayback {
    pub output_root: String,
    pub request: YoutubeDurableRequest,
}

pub(super) fn load_run(
    connection: &Connection,
    run_id: &str,
) -> Result<RunPlayback, PlaybackError> {
    if !is_valid_id(run_id) {
        return Err(PlaybackError::RunNotFound);
    }
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT workflow_type, output_root, request_json FROM workflow_runs WHERE id = ?1",
            params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| PlaybackError::Database)?;
    let Some((workflow_type, output_root, request_json)) = row else {
        return Err(PlaybackError::RunNotFound);
    };
    if workflow_type != YOUTUBE_WORKFLOW_TYPE {
        return Err(PlaybackError::RunNotFound);
    }
    let request: YoutubeDurableRequest =
        serde_json::from_str(&request_json).map_err(|_| PlaybackError::RunNotFound)?;
    Ok(RunPlayback {
        output_root,
        request,
    })
}

impl RunPlayback {
    pub(super) fn item(&self, occurrence_id: &str) -> Result<&TransientWorkItem, PlaybackError> {
        if !is_valid_id(occurrence_id) {
            return Err(PlaybackError::ItemNotFound);
        }
        self.request
            .work_items
            .iter()
            .find(|item| item.occurrence_id == occurrence_id)
            .ok_or(PlaybackError::ItemNotFound)
    }
}

/// Resolve one item's playable files from the run's output root.
///
/// Containment rules: the item folder must be a direct child of the canonical
/// output root; every artifact a direct child of the canonical item folder; no
/// symlink or reparse point at any of those levels; each file non-empty with
/// exactly the size the manifest recorded. The content hash is deliberately not
/// recomputed here (it runs once per Range request); the executor verified it
/// when the folder was published.
pub(super) fn locate_item_files(
    output_root: &str,
    item: &TransientWorkItem,
) -> Result<ItemFiles, Unavailable> {
    let root = Path::new(output_root);
    if !root.is_absolute() {
        return Err(Unavailable::RootMissing);
    }
    let root_metadata = fs::symlink_metadata(root).map_err(|_| Unavailable::RootMissing)?;
    if is_symlink_or_reparse(&root_metadata) || !root_metadata.file_type().is_dir() {
        return Err(Unavailable::RootMissing);
    }
    let canonical_root = root.canonicalize().map_err(|_| Unavailable::RootMissing)?;

    let item_name = published_item_name(root, item.ordinal, &item.title, &item.video_id)
        .map_err(|_| Unavailable::FolderMissing)?;
    let item_dir = root.join(&item_name);
    let dir_metadata = match fs::symlink_metadata(&item_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Unavailable::FolderMissing)
        }
        Err(_) => return Err(Unavailable::Unsafe),
    };
    if is_symlink_or_reparse(&dir_metadata) || !dir_metadata.file_type().is_dir() {
        return Err(Unavailable::Unsafe);
    }
    let canonical_dir = item_dir
        .canonicalize()
        .map_err(|_| Unavailable::FolderMissing)?;
    if canonical_dir.parent() != Some(canonical_root.as_path()) {
        return Err(Unavailable::Unsafe);
    }

    let manifest = read_manifest(&canonical_dir, item)?;

    let mut files = ItemFiles::default();
    for artifact in &manifest.artifacts {
        let slot = match artifact.kind {
            ManifestArtifactKind::Media => &mut files.media,
            ManifestArtifactKind::Vtt => &mut files.vtt,
            ManifestArtifactKind::TranscriptJson => &mut files.transcript_json,
            ManifestArtifactKind::Metadata => continue,
        };
        if slot.is_some() {
            continue;
        }
        match verify_artifact(&canonical_dir, artifact) {
            Ok(file) => *slot = Some(file),
            // Captions are optional: a bad caption file only drops captions.
            Err(error) if matches!(artifact.kind, ManifestArtifactKind::Media) => {
                return Err(error)
            }
            Err(_) => {}
        }
    }
    if files.media.is_none() {
        return Err(Unavailable::NoVideo);
    }
    Ok(files)
}

fn read_manifest(
    canonical_dir: &Path,
    item: &TransientWorkItem,
) -> Result<YouTubeArtifactManifest, Unavailable> {
    let path = canonical_dir.join("manifest.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Unavailable::ManifestMissing)
        }
        Err(_) => return Err(Unavailable::ManifestInvalid),
    };
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_file() {
        return Err(Unavailable::Unsafe);
    }
    if metadata.len() == 0 || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(Unavailable::ManifestInvalid);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&path)
        .and_then(|file| file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| Unavailable::ManifestInvalid)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(Unavailable::ManifestInvalid);
    }
    let manifest: YouTubeArtifactManifest =
        serde_json::from_slice(&bytes).map_err(|_| Unavailable::ManifestInvalid)?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION
        || manifest.provider != YOUTUBE_PROVIDER
        || manifest.occurrence_id != item.occurrence_id
        || manifest.video_id != item.video_id
    {
        return Err(Unavailable::ManifestInvalid);
    }
    // One unsafe path invalidates the whole record: a manifest that names a
    // traversal is not trusted for any of its other entries either.
    for artifact in &manifest.artifacts {
        if validate_output_component(&artifact.relative_path).is_err() {
            return Err(Unavailable::ManifestInvalid);
        }
    }
    Ok(manifest)
}

fn verify_artifact(
    canonical_dir: &Path,
    artifact: &ManifestArtifact,
) -> Result<VerifiedFile, Unavailable> {
    let path = canonical_dir.join(&artifact.relative_path);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Unavailable::FileMissing)
        }
        Err(_) => return Err(Unavailable::Unsafe),
    };
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_file() {
        return Err(Unavailable::Unsafe);
    }
    if metadata.len() == 0 || metadata.len() != artifact.size_bytes {
        return Err(Unavailable::FileChanged);
    }
    let canonical = path.canonicalize().map_err(|_| Unavailable::FileMissing)?;
    if canonical.parent() != Some(canonical_dir) {
        return Err(Unavailable::Unsafe);
    }
    Ok(VerifiedFile {
        path: canonical,
        len: metadata.len(),
        name: artifact.relative_path.clone(),
    })
}

/// MIME for playable artifacts: the shared video types plus the audio and
/// container extensions yt-dlp can produce.
pub(super) fn mime_for_media(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("m4a") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        _ => crate::app::media_range::mime_for_path(name),
    }
}

/// WebVTT body for the item, or `None` when it has no usable captions.
///
/// Prefers the normalized transcript (rolling auto-caption repeats removed,
/// text escaped); falls back to the raw `.vtt` when no JSON exists or it fails
/// to parse.
pub(super) fn subtitles_body(files: &ItemFiles) -> Option<Vec<u8>> {
    if let Some(json) = &files.transcript_json {
        if let Some(body) = generated_subtitles(json) {
            return Some(body);
        }
    }
    let vtt = files.vtt.as_ref()?;
    if vtt.len > MAX_SUBTITLE_BYTES as u64 {
        return None;
    }
    let bytes = read_bounded(&vtt.path, vtt.len, MAX_SUBTITLE_BYTES as u64)?;
    std::str::from_utf8(&bytes).ok()?;
    Some(bytes)
}

fn generated_subtitles(json: &VerifiedFile) -> Option<Vec<u8>> {
    let bytes = read_bounded(
        &json.path,
        json.len,
        MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES as u64,
    )?;
    let transcript: NormalizedTranscript = serde_json::from_slice(&bytes).ok()?;
    drop(bytes);
    if transcript.schema_version != NORMALIZED_TRANSCRIPT_SCHEMA_VERSION {
        return None;
    }
    webvtt_from_transcript(&transcript, MAX_SUBTITLE_BYTES)
}

fn read_bounded(path: &Path, expected_len: u64, max: u64) -> Option<Vec<u8>> {
    if expected_len > max {
        return None;
    }
    let mut bytes = Vec::with_capacity(expected_len as usize);
    File::open(path)
        .and_then(|file| file.take(max + 1).read_to_end(&mut bytes))
        .ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

struct DisplayCue {
    start_ms: u64,
    end_ms: u64,
    lines: Vec<String>,
}

/// Render normalized cues as WebVTT.
///
/// Rolling auto-captions repeat the previous cue's last line(s) at the top of
/// the next one. Lines that repeat the previous cue are dropped, and a cue left
/// empty only extends the previous cue's end. All text is escaped (`&`, `<`,
/// `>`), so a transcript can never inject markup or a `-->` timing separator.
/// Output stops at a cue boundary once `max_bytes` would be exceeded. Returns
/// `None` when no cue has text.
pub(super) fn webvtt_from_transcript(
    transcript: &NormalizedTranscript,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let mut cues: Vec<DisplayCue> = Vec::new();
    let mut previous_lines: Vec<String> = Vec::new();
    for cue in &transcript.cues {
        let lines: Vec<String> = cue
            .text
            .split('\n')
            .map(|line| {
                line.chars()
                    .filter(|character| !character.is_control())
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|line| !line.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        let overlap = (0..=previous_lines.len().min(lines.len()))
            .rev()
            .find(|count| previous_lines[previous_lines.len() - count..] == lines[..*count])
            .unwrap_or(0);
        let fresh: Vec<String> = lines[overlap..].to_vec();
        previous_lines = lines;
        if fresh.is_empty() {
            if let Some(last) = cues.last_mut() {
                if cue.start_ms <= last.end_ms.saturating_add(1_000) {
                    last.end_ms = last.end_ms.max(cue.end_ms);
                }
            }
            continue;
        }
        cues.push(DisplayCue {
            start_ms: cue.start_ms,
            end_ms: cue.end_ms.max(cue.start_ms.saturating_add(1)),
            lines: fresh,
        });
    }
    if cues.is_empty() {
        return None;
    }
    let mut out = String::from("WEBVTT\n\n");
    for cue in &cues {
        let mut block = format!(
            "{} --> {}\n",
            format_timestamp(cue.start_ms),
            format_timestamp(cue.end_ms)
        );
        for line in &cue.lines {
            escape_cue_text(line, &mut block);
            block.push('\n');
        }
        block.push('\n');
        if out.len() + block.len() > max_bytes {
            break;
        }
        out.push_str(&block);
    }
    if out.len() <= "WEBVTT\n\n".len() {
        return None;
    }
    Some(out.into_bytes())
}

fn escape_cue_text(line: &str, out: &mut String) {
    for character in line.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

fn format_timestamp(total_ms: u64) -> String {
    let millis = total_ms % 1_000;
    let total_seconds = total_ms / 1_000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        total_seconds / 3_600,
        (total_seconds / 60) % 60,
        total_seconds % 60,
        millis
    )
}

// ---------------------------------------------------------------------------
// Listing and resume progress
// ---------------------------------------------------------------------------

fn read_progress_rows(
    connection: &Connection,
    run_id: &str,
) -> Result<Vec<(String, YouTubePlaybackProgress)>, PlaybackError> {
    let mut statement = connection
        .prepare(
            "SELECT occurrence_id, position_seconds, duration_seconds, completed
             FROM youtube_playback_progress WHERE run_id = ?1",
        )
        .map_err(|_| PlaybackError::Database)?;
    let rows = statement
        .query_map(params![run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                YouTubePlaybackProgress {
                    position_seconds: row.get(1)?,
                    duration_seconds: row.get(2)?,
                    completed: row.get::<_, i64>(3)? != 0,
                },
            ))
        })
        .map_err(|_| PlaybackError::Database)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| PlaybackError::Database)
}

pub(super) fn list_run_videos(
    connection: &Connection,
    run_id: &str,
) -> Result<Vec<YouTubeRunVideo>, PlaybackError> {
    let run = load_run(connection, run_id)?;
    let progress = read_progress_rows(connection, run_id)?;
    let mut videos: Vec<YouTubeRunVideo> = run
        .request
        .work_items
        .iter()
        .filter(|item| is_valid_id(&item.occurrence_id))
        .map(|item| {
            let saved = progress
                .iter()
                .find(|(occurrence_id, _)| *occurrence_id == item.occurrence_id)
                .map(|(_, saved)| saved);
            let source_duration = run
                .request
                .context
                .items
                .iter()
                .find(|context| context.occurrence_id == item.occurrence_id)
                .and_then(|context| context.source_duration_seconds)
                .map(|seconds| seconds as f64);
            let (available, media_url_value, subtitles_url, reason) =
                match locate_item_files(&run.output_root, item) {
                    Ok(files) => (
                        true,
                        Some(media_url(MediaKind::Media, run_id, &item.occurrence_id)),
                        files
                            .has_captions()
                            .then(|| media_url(MediaKind::Subs, run_id, &item.occurrence_id)),
                        None,
                    ),
                    Err(unavailable) => (false, None, None, Some(unavailable.reason().to_string())),
                };
            YouTubeRunVideo {
                occurrence_id: item.occurrence_id.clone(),
                ordinal: item.ordinal,
                title: item.title.clone(),
                video_id: item.video_id.clone(),
                media_url: media_url_value,
                subtitles_url,
                available,
                unavailable_reason: reason,
                position_seconds: saved.map(|saved| saved.position_seconds),
                duration_seconds: saved
                    .map(|saved| saved.duration_seconds)
                    .filter(|duration| *duration > 0.0)
                    .or(source_duration),
                completed: saved.is_some_and(|saved| saved.completed),
            }
        })
        .collect();
    videos.sort_by_key(|video| video.ordinal);
    Ok(videos)
}

/// A position within the last few seconds counts as finished, so resuming
/// restarts instead of landing on the closing frame.
fn is_completed(position: f64, duration: f64) -> bool {
    duration - position <= (duration * 0.05).min(10.0)
}

fn invalid(message: &str) -> YouTubeError {
    YouTubeError::new("INVALID_PLAYBACK_PROGRESS", message)
}

/// Validate a progress save; returns the (position, duration) to store.
fn validate_progress(
    request: &SaveYouTubePlaybackProgressRequest,
) -> Result<(f64, f64), YouTubeError> {
    if !is_valid_id(&request.run_id) || !is_valid_id(&request.occurrence_id) {
        return Err(invalid("The video reference is malformed."));
    }
    let (position, duration) = (request.position_seconds, request.duration_seconds);
    if !position.is_finite() || !duration.is_finite() {
        return Err(invalid(
            "Playback position and duration must be finite numbers.",
        ));
    }
    if position < 0.0 || duration <= 0.0 {
        return Err(invalid(
            "Playback position must be non-negative and duration positive.",
        ));
    }
    if position > MAX_PLAYBACK_SECONDS || duration > MAX_PLAYBACK_SECONDS {
        return Err(invalid("Playback position or duration is out of range."));
    }
    if position > duration + POSITION_SLACK_SECONDS {
        return Err(invalid("Playback position is past the end of the video."));
    }
    Ok((position.min(duration), duration))
}

fn playback_error(error: PlaybackError) -> YouTubeError {
    match error {
        PlaybackError::RunNotFound => YouTubeError::new(
            "RUN_NOT_FOUND",
            "That YouTube download is no longer in history.",
        ),
        PlaybackError::ItemNotFound => {
            YouTubeError::new("ITEM_NOT_FOUND", "That video is not part of this download.")
        }
        PlaybackError::Database => {
            YouTubeError::new("PLAYBACK_UNAVAILABLE", "Playback data could not be read.")
        }
    }
}

fn write_context() -> DatabaseWriteContext {
    DatabaseWriteContext {
        operation: "save_youtube_playback_progress",
        provider: DatabaseProvider::Youtube,
        workflow_id: None,
    }
}

fn save_progress(
    db_path: &Path,
    writer: &DatabaseWriter,
    request: SaveYouTubePlaybackProgressRequest,
) -> Result<YouTubePlaybackProgress, YouTubeError> {
    let (position, duration) = validate_progress(&request)?;
    // The run and the occurrence must exist before anything is written. Runs
    // are immutable once recorded, so this read cannot go stale in a way that
    // matters; the foreign key still backstops the run id on the write.
    let connection =
        crate::cache::open_runtime(db_path).map_err(|_| playback_error(PlaybackError::Database))?;
    load_run(&connection, &request.run_id)
        .and_then(|run| run.item(&request.occurrence_id).map(|_| ()))
        .map_err(playback_error)?;
    drop(connection);

    let completed = is_completed(position, duration);
    let now = chrono::Utc::now().timestamp();
    let run_id = request.run_id;
    let occurrence_id = request.occurrence_id;
    writer
        .execute(write_context(), move |connection| {
            connection.execute(
                "INSERT INTO youtube_playback_progress (
                    run_id, occurrence_id, position_seconds, duration_seconds, completed, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(run_id, occurrence_id) DO UPDATE SET
                    position_seconds = excluded.position_seconds,
                    duration_seconds = excluded.duration_seconds,
                    completed = excluded.completed,
                    updated_at = excluded.updated_at",
                params![run_id, occurrence_id, position, duration, completed, now],
            )?;
            Ok::<(), DatabaseWriteError>(())
        })
        .map_err(|_| {
            YouTubeError::new(
                "PLAYBACK_SAVE_FAILED",
                "Playback progress could not be saved.",
            )
        })?;
    Ok(YouTubePlaybackProgress {
        position_seconds: position,
        duration_seconds: duration,
        completed,
    })
}

fn get_progress(
    db_path: &Path,
    request: &GetYouTubePlaybackProgressRequest,
) -> Result<Option<YouTubePlaybackProgress>, YouTubeError> {
    if !is_valid_id(&request.run_id) || !is_valid_id(&request.occurrence_id) {
        return Err(invalid("The video reference is malformed."));
    }
    let connection =
        crate::cache::open_runtime(db_path).map_err(|_| playback_error(PlaybackError::Database))?;
    connection
        .query_row(
            "SELECT position_seconds, duration_seconds, completed
             FROM youtube_playback_progress WHERE run_id = ?1 AND occurrence_id = ?2",
            params![request.run_id, request.occurrence_id],
            |row| {
                Ok(YouTubePlaybackProgress {
                    position_seconds: row.get(0)?,
                    duration_seconds: row.get(1)?,
                    completed: row.get::<_, i64>(2)? != 0,
                })
            },
        )
        .optional()
        .map_err(|_| playback_error(PlaybackError::Database))
}

// ---------------------------------------------------------------------------
// Tauri commands (thin: validate, then delegate to the functions above)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_youtube_run_videos(
    state: State<'_, YouTubeState>,
    request: ListYouTubeRunVideosRequest,
) -> Result<Vec<YouTubeRunVideo>, YouTubeError> {
    let db_path = state.db_path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let connection = crate::cache::open_runtime(&db_path)
            .map_err(|_| playback_error(PlaybackError::Database))?;
        list_run_videos(&connection, &request.run_id).map_err(playback_error)
    })
    .await
    .map_err(|_| playback_error(PlaybackError::Database))?
}

#[tauri::command]
pub async fn get_youtube_playback_progress(
    state: State<'_, YouTubeState>,
    request: GetYouTubePlaybackProgressRequest,
) -> Result<Option<YouTubePlaybackProgress>, YouTubeError> {
    let db_path = state.db_path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || get_progress(&db_path, &request))
        .await
        .map_err(|_| playback_error(PlaybackError::Database))?
}

#[tauri::command]
pub async fn save_youtube_playback_progress(
    state: State<'_, YouTubeState>,
    writer: State<'_, DatabaseWriter>,
    request: SaveYouTubePlaybackProgressRequest,
) -> Result<YouTubePlaybackProgress, YouTubeError> {
    let db_path = state.db_path().to_path_buf();
    let writer = writer.inner().clone();
    tauri::async_runtime::spawn_blocking(move || save_progress(&db_path, &writer, request))
        .await
        .map_err(|_| playback_error(PlaybackError::Database))?
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::app::database::initialize_database;
    use crate::app::database_diagnostics::DatabaseDiagnostics;
    use crate::providers::youtube::manifest_contract::{
        canonical_manifest_bytes, ManifestStatus, YouTubeArtifactManifest,
    };
    use crate::providers::youtube::models::YouTubeTranscriptSource;
    use crate::providers::youtube::models::{StartYouTubeDownloadRequest, YouTubeDownloadMode};
    use crate::providers::youtube::transcript_normalizer::TranscriptCue;

    pub(crate) const OCC: &str = "occ-1";
    pub(crate) const RUN: &str = "run-1";

    pub(crate) fn work_item(ordinal: u32) -> TransientWorkItem {
        TransientWorkItem {
            occurrence_id: format!("occ-{ordinal}"),
            artifact_fingerprint: format!("af-{ordinal}"),
            video_id: format!("vid{ordinal}"),
            ordinal,
            title: format!("Video {ordinal}"),
            source_url: format!("https://www.youtube.com/watch?v=vid{ordinal}"),
        }
    }

    pub(crate) struct Fixture {
        pub dir: tempfile::TempDir,
        pub root: PathBuf,
        pub item: TransientWorkItem,
        pub item_dir: PathBuf,
    }

    pub(crate) fn manifest_for(
        item: &TransientWorkItem,
        artifacts: Vec<ManifestArtifact>,
    ) -> YouTubeArtifactManifest {
        YouTubeArtifactManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            provider: YOUTUBE_PROVIDER.to_string(),
            source_snapshot_digest: "a".repeat(64),
            artifact_fingerprint: "b".repeat(64),
            occurrence_id: item.occurrence_id.clone(),
            video_id: item.video_id.clone(),
            playlist_id: None,
            playlist_index: None,
            mode: YouTubeDownloadMode::VideoAndTranscript,
            format_policy_version: 1,
            max_height: None,
            selected_transcript: None,
            helper_lock_digest: "c".repeat(64),
            artifacts,
            status: ManifestStatus::Verified,
        }
    }

    pub(crate) fn artifact(kind: ManifestArtifactKind, name: &str, size: u64) -> ManifestArtifact {
        ManifestArtifact {
            kind,
            relative_path: name.to_string(),
            size_bytes: size,
            sha256: "d".repeat(64),
        }
    }

    pub(crate) fn write_manifest(item_dir: &Path, manifest: &YouTubeArtifactManifest) {
        fs::write(
            item_dir.join("manifest.json"),
            canonical_manifest_bytes(manifest).unwrap(),
        )
        .unwrap();
    }

    /// A published item: media.mp4 (8 bytes), captions.en.vtt and a transcript.
    pub(crate) fn published_fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("out");
        fs::create_dir_all(&root).unwrap();
        let item = work_item(1);
        let name = published_item_name(&root, item.ordinal, &item.title, &item.video_id).unwrap();
        let item_dir = root.join(name);
        fs::create_dir_all(&item_dir).unwrap();
        fs::write(item_dir.join("media.mp4"), b"12345678").unwrap();
        fs::write(
            item_dir.join("video.en.vtt"),
            b"WEBVTT\n\n00:00.000 --> 00:01.000\nhi\n",
        )
        .unwrap();
        let transcript = NormalizedTranscript {
            schema_version: NORMALIZED_TRANSCRIPT_SCHEMA_VERSION,
            provider: "youtube".to_string(),
            video_id: item.video_id.clone(),
            language_tag: "en".to_string(),
            source: YouTubeTranscriptSource::Uploader,
            source_track_key: "en".to_string(),
            source_vtt_sha256: "e".repeat(64),
            cues: vec![TranscriptCue {
                start_ms: 0,
                end_ms: 1_000,
                text: "hello".to_string(),
            }],
        };
        let json = serde_json::to_vec(&transcript).unwrap();
        fs::write(item_dir.join("video.en.transcript.json"), &json).unwrap();
        let manifest = manifest_for(
            &item,
            vec![
                artifact(ManifestArtifactKind::Media, "media.mp4", 8),
                artifact(
                    ManifestArtifactKind::Vtt,
                    "video.en.vtt",
                    fs::metadata(item_dir.join("video.en.vtt")).unwrap().len(),
                ),
                artifact(
                    ManifestArtifactKind::TranscriptJson,
                    "video.en.transcript.json",
                    json.len() as u64,
                ),
            ],
        );
        write_manifest(&item_dir, &manifest);
        Fixture {
            dir,
            root,
            item,
            item_dir,
        }
    }

    fn locate(fixture: &Fixture) -> Result<ItemFiles, Unavailable> {
        locate_item_files(fixture.root.to_str().unwrap(), &fixture.item)
    }

    #[test]
    fn resolves_media_and_captions_from_the_manifest() {
        let fixture = published_fixture();
        let files = locate(&fixture).unwrap();
        assert_eq!(files.media.as_ref().unwrap().len, 8);
        assert!(files.vtt.is_some() && files.transcript_json.is_some());
        assert!(files.has_captions());
    }

    #[test]
    fn traversal_and_nested_relative_paths_invalidate_the_manifest() {
        for bad in [
            "../outside.mp4",
            "sub/media.mp4",
            "..\\outside.mp4",
            "C:\\x.mp4",
            "/abs.mp4",
        ] {
            let fixture = published_fixture();
            let manifest = manifest_for(
                &fixture.item,
                vec![artifact(ManifestArtifactKind::Media, bad, 8)],
            );
            write_manifest(&fixture.item_dir, &manifest);
            assert_eq!(
                locate(&fixture).unwrap_err(),
                Unavailable::ManifestInvalid,
                "{bad}"
            );
        }
    }

    #[test]
    fn size_mismatch_is_reported_as_changed() {
        let fixture = published_fixture();
        fs::write(fixture.item_dir.join("media.mp4"), b"123456789").unwrap();
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::FileChanged);
    }

    #[test]
    fn missing_media_file_missing_manifest_and_missing_folder_are_unavailable() {
        let fixture = published_fixture();
        fs::remove_file(fixture.item_dir.join("media.mp4")).unwrap();
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::FileMissing);

        let fixture = published_fixture();
        fs::remove_file(fixture.item_dir.join("manifest.json")).unwrap();
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::ManifestMissing);

        let fixture = published_fixture();
        fs::remove_dir_all(&fixture.item_dir).unwrap();
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::FolderMissing);

        let fixture = published_fixture();
        let missing_root = fixture.dir.path().join("gone");
        assert_eq!(
            locate_item_files(missing_root.to_str().unwrap(), &fixture.item).unwrap_err(),
            Unavailable::RootMissing
        );
        assert_eq!(
            locate_item_files("relative/out", &fixture.item).unwrap_err(),
            Unavailable::RootMissing
        );
    }

    #[test]
    fn manifest_for_a_different_occurrence_or_video_is_rejected() {
        let fixture = published_fixture();
        let mut other = work_item(1);
        other.occurrence_id = "occ-other".to_string();
        let manifest = manifest_for(
            &other,
            vec![artifact(ManifestArtifactKind::Media, "media.mp4", 8)],
        );
        write_manifest(&fixture.item_dir, &manifest);
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::ManifestInvalid);
    }

    #[test]
    fn oversized_or_garbage_manifest_is_rejected() {
        let fixture = published_fixture();
        fs::write(fixture.item_dir.join("manifest.json"), b"not json").unwrap();
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::ManifestInvalid);
        let big = vec![b' '; (MAX_MANIFEST_BYTES + 1) as usize];
        fs::write(fixture.item_dir.join("manifest.json"), big).unwrap();
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::ManifestInvalid);
    }

    #[test]
    fn transcript_only_item_has_no_video() {
        let fixture = published_fixture();
        let manifest = manifest_for(
            &fixture.item,
            vec![artifact(
                ManifestArtifactKind::Vtt,
                "video.en.vtt",
                fs::metadata(fixture.item_dir.join("video.en.vtt"))
                    .unwrap()
                    .len(),
            )],
        );
        write_manifest(&fixture.item_dir, &manifest);
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::NoVideo);
    }

    #[test]
    fn a_bad_caption_file_only_drops_captions() {
        let fixture = published_fixture();
        fs::write(fixture.item_dir.join("video.en.vtt"), b"changed").unwrap();
        fs::remove_file(fixture.item_dir.join("video.en.transcript.json")).unwrap();
        let files = locate(&fixture).unwrap();
        assert!(files.media.is_some());
        assert!(!files.has_captions());
    }

    /// Make `link` point at `target` (a directory junction on Windows, a
    /// symlink elsewhere). Returns false when the platform refuses.
    fn link_dir(link: &Path, target: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    #[test]
    fn item_folder_that_is_a_junction_or_symlink_is_rejected() {
        let fixture = published_fixture();
        let outside = fixture.dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("media.mp4"), b"12345678").unwrap();
        write_manifest(
            &outside,
            &manifest_for(
                &fixture.item,
                vec![artifact(ManifestArtifactKind::Media, "media.mp4", 8)],
            ),
        );
        fs::remove_dir_all(&fixture.item_dir).unwrap();
        if !link_dir(&fixture.item_dir, &outside) {
            eprintln!("skipping: this environment cannot create directory links");
            return;
        }
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::Unsafe);
    }

    #[test]
    fn output_root_that_is_a_junction_or_symlink_is_rejected() {
        let fixture = published_fixture();
        let link = fixture.dir.path().join("linked-out");
        if !link_dir(&link, &fixture.root) {
            eprintln!("skipping: this environment cannot create directory links");
            return;
        }
        assert_eq!(
            locate_item_files(link.to_str().unwrap(), &fixture.item).unwrap_err(),
            Unavailable::RootMissing
        );
    }

    #[test]
    fn media_file_that_is_a_symlink_is_rejected() {
        let fixture = published_fixture();
        let outside = fixture.dir.path().join("secret.mp4");
        fs::write(&outside, b"12345678").unwrap();
        fs::remove_file(fixture.item_dir.join("media.mp4")).unwrap();
        #[cfg(windows)]
        let linked =
            std::os::windows::fs::symlink_file(&outside, fixture.item_dir.join("media.mp4"))
                .is_ok();
        #[cfg(unix)]
        let linked =
            std::os::unix::fs::symlink(&outside, fixture.item_dir.join("media.mp4")).is_ok();
        if !linked {
            eprintln!("skipping: this environment cannot create file symlinks");
            return;
        }
        assert_eq!(locate(&fixture).unwrap_err(), Unavailable::Unsafe);
    }

    #[test]
    fn id_validation_rejects_paths_and_oversized_ids() {
        assert!(is_valid_id("abc-DEF_123"));
        for bad in ["", "..", "a/b", "a\\b", "a.b", "a b", &"x".repeat(129)] {
            assert!(!is_valid_id(bad), "{bad}");
        }
    }

    // --- WebVTT generation ---------------------------------------------

    fn transcript(cues: &[(u64, u64, &str)]) -> NormalizedTranscript {
        NormalizedTranscript {
            schema_version: NORMALIZED_TRANSCRIPT_SCHEMA_VERSION,
            provider: "youtube".to_string(),
            video_id: "vid".to_string(),
            language_tag: "en".to_string(),
            source: YouTubeTranscriptSource::Automatic,
            source_track_key: "en".to_string(),
            source_vtt_sha256: "e".repeat(64),
            cues: cues
                .iter()
                .map(|(start_ms, end_ms, text)| TranscriptCue {
                    start_ms: *start_ms,
                    end_ms: *end_ms,
                    text: text.to_string(),
                })
                .collect(),
        }
    }

    fn vtt_text(transcript: &NormalizedTranscript) -> String {
        String::from_utf8(webvtt_from_transcript(transcript, MAX_SUBTITLE_BYTES).unwrap()).unwrap()
    }

    #[test]
    fn generated_vtt_escapes_markup_and_timing_separators() {
        let body = vtt_text(&transcript(&[(
            1_500,
            3_661_250,
            "a --> b <c>bold</c> & done",
        )]));
        assert!(body.starts_with("WEBVTT\n\n00:00:01.500 --> 01:01:01.250\n"));
        assert!(body.contains("a --&gt; b &lt;c&gt;bold&lt;/c&gt; &amp; done\n"));
        // The only timing separator is the one the generator wrote.
        assert_eq!(body.matches("-->").count(), 1);
        assert!(!body.contains("<c>"));
    }

    #[test]
    fn rolling_auto_captions_do_not_repeat_lines() {
        let body = vtt_text(&transcript(&[
            (0, 2_000, "first line\nsecond line"),
            (2_000, 2_010, "second line"),
            (2_010, 4_000, "second line\nthird line"),
            (4_000, 6_000, "third line\nfourth line"),
        ]));
        assert_eq!(body.matches("second line").count(), 1);
        assert_eq!(body.matches("third line").count(), 1);
        assert_eq!(body.matches("fourth line").count(), 1);
        assert_eq!(body.matches("-->").count(), 3);
        // The repeated-only cue extended the cue it repeated.
        assert!(body.contains("00:00:00.000 --> 00:00:02.010"));
    }

    #[test]
    fn blank_control_and_empty_cues_are_dropped() {
        assert!(webvtt_from_transcript(&transcript(&[(0, 1_000, " \n\u{7} ")]), 1024).is_none());
        let body = vtt_text(&transcript(&[(0, 1_000, "a\u{0}b\n\n  c  ")]));
        assert!(body.contains("ab\nc\n"));
    }

    #[test]
    fn generated_vtt_is_cut_at_a_cue_boundary_within_the_cap() {
        let text = "x".repeat(40);
        let cues: Vec<(u64, u64, &str)> = (0..200)
            .map(|index| (index * 1_000, index * 1_000 + 900, text.as_str()))
            .collect();
        let transcript = transcript(&cues);
        let body = webvtt_from_transcript(&transcript, 1_000).unwrap();
        assert!(body.len() <= 1_000);
        let text = String::from_utf8(body).unwrap();
        assert!(text.ends_with("\n\n"));
        assert!(text.matches("-->").count() < 200);
    }

    #[test]
    fn subtitles_prefer_the_transcript_and_fall_back_to_raw_vtt() {
        let fixture = published_fixture();
        let files = locate(&fixture).unwrap();
        let generated = String::from_utf8(subtitles_body(&files).unwrap()).unwrap();
        assert!(generated.contains("hello"));

        // Corrupt the JSON (same length keeps the manifest size valid).
        let json_path = fixture.item_dir.join("video.en.transcript.json");
        let len = fs::metadata(&json_path).unwrap().len() as usize;
        fs::write(&json_path, vec![b'x'; len]).unwrap();
        let files = locate(&fixture).unwrap();
        let raw = String::from_utf8(subtitles_body(&files).unwrap()).unwrap();
        assert!(raw.contains("00:00.000 --> 00:01.000\nhi"));
    }

    #[test]
    fn mime_covers_audio_and_falls_back_to_the_shared_table() {
        assert_eq!(mime_for_media("a.m4a"), "audio/mp4");
        assert_eq!(mime_for_media("a.mov"), "video/quicktime");
        assert_eq!(mime_for_media("a.MP4"), "video/mp4");
        assert_eq!(mime_for_media("a.mkv"), "video/x-matroska");
        assert_eq!(mime_for_media("a.xyz"), "application/octet-stream");
    }

    // --- Listing, progress and database ------------------------------------

    pub(crate) fn durable_request(
        output_dir: &Path,
        items: Vec<TransientWorkItem>,
    ) -> YoutubeDurableRequest {
        YoutubeDurableRequest {
            schema_version: 1,
            start: StartYouTubeDownloadRequest {
                client_submission_id: "sub".to_string(),
                scan_plan_id: "plan".to_string(),
                selected_occurrence_ids: items
                    .iter()
                    .map(|item| item.occurrence_id.clone())
                    .collect(),
                output_dir: output_dir.to_string_lossy().into_owned(),
                mode: YouTubeDownloadMode::VideoAndTranscript,
                max_height: None,
                preferred_language: None,
                fallback_languages: Vec::new(),
                allow_automatic_captions: false,
                continue_without_transcript: false,
            },
            work_items: items,
            context: Default::default(),
            plan_fingerprint: "fp".to_string(),
            client_submission_id: "sub".to_string(),
        }
    }

    pub(crate) fn insert_run(
        connection: &Connection,
        run_id: &str,
        workflow_type: &str,
        output_root: &Path,
        items: Vec<TransientWorkItem>,
    ) {
        let request = durable_request(output_root, items);
        connection
            .execute(
                "INSERT INTO workflow_runs (
                    id, workflow_type, provider, state, request_json, output_root,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'youtube', 'succeeded', ?3, ?4, 1, 1)",
                params![
                    run_id,
                    workflow_type,
                    serde_json::to_string(&request).unwrap(),
                    output_root.to_string_lossy().into_owned()
                ],
            )
            .unwrap();
    }

    fn database(fixture: &Fixture) -> (PathBuf, Connection) {
        let db_path = fixture.dir.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        insert_run(
            &connection,
            RUN,
            YOUTUBE_WORKFLOW_TYPE,
            &fixture.root,
            vec![fixture.item.clone(), work_item(2)],
        );
        (db_path, connection)
    }

    #[test]
    fn listing_marks_missing_items_unavailable_instead_of_failing() {
        let fixture = published_fixture();
        let (_db_path, connection) = database(&fixture);
        let videos = list_run_videos(&connection, RUN).unwrap();
        assert_eq!(videos.len(), 2);
        let first = &videos[0];
        assert!(first.available);
        assert!(first
            .media_url
            .as_deref()
            .unwrap()
            .contains(&format!("/media/{RUN}/{OCC}")));
        assert!(first
            .subtitles_url
            .as_deref()
            .unwrap()
            .contains(&format!("/subs/{RUN}/{OCC}")));
        assert!(first.unavailable_reason.is_none());
        let second = &videos[1];
        assert!(!second.available);
        assert!(second.media_url.is_none() && second.subtitles_url.is_none());
        assert!(second.unavailable_reason.is_some());
        // URLs carry only ids.
        let url = first.media_url.as_deref().unwrap();
        assert!(!url.contains(fixture.root.to_str().unwrap()));
    }

    #[test]
    fn listing_rejects_unknown_runs_and_other_workflows() {
        let fixture = published_fixture();
        let (_db_path, connection) = database(&fixture);
        insert_run(
            &connection,
            "run-li",
            "linkedin_download",
            &fixture.root,
            vec![work_item(1)],
        );
        assert_eq!(
            list_run_videos(&connection, "nope").unwrap_err(),
            PlaybackError::RunNotFound
        );
        assert_eq!(
            list_run_videos(&connection, "run-li").unwrap_err(),
            PlaybackError::RunNotFound
        );
        assert_eq!(
            list_run_videos(&connection, "../x").unwrap_err(),
            PlaybackError::RunNotFound
        );
    }

    fn save_request(position: f64, duration: f64) -> SaveYouTubePlaybackProgressRequest {
        SaveYouTubePlaybackProgressRequest {
            run_id: RUN.to_string(),
            occurrence_id: OCC.to_string(),
            position_seconds: position,
            duration_seconds: duration,
        }
    }

    #[test]
    fn progress_validation_rejects_bad_numbers_and_ids() {
        for (position, duration) in [
            (f64::NAN, 10.0),
            (1.0, f64::INFINITY),
            (-1.0, 10.0),
            (1.0, 0.0),
            (1.0, -3.0),
            (MAX_PLAYBACK_SECONDS + 1.0, MAX_PLAYBACK_SECONDS + 1.0),
            (100.0, 10.0),
        ] {
            assert!(
                validate_progress(&save_request(position, duration)).is_err(),
                "{position} / {duration}"
            );
        }
        let mut bad_id = save_request(1.0, 10.0);
        bad_id.occurrence_id = "../etc".to_string();
        assert!(validate_progress(&bad_id).is_err());
        let mut bad_run = save_request(1.0, 10.0);
        bad_run.run_id = String::new();
        assert!(validate_progress(&bad_run).is_err());
        // A small overshoot is clamped to the duration.
        assert_eq!(
            validate_progress(&save_request(12.0, 10.0)).unwrap(),
            (10.0, 10.0)
        );
    }

    #[test]
    fn completion_threshold_scales_with_duration() {
        assert!(!is_completed(50.0, 100.0));
        assert!(is_completed(96.0, 100.0));
        assert!(!is_completed(4.0, 10.0));
        assert!(is_completed(9.8, 10.0));
        assert!(!is_completed(3_000.0, 3_600.0));
        assert!(is_completed(3_595.0, 3_600.0));
    }

    #[test]
    fn saving_progress_goes_through_the_writer_and_round_trips() {
        let fixture = published_fixture();
        let (db_path, connection) = database(&fixture);
        drop(connection);
        let writer =
            DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default()).unwrap();

        let saved = save_progress(&db_path, &writer, save_request(30.5, 120.0)).unwrap();
        assert_eq!(
            saved,
            YouTubePlaybackProgress {
                position_seconds: 30.5,
                duration_seconds: 120.0,
                completed: false
            }
        );
        let again = save_progress(&db_path, &writer, save_request(119.0, 120.0)).unwrap();
        assert!(again.completed);

        let got = get_progress(
            &db_path,
            &GetYouTubePlaybackProgressRequest {
                run_id: RUN.to_string(),
                occurrence_id: OCC.to_string(),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.position_seconds, 119.0);
        assert!(got.completed);

        let connection = crate::cache::open_runtime(&db_path).unwrap();
        let videos = list_run_videos(&connection, RUN).unwrap();
        assert_eq!(videos[0].position_seconds, Some(119.0));
        assert_eq!(videos[0].duration_seconds, Some(120.0));
        assert!(videos[0].completed);
        let rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM youtube_playback_progress",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "the same video upserts one row");
        writer.shutdown().unwrap();
    }

    #[test]
    fn saving_progress_requires_an_existing_run_and_occurrence() {
        let fixture = published_fixture();
        let (db_path, connection) = database(&fixture);
        drop(connection);
        let writer =
            DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default()).unwrap();

        let mut unknown_run = save_request(1.0, 10.0);
        unknown_run.run_id = "missing-run".to_string();
        assert_eq!(
            save_progress(&db_path, &writer, unknown_run)
                .unwrap_err()
                .code,
            "RUN_NOT_FOUND"
        );
        let mut unknown_item = save_request(1.0, 10.0);
        unknown_item.occurrence_id = "occ-missing".to_string();
        assert_eq!(
            save_progress(&db_path, &writer, unknown_item)
                .unwrap_err()
                .code,
            "ITEM_NOT_FOUND"
        );
        assert_eq!(
            save_progress(&db_path, &writer, save_request(-1.0, 10.0))
                .unwrap_err()
                .code,
            "INVALID_PLAYBACK_PROGRESS"
        );

        let connection = crate::cache::open_runtime(&db_path).unwrap();
        let rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM youtube_playback_progress",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "rejected saves write nothing");
        writer.shutdown().unwrap();
    }
}
