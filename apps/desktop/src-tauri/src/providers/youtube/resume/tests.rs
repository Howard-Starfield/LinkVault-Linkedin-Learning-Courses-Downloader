use super::*;
use crate::app::database::initialize_database;
use crate::app::database_diagnostics::DatabaseDiagnostics;
use crate::app::database_writer::DatabaseWriter;
use crate::providers::youtube::download_progress::parse_formats_line;
use crate::providers::youtube::models::{StartYouTubeDownloadRequest, YouTubeDownloadMode};
use crate::workflow::domain::types::WorkflowType;
use tempfile::tempdir;

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

// ---- format-change cleanup -------------------------------------------------

#[test]
fn format_ids_are_read_from_stream_files_only() {
    assert_eq!(format_id_of_leaf("media.f137.mp4.part"), Some("137"));
    assert_eq!(format_id_of_leaf("media.f137.mp4.ytdl"), Some("137"));
    assert_eq!(format_id_of_leaf("media.f140.m4a"), Some("140"));
    assert_eq!(format_id_of_leaf("media.f137.mp4.part-Frag12"), Some("137"));
    assert_eq!(format_id_of_leaf("media.fhls-720p.mp4.part"), Some("hls-720p"));
    // A French subtitle track starts with "f" too; it is not a format.
    assert_eq!(format_id_of_leaf("media.fr.vtt"), None);
    assert_eq!(format_id_of_leaf("media.mp4"), None);
    assert_eq!(format_id_of_leaf("media.en-US.vtt"), None);
}

#[test]
fn only_files_of_formats_that_were_not_chosen_are_stale() {
    let present = names(&[
        "media.f299.mp4.part",
        "media.f299.mp4.ytdl",
        "media.f137.mp4.part",
        "media.f140.m4a.part",
        "media.fr.vtt",
        "media.en.vtt",
    ]);
    assert_eq!(
        stale_part_leaves(&present, &names(&["137", "140"])),
        names(&["media.f299.mp4.part", "media.f299.mp4.ytdl"])
    );
    assert!(stale_part_leaves(&present, &[]).is_empty());
    assert!(stale_part_leaves(&present, &names(&["299", "137", "140"])).is_empty());
}

#[test]
fn the_formats_record_lists_ids_in_order() {
    assert_eq!(
        parse_formats_line("LVFORMATS 137, 140"),
        Some(names(&["137", "140"]))
    );
    assert_eq!(parse_formats_line("LVFORMATS"), None);
    assert_eq!(parse_formats_line("LVFORMATSX 1"), None);
    assert_eq!(parse_formats_line("noise"), None);
}

// ---- failure classification -----------------------------------------------

#[test]
fn helper_failures_are_classified_from_their_output() {
    assert_eq!(
        classify_helper_failure("OSError: [Errno 28] No space left on device"),
        FailureKind::DiskFull
    );
    assert_eq!(
        classify_helper_failure("ERROR: [Errno 112] There is not enough space on the disk"),
        FailureKind::DiskFull
    );
    assert_eq!(
        classify_helper_failure("ERROR: [youtube] x: Video unavailable"),
        FailureKind::VideoUnavailable
    );
    assert_eq!(
        classify_helper_failure("ERROR: [youtube] x: Private video. Sign in"),
        FailureKind::VideoUnavailable
    );
    assert_eq!(
        classify_helper_failure("HTTP Error 403: Forbidden"),
        FailureKind::Other
    );
}

#[test]
fn only_transport_failures_keep_their_partial_files() {
    for code in [
        "HELPER_FAILED",
        "HELPER_TIMEOUT",
        "DISK_FULL",
        "VIDEO_UNAVAILABLE",
        "OUTPUT_VERIFY_FAILED",
        "MEDIA_VERIFY_FAILED",
    ] {
        assert!(keep_staging_after(code), "{code}");
    }
    for code in [
        "NO_ARTIFACT",
        "TRANSCRIPT_INVALID",
        "CANCELLED",
    ] {
        assert!(!keep_staging_after(code), "{code}");
    }
}

#[test]
fn pause_reasons_name_the_cause() {
    assert!(pause_reason("DISK_FULL", "x").contains("disk"));
    assert!(pause_reason("VIDEO_UNAVAILABLE", "x").contains("no longer available"));
    assert!(pause_reason("HELPER_FAILED", "HTTP Error 403").contains("HTTP Error 403"));
    assert!(pause_reason("HELPER_FAILED", "  ").contains("Continue"));
}

// ---- runtime fixtures -------------------------------------------------------

fn work_item(ordinal: u32) -> TransientWorkItem {
    TransientWorkItem {
        occurrence_id: format!("occ-{ordinal}"),
        artifact_fingerprint: format!("af-{ordinal}"),
        video_id: format!("vid-{ordinal}"),
        ordinal,
        title: format!("Video {ordinal}"),
        source_url: format!("https://www.youtube.com/watch?v=vid-{ordinal}"),
    }
}

