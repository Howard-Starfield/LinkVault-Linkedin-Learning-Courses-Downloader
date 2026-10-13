//! In-app playback of finished YouTube downloads.
//!
//! Everything here re-derives what to play from durable state on every call:
//! `workflow_runs.output_root` and the run's recorded work items give the item
//! folder name (the same function the executor publishes under), the folder's
//! `manifest.json` names the artifacts, and each artifact is checked on disk
//! before a byte is served. URLs handed to the webview carry only opaque ids,
//! never paths, and nothing is discovered by scanning folders.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriteError, DatabaseWriter};
use crate::app::managed_process::TransientWorkItem;
use crate::app::safe_output_filesystem::{
    validate_output_component, validate_output_root_for_read, verify_open_file,
    ExistingOutputDirectoryLease, SafeOutputError,
};
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
    YouTubeTranscriptSource,
};
use crate::providers::youtube::transcript_normalizer::{
    NormalizedTranscript, MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES,
    NORMALIZED_TRANSCRIPT_SCHEMA_VERSION,
};

/// Longest subtitle document served (generated or raw). Longer transcripts are
/// cut at a cue boundary; a raw `.vtt` over the cap is not offered.
pub(super) const MAX_SUBTITLE_BYTES: usize = 16 * 1024 * 1024;
/// Longest position or duration accepted from the webview (seven days).
pub(super) const MAX_PLAYBACK_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;
/// A saved position may not exceed the duration by more than this.
const POSITION_SLACK_SECONDS: f64 = 5.0;
const MAX_ID_LEN: usize = 128;
pub(super) const YOUTUBE_WORKFLOW_TYPE: &str = "youtube_download";

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
/// whose size matches its manifest entry. It carries the open, verified
/// handle: bytes are only ever read from `file`, never from a path again.
#[derive(Debug)]
pub(super) struct VerifiedFile {
    pub file: File,
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
/// The output root goes through the same admission checks as a download (no
/// reparse point on any ancestor, identity leases); the item folder is leased
/// without following links; every artifact is opened once, no-follow, through
/// that lease and verified on the handle (regular file, exact manifest size,
/// real location inside the canonical root). The content hash is deliberately
/// not recomputed here (it would run once per Range request); the executor
/// verified it when the folder was published.
pub(super) fn locate_item_files(
    output_root: &str,
    item: &TransientWorkItem,
) -> Result<ItemFiles, Unavailable> {
    let root = validate_output_root_for_read(Path::new(output_root))
        .map_err(|_| Unavailable::RootMissing)?;
    let item_name = published_item_name(root.path(), item.ordinal, &item.title, &item.video_id)
        .map_err(|_| Unavailable::FolderMissing)?;
    let lease = match root.existing_item_lease(&item_name) {
        Ok(Some(lease)) => lease,
        Ok(None) => return Err(Unavailable::FolderMissing),
        Err(_) => return Err(Unavailable::Unsafe),
    };

    let manifest = read_manifest(&lease, item)?;

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
        match verify_artifact(&lease, root.canonical_path(), artifact) {
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

fn is_not_found(error: &SafeOutputError) -> bool {
    matches!(error, SafeOutputError::Io(io) if io.kind() == std::io::ErrorKind::NotFound)
}

fn read_manifest(
    lease: &ExistingOutputDirectoryLease,
    item: &TransientWorkItem,
) -> Result<YouTubeArtifactManifest, Unavailable> {
    let mut file = match lease.open_leaf("manifest.json") {
        Ok(file) => file,
        Err(error) if is_not_found(&error) => return Err(Unavailable::ManifestMissing),
        Err(_) => return Err(Unavailable::Unsafe),
    };
    let len = file
        .metadata()
        .map_err(|_| Unavailable::ManifestInvalid)?
        .len();
    if len == 0 || len > MAX_MANIFEST_BYTES {
        return Err(Unavailable::ManifestInvalid);
    }
    let mut bytes = Vec::with_capacity(len as usize);
    (&mut file)
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
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
    lease: &ExistingOutputDirectoryLease,
    canonical_root: &Path,
    artifact: &ManifestArtifact,
) -> Result<VerifiedFile, Unavailable> {
    let file = match lease.open_leaf(&artifact.relative_path) {
        Ok(file) => file,
        Err(error) if is_not_found(&error) => return Err(Unavailable::FileMissing),
        Err(_) => return Err(Unavailable::Unsafe),
    };
    let len = file.metadata().map_err(|_| Unavailable::Unsafe)?.len();
    if len == 0 || len != artifact.size_bytes {
        return Err(Unavailable::FileChanged);
    }
    verify_open_file(&file, artifact.size_bytes, canonical_root)
        .map_err(|_| Unavailable::Unsafe)?;
    Ok(VerifiedFile {
        file,
        len,
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
    let bytes = read_bounded(&vtt.file, vtt.len, MAX_SUBTITLE_BYTES as u64)?;
    std::str::from_utf8(&bytes).ok()?;
    Some(bytes)
}

fn generated_subtitles(json: &VerifiedFile) -> Option<Vec<u8>> {
    let bytes = read_bounded(
        &json.file,
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

fn read_bounded(mut file: &File, expected_len: u64, max: u64) -> Option<Vec<u8>> {
    if expected_len > max {
        return None;
    }
    // The handle is shared by reference; always read from the start.
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::with_capacity(expected_len as usize);
    file.take(max + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

/// Largest gap between two cues still treated as one rolling caption.
const ROLLING_CONTIGUITY_MS: u64 = 100;

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
    let rolling = matches!(transcript.source, YouTubeTranscriptSource::Automatic);
    let mut cues: Vec<DisplayCue> = Vec::new();
    let mut previous_lines: Vec<String> = Vec::new();
    let mut previous_end_ms: u64 = 0;
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
        // Rolling auto-captions repeat the previous cue only when the two cues
        // are back to back. Uploader captions, and a line that merely recurs
        // later in the video, are never removed.
        let contiguous = cue.start_ms <= previous_end_ms.saturating_add(ROLLING_CONTIGUITY_MS);
        let max_overlap = if rolling && contiguous {
            previous_lines.len().min(lines.len())
        } else {
            0
        };
        let overlap = (0..=max_overlap)
            .rev()
            .find(|count| previous_lines[previous_lines.len() - count..] == lines[..*count])
            .unwrap_or(0);
        let fresh: Vec<String> = lines[overlap..].to_vec();
        previous_lines = lines;
        previous_end_ms = cue.end_ms;
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
pub(super) fn is_completed(position: f64, duration: f64) -> bool {
    duration - position <= (duration * 0.05).min(10.0)
}

fn invalid(message: &str) -> YouTubeError {
    YouTubeError::new("INVALID_PLAYBACK_PROGRESS", message)
}

/// Validate a progress save; returns the (position, duration) to store.
pub(super) fn validate_progress(
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

pub(super) fn save_progress(
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

pub(super) fn get_progress(
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
