use crate::app::managed_process::{
    helper_identity, DiscoveryOperation, HelperIdentity, ManagedProcessError, TransientRunSnapshot, TransientWorkItem,
    MAX_SELECTED_ITEMS,
};
use crate::app::safe_output_filesystem::validate_output_root;
use crate::cache::{get_setting, open_runtime, upsert_setting_json};
use crate::providers::youtube::error::{YouTubeError, YouTubeInternalError};
use crate::providers::youtube::executor::{YouTubeExecutorContext, YouTubeExecutorItemContext};
use crate::providers::youtube::helper::helper_kind;
use crate::providers::youtube::live::{DiscoveryGuard, YouTubeLiveHandle, YoutubeDurableRequest};
use crate::providers::youtube::manifest_contract::{
    artifact_fingerprint, select_transcript, validate_helper_lock_digest, ArtifactFingerprintInput,
    ManifestContractError, SelectedTranscript, FORMAT_POLICY_VERSION,
};
use crate::providers::youtube::models::{
    CancelYouTubeRunRequest, GetYouTubeDownloadStateRequest, GetYouTubeDownloadStateResponse,
    GetYouTubeHelperStatusResponse, InspectYouTubeTranscriptsRequest,
    InspectYouTubeTranscriptsResponse, ListYouTubeHistoryRequest, MutateYouTubeRunRequest,
    OpenYouTubeDownloadFolderRequest, OpenYouTubeDownloadFolderResponse, SavedYouTubePreferences,
    ScanYouTubeSourceRequest, ScanYouTubeSourceResponse, StartYouTubeDownloadRequest,
    StartYouTubeDownloadResponse, YouTubeAvailability, YouTubeHelperBackendStatus,
    YouTubeHistoryEntry, YouTubeStartReceipt, YouTubeStartReceiptState, YouTubeTranscriptOccurrence,
};
use crate::shell::open_folder_in_explorer;
use crate::providers::youtube::scan::{
    revalidate_selected_source, scan_source, selected_items_from_plan, YouTubeScanPlan,
};
use crate::providers::youtube::scan::PlannedYouTubeItem;
use crate::providers::youtube::transcript_inspection::{
    inspect_transcripts, into_plan_inspection, stored_inspection, TranscriptInspectionResult,
};
use crate::workflow::WorkflowRuntime;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{Manager, State};

const YOUTUBE_PREFERENCES_KEY: &str = "youtube.preferences";

/// Owns the shared SQLite path for YouTube preferences (settings key).
pub struct YouTubeState {
    db_path: PathBuf,
}

impl YouTubeState {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn connection(&self) -> Result<Connection, YouTubeError> {
        open_runtime(&self.db_path)
            .map_err(|error| YouTubeError::new("PREFERENCES_UNAVAILABLE", error.to_string()))
    }
}

#[derive(Default)]
pub struct YouTubePlanStore {
    plans: Mutex<PlanCache>,
}

const MAX_SCAN_PLANS: usize = 8;

#[derive(Default)]
struct PlanCache {
    plans: HashMap<String, YouTubeScanPlan>,
    order: VecDeque<String>,
}

impl PlanCache {
    fn insert(&mut self, plan: YouTubeScanPlan) {
        let key = plan.response.scan_plan_id.clone();
        if self.plans.contains_key(&key) {
            self.order.retain(|entry| entry != &key);
        }
        while self.plans.len() >= MAX_SCAN_PLANS {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            self.plans.remove(&evicted);
        }
        self.order.push_back(key.clone());
        self.plans.insert(key, plan);
    }

    fn get(&mut self, scan_plan_id: &str) -> Option<YouTubeScanPlan> {
        let now = Instant::now();
        self.plans.retain(|_, plan| plan.expires_at > now);
        self.order.retain(|key| self.plans.contains_key(key));
        self.plans.get(scan_plan_id).cloned()
    }

    fn apply_transcript_inspections(
        &mut self,
        scan_plan_id: &str,
        inspections: Vec<crate::providers::youtube::models::PlannedYouTubeTranscriptInspection>,
    ) -> Result<(), YouTubeInternalError> {
        let plan = self
            .plans
            .get_mut(scan_plan_id)
            .ok_or(YouTubeInternalError::PlanNotFound)?;
        if plan.expires_at <= Instant::now() {
            return Err(YouTubeInternalError::PlanExpired);
        }
        for inspection in inspections {
            let item = plan
                .items
                .iter_mut()
                .find(|item| item.public.occurrence_id == inspection.context.occurrence_id)
                .ok_or(YouTubeInternalError::UnknownOccurrence)?;
            if inspection.context.source_snapshot_digest != plan.source_snapshot_digest
                || inspection.context.video_id != item.public.video_id
                || inspection.context.metadata_digest != item.public.metadata_digest
            {
                return Err(YouTubeInternalError::InvalidRequest(
                    "transcript inspection no longer matches the immutable scan plan".to_string(),
                ));
            }
            item.transcript_inspection = Some(inspection);
        }
        Ok(())
    }
}

const HELPER_MISSING_MESSAGE: &str =
    "YouTube needs a one-time signed helper pack (~220 MB). It installs inside LinkVaultData and is not bundled in the app installer.";
const HELPER_INTEGRITY_MESSAGE: &str =
    "Installed YouTube helpers failed verification. Download YouTube helpers again to repair them.";

/// Maps the start-path helper identity to UI status. `install_present` is only
/// consulted on failure, to tell "never installed" from "installed but broken".
/// Internal embedded-lock builds with a corrupted sidecar report HELPER_MISSING
/// (no managed install); Download YouTube helpers still repairs it because a
/// managed install is resolved first.
fn helper_status_response(
    identity: Result<HelperIdentity, ManagedProcessError>,
    install_present: impl FnOnce() -> bool,
) -> GetYouTubeHelperStatusResponse {
    let blocked = |code: &str, message: &str| GetYouTubeHelperStatusResponse {
        status: YouTubeHelperBackendStatus::Blocked,
        code: Some(code.to_string()),
        message: message.to_string(),
    };
    match identity {
        Ok(identity) => match validate_helper_lock_digest(&identity.digest) {
            Ok(()) => GetYouTubeHelperStatusResponse {
                status: YouTubeHelperBackendStatus::Ready,
                code: None,
                message: "YouTube helper integrity validation passed.".to_string(),
            },
            Err(_) => blocked("HELPER_INTEGRITY_FAILED", HELPER_INTEGRITY_MESSAGE),
        },
        Err(_) if install_present() => blocked("HELPER_INTEGRITY_FAILED", HELPER_INTEGRITY_MESSAGE),
        Err(_) => blocked("HELPER_MISSING", HELPER_MISSING_MESSAGE),
    }
}

/// Start-path counterpart of `helper_status_response`: the same missing-vs-broken
/// split, so Start does not tell a never-installed user their helpers "failed
/// integrity validation". Integrity failures carry the bare reason, exactly as
/// scan, inspection and download report them via `from_helper_run`.
/// Only `Integrity` errors get the missing-vs-broken split. Anything else (for
/// example `UnsupportedPlatform`) stays `HELPER_FAILED` so the UI does not offer a
/// helper reinstall that cannot help.
fn helper_start_error(
    error: ManagedProcessError,
    install_present: impl FnOnce() -> bool,
) -> YouTubeError {
    if matches!(error, ManagedProcessError::Integrity(_)) && !install_present() {
        return YouTubeError::new("HELPER_MISSING", HELPER_MISSING_MESSAGE);
    }
    YouTubeError::from(YouTubeInternalError::from_helper_run(error))
}