fn request_json(items: &[TransientWorkItem]) -> String {
    let request = YoutubeDurableRequest {
        schema_version: 1,
        start: StartYouTubeDownloadRequest {
            client_submission_id: "sub-resume".to_string(),
            scan_plan_id: "plan".to_string(),
            selected_occurrence_ids: items.iter().map(|i| i.occurrence_id.clone()).collect(),
            output_dir: ".".to_string(),
            mode: YouTubeDownloadMode::VideoOnly,
            max_height: None,
            preferred_language: None,
            fallback_languages: Vec::new(),
            allow_automatic_captions: false,
            continue_without_transcript: false,
        },
        work_items: items.to_vec(),
        context: Default::default(),
        plan_fingerprint: "plan-fp".to_string(),
        client_submission_id: "sub-resume".to_string(),
    };
    serde_json::to_string(&request).unwrap()
}

fn runtime() -> (tempfile::TempDir, WorkflowRuntime) {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("linkvault.sqlite3");
    let (connection, _) = initialize_database(&db_path).unwrap();
    drop(connection);
    let writer = DatabaseWriter::start(db_path, DatabaseDiagnostics::default()).unwrap();
    (directory, WorkflowRuntime::new(writer))
}

/// A run that was running when the app died and has been reconciled.
fn interrupted_run(runtime: &WorkflowRuntime, id: &str, root: &str) -> RunRecord {
    let items = vec![work_item(1)];
    runtime
        .submit_youtube_download(
            id.to_string(),
            "vid-1".to_string(),
            request_json(&items),
            root.to_string(),
            100,
        )
        .unwrap();
    runtime.reconcile_youtube_after_restart(110).unwrap();
    runtime.get_run(id.to_string()).unwrap().unwrap()
}

fn generation_of(runtime: &WorkflowRuntime, id: &str) -> i64 {
    runtime.get_resume(id.to_string()).unwrap().unwrap().generation
}

fn go(_: &RunRecord, _: &RunResumeRecord) -> Preflight {
    Preflight::Go
}

// ---- automatic resume -------------------------------------------------------

#[test]
fn the_automatic_resume_runs_once_and_spends_the_budget_before_the_attempt() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    let run = interrupted_run(&runtime, "yt-auto", ".");
    assert_eq!(run.state, RunState::Paused);

    let step = auto_resume_next(&runtime, &live, &go).unwrap();
    assert_eq!(step, AutoResumeStep::Started);
    let record = runtime.get_resume("yt-auto".to_string()).unwrap().unwrap();
    assert!(record.auto_resume_used);
    assert_eq!(record.state, ResumeState::Resuming);
    assert!(live.has_active_run("yt-auto"), "live control exists before the claim");
    assert_ne!(
        runtime.get_run("yt-auto".to_string()).unwrap().unwrap().state,
        RunState::Paused
    );
    assert_eq!(
        auto_resume_next(&runtime, &live, &go).unwrap(),
        AutoResumeStep::Idle,
        "nothing else is pending, and the same run is never resumed twice"
    );
}

#[test]
fn a_crash_during_the_automatic_attempt_does_not_grant_another_automatic_resume() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-crash", ".");
    assert_eq!(
        auto_resume_next(&runtime, &live, &go).unwrap(),
        AutoResumeStep::Started
    );

    // Kill: a new launch reconciles again.
    runtime.reconcile_youtube_after_restart(200).unwrap();
    let record = runtime.get_resume("yt-crash".to_string()).unwrap().unwrap();
    assert_eq!(record.state, ResumeState::NeedsUser);
    assert!(record.auto_resume_used);
    let fresh_live = YouTubeLiveHandle::default();
    assert_eq!(
        auto_resume_next(&runtime, &fresh_live, &go).unwrap(),
        AutoResumeStep::Idle
    );
    assert_eq!(
        runtime.get_run("yt-crash".to_string()).unwrap().unwrap().state,
        RunState::Paused
    );
}

#[test]
fn a_blocked_preflight_parks_the_run_without_spending_the_budget() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-park", ".");
    let step = auto_resume_next(&runtime, &live, &|_, _| Preflight::Park {
        code: "HELPER_MISSING".to_string(),
        reason: "set up helpers".to_string(),
    })
    .unwrap();
    assert_eq!(step, AutoResumeStep::Parked);
    let record = runtime.get_resume("yt-park".to_string()).unwrap().unwrap();
    assert_eq!(record.state, ResumeState::NeedsUser);
    assert!(!record.auto_resume_used);
    assert_eq!(record.reason.as_deref(), Some("set up helpers"));
    assert!(!live.has_active_run("yt-park"));
    // The run is not retried automatically.
    assert_eq!(auto_resume_next(&runtime, &live, &go).unwrap(), AutoResumeStep::Idle);
}

