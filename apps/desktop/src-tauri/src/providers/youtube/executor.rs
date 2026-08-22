use crate::app::safe_output_filesystem::{
    validate_output_component, OutputAttemptLease, SafeOutputError, ValidatedOutputRoot,
};
use crate::providers::youtube::error::YouTubeError;
use crate::providers::youtube::helper::{invocation, output_error, MAX_RECORD_STDOUT_BYTES};
use crate::providers::youtube::models::{StartYouTubeDownloadRequest, YouTubeDownloadMode};
use crate::workflow::transient::managed_process::run;
use crate::workflow::transient::{
    TransientError, TransientExecutionOutcome, TransientExecutor, TransientItemPhase,
    TransientProgressUpdate, TransientRunControl, TransientWorkItem,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct YouTubeExecutor {
    output_root: ValidatedOutputRoot,
    mode: YouTubeDownloadMode,
    max_height: Option<u16>,
    preferred_language: Option<String>,
    fallback_languages: Vec<String>,
    allow_automatic_captions: bool,
    continue_without_transcript: bool,
}

struct AttemptCleanup {
    root: ValidatedOutputRoot,
    lease: Option<OutputAttemptLease>,
}

impl AttemptCleanup {
    fn new(root: &ValidatedOutputRoot, lease: OutputAttemptLease) -> Self {
        Self {
            root: root.clone(),
            lease: Some(lease),
        }
    }

    fn lease(&self) -> &OutputAttemptLease {
        self.lease
            .as_ref()
            .expect("attempt cleanup lease must remain armed")
    }

    fn take_lease(&mut self) -> OutputAttemptLease {
        self.lease
            .take()
            .expect("attempt cleanup lease must remain armed")
    }

    fn disarm(&mut self) {
        self.lease = None;
    }
}

impl Drop for AttemptCleanup {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self.root.discard_attempt_lease(lease);
        }
    }
}

#[derive(Clone, Debug)]
struct VerifiedArtifact {
    name: String,
    kind: String,
    size_bytes: u64,
    sha256: String,
}

struct VerifiedLeafHandle {
    artifact: VerifiedArtifact,
    file: File,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct YouTubeArtifactManifest<'a> {
    schema_version: u32,
    provider: &'static str,
    artifact_fingerprint: &'a str,
    occurrence_id: &'a str,
    video_id: &'a str,
    ordinal: u32,
    source_url: &'a str,
    mode: &'static str,
    format_policy_version: u32,
    max_height: Option<u16>,
    preferred_language: Option<&'a str>,
    fallback_languages: &'a [String],
    allow_automatic_captions: bool,
    continue_without_transcript: bool,
    artifacts: Vec<ManifestArtifact<'a>>,
    status: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestArtifact<'a> {
    kind: &'a str,
    relative_path: &'a str,
    size_bytes: u64,
    sha256: &'a str,
}

impl YouTubeExecutor {
    pub fn new(
        output_root: ValidatedOutputRoot,
        request: &StartYouTubeDownloadRequest,
    ) -> Result<Arc<Self>, YouTubeError> {
        validate_options(request)?;
        Ok(Arc::new(Self {
            output_root,
            mode: request.mode.clone(),
            max_height: request.max_height,
            preferred_language: request.preferred_language.clone(),
            fallback_languages: request.fallback_languages.clone(),
            allow_automatic_captions: request.allow_automatic_captions,
            continue_without_transcript: request.continue_without_transcript,
        }))
    }

