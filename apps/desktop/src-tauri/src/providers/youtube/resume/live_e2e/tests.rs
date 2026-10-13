//! Real-helper end-to-end harness for the resume flow (ignored by default).
//!
//! One process per phase, so the harness script can kill it mid-download:
//!
//! ```text
//! LINKVAULT_DATA_DIR=<dir with media-toolchain> LV_E2E_OUT=<output folder>
//! LV_E2E_URL=<watch url> LV_E2E_VIDEO_ID=<id> LV_E2E_PHASE=start|relaunch|continue
//! cargo test --lib live_resume_phase -- --ignored --nocapture
//! ```

use super::*;
use crate::app::database_diagnostics::DatabaseDiagnostics;
use crate::app::database_writer::DatabaseWriter;
use crate::app::managed_process::{helper_identity, TransientWorkItem};
use crate::providers::youtube::models::{StartYouTubeDownloadRequest, YouTubeDownloadMode};
use crate::providers::youtube::executor::{YouTubeExecutorContext, YouTubeExecutorItemContext};
use crate::providers::youtube::helper::helper_kind;
use crate::providers::youtube::kernel::YoutubeDownloadExecutor;
use crate::providers::youtube::manifest_contract::{
    artifact_fingerprint, ArtifactFingerprintInput, FORMAT_POLICY_VERSION,
};
use std::time::{Duration, Instant};

const RUN_ID: &str = "yt-e2e";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

#[test]
#[ignore = "needs installed helpers (LINKVAULT_DATA_DIR) and network; driven by a harness script"]
fn live_resume_phase() {
    let phase = env("LV_E2E_PHASE");
    let out = env("LV_E2E_OUT");
    let data_dir = crate::app::storage::resolve_data_dir().expect("data dir");
    let db_path = data_dir.join("linkvault.sqlite3");
    let (connection, _) = crate::app::database::initialize_database(&db_path).expect("db");
    drop(connection);
    let writer = DatabaseWriter::start(db_path, DatabaseDiagnostics::default()).expect("writer");
    let runtime = WorkflowRuntime::new(writer);
    let live = Arc::new(YouTubeLiveHandle::default());
    runtime.register_executor(Arc::new(YoutubeDownloadExecutor {
        live: Arc::clone(&live),
        resume: Some(Arc::new(runtime.clone())),
        notify_resume_changed: None,
    }));
    runtime.reconcile_youtube_after_restart(chrono::Utc::now().timestamp()).expect("reconcile");
    runtime.register_supervisor_hook(Arc::new(YoutubeResumeSupervisor::new(
        Arc::clone(&live),
        None,
    )));

    if phase == "start" {
        let helper = helper_identity(helper_kind()).expect("helper identity");
        let video_id = env("LV_E2E_VIDEO_ID");
        let fingerprint = artifact_fingerprint(&ArtifactFingerprintInput {
            occurrence_id: "occ-e2e".to_string(),
            video_id: video_id.clone(),
            mode: YouTubeDownloadMode::VideoOnly,
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: Some(480),
            selected_transcript: None,
            helper_lock_digest: helper.digest.clone(),
        })
        .expect("fingerprint");
        let item = TransientWorkItem {
            occurrence_id: "occ-e2e".to_string(),
            artifact_fingerprint: fingerprint,
            video_id,
            ordinal: 1,
            title: "E2E long video".to_string(),
            source_url: env("LV_E2E_URL"),
        };
        let request = YoutubeDurableRequest {
            schema_version: 1,
            start: StartYouTubeDownloadRequest {
                client_submission_id: "sub-e2e".to_string(),
                scan_plan_id: "plan".to_string(),
                selected_occurrence_ids: vec!["occ-e2e".to_string()],
                output_dir: out.clone(),
                mode: YouTubeDownloadMode::VideoOnly,
                max_height: Some(480),
                preferred_language: None,
                fallback_languages: Vec::new(),
                allow_automatic_captions: false,
                continue_without_transcript: false,
            },
            work_items: vec![item.clone()],
            context: YouTubeExecutorContext {
                source_snapshot_digest: "0".repeat(64),
                playlist_id: None,
                helper_lock_digest: helper.digest,
                items: vec![YouTubeExecutorItemContext {
                    occurrence_id: "occ-e2e".to_string(),
                    playlist_index: None,
                    source_duration_seconds: None,
                    selected_transcript: None,
                }],
            },
            plan_fingerprint: "pf".to_string(),
            client_submission_id: "sub-e2e".to_string(),
        };
        let json = serde_json::to_string(&request).expect("json");
        runtime.with_drain_lock(|| {
            live.attach_run(RUN_ID.to_string(), "sub-e2e".to_string(), "pf".to_string(), &[item])
                .expect("attach");
            runtime
                .submit_youtube_download(
                    RUN_ID.to_string(),
                    "vid".to_string(),
                    json,
                    out.clone(),
                    chrono::Utc::now().timestamp(),
                )
                .expect("submit");
        });
    }
    runtime.start_supervisor().expect("supervisor");

    let mut continued = false;
    let deadline = Instant::now() + Duration::from_secs(1500);
    let mut waited_paused = 0;
    while Instant::now() < deadline {
        let run = runtime.get_run(RUN_ID.to_string()).expect("get").expect("run");
        let resume = runtime.get_resume(RUN_ID.to_string()).expect("resume");
        let live_bytes = live
            .snapshot(Some(RUN_ID))
            .ok()
            .flatten()
            .and_then(|snapshot| snapshot.progress.bytes_completed);
        let item = TransientWorkItem {
            occurrence_id: "occ-e2e".to_string(),
            artifact_fingerprint: String::new(),
            video_id: String::new(),
            ordinal: 1,
            title: String::new(),
            source_url: String::new(),
        };
        let _ = item;
        let staging_bytes = staging_total(&out);
        println!(
            "STATUS phase={phase} run={:?} resume={:?} used={:?} live_bytes={live_bytes:?} staging_bytes={staging_bytes} staging_dirs={:?}",
            run.state,
            resume.as_ref().map(|r| r.state),
            resume.as_ref().map(|r| r.auto_resume_used),
            staging_dirs(&out),
        );
        if run.state.is_terminal() {
            println!("DONE terminal={:?} err={:?}", run.state, run.error_message);
            return;
        }
        let needs_user = resume
            .as_ref()
            .is_some_and(|r| r.state == ResumeState::NeedsUser)
            && run.state == RunState::Paused;
        if needs_user && phase == "continue" && !continued {
            let response = continue_run(
                &runtime,
                &live,
                RUN_ID,
                resume.as_ref().map_or(0, |r| r.generation),
            ).expect("continue");
            println!("CONTINUE started={}", response.started);
            continued = true;
        } else if needs_user && phase != "continue" {
            waited_paused += 1;
            if waited_paused >= 8 {
                println!("PAUSED reason={:?}", resume.and_then(|r| r.reason));
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    panic!("timed out");
}

fn staging_dirs(out: &str) -> Vec<String> {
    let lv = Path::new(out).join(STAGING_DIR_NAME);
    std::fs::read_dir(lv)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn staging_total(out: &str) -> u64 {
    let lv = Path::new(out).join(STAGING_DIR_NAME);
    let mut total = 0;
    if let Ok(dirs) = std::fs::read_dir(lv) {
        for dir in dirs.flatten() {
            if let Ok(files) = std::fs::read_dir(dir.path()) {
                for file in files.flatten() {
                    total += file.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
    }
    total
}
