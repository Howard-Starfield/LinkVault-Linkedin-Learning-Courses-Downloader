//! Safe automatic resume of interrupted YouTube downloads.
//!
//! # How the pieces fit
//!
//! * `WorkflowRuntime::reconcile_youtube_after_restart` pauses interrupted
//!   runs durably (`ResumeState::AutoPending`) instead of failing them.
//! * [`YoutubeResumeSupervisor`] is a `SupervisorHook` on the existing
//!   workflow supervisor (no new scheduler or loop). When no download or
//!   discovery is active it resumes one `auto_pending` run at a time: it
//!   re-checks helpers, the run's own output folder and free disk space,
//!   then attaches the live control and calls `begin_resume`, which durably
//!   spends the one-shot budget **before** the supervisor can claim the run.
//! * [`continue_run`] is the user's Continue button: the same admission with
//!   the manual trigger, idempotent under double clicks.
//! * A resumed attempt that fails does not retry and does not fail the run:
//!   the kernel returns `ExecutorOutcome::paused_for_user`, the run becomes
//!   `needs_user`, and only another Continue starts another attempt.
//!
//! # Budget and crash semantics
//!
//! `auto_resume_used` is set in the same transaction that requeues the run.
//! If the app dies during the automatic attempt, the next launch finds a
//! `resuming` record with the budget spent and parks the run for the user
//! (a second interruption pauses). The budget is spent only when an attempt
//! is actually admitted: a missing helper, an unavailable folder or too
//! little disk space park the run for the user without spending it.
//!
//! # Interaction with in-attempt retries
//!
//! yt-dlp keeps its own `--retries 10` / `--fragment-retries 10` inside one
//! attempt. They are not restarts: they never launch a new attempt, never
//! touch the budget, and a failure that survives them is the attempt's
//! failure. Workflow-level retry is never used for YouTube
//! (`retryable_failure` is not produced), so no automatic restart exists
//! beyond the single budgeted one.

use crate::app::disk_space::{self, SpaceDecision};
use crate::app::managed_process::{helper_identity, TransientWorkItem};
use crate::app::safe_output_filesystem::resume::{
    resumable_staging_name, ORPHAN_MIN_AGE, STAGING_DIR_NAME,
};
use crate::app::safe_output_filesystem::{validate_output_root, ValidatedOutputRoot};
use crate::providers::youtube::commands::{helper_status_response, opaque_id};
use crate::providers::youtube::error::YouTubeError;
use crate::providers::youtube::helper::helper_kind;
use crate::providers::youtube::live::{YouTubeLiveHandle, YoutubeDurableRequest};
use crate::providers::youtube::models::YouTubeHelperBackendStatus;
use crate::workflow::application::runtime::SupervisorHook;
use crate::workflow::domain::errors::WorkflowError;
use crate::workflow::domain::resume::{ResumeState, ResumeTrigger, RunResumeRecord};
use crate::workflow::domain::state::RunState;
use crate::workflow::domain::types::RunRecord;
use crate::workflow::WorkflowRuntime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Event the UI listens to for resume-state changes.
pub const RESUME_EVENT_NAME: &str = "linkvault://youtube-resume-changed";

const RETRY_SECS: i64 = 5;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

const MEDIA_EXTENSIONS: [&str; 11] = [
    "mp4", "m4a", "webm", "mkv", "mp3", "opus", "ogg", "mov", "flv", "3gp", "aac",
];

/// The yt-dlp format id a staging file belongs to, for names like
/// `media.f137.mp4.part`, `media.f137.mp4.ytdl`, `media.f140.m4a`. Subtitle
/// files (`media.fr.vtt`) and merged output (`media.mp4`) have none.
pub fn format_id_of_leaf(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("media.f")?;
    let (id, tail) = rest.split_once('.')?;
    if id.is_empty() {
        return None;
    }
    let is_part = tail.contains(".part") || tail.ends_with(".ytdl") || tail.starts_with("part");
    let extension = tail.split('.').next().unwrap_or("");
    let is_stream_file = MEDIA_EXTENSIONS.contains(&extension) && !tail.ends_with(".vtt");
    (is_part || is_stream_file).then_some(id)
}