    fn args_for(&self, item: &TransientWorkItem, output_template: &Path) -> Vec<String> {
        let mut args = vec![
            "--ignore-config".to_string(),
            "--no-plugin-dirs".to_string(),
            "--no-update".to_string(),
            "--no-cache-dir".to_string(),
            "--no-warnings".to_string(),
            "--no-playlist".to_string(),
            "--newline".to_string(),
            "--progress-template".to_string(),
            "%(progress.downloaded_bytes)s/%(progress.total_bytes)s".to_string(),
            "--output".to_string(),
            output_template.to_string_lossy().into_owned(),
        ];
        match &self.mode {
            YouTubeDownloadMode::TranscriptOnly => {
                args.push("--skip-download".to_string());
                args.push("--write-subs".to_string());
                args.push("--sub-format".to_string());
                args.push("vtt".to_string());
                if self.allow_automatic_captions {
                    args.push("--write-auto-subs".to_string());
                }
                if let Some(languages) = self.language_filter() {
                    args.push("--sub-langs".to_string());
                    args.push(languages);
                }
            }
            YouTubeDownloadMode::VideoAndTranscript => {
                args.push("--write-subs".to_string());
                args.push("--sub-format".to_string());
                args.push("vtt".to_string());
                if self.allow_automatic_captions {
                    args.push("--write-auto-subs".to_string());
                }
                if let Some(languages) = self.language_filter() {
                    args.push("--sub-langs".to_string());
                    args.push(languages);
                }
                args.push("--format".to_string());
                args.push(self.format_selector());
            }
            YouTubeDownloadMode::VideoOnly => {
                args.push("--format".to_string());
                args.push(self.format_selector());
            }
        }
        args.push(item.source_url.clone());
        args
    }

    fn format_selector(&self) -> String {
        self.max_height.map_or_else(
            || "bestvideo*+bestaudio/best".to_string(),
            |height| format!("bestvideo[height<={height}]+bestaudio/best[height<={height}]"),
        )
    }

    fn language_filter(&self) -> Option<String> {
        let mut languages = Vec::new();
        if let Some(preferred) = self.preferred_language.as_deref() {
            languages.push(preferred.to_string());
        }
        languages.extend(self.fallback_languages.iter().cloned());
        if languages.is_empty() {
            None
        } else {
            Some(languages.join(","))
        }
    }
}

