use crate::cache::{
    append_job_event, get_job, list_artifacts_for_job, update_artifact_status, upsert_artifact,
    ArtifactRecord, CacheError, NewJobEvent,
};
use crate::exercise_archive::{extract_zip_and_delete_archive, ExerciseArchiveExtractionResult};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use url::Url;

// Conservative load-smoothing defaults, not a provider-published safe rate.
#[cfg(not(test))]
const DEFAULT_VIDEO_WAIT_MIN_SECONDS: u32 = 20;
#[cfg(not(test))]
const DEFAULT_VIDEO_WAIT_MAX_SECONDS: u32 = 40;
#[cfg(test)]
const DEFAULT_VIDEO_WAIT_MIN_SECONDS: u32 = 0;
#[cfg(test)]
const DEFAULT_VIDEO_WAIT_MAX_SECONDS: u32 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactDownloadSource {
    Url(String),
    Urls(Vec<String>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedArtifactDownload {
    pub artifact: ArtifactRecord,
    pub source: ArtifactDownloadSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHttpResponse {
    pub status: u16,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHttpAttempt {
    pub url: String,
    pub status: Option<u16>,
    pub error_kind: Option<String>,
}

pub trait ArtifactHttpClient {
    fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError>;
}

pub trait CancellationFlag {
    fn is_cancelled(&self) -> bool;

    fn is_paused(&self) -> bool {
        false
    }

    fn wait_if_paused(&self) {
        while self.is_paused() && !self.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

pub struct NeverCancelled;

impl CancellationFlag for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDownloadSummary {
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

#[derive(Debug, Error)]
pub enum ArtifactDownloadError {
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("artifact download network request failed: {0}")]
    Network(String),
    #[error("artifact download returned HTTP status {status}")]
    Http {
        status: u16,
        attempts: Vec<ArtifactHttpAttempt>,
    },
    #[error("artifact download failed after all URL attempts")]
    AttemptsFailed { attempts: Vec<ArtifactHttpAttempt> },
    #[error("job must be active before artifact downloads can run: {job_id}")]
    JobNotActive { job_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ArtifactWriteOutcome {
    Written(i64),
    CancelledBeforeWrite,
}

pub fn download_artifacts_for_active_job(
    connection: &Connection,
    client: &mut impl ArtifactHttpClient,
    cancellation: &impl CancellationFlag,
    job_id: &str,
    downloads: &[PlannedArtifactDownload],
    timestamp: i64,
) -> Result<ArtifactDownloadSummary, ArtifactDownloadError> {
    let mut pacing = VideoPacingPolicy::new(
        DEFAULT_VIDEO_WAIT_MIN_SECONDS,
        DEFAULT_VIDEO_WAIT_MAX_SECONDS,
        video_pacing_seed(job_id, timestamp),
    );
    download_artifacts_for_active_job_with_pacing(
        connection,
        client,
        cancellation,
        job_id,
        downloads,
        timestamp,
        &mut pacing,
        &mut wait_for_video_pacing,
    )
}

#[allow(clippy::too_many_arguments)]
fn download_artifacts_for_active_job_with_pacing(
    connection: &Connection,
    client: &mut impl ArtifactHttpClient,
    cancellation: &impl CancellationFlag,
    job_id: &str,
    downloads: &[PlannedArtifactDownload],
    timestamp: i64,
    pacing: &mut VideoPacingPolicy,
    wait_for: &mut impl FnMut(u32, &dyn CancellationFlag) -> bool,
) -> Result<ArtifactDownloadSummary, ArtifactDownloadError> {
    let job = get_job(connection, job_id)?
        .filter(|job| job.status == "active")
        .ok_or_else(|| ArtifactDownloadError::JobNotActive {
            job_id: job_id.to_string(),
        })?;

    let mut summary = ArtifactDownloadSummary {
        completed: 0,
        failed: 0,
        cancelled: 0,
    };
    let existing_artifacts = list_artifacts_for_job(connection, &job.id)?;
    let mut attempted_video_request = false;

    for (index, download) in downloads.iter().enumerate() {
        cancellation.wait_if_paused();
        let mut cancellation_reason = cancellation
            .is_cancelled()
            .then_some("Download was cancelled before all artifacts completed.");

        if cancellation_reason.is_none() {
            upsert_artifact(connection, &download.artifact)?;
            if let Some(size_bytes) = reusable_artifact_size(&existing_artifacts, download) {
                update_artifact_status(
                    connection,
                    &download.artifact.id,
                    "completed",
                    size_bytes,
                    timestamp,
                )?;
                append_artifact_event(
                    connection,
                    &job.id,
                    "artifact.completed",
                    format!(
                        "Reused existing {}.",
                        artifact_display_name(&download.artifact)
                    ),
                    &download.artifact.id,
                    timestamp,
                )?;
                summary.completed += 1;
                continue;
            }

            if is_video_artifact(&download.artifact) {
                if attempted_video_request {
                    let wait_seconds = pacing.next_wait_seconds(&download.artifact.id);
                    if wait_seconds > 0 {
                        append_video_pacing_event(
                            connection,
                            &job.id,
                            &download.artifact.id,
                            wait_seconds,
                        )?;
                        if !wait_for(wait_seconds, cancellation) {
                            cancellation_reason =
                                Some("Download was cancelled while pacing video requests.");
                        }
                    }
                }
                attempted_video_request = true;
            }
        }

        if let Some(reason) = cancellation_reason {
            cancel_artifacts_from(connection, &job.id, &downloads[index..], None, timestamp)?;
            summary.cancelled += downloads.len() - index;
            crate::cache::transition_job_status(
                connection,
                &job.id,
                "cancelled",
                timestamp,
                Some(reason),
            )?;
            return Ok(summary);
        }

        update_artifact_status(connection, &download.artifact.id, "active", None, timestamp)?;
        append_artifact_event(
            connection,
            &job.id,
            "artifact.active",
            format!("Started {}.", artifact_display_name(&download.artifact)),
            &download.artifact.id,
            timestamp,
        )?;

        match write_artifact(client, cancellation, download) {
            Ok(ArtifactWriteOutcome::CancelledBeforeWrite) => {
                cancel_artifacts_from(connection, &job.id, &downloads[index..], None, timestamp)?;
                summary.cancelled += downloads.len() - index;
                crate::cache::transition_job_status(
                    connection,
                    &job.id,
                    "cancelled",
                    timestamp,
                    Some("Download was cancelled before the active artifact was written."),
                )?;
                return Ok(summary);
            }
            Ok(ArtifactWriteOutcome::Written(size_bytes)) => {
                if is_exercise_zip_artifact(&download.artifact) {
                    if cancellation.is_cancelled() {
                        cancel_artifacts_from(
                            connection,
                            &job.id,
                            &downloads[index..],
                            Some(size_bytes),
                            timestamp,
                        )?;
                        summary.cancelled += downloads.len() - index;
                        crate::cache::transition_job_status(
                            connection,
                            &job.id,
                            "cancelled",
                            timestamp,
                            Some("Download was cancelled before exercise zip extraction."),
                        )?;
                        return Ok(summary);
                    }

                    let extraction =
                        extract_zip_and_delete_archive(Path::new(&download.artifact.path));
                    if extraction.attempted && !extraction.succeeded {
                        update_artifact_status(
                            connection,
                            &download.artifact.id,
                            "failed",
                            Some(size_bytes),
                            timestamp,
                        )?;
                        append_artifact_event(
                            connection,
                            &job.id,
                            "artifact.failed",
                            format_extraction_failure_message(&extraction),
                            &download.artifact.id,
                            timestamp,
                        )?;
                        summary.failed += 1;
                        continue;
                    }

                    if extraction.succeeded {
                        append_artifact_event(
                            connection,
                            &job.id,
                            "artifact.extracted",
                            format_extraction_success_message(&extraction),
                            &download.artifact.id,
                            timestamp,
                        )?;
                        if let Some(message) = &extraction.message {
                            append_artifact_event(
                                connection,
                                &job.id,
                                "artifact.extraction.warning",
                                format!("Exercise archive extracted with warning: {message}"),
                                &download.artifact.id,
                                timestamp,
                            )?;
                        }
                    }

                    if cancellation.is_cancelled() {
                        update_artifact_status(
                            connection,
                            &download.artifact.id,
                            "completed",
                            Some(size_bytes),
                            timestamp,
                        )?;
                        append_artifact_event(
                            connection,
                            &job.id,
                            "artifact.completed",
                            format!("Completed {}.", artifact_display_name(&download.artifact)),
                            &download.artifact.id,
                            timestamp,
                        )?;
                        summary.completed += 1;
                        cancel_artifacts_from(
                            connection,
                            &job.id,
                            &downloads[index + 1..],
                            None,
                            timestamp,
                        )?;
                        summary.cancelled += downloads.len() - index - 1;
                        crate::cache::transition_job_status(
                            connection,
                            &job.id,
                            "cancelled",
                            timestamp,
                            Some("Download was cancelled after exercise zip extraction completed."),
                        )?;
                        return Ok(summary);
                    }
                }

                update_artifact_status(
                    connection,
                    &download.artifact.id,
                    "completed",
                    Some(size_bytes),
                    timestamp,
                )?;
                append_artifact_event(
                    connection,
                    &job.id,
                    "artifact.completed",
                    format!("Completed {}.", artifact_display_name(&download.artifact)),
                    &download.artifact.id,
                    timestamp,
                )?;
                summary.completed += 1;
            }
            Err(error) if is_exercise_artifact(&download.artifact) => {
                let failure_reason = safe_artifact_error_reason(&error);
                update_artifact_status(
                    connection,
                    &download.artifact.id,
                    "failed",
                    None,
                    timestamp,
                )?;
                append_artifact_event(
                    connection,
                    &job.id,
                    "artifact.failed",
                    format!("Exercise artifact download failed and was skipped: {failure_reason}."),
                    &download.artifact.id,
                    timestamp,
                )?;
                append_artifact_source_event(
                    connection,
                    &job.id,
                    &download.artifact.id,
                    &download.source,
                    Some(&error),
                    timestamp,
                )?;
                summary.failed += 1;
            }
            Err(error) => {
                let failure_reason = safe_artifact_error_reason(&error);
                update_artifact_status(
                    connection,
                    &download.artifact.id,
                    "failed",
                    None,
                    timestamp,
                )?;
                append_artifact_event(
                    connection,
                    &job.id,
                    "artifact.failed",
                    format!("Artifact download failed: {failure_reason}."),
                    &download.artifact.id,
                    timestamp,
                )?;
                append_artifact_source_event(
                    connection,
                    &job.id,
                    &download.artifact.id,
                    &download.source,
                    Some(&error),
                    timestamp,
                )?;
                crate::cache::transition_job_status(
                    connection,
                    &job.id,
                    "failed",
                    timestamp,
                    Some(&format!("Artifact download failed: {failure_reason}.")),
                )?;
                return Err(error);
            }
        }
    }

    crate::cache::transition_job_status(
        connection,
        &job.id,
        "completed",
        timestamp,
        Some("All required artifacts finished."),
    )?;
    Ok(summary)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VideoPacingPolicy {
    min_seconds: u32,
    max_seconds: u32,
    state: u64,
}

impl VideoPacingPolicy {
    fn new(min_seconds: u32, max_seconds: u32, seed: u64) -> Self {
        Self {
            min_seconds: min_seconds.min(max_seconds),
            max_seconds: max_seconds.max(min_seconds),
            state: seed,
        }
    }

    fn next_wait_seconds(&mut self, artifact_id: &str) -> u32 {
        for byte in artifact_id.bytes() {
            self.state ^= u64::from(byte);
            self.state = self.state.wrapping_mul(0x100000001b3);
        }
        self.state = splitmix64(self.state);
        if self.max_seconds <= self.min_seconds {
            return self.min_seconds;
        }
        self.min_seconds + (self.state % u64::from(self.max_seconds - self.min_seconds + 1)) as u32
    }
}

fn video_pacing_seed(job_id: &str, timestamp: i64) -> u64 {
    let mut seed = 0xcbf29ce484222325_u64 ^ timestamp as u64;
    for byte in job_id.bytes() {
        seed ^= u64::from(byte);
        seed = seed.wrapping_mul(0x100000001b3);
    }
    seed
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn wait_for_video_pacing(seconds: u32, cancellation: &dyn CancellationFlag) -> bool {
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
    loop {
        cancellation.wait_if_paused();
        if cancellation.is_cancelled() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        thread::sleep(remaining.min(Duration::from_secs(1)));
    }
}

fn is_video_artifact(artifact: &ArtifactRecord) -> bool {
    artifact.artifact_type == "video"
}

fn reusable_artifact_size(
    existing_artifacts: &[ArtifactRecord],
    download: &PlannedArtifactDownload,
) -> Option<Option<i64>> {
    let previous_completed = existing_artifacts
        .iter()
        .any(|artifact| artifact.id == download.artifact.id && artifact.status == "completed");

    if previous_completed && is_exercise_zip_artifact(&download.artifact) {
        return Some(download.artifact.size_bytes.or_else(|| {
            existing_artifacts
                .iter()
                .find(|artifact| artifact.id == download.artifact.id)
                .and_then(|artifact| artifact.size_bytes)
        }));
    }

    file_size_if_reusable(Path::new(&download.artifact.path)).map(Some)
}

fn file_size_if_reusable(path: &Path) -> Option<i64> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return None;
    }
    i64::try_from(metadata.len()).ok()
}

fn write_artifact(
    client: &mut impl ArtifactHttpClient,
    cancellation: &impl CancellationFlag,
    download: &PlannedArtifactDownload,
) -> Result<ArtifactWriteOutcome, ArtifactDownloadError> {
    let bytes = match &download.source {
        ArtifactDownloadSource::Text(text) => text.as_bytes().to_vec(),
        ArtifactDownloadSource::Url(url) => download_bytes_from_urls(client, &[url])?,
        ArtifactDownloadSource::Urls(urls) => {
            let url_refs = urls.iter().map(String::as_str).collect::<Vec<_>>();
            download_bytes_from_urls(client, &url_refs)?
        }
    };
    cancellation.wait_if_paused();
    if cancellation.is_cancelled() {
        return Ok(ArtifactWriteOutcome::CancelledBeforeWrite);
    }

    write_bytes_atomically(Path::new(&download.artifact.path), &bytes)?;
    Ok(ArtifactWriteOutcome::Written(bytes.len() as i64))
}

fn download_bytes_from_urls(
    client: &mut impl ArtifactHttpClient,
    urls: &[&str],
) -> Result<Vec<u8>, ArtifactDownloadError> {
    let mut attempts = Vec::new();
    for url in urls {
        match client.get_bytes(url) {
            Ok(response) if (200..300).contains(&response.status) => {
                return Ok(response.bytes);
            }
            Ok(response) => {
                attempts.push(ArtifactHttpAttempt {
                    url: (*url).to_string(),
                    status: Some(response.status),
                    error_kind: None,
                });
            }
            Err(ArtifactDownloadError::Network(kind)) => {
                attempts.push(ArtifactHttpAttempt {
                    url: (*url).to_string(),
                    status: None,
                    error_kind: Some(kind),
                });
            }
            Err(error) => return Err(error),
        }
    }

    if let Some(status) = attempts.iter().rev().find_map(|attempt| attempt.status) {
        Err(ArtifactDownloadError::Http { status, attempts })
    } else {
        Err(ArtifactDownloadError::AttemptsFailed { attempts })
    }
}

fn write_bytes_atomically(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = partial_path(path);
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

fn partial_path(path: &Path) -> PathBuf {
    let mut file_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "artifact".into());
    file_name.push(".part");
    path.with_file_name(file_name)
}

fn cancel_artifacts_from(
    connection: &Connection,
    job_id: &str,
    downloads: &[PlannedArtifactDownload],
    first_size_bytes: Option<i64>,
    timestamp: i64,
) -> Result<(), ArtifactDownloadError> {
    for (index, download) in downloads.iter().enumerate() {
        let mut cancelled = download.artifact.clone();
        cancelled.status = "cancelled".to_string();
        if index == 0 {
            cancelled.size_bytes = first_size_bytes;
        }
        cancelled.updated_at = timestamp;
        upsert_artifact(connection, &cancelled)?;
        append_artifact_event(
            connection,
            job_id,
            "artifact.cancelled",
            format!("Cancelled {}.", artifact_display_name(&cancelled)),
            &cancelled.id,
            timestamp,
        )?;
    }
    Ok(())
}

fn append_artifact_event(
    connection: &Connection,
    job_id: &str,
    event_type: &str,
    message: String,
    artifact_id: &str,
    timestamp: i64,
) -> Result<(), ArtifactDownloadError> {
    append_artifact_event_with_payload(
        connection,
        job_id,
        event_type,
        message,
        serde_json::json!({ "artifactId": artifact_id }),
        timestamp,
    )
}

fn append_artifact_event_with_payload(
    connection: &Connection,
    job_id: &str,
    event_type: &str,
    message: String,
    payload: serde_json::Value,
    timestamp: i64,
) -> Result<(), ArtifactDownloadError> {
    append_job_event(
        connection,
        &NewJobEvent {
            job_id: job_id.to_string(),
            event_type: event_type.to_string(),
            message,
            payload_json: Some(payload.to_string()),
            created_at: timestamp,
        },
    )?;
    Ok(())
}

fn append_video_pacing_event(
    connection: &Connection,
    job_id: &str,
    artifact_id: &str,
    wait_seconds: u32,
) -> Result<(), ArtifactDownloadError> {
    let wait_started_at = current_unix_timestamp();
    let wait_until = wait_started_at.saturating_add(i64::from(wait_seconds));
    append_artifact_event_with_payload(
        connection,
        job_id,
        "video.pacing.wait",
        format!("Waiting {wait_seconds} seconds before the next video request."),
        serde_json::json!({
            "artifactId": artifact_id,
            "waitSeconds": wait_seconds,
            "waitStartedAt": wait_started_at,
            "waitUntil": wait_until,
        }),
        wait_started_at,
    )
}

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn append_artifact_source_event(
    connection: &Connection,
    job_id: &str,
    artifact_id: &str,
    source: &ArtifactDownloadSource,
    error: Option<&ArtifactDownloadError>,
    timestamp: i64,
) -> Result<(), ArtifactDownloadError> {
    append_job_event(
        connection,
        &NewJobEvent {
            job_id: job_id.to_string(),
            event_type: "artifact.source.diagnostic".to_string(),
            message: "Recorded safe artifact source diagnostic.".to_string(),
            payload_json: Some(
                serde_json::json!({
                    "artifactId": artifact_id,
                    "source": artifact_source_summary(source),
                    "failure": error.map(artifact_error_summary),
                })
                .to_string(),
            ),
            created_at: timestamp,
        },
    )?;
    Ok(())
}

fn artifact_error_summary(error: &ArtifactDownloadError) -> serde_json::Value {
    match error {
        ArtifactDownloadError::Http { status, attempts } => {
            serde_json::json!({
                "kind": "http",
                "status": status,
                "attempts": artifact_attempts_summary(attempts),
            })
        }
        ArtifactDownloadError::AttemptsFailed { attempts } => serde_json::json!({
            "kind": "attempts_failed",
            "attempts": artifact_attempts_summary(attempts),
        }),
        ArtifactDownloadError::Network(_) => serde_json::json!({ "kind": "network" }),
        ArtifactDownloadError::Io(error) => {
            serde_json::json!({ "kind": "io", "errorKind": error.kind().to_string() })
        }
        ArtifactDownloadError::Cache(_) => serde_json::json!({ "kind": "cache" }),
        ArtifactDownloadError::JobNotActive { .. } => {
            serde_json::json!({ "kind": "job_not_active" })
        }
    }
}

fn artifact_attempts_summary(attempts: &[ArtifactHttpAttempt]) -> Vec<serde_json::Value> {
    attempts
        .iter()
        .map(|attempt| {
            serde_json::json!({
                "status": attempt.status,
                "errorKind": attempt.error_kind,
                "url": summarize_url_source(&attempt.url),
            })
        })
        .collect()
}

fn artifact_source_summary(source: &ArtifactDownloadSource) -> serde_json::Value {
    match source {
        ArtifactDownloadSource::Text(_) => serde_json::json!({ "kind": "text" }),
        ArtifactDownloadSource::Url(url) => summarize_url_source(url),
        ArtifactDownloadSource::Urls(urls) => {
            let summaries = urls
                .iter()
                .map(|url| summarize_url_source(url))
                .collect::<Vec<_>>();
            serde_json::json!({ "kind": "urls", "count": urls.len(), "urls": summaries })
        }
    }
}

fn summarize_url_source(value: &str) -> serde_json::Value {
    let Ok(url) = Url::parse(value) else {
        return serde_json::json!({ "kind": "url", "valid": false });
    };
    let query_keys = url
        .query_pairs()
        .map(|(key, _)| key.to_string())
        .collect::<Vec<_>>();
    let query_count = query_keys.len();
    let file_name = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.trim().is_empty())
        .unwrap_or("");

    serde_json::json!({
        "kind": "url",
        "valid": true,
        "host": url.host_str().unwrap_or(""),
        "path": url.path(),
        "fileName": file_name,
        "queryKeys": query_keys,
        "queryCount": query_count,
        "isAmbry": url.host_str().is_some_and(|host| host.eq_ignore_ascii_case("www.linkedin.com") || host.eq_ignore_ascii_case("linkedin.com")) && url.path().eq_ignore_ascii_case("/ambry/"),
    })
}

fn is_exercise_artifact(artifact: &ArtifactRecord) -> bool {
    matches!(
        artifact.artifact_type.as_str(),
        "exercise_zip" | "exercise_file"
    )
}

fn is_exercise_zip_artifact(artifact: &ArtifactRecord) -> bool {
    artifact.artifact_type == "exercise_zip"
}

fn safe_artifact_error_reason(error: &ArtifactDownloadError) -> String {
    match error {
        ArtifactDownloadError::Http { status, .. } => format!("HTTP status {status}"),
        ArtifactDownloadError::AttemptsFailed { attempts } => attempts
            .iter()
            .rev()
            .find_map(|attempt| attempt.status)
            .map(|status| format!("HTTP status {status}"))
            .unwrap_or_else(|| "network request failed".to_string()),
        ArtifactDownloadError::Network(_) => "network request failed".to_string(),
        ArtifactDownloadError::Io(error) => format!("file write failed: {}", error.kind()),
        ArtifactDownloadError::Cache(_) => "cache update failed".to_string(),
        ArtifactDownloadError::JobNotActive { .. } => "job was not active".to_string(),
    }
}

fn artifact_display_name(artifact: &ArtifactRecord) -> String {
    Path::new(&artifact.path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(artifact.artifact_type.as_str())
        .to_string()
}

fn format_extraction_success_message(result: &ExerciseArchiveExtractionResult) -> String {
    match &result.destination_directory {
        Some(destination_directory) => format!(
            "Extracted exercise archive to {}.",
            destination_directory
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("exercise files")
        ),
        None => "Extracted exercise archive.".to_string(),
    }
}

fn format_extraction_failure_message(result: &ExerciseArchiveExtractionResult) -> String {
    match &result.message {
        Some(message) => format!("Exercise zip extraction failed: {message}"),
        None => "Exercise zip extraction failed.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{
        get_job, initialize, insert_job, list_artifacts_for_job, list_job_events, JobRecord,
    };
    use rusqlite::Connection;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::io::{Cursor, Write};
    use std::rc::Rc;
    use tempfile::tempdir;
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn initialized_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn downloads_url_and_text_artifacts_marks_job_completed() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let video_path = output.path().join("Sample Course").join("welcome.mp4");
        let subtitle_path = output.path().join("Sample Course").join("welcome.srt");
        let downloads = vec![
            planned_url(
                "artifact-video",
                "job-1",
                "video",
                &video_path,
                "https://cdn/video.mp4",
            ),
            planned_text(
                "artifact-subtitle",
                "job-1",
                "subtitle",
                &subtitle_path,
                "1\ntext\n",
            ),
        ];
        let mut client = FakeArtifactClient::new(vec![("https://cdn/video.mp4", 200, b"video")]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 2,
                failed: 0,
                cancelled: 0
            }
        );
        assert_eq!(job.status, "completed");
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
        assert_eq!(fs::read_to_string(&subtitle_path).unwrap(), "1\ntext\n");
        assert!(!video_path.with_file_name("welcome.mp4.part").exists());
        assert_eq!(artifacts.len(), 2);
        assert!(artifacts
            .iter()
            .all(|artifact| artifact.status == "completed"));
        assert!(events
            .iter()
            .any(|event| event.event_type == "artifact.completed"));
        assert!(events
            .iter()
            .filter(|event| event.event_type.starts_with("artifact."))
            .all(|event| !event.message.contains(&output.path().display().to_string())));
        assert!(events
            .iter()
            .any(|event| event.message == "Started welcome.mp4."));
        assert!(events
            .iter()
            .any(|event| event.message == "Completed welcome.mp4."));
        assert!(events
            .iter()
            .any(|event| event.event_type == "job.completed"));
    }

    #[test]
    fn video_requests_wait_once_between_actual_video_downloads() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let first_video = output.path().join("Sample Course").join("first.mp4");
        let subtitle = output.path().join("Sample Course").join("first.srt");
        let second_video = output.path().join("Sample Course").join("second.mp4");
        let downloads = vec![
            planned_url(
                "artifact-video-1",
                "job-1",
                "video",
                &first_video,
                "https://cdn/first.mp4",
            ),
            planned_text(
                "artifact-subtitle",
                "job-1",
                "subtitle",
                &subtitle,
                "1\ntext\n",
            ),
            planned_url(
                "artifact-video-2",
                "job-1",
                "video",
                &second_video,
                "https://cdn/second.mp4",
            ),
        ];
        let mut client = FakeArtifactClient::new(vec![
            ("https://cdn/first.mp4", 200, b"first"),
            ("https://cdn/second.mp4", 200, b"second"),
        ]);
        let mut pacing = VideoPacingPolicy::new(20, 40, 7);
        let mut observed_waits = Vec::new();
        let mut waiter = |seconds: u32, _cancellation: &dyn CancellationFlag| {
            observed_waits.push(seconds);
            true
        };

        let summary = download_artifacts_for_active_job_with_pacing(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
            &mut pacing,
            &mut waiter,
        )
        .unwrap();

        assert_eq!(summary.completed, 3);
        assert_eq!(observed_waits.len(), 1);
        assert!((20..=40).contains(&observed_waits[0]));
        let events = list_job_events(&connection, "job-1").unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "video.pacing.wait")
                .count(),
            1
        );
        let pacing_payload = events
            .iter()
            .find(|event| event.event_type == "video.pacing.wait")
            .and_then(|event| event.payload_json.as_deref())
            .and_then(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
            .unwrap();
        assert_eq!(pacing_payload["artifactId"], "artifact-video-2");
        assert_eq!(
            pacing_payload["waitUntil"].as_i64().unwrap()
                - pacing_payload["waitStartedAt"].as_i64().unwrap(),
            pacing_payload["waitSeconds"].as_i64().unwrap()
        );
    }

    #[test]
    fn video_pacing_is_bounded_and_varies_across_artifacts() {
        let mut pacing = VideoPacingPolicy::new(20, 40, 7);
        let waits = (0..100)
            .map(|index| pacing.next_wait_seconds(&format!("artifact-video-{index}")))
            .collect::<Vec<_>>();

        assert!(waits.iter().all(|seconds| (20..=40).contains(seconds)));
        assert!(waits.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn cancellation_during_video_pacing_stops_before_next_request() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let downloads = vec![
            planned_url(
                "artifact-video-1",
                "job-1",
                "video",
                &output.path().join("first.mp4"),
                "https://cdn/first.mp4",
            ),
            planned_url(
                "artifact-video-2",
                "job-1",
                "video",
                &output.path().join("second.mp4"),
                "https://cdn/second.mp4",
            ),
        ];
        let mut client = FakeArtifactClient::new(vec![("https://cdn/first.mp4", 200, b"first")]);
        let mut pacing = VideoPacingPolicy::new(20, 40, 11);
        let mut waiter = |_seconds: u32, _cancellation: &dyn CancellationFlag| false;

        let summary = download_artifacts_for_active_job_with_pacing(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
            &mut pacing,
            &mut waiter,
        )
        .unwrap();

        assert_eq!(summary.completed, 1);
        assert_eq!(summary.cancelled, 1);
        assert_eq!(
            get_job(&connection, "job-1").unwrap().unwrap().status,
            "cancelled"
        );
        assert!(!output.path().join("second.mp4").exists());
    }

    #[test]
    fn existing_downloaded_files_are_reused_on_retry_without_network_request() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let existing_video_path = output.path().join("Sample Course").join("welcome.mp4");
        let missing_video_path = output.path().join("Sample Course").join("next.mp4");
        fs::create_dir_all(existing_video_path.parent().unwrap()).unwrap();
        fs::write(&existing_video_path, b"already downloaded").unwrap();
        let downloads = vec![
            planned_url(
                "artifact-existing-video",
                "job-1",
                "video",
                &existing_video_path,
                "https://cdn/existing.mp4",
            ),
            planned_url(
                "artifact-missing-video",
                "job-1",
                "video",
                &missing_video_path,
                "https://cdn/missing.mp4",
            ),
        ];
        let mut client = FakeArtifactClient::new(vec![("https://cdn/missing.mp4", 200, b"new")]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 2,
                failed: 0,
                cancelled: 0
            }
        );
        assert_eq!(
            fs::read(&existing_video_path).unwrap(),
            b"already downloaded"
        );
        assert_eq!(fs::read(&missing_video_path).unwrap(), b"new");
        assert!(artifacts
            .iter()
            .all(|artifact| artifact.status == "completed"));
        assert!(events
            .iter()
            .any(|event| event.message == "Reused existing welcome.mp4."));
    }

    #[test]
    fn exercise_404_marks_artifact_failed_and_continues_remaining_downloads() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let video_path = output.path().join("Sample Course").join("welcome.mp4");
        let downloads = vec![
            planned_url(
                "artifact-exercise",
                "job-1",
                "exercise_zip",
                &exercise_path,
                "https://cdn/exercise.zip",
            ),
            planned_url(
                "artifact-video",
                "job-1",
                "video",
                &video_path,
                "https://cdn/video.mp4",
            ),
        ];
        let mut client = FakeArtifactClient::new(vec![
            ("https://cdn/exercise.zip", 404, b""),
            ("https://cdn/video.mp4", 200, b"video"),
        ]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 1,
                cancelled: 0
            }
        );
        assert_eq!(job.status, "completed");
        assert!(!exercise_path.exists());
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.id == "artifact-exercise")
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.id == "artifact-video")
                .unwrap()
                .status,
            "completed"
        );
        assert!(events.iter().any(|event| event.message
            == "Exercise artifact download failed and was skipped: HTTP status 404."));
        assert!(!events
            .iter()
            .any(|event| event.message.contains("exercise.zip?token=")));
    }

    #[test]
    fn exercise_non_404_download_failure_marks_artifact_failed_and_continues() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let video_path = output.path().join("Sample Course").join("welcome.mp4");
        let downloads = vec![
            planned_url(
                "artifact-exercise",
                "job-1",
                "exercise_zip",
                &exercise_path,
                "https://cdn/exercise.zip?token=do-not-render",
            ),
            planned_url(
                "artifact-video",
                "job-1",
                "video",
                &video_path,
                "https://cdn/video.mp4",
            ),
        ];
        let mut client = FakeArtifactClient::new(vec![
            ("https://cdn/exercise.zip?token=do-not-render", 403, b""),
            ("https://cdn/video.mp4", 200, b"video"),
        ]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 1,
                cancelled: 0
            }
        );
        assert_eq!(job.status, "completed");
        assert!(!exercise_path.exists());
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.id == "artifact-exercise")
                .unwrap()
                .status,
            "failed"
        );
        assert!(events.iter().any(|event| event.message
            == "Exercise artifact download failed and was skipped: HTTP status 403."));
        assert!(!events
            .iter()
            .any(|event| event.message.contains("do-not-render")));
        let diagnostic = events
            .iter()
            .find(|event| event.event_type == "artifact.source.diagnostic")
            .unwrap();
        assert!(!diagnostic
            .payload_json
            .as_deref()
            .unwrap()
            .contains("do-not-render"));
        assert!(diagnostic
            .payload_json
            .as_deref()
            .unwrap()
            .contains(r#""host":"cdn""#));
        assert!(diagnostic
            .payload_json
            .as_deref()
            .unwrap()
            .contains(r#""status":403"#));
    }

    #[test]
    fn exercise_url_list_tries_alternate_after_http_failure() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let zip_bytes = create_zip_bytes(&[("readme.txt", "hello")]);
        let downloads = vec![planned(
            "artifact-exercise",
            "job-1",
            "exercise_zip",
            &exercise_path,
            ArtifactDownloadSource::Urls(vec![
                "https://www.linkedin.com/ambry/?x-li-ambry-ep=bad&download=true".to_string(),
                "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh"
                    .to_string(),
            ]),
        )];
        let mut client = FakeArtifactClient::new_owned(vec![
            (
                "https://www.linkedin.com/ambry/?x-li-ambry-ep=bad&download=true",
                400,
                Vec::new(),
            ),
            (
                "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=fresh",
                200,
                zip_bytes,
            ),
        ]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 0,
                cancelled: 0
            }
        );
        assert!(!exercise_path.exists());
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.id == "artifact-exercise")
                .unwrap()
                .status,
            "completed"
        );
        assert!(events
            .iter()
            .any(|event| event.event_type == "artifact.extracted"));
    }

    #[test]
    fn exercise_url_list_tries_alternate_after_network_failure() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let zip_bytes = create_zip_bytes(&[("readme.txt", "hello")]);
        let downloads = vec![planned(
            "artifact-exercise",
            "job-1",
            "exercise_zip",
            &exercise_path,
            ArtifactDownloadSource::Urls(vec![
                "https://lilcdn-a.akamaihd.net/secure/courses/123/exercise.zip?hashval=secret"
                    .to_string(),
                "https://www.linkedin.com/ambry/?x-li-ambry-ep=fallback".to_string(),
            ]),
        )];
        let mut client = NetworkThenSuccessArtifactClient {
            expected_urls: VecDeque::from(vec![
                "https://lilcdn-a.akamaihd.net/secure/courses/123/exercise.zip?hashval=secret",
                "https://www.linkedin.com/ambry/?x-li-ambry-ep=fallback",
            ]),
            success_bytes: zip_bytes,
        };

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let events = list_job_events(&connection, "job-1").unwrap();
        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 0,
                cancelled: 0
            }
        );
        assert!(events
            .iter()
            .any(|event| event.event_type == "artifact.extracted"));
    }

    #[test]
    fn artifact_download_accepts_successful_partial_content_response() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let video_path = output.path().join("Sample Course").join("welcome.mp4");
        let downloads = vec![planned_url(
            "artifact-video",
            "job-1",
            "video",
            &video_path,
            "https://cdn.example.test/welcome.mp4",
        )];
        let mut client = FakeArtifactClient::new(vec![(
            "https://cdn.example.test/welcome.mp4",
            206,
            b"video",
        )]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 0,
                cancelled: 0
            }
        );
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
    }

    #[test]
    fn exercise_url_list_failure_records_sanitized_attempt_statuses() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let downloads = vec![planned(
            "artifact-exercise",
            "job-1",
            "exercise_zip",
            &exercise_path,
            ArtifactDownloadSource::Urls(vec![
                "https://www.linkedin.com/ambry/?x-li-ambry-ep=secret-one&download=true"
                    .to_string(),
                "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=secret-two"
                    .to_string(),
            ]),
        )];
        let mut client = FakeArtifactClient::new(vec![
            (
                "https://www.linkedin.com/ambry/?x-li-ambry-ep=secret-one&download=true",
                400,
                b"",
            ),
            (
                "https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=secret-two",
                403,
                b"",
            ),
        ]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let events = list_job_events(&connection, "job-1").unwrap();
        let diagnostic = events
            .iter()
            .find(|event| event.event_type == "artifact.source.diagnostic")
            .unwrap()
            .payload_json
            .as_deref()
            .unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 0,
                failed: 1,
                cancelled: 0
            }
        );
        assert!(diagnostic.contains(r#""status":400"#));
        assert!(diagnostic.contains(r#""status":403"#));
        assert!(diagnostic.contains(r#""queryKeys":["x-li-ambry-ep","download"]"#));
        assert!(diagnostic.contains(r#""queryKeys":["token"]"#));
        assert!(!diagnostic.contains("secret-one"));
        assert!(!diagnostic.contains("secret-two"));
    }

    #[test]
    fn exercise_zip_download_extracts_deletes_archive_and_records_events() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let zip_bytes = create_zip_bytes(&[("chapter-1/readme.txt", "hello")]);
        let zip_size = zip_bytes.len() as i64;
        let downloads = vec![planned_url(
            "artifact-exercise",
            "job-1",
            "exercise_zip",
            &exercise_path,
            "https://cdn/exercise.zip",
        )];
        let mut client =
            FakeArtifactClient::new_owned(vec![("https://cdn/exercise.zip", 200, zip_bytes)]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 0,
                cancelled: 0
            }
        );
        assert!(!exercise_path.exists());
        assert_eq!(
            fs::read_to_string(
                output
                    .path()
                    .join("Sample Course")
                    .join("exercise/chapter-1/readme.txt")
            )
            .unwrap(),
            "hello"
        );
        assert_eq!(artifacts[0].status, "completed");
        assert_eq!(artifacts[0].size_bytes, Some(zip_size));
        assert!(events
            .iter()
            .any(|event| event.event_type == "artifact.extracted"));
        assert!(events
            .iter()
            .any(|event| event.event_type == "artifact.completed"));
    }

    #[test]
    fn unsafe_exercise_zip_fails_artifact_keeps_zip_and_continues_remaining_downloads() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let root_name = output.path().file_name().unwrap().to_string_lossy();
        let outside_file_name = format!("{root_name}-outside.txt");
        let outside_path = output.path().parent().unwrap().join(&outside_file_name);
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let video_path = output.path().join("Sample Course").join("welcome.mp4");
        let zip_bytes = create_zip_bytes(&[(&format!("../{outside_file_name}"), "escape")]);
        let downloads = vec![
            planned_url(
                "artifact-exercise",
                "job-1",
                "exercise_zip",
                &exercise_path,
                "https://cdn/exercise.zip",
            ),
            planned_url(
                "artifact-video",
                "job-1",
                "video",
                &video_path,
                "https://cdn/video.mp4",
            ),
        ];
        let mut client = FakeArtifactClient::new_owned(vec![
            ("https://cdn/exercise.zip", 200, zip_bytes),
            ("https://cdn/video.mp4", 200, b"video".to_vec()),
        ]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &NeverCancelled,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 1,
                cancelled: 0
            }
        );
        assert_eq!(job.status, "completed");
        assert!(exercise_path.exists());
        assert!(!outside_path.exists());
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.id == "artifact-exercise")
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.id == "artifact-video")
                .unwrap()
                .status,
            "completed"
        );
        assert!(events.iter().any(|event| {
            event.event_type == "artifact.failed"
                && event.message.contains("Exercise zip extraction failed")
        }));
    }

    #[test]
    fn cancellation_after_artifact_response_cancels_before_writing_file() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let first_path = output.path().join("Sample Course").join("first.mp4");
        let second_path = output.path().join("Sample Course").join("second.mp4");
        let downloads = vec![
            planned_url(
                "artifact-first",
                "job-1",
                "video",
                &first_path,
                "https://cdn/first.mp4",
            ),
            planned_url(
                "artifact-second",
                "job-1",
                "video",
                &second_path,
                "https://cdn/second.mp4",
            ),
        ];
        let cancellation = SharedCancellation::new(false);
        let mut client = CancellingArtifactClient::new(
            vec![("https://cdn/first.mp4", 200, b"first")],
            cancellation.clone(),
        );

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &cancellation,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 0,
                failed: 0,
                cancelled: 2
            }
        );
        assert_eq!(job.status, "cancelled");
        assert!(!first_path.exists());
        assert!(!second_path.exists());
        assert!(artifacts
            .iter()
            .all(|artifact| artifact.status == "cancelled"));
    }

    #[test]
    fn cancellation_after_zip_download_keeps_zip_and_skips_extraction() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let extracted_path = output
            .path()
            .join("Sample Course")
            .join("exercise/chapter-1/readme.txt");
        let zip_bytes = create_zip_bytes(&[("chapter-1/readme.txt", "hello")]);
        let zip_size = zip_bytes.len() as i64;
        let downloads = vec![planned_url(
            "artifact-exercise",
            "job-1",
            "exercise_zip",
            &exercise_path,
            "https://cdn/exercise.zip",
        )];
        let cancellation = CancelAfterPolls::new(2);
        let mut client =
            FakeArtifactClient::new_owned(vec![("https://cdn/exercise.zip", 200, zip_bytes)]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &cancellation,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 0,
                failed: 0,
                cancelled: 1
            }
        );
        assert_eq!(job.status, "cancelled");
        assert!(exercise_path.exists());
        assert!(!extracted_path.exists());
        assert_eq!(artifacts[0].status, "cancelled");
        assert_eq!(artifacts[0].size_bytes, Some(zip_size));
    }

    #[test]
    fn cancellation_after_zip_extraction_keeps_artifact_completed_and_job_cancelled() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let exercise_path = output.path().join("Sample Course").join("exercise.zip");
        let extracted_path = output
            .path()
            .join("Sample Course")
            .join("exercise/chapter-1/readme.txt");
        let zip_bytes = create_zip_bytes(&[("chapter-1/readme.txt", "hello")]);
        let zip_size = zip_bytes.len() as i64;
        let downloads = vec![planned_url(
            "artifact-exercise",
            "job-1",
            "exercise_zip",
            &exercise_path,
            "https://cdn/exercise.zip",
        )];
        let cancellation = CancelAfterPolls::new(3);
        let mut client =
            FakeArtifactClient::new_owned(vec![("https://cdn/exercise.zip", 200, zip_bytes)]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &cancellation,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();
        let events = list_job_events(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 0,
                cancelled: 0
            }
        );
        assert_eq!(job.status, "cancelled");
        assert!(!exercise_path.exists());
        assert_eq!(fs::read_to_string(extracted_path).unwrap(), "hello");
        assert_eq!(artifacts[0].status, "completed");
        assert_eq!(artifacts[0].size_bytes, Some(zip_size));
        assert!(events
            .iter()
            .any(|event| event.event_type == "artifact.extracted"));
        assert!(events
            .iter()
            .any(|event| event.message == "Extracted exercise archive to exercise."));
        assert!(events
            .iter()
            .filter(|event| event.event_type.starts_with("artifact."))
            .all(|event| !event.message.contains(&output.path().display().to_string())));
        assert!(events
            .iter()
            .any(|event| event.event_type == "job.cancelled"));
    }

    #[test]
    fn cancellation_marks_job_and_remaining_artifacts_cancelled() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let first_path = output.path().join("Sample Course").join("first.mp4");
        let second_path = output.path().join("Sample Course").join("second.mp4");
        let downloads = vec![
            planned_url(
                "artifact-first",
                "job-1",
                "video",
                &first_path,
                "https://cdn/first.mp4",
            ),
            planned_url(
                "artifact-second",
                "job-1",
                "video",
                &second_path,
                "https://cdn/second.mp4",
            ),
        ];
        let mut client = FakeArtifactClient::new(vec![("https://cdn/first.mp4", 200, b"first")]);
        let cancellation = CancelAfterPolls::new(2);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &cancellation,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        let job = get_job(&connection, "job-1").unwrap().unwrap();
        let artifacts = list_artifacts_for_job(&connection, "job-1").unwrap();

        assert_eq!(
            summary,
            ArtifactDownloadSummary {
                completed: 1,
                failed: 0,
                cancelled: 1
            }
        );
        assert_eq!(job.status, "cancelled");
        assert_eq!(fs::read(&first_path).unwrap(), b"first");
        assert!(!second_path.exists());
        assert_eq!(
            artifacts
                .iter()
                .find(|artifact| artifact.id == "artifact-second")
                .unwrap()
                .status,
            "cancelled"
        );
    }

    #[test]
    fn active_download_waits_for_resume_and_finishes_with_atomic_file() {
        let connection = initialized_connection();
        let output = tempdir().unwrap();
        insert_job(&connection, &sample_job("job-1", "active", output.path())).unwrap();
        let video_path = output.path().join("Sample Course").join("welcome.mp4");
        let downloads = vec![planned_url(
            "artifact-video",
            "job-1",
            "video",
            &video_path,
            "https://cdn/video.mp4",
        )];
        let pause = PauseForPolls::new(3);
        let mut client = FakeArtifactClient::new(vec![("https://cdn/video.mp4", 200, b"video")]);

        let summary = download_artifacts_for_active_job(
            &connection,
            &mut client,
            &pause,
            "job-1",
            &downloads,
            200,
        )
        .unwrap();

        assert!(pause.polls.get() >= 3);
        assert_eq!(summary.completed, 1);
        assert_eq!(
            get_job(&connection, "job-1").unwrap().unwrap().status,
            "completed"
        );
        assert_eq!(fs::read(&video_path).unwrap(), b"video");
        assert!(!partial_path(&video_path).exists());
    }

    struct PauseForPolls {
        remaining: Cell<usize>,
        polls: Cell<usize>,
    }

    impl PauseForPolls {
        fn new(polls: usize) -> Self {
            Self {
                remaining: Cell::new(polls),
                polls: Cell::new(0),
            }
        }
    }

    impl CancellationFlag for PauseForPolls {
        fn is_cancelled(&self) -> bool {
            false
        }

        fn is_paused(&self) -> bool {
            self.polls.set(self.polls.get() + 1);
            let remaining = self.remaining.get();
            if remaining == 0 {
                return false;
            }
            self.remaining.set(remaining - 1);
            true
        }
    }

    #[derive(Clone)]
    struct SharedCancellation {
        cancelled: Rc<Cell<bool>>,
    }

    impl SharedCancellation {
        fn new(cancelled: bool) -> Self {
            Self {
                cancelled: Rc::new(Cell::new(cancelled)),
            }
        }

        fn cancel(&self) {
            self.cancelled.set(true);
        }
    }

    impl CancellationFlag for SharedCancellation {
        fn is_cancelled(&self) -> bool {
            self.cancelled.get()
        }
    }

    struct CancellingArtifactClient {
        inner: FakeArtifactClient,
        cancellation: SharedCancellation,
    }

    impl CancellingArtifactClient {
        fn new(
            responses: Vec<(&'static str, u16, &'static [u8])>,
            cancellation: SharedCancellation,
        ) -> Self {
            Self {
                inner: FakeArtifactClient::new(responses),
                cancellation,
            }
        }
    }

    impl ArtifactHttpClient for CancellingArtifactClient {
        fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError> {
            let response = self.inner.get_bytes(url)?;
            self.cancellation.cancel();
            Ok(response)
        }
    }

    struct FakeArtifactClient {
        responses: VecDeque<(&'static str, u16, Vec<u8>)>,
    }

    impl FakeArtifactClient {
        fn new(responses: Vec<(&'static str, u16, &'static [u8])>) -> Self {
            Self {
                responses: responses
                    .into_iter()
                    .map(|(url, status, bytes)| (url, status, bytes.to_vec()))
                    .collect(),
            }
        }

        fn new_owned(responses: Vec<(&'static str, u16, Vec<u8>)>) -> Self {
            Self {
                responses: responses.into(),
            }
        }
    }

    impl ArtifactHttpClient for FakeArtifactClient {
        fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError> {
            let Some((expected_url, status, bytes)) = self.responses.pop_front() else {
                panic!("unexpected request: {url}");
            };
            assert_eq!(url, expected_url);
            Ok(ArtifactHttpResponse { status, bytes })
        }
    }

    struct NetworkThenSuccessArtifactClient {
        expected_urls: VecDeque<&'static str>,
        success_bytes: Vec<u8>,
    }

    impl ArtifactHttpClient for NetworkThenSuccessArtifactClient {
        fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError> {
            let Some(expected_url) = self.expected_urls.pop_front() else {
                panic!("unexpected request: {url}");
            };
            assert_eq!(url, expected_url);
            if self.expected_urls.len() == 1 {
                return Err(ArtifactDownloadError::Network(
                    "simulated network failure".to_string(),
                ));
            }
            Ok(ArtifactHttpResponse {
                status: 200,
                bytes: self.success_bytes.clone(),
            })
        }
    }

    struct CancelAfterPolls {
        remaining_false_polls: Cell<usize>,
    }

    impl CancelAfterPolls {
        fn new(remaining_false_polls: usize) -> Self {
            Self {
                remaining_false_polls: Cell::new(remaining_false_polls),
            }
        }
    }

    impl CancellationFlag for CancelAfterPolls {
        fn is_cancelled(&self) -> bool {
            let remaining = self.remaining_false_polls.get();
            if remaining == 0 {
                true
            } else {
                self.remaining_false_polls.set(remaining - 1);
                false
            }
        }
    }

    fn sample_job(id: &str, status: &str, output_dir: &Path) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            course_slug: "sample-course".to_string(),
            source_url: "https://www.linkedin.com/learning/sample-course".to_string(),
            status: status.to_string(),
            selected_quality: "1080".to_string(),
            download_videos: true,
            download_exercises: true,
            download_subtitles: true,
            download_quizzes: true,
            quiz_hints_json: "[]".to_string(),
            output_dir: output_dir.to_string_lossy().to_string(),
            paused: false,
            scheduled_at: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    fn planned_url(
        id: &str,
        job_id: &str,
        artifact_type: &str,
        path: &Path,
        url: &str,
    ) -> PlannedArtifactDownload {
        planned(
            id,
            job_id,
            artifact_type,
            path,
            ArtifactDownloadSource::Url(url.to_string()),
        )
    }

    fn planned_text(
        id: &str,
        job_id: &str,
        artifact_type: &str,
        path: &Path,
        text: &str,
    ) -> PlannedArtifactDownload {
        planned(
            id,
            job_id,
            artifact_type,
            path,
            ArtifactDownloadSource::Text(text.to_string()),
        )
    }

    fn planned(
        id: &str,
        job_id: &str,
        artifact_type: &str,
        path: &Path,
        source: ArtifactDownloadSource,
    ) -> PlannedArtifactDownload {
        PlannedArtifactDownload {
            artifact: ArtifactRecord {
                id: id.to_string(),
                job_id: job_id.to_string(),
                artifact_type: artifact_type.to_string(),
                path: path.to_string_lossy().to_string(),
                status: "pending".to_string(),
                size_bytes: None,
                created_at: 200,
                updated_at: 200,
            },
            source,
        }
    }

    fn create_zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();

        for (entry_name, contents) in entries {
            zip.start_file(entry_name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }
}