#[test]
fn waiting_for_a_helper_repair_changes_nothing() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-wait", ".");
    assert_eq!(
        auto_resume_next(&runtime, &live, &|_, _| Preflight::Wait).unwrap(),
        AutoResumeStep::Waiting
    );
    let record = runtime.get_resume("yt-wait".to_string()).unwrap().unwrap();
    assert_eq!(record.state, ResumeState::AutoPending);
    assert!(!record.auto_resume_used);
}

#[test]
fn runs_resume_sequentially_one_at_a_time() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    for (index, id) in ["yt-one", "yt-two"].iter().enumerate() {
        runtime
            .submit_youtube_download(
                (*id).to_string(),
                "vid".to_string(),
                request_json(&[work_item(1)]),
                ".".to_string(),
                100 + index as i64,
            )
            .unwrap();
    }
    runtime.reconcile_youtube_after_restart(110).unwrap();
    assert_eq!(
        auto_resume_next(&runtime, &live, &go).unwrap(),
        AutoResumeStep::Started
    );
    assert!(live.has_active_run("yt-one"));
    // The slot is taken, so the second one waits for its turn.
    assert_eq!(
        auto_resume_next(&runtime, &live, &go).unwrap(),
        AutoResumeStep::Waiting
    );
    assert_eq!(
        runtime.get_run("yt-two".to_string()).unwrap().unwrap().state,
        RunState::Paused
    );
}

#[test]
fn continue_starts_a_single_attempt_even_when_clicked_twice() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-continue", ".");
    runtime
        .park_for_user("yt-continue".to_string(), "network".to_string(), None, 120, None)
        .unwrap();

    let first = continue_run_with(&runtime, &live, "yt-continue", generation_of(&runtime, "yt-continue"), &go).unwrap();
    let second = continue_run_with(&runtime, &live, "yt-continue", generation_of(&runtime, "yt-continue"), &go).unwrap();
    assert!(first.started);
    assert!(!second.started, "a second click must do nothing");
    assert!(live.has_active_run("yt-continue"));
}

#[test]
fn continue_while_the_attempt_is_running_does_nothing() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-busy", ".");
    assert_eq!(
        auto_resume_next(&runtime, &live, &go).unwrap(),
        AutoResumeStep::Started
    );
    let response = continue_run_with(&runtime, &live, "yt-busy", generation_of(&runtime, "yt-busy"), &go).unwrap();
    assert!(!response.started);
}

#[test]
fn continue_reports_why_it_cannot_start_and_keeps_the_run_paused() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-blocked", ".");
    let error = continue_run_with(&runtime, &live, "yt-blocked", generation_of(&runtime, "yt-blocked"), &|_, _| Preflight::Park {
        code: "DISK_FULL".to_string(),
        reason: "Not enough free disk space".to_string(),
    })
    .unwrap_err();
    assert_eq!(error.code, "DISK_FULL");
    let record = runtime.get_resume("yt-blocked".to_string()).unwrap().unwrap();
    assert_eq!(record.state, ResumeState::NeedsUser);
    assert_eq!(record.reason.as_deref(), Some("Not enough free disk space"));
    assert_eq!(
        runtime.get_run("yt-blocked".to_string()).unwrap().unwrap().state,
        RunState::Paused
    );
}

#[test]
fn the_supervisor_hook_is_quiet_without_pending_runs_and_defers_to_a_busy_slot() {
    let (_dir, runtime) = runtime();
    let live = Arc::new(YouTubeLiveHandle::default());
    let hook = YoutubeResumeSupervisor::new(Arc::clone(&live), None);
    assert_eq!(hook.reconcile(&runtime, 1000).unwrap(), None);

    interrupted_run(&runtime, "yt-hook", ".");
    let _scan = live.begin_discovery("scan-1".to_string()).unwrap();
    assert_eq!(
        hook.reconcile(&runtime, 1000).unwrap(),
        Some(1000 + RETRY_SECS),
        "a scan owns the slot, so the run waits"
    );
    assert_eq!(
        runtime.get_run("yt-hook".to_string()).unwrap().unwrap().state,
        RunState::Paused
    );
}

// ---- preflight ---------------------------------------------------------------

fn resume_record(expected: Option<u64>, key: Option<&str>) -> RunResumeRecord {
    RunResumeRecord {
        run_id: "r".to_string(),
        state: ResumeState::AutoPending,
        auto_resume_used: false,
        reason: None,
        reason_code: None,
        expected_bytes: expected,
        expected_item_key: key.map(str::to_string),
        updated_at: 1,
        generation: 0,
        last_revision: 0,
    }
}