#[tauri::command]
pub async fn get_youtube_helper_status() -> Result<GetYouTubeHelperStatusResponse, String> {
    // Hashes ~230 MB of helpers; never run on the Tauri main thread.
    tauri::async_runtime::spawn_blocking(|| {
        helper_status_response(
            helper_identity(helper_kind()),
            crate::app::media_toolchain::managed_install_present,
        )
    })
    .await
    .map_err(|error| format!("YouTube helper status task failed: {error}"))
}

#[tauri::command]
pub async fn get_media_toolchain_status(
) -> Result<crate::app::media_toolchain::MediaToolchainStatus, String> {
    tauri::async_runtime::spawn_blocking(crate::app::media_toolchain::status)
        .await
        .map_err(|error| format!("Media toolchain status task failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn install_media_toolchain(
) -> Result<crate::app::media_toolchain::MediaToolchainStatus, String> {
    tauri::async_runtime::spawn_blocking(crate::app::media_toolchain::install_latest)
        .await
        .map_err(|error| format!("Media toolchain install task failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_youtube_preferences(
    state: State<'_, YouTubeState>,
) -> Result<SavedYouTubePreferences, YouTubeError> {
    let connection = state.connection()?;
    load_youtube_preferences(&connection)
}

#[tauri::command]
pub fn save_youtube_preferences(
    state: State<'_, YouTubeState>,
    preferences: SavedYouTubePreferences,
) -> Result<SavedYouTubePreferences, YouTubeError> {
    let connection = state.connection()?;
    persist_youtube_preferences(&connection, preferences, now_unix_timestamp())
}

/// Runs blocking YouTube command work (yt-dlp, helper hashing, SQLite) off the
/// Tauri main thread. A panicked or cancelled task surfaces as a public error.
async fn run_youtube_blocking<T, F>(task: F) -> Result<T, YouTubeError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, YouTubeError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| {
            YouTubeError::new(
                "TASK_FAILED",
                format!("YouTube background task failed: {error}"),
            )
        })?
}

fn runtime_state_unavailable() -> YouTubeError {
    YouTubeError::new("TASK_FAILED", "YouTube runtime state is not available")
}

#[tauri::command]
pub async fn scan_youtube_source(
    app: tauri::AppHandle,
    request: ScanYouTubeSourceRequest,
) -> Result<ScanYouTubeSourceResponse, YouTubeError> {
    run_youtube_blocking(move || {
        let (Some(live), Some(state)) = (
            app.try_state::<Arc<YouTubeLiveHandle>>(),
            app.try_state::<YouTubePlanStore>(),
        ) else {
            return Err(runtime_state_unavailable());
        };
        scan_youtube_source_blocking(&live, &state, request)
    })
    .await
}

fn scan_youtube_source_blocking(
    live: &YouTubeLiveHandle,
    state: &YouTubePlanStore,
    request: ScanYouTubeSourceRequest,
) -> Result<ScanYouTubeSourceResponse, YouTubeError> {
    validate_operation_id(&request.client_operation_id)?;
    let guard = live
        .begin_discovery(request.client_operation_id.clone())
        .map_err(YouTubeError::from)?;
    let plan = scan_source(&request, &guard.operation).map_err(YouTubeError::from)?;
    let response = plan.response.clone();
    state
        .plans
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(plan);
    Ok(response)
}

#[tauri::command]
pub async fn inspect_youtube_transcripts(
    app: tauri::AppHandle,
    request: InspectYouTubeTranscriptsRequest,
) -> Result<InspectYouTubeTranscriptsResponse, YouTubeError> {
    run_youtube_blocking(move || {
        let (Some(live), Some(state)) = (
            app.try_state::<Arc<YouTubeLiveHandle>>(),
            app.try_state::<YouTubePlanStore>(),
        ) else {
            return Err(runtime_state_unavailable());
        };
        inspect_youtube_transcripts_blocking(&live, &state, request)
    })
    .await
}

fn inspect_youtube_transcripts_blocking(
    live: &YouTubeLiveHandle,
    state: &YouTubePlanStore,
    request: InspectYouTubeTranscriptsRequest,
) -> Result<InspectYouTubeTranscriptsResponse, YouTubeError> {
    inspect_youtube_transcripts_with(live, state, request, &inspect_transcripts)
}

/// Launches the helper for one occurrence that has no stored inspection.
type InspectionLauncher<'a> = &'a dyn Fn(
    &PlannedYouTubeItem,
    &DiscoveryOperation,
) -> Result<TranscriptInspectionResult, YouTubeInternalError>;

/// Answers from the plan's stored inspection data (filled at scan time for a
/// video, or by an earlier inspection) and launches the helper only for
/// occurrences without it, e.g. flat playlist entries.  The discovery slot is
/// taken only when a launch is actually needed, so a fully stored answer is
/// instant and cannot collide with other discovery work or a running download.
fn inspect_youtube_transcripts_with(
    live: &YouTubeLiveHandle,
    state: &YouTubePlanStore,
    request: InspectYouTubeTranscriptsRequest,
    launch: InspectionLauncher<'_>,
) -> Result<InspectYouTubeTranscriptsResponse, YouTubeError> {
    validate_operation_id(&request.client_operation_id)?;
    let plan = get_plan(state, &request.scan_plan_id)?;
    let mut seen = std::collections::HashSet::new();
    let mut targets: Vec<(&PlannedYouTubeItem, Option<TranscriptInspectionResult>)> = Vec::new();
    for occurrence_id in &request.occurrence_ids {
        if !seen.insert(occurrence_id.clone()) {
            return Err(YouTubeError::from(
                YouTubeInternalError::DuplicateOccurrence,
            ));
        }
        let item = plan
            .items
            .iter()
            .find(|item| &item.public.occurrence_id == occurrence_id)
            .ok_or_else(|| YouTubeError::from(YouTubeInternalError::UnknownOccurrence))?;
        if item.public.availability != YouTubeAvailability::Available {
            return Err(YouTubeError::from(YouTubeInternalError::ScanPlanStale));
        }
        targets.push((item, stored_inspection(item, &plan.source_snapshot_digest)));
    }
    let guard = if targets.iter().any(|(_, stored)| stored.is_none()) {
        Some(
            live.begin_discovery(request.client_operation_id.clone())
                .map_err(YouTubeError::from)?,
        )
    } else {
        None
    };
    let mut occurrences = Vec::new();
    let mut new_inspections = Vec::new();
    for (item, stored) in targets {
        let inspected = match (stored, guard.as_ref()) {
            (Some(stored), _) => stored,
            (None, Some(guard)) => {
                if guard.operation.cancellation_requested() {
                    return Err(YouTubeError::new(
                        "DISCOVERY_CANCELLED",
                        "transcript inspection was cancelled",
                    ));
                }
                let inspected = launch(item, &guard.operation).map_err(YouTubeError::from)?;
                new_inspections.push(into_plan_inspection(
                    inspected.clone(),
                    &plan.source_snapshot_digest,
                    &item.public.metadata_digest,
                ));
                inspected
            }
            (None, None) => return Err(runtime_state_unavailable()),
        };
        occurrences.push(YouTubeTranscriptOccurrence {
            occurrence_id: inspected.occurrence_id,
            video_id: inspected.video_id,
            tracks: inspected.tracks,
        });
    }
    if !new_inspections.is_empty() {
        state
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .apply_transcript_inspections(&request.scan_plan_id, new_inspections)
            .map_err(YouTubeError::from)?;
    }
    Ok(InspectYouTubeTranscriptsResponse { occurrences })
}

#[tauri::command]
pub fn cancel_youtube_discovery(
    live: State<'_, Arc<YouTubeLiveHandle>>,
    _state: State<'_, YouTubePlanStore>,
    request: CancelYouTubeDiscoveryRequest,
) -> Result<(), YouTubeError> {
    validate_operation_id(&request.client_operation_id)?;
    live.cancel_discovery(&request.client_operation_id)
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub async fn start_youtube_download(
    app: tauri::AppHandle,
    request: StartYouTubeDownloadRequest,
) -> Result<StartYouTubeDownloadResponse, YouTubeError> {
    // Source revalidation (yt-dlp) and the ~230 MB helper identity hash must not
    // run on the Tauri main thread. Only this preparation moves; the run is still
    // admitted and drained exactly as before.
    run_youtube_blocking(move || {
        let (Some(live), Some(state), Some(runtime)) = (
            app.try_state::<Arc<YouTubeLiveHandle>>(),
            app.try_state::<YouTubePlanStore>(),
            app.try_state::<WorkflowRuntime>(),
        ) else {
            return Err(runtime_state_unavailable());
        };
        start_youtube_download_blocking(&live, &state, &runtime, request)
    })
    .await
}

/// Re-runs discovery for the selected source on a launch.
type SourceRevalidator<'a> = &'a dyn Fn(
    &YouTubeScanPlan,
    &[String],
    &DiscoveryOperation,
) -> Result<Vec<PlannedYouTubeItem>, YouTubeInternalError>;

/// Resolves the items a download starts from.  A plan scanned within
/// `START_REVALIDATION_FRESH_FOR` is trusted: its own items are used and no
/// discovery process or discovery slot is needed.  An older plan is re-scanned
/// first (holding the discovery slot until the guard is dropped), so a video
/// that changed or disappeared still fails the start with a visible error.
fn selected_items_for_start<'a>(
    live: &'a YouTubeLiveHandle,
    plan: &YouTubeScanPlan,
    selected_occurrence_ids: &[String],
    now: Instant,
    revalidate: SourceRevalidator<'_>,
) -> Result<(Vec<PlannedYouTubeItem>, Option<DiscoveryGuard<'a>>), YouTubeError> {
    if plan.is_fresh_for_start(now) {
        let items =
            selected_items_from_plan(plan, selected_occurrence_ids).map_err(YouTubeError::from)?;
        return Ok((items, None));
    }
    let guard = live
        .begin_discovery(opaque_id("revalidation"))
        .map_err(YouTubeError::from)?;
    let items = revalidate(plan, selected_occurrence_ids, &guard.operation)
        .map_err(YouTubeError::from)?;
    Ok((items, Some(guard)))
}

fn start_youtube_download_blocking(
    live: &YouTubeLiveHandle,
    state: &YouTubePlanStore,
    runtime: &WorkflowRuntime,
    request: StartYouTubeDownloadRequest,
) -> Result<StartYouTubeDownloadResponse, YouTubeError> {
    validate_submission_id(&request.client_submission_id)?;
    let submission_fp = submission_fingerprint(&request)?;
    if let Some(receipt) = live
        .lookup_submission(&request.client_submission_id, &submission_fp)
        .map_err(YouTubeError::from)?
    {
        return Ok(StartYouTubeDownloadResponse {
            receipt,
            replayed: true,
        });
    }
    live.ensure_submission_capacity(&request.client_submission_id)
        .map_err(YouTubeError::from)?;
    let plan = get_plan(state, &request.scan_plan_id)?;
    if plan.expires_at <= Instant::now() {
        return Err(YouTubeError::from(YouTubeInternalError::PlanExpired));
    }
    if request.selected_occurrence_ids.is_empty() {
        return Err(YouTubeError::from(YouTubeInternalError::EmptySelection));
    }
    if request.selected_occurrence_ids.len() > MAX_SELECTED_ITEMS {
        return Err(YouTubeError::from(YouTubeInternalError::TooManySelected));
    }
    let mut selected = std::collections::HashSet::new();
    for occurrence_id in &request.selected_occurrence_ids {
        if !selected.insert(occurrence_id) {
            return Err(YouTubeError::from(
                YouTubeInternalError::DuplicateOccurrence,
            ));
        }
    }
    let planned_selected_items = plan
        .items
        .iter()
        .filter(|item| selected.contains(&item.public.occurrence_id))
        .collect::<Vec<_>>();
    if planned_selected_items.len() != selected.len() {
        return Err(YouTubeError::from(YouTubeInternalError::UnknownOccurrence));
    }
    if planned_selected_items
        .iter()
        .any(|item| item.public.availability != YouTubeAvailability::Available)
    {
        return Err(YouTubeError::from(YouTubeInternalError::ScanPlanStale));
    }
    let (current_selected_items, revalidation) = selected_items_for_start(
        live,
        &plan,
        &request.selected_occurrence_ids,
        Instant::now(),
        &revalidate_selected_source,
    )?;
    let _output_root = validate_output_root(PathBuf::from(&request.output_dir).as_path())?;
    let helper = helper_identity(helper_kind()).map_err(|error| {
        helper_start_error(error, crate::app::media_toolchain::managed_install_present)
    })?;
    let plan_fingerprint = fingerprint(&request, &plan, &helper.digest)?;
    let run_id = opaque_id("run");
    let selected_items = current_selected_items
        .into_iter()
        .map(|current_item| {
            let planned_item = planned_selected_items
                .iter()
                .find(|item| item.public.occurrence_id == current_item.public.occurrence_id)
                .ok_or(YouTubeInternalError::ScanPlanStale)?;
            let selected_transcript = selected_transcript(planned_item, &request)
                .map_err(YouTubeInternalError::Public)?;
            Ok((current_item, selected_transcript))
        })
        .collect::<Result<Vec<_>, YouTubeInternalError>>()
        .map_err(YouTubeError::from)?;
    let work_items = selected_items
        .iter()
        .map(|(item, selected_transcript)| {
            Ok(TransientWorkItem {
                occurrence_id: item.public.occurrence_id.clone(),
                artifact_fingerprint: fingerprint_item(
                    item,
                    &request,
                    selected_transcript.clone(),
                    &helper.digest,
                )?,
                video_id: item.public.video_id.clone(),
                ordinal: item.public.ordinal,
                title: item.public.title.clone(),
                source_url: item.source_url.clone(),
            })
        })
        .collect::<Result<Vec<_>, YouTubeError>>()?;
    let context = YouTubeExecutorContext {
        source_snapshot_digest: plan.source_snapshot_digest.clone(),
        playlist_id: plan.response.playlist_id.clone(),
        helper_lock_digest: helper.digest.clone(),
        items: selected_items
            .iter()
            .map(|(item, selected_transcript)| YouTubeExecutorItemContext {
                occurrence_id: item.public.occurrence_id.clone(),
                playlist_index: plan
                    .response
                    .playlist_id
                    .as_ref()
                    .map(|_| item.public.ordinal),
                source_duration_seconds: item.public.duration_seconds,
                selected_transcript: selected_transcript.clone(),
            })
            .collect(),
    };
    drop(revalidation);
    let durable = YoutubeDurableRequest {
        schema_version: 1,
        start: request.clone(),
        work_items: work_items.clone(),
        context,
        plan_fingerprint: plan_fingerprint.clone(),
        client_submission_id: request.client_submission_id.clone(),
    };
    let request_json = serde_json::to_string(&durable)
        .map_err(|error| YouTubeError::new("MANIFEST_CONTRACT_INVALID", error.to_string()))?;
    let now = chrono::Utc::now().timestamp();
    let video_id = work_items
        .first()
        .map(|item| item.video_id.clone())
        .unwrap_or_else(|| run_id.clone());
    let output_dir = request.output_dir.clone();
    runtime.with_drain_lock(|| {
        live.attach_run(
            run_id.clone(),
            request.client_submission_id.clone(),
            plan_fingerprint.clone(),
            &work_items,
        )
        .map_err(YouTubeError::from)?;
        match runtime.submit_youtube_download(
            run_id.clone(),
            video_id.clone(),
            request_json.clone(),
            output_dir.clone(),
            now,
        ) {
            Ok(_) => Ok(()),
            Err(error) => {
                live.clear_run(&run_id);
                Err(YouTubeError::new("SUBMIT_FAILED", error.to_string()))
            }
        }
    })?;
    let receipt = YouTubeStartReceipt {
        client_submission_id: request.client_submission_id.clone(),
        run_id: run_id.clone(),
        revision: 1,
        scan_plan_id: request.scan_plan_id.clone(),
        plan_fingerprint: plan_fingerprint.clone(),
        state: YouTubeStartReceiptState::Running,
    };
    if let Err(error) = live.record_submission(
        request.client_submission_id.clone(),
        submission_fp,
        receipt.clone(),
    ) {
        // Durable admit already succeeded under drain_lock; do not clear the live
        // slot (that would recreate the orphan-control race). Capacity is checked
        // before admit; surface residual ledger errors to the client.
        return Err(YouTubeError::from(error));
    }
    let runtime = runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = runtime.drain_type("youtube_download");
    });
    Ok(StartYouTubeDownloadResponse {
        receipt,
        replayed: false,
    })
}

