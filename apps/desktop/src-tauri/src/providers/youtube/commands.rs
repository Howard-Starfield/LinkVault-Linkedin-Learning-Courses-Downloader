use crate::app::safe_output_filesystem::validate_output_root;
use crate::providers::youtube::error::{YouTubeError, YouTubeInternalError};
use crate::providers::youtube::executor::YouTubeExecutor;
use crate::providers::youtube::helper::helper_kind;
use crate::providers::youtube::models::{
    CancelYouTubeRunRequest, GetYouTubeDownloadStateRequest, GetYouTubeDownloadStateResponse,
    GetYouTubeHelperStatusResponse, InspectYouTubeTranscriptsRequest,
    InspectYouTubeTranscriptsResponse, MutateYouTubeRunRequest, ScanYouTubeSourceRequest,
    ScanYouTubeSourceResponse, StartYouTubeDownloadRequest, StartYouTubeDownloadResponse,
    YouTubeHelperBackendStatus, YouTubeTranscriptOccurrence,
};
use crate::providers::youtube::scan::{scan_source, YouTubeScanPlan};
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
            message: "YouTube helper execution remains blocked until the reviewed helper inventory and native runtime hardening are complete."
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
        if operation.cancellation_requested() {
            return Err(YouTubeError::new(
                "DISCOVERY_CANCELLED",
                "transcript inspection was cancelled",
            ));
        }
        // The flat scan intentionally does not expose signed subtitle URLs.
        // A later bounded inspection call can populate this typed list without
        // changing the React contract; an empty list is an honest "unknown".
        occurrences.push(YouTubeTranscriptOccurrence {
            occurrence_id: item.public.occurrence_id.clone(),
            video_id: item.public.video_id.clone(),
            tracks: Vec::new(),
        });
    }
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
    let selected_items = plan
        .items
        .iter()
        .filter(|item| selected.contains(&item.public.occurrence_id))
        .collect::<Vec<_>>();
    if selected_items.len() != selected.len() {
        return Err(YouTubeError::from(YouTubeInternalError::UnknownOccurrence));
    }
    let output_root = validate_output_root(PathBuf::from(&request.output_dir).as_path())?;
    // Helper resolution happens before admission and before any helper launch.
    // The embedded lock is currently unpopulated, so this intentionally fails
    // closed until Y0 supplies a reviewed ready lock and packaged executable.
    let helper = helper_identity(helper_kind())
        .map_err(|error| YouTubeError::new("HELPER_INTEGRITY_FAILED", error.to_string()))?;
    let plan_fingerprint = fingerprint(&request, &plan, &helper.digest);
    if let Some(receipt) = workflow
        .runtime()
        .find_submission(&request.client_submission_id, &plan_fingerprint)
        .map_err(YouTubeError::from)?
    {
        return Ok(StartYouTubeDownloadResponse {
            client_submission_id: receipt.client_submission_id,
            run_id: receipt.run_id,
            revision: receipt.revision,
            scan_plan_id: receipt.scan_plan_id,
            plan_fingerprint: receipt.plan_fingerprint,
            state: "running".to_string(),
        });
    }
    let run_id = opaque_id("run");
    let work_items = selected_items
        .into_iter()
        .map(|item| TransientWorkItem {
            occurrence_id: item.public.occurrence_id.clone(),
            artifact_fingerprint: fingerprint_item(item, &request, &helper.digest),
            video_id: item.public.video_id.clone(),
            ordinal: item.public.ordinal,
            title: item.public.title.clone(),
            source_url: item.source_url.clone(),
        })
        .collect::<Vec<_>>();
    let executor = YouTubeExecutor::new(output_root, &request)?;
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

fn fingerprint(
    request: &StartYouTubeDownloadRequest,
    plan: &YouTubeScanPlan,
    helper_digest: &str,
) -> String {
    let mut input = format!(
        "1|{}|{}|{}|{:?}|{:?}|{}|{}|{}|{}|{}",
        plan.response.scan_plan_id,
        plan.response.canonical_url,
        request.mode.clone() as u8,
        request.max_height,
        request.preferred_language,
        request.fallback_languages.join(","),
        request.allow_automatic_captions,
        request.continue_without_transcript,
        helper_digest,
        request.selected_occurrence_ids.join(",")
    );
    input.push('|');
    input.push_str(&request.output_dir);
    digest(&input)
}

fn fingerprint_item(
    item: &crate::providers::youtube::scan::PlannedYouTubeItem,
    request: &StartYouTubeDownloadRequest,
    helper_digest: &str,
) -> String {
    digest(&format!(
        "1|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        item.public.occurrence_id,
        item.public.video_id,
        mode_key(&request.mode),
        request
            .max_height
            .map_or_else(|| "best".to_string(), |height| height.to_string()),
        request.preferred_language.as_deref().unwrap_or_default(),
        request.fallback_languages.join(","),
        request.allow_automatic_captions,
        request.continue_without_transcript,
        helper_digest,
    ))
}

fn mode_key(mode: &crate::providers::youtube::models::YouTubeDownloadMode) -> &'static str {
    match mode {
        crate::providers::youtube::models::YouTubeDownloadMode::VideoAndTranscript => {
            "video_and_transcript"
        }
        crate::providers::youtube::models::YouTubeDownloadMode::VideoOnly => "video_only",
        crate::providers::youtube::models::YouTubeDownloadMode::TranscriptOnly => "transcript_only",
    }
}

fn digest(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
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