fn run_with_root(root: &str) -> RunRecord {
    RunRecord {
        id: "r".to_string(),
        workflow_type: WorkflowType::youtube_download(),
        provider: "youtube".to_string(),
        state: RunState::Paused,
        legacy_origin: None,
        legacy_id: None,
        request_json: request_json(&[work_item(1)]),
        output_root: root.to_string(),
        error_message: None,
        created_at: 1,
        updated_at: 1,
        completed_at: None,
    }
}

fn probes<'a>(
    helper: &'a dyn Fn() -> HelperReadiness,
    free: &'a dyn Fn(&Path) -> Option<u64>,
) -> PreflightProbes<'a> {
    static VALIDATE: fn(&Path) -> Result<ValidatedOutputRoot, String> =
        |path| validate_output_root(path).map_err(|error| error.to_string());
    PreflightProbes {
        helper,
        validate_root: &VALIDATE,
        free_space: free,
    }
}

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn preflight_goes_when_everything_holds_and_the_size_is_unknown() {
    let dir = tempdir().unwrap();
    let run = run_with_root(&dir.path().to_string_lossy());
    let outcome = preflight_resume(
        &run,
        &resume_record(None, None),
        true,
        &probes(&|| HelperReadiness::Ready, &|_| Some(1)),
    );
    assert_eq!(outcome, Preflight::Go, "an unknown size skips the disk check");
}

#[test]
fn preflight_waits_for_a_repair_only_on_the_automatic_path() {
    let dir = tempdir().unwrap();
    let run = run_with_root(&dir.path().to_string_lossy());
    let p = probes(&|| HelperReadiness::Repairing, &|_| None);
    assert_eq!(
        preflight_resume(&run, &resume_record(None, None), true, &p),
        Preflight::Wait
    );
    assert!(matches!(
        preflight_resume(&run, &resume_record(None, None), false, &p),
        Preflight::Park { .. }
    ));
}

#[test]
fn preflight_parks_when_the_helpers_are_missing() {
    let dir = tempdir().unwrap();
    let run = run_with_root(&dir.path().to_string_lossy());
    let outcome = preflight_resume(
        &run,
        &resume_record(None, None),
        true,
        &probes(
            &|| HelperReadiness::Missing {
                code: "HELPER_MISSING".to_string(),
                message: "missing".to_string(),
            },
            &|_| None,
        ),
    );
    match outcome {
        Preflight::Park { code, reason } => {
            assert_eq!(code, "HELPER_MISSING");
            assert!(reason.contains("helpers"));
        }
        other => panic!("expected park, got {other:?}"),
    }
}

#[test]
fn preflight_parks_when_the_runs_own_folder_is_gone() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("unplugged-drive");
    let run = run_with_root(&missing.to_string_lossy());
    let outcome = preflight_resume(
        &run,
        &resume_record(None, None),
        true,
        &probes(&|| HelperReadiness::Ready, &|_| None),
    );
    match outcome {
        Preflight::Park { code, reason } => {
            assert_eq!(code, "OUTPUT_ROOT_UNAVAILABLE");
            assert!(reason.contains("unplugged-drive"));
        }
        other => panic!("expected park, got {other:?}"),
    }
}

#[test]
fn preflight_parks_for_low_disk_space_counting_the_part_bytes_already_present() {
    let dir = tempdir().unwrap();
    let run = run_with_root(&dir.path().to_string_lossy());
    let item = work_item(1);
    let staging = dir
        .path()
        .join(STAGING_DIR_NAME)
        .join(resumable_staging_name(&item.occurrence_id, &item.artifact_fingerprint));
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("media.f137.mp4.part"), vec![0u8; 1024]).unwrap();

    let record = resume_record(Some(10 * GIB), Some("occ-1"));
    let enough = 10 * GIB + 1024 * 1024 * 1024;
    let ready_low = || HelperReadiness::Ready;
    let low_free = |_: &Path| Some(GIB);
    let low = probes(&ready_low, &low_free);
    match preflight_resume(&run, &record, true, &low) {
        Preflight::Park { code, reason } => {
            assert_eq!(code, "DISK_FULL");
            assert!(reason.contains("free disk space"));
        }
        other => panic!("expected park, got {other:?}"),
    }
    let ready = || HelperReadiness::Ready;
    let plenty_free = |_: &Path| Some(enough);
    let plenty = probes(&ready, &plenty_free);
    assert_eq!(preflight_resume(&run, &record, true, &plenty), Preflight::Go);
}

#[test]
fn preflight_parks_on_an_unreadable_saved_request() {
    let dir = tempdir().unwrap();
    let mut run = run_with_root(&dir.path().to_string_lossy());
    run.request_json = "{not json".to_string();
    let outcome = preflight_resume(
        &run,
        &resume_record(None, None),
        true,
        &probes(&|| HelperReadiness::Ready, &|_| None),
    );
    assert!(matches!(outcome, Preflight::Park { code, .. } if code == "REQUEST_INVALID"));
}