impl TransientExecutor for YouTubeExecutor {
    fn execute(
        &self,
        item: &TransientWorkItem,
        control: &TransientRunControl,
        progress: &mut dyn FnMut(TransientProgressUpdate),
    ) -> Result<TransientExecutionOutcome, TransientError> {
        if control.is_cancelled() {
            return Err(TransientError {
                code: "CANCELLED".to_string(),
                message: "download was cancelled".to_string(),
            });
        }
        let lease = self
            .output_root
            .staging_attempt_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .map_err(|error| safe_output_error(error))?;
        let staging = lease.path().to_path_buf();
        let mut cleanup = AttemptCleanup::new(&self.output_root, lease);
        if control.is_cancelled() {
            return Err(transient_error("CANCELLED", "download was cancelled"));
        }
        let stem = safe_stem(item.ordinal, &item.title, &item.video_id);
        let output_template = staging.join(format!("{stem}.%(ext)s"));
        progress(TransientProgressUpdate {
            occurrence_id: item.occurrence_id.clone(),
            phase: match &self.mode {
                YouTubeDownloadMode::TranscriptOnly => TransientItemPhase::Transcript,
                _ => TransientItemPhase::Media,
            },
            bytes_completed: None,
            bytes_total: None,
            fraction: Some(0.0),
        });
        cleanup
            .lease()
            .revalidate()
            .map_err(|error| transient_error("SAFE_FILESYSTEM_VIOLATION", error.to_string()))?;
        let output = run(
            invocation(
                self.args_for(item, &output_template),
                MAX_RECORD_STDOUT_BYTES,
            ),
            Some(control),
            None,
        )
        .map_err(|error| transient_error("HELPER_FAILED", error.to_string()))?;
        cleanup
            .lease()
            .revalidate()
            .map_err(|error| transient_error("SAFE_FILESYSTEM_VIOLATION", error.to_string()))?;
        if output.cancelled || control.is_cancelled() {
            return Err(transient_error("CANCELLED", "download was cancelled"));
        }
        if output.timed_out {
            return Err(transient_error("HELPER_TIMEOUT", "yt-dlp helper timed out"));
        }
        if !output.status.success() {
            return Err(transient_error("HELPER_FAILED", output_error(&output)));
        }
        if control.is_cancelled() {
            return Err(transient_error("CANCELLED", "download was cancelled"));
        }
        let (verified, initial_leaf_handles) = verify_artifacts(cleanup.lease(), false)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        drop(initial_leaf_handles);
        let has_media = verified.iter().any(|artifact| artifact.kind == "media");
        let has_transcript = verified.iter().any(|artifact| artifact.kind == "vtt");
        let transcript_missing = match &self.mode {
            YouTubeDownloadMode::TranscriptOnly => !has_transcript,
            YouTubeDownloadMode::VideoAndTranscript => !has_transcript,
            YouTubeDownloadMode::VideoOnly => false,
        };
        if verified.is_empty() {
            if self.continue_without_transcript
                && matches!(
                    &self.mode,
                    YouTubeDownloadMode::TranscriptOnly | YouTubeDownloadMode::VideoAndTranscript
                )
            {
                return Ok(TransientExecutionOutcome::warning(
                    "TRANSCRIPT_MISSING",
                    Vec::new(),
                ));
            }
            return Err(transient_error(
                "NO_ARTIFACT",
                "yt-dlp completed without a published artifact",
            ));
        }
        if matches!(
            &self.mode,
            YouTubeDownloadMode::VideoOnly | YouTubeDownloadMode::VideoAndTranscript
        ) && !has_media
        {
            return Err(transient_error(
                "NO_ARTIFACT",
                "yt-dlp completed without a media artifact",
            ));
        }
        if transcript_missing
            && !self.continue_without_transcript
            && matches!(
                &self.mode,
                YouTubeDownloadMode::TranscriptOnly | YouTubeDownloadMode::VideoAndTranscript
            )
        {
            return Err(transient_error(
                "NO_ARTIFACT",
                "yt-dlp completed without the requested transcript artifact",
            ));
        }
        if matches!(&self.mode, YouTubeDownloadMode::TranscriptOnly) && transcript_missing {
            return Ok(TransientExecutionOutcome::warning(
                "TRANSCRIPT_MISSING",
                Vec::new(),
            ));
        }
        let artifact_kinds = verified
            .iter()
            .map(|artifact| artifact.kind.clone())
            .collect::<Vec<_>>();
        write_manifest(cleanup.lease(), self, item, &verified)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let manifest_leaf = validate_manifest_file(cleanup.lease(), item)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        cleanup
            .lease()
            .validate_contents()
            .map_err(|error| transient_error("OUTPUT_VERIFY_FAILED", error.to_string()))?;
        let (rechecked, mut held_leaf_handles) = verify_artifacts(cleanup.lease(), true)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        held_leaf_handles.push(manifest_leaf);
        if !same_artifacts(&verified, &rechecked) {
            return Err(transient_error(
                "OUTPUT_VERIFY_FAILED",
                "staging artifacts changed during publication verification",
            ));
        }
        if control.is_cancelled() {
            return Err(transient_error("CANCELLED", "download was cancelled"));
        }
        verify_held_leaf_handles(&mut held_leaf_handles)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let published_expectations = held_leaf_handles
            .iter()
            .map(|leaf| leaf.artifact.clone())
            .collect::<Vec<_>>();
        // The helper has exited. Drop write-denying child handles immediately
        // before the atomic directory rename, then recheck the same hashes at
        // the published path before reporting completion.
        drop(held_leaf_handles);
        let destination = self
            .output_root
            .publish_attempt_lease(cleanup.take_lease(), &stem)
            .map_err(safe_output_error)?;
        verify_published_artifacts(&destination, &published_expectations)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        cleanup.disarm();
        let _ = sync_directory_best_effort(self.output_root.path());
        let outcome = if transcript_missing {
            TransientExecutionOutcome::warning("TRANSCRIPT_MISSING", artifact_kinds)
        } else {
            TransientExecutionOutcome::completed(artifact_kinds)
        };
        progress(TransientProgressUpdate {
            occurrence_id: item.occurrence_id.clone(),
            phase: TransientItemPhase::Completed,
            bytes_completed: None,
            bytes_total: None,
            fraction: Some(1.0),
        });
        Ok(outcome)
    }
}

fn validate_options(request: &StartYouTubeDownloadRequest) -> Result<(), YouTubeError> {
    if let Some(height) = request.max_height {
        if !matches!(height, 480 | 720 | 1080 | 1440 | 2160) {
            return Err(YouTubeError::new(
                "INVALID_REQUEST",
                "unsupported quality cap",
            ));
        }
    }
    for language in request
        .preferred_language
        .iter()
        .chain(request.fallback_languages.iter())
    {
        if language.is_empty()
            || language.len() > 32
            || !language.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(YouTubeError::new(
                "INVALID_REQUEST",
                "invalid transcript language",
            ));
        }
    }
    Ok(())
}