/// Staging files that belong to a format yt-dlp did not choose this time.
/// Mixing them with the new format's bytes would corrupt the result, so they
/// are removed before the download continues.
pub fn stale_part_leaves(names: &[String], chosen_format_ids: &[String]) -> Vec<String> {
    if chosen_format_ids.is_empty() {
        return Vec::new();
    }
    names
        .iter()
        .filter(|name| {
            format_id_of_leaf(name).is_some_and(|id| !chosen_format_ids.iter().any(|c| c == id))
        })
        .cloned()
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    DiskFull,
    VideoUnavailable,
    Other,
}

/// Classify a helper failure from yt-dlp's output.
pub fn classify_helper_failure(message: &str) -> FailureKind {
    let lowered = message.to_lowercase();
    const DISK_FULL: [&str; 5] = [
        "no space left on device",
        "errno 28",
        "not enough space on the disk",
        "there is not enough space",
        "disk full",
    ];
    const UNAVAILABLE: [&str; 6] = [
        "video unavailable",
        "private video",
        "this video has been removed",
        "video has been removed",
        "account associated with this video has been terminated",
        "this video is no longer available",
    ];
    if DISK_FULL.iter().any(|needle| lowered.contains(needle)) {
        FailureKind::DiskFull
    } else if UNAVAILABLE.iter().any(|needle| lowered.contains(needle)) {
        FailureKind::VideoUnavailable
    } else {
        FailureKind::Other
    }
}

/// Whether a failed attempt's partial files are worth resuming from. Failures
/// after the download finished (verification, transcript, manifest) would
/// fail the same way again, so those start over.
pub fn keep_staging_after(code: &str) -> bool {
    matches!(
        code,
        "HELPER_FAILED"
            | "HELPER_TIMEOUT"
            | "HELPER_TEMP_ROOT_INVALID"
            | "HELPER_INTEGRITY_FAILED"
            | "DISK_FULL"
            | "VIDEO_UNAVAILABLE"
            | "STAGING_BUSY"
    )
}

/// The wording shown beside Continue for a failed attempt.
pub fn pause_reason(code: &str, message: &str) -> String {
    match code {
        "DISK_FULL" => {
            "The disk filled up. Free some space, then press Continue to pick up where it stopped."
                .to_string()
        }
        "VIDEO_UNAVAILABLE" => {
            "This video is no longer available on YouTube (deleted, private or restricted)."
                .to_string()
        }
        "HELPER_MISSING" | "HELPER_INTEGRITY_FAILED" => {
            "The YouTube helpers need to be set up. Choose Download YouTube helpers, then press Continue."
                .to_string()
        }
        _ => {
            let trimmed = message.trim();
            if trimmed.is_empty() {
                "The download stopped. Press Continue to try again.".to_string()
            } else {
                let short: String = trimmed.chars().take(300).collect();
                format!("The download stopped: {short}")
            }
        }
    }
}

/// Bytes of the item's resumable staging directory (zero when absent).
pub fn present_staging_bytes(root: &Path, occurrence_id: &str, fingerprint: &str) -> u64 {
    let directory = root
        .join(STAGING_DIR_NAME)
        .join(resumable_staging_name(occurrence_id, fingerprint));
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .fold(0u64, u64::saturating_add)
}

/// Staging directory names that non-terminal runs still reference, grouped by
/// normalized output root.
pub fn referenced_staging_names(runs: &[RunRecord]) -> BTreeMap<String, HashSet<String>> {
    let mut referenced: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for run in runs.iter().filter(|run| !run.state.is_terminal()) {
        let entry = referenced.entry(normalize_root(&run.output_root)).or_default();
        if let Ok(request) = serde_json::from_str::<YoutubeDurableRequest>(&run.request_json) {
            for item in &request.work_items {
                entry.insert(resumable_staging_name(
                    &item.occurrence_id,
                    &item.artifact_fingerprint,
                ));
            }
        }
    }
    referenced
}