// ---- staging references / cancel ---------------------------------------------

#[test]
fn only_non_terminal_runs_reference_their_staging() {
    let mut paused = run_with_root("C:/Out/");
    paused.id = "paused".to_string();
    let mut done = run_with_root("C:\\out");
    done.id = "done".to_string();
    done.state = RunState::Succeeded;
    let mut other = run_with_root("D:/Other");
    other.id = "other".to_string();
    other.request_json = request_json(&[work_item(2)]);
    let referenced = referenced_staging_names(&[paused, done, other]);
    let expected_one = resumable_staging_name("occ-1", "af-1");
    let expected_two = resumable_staging_name("occ-2", "af-2");
    assert_eq!(
        referenced.get("c:\\out"),
        Some(&HashSet::from([expected_one]))
    );
    assert_eq!(
        referenced.get("d:\\other"),
        Some(&HashSet::from([expected_two]))
    );
}

#[test]
fn cancelling_a_paused_run_removes_its_partial_files() {
    let dir = tempdir().unwrap();
    let root = validate_output_root(dir.path()).unwrap();
    let run = run_with_root(&dir.path().to_string_lossy());
    let item = work_item(1);
    let held = root
        .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
        .unwrap();
    let staging_path = held.lease.path().to_path_buf();
    std::fs::write(staging_path.join("media.f137.mp4.part"), b"partial").unwrap();
    drop(held);

    discard_paused_run_staging(&run, &HashSet::new());
    assert!(!staging_path.exists());
}

#[test]
fn the_startup_sweep_keeps_resumable_runs_and_removes_nothing_recent() {
    let (_dir, runtime) = runtime();
    let out = tempdir().unwrap();
    let root = validate_output_root(out.path()).unwrap();
    let item = work_item(1);
    let held = root
        .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
        .unwrap();
    let path = held.lease.path().to_path_buf();
    std::fs::write(path.join("media.f137.mp4.part"), b"partial").unwrap();
    drop(held);
    interrupted_run(&runtime, "yt-sweep", &out.path().to_string_lossy());

    assert_eq!(sweep_orphans(&runtime, &[]), 0);
    assert!(path.exists());
}

#[test]
fn a_projection_carries_the_state_reason_and_preserved_bytes() {
    let dir = tempdir().unwrap();
    let (_db, runtime) = runtime();
    let root = validate_output_root(dir.path()).unwrap();
    let item = work_item(1);
    let held = root
        .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
        .unwrap();
    std::fs::write(held.lease.path().join("media.f137.mp4.part"), vec![0u8; 321]).unwrap();
    drop(held);
    interrupted_run(&runtime, "yt-project", &dir.path().to_string_lossy());
    let (run, record) = runtime.list_youtube_resumable().unwrap().remove(0);
    let projected = project_resumable(&run, &record);
    assert_eq!(projected.state, "auto_pending");
    assert_eq!(projected.title, "Video 1");
    assert_eq!(projected.video_count, 1);
    assert_eq!(projected.preserved_bytes, 321);
    assert!(projected.reason.unwrap().contains("restart"));
}

// ---- cancel vs. admission ------------------------------------------------------

#[test]
fn a_cancel_that_lands_first_makes_a_later_admission_refuse_the_run() {
    use crate::providers::youtube::commands::cancel_youtube_download_blocking;
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    let run = interrupted_run(&runtime, "yt-cancel-first", ".");

    cancel_youtube_download_blocking(&live, &runtime, "yt-cancel-first").unwrap();
    let admission = attach_and_begin(&runtime, &live, &run, ResumeTrigger::Auto).unwrap();

    assert_eq!(admission, Admission::NotResumable);
    assert!(!live.has_active_run("yt-cancel-first"), "no control may be left attached");
    assert_eq!(
        runtime.get_run("yt-cancel-first".to_string()).unwrap().unwrap().state,
        RunState::Cancelled
    );
}

#[test]
fn a_cancel_after_admission_signals_the_attached_control() {
    use crate::providers::youtube::commands::cancel_youtube_download_blocking;
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-cancel-after", ".");
    assert_eq!(
        auto_resume_next(&runtime, &live, &go).unwrap(),
        AutoResumeStep::Started
    );
    let control = live.control_for("yt-cancel-after").unwrap();

    cancel_youtube_download_blocking(&live, &runtime, "yt-cancel-after").unwrap();

    assert!(control.is_cancelled(), "the running download must be told to stop");
}