fn verify_artifacts(
    attempt: &OutputAttemptLease,
    allow_manifest: bool,
) -> Result<(Vec<VerifiedArtifact>, Vec<VerifiedLeafHandle>), String> {
    attempt
        .validate_contents()
        .map_err(|error| error.to_string())?;
    let mut artifacts = Vec::new();
    let mut handles = Vec::new();
    for entry in fs::read_dir(attempt.path()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("staging contained an unnamed artifact: {}", path.display()))?;
        validate_output_component(name).map_err(|error| error.to_string())?;
        if name == "manifest.json" {
            if allow_manifest {
                continue;
            }
            return Err("helper-created manifest is not accepted".to_string());
        }
        if is_transient_artifact_name(name) {
            return Err(format!("staging contained an incomplete artifact: {name}"));
        }
        let held = hash_regular_file(attempt, name)?;
        artifacts.push(held.artifact.clone());
        handles.push(held);
    }
    Ok((artifacts, handles))
}

fn hash_regular_file(
    attempt: &OutputAttemptLease,
    name: &str,
) -> Result<VerifiedLeafHandle, String> {
    let mut file = attempt.open_leaf(name).map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() || before.len() == 0 {
        return Err(format!(
            "artifact is not a non-empty regular file: {}",
            attempt.path().join(name).display()
        ));
    }
    let sha256 = hash_open_file(&mut file)?;
    let size_bytes = before.len();
    let after = file.metadata().map_err(|error| error.to_string())?;
    if !after.is_file() || after.len() != size_bytes {
        return Err(format!(
            "artifact changed while it was verified: {}",
            attempt.path().join(name).display()
        ));
    }
    attempt.revalidate().map_err(|error| error.to_string())?;
    Ok(VerifiedLeafHandle {
        artifact: VerifiedArtifact {
            name: name.to_string(),
            kind: artifact_kind(name),
            size_bytes,
            sha256,
        },
        file,
    })
}

fn write_manifest(
    attempt: &OutputAttemptLease,
    executor: &YouTubeExecutor,
    item: &TransientWorkItem,
    artifacts: &[VerifiedArtifact],
) -> Result<(), String> {
    let manifest_artifacts = artifacts
        .iter()
        .map(|artifact| ManifestArtifact {
            kind: &artifact.kind,
            relative_path: &artifact.name,
            size_bytes: artifact.size_bytes,
            sha256: &artifact.sha256,
        })
        .collect();
    let manifest = YouTubeArtifactManifest {
        schema_version: 1,
        provider: "youtube",
        artifact_fingerprint: &item.artifact_fingerprint,
        occurrence_id: &item.occurrence_id,
        video_id: &item.video_id,
        ordinal: item.ordinal,
        source_url: &item.source_url,
        mode: mode_name(&executor.mode),
        format_policy_version: 1,
        max_height: executor.max_height,
        preferred_language: executor.preferred_language.as_deref(),
        fallback_languages: &executor.fallback_languages,
        allow_automatic_captions: executor.allow_automatic_captions,
        continue_without_transcript: executor.continue_without_transcript,
        artifacts: manifest_artifacts,
        status: "verified",
    };
    let payload = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    let mut file = attempt
        .create_leaf("manifest.json")
        .map_err(|error| error.to_string())?;
    file.write_all(&payload)
        .map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    attempt.revalidate().map_err(|error| error.to_string())?;
    Ok(())
}

fn validate_manifest_file(
    attempt: &OutputAttemptLease,
    item: &TransientWorkItem,
) -> Result<VerifiedLeafHandle, String> {
    let mut file = attempt
        .open_leaf("manifest.json")
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("published manifest is not a non-empty regular file".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("published manifest is invalid JSON: {error}"))?;
    let matches = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(1)
        && value.get("provider").and_then(serde_json::Value::as_str) == Some("youtube")
        && value
            .get("artifactFingerprint")
            .and_then(serde_json::Value::as_str)
            == Some(item.artifact_fingerprint.as_str())
        && value
            .get("occurrenceId")
            .and_then(serde_json::Value::as_str)
            == Some(item.occurrence_id.as_str())
        && value.get("videoId").and_then(serde_json::Value::as_str) == Some(item.video_id.as_str())
        && value.get("status").and_then(serde_json::Value::as_str) == Some("verified");
    if !matches {
        return Err("published manifest identity does not match the work item".to_string());
    }
    let sha256 = hash_open_file(&mut file)?;
    attempt.revalidate().map_err(|error| error.to_string())?;
    Ok(VerifiedLeafHandle {
        artifact: VerifiedArtifact {
            name: "manifest.json".to_string(),
            kind: "metadata".to_string(),
            size_bytes: metadata.len(),
            sha256,
        },
        file,
    })
}

