use crate::app::safe_output_filesystem::validate_output_root;
use crate::providers::youtube::error::{YouTubeError, YouTubeInternalError};
use crate::providers::youtube::executor::{
    YouTubeExecutor, YouTubeExecutorContext, YouTubeExecutorItemContext,
};
use crate::providers::youtube::helper::helper_kind;
use crate::providers::youtube::manifest_contract::{
    artifact_fingerprint, select_transcript, ArtifactFingerprintInput, ManifestContractError,
    SelectedTranscript, FORMAT_POLICY_VERSION,
};
use crate::providers::youtube::models::{
    CancelYouTubeRunRequest, GetYouTubeDownloadStateRequest, GetYouTubeDownloadStateResponse,
    GetYouTubeHelperStatusResponse, InspectYouTubeTranscriptsRequest,
    InspectYouTubeTranscriptsResponse, MutateYouTubeRunRequest, ScanYouTubeSourceRequest,
    ScanYouTubeSourceResponse, StartYouTubeDownloadRequest, StartYouTubeDownloadResponse,
    YouTubeAvailability, YouTubeHelperBackendStatus, YouTubeTranscriptOccurrence,
};
use crate::providers::youtube::scan::{revalidate_selected_source, scan_source, YouTubeScanPlan};
use crate::providers::youtube::transcript_inspection::{inspect_transcripts, into_plan_inspection};
use crate::workflow::transient::managed_process::helper_identity;
use crate::workflow::transient::{
    TransientRunSnapshot, TransientSubmissionReceipt, TransientWorkItem, TransientWorkflowState,
    MAX_SELECTED_ITEMS,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;
use tauri::State;

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

#[tauri::command]
pub fn get_youtube_helper_status() -> GetYouTubeHelperStatusResponse {
    match helper_identity(helper_kind()) {
        Ok(_) => GetYouTubeHelperStatusResponse {
            status: YouTubeHelperBackendStatus::Ready,
            code: None,
            message: "YouTube helper integrity validation passed.".to_string(),
        },
        Err(_) => GetYouTubeHelperStatusResponse {
            status: YouTubeHelperBackendStatus::Blocked,
            code: Some("HELPER_EXECUTION_BLOCKED".to_string()),
            message: "YouTube helper execution is blocked because the reviewed packaged helper set is missing or failed integrity validation."
                .to_string(),
        },
    }
}

#[tauri::command]
pub fn scan_youtube_source(
    workflow: State<'_, TransientWorkflowState>,
    state: State<'_, YouTubePlanStore>,
    request: ScanYouTubeSourceRequest,
) -> Result<ScanYouTubeSourceResponse, YouTubeError> {
    validate_operation_id(&request.client_operation_id)?;
    let operation = workflow
        .runtime()
        .begin_discovery(request.client_operation_id.clone())
        .map_err(YouTubeError::from)?;
    let plan = scan_source(&request, &operation).map_err(YouTubeError::from)?;
    let response = plan.response.clone();
    state
        .plans
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(plan);
    Ok(response)
}

#[tauri::command]
pub fn inspect_youtube_transcripts(
    workflow: State<'_, TransientWorkflowState>,
    state: State<'_, YouTubePlanStore>,
    request: InspectYouTubeTranscriptsRequest,
) -> Result<InspectYouTubeTranscriptsResponse, YouTubeError> {
    validate_operation_id(&request.client_operation_id)?;
    let operation = workflow
        .runtime()
        .begin_discovery(request.client_operation_id)
        .map_err(YouTubeError::from)?;
    let plan = get_plan(&state, &request.scan_plan_id)?;
    let mut seen = std::collections::HashSet::new();
    let mut occurrences = Vec::new();
    let mut plan_inspections = Vec::new();
    for occurrence_id in request.occurrence_ids {
        if !seen.insert(occurrence_id.clone()) {
            return Err(YouTubeError::from(
                YouTubeInternalError::DuplicateOccurrence,
            ));
        }
        let item = plan
            .items
            .iter()
            .find(|item| item.public.occurrence_id == occurrence_id)
            .ok_or_else(|| YouTubeError::from(YouTubeInternalError::UnknownOccurrence))?;
        if item.public.availability != YouTubeAvailability::Available {
            return Err(YouTubeError::from(YouTubeInternalError::ScanPlanStale));
        }
        if operation.cancellation_requested() {
            return Err(YouTubeError::new(
                "DISCOVERY_CANCELLED",
                "transcript inspection was cancelled",
            ));
        }
        let inspected = inspect_transcripts(item, &operation).map_err(YouTubeError::from)?;
        occurrences.push(YouTubeTranscriptOccurrence {
            occurrence_id: inspected.occurrence_id.clone(),
            video_id: inspected.video_id.clone(),
            tracks: inspected.tracks.clone(),
        });
        plan_inspections.push(into_plan_inspection(
            inspected,
            &plan.source_snapshot_digest,
            &item.public.metadata_digest,
        ));
    }
    state
        .plans
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .apply_transcript_inspections(&request.scan_plan_id, plan_inspections)
        .map_err(YouTubeError::from)?;
    Ok(InspectYouTubeTranscriptsResponse { occurrences })
}

#[tauri::command]
pub fn cancel_youtube_discovery(
    workflow: State<'_, TransientWorkflowState>,
    _state: State<'_, YouTubePlanStore>,
    request: CancelYouTubeDiscoveryRequest,
) -> Result<(), YouTubeError> {
    validate_operation_id(&request.client_operation_id)?;
    workflow
        .runtime()
        .cancel_discovery(&request.client_operation_id)
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub fn start_youtube_download(
    workflow: State<'_, TransientWorkflowState>,
    state: State<'_, YouTubePlanStore>,
    request: StartYouTubeDownloadRequest,
) -> Result<StartYouTubeDownloadResponse, YouTubeError> {
    validate_submission_id(&request.client_submission_id)?;
    let request_fingerprint = submission_fingerprint(&request);
    if let Some(receipt) = workflow
        .runtime()
        .find_submission(&request.client_submission_id, &request_fingerprint)
        .map_err(YouTubeError::from)?
    {
        let snapshot = workflow
            .runtime()
            .get_state(Some(&receipt.run_id))
            .map_err(YouTubeError::from)?
            .ok_or_else(|| YouTubeError::new("RUN_NOT_FOUND", "download state is unavailable"))?;
        return Ok(StartYouTubeDownloadResponse {
            client_submission_id: receipt.client_submission_id,
            run_id: receipt.run_id,
            revision: snapshot.revision,
            scan_plan_id: receipt.scan_plan_id,
            plan_fingerprint: receipt.plan_fingerprint,
            state: snapshot.state.as_str().to_string(),
        });
    }
    let plan = get_plan(&state, &request.scan_plan_id)?;
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
    let revalidation = workflow
        .runtime()
        .begin_discovery(opaque_id("revalidation"))
        .map_err(YouTubeError::from)?;
    let current_selected_items =
        revalidate_selected_source(&plan, &request.selected_occurrence_ids, &revalidation)
            .map_err(YouTubeError::from)?;
    drop(revalidation);
    let output_root = validate_output_root(PathBuf::from(&request.output_dir).as_path())?;
    // Revalidation has released its discovery admission. Re-resolve the same
    // reviewed helper set before constructing the immutable run; a missing,
    // changed, or non-ready inventory fails closed here.
    let helper = helper_identity(helper_kind())
        .map_err(|error| YouTubeError::new("HELPER_INTEGRITY_FAILED", error.to_string()))?;
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
    let executor = YouTubeExecutor::new_with_context(
        output_root,
        &request,
        YouTubeExecutorContext {
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
        },
    )?;
    let snapshot = workflow
        .runtime()
        .start_run(
            run_id,
            request.client_submission_id.clone(),
            plan_fingerprint.clone(),
            work_items,
            executor,
        )
        .map_err(YouTubeError::from)?;
    let response = StartYouTubeDownloadResponse {
        client_submission_id: request.client_submission_id.clone(),
        run_id: snapshot.run_id,
        revision: snapshot.revision,
        scan_plan_id: request.scan_plan_id,
        plan_fingerprint: plan_fingerprint.clone(),
        state: "running".to_string(),
    };
    workflow
        .runtime()
        .record_submission(TransientSubmissionReceipt {
            client_submission_id: request.client_submission_id,
            request_fingerprint,
            plan_fingerprint,
            run_id: response.run_id.clone(),
            revision: response.revision,
            scan_plan_id: response.scan_plan_id.clone(),
        });
    Ok(response)
}

#[tauri::command]
pub fn get_youtube_download_state(
    workflow: State<'_, TransientWorkflowState>,
    state: State<'_, YouTubePlanStore>,
    request: GetYouTubeDownloadStateRequest,
) -> Result<GetYouTubeDownloadStateResponse, YouTubeError> {
    let _ = state;
    workflow
        .runtime()
        .get_state(request.run_id.as_deref())
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub fn pause_youtube_download(
    workflow: State<'_, TransientWorkflowState>,
    state: State<'_, YouTubePlanStore>,
    request: MutateYouTubeRunRequest,
) -> Result<TransientRunSnapshot, YouTubeError> {
    let _ = state;
    workflow
        .runtime()
        .pause(&request.run_id, request.expected_revision)
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub fn resume_youtube_download(
    workflow: State<'_, TransientWorkflowState>,
    state: State<'_, YouTubePlanStore>,
    request: MutateYouTubeRunRequest,
) -> Result<TransientRunSnapshot, YouTubeError> {
    let _ = state;
    workflow
        .runtime()
        .resume(&request.run_id, request.expected_revision)
        .map_err(YouTubeError::from)
}

#[tauri::command]
pub fn cancel_youtube_download(
    workflow: State<'_, TransientWorkflowState>,
    state: State<'_, YouTubePlanStore>,
    request: CancelYouTubeRunRequest,
) -> Result<TransientRunSnapshot, YouTubeError> {
    let _ = state;
    workflow
        .runtime()
        .cancel(&request.run_id)
        .map_err(YouTubeError::from)
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

fn submission_fingerprint(request: &StartYouTubeDownloadRequest) -> String {
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
    .expect("YouTube submission fields are infallibly serializable");
    let mut hasher = Sha256::new();
    hasher.update(encoded);
    format!("{:x}", hasher.finalize())
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
    use std::time::{SystemTime, UNIX_EPOCH};
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    digest(&format!("{prefix}|{}|{now}|{sequence}", std::process::id()))
}