#[test]
fn cancel_is_serialized_with_admission_by_the_drain_lock() {
    use crate::providers::youtube::commands::cancel_youtube_download_blocking;
    let (_dir, runtime) = runtime();
    let live = Arc::new(YouTubeLiveHandle::default());
    interrupted_run(&runtime, "yt-serial", ".");
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    runtime.with_drain_lock(|| {
        let runtime = runtime.clone();
        let live = Arc::clone(&live);
        std::thread::spawn(move || {
            let _ = cancel_youtube_download_blocking(&live, &runtime, "yt-serial");
            let _ = finished_tx.send(());
        });
        assert!(
            finished_rx
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "cancel must wait for an admission in progress"
        );
    });
    finished_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("cancel proceeds once the lock is free");
}

// ---- sweep references ------------------------------------------------------------

#[test]
fn the_sweep_keeps_an_old_paused_run_behind_more_than_a_thousand_newer_runs() {
    let (_db, runtime) = runtime();
    let out = tempdir().unwrap();
    let root = validate_output_root(out.path()).unwrap();
    let item = work_item(1);
    let held = root
        .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
        .unwrap();
    let path = held.lease.path().to_path_buf();
    std::fs::write(path.join("media.f137.mp4.part"), b"partial").unwrap();
    drop(held);
    let root_text = out.path().to_string_lossy().into_owned();
    runtime
        .submit_youtube_download(
            "yt-oldest".to_string(),
            "vid-1".to_string(),
            request_json(&[work_item(1)]),
            root_text.clone(),
            1,
        )
        .unwrap();
    for index in 0..1100 {
        runtime
            .submit_youtube_download(
                format!("yt-newer-{index}"),
                "vid-2".to_string(),
                request_json(&[work_item(2)]),
                root_text.clone(),
                10 + index,
            )
            .unwrap();
    }
    runtime.reconcile_youtube_after_restart(5000).unwrap();

    let thirty_days = std::time::Duration::from_secs(30 * 24 * 60 * 60);
    let removed = sweep_orphans_at(&runtime, &[], SystemTime::now() + thirty_days);

    assert_eq!(removed, 0);
    assert!(path.exists(), "the oldest paused run still owns its partial files");
}

#[test]
fn the_sweep_removes_a_cancelled_runs_staging_whatever_its_age() {
    let (_db, runtime) = runtime();
    let out = tempdir().unwrap();
    let root = validate_output_root(out.path()).unwrap();
    let item = work_item(1);
    let held = root
        .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
        .unwrap();
    let path = held.lease.path().to_path_buf();
    std::fs::write(path.join("media.f137.mp4.part"), b"partial").unwrap();
    drop(held);
    runtime
        .submit_youtube_download(
            "yt-cancelled".to_string(),
            "vid-1".to_string(),
            request_json(&[work_item(1)]),
            out.path().to_string_lossy().into_owned(),
            1,
        )
        .unwrap();
    runtime.cancel_run("yt-cancelled".to_string(), 2).unwrap();

    sweep_orphans(&runtime, &[]);

    assert!(!path.exists());
}

struct FailsAgain;

impl crate::workflow::ports::executor::StepExecutor for FailsAgain {
    fn workflow_type(&self) -> &'static str {
        "youtube_download"
    }

    fn execute(
        &self,
        _run: &RunRecord,
        _step: &crate::workflow::domain::types::StepRecord,
    ) -> crate::workflow::ports::executor::ExecutorOutcome {
        crate::workflow::ports::executor::ExecutorOutcome::paused_for_user(
            "dropped again".to_string(),
            None,
        )
    }
}

// ---- Continue names the pause it answers -------------------------------------

#[test]
fn a_delayed_duplicate_continue_cannot_start_another_attempt_after_a_failure() {
    let (_dir, runtime) = runtime();
    let live = YouTubeLiveHandle::default();
    interrupted_run(&runtime, "yt-dup", ".");
    runtime
        .park_for_user("yt-dup".to_string(), "network".to_string(), None, 120, None)
        .unwrap();
    let shown = generation_of(&runtime, "yt-dup");

    // First click: admitted, the attempt runs and fails, the run pauses again.
    assert!(
        continue_run_with(&runtime, &live, "yt-dup", shown, &go)
            .unwrap()
            .started
    );
    runtime.register_executor(Arc::new(FailsAgain));
    runtime.drain_type("youtube_download").unwrap();
    live.clear_run("yt-dup");
    assert_eq!(
        runtime.get_run("yt-dup".to_string()).unwrap().unwrap().state,
        RunState::Paused
    );
    assert!(generation_of(&runtime, "yt-dup") > shown);

    // The duplicate of the first click arrives late, carrying the old pause.
    let late = continue_run_with(&runtime, &live, "yt-dup", shown, &go).unwrap();
    assert!(!late.started, "a stale generation must not start an attempt");
    assert!(!live.has_active_run("yt-dup"));
    assert_eq!(
        runtime.get_run("yt-dup".to_string()).unwrap().unwrap().state,
        RunState::Paused
    );
}