fn hash_open_file(file: &mut File) -> Result<String, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_held_leaf_handles(held: &mut [VerifiedLeafHandle]) -> Result<(), String> {
    for leaf in held {
        if leaf
            .file
            .metadata()
            .map_err(|error| error.to_string())?
            .len()
            != leaf.artifact.size_bytes
            || hash_open_file(&mut leaf.file)? != leaf.artifact.sha256
        {
            return Err(format!(
                "verified artifact handle changed before publication: {}",
                leaf.artifact.name
            ));
        }
    }
    Ok(())
}

fn verify_published_artifacts(
    destination: &Path,
    expected: &[VerifiedArtifact],
) -> Result<(), String> {
    let mut published_names = fs::read_dir(destination)
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry
                .map_err(|error| error.to_string())?
                .file_name()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "published output contained an unnamed artifact".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected_names = expected
        .iter()
        .map(|artifact| artifact.name.clone())
        .collect::<Vec<_>>();
    published_names.sort();
    expected_names.sort();
    if published_names != expected_names {
        return Err("published artifact names changed during publication".to_string());
    }
    for artifact in expected {
        let path = destination.join(&artifact.name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(format!(
                "published artifact is not a regular file: {}",
                artifact.name
            ));
        }
        let mut published = File::open(&path).map_err(|error| error.to_string())?;
        if published
            .metadata()
            .map_err(|error| error.to_string())?
            .len()
            != artifact.size_bytes
            || hash_open_file(&mut published)? != artifact.sha256
        {
            return Err(format!(
                "published artifact does not match its verified handle: {}",
                artifact.name
            ));
        }
    }
    Ok(())
}

fn sync_directory_best_effort(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn same_artifacts(left: &[VerifiedArtifact], right: &[VerifiedArtifact]) -> bool {
    let mut left = left
        .iter()
        .map(|artifact| {
            (
                artifact.name.clone(),
                artifact.kind.clone(),
                artifact.size_bytes,
                artifact.sha256.clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut right = right
        .iter()
        .map(|artifact| {
            (
                artifact.name.clone(),
                artifact.kind.clone(),
                artifact.size_bytes,
                artifact.sha256.clone(),
            )
        })
        .collect::<Vec<_>>();
    left.sort();
    right.sort();
    left == right
}

fn is_transient_artifact_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".part", ".ytdl", ".tmp", ".temp"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn safe_stem(ordinal: u32, title: &str, video_id: &str) -> String {
    let mut stem = title
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    if stem.is_empty() {
        stem = "youtube-video".to_string();
    }
    stem = truncate_utf16(&stem, 80);
    stem = stem
        .trim()
        .trim_matches(|character| character == '.' || character == ' ')
        .to_string();
    if stem.is_empty() {
        stem = "youtube-video".to_string();
    }
    let safe_video_id = video_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{:03}-{}-{}", ordinal, stem, safe_video_id)
}

fn truncate_utf16(value: &str, max_units: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let width = character.len_utf16();
            if units + width > max_units {
                false
            } else {
                units += width;
                true
            }
        })
        .collect()
}

fn artifact_kind(name: &str) -> String {
    match Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "vtt" | "srt" => "vtt".to_string(),
        "json" => "metadata".to_string(),
        _ => "media".to_string(),
    }
}

fn mode_name(mode: &YouTubeDownloadMode) -> &'static str {
    match mode {
        YouTubeDownloadMode::VideoAndTranscript => "video_and_transcript",
        YouTubeDownloadMode::VideoOnly => "video_only",
        YouTubeDownloadMode::TranscriptOnly => "transcript_only",
    }
}