#[tauri::command]
pub fn get_youtube_download_state(
    live: State<'_, Arc<YouTubeLiveHandle>>,
    state: State<'_, YouTubePlanStore>,
    request: GetYouTubeDownloadStateRequest,
) -> Result<GetYouTubeDownloadStateResponse, YouTubeError> {
    let _ = state;
    live.snapshot(request.run_id.as_deref())
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub fn pause_youtube_download(
    live: State<'_, Arc<YouTubeLiveHandle>>,
    state: State<'_, YouTubePlanStore>,
    request: MutateYouTubeRunRequest,
) -> Result<TransientRunSnapshot, YouTubeError> {
    let _ = state;
    live.pause(&request.run_id, request.expected_revision)
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub fn resume_youtube_download(
    live: State<'_, Arc<YouTubeLiveHandle>>,
    state: State<'_, YouTubePlanStore>,
    request: MutateYouTubeRunRequest,
) -> Result<TransientRunSnapshot, YouTubeError> {
    let _ = state;
    live.resume(&request.run_id, request.expected_revision)
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub fn cancel_youtube_download(
    live: State<'_, Arc<YouTubeLiveHandle>>,
    state: State<'_, YouTubePlanStore>,
    runtime: State<'_, WorkflowRuntime>,
    request: CancelYouTubeRunRequest,
) -> Result<TransientRunSnapshot, YouTubeError> {
    let _ = state;
    let snapshot = live.cancel(&request.run_id).map_err(YouTubeError::from)?;
    runtime
        .cancel_run(request.run_id, chrono::Utc::now().timestamp())
        .map_err(|error| YouTubeError::new("CANCEL_FAILED", error.to_string()))?;
    Ok(snapshot)
}

#[tauri::command]
pub fn open_youtube_download_folder(
    runtime: State<'_, WorkflowRuntime>,
    request: OpenYouTubeDownloadFolderRequest,
) -> Result<OpenYouTubeDownloadFolderResponse, YouTubeError> {
    // Item outcomes do not yet publish absolute/relative media paths for reveal.
    // Prefer the durable run output_root; otherwise open the client fallback
    // outputDir (usually the Start request / preference folder).
    let _ = request.occurrence_id;
    let mut candidate = request.output_dir.trim().to_string();
    if let Some(run_id) = request
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if let Some(run) = runtime
            .get_run(run_id.to_string())
            .map_err(|error| YouTubeError::new("RUN_LOOKUP_FAILED", error.to_string()))?
        {
            let root = run.output_root.trim();
            if !root.is_empty() {
                candidate = root.to_string();
            }
        }
    }
    if candidate.is_empty() {
        return Err(YouTubeError::new(
            "OUTPUT_DIR_REQUIRED",
            "Choose a download folder before opening files.",
        ));
    }
    let validated = validate_output_root(Path::new(&candidate))?;
    open_folder_in_explorer(validated.path()).map_err(|error| {
        YouTubeError::new("OPEN_FOLDER_FAILED", error)
    })?;
    Ok(OpenYouTubeDownloadFolderResponse {
        path: validated.path().to_string_lossy().into_owned(),
    })
}

const YOUTUBE_HISTORY_DEFAULT_LIMIT: u32 = 100;
const YOUTUBE_HISTORY_MAX_LIMIT: u32 = 500;
const YOUTUBE_HISTORY_ERROR_MAX_CHARS: usize = 500;

fn youtube_history_limit(request: &ListYouTubeHistoryRequest) -> i64 {
    let limit = request
        .limit
        .unwrap_or(YOUTUBE_HISTORY_DEFAULT_LIMIT)
        .clamp(1, YOUTUBE_HISTORY_MAX_LIMIT);
    i64::from(limit)
}

fn bound_youtube_history_error(message: Option<&str>) -> Option<String> {
    let trimmed = message?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let truncated: String = trimmed.chars().take(YOUTUBE_HISTORY_ERROR_MAX_CHARS).collect();
    Some(truncated)
}

fn project_youtube_history_entry(
    run: &crate::workflow::domain::types::RunRecord,
) -> YouTubeHistoryEntry {
    let (title, source_url, video_count) =
        match serde_json::from_str::<YoutubeDurableRequest>(&run.request_json) {
            Ok(request) => {
                let first = request.work_items.first();
                let title = first
                    .map(|item| item.title.trim())
                    .filter(|title| !title.is_empty())
                    .unwrap_or("YouTube download")
                    .to_string();
                let source_url = first
                    .map(|item| item.source_url.clone())
                    .unwrap_or_default();
                let video_count = request.work_items.len() as u32;
                (title, source_url, video_count)
            }
            Err(_) => ("YouTube download".to_string(), String::new(), 0),
        };
    YouTubeHistoryEntry {
        run_id: run.id.clone(),
        state: run.state.as_str().to_string(),
        title,
        source_url,
        video_count,
        output_dir: run.output_root.clone(),
        created_at: run.created_at,
        completed_at: run.completed_at,
        error_message: bound_youtube_history_error(run.error_message.as_deref()),
    }
}

#[tauri::command]
pub fn list_youtube_history(
    runtime: State<'_, WorkflowRuntime>,
    request: ListYouTubeHistoryRequest,
) -> Result<Vec<YouTubeHistoryEntry>, YouTubeError> {
    let limit = youtube_history_limit(&request);
    let runs = runtime
        .list_youtube_runs(limit)
        .map_err(|error| YouTubeError::new("HISTORY_UNAVAILABLE", error.to_string()))?;
    Ok(runs
        .into_iter()
        .filter(|run| run.state.is_terminal())
        .map(|run| project_youtube_history_entry(&run))
        .collect())
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelYouTubeDiscoveryRequest {
    pub client_operation_id: String,
}

fn get_plan(state: &YouTubePlanStore, scan_plan_id: &str) -> Result<YouTubeScanPlan, YouTubeError> {
    let plan = state
        .plans
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(scan_plan_id)
        .ok_or_else(|| YouTubeError::from(YouTubeInternalError::PlanNotFound))?;
    if plan.expires_at <= Instant::now() {
        return Err(YouTubeError::from(YouTubeInternalError::PlanExpired));
    }
    Ok(plan)
}

fn validate_operation_id(id: &str) -> Result<(), YouTubeError> {
    if id.len() < 8
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(YouTubeError::new(
            "INVALID_OPERATION_ID",
            "operation id is malformed",
        ));
    }
    Ok(())
}

fn validate_submission_id(id: &str) -> Result<(), YouTubeError> {
    validate_operation_id(id)
        .map_err(|_| YouTubeError::new("INVALID_SUBMISSION_ID", "submission id is malformed"))
}

fn submission_fingerprint(request: &StartYouTubeDownloadRequest) -> Result<String, YouTubeError> {
    let encoded = serde_json::to_vec(&(
        &request.scan_plan_id,
        &request.selected_occurrence_ids,
        &request.output_dir,
        &request.mode,
        request.max_height,
        &request.preferred_language,
        &request.fallback_languages,
        request.allow_automatic_captions,
        request.continue_without_transcript,
    ))
    .map_err(|error| YouTubeError::new("MANIFEST_CONTRACT_INVALID", error.to_string()))?;
    Ok(digest_bytes(&encoded))
}

fn fingerprint(
    request: &StartYouTubeDownloadRequest,
    plan: &YouTubeScanPlan,
    helper_digest: &str,
) -> Result<String, YouTubeError> {
    let encoded = serde_json::to_vec(&(
        &plan.source_snapshot_digest,
        &request.selected_occurrence_ids,
        &request.mode,
        FORMAT_POLICY_VERSION,
        effective_max_height(request),
        &request.preferred_language,
        &request.fallback_languages,
        request.allow_automatic_captions,
        request.continue_without_transcript,
        helper_digest,
    ))
    .map_err(|error| YouTubeError::new("MANIFEST_CONTRACT_INVALID", error.to_string()))?;
    Ok(digest_bytes(&encoded))
}

fn fingerprint_item(
    item: &crate::providers::youtube::scan::PlannedYouTubeItem,
    request: &StartYouTubeDownloadRequest,
    selected_transcript: Option<SelectedTranscript>,
    helper_digest: &str,
) -> Result<String, YouTubeError> {
    artifact_fingerprint(&ArtifactFingerprintInput {
        occurrence_id: item.public.occurrence_id.clone(),
        video_id: item.public.video_id.clone(),
        mode: request.mode.clone(),
        format_policy_version: FORMAT_POLICY_VERSION,
        max_height: effective_max_height(request),
        selected_transcript,
        helper_lock_digest: helper_digest.to_string(),
    })
    .map_err(manifest_contract_error)
}

fn selected_transcript(
    item: &crate::providers::youtube::scan::PlannedYouTubeItem,
    request: &StartYouTubeDownloadRequest,
) -> Result<Option<SelectedTranscript>, YouTubeError> {
    if matches!(
        request.mode,
        crate::providers::youtube::models::YouTubeDownloadMode::VideoOnly
    ) {
        return Ok(None);
    }
    let inspection = item.transcript_inspection.as_ref().ok_or_else(|| {
        YouTubeError::new(
            "TRANSCRIPT_INSPECTION_REQUIRED",
            "inspect transcripts for every selected item before starting the download",
        )
    })?;
    if inspection.context.occurrence_id != item.public.occurrence_id
        || inspection.context.video_id != item.public.video_id
        || inspection.context.metadata_digest != item.public.metadata_digest
    {
        return Err(YouTubeError::new(
            "TRANSCRIPT_INSPECTION_STALE",
            "transcript inspection no longer matches the selected item",
        ));
    }
    let excluded = ["live_chat".to_string()];
    let selected = select_transcript(
        &inspection.tracks,
        request.preferred_language.as_deref(),
        &request.fallback_languages,
        request.allow_automatic_captions,
        &excluded,
    );
    if selected.is_none()
        && (!request.continue_without_transcript
            || matches!(
                request.mode,
                crate::providers::youtube::models::YouTubeDownloadMode::TranscriptOnly
            ))
    {
        return Err(YouTubeError::new(
            "TRANSCRIPT_NOT_AVAILABLE",
            "no inspected transcript matches the requested language policy",
        ));
    }
    Ok(selected)
}

fn effective_max_height(request: &StartYouTubeDownloadRequest) -> Option<u16> {
    match &request.mode {
        crate::providers::youtube::models::YouTubeDownloadMode::TranscriptOnly => None,
        _ => request.max_height,
    }
}

fn manifest_contract_error(error: ManifestContractError) -> YouTubeError {
    YouTubeError::new("MANIFEST_CONTRACT_INVALID", error.to_string())
}

fn digest(input: &str) -> String {
    digest_bytes(input.as_bytes())
}

fn digest_bytes(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    format!("{:x}", hasher.finalize())
}

fn opaque_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    digest(&format!("{prefix}|{}|{now}|{sequence}", std::process::id()))
}

fn now_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn load_youtube_preferences(
    connection: &Connection,
) -> Result<SavedYouTubePreferences, YouTubeError> {
    match get_setting(connection, YOUTUBE_PREFERENCES_KEY)
        .map_err(|error| YouTubeError::new("PREFERENCES_UNAVAILABLE", error.to_string()))?
    {
        Some(record) => serde_json::from_str(&record.value_json).map_err(|error| {
            YouTubeError::new(
                "PREFERENCES_CORRUPT",
                format!("youtube.preferences could not be parsed: {error}"),
            )
        }),
        None => Ok(SavedYouTubePreferences::default()),
    }
}

fn persist_youtube_preferences(
    connection: &Connection,
    preferences: SavedYouTubePreferences,
    updated_at: i64,
) -> Result<SavedYouTubePreferences, YouTubeError> {
    let trimmed = preferences.output_dir.trim();
    if trimmed.is_empty() {
        return Err(YouTubeError::new(
            "OUTPUT_DIR_REQUIRED",
            "Choose a download folder before saving YouTube preferences.",
        ));
    }
    let validated = validate_output_root(Path::new(trimmed))?;
    let saved = SavedYouTubePreferences {
        output_dir: validated.path().to_string_lossy().into_owned(),
    };
    let settings_json = serde_json::to_string(&saved)
        .map_err(|error| YouTubeError::new("PREFERENCES_UNAVAILABLE", error.to_string()))?;
    upsert_setting_json(
        connection,
        YOUTUBE_PREFERENCES_KEY,
        &settings_json,
        updated_at,
    )
    .map_err(|error| YouTubeError::new("PREFERENCES_UNAVAILABLE", error.to_string()))?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::initialize_database;
    use tempfile::tempdir;

    fn preferences_harness() -> (tempfile::TempDir, Connection) {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        (directory, connection)
    }

    #[test]
    fn youtube_preferences_round_trip_through_settings_key() {
        let (directory, connection) = preferences_harness();
        let output = directory.path().join("youtube-out");
        std::fs::create_dir_all(&output).unwrap();

        let saved = persist_youtube_preferences(
            &connection,
            SavedYouTubePreferences {
                output_dir: output.to_string_lossy().into_owned(),
            },
            1_700_000_000,
        )
        .unwrap();

        let loaded = load_youtube_preferences(&connection).unwrap();
        assert_eq!(loaded, saved);
        assert_eq!(loaded.output_dir, output.to_string_lossy());

        let record = get_setting(&connection, YOUTUBE_PREFERENCES_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(record.key, YOUTUBE_PREFERENCES_KEY);
        assert!(record.value_json.contains("\"output_dir\""));
        assert!(!record.value_json.contains("\"outputDir\""));
        assert!(!record.value_json.contains("mode"));
        assert!(!record.value_json.contains("max_height"));
    }

    #[test]
    fn youtube_preferences_reject_empty_and_invalid_output_roots() {
        let (_directory, connection) = preferences_harness();

        let empty = persist_youtube_preferences(
            &connection,
            SavedYouTubePreferences {
                output_dir: "   ".to_string(),
            },
            1,
        );
        assert!(empty.is_err());
        assert_eq!(empty.unwrap_err().code, "OUTPUT_DIR_REQUIRED");

        let missing = persist_youtube_preferences(
            &connection,
            SavedYouTubePreferences {
                output_dir: "C:\\Users\\Public\\DefinitelyMissingYouTubeRoot-xyz".to_string(),
            },
            2,
        );
        assert!(missing.is_err());
        assert_eq!(missing.unwrap_err().code, "OUTPUT_ROOT_INVALID");

        assert!(load_youtube_preferences(&connection)
            .unwrap()
            .output_dir
            .is_empty());
    }

    #[test]
    fn youtube_preferences_default_when_settings_key_missing() {
        let (_directory, connection) = preferences_harness();
        let loaded = load_youtube_preferences(&connection).unwrap();
        assert_eq!(loaded, SavedYouTubePreferences::default());
    }

    #[test]
    fn youtube_history_projects_terminal_runs_and_falls_back_on_bad_json() {
        use crate::workflow::domain::state::RunState;
        use crate::workflow::domain::types::{RunRecord, WorkflowType};

        let durable = YoutubeDurableRequest {
            schema_version: 1,
            start: StartYouTubeDownloadRequest {
                client_submission_id: "sub-1".to_string(),
                scan_plan_id: "plan-1".to_string(),
                selected_occurrence_ids: vec!["occ-1".to_string()],
                output_dir: "C:\\Videos".to_string(),
                mode: crate::providers::youtube::models::YouTubeDownloadMode::VideoOnly,
                max_height: Some(1080),
                preferred_language: None,
                fallback_languages: vec![],
                allow_automatic_captions: true,
                continue_without_transcript: true,
            },
            work_items: vec![TransientWorkItem {
                occurrence_id: "occ-1".to_string(),
                artifact_fingerprint: "fp".to_string(),
                video_id: "vid-1".to_string(),
                ordinal: 1,
                title: "Sample clip".to_string(),
                source_url: "https://www.youtube.com/watch?v=vid-1".to_string(),
            }],
            context: YouTubeExecutorContext {
                source_snapshot_digest: "a".repeat(64),
                playlist_id: None,
                helper_lock_digest: "b".repeat(64),
                items: vec![],
            },
            plan_fingerprint: "plan-fp".to_string(),
            client_submission_id: "sub-1".to_string(),
        };
        let request_json = serde_json::to_string(&durable).unwrap();

        let succeeded = RunRecord {
            id: "run-ok".to_string(),
            workflow_type: WorkflowType::youtube_download(),
            provider: "youtube".to_string(),
            state: RunState::Succeeded,
            legacy_origin: None,
            legacy_id: None,
            request_json: request_json.clone(),
            output_root: "C:\\Videos\\out".to_string(),
            error_message: None,
            created_at: 100,
            updated_at: 200,
            completed_at: Some(200),
        };
        let running = RunRecord {
            id: "run-live".to_string(),
            workflow_type: WorkflowType::youtube_download(),
            provider: "youtube".to_string(),
            state: RunState::Running,
            legacy_origin: None,
            legacy_id: None,
            request_json: request_json.clone(),
            output_root: "C:\\Videos\\out".to_string(),
            error_message: None,
            created_at: 100,
            updated_at: 150,
            completed_at: None,
        };
        let failed_corrupt = RunRecord {
            id: "run-bad".to_string(),
            workflow_type: WorkflowType::youtube_download(),
            provider: "youtube".to_string(),
            state: RunState::Failed,
            legacy_origin: None,
            legacy_id: None,
            request_json: "{not-json".to_string(),
            output_root: "C:\\Videos\\broken".to_string(),
            error_message: Some("x".repeat(600)),
            created_at: 50,
            updated_at: 60,
            completed_at: Some(60),
        };

        let projected: Vec<_> = [succeeded, running, failed_corrupt]
            .into_iter()
            .filter(|run| run.state.is_terminal())
            .map(|run| project_youtube_history_entry(&run))
            .collect();

        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].run_id, "run-ok");
        assert_eq!(projected[0].state, "succeeded");
        assert_eq!(projected[0].title, "Sample clip");
        assert_eq!(
            projected[0].source_url,
            "https://www.youtube.com/watch?v=vid-1"
        );
        assert_eq!(projected[0].video_count, 1);
        assert_eq!(projected[0].output_dir, "C:\\Videos\\out");

        assert_eq!(projected[1].run_id, "run-bad");
        assert_eq!(projected[1].title, "YouTube download");
        assert_eq!(projected[1].source_url, "");
        assert_eq!(projected[1].video_count, 0);
        let error = projected[1].error_message.as_deref().unwrap();
        assert_eq!(error.chars().count(), YOUTUBE_HISTORY_ERROR_MAX_CHARS);

        assert_eq!(
            youtube_history_limit(&ListYouTubeHistoryRequest { limit: None }),
            i64::from(YOUTUBE_HISTORY_DEFAULT_LIMIT)
        );
        assert_eq!(
            youtube_history_limit(&ListYouTubeHistoryRequest {
                limit: Some(9_999)
            }),
            i64::from(YOUTUBE_HISTORY_MAX_LIMIT)
        );
    }

    #[test]
    fn helper_status_is_ready_only_when_the_start_path_digest_contract_passes() {
        let ready = helper_status_response(
            Ok(HelperIdentity {
                digest: "a".repeat(64),
            }),
            || unreachable!("install probe must not run when identity resolved"),
        );
        assert_eq!(ready.status, YouTubeHelperBackendStatus::Ready);
        assert_eq!(ready.code, None);
    }

    #[test]
    fn helper_status_blocks_a_digest_the_start_path_would_reject() {
        // Regression: v0.2.28 reported ready for `media-toolchain:<version>`.
        let blocked = helper_status_response(
            Ok(HelperIdentity {
                digest: "media-toolchain:2026.10.1".to_string(),
            }),
            || unreachable!("install probe must not run when identity resolved"),
        );
        assert_eq!(blocked.status, YouTubeHelperBackendStatus::Blocked);
        assert_eq!(blocked.code.as_deref(), Some("HELPER_INTEGRITY_FAILED"));
    }

    #[test]
    fn helper_status_reports_missing_when_no_managed_install_exists() {
        let missing =
            helper_status_response(Err(ManagedProcessError::Integrity("absent".into())), || {
                false
            });
        assert_eq!(missing.code.as_deref(), Some("HELPER_MISSING"));
        assert!(missing.message.contains("one-time"));
    }

    #[test]
    fn helper_status_reports_integrity_failure_when_an_install_exists() {
        let failed =
            helper_status_response(Err(ManagedProcessError::Integrity("bad".into())), || true);
        assert_eq!(failed.code.as_deref(), Some("HELPER_INTEGRITY_FAILED"));
    }

    #[test]
    fn blocking_command_work_runs_off_the_calling_thread() {
        let caller = std::thread::current().id();
        let worker = tauri::async_runtime::block_on(run_youtube_blocking(move || {
            Ok(std::thread::current().id())
        }))
        .unwrap();
        assert_ne!(caller, worker);
    }

    #[test]
    fn blocking_command_task_failure_maps_to_a_public_error() {
        let error = tauri::async_runtime::block_on(run_youtube_blocking::<(), _>(|| {
            panic!("simulated worker panic")
        }))
        .unwrap_err();
        assert_eq!(error.code, "TASK_FAILED");
        assert!(error.message.contains("YouTube background task failed"));
    }

    #[test]
    fn blocking_command_bodies_keep_their_error_shapes() {
        let live = YouTubeLiveHandle::default();
        let store = YouTubePlanStore::default();
        let scan = scan_youtube_source_blocking(
            &live,
            &store,
            ScanYouTubeSourceRequest {
                client_operation_id: "bad".to_string(),
                url: "https://www.youtube.com/watch?v=aaaaaaaaaaa".to_string(),
                playlist_mode: None,
            },
        )
        .unwrap_err();
        assert_eq!(scan.code, "INVALID_OPERATION_ID");
    }

    #[test]
    fn helper_start_error_reports_missing_when_no_managed_install_exists() {
        let error = helper_start_error(ManagedProcessError::Integrity("absent".into()), || false);
        assert_eq!(error.code, "HELPER_MISSING");
        assert_eq!(error.message, HELPER_MISSING_MESSAGE);
    }

    #[test]
    fn helper_start_error_reports_the_bare_integrity_reason_when_an_install_exists() {
        let error = helper_start_error(ManagedProcessError::Integrity("bad hash".into()), || true);
        assert_eq!(error.code, "HELPER_INTEGRITY_FAILED");
        assert_eq!(error.message, "bad hash");
        assert!(!error.message.contains("helper integrity validation failed"));
    }

    #[test]
    fn helper_start_error_does_not_blame_the_install_for_non_integrity_failures() {
        let error = helper_start_error(ManagedProcessError::UnsupportedPlatform, || {
            unreachable!("install probe is only for integrity failures")
        });
        assert_eq!(error.code, "HELPER_FAILED");
    }

    mod stored_inspection {
        use super::*;
        use crate::providers::youtube::models::{YouTubeTranscriptSource, YouTubeTranscriptTrack};
        use crate::providers::youtube::scan::{parse_scan_plan, validate_url};
        use crate::providers::youtube::transcript_inspection::parse_transcript_tracks;
        use std::cell::Cell;

        const VIDEO: &str = r#"{"id":"abc123_XY","title":"Video","availability":"public","subtitles":{"en":[{"ext":"vtt","name":"English"}]},"automatic_captions":{"de":[{"ext":"vtt"}]}}"#;
        const FLAT_PLAYLIST: &str = r#"{"id":"PL_12345","title":"List","entries":[{"id":"abc123_XY","title":"A","availability":"public"},{"id":"def456_ZZ","title":"B","availability":"public"}]}"#;

        fn store_with(url: &str, payload: &str) -> (YouTubePlanStore, YouTubeScanPlan) {
            let plan = parse_scan_plan(validate_url(url, None).unwrap(), payload).unwrap();
            let store = YouTubePlanStore::default();
            store.plans.lock().unwrap().insert(plan.clone());
            (store, plan)
        }

        fn request(plan: &YouTubeScanPlan, operation: &str) -> InspectYouTubeTranscriptsRequest {
            InspectYouTubeTranscriptsRequest {
                client_operation_id: operation.to_string(),
                scan_plan_id: plan.response.scan_plan_id.clone(),
                occurrence_ids: plan
                    .items
                    .iter()
                    .map(|item| item.public.occurrence_id.clone())
                    .collect(),
            }
        }

        fn fake_tracks(occurrence_id: &str) -> Vec<YouTubeTranscriptTrack> {
            parse_transcript_tracks(
                occurrence_id,
                r#"{"subtitles":{"it":[{"ext":"vtt","name":"Italian"}]}}"#,
            )
            .unwrap()
        }

        #[test]
        fn stored_scan_data_answers_without_launching_a_helper_or_taking_the_slot() {
            let (store, plan) = store_with("https://www.youtube.com/watch?v=abc123_XY", VIDEO);
            let live = YouTubeLiveHandle::default();
            // Another discovery already owns the single slot: a launch would be
            // refused with RUNTIME_BUSY, a stored answer must not care.
            let _busy = live
                .begin_discovery("busy-operation-01".to_string())
                .unwrap();
            let launches = Cell::new(0);
            let response = inspect_youtube_transcripts_with(
                &live,
                &store,
                request(&plan, "inspect-operation-01"),
                &|_, _| {
                    launches.set(launches.get() + 1);
                    Err(YouTubeInternalError::Helper("must not launch".to_string()))
                },
            )
            .unwrap();
            assert_eq!(launches.get(), 0);
            assert_eq!(response.occurrences.len(), 1);
            // Identical to what a dedicated inspection computes from the same JSON.
            let occurrence_id = &plan.items[0].public.occurrence_id;
            assert_eq!(
                response.occurrences[0].tracks,
                parse_transcript_tracks(occurrence_id, VIDEO).unwrap()
            );
            assert_eq!(response.occurrences[0].tracks.len(), 2);
            assert!(response.occurrences[0]
                .tracks
                .iter()
                .any(|track| track.source == YouTubeTranscriptSource::Automatic));
        }

        #[test]
        fn stored_scan_data_leaves_the_slot_free_for_the_next_discovery() {
            let (store, plan) = store_with("https://www.youtube.com/watch?v=abc123_XY", VIDEO);
            let live = YouTubeLiveHandle::default();
            inspect_youtube_transcripts_with(
                &live,
                &store,
                request(&plan, "inspect-operation-02"),
                &|_, _| Err(YouTubeInternalError::Helper("must not launch".to_string())),
            )
            .unwrap();
            assert!(live
                .begin_discovery("next-operation-02".to_string())
                .is_ok());
        }

        #[test]
        fn occurrences_without_stored_data_fall_back_to_a_launch_once() {
            let (store, plan) = store_with(
                "https://www.youtube.com/playlist?list=PL_12345",
                FLAT_PLAYLIST,
            );
            let live = YouTubeLiveHandle::default();
            let launches = Cell::new(0);
            let launcher = |item: &PlannedYouTubeItem, _: &DiscoveryOperation| {
                launches.set(launches.get() + 1);
                Ok(TranscriptInspectionResult {
                    occurrence_id: item.public.occurrence_id.clone(),
                    video_id: item.public.video_id.clone(),
                    tracks: fake_tracks(&item.public.occurrence_id),
                })
            };
            let first = inspect_youtube_transcripts_with(
                &live,
                &store,
                request(&plan, "inspect-operation-03"),
                &launcher,
            )
            .unwrap();
            assert_eq!(launches.get(), 2);
            assert_eq!(first.occurrences.len(), 2);
            assert_eq!(first.occurrences[0].tracks[0].language_tag, "it");

            // The launched answers are stored on the plan, so a repeat is instant.
            let second = inspect_youtube_transcripts_with(
                &live,
                &store,
                request(&plan, "inspect-operation-04"),
                &launcher,
            )
            .unwrap();
            assert_eq!(launches.get(), 2);
            assert_eq!(second.occurrences, first.occurrences);
        }

        #[test]
        fn a_launch_is_still_refused_while_another_discovery_holds_the_slot() {
            let (store, plan) = store_with(
                "https://www.youtube.com/playlist?list=PL_12345",
                FLAT_PLAYLIST,
            );
            let live = YouTubeLiveHandle::default();
            let _busy = live
                .begin_discovery("busy-operation-05".to_string())
                .unwrap();
            let launches = Cell::new(0);
            let error = inspect_youtube_transcripts_with(
                &live,
                &store,
                request(&plan, "inspect-operation-05"),
                &|_, _| {
                    launches.set(launches.get() + 1);
                    Err(YouTubeInternalError::Helper("must not launch".to_string()))
                },
            )
            .unwrap_err();
            assert_eq!(error.code, "RUNTIME_BUSY");
            assert_eq!(launches.get(), 0);
        }

        #[test]
        fn a_mixed_request_launches_only_for_the_occurrences_without_stored_data() {
            let (store, plan) = store_with(
                "https://www.youtube.com/playlist?list=PL_12345",
                FLAT_PLAYLIST,
            );
            let live = YouTubeLiveHandle::default();
            // Store the first occurrence as an earlier inspection would have.
            let first = &plan.items[0];
            store
                .plans
                .lock()
                .unwrap()
                .apply_transcript_inspections(
                    &plan.response.scan_plan_id,
                    vec![into_plan_inspection(
                        TranscriptInspectionResult {
                            occurrence_id: first.public.occurrence_id.clone(),
                            video_id: first.public.video_id.clone(),
                            tracks: fake_tracks(&first.public.occurrence_id),
                        },
                        &plan.source_snapshot_digest,
                        &first.public.metadata_digest,
                    )],
                )
                .unwrap();
            let launched = std::cell::RefCell::new(Vec::new());
            let response = inspect_youtube_transcripts_with(
                &live,
                &store,
                request(&plan, "inspect-operation-06"),
                &|item, _| {
                    launched.borrow_mut().push(item.public.video_id.clone());
                    Ok(TranscriptInspectionResult {
                        occurrence_id: item.public.occurrence_id.clone(),
                        video_id: item.public.video_id.clone(),
                        tracks: Vec::new(),
                    })
                },
            )
            .unwrap();
            assert_eq!(*launched.borrow(), vec!["def456_ZZ".to_string()]);
            assert_eq!(response.occurrences.len(), 2);
            assert_eq!(response.occurrences[0].tracks.len(), 1);
            assert!(response.occurrences[1].tracks.is_empty());
        }
    }

    mod start_revalidation {
        use super::*;
        use crate::providers::youtube::scan::{
            parse_scan_plan, validate_url, START_REVALIDATION_FRESH_FOR,
        };
        use std::cell::Cell;
        use std::time::Duration;

        const PLAYLIST: &str = r#"{"id":"PL_12345","title":"List","entries":[{"id":"abc123_XY","title":"A","availability":"public"},{"id":"def456_ZZ","title":"B","availability":"public"}]}"#;

        fn plan() -> YouTubeScanPlan {
            parse_scan_plan(
                validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap(),
                PLAYLIST,
            )
            .unwrap()
        }

        fn selection(plan: &YouTubeScanPlan) -> Vec<String> {
            // Reverse order: the start keeps the request's order.
            plan.items
                .iter()
                .rev()
                .map(|item| item.public.occurrence_id.clone())
                .collect()
        }

        #[test]
        fn a_plan_scanned_just_under_ten_minutes_ago_starts_without_discovery() {
            let plan = plan();
            let live = YouTubeLiveHandle::default();
            // The slot being busy must not matter: no discovery is needed.
            let _busy = live
                .begin_discovery("busy-operation-11".to_string())
                .unwrap();
            let launches = Cell::new(0);
            let now = plan.scanned_at + START_REVALIDATION_FRESH_FOR - Duration::from_secs(1);
            let (items, guard) = selected_items_for_start(
                &live,
                &plan,
                &selection(&plan),
                now,
                &|_, _, _| {
                    launches.set(launches.get() + 1);
                    Err(YouTubeInternalError::Helper("must not launch".to_string()))
                },
            )
            .unwrap();
            assert_eq!(launches.get(), 0);
            assert!(guard.is_none());
            assert_eq!(
                items
                    .iter()
                    .map(|item| item.public.video_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["def456_ZZ", "abc123_XY"]
            );
        }

        #[test]
        fn a_plan_scanned_ten_minutes_ago_or_more_still_revalidates() {
            let plan = plan();
            let live = YouTubeLiveHandle::default();
            let launches = Cell::new(0);
            let now = plan.scanned_at + START_REVALIDATION_FRESH_FOR;
            let (items, guard) = selected_items_for_start(
                &live,
                &plan,
                &selection(&plan),
                now,
                &|plan, selected, _| {
                    launches.set(launches.get() + 1);
                    Ok(selected
                        .iter()
                        .filter_map(|id| {
                            plan.items
                                .iter()
                                .find(|item| &item.public.occurrence_id == id)
                                .cloned()
                        })
                        .collect())
                },
            )
            .unwrap();
            assert_eq!(launches.get(), 1);
            assert_eq!(items.len(), 2);
            // The revalidation holds the discovery slot until the guard drops.
            assert!(guard.is_some());
            assert_eq!(
                live.begin_discovery("other-operation-12".to_string())
                    .map(|_| ())
                    .unwrap_err()
                    .to_string(),
                crate::app::managed_process::TransientRuntimeError::Busy.to_string()
            );
            drop(guard);
            assert!(live
                .begin_discovery("other-operation-13".to_string())
                .is_ok());
        }

        #[test]
        fn a_stale_plan_whose_video_changed_fails_the_start_visibly() {
            let plan = plan();
            let live = YouTubeLiveHandle::default();
            let now = plan.scanned_at + START_REVALIDATION_FRESH_FOR + Duration::from_secs(1);
            let error = selected_items_for_start(
                &live,
                &plan,
                &selection(&plan),
                now,
                &|_, _, _| Err(YouTubeInternalError::ScanPlanStale),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(error.code, "SCAN_PLAN_STALE");
            // The failed start releases the slot.
            assert!(live
                .begin_discovery("other-operation-14".to_string())
                .is_ok());
        }

        #[test]
        fn a_stale_plan_still_needs_the_discovery_slot() {
            let plan = plan();
            let live = YouTubeLiveHandle::default();
            let _busy = live
                .begin_discovery("busy-operation-15".to_string())
                .unwrap();
            let now = plan.scanned_at + START_REVALIDATION_FRESH_FOR;
            let error = selected_items_for_start(
                &live,
                &plan,
                &selection(&plan),
                now,
                &|_, _, _| Ok(Vec::new()),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(error.code, "RUNTIME_BUSY");
        }

        #[test]
        fn the_fresh_path_keeps_the_selection_rules() {
            let plan = plan();
            let live = YouTubeLiveHandle::default();
            let now = plan.scanned_at;
            let launcher = |_: &YouTubeScanPlan, _: &[String], _: &DiscoveryOperation| {
                Err(YouTubeInternalError::Helper("must not launch".to_string()))
            };
            let first = plan.items[0].public.occurrence_id.clone();
            let code = |ids: Vec<String>| {
                selected_items_for_start(&live, &plan, &ids, now, &launcher)
                    .map(|_| ())
                    .unwrap_err()
                    .code
            };
            assert_eq!(code(Vec::new()), "EMPTY_SELECTION");
            assert_eq!(code(vec![first.clone(), first]), "DUPLICATE_OCCURRENCE");
            assert_eq!(code(vec!["nope".to_string()]), "UNKNOWN_OCCURRENCE");
        }
    }
}