#[test]
fn the_format_id_record_gives_the_exact_selection_for_merged_and_single_formats() {
    use crate::providers::youtube::download_progress::{parse_format_id_line, parse_pinned_formats};
    assert_eq!(
        parse_format_id_line("LVFORMATID 137+140"),
        Some(names(&["137", "140"]))
    );
    assert_eq!(parse_format_id_line("LVFORMATID 18"), Some(names(&["18"])));
    assert_eq!(parse_format_id_line("LVFORMATID "), None);
    assert_eq!(parse_pinned_formats("bestvideo*+bestaudio/best"), None);
    assert_eq!(parse_pinned_formats("137+"), None);
    assert!(pinned_format_unavailable("ERROR: Requested format is not available. Use --list-formats"));
    assert!(!pinned_format_unavailable("HTTP Error 403"));
}

// ---- shared staging, stale park, durable revisions ---------------------------------

struct PausesAtRevision(WorkflowRuntime);

impl crate::workflow::ports::executor::StepExecutor for PausesAtRevision {
    fn workflow_type(&self) -> &'static str {
        "youtube_download"
    }

    fn execute(
        &self,
        run: &RunRecord,
        _step: &crate::workflow::domain::types::StepRecord,
    ) -> crate::workflow::ports::executor::ExecutorOutcome {
        self.0
            .pause_running_for_user(run.id.clone(), "x".to_string(), "HELPER_FAILED".to_string(), 4000)
            .unwrap();
        crate::workflow::ports::executor::ExecutorOutcome::paused_for_user("x".to_string(), None)
    }
}

fn stage_partials(root: &Path) -> std::path::PathBuf {
    let root = validate_output_root(root).unwrap();
    let item = work_item(1);
    let held = root
        .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
        .unwrap();
    let path = held.lease.path().to_path_buf();
    std::fs::write(path.join("media.f137.mp4.part"), b"partial").unwrap();
    path
}

#[test]
fn a_cancelled_run_never_wipes_the_partials_of_a_paused_run_sharing_its_staging() {
    let (_db, runtime) = runtime();
    let out = tempdir().unwrap();
    let path = stage_partials(out.path());
    let root_text = out.path().to_string_lossy().into_owned();
    for (id, created) in [("yt-a", 1), ("yt-b", 2)] {
        runtime
            .submit_youtube_download(
                id.to_string(),
                "vid-1".to_string(),
                request_json(&[work_item(1)]),
                root_text.clone(),
                created,
            )
            .unwrap();
    }
    runtime.reconcile_youtube_after_restart(10).unwrap();

    // The startup sweep: A is cancelled, B (same video and options) is paused.
    runtime.cancel_run("yt-a".to_string(), 11).unwrap();
    sweep_orphans(&runtime, &[]);
    assert!(path.exists(), "B's partial download must survive A's cleanup");

    // The same through the live cancel path.
    runtime
        .submit_youtube_download(
            "yt-c".to_string(),
            "vid-1".to_string(),
            request_json(&[work_item(1)]),
            root_text,
            3,
        )
        .unwrap();
    runtime.reconcile_youtube_after_restart(12).unwrap();
    let live = YouTubeLiveHandle::default();
    crate::providers::youtube::commands::cancel_youtube_download_blocking(&live, &runtime, "yt-c")
        .unwrap();
    assert!(path.exists(), "cancelling C must not wipe B's partials either");

    // Once B is gone too, nothing references it and the cancel cleans up.
    crate::providers::youtube::commands::cancel_youtube_download_blocking(&live, &runtime, "yt-b")
        .unwrap();
    assert!(!path.exists());
}

#[test]
fn a_stale_park_cannot_overwrite_a_newer_pause() {
    let (_dir, runtime) = runtime();
    interrupted_run(&runtime, "yt-stale", ".");
    let shown = generation_of(&runtime, "yt-stale");
    // Another pause lands (a newer generation with its own reason).
    runtime
        .park_for_user("yt-stale".to_string(), "disk is full".to_string(), None, 130, None)
        .unwrap();
    let newer = generation_of(&runtime, "yt-stale");
    assert!(newer > shown);

    // A preflight that was computed against the older pause arrives late.
    let applied = runtime
        .park_for_user(
            "yt-stale".to_string(),
            "helpers missing".to_string(),
            None,
            140,
            Some(shown),
        )
        .unwrap();

    assert!(!applied, "a stale park is dropped");
    let record = runtime.get_resume("yt-stale".to_string()).unwrap().unwrap();
    assert_eq!(record.reason.as_deref(), Some("disk is full"));
    assert_eq!(record.generation, newer, "and it must not bump the generation");
}

