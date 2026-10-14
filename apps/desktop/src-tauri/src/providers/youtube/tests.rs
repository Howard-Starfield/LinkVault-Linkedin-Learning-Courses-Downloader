//! Tests for in-app YouTube playback: item resolution, caption generation,
//! resume progress, and the `youtube-media` protocol.
//!
//! Kept in one `tests.rs` so the persistence verifier treats it as test code.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection};
use tauri::http::{
    header::{CONTENT_RANGE, CONTENT_TYPE, RANGE},
    Request, StatusCode,
};

use super::executor::{published_item_name, MAX_MANIFEST_BYTES};
use super::live::YoutubeDurableRequest;
use super::manifest_contract::{
    canonical_manifest_bytes, ManifestArtifact, ManifestArtifactKind, ManifestStatus,
    YouTubeArtifactManifest, MANIFEST_SCHEMA_VERSION, YOUTUBE_PROVIDER,
};
use super::media_protocol::handle_request;
use super::models::{
    GetYouTubePlaybackProgressRequest, SaveYouTubePlaybackProgressRequest,
    StartYouTubeDownloadRequest, YouTubeDownloadMode, YouTubePlaybackProgress,
    YouTubeTranscriptSource,
};
use super::playback::{
    get_progress, is_completed, is_valid_id, list_run_videos, locate_item_files, mime_for_media,
    save_progress, subtitles_body, validate_progress, webvtt_from_transcript, ItemFiles,
    PlaybackError, Unavailable, MAX_PLAYBACK_SECONDS, MAX_SUBTITLE_BYTES, YOUTUBE_WORKFLOW_TYPE,
};
use super::transcript_normalizer::{
    NormalizedTranscript, TranscriptCue, NORMALIZED_TRANSCRIPT_SCHEMA_VERSION,
};
use crate::app::database::initialize_database;
use crate::app::database_diagnostics::DatabaseDiagnostics;
use crate::app::database_writer::DatabaseWriter;
use crate::app::managed_process::TransientWorkItem;

pub(super) const OCC: &str = "occ-1";
pub(super) const RUN: &str = "run-1";

pub(super) fn work_item(ordinal: u32) -> TransientWorkItem {
    TransientWorkItem {
        occurrence_id: format!("occ-{ordinal}"),
        artifact_fingerprint: format!("af-{ordinal}"),
        video_id: format!("vid{ordinal}"),
        ordinal,
        title: format!("Video {ordinal}"),
        source_url: format!("https://www.youtube.com/watch?v=vid{ordinal}"),
    }
}

pub(super) struct Fixture {
    pub dir: tempfile::TempDir,
    pub root: PathBuf,
    pub item: TransientWorkItem,
    pub item_dir: PathBuf,
}

pub(super) fn manifest_for(
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

pub(super) fn artifact(kind: ManifestArtifactKind, name: &str, size: u64) -> ManifestArtifact {
    ManifestArtifact {
        kind,
        relative_path: name.to_string(),
        size_bytes: size,
        sha256: "d".repeat(64),
    }
}

pub(super) fn write_manifest(item_dir: &Path, manifest: &YouTubeArtifactManifest) {
    fs::write(
        item_dir.join("manifest.json"),
        canonical_manifest_bytes(manifest).unwrap(),
    )
    .unwrap();
}

/// A published item: media.mp4 (8 bytes), captions.en.vtt and a transcript.
pub(super) fn published_fixture() -> Fixture {
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
    if !crate::app::test_support::link_dir(&fixture.item_dir, &outside) {
        eprintln!("skipping: this environment cannot create directory links");
        return;
    }
    assert_eq!(locate(&fixture).unwrap_err(), Unavailable::Unsafe);
}

#[test]
fn output_root_that_is_a_junction_or_symlink_is_rejected() {
    let fixture = published_fixture();
    let link = fixture.dir.path().join("linked-out");
    if !crate::app::test_support::link_dir(&link, &fixture.root) {
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
        std::os::windows::fs::symlink_file(&outside, fixture.item_dir.join("media.mp4")).is_ok();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, fixture.item_dir.join("media.mp4")).is_ok();
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
    drop(files);

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

pub(super) fn durable_request(
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

pub(super) fn insert_run(
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
    let writer = DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default()).unwrap();

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
    let writer = DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default()).unwrap();

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

#[test]
fn a_junction_on_an_ancestor_of_the_output_root_is_rejected() {
    let fixture = published_fixture();
    // The stored root is link\out, where link is a junction to the real parent.
    let link = fixture.dir.path().join("link");
    if !crate::app::test_support::link_dir(&link, fixture.dir.path()) {
        eprintln!("skipping: this environment cannot create directory links");
        return;
    }
    let through_ancestor = link.join("out");
    assert!(through_ancestor
        .join(fixture.item_dir.file_name().unwrap())
        .is_dir());
    assert_eq!(
        locate_item_files(through_ancestor.to_str().unwrap(), &fixture.item).unwrap_err(),
        Unavailable::RootMissing
    );
}

#[test]
fn a_recurring_caption_later_in_the_video_is_kept() {
    let body = vtt_text(&transcript(&[
        (0, 1_000, "Hello"),
        (5_000, 6_000, "Something else"),
        (10_000, 11_000, "Hello"),
    ]));
    assert_eq!(body.matches("Hello").count(), 2);
}

#[test]
fn contiguous_identical_cues_still_merge_for_automatic_captions_only() {
    let cues = [(0, 1_000, "Hello"), (1_050, 2_000, "Hello")];
    let automatic = vtt_text(&transcript(&cues));
    assert_eq!(automatic.matches("Hello").count(), 1);
    assert!(automatic.contains("00:00:00.000 --> 00:00:02.000"));

    let mut uploader = transcript(&cues);
    uploader.source = YouTubeTranscriptSource::Uploader;
    let kept = vtt_text(&uploader);
    assert_eq!(kept.matches("Hello").count(), 2);
}

mod protocol {
    use super::*;

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
