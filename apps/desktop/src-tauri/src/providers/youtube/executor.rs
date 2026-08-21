use crate::app::safe_output_filesystem::{
    validate_output_component, SafeOutputError, ValidatedOutputRoot,
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
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
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

struct AttemptCleanup<'a> {
    root: &'a ValidatedOutputRoot,
    path: Option<PathBuf>,
}

impl<'a> AttemptCleanup<'a> {
    fn new(root: &'a ValidatedOutputRoot, path: PathBuf) -> Self {
        Self {
            root,
            path: Some(path),
        }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for AttemptCleanup<'_> {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = self.root.discard_attempt(&path);
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
        let staging = self
            .output_root
            .staging_attempt_dir(&item.occurrence_id, &item.artifact_fingerprint)
            .map_err(|error| safe_output_error(error))?;
        let mut cleanup = AttemptCleanup::new(&self.output_root, staging.clone());
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
        let output = run(
            invocation(
                self.args_for(item, &output_template),
                MAX_RECORD_STDOUT_BYTES,
            ),
            Some(control),
            None,
        )
        .map_err(|error| transient_error("HELPER_FAILED", error.to_string()))?;
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
        let verified = verify_artifacts(&self.output_root, &staging, false)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
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
        write_manifest(&staging, self, item, &verified)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        validate_manifest_file(&staging, item)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        self.output_root
            .validate_attempt_contents(&staging)
            .map_err(|error| transient_error("OUTPUT_VERIFY_FAILED", error.to_string()))?;
        let rechecked = verify_artifacts(&self.output_root, &staging, true)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        if !same_artifacts(&verified, &rechecked) {
            return Err(transient_error(
                "OUTPUT_VERIFY_FAILED",
                "staging artifacts changed during publication verification",
            ));
        }
        if control.is_cancelled() {
            return Err(transient_error("CANCELLED", "download was cancelled"));
        }
        self.output_root
            .publish_attempt(&staging, &stem)
            .map_err(safe_output_error)?;
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
    output_root: &ValidatedOutputRoot,
    staging: &Path,
    allow_manifest: bool,
) -> Result<Vec<VerifiedArtifact>, String> {
    output_root
        .validate_attempt_contents(staging)
        .map_err(|error| error.to_string())?;
    let mut artifacts = Vec::new();
    for entry in fs::read_dir(staging).map_err(|error| error.to_string())? {
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
        let (size_bytes, sha256) = hash_regular_file(output_root, staging, &path)?;
        artifacts.push(VerifiedArtifact {
            name: name.to_string(),
            kind: artifact_kind(name),
            size_bytes,
            sha256,
        });
    }
    Ok(artifacts)
}

fn hash_regular_file(
    output_root: &ValidatedOutputRoot,
    staging: &Path,
    path: &Path,
) -> Result<(u64, String), String> {
    output_root
        .validate_attempt_contents(staging)
        .map_err(|error| error.to_string())?;
    let before = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !before.file_type().is_file() || before.len() == 0 {
        return Err(format!(
            "artifact is not a non-empty regular file: {}",
            path.display()
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
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
    let size_bytes = before.len();
    let after = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !after.file_type().is_file() || after.len() != size_bytes {
        return Err(format!(
            "artifact changed while it was verified: {}",
            path.display()
        ));
    }
    let _ = file.sync_all();
    output_root
        .validate_attempt_contents(staging)
        .map_err(|error| error.to_string())?;
    Ok((size_bytes, format!("{:x}", hasher.finalize())))
}

fn write_manifest(
    staging: &Path,
    executor: &YouTubeExecutor,
    item: &TransientWorkItem,
    artifacts: &[VerifiedArtifact],
) -> Result<(), String> {
    let manifest_path = staging.join("manifest.json");
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
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest_path)
        .map_err(|error| error.to_string())?;
    file.write_all(&payload)
        .map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    let _ = sync_directory_best_effort(staging);
    Ok(())
}

fn validate_manifest_file(staging: &Path, item: &TransientWorkItem) -> Result<(), String> {
    let path = staging.join("manifest.json");
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err("published manifest is not a non-empty regular file".to_string());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
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
        let staging = root
            .staging_attempt_dir(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        fs::write(
            staging.join("001-Example title-video-1.mp4"),
            b"media bytes",
        )
        .unwrap();
        let artifacts = verify_artifacts(&root, &staging, false).unwrap();
        write_manifest(&staging, &executor, &item, &artifacts).unwrap();
        root.validate_attempt_contents(&staging).unwrap();
        let rechecked = verify_artifacts(&root, &staging, true).unwrap();
        assert!(same_artifacts(&artifacts, &rechecked));

        let destination = root
            .publish_attempt(
                &staging,
                &safe_stem(item.ordinal, &item.title, &item.video_id),
            )
            .unwrap();
        assert!(destination.is_dir());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 1);
        assert_eq!(manifest["artifactFingerprint"], "fingerprint-1");
        assert_eq!(manifest["artifacts"][0]["sizeBytes"], 11);
        assert!(!temp
            .path()
            .join(".linkvault-staging")
            .join("youtube")
            .join("occurrence-1")
            .join("fingerprint-1")
            .exists());
    }

    #[test]
    fn failed_attempt_cleanup_leaves_no_visible_partial_directory() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let staging = root
            .staging_attempt_dir("occurrence-1", "fingerprint-1")
            .unwrap();
        fs::write(staging.join("partial.mp4"), b"partial").unwrap();
        {
            let _cleanup = AttemptCleanup::new(&root, staging.clone());
        }
        assert!(!staging.exists());
        assert!(!temp.path().join("001-video-video-1").exists());
    }
}