#[test]
fn revisions_continue_from_the_durable_base_even_with_a_forgetful_live_handle() {
    let (_dir, runtime) = runtime();
    interrupted_run(&runtime, "yt-rev", ".");
    runtime
        .park_for_user("yt-rev".to_string(), "x".to_string(), None, 120, None)
        .unwrap();
    // The run was paused when its live view had reached revision 4000.
    assert_eq!(
        auto_resume_next(&runtime, &YouTubeLiveHandle::default(), &go).unwrap(),
        AutoResumeStep::Idle
    );
    runtime.register_executor(Arc::new(PausesAtRevision(runtime.clone())));
    let live = YouTubeLiveHandle::default();
    let run = runtime.get_run("yt-rev".to_string()).unwrap().unwrap();
    assert_eq!(
        attach_and_begin(&runtime, &live, &run, ResumeTrigger::Manual).unwrap(),
        Admission::Started
    );
    runtime.drain_type("youtube_download").unwrap();
    live.clear_run("yt-rev");

    // A handle that has forgotten everything (cache eviction, new handle).
    let fresh = YouTubeLiveHandle::default();
    let run = runtime.get_run("yt-rev".to_string()).unwrap().unwrap();
    attach_and_begin(&runtime, &fresh, &run, ResumeTrigger::Manual).unwrap();

    assert!(fresh.revision_of("yt-rev") > 4000);
}

#[test]
fn only_incomplete_downloads_count_as_partials() {
    assert!(has_incomplete_partials(&names(&["media.f137.mp4.part"])));
    assert!(has_incomplete_partials(&names(&["media.f137.mp4.part-Frag3"])));
    assert!(has_incomplete_partials(&names(&["media.f137.mp4.ytdl"])));
    assert!(!has_incomplete_partials(&names(&["media.mp4", "media.en.vtt"])));
}

// ---- finished downloads, FFmpeg evidence, admission lock -----------------------------

#[test]
fn only_a_merged_download_with_its_marker_skips_yt_dlp() {
    let marker = "media.lvformat";
    assert!(has_complete_merged_media(&names(&[marker, "media.webm"]), false));
    assert!(has_complete_merged_media(
        &names(&[marker, "media.mp4", "media.en.vtt"]),
        true
    ));
    // The transcript is expected but was never downloaded.
    assert!(!has_complete_merged_media(&names(&[marker, "media.mp4"]), true));
    // No record of the format, unfinished parts, or an unmerged pair of streams.
    assert!(!has_complete_merged_media(&names(&["media.mp4"]), false));
    assert!(!has_complete_merged_media(
        &names(&[marker, "media.mp4", "media.f140.m4a.part"]),
        false
    ));
    assert!(!has_complete_merged_media(
        &names(&[marker, "media.f137.mp4", "media.f140.m4a"]),
        false
    ));
    assert_eq!(
        incomplete_leaves(&names(&["media.mp4", "media.f1.mp4.part", "media.f1.mp4.ytdl"])),
        names(&["media.f1.mp4.part", "media.f1.mp4.ytdl"])
    );
}

#[test]
fn only_ffmpegs_own_words_about_the_data_prove_corruption() {
    for proof in [
        "x.mp4: Invalid data found when processing input",
        "[mov,mp4] moov atom not found",
        "Could not find codec parameters for stream 0",
        "EBML header parsing failed",
    ] {
        assert!(ffmpeg_stderr_proves_invalid(proof), "{proof}");
    }
    for nothing in [
        "x.mp4: Permission denied",
        "Input/output error",
        "The process cannot access the file because it is being used by another process",
        "",
    ] {
        assert!(!ffmpeg_stderr_proves_invalid(nothing), "{nothing}");
    }
}

#[test]
fn a_fresh_run_pauses_on_a_busy_staging_directory_instead_of_failing() {
    assert!(pauses_even_when_fresh("STAGING_BUSY"));
    assert!(pauses_even_when_fresh("MEDIA_VERIFY_FAILED"));
    assert!(!pauses_even_when_fresh("MEDIA_CORRUPT"));
    assert!(!pauses_even_when_fresh("HELPER_FAILED"));
}

#[test]
fn the_cleanup_decision_runs_under_the_admission_lock() {
    use crate::providers::youtube::kernel::ResumeLedger;
    let (_dir, runtime) = runtime();
    let (admitted_tx, admitted_rx) = std::sync::mpsc::channel();
    runtime.with_shared_staging("any-run", ".", &mut |_shared| {
        let racing = runtime.clone();
        let tx = admitted_tx.clone();
        std::thread::spawn(move || {
            // An admission of a run that uses the same staging.
            racing.with_drain_lock(|| ());
            let _ = tx.send(());
        });
        assert!(
            admitted_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "admission must wait while cleanup decides and deletes"
        );
    });
    admitted_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("admission proceeds once cleanup is done");
}