fn transient_error(code: impl Into<String>, message: impl Into<String>) -> TransientError {
    TransientError {
        code: code.into(),
        message: message.into(),
    }
}

fn safe_output_error(error: SafeOutputError) -> TransientError {
    let code = match &error {
        SafeOutputError::OutputCollision { .. } => "OUTPUT_COLLISION",
        SafeOutputError::PathTooLong | SafeOutputError::InvalidChildName => "OUTPUT_PATH_INVALID",
        SafeOutputError::UnsafeDescendant { .. } => "OUTPUT_VERIFY_FAILED",
        _ => "OUTPUT_ROOT_INVALID",
    };
    transient_error(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::safe_output_filesystem::validate_output_root;
    use tempfile::tempdir;

    #[test]
    fn verified_manifest_and_complete_attempt_publish_atomically() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let request = StartYouTubeDownloadRequest {
            client_submission_id: "submission-1".to_string(),
            scan_plan_id: "plan-1".to_string(),
            selected_occurrence_ids: vec!["occurrence-1".to_string()],
            output_dir: temp.path().to_string_lossy().into_owned(),
            mode: YouTubeDownloadMode::VideoOnly,
            max_height: Some(720),
            preferred_language: None,
            fallback_languages: Vec::new(),
            allow_automatic_captions: false,
            continue_without_transcript: false,
        };
        let executor = YouTubeExecutor::new(root.clone(), &request).unwrap();
        let item = TransientWorkItem {
            occurrence_id: "occurrence-1".to_string(),
            artifact_fingerprint: "fingerprint-1".to_string(),
            video_id: "video-1".to_string(),
            ordinal: 1,
            title: "Example title".to_string(),
            source_url: "https://www.youtube.com/watch?v=video-1".to_string(),
        };
        let args = executor.args_for(&item, &temp.path().join("output.%(ext)s"));
        for required in ["--ignore-config", "--no-plugin-dirs", "--no-cache-dir"] {
            assert!(args.iter().any(|argument| argument == required));
        }
        let attempt = root
            .staging_attempt_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        let staging = attempt.path().to_path_buf();
        let mut cleanup = AttemptCleanup::new(&root, attempt);
        fs::write(
            staging.join("001-Example title-video-1.mp4"),
            b"media bytes",
        )
        .unwrap();
        let (artifacts, initial_handles) = verify_artifacts(cleanup.lease(), false).unwrap();
        drop(initial_handles);
        write_manifest(cleanup.lease(), &executor, &item, &artifacts).unwrap();
        let manifest_handle = validate_manifest_file(cleanup.lease(), &item).unwrap();
        cleanup.lease().validate_contents().unwrap();
        let (rechecked, mut held_handles) = verify_artifacts(cleanup.lease(), true).unwrap();
        held_handles.push(manifest_handle);
        assert!(same_artifacts(&artifacts, &rechecked));
        verify_held_leaf_handles(&mut held_handles).unwrap();
        let expectations = held_handles
            .iter()
            .map(|leaf| leaf.artifact.clone())
            .collect::<Vec<_>>();
        drop(held_handles);

        let destination = root
            .publish_attempt_lease(
                cleanup.take_lease(),
                &safe_stem(item.ordinal, &item.title, &item.video_id),
            )
            .unwrap();
        verify_published_artifacts(&destination, &expectations).unwrap();
        assert!(destination.is_dir());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 1);
        assert_eq!(manifest["artifactFingerprint"], "fingerprint-1");
        assert_eq!(manifest["artifacts"][0]["sizeBytes"], 11);
        let fingerprint_staging = temp
            .path()
            .join(".linkvault-staging")
            .join("youtube")
            .join("occurrence-1")
            .join("fingerprint-1");
        assert_eq!(fs::read_dir(fingerprint_staging).unwrap().count(), 0);
    }

    #[test]
    fn failed_attempt_cleanup_leaves_no_visible_partial_directory() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let lease = root
            .staging_attempt_lease("occurrence-1", "fingerprint-1")
            .unwrap();
        fs::write(lease.path().join("partial.mp4"), b"partial").unwrap();
        let staging = lease.path().to_path_buf();
        {
            let _cleanup = AttemptCleanup::new(&root, lease);
        }
        assert!(!staging.exists());
        assert!(!temp.path().join("001-video-video-1").exists());
    }
}