pub fn normalize_root(root: &str) -> String {
    root.trim()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperReadiness {
    Ready,
    /// An install or repair is running; wait for it.
    Repairing,
    Missing { code: String, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preflight {
    /// Every precondition holds; an attempt may be admitted.
    Go,
    /// Not ready yet but expected to be soon (helper repair). Retry later,
    /// change nothing, spend nothing.
    Wait,
    /// Cannot start. Keep the run paused for the user with this reason.
    Park { code: String, reason: String },
}

/// External facts the preflight depends on, injectable for tests.
pub struct PreflightProbes<'a> {
    pub helper: &'a dyn Fn() -> HelperReadiness,
    pub validate_root: &'a dyn Fn(&Path) -> Result<ValidatedOutputRoot, String>,
    pub free_space: &'a dyn Fn(&Path) -> Option<u64>,
}

fn park(code: &str, reason: impl Into<String>) -> Preflight {
    Preflight::Park {
        code: code.to_string(),
        reason: reason.into(),
    }
}

/// Decide whether a resumed attempt may start now. `allow_wait` is true for
/// the automatic path (a helper repair is waited for) and false for the
/// user's Continue (which is told to try again after the repair).
pub fn preflight_resume(
    run: &RunRecord,
    resume: &RunResumeRecord,
    allow_wait: bool,
    probes: &PreflightProbes<'_>,
) -> Preflight {
    let request = match serde_json::from_str::<YoutubeDurableRequest>(&run.request_json) {
        Ok(request) => request,
        Err(_) => {
            return park(
                "REQUEST_INVALID",
                "This download's saved request could not be read, so it cannot be resumed. Cancel it and start it again.",
            )
        }
    };
    match (probes.helper)() {
        HelperReadiness::Ready => {}
        HelperReadiness::Repairing if allow_wait => return Preflight::Wait,
        HelperReadiness::Repairing => {
            return park(
                "HELPER_REPAIRING",
                "The YouTube helpers are still being set up. Press Continue when that finishes.",
            )
        }
        HelperReadiness::Missing { code, message } => {
            let reason = pause_reason(&code, &message);
            return park(&code, reason);
        }
    }
    // The run's own folder, never today's folder preference.
    let root_path = PathBuf::from(&run.output_root);
    let root = match (probes.validate_root)(&root_path) {
        Ok(root) => root,
        Err(_) => {
            return park(
                "OUTPUT_ROOT_UNAVAILABLE",
                format!(
                    "The download folder isn't available: {}. Reconnect the drive or make the folder writable, then press Continue.",
                    run.output_root
                ),
            )
        }
    };
    let item = resume
        .expected_item_key
        .as_deref()
        .and_then(|key| request.work_items.iter().find(|item| item.occurrence_id == key));
    let present = item.map_or(0, |item| {
        present_staging_bytes(root.path(), &item.occurrence_id, &item.artifact_fingerprint)
    });
    let free = (probes.free_space)(root.path());
    if let SpaceDecision::Insufficient { needed, free } =
        disk_space::decide_space(resume.expected_bytes, present, free)
    {
        return park(
            "DISK_FULL",
            format!(
                "Not enough free disk space: about {} is needed and {} is free. Free up space, then press Continue.",
                disk_space::format_bytes(needed),
                disk_space::format_bytes(free)
            ),
        );
    }
    Preflight::Go
}

/// Probes backed by the real helpers, filesystem and volume.
pub fn real_preflight(
    run: &RunRecord,
    resume: &RunResumeRecord,
    allow_wait: bool,
) -> Preflight {
    let helper = || {
        if crate::app::media_toolchain::install_in_progress() {
            return HelperReadiness::Repairing;
        }
        let status = helper_status_response(
            helper_identity(helper_kind()),
            crate::app::media_toolchain::managed_install_present,
        );
        match status.status {
            YouTubeHelperBackendStatus::Ready => HelperReadiness::Ready,
            YouTubeHelperBackendStatus::Blocked => HelperReadiness::Missing {
                code: status.code.unwrap_or_else(|| "HELPER_MISSING".to_string()),
                message: status.message,
            },
        }
    };
    let validate_root = |path: &Path| validate_output_root(path).map_err(|error| error.to_string());
    let free_space = |path: &Path| disk_space::free_space_bytes(path).ok();
    preflight_resume(
        run,
        resume,
        allow_wait,
        &PreflightProbes {
            helper: &helper,
            validate_root: &validate_root,
            free_space: &free_space,
        },
    )
}

// ---------------------------------------------------------------------------
// Admission
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    Started,
    /// Another caller already admitted it, or it is not paused any more.
    NotResumable,
}

/// Attach live control for a paused run and durably admit one resumed
/// attempt, atomically with respect to the supervisor's claim.
pub fn attach_and_begin(
    runtime: &WorkflowRuntime,
    live: &YouTubeLiveHandle,
    run: &RunRecord,
    trigger: ResumeTrigger,
) -> Result<Admission, YouTubeError> {
    let request = serde_json::from_str::<YoutubeDurableRequest>(&run.request_json)
        .map_err(|error| YouTubeError::new("REQUEST_INVALID", error.to_string()))?;
    // Discovery and a download are mutually exclusive; hold the slot until
    // the run is attached, exactly like Start.
    let guard = live
        .begin_discovery(opaque_id("resume"))
        .map_err(YouTubeError::from)?;
    let now = chrono::Utc::now().timestamp();
    let result = runtime.with_drain_lock(|| {
        live.attach_run(
            run.id.clone(),
            request.client_submission_id.clone(),
            request.plan_fingerprint.clone(),
            &request.work_items,
        )
        .map_err(YouTubeError::from)?;
        match runtime.begin_resume(run.id.clone(), trigger, now) {
            Ok(true) => Ok(Admission::Started),
            Ok(false) => {
                live.clear_run(&run.id);
                Ok(Admission::NotResumable)
            }
            Err(error) => {
                live.clear_run(&run.id);
                Err(YouTubeError::new("RESUME_FAILED", error.to_string()))
            }
        }
    });
    drop(guard);
    result
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueYouTubeRunRequest {
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContinueYouTubeRunResponse {
    /// False when nothing was started because the run was already resuming,
    /// running or finished.
    pub started: bool,
}

/// The Continue button. Starts at most one attempt: repeated calls, and calls
/// while an attempt is already running, return `started: false`.
pub fn continue_run(
    runtime: &WorkflowRuntime,
    live: &YouTubeLiveHandle,
    run_id: &str,
) -> Result<ContinueYouTubeRunResponse, YouTubeError> {
    continue_run_with(runtime, live, run_id, &|run, resume| {
        real_preflight(run, resume, false)
    })
}

/// [`continue_run`] with the preflight injected.
pub fn continue_run_with(
    runtime: &WorkflowRuntime,
    live: &YouTubeLiveHandle,
    run_id: &str,
    preflight: &dyn Fn(&RunRecord, &RunResumeRecord) -> Preflight,
) -> Result<ContinueYouTubeRunResponse, YouTubeError> {
    let not_started = Ok(ContinueYouTubeRunResponse { started: false });
    let lookup = |error: WorkflowError| YouTubeError::new("RUN_LOOKUP_FAILED", error.to_string());
    let Some(run) = runtime.get_run(run_id.to_string()).map_err(lookup)? else {
        return Err(YouTubeError::new("RUN_NOT_FOUND", "That download no longer exists."));
    };
    if run.state != RunState::Paused || live.has_active_run(run_id) {
        return not_started;
    }
    let Some(resume) = runtime.get_resume(run_id.to_string()).map_err(lookup)? else {
        return not_started;
    };
    if resume.state == ResumeState::Resuming {
        return not_started;
    }
    let now = chrono::Utc::now().timestamp();
    if let Preflight::Park { code, reason } = preflight(&run, &resume) {
        let _ = runtime.park_for_user(run_id.to_string(), reason.clone(), Some(code.clone()), now);
        return Err(YouTubeError::new(code, reason));
    }
    match attach_and_begin(runtime, live, &run, ResumeTrigger::Manual) {
        Ok(Admission::Started) => Ok(ContinueYouTubeRunResponse { started: true }),
        Ok(Admission::NotResumable) => not_started,
        Err(error) => {
            // A concurrent click or the automatic path may have won; if the
            // run is no longer paused there is nothing left to report.
            let still_paused = runtime
                .get_run(run_id.to_string())
                .ok()
                .flatten()
                .is_some_and(|current| current.state == RunState::Paused);
            if still_paused {
                Err(error)
            } else {
                not_started
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Automatic resume
// ---------------------------------------------------------------------------

/// Resumes interrupted runs once, sequentially, from the existing workflow
/// supervisor loop.
pub struct YoutubeResumeSupervisor {
    live: Arc<YouTubeLiveHandle>,
    /// Tells the UI the resume list changed (`RESUME_EVENT_NAME`). Injected so
    /// this module stays free of the UI runtime.
    notify_changed: Option<ChangeNotifier>,
}

pub type ChangeNotifier = Arc<dyn Fn() + Send + Sync + 'static>;

impl YoutubeResumeSupervisor {
    pub fn new(live: Arc<YouTubeLiveHandle>, notify_changed: Option<ChangeNotifier>) -> Self {
        Self {
            live,
            notify_changed,
        }
    }
}

/// Runs waiting for their one automatic resume, oldest first.
fn auto_pending(runtime: &WorkflowRuntime) -> Result<Vec<(RunRecord, RunResumeRecord)>, WorkflowError> {
    Ok(runtime
        .list_youtube_resumable()?
        .into_iter()
        .filter(|(run, resume)| {
            run.state == RunState::Paused
                && resume.state == ResumeState::AutoPending
                && !resume.auto_resume_used
        })
        .collect())
}

/// One automatic resume step for the oldest pending run.
pub fn auto_resume_next(
    runtime: &WorkflowRuntime,
    live: &YouTubeLiveHandle,
    preflight: &dyn Fn(&RunRecord, &RunResumeRecord) -> Preflight,
) -> Result<AutoResumeStep, WorkflowError> {
    let Some((run, resume)) = auto_pending(runtime)?.into_iter().next() else {
        return Ok(AutoResumeStep::Idle);
    };
    let now = chrono::Utc::now().timestamp();
    match preflight(&run, &resume) {
        Preflight::Wait => Ok(AutoResumeStep::Waiting),
        Preflight::Park { code, reason } => {
            runtime.park_for_user(run.id.clone(), reason, Some(code), now)?;
            Ok(AutoResumeStep::Parked)
        }
        Preflight::Go => match attach_and_begin(runtime, live, &run, ResumeTrigger::Auto) {
            Ok(Admission::Started) => Ok(AutoResumeStep::Started),
            Ok(Admission::NotResumable) => Ok(AutoResumeStep::Parked),
            Err(error) if error.code == "RUNTIME_BUSY" || error.code == "RUN_ALREADY_ACTIVE" => {
                Ok(AutoResumeStep::Waiting)
            }
            Err(error) => {
                runtime.park_for_user(run.id.clone(), error.message, Some(error.code), now)?;
                Ok(AutoResumeStep::Parked)
            }
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AutoResumeStep {
    Idle,
    /// Not ready yet; try again shortly. Nothing was spent.
    Waiting,
    Parked,
    Started,
}

impl SupervisorHook for YoutubeResumeSupervisor {
    fn reconcile(&self, runtime: &WorkflowRuntime, now: i64) -> Result<Option<i64>, WorkflowError> {
        if runtime.is_shutting_down() || auto_pending(runtime)?.is_empty() {
            return Ok(None);
        }
        // A user-started download or a scan owns the slot: wait. Finishing a
        // run wakes the supervisor, so this deadline is only a backstop.
        if self.live.is_busy() || runtime.is_workflow_type_executing("youtube_download") {
            return Ok(Some(now + RETRY_SECS));
        }
        let live = Arc::clone(&self.live);
        let notify = self.notify_changed.clone();
        let task_runtime = runtime.clone();
        // Helper identity hashing and folder probing are blocking: never on
        // the supervisor thread itself.
        let spawned = runtime.spawn_supervisor_task("youtube_resume", move || {
            let step = auto_resume_next(&task_runtime, &live, &|run, resume| {
                real_preflight(run, resume, true)
            });
            if matches!(step, Ok(AutoResumeStep::Started | AutoResumeStep::Parked)) {
                if let Some(notify) = &notify {
                    notify();
                }
            }
        })?;
        let _ = spawned;
        Ok(Some(now + RETRY_SECS))
    }
}

// ---------------------------------------------------------------------------
// UI projection
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeResumableRun {
    pub run_id: String,
    /// `auto_pending`, `needs_user` or `resuming`.
    pub state: String,
    pub reason: Option<String>,
    pub reason_code: Option<String>,
    pub title: String,
    pub video_count: u32,
    pub output_dir: String,
    pub auto_resume_used: bool,
    /// Bytes of partial download a resume keeps.
    pub preserved_bytes: u64,
    pub updated_at: i64,
}

pub fn project_resumable(run: &RunRecord, resume: &RunResumeRecord) -> YouTubeResumableRun {
    let request = serde_json::from_str::<YoutubeDurableRequest>(&run.request_json).ok();
    let title = request
        .as_ref()
        .and_then(|request| request.work_items.first())
        .map(|item| item.title.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "YouTube download".to_string());
    let video_count = request
        .as_ref()
        .map_or(0, |request| request.work_items.len() as u32);
    let preserved_bytes = request.as_ref().map_or(0, |request| {
        request
            .work_items
            .iter()
            .map(|item| {
                present_staging_bytes(
                    Path::new(&run.output_root),
                    &item.occurrence_id,
                    &item.artifact_fingerprint,
                )
            })
            .fold(0u64, u64::saturating_add)
    });
    YouTubeResumableRun {
        run_id: run.id.clone(),
        state: resume.state.as_str().to_string(),
        reason: resume
            .reason
            .clone()
            .or_else(|| run.error_message.clone())
            .map(|reason| reason.chars().take(500).collect()),
        reason_code: resume.reason_code.clone(),
        title,
        video_count,
        output_dir: run.output_root.clone(),
        auto_resume_used: resume.auto_resume_used,
        preserved_bytes,
        updated_at: resume.updated_at,
    }
}

// ---------------------------------------------------------------------------
// Staging cleanup
// ---------------------------------------------------------------------------

/// Remove the resumable staging of every item of a run (cancel, terminal
/// failure). Best effort: an item whose directory is in use is skipped.
pub fn discard_run_staging(root: &ValidatedOutputRoot, items: &[TransientWorkItem]) {
    for item in items {
        let _ = root.discard_resumable_staging(&item.occurrence_id, &item.artifact_fingerprint);
    }
}

/// Discard staging for a run that no attempt owns (a paused run the user
/// cancelled). Blocking; call from `spawn_blocking`.
pub fn discard_paused_run_staging(run: &RunRecord) {
    let Ok(request) = serde_json::from_str::<YoutubeDurableRequest>(&run.request_json) else {
        return;
    };
    let Ok(root) = validate_output_root(Path::new(&run.output_root)) else {
        return;
    };
    discard_run_staging(&root, &request.work_items);
}

/// Startup sweep of abandoned staging under every known output root.
/// Blocking; call from `spawn_blocking` after the restart reconcile.
pub fn sweep_orphans(runtime: &WorkflowRuntime, extra_roots: &[String]) -> usize {
    let Ok(runs) = runtime.list_youtube_runs(1000) else {
        return 0;
    };
    let referenced = referenced_staging_names(&runs);
    let mut roots: BTreeMap<String, String> = BTreeMap::new();
    for root in runs
        .iter()
        .map(|run| run.output_root.as_str())
        .chain(extra_roots.iter().map(String::as_str))
    {
        if !root.trim().is_empty() {
            roots
                .entry(normalize_root(root))
                .or_insert_with(|| root.trim().to_string());
        }
    }
    let now = SystemTime::now();
    let mut removed = 0;
    for (key, root_path) in roots {
        let Ok(root) = validate_output_root(Path::new(&root_path)) else {
            continue;
        };
        let keep = referenced.get(&key).cloned().unwrap_or_default();
        if let Ok(report) = root.sweep_orphan_staging(&keep, now, ORPHAN_MIN_AGE) {
            removed += report.removed.len();
        }
    }
    removed
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod live_e2e;
