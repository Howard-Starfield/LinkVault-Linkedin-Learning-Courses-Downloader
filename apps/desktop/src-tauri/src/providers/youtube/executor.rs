use crate::app::safe_output_filesystem::{
    validate_output_component, ExistingOutputDirectoryLease, OutputAttemptLease, SafeOutputError,
    ValidatedOutputRoot,
};
use crate::providers::youtube::error::YouTubeError;
use crate::providers::youtube::helper::{
    ffprobe_invocation, invocation, output_error, MAX_RECORD_STDOUT_BYTES,
};
use crate::providers::youtube::manifest_contract::{
    artifact_fingerprint, canonical_manifest_bytes, project_manifest, ArtifactFingerprintInput,
    ManifestArtifact as StrictManifestArtifact, ManifestArtifactKind, ManifestProjectionInput,
    SelectedTranscript, FORMAT_POLICY_VERSION,
};
use crate::providers::youtube::media_verifier::{
    verify_ffprobe_json, MediaMode, PLAYBACK_COMPATIBILITY_WARNING,
};
use crate::providers::youtube::models::{
    StartYouTubeDownloadRequest, YouTubeDownloadMode, YouTubeTranscriptSource,
};
use crate::providers::youtube::transcript_normalizer::{
    normalize_vtt_json, TranscriptMetadata, TranscriptNormalizationError, MAX_VTT_BYTES,
};
use crate::workflow::transient::managed_process::run;
use crate::workflow::transient::{
    TransientError, TransientExecutionOutcome, TransientExecutor, TransientItemPhase,
    TransientProgressUpdate, TransientRunControl, TransientWorkItem,
};
#[cfg(test)]
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct YouTubeExecutor {
    output_root: ValidatedOutputRoot,
    mode: YouTubeDownloadMode,
    max_height: Option<u16>,
    #[cfg(test)]
    preferred_language: Option<String>,
    #[cfg(test)]
    fallback_languages: Vec<String>,
    allow_automatic_captions: bool,
    continue_without_transcript: bool,
    context: Option<YouTubeExecutorContext>,
}

/// Immutable execution facts captured by the command layer after scan and
/// transcript inspection.  The executor never derives a transcript choice
/// from language preferences alone: transcript modes require one exact
/// occurrence-level selection and the helper-lock/source identities that were
/// used to compute the work-item fingerprint.
#[derive(Clone, Debug, Default)]
pub struct YouTubeExecutorContext {
    pub source_snapshot_digest: String,
    pub playlist_id: Option<String>,
    pub helper_lock_digest: String,
    pub items: Vec<YouTubeExecutorItemContext>,
}

#[derive(Clone, Debug)]
pub struct YouTubeExecutorItemContext {
    pub occurrence_id: String,
    pub playlist_index: Option<u32>,
    pub source_duration_seconds: Option<u64>,
    pub selected_transcript: Option<SelectedTranscript>,
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

#[cfg(test)]
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

#[cfg(test)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestArtifact<'a> {
    kind: &'a str,
    relative_path: &'a str,
    size_bytes: u64,
    sha256: &'a str,
}

impl YouTubeExecutor {
    #[cfg(test)]
    pub fn new(
        output_root: ValidatedOutputRoot,
        request: &StartYouTubeDownloadRequest,
    ) -> Result<Arc<Self>, YouTubeError> {
        validate_options(request)?;
        Ok(Arc::new(Self {
            output_root,
            mode: request.mode.clone(),
            max_height: request.max_height,
            #[cfg(test)]
            preferred_language: request.preferred_language.clone(),
            #[cfg(test)]
            fallback_languages: request.fallback_languages.clone(),
            allow_automatic_captions: request.allow_automatic_captions,
            continue_without_transcript: request.continue_without_transcript,
            context: None,
        }))
    }

    /// Constructs an executor with the immutable scan/inspection facts needed
    /// by transcript-only publication.  `new` remains available for the
    /// existing media path; callers admitting transcript work should use this
    /// constructor so missing policy fails closed before helper launch.
    pub fn new_with_context(
        output_root: ValidatedOutputRoot,
        request: &StartYouTubeDownloadRequest,
        context: YouTubeExecutorContext,
    ) -> Result<Arc<Self>, YouTubeError> {
        validate_options(request)?;
        Ok(Arc::new(Self {
            output_root,
            mode: request.mode.clone(),
            max_height: request.max_height,
            #[cfg(test)]
            preferred_language: request.preferred_language.clone(),
            #[cfg(test)]
            fallback_languages: request.fallback_languages.clone(),
            allow_automatic_captions: request.allow_automatic_captions,
            continue_without_transcript: request.continue_without_transcript,
            context: Some(context),
        }))
    }

    fn args_for(
        &self,
        item: &TransientWorkItem,
        output_template: &Path,
    ) -> Result<Vec<String>, String> {
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
                let selected = self.selected_transcript(item)?;
                args.push("--skip-download".to_string());
                args.push(match selected.source {
                    YouTubeTranscriptSource::Uploader => "--write-subs".to_string(),
                    YouTubeTranscriptSource::Automatic => "--write-auto-subs".to_string(),
                });
                args.push("--sub-format".to_string());
                args.push("vtt".to_string());
                args.push("--sub-langs".to_string());
                args.push(selected.language_tag);
            }
            YouTubeDownloadMode::VideoAndTranscript => {
                if let Some(selected) = self.item_selected_transcript(item)? {
                    args.push(match selected.source {
                        YouTubeTranscriptSource::Uploader => "--write-subs".to_string(),
                        YouTubeTranscriptSource::Automatic => "--write-auto-subs".to_string(),
                    });
                    args.push("--sub-format".to_string());
                    args.push("vtt".to_string());
                    args.push("--sub-langs".to_string());
                    args.push(selected.language_tag);
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
        Ok(args)
    }

    fn item_context(
        &self,
        item: &TransientWorkItem,
    ) -> Result<&YouTubeExecutorItemContext, String> {
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| "transcript execution policy is unavailable".to_string())?;
        context
            .items
            .iter()
            .find(|candidate| candidate.occurrence_id == item.occurrence_id)
            .ok_or_else(|| {
                format!(
                    "transcript execution policy is missing occurrence {}",
                    item.occurrence_id
                )
            })
    }

    fn selected_transcript(&self, item: &TransientWorkItem) -> Result<SelectedTranscript, String> {
        self.item_selected_transcript(item)?
            .ok_or_else(|| "transcript execution policy has no selected track".to_string())
    }

    fn item_selected_transcript(
        &self,
        item: &TransientWorkItem,
    ) -> Result<Option<SelectedTranscript>, String> {
        let selected = self.item_context(item)?.selected_transcript.clone();
        let Some(selected) = selected else {
            self.validate_item_fingerprint(item, None)?;
            return Ok(None);
        };
        if selected.language_tag.is_empty()
            || selected.language_tag.len() > 32
            || !selected.language_tag.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err("transcript execution policy has an invalid language tag".to_string());
        }
        if selected.source == YouTubeTranscriptSource::Automatic && !self.allow_automatic_captions {
            return Err("automatic captions are not enabled by the request".to_string());
        }
        self.validate_item_fingerprint(item, Some(selected.clone()))?;
        Ok(Some(selected))
    }

    fn validate_item_fingerprint(
        &self,
        item: &TransientWorkItem,
        selected_transcript: Option<SelectedTranscript>,
    ) -> Result<(), String> {
        let helper_lock_digest = self
            .context
            .as_ref()
            .ok_or_else(|| "transcript execution policy is unavailable".to_string())?
            .helper_lock_digest
            .clone();
        let expected_fingerprint = artifact_fingerprint(&ArtifactFingerprintInput {
            occurrence_id: item.occurrence_id.clone(),
            video_id: item.video_id.clone(),
            mode: self.mode.clone(),
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: match self.mode {
                YouTubeDownloadMode::TranscriptOnly => None,
                _ => self.max_height,
            },
            selected_transcript,
            helper_lock_digest,
        })
        .map_err(|error| error.to_string())?;
        if expected_fingerprint != item.artifact_fingerprint {
            return Err(
                "transcript execution policy does not match the work-item fingerprint".to_string(),
            );
        }
        Ok(())
    }

    fn strict_manifest(
        &self,
        item: &TransientWorkItem,
        artifacts: &[VerifiedArtifact],
    ) -> Result<crate::providers::youtube::manifest_contract::YouTubeArtifactManifest, String> {
        let context = self.item_context(item)?;
        let selected = self.item_selected_transcript(item)?;
        let mut manifest_artifacts = artifacts
            .iter()
            .map(|artifact| {
                Ok(StrictManifestArtifact {
                    kind: manifest_artifact_kind(&artifact.kind)?,
                    relative_path: artifact.name.clone(),
                    size_bytes: artifact.size_bytes,
                    sha256: artifact.sha256.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        manifest_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        project_manifest(ManifestProjectionInput {
            source_snapshot_digest: self
                .context
                .as_ref()
                .ok_or_else(|| "transcript execution policy is unavailable".to_string())?
                .source_snapshot_digest
                .clone(),
            artifact_fingerprint: item.artifact_fingerprint.clone(),
            occurrence_id: item.occurrence_id.clone(),
            video_id: item.video_id.clone(),
            playlist_id: self
                .context
                .as_ref()
                .and_then(|value| value.playlist_id.clone()),
            playlist_index: context.playlist_index,
            mode: self.mode.clone(),
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: match self.mode {
                YouTubeDownloadMode::TranscriptOnly => None,
                _ => self.max_height,
            },
            selected_transcript: selected,
            helper_lock_digest: self
                .context
                .as_ref()
                .ok_or_else(|| "transcript execution policy is unavailable".to_string())?
                .helper_lock_digest
                .clone(),
            artifacts: manifest_artifacts,
        })
        .map_err(|error| error.to_string())
    }

    fn try_reuse_existing(
        &self,
        item: &TransientWorkItem,
        final_name: &str,
        control: &TransientRunControl,
    ) -> Result<Option<Vec<String>>, TransientError> {
        let Some(lease) = self
            .output_root
            .existing_item_lease(final_name)
            .map_err(safe_output_error)?
        else {
            return Ok(None);
        };
        verify_existing_item(&lease, self, item, control)
            .map(Some)
            .map_err(|message| {
                transient_error(
                    "OUTPUT_COLLISION",
                    format!("existing output is not an exact verified match: {message}"),
                )
            })
    }

    fn format_selector(&self) -> String {
        self.max_height.map_or_else(
            || "bestvideo*+bestaudio/best".to_string(),
            |height| format!("bestvideo[height<={height}]+bestaudio/best[height<={height}]"),
        )
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
        let stem = safe_stem(item.ordinal, &item.title, &item.video_id);
        if let Some(artifact_kinds) = self.try_reuse_existing(item, &stem, control)? {
            progress(TransientProgressUpdate {
                occurrence_id: item.occurrence_id.clone(),
                phase: TransientItemPhase::Completed,
                bytes_completed: None,
                bytes_total: None,
                fraction: Some(1.0),
            });
            return Ok(TransientExecutionOutcome::skipped_existing(artifact_kinds));
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
        let helper_args = self
            .args_for(item, &output_template)
            .map_err(|message| transient_error("TRANSCRIPT_POLICY_MISSING", message))?;
        let output = run(
            invocation(helper_args, MAX_RECORD_STDOUT_BYTES),
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
        if matches!(
            &self.mode,
            YouTubeDownloadMode::TranscriptOnly | YouTubeDownloadMode::VideoAndTranscript
        ) {
            let selected = self
                .item_selected_transcript(item)
                .map_err(|message| transient_error("TRANSCRIPT_POLICY_MISSING", message))?;
            match selected
                .as_ref()
                .map(|selected| post_process_transcript(cleanup.lease(), item, selected))
                .transpose()
            {
                Ok(None) => {}
                Ok(Some(true)) => {}
                Ok(Some(false)) if self.continue_without_transcript => {
                    if matches!(&self.mode, YouTubeDownloadMode::TranscriptOnly) {
                        return Ok(TransientExecutionOutcome::warning(
                            "TRANSCRIPT_MISSING",
                            Vec::new(),
                        ));
                    }
                }
                Ok(Some(false)) => {
                    return Err(transient_error(
                        "NO_ARTIFACT",
                        "yt-dlp completed without the requested transcript artifact",
                    ));
                }
                Err(message) => {
                    return Err(transient_error("TRANSCRIPT_INVALID", message));
                }
            }
        }
        let (verified, initial_leaf_handles) = verify_artifacts(cleanup.lease(), false)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let artifact_set =
            validate_artifact_set(&self.mode, self.continue_without_transcript, &verified)
                .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let media_warnings = if matches!(
            &self.mode,
            YouTubeDownloadMode::VideoOnly | YouTubeDownloadMode::VideoAndTranscript
        ) {
            verify_media_with_ffprobe(
                cleanup.lease(),
                self.item_context(item)
                    .map_err(|message| transient_error("MEDIA_POLICY_MISSING", message))?,
                &verified,
                control,
            )
            .map_err(|message| transient_error("MEDIA_VERIFY_FAILED", message))?
        } else {
            Vec::new()
        };
        drop(initial_leaf_handles);
        let has_media = artifact_set.has_media;
        let has_transcript = artifact_set.has_transcript;
        let transcript_missing =
            matches!(&self.mode, YouTubeDownloadMode::VideoAndTranscript) && !has_transcript;
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
        write_strict_manifest(cleanup.lease(), self, item, &verified)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let manifest_leaf = validate_strict_manifest_file(cleanup.lease(), self, item, &verified)
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
        let mut warnings = media_warnings;
        if transcript_missing {
            warnings.push("TRANSCRIPT_MISSING".to_string());
        }
        warnings.sort();
        warnings.dedup();
        let outcome = if warnings.is_empty() {
            TransientExecutionOutcome::completed(artifact_kinds)
        } else {
            let mut outcome =
                TransientExecutionOutcome::warning(warnings[0].clone(), artifact_kinds);
            outcome.warnings = warnings;
            outcome
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

fn verify_existing_item(
    lease: &ExistingOutputDirectoryLease,
    executor: &YouTubeExecutor,
    item: &TransientWorkItem,
    control: &TransientRunControl,
) -> Result<Vec<String>, String> {
    lease
        .validate_contents()
        .map_err(|error| error.to_string())?;
    let mut artifacts = Vec::new();
    let mut handles = Vec::new();
    for entry in fs::read_dir(lease.path()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| "existing output contains a non-UTF-8 name".to_string())?;
        validate_output_component(&name).map_err(|error| error.to_string())?;
        if name == "manifest.json" {
            continue;
        }
        if is_transient_artifact_name(&name) {
            return Err(format!(
                "existing output contains an incomplete artifact: {name}"
            ));
        }
        let held = hash_existing_regular_file(lease, &name)?;
        artifacts.push(held.artifact.clone());
        handles.push(held);
    }
    artifacts.sort_by(|left, right| left.name.cmp(&right.name));
    if artifacts.is_empty() {
        return Err("existing output has no artifacts".to_string());
    }
    let artifact_set = validate_artifact_set(
        &executor.mode,
        executor.continue_without_transcript,
        &artifacts,
    )?;
    if artifact_set.has_transcript {
        let selected = executor.item_selected_transcript(item)?.ok_or_else(|| {
            "existing output has a transcript without a selected track".to_string()
        })?;
        let vtt_name = artifacts
            .iter()
            .find(|artifact| artifact.kind == "vtt")
            .map(|artifact| artifact.name.as_str())
            .ok_or_else(|| "existing output is missing its raw VTT".to_string())?;
        let json_name = artifacts
            .iter()
            .find(|artifact| artifact.kind == "transcript_json")
            .map(|artifact| artifact.name.as_str())
            .ok_or_else(|| "existing output is missing normalized transcript JSON".to_string())?;
        validate_existing_transcript_pair(lease, item, &selected, vtt_name, json_name)?;
    }
    let expected = executor.strict_manifest(item, &artifacts)?;
    let expected_bytes = canonical_manifest_bytes(&expected).map_err(|error| error.to_string())?;
    let mut manifest = lease
        .open_leaf("manifest.json")
        .map_err(|error| error.to_string())?;
    let metadata = manifest.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_MANIFEST_BYTES {
        return Err("existing manifest is empty, oversized, or not a regular file".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    manifest
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let parsed: crate::providers::youtube::manifest_contract::YouTubeArtifactManifest =
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("existing manifest is invalid: {error}"))?;
    if parsed != expected || bytes != expected_bytes {
        return Err("existing manifest does not match the current verified projection".to_string());
    }
    if artifact_set.has_media {
        verify_existing_media_with_ffprobe(
            lease,
            executor
                .item_context(item)
                .map_err(|message| message.to_string())?,
            &artifacts,
            control,
        )?;
    }
    let manifest_sha256 = hash_open_file(&mut manifest)?;
    handles.push(VerifiedLeafHandle {
        artifact: VerifiedArtifact {
            name: "manifest.json".to_string(),
            kind: "metadata".to_string(),
            size_bytes: metadata.len(),
            sha256: manifest_sha256,
        },
        file: manifest,
    });
    verify_held_leaf_handles(&mut handles)?;
    lease.revalidate().map_err(|error| error.to_string())?;
    let mut kinds = artifacts
        .into_iter()
        .map(|artifact| artifact.kind)
        .collect::<Vec<_>>();
    kinds.sort();
    kinds.dedup();
    Ok(kinds)
}

fn hash_existing_regular_file(
    lease: &ExistingOutputDirectoryLease,
    name: &str,
) -> Result<VerifiedLeafHandle, String> {
    let mut file = lease.open_leaf(name).map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() || before.len() == 0 {
        return Err(format!("existing artifact is empty or not regular: {name}"));
    }
    let sha256 = hash_open_file(&mut file)?;
    let after = file.metadata().map_err(|error| error.to_string())?;
    if !after.is_file() || after.len() != before.len() {
        return Err(format!(
            "existing artifact changed during verification: {name}"
        ));
    }
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(VerifiedLeafHandle {
        artifact: VerifiedArtifact {
            name: name.to_string(),
            kind: artifact_kind(name),
            size_bytes: before.len(),
            sha256,
        },
        file,
    })
}

fn verify_media_with_ffprobe(
    attempt: &OutputAttemptLease,
    context: &YouTubeExecutorItemContext,
    artifacts: &[VerifiedArtifact],
    control: &TransientRunControl,
) -> Result<Vec<String>, String> {
    let media = artifacts
        .iter()
        .filter(|artifact| artifact.kind == "media")
        .collect::<Vec<_>>();
    if media.len() != 1 {
        return Err("media execution must produce exactly one media artifact".to_string());
    }
    attempt.revalidate().map_err(|error| error.to_string())?;
    let media_path = attempt.path().join(&media[0].name);
    let output = run(
        ffprobe_invocation(vec![
            "-v".to_string(),
            "error".to_string(),
            "-print_format".to_string(),
            "json".to_string(),
            "-show_format".to_string(),
            "-show_streams".to_string(),
            media_path.to_string_lossy().into_owned(),
        ]),
        Some(control),
        None,
    )
    .map_err(|error| error.to_string())?;
    attempt.revalidate().map_err(|error| error.to_string())?;
    if output.cancelled || control.is_cancelled() {
        return Err("media verification was cancelled".to_string());
    }
    if output.timed_out {
        return Err("FFprobe verification timed out".to_string());
    }
    if output.stdout_truncated {
        return Err("FFprobe verification output exceeded the safety limit".to_string());
    }
    if !output.status.success() {
        return Err("FFprobe could not read the media artifact".to_string());
    }
    let verified = verify_ffprobe_json(
        &output.stdout,
        MediaMode::VideoAndAudio,
        context.source_duration_seconds.map(|value| value as f64),
    )
    .map_err(|error| error.to_string())?;
    Ok(if verified.warnings.is_empty() {
        Vec::new()
    } else {
        vec![PLAYBACK_COMPATIBILITY_WARNING.to_string()]
    })
}

fn verify_existing_media_with_ffprobe(
    lease: &ExistingOutputDirectoryLease,
    context: &YouTubeExecutorItemContext,
    artifacts: &[VerifiedArtifact],
    control: &TransientRunControl,
) -> Result<(), String> {
    let media = artifacts
        .iter()
        .filter(|artifact| artifact.kind == "media")
        .collect::<Vec<_>>();
    if media.len() != 1 {
        return Err("media reuse requires exactly one media artifact".to_string());
    }
    lease.revalidate().map_err(|error| error.to_string())?;
    let media_path = lease.path().join(&media[0].name);
    let output = run(
        ffprobe_invocation(vec![
            "-v".to_string(),
            "error".to_string(),
            "-print_format".to_string(),
            "json".to_string(),
            "-show_format".to_string(),
            "-show_streams".to_string(),
            media_path.to_string_lossy().into_owned(),
        ]),
        Some(control),
        None,
    )
    .map_err(|error| error.to_string())?;
    lease.revalidate().map_err(|error| error.to_string())?;
    if output.cancelled || control.is_cancelled() {
        return Err("media reuse verification was cancelled".to_string());
    }
    if output.timed_out {
        return Err("FFprobe reuse verification timed out".to_string());
    }
    if output.stdout_truncated {
        return Err("FFprobe reuse verification output exceeded the safety limit".to_string());
    }
    if !output.status.success() {
        return Err("FFprobe could not read the reused media artifact".to_string());
    }
    verify_ffprobe_json(
        &output.stdout,
        MediaMode::VideoAndAudio,
        context.source_duration_seconds.map(|value| value as f64),
    )
    .map_err(|error| error.to_string())?;
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(())
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

/// Preserve the helper's raw VTT and add one deterministic normalized JSON
/// sibling. A transcript-only attempt must contain at most one raw VTT: if
/// the helper ignored the exact language/source policy and emitted several
/// tracks, publication fails closed instead of guessing which one to keep.
fn post_process_transcript(
    attempt: &OutputAttemptLease,
    item: &TransientWorkItem,
    selected: &SelectedTranscript,
) -> Result<bool, String> {
    attempt
        .validate_contents()
        .map_err(|error| error.to_string())?;
    let mut vtt_names = Vec::new();
    for entry in fs::read_dir(attempt.path()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| "transcript artifact has a non-UTF-8 name".to_string())?;
        validate_output_component(&name).map_err(|error| error.to_string())?;
        if is_transient_artifact_name(&name) {
            return Err(format!("staging contained an incomplete artifact: {name}"));
        }
        if name
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("vtt"))
        {
            vtt_names.push(name);
        }
    }
    if vtt_names.is_empty() {
        return Ok(false);
    }
    if vtt_names.len() != 1 {
        vtt_names.sort();
        return Err(format!(
            "transcript helper emitted multiple VTT tracks: {}",
            vtt_names.join(", ")
        ));
    }
    let vtt_name = vtt_names
        .pop()
        .ok_or_else(|| "transcript VTT selection unexpectedly empty".to_string())?;
    if !transcript_filename_matches_language(&vtt_name, selected) {
        return Err(format!(
            "transcript VTT filename does not match selected language {}: {vtt_name}",
            selected.language_tag
        ));
    }
    let json_name = transcript_json_name(&vtt_name)?;
    let mut vtt = attempt
        .open_leaf(&vtt_name)
        .map_err(|error| error.to_string())?;
    let raw_vtt = read_bounded_file(&mut vtt, &vtt_name, MAX_VTT_BYTES as u64)?;
    let normalized = normalize_vtt_json(
        &raw_vtt,
        TranscriptMetadata {
            video_id: item.video_id.clone(),
            language_tag: selected.language_tag.clone(),
            source: selected.source.clone(),
            source_track_key: selected.track_key.clone(),
        },
    )
    .map_err(transcript_normalization_error)?;
    let mut output = attempt
        .create_leaf(&json_name)
        .map_err(|error| error.to_string())?;
    output
        .write_all(&normalized)
        .map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    attempt.revalidate().map_err(|error| error.to_string())?;
    Ok(true)
}

fn transcript_json_name(vtt_name: &str) -> Result<String, String> {
    let base = vtt_name
        .get(..vtt_name.len().saturating_sub(4))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "transcript VTT has no usable base name".to_string())?;
    let name = format!("{base}.transcript.json");
    validate_output_component(&name).map_err(|error| error.to_string())?;
    Ok(name)
}

fn transcript_normalization_error(error: TranscriptNormalizationError) -> String {
    error.to_string()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArtifactSetSummary {
    has_media: bool,
    has_transcript: bool,
}

/// Enforce the product-owned artifact grammar before manifest projection.  A
/// helper can create arbitrary flat files, so extension-derived kinds are
/// treated as an allowlist rather than defaulting unknown names to media.
fn validate_artifact_set(
    mode: &YouTubeDownloadMode,
    continue_without_transcript: bool,
    artifacts: &[VerifiedArtifact],
) -> Result<ArtifactSetSummary, String> {
    let mut media_count = 0;
    let mut vtt = Vec::new();
    let mut transcript_json = Vec::new();
    let mut metadata_count = 0;

    for artifact in artifacts {
        match artifact.kind.as_str() {
            "media" => {
                if !is_known_media_name(&artifact.name) {
                    return Err(format!(
                        "unsupported media artifact extension: {}",
                        artifact.name
                    ));
                }
                media_count += 1;
            }
            "vtt" => {
                if !is_vtt_name(&artifact.name) {
                    return Err(format!(
                        "unsupported transcript artifact: {}",
                        artifact.name
                    ));
                }
                vtt.push(artifact.name.as_str());
            }
            "transcript_json" => {
                if !artifact
                    .name
                    .to_ascii_lowercase()
                    .ends_with(".transcript.json")
                {
                    return Err(format!(
                        "unsupported normalized transcript artifact: {}",
                        artifact.name
                    ));
                }
                transcript_json.push(artifact.name.as_str());
            }
            "metadata" => {
                if !artifact.name.eq_ignore_ascii_case("metadata.json") {
                    return Err(format!("unknown JSON artifact: {}", artifact.name));
                }
                metadata_count += 1;
            }
            kind => return Err(format!("unknown artifact kind {kind}: {}", artifact.name)),
        }
    }

    if metadata_count > 1 {
        return Err("artifact set contains duplicate metadata.json files".to_string());
    }
    let has_transcript = match (vtt.as_slice(), transcript_json.as_slice()) {
        ([], []) => false,
        ([vtt_name], [json_name]) => {
            let expected_json = transcript_json_name(vtt_name)?;
            if expected_json != *json_name {
                return Err(format!(
                    "normalized transcript does not pair with raw VTT: {json_name}"
                ));
            }
            true
        }
        _ => {
            return Err(
                "transcript artifact set must contain exactly one VTT and one normalized JSON"
                    .to_string(),
            )
        }
    };

    match mode {
        YouTubeDownloadMode::VideoOnly => {
            if media_count != 1 || has_transcript {
                return Err(
                    "video-only output must contain one media artifact and no transcript pair"
                        .to_string(),
                );
            }
        }
        YouTubeDownloadMode::TranscriptOnly => {
            if media_count != 0 || !has_transcript {
                return Err(
                    "transcript-only output must contain one transcript pair and no media"
                        .to_string(),
                );
            }
        }
        YouTubeDownloadMode::VideoAndTranscript => {
            if media_count != 1 {
                return Err(
                    "video-and-transcript output must contain exactly one media artifact"
                        .to_string(),
                );
            }
            if !has_transcript && !continue_without_transcript {
                return Err(
                    "video-and-transcript output is missing its required transcript pair"
                        .to_string(),
                );
            }
        }
    }

    Ok(ArtifactSetSummary {
        has_media: media_count != 0,
        has_transcript,
    })
}

fn read_bounded_file(file: &mut File, name: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() || before.len() == 0 {
        return Err(format!("{name} is empty or not a regular file"));
    }
    if before.len() > max_bytes {
        return Err(format!("{name} exceeds the {max_bytes}-byte safety limit"));
    }
    let size = usize::try_from(before.len())
        .map_err(|_| format!("{name} is too large for this platform"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut bytes = vec![0_u8; size];
    file.read_exact(&mut bytes)
        .map_err(|error| error.to_string())?;
    let mut extra = [0_u8; 1];
    if file.read(&mut extra).map_err(|error| error.to_string())? != 0 {
        return Err(format!("{name} grew beyond its bounded size"));
    }
    let after = file.metadata().map_err(|error| error.to_string())?;
    if !after.is_file() || after.len() != before.len() {
        return Err(format!("{name} changed during bounded read"));
    }
    Ok(bytes)
}

fn validate_existing_transcript_pair(
    lease: &ExistingOutputDirectoryLease,
    item: &TransientWorkItem,
    selected: &SelectedTranscript,
    vtt_name: &str,
    json_name: &str,
) -> Result<(), String> {
    if !transcript_filename_matches_language(vtt_name, selected) {
        return Err(format!(
            "existing transcript VTT filename does not match selected language {}: {vtt_name}",
            selected.language_tag
        ));
    }
    let mut vtt = lease
        .open_leaf(vtt_name)
        .map_err(|error| error.to_string())?;
    let raw_vtt = read_bounded_file(&mut vtt, vtt_name, MAX_VTT_BYTES as u64)?;
    let mut normalized = lease
        .open_leaf(json_name)
        .map_err(|error| error.to_string())?;
    let normalized_bytes = read_bounded_file(
        &mut normalized,
        json_name,
        MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES,
    )?;
    let expected = normalize_vtt_json(
        &raw_vtt,
        TranscriptMetadata {
            video_id: item.video_id.clone(),
            language_tag: selected.language_tag.clone(),
            source: selected.source.clone(),
            source_track_key: selected.track_key.clone(),
        },
    )
    .map_err(transcript_normalization_error)?;
    if normalized_bytes != expected {
        return Err(
            "normalized transcript does not match the deterministic projection of its raw VTT"
                .to_string(),
        );
    }
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(())
}

fn transcript_filename_matches_language(name: &str, selected: &SelectedTranscript) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if !extension.eq_ignore_ascii_case("vtt") {
        return false;
    }
    let suffix = format!(".{}", normalize_language_suffix(&selected.language_tag));
    normalize_language_suffix(stem).ends_with(&suffix)
}

fn normalize_language_suffix(value: &str) -> String {
    value.trim().replace('_', "-").to_ascii_lowercase()
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

#[cfg(test)]
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

fn write_strict_manifest(
    attempt: &OutputAttemptLease,
    executor: &YouTubeExecutor,
    item: &TransientWorkItem,
    artifacts: &[VerifiedArtifact],
) -> Result<(), String> {
    let manifest = executor.strict_manifest(item, artifacts)?;
    let payload = canonical_manifest_bytes(&manifest).map_err(|error| error.to_string())?;
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

#[cfg(test)]
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

fn validate_strict_manifest_file(
    attempt: &OutputAttemptLease,
    executor: &YouTubeExecutor,
    item: &TransientWorkItem,
    artifacts: &[VerifiedArtifact],
) -> Result<VerifiedLeafHandle, String> {
    let expected = executor.strict_manifest(item, artifacts)?;
    let expected_bytes = canonical_manifest_bytes(&expected).map_err(|error| error.to_string())?;
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
    let parsed: crate::providers::youtube::manifest_contract::YouTubeArtifactManifest =
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("published manifest is invalid JSON: {error}"))?;
    if parsed != expected || bytes != expected_bytes {
        return Err("published strict manifest does not match verified artifacts".to_string());
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

fn manifest_artifact_kind(kind: &str) -> Result<ManifestArtifactKind, String> {
    match kind {
        "media" => Ok(ManifestArtifactKind::Media),
        "vtt" => Ok(ManifestArtifactKind::Vtt),
        "transcript_json" => Ok(ManifestArtifactKind::TranscriptJson),
        "metadata" => Ok(ManifestArtifactKind::Metadata),
        _ => Err(format!("unsupported artifact kind: {kind}")),
    }
}

fn artifact_kind(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".transcript.json") {
        return "transcript_json".to_string();
    }
    match Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "vtt" => "vtt".to_string(),
        "json" => "metadata".to_string(),
        _ if is_known_media_name(name) => "media".to_string(),
        _ => "unsupported".to_string(),
    }
}

fn is_vtt_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("vtt"))
}

fn is_known_media_name(name: &str) -> bool {
    matches!(
        Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some(
            "3g2"
                | "3gp"
                | "aac"
                | "avi"
                | "flac"
                | "flv"
                | "m2ts"
                | "m4a"
                | "m4v"
                | "mka"
                | "mkv"
                | "mov"
                | "mp3"
                | "mp4"
                | "mpeg"
                | "mpg"
                | "mts"
                | "mxf"
                | "oga"
                | "ogg"
                | "ogv"
                | "opus"
                | "ts"
                | "wav"
                | "webm"
                | "wmv"
        )
    )
}

#[cfg(test)]
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
        let args = executor
            .args_for(&item, &temp.path().join("output.%(ext)s"))
            .unwrap();
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

    const VALID_VTT: &str = "WEBVTT\n\n00:00.000 --> 00:01.500\nHello &amp; world\n";
    const MALFORMED_VTT: &str = "NOT WEBVTT\n\n00:00.000 --> 00:01.500\nBroken\n";

    fn transcript_request() -> StartYouTubeDownloadRequest {
        StartYouTubeDownloadRequest {
            client_submission_id: "submission-1".to_string(),
            scan_plan_id: "plan-1".to_string(),
            selected_occurrence_ids: vec!["occurrence-1".to_string()],
            output_dir: String::new(),
            mode: YouTubeDownloadMode::TranscriptOnly,
            max_height: None,
            preferred_language: Some("en".to_string()),
            fallback_languages: vec!["fr".to_string()],
            allow_automatic_captions: true,
            continue_without_transcript: false,
        }
    }

    fn transcript_item(
        selected: Option<SelectedTranscript>,
    ) -> (TransientWorkItem, YouTubeExecutorContext) {
        let helper_lock_digest = "a".repeat(64);
        let artifact_fingerprint = artifact_fingerprint(&ArtifactFingerprintInput {
            occurrence_id: "occurrence-1".to_string(),
            video_id: "video-1".to_string(),
            mode: YouTubeDownloadMode::TranscriptOnly,
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: None,
            selected_transcript: selected.clone(),
            helper_lock_digest: helper_lock_digest.clone(),
        })
        .unwrap_or_else(|error| panic!("fixture fingerprint failed: {error}"));
        (
            TransientWorkItem {
                occurrence_id: "occurrence-1".to_string(),
                artifact_fingerprint,
                video_id: "video-1".to_string(),
                ordinal: 1,
                title: "Example title".to_string(),
                source_url: "https://www.youtube.com/watch?v=video-1".to_string(),
            },
            YouTubeExecutorContext {
                source_snapshot_digest: "b".repeat(64),
                playlist_id: Some("playlist-1".to_string()),
                helper_lock_digest,
                items: vec![YouTubeExecutorItemContext {
                    occurrence_id: "occurrence-1".to_string(),
                    playlist_index: Some(1),
                    source_duration_seconds: None,
                    selected_transcript: selected,
                }],
            },
        )
    }

    fn selected_uploader() -> SelectedTranscript {
        SelectedTranscript {
            track_key: "uploader-en".to_string(),
            language_tag: "en-US".to_string(),
            source: YouTubeTranscriptSource::Uploader,
        }
    }

    #[test]
    fn transcript_args_use_exact_selected_source_and_language() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (item, context) = transcript_item(Some(selected_uploader()));
        let executor =
            YouTubeExecutor::new_with_context(root.clone(), &transcript_request(), context)
                .unwrap();
        let args = executor
            .args_for(&item, &temp.path().join("output.%(ext)s"))
            .unwrap();
        assert!(args.iter().any(|argument| argument == "--write-subs"));
        assert!(!args.iter().any(|argument| argument == "--write-auto-subs"));
        let language_index = args
            .iter()
            .position(|argument| argument == "--sub-langs")
            .unwrap();
        assert_eq!(args[language_index + 1], "en-US");
        assert!(!args.iter().any(|argument| argument == "fr"));

        let automatic = SelectedTranscript {
            track_key: "automatic-en".to_string(),
            language_tag: "en".to_string(),
            source: YouTubeTranscriptSource::Automatic,
        };
        let (automatic_item, automatic_context) = transcript_item(Some(automatic));
        let automatic_executor =
            YouTubeExecutor::new_with_context(root, &transcript_request(), automatic_context)
                .unwrap();
        let automatic_args = automatic_executor
            .args_for(&automatic_item, &temp.path().join("automatic.%(ext)s"))
            .unwrap();
        assert!(automatic_args
            .iter()
            .any(|argument| argument == "--write-auto-subs"));
        assert!(!automatic_args
            .iter()
            .any(|argument| argument == "--write-subs"));
        assert_eq!(
            automatic_args
                .iter()
                .position(|argument| argument == "--sub-langs")
                .map(|index| automatic_args[index + 1].as_str()),
            Some("en")
        );
    }

    #[test]
    fn transcript_missing_policy_fails_before_helper_arguments_are_built() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (item, context) = transcript_item(None);
        let executor =
            YouTubeExecutor::new_with_context(root, &transcript_request(), context).unwrap();
        let error = executor
            .args_for(&item, &temp.path().join("output.%(ext)s"))
            .unwrap_err();
        assert!(error.contains("no selected track"));
    }

    #[test]
    fn transcript_post_processing_preserves_raw_vtt_and_writes_strict_manifest_inputs() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let selected = selected_uploader();
        let (item, context) = transcript_item(Some(selected.clone()));
        let executor =
            YouTubeExecutor::new_with_context(root.clone(), &transcript_request(), context)
                .unwrap();
        let attempt = root
            .staging_attempt_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        let raw_name = "001-Example-video-1.en-US.vtt";
        fs::write(attempt.path().join(raw_name), VALID_VTT).unwrap();
        assert!(post_process_transcript(&attempt, &item, &selected).unwrap());
        assert_eq!(
            fs::read(attempt.path().join(raw_name)).unwrap(),
            VALID_VTT.as_bytes()
        );
        let normalized_name = transcript_json_name(raw_name).unwrap();
        let normalized = fs::read(attempt.path().join(&normalized_name)).unwrap();
        assert!(normalized
            .windows(b"sourceVttSha256".len())
            .any(|window| { window == b"sourceVttSha256" }));
        let (artifacts, handles) = verify_artifacts(&attempt, false).unwrap();
        drop(handles);
        write_strict_manifest(&attempt, &executor, &item, &artifacts).unwrap();
        let manifest_handle =
            validate_strict_manifest_file(&attempt, &executor, &item, &artifacts).unwrap();
        let manifest: crate::providers::youtube::manifest_contract::YouTubeArtifactManifest =
            serde_json::from_slice(&fs::read(attempt.path().join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest.source_snapshot_digest, "b".repeat(64));
        assert_eq!(manifest.selected_transcript, Some(selected));
        assert!(manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == ManifestArtifactKind::TranscriptJson));
        drop(manifest_handle);
        root.discard_attempt_lease(attempt).unwrap();
    }

    fn publish_strict_transcript_fixture(
        root: &ValidatedOutputRoot,
        executor: &YouTubeExecutor,
        item: &TransientWorkItem,
        selected: &SelectedTranscript,
    ) -> std::path::PathBuf {
        let attempt = root
            .staging_attempt_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        let raw_name = "001-Example-video-1.en-US.vtt";
        fs::write(attempt.path().join(raw_name), VALID_VTT).unwrap();
        assert!(post_process_transcript(&attempt, item, selected).unwrap());
        let (artifacts, initial_handles) = verify_artifacts(&attempt, false).unwrap();
        drop(initial_handles);
        write_strict_manifest(&attempt, executor, item, &artifacts).unwrap();
        let manifest_handle =
            validate_strict_manifest_file(&attempt, executor, item, &artifacts).unwrap();
        let (rechecked, mut held_handles) = verify_artifacts(&attempt, true).unwrap();
        assert!(same_artifacts(&artifacts, &rechecked));
        held_handles.push(manifest_handle);
        verify_held_leaf_handles(&mut held_handles).unwrap();
        drop(held_handles);
        root.publish_attempt_lease(
            attempt,
            &safe_stem(item.ordinal, &item.title, &item.video_id),
        )
        .unwrap()
    }

    #[test]
    fn exact_existing_output_is_reused_without_helper_execution() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let selected = selected_uploader();
        let (item, context) = transcript_item(Some(selected.clone()));
        let executor =
            YouTubeExecutor::new_with_context(root.clone(), &transcript_request(), context)
                .unwrap();
        let final_name = safe_stem(item.ordinal, &item.title, &item.video_id);
        publish_strict_transcript_fixture(&root, &executor, &item, &selected);

        assert_eq!(
            executor
                .try_reuse_existing(&item, &final_name, &TransientRunControl::default())
                .unwrap(),
            Some(vec!["transcript_json".to_string(), "vtt".to_string()])
        );
    }

    #[test]
    fn changed_existing_artifact_is_a_collision_not_a_reuse() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let selected = selected_uploader();
        let (item, context) = transcript_item(Some(selected.clone()));
        let executor =
            YouTubeExecutor::new_with_context(root.clone(), &transcript_request(), context)
                .unwrap();
        let final_name = safe_stem(item.ordinal, &item.title, &item.video_id);
        let destination = publish_strict_transcript_fixture(&root, &executor, &item, &selected);
        fs::write(
            destination.join("001-Example-video-1.en-US.vtt"),
            b"WEBVTT\n\n00:00.000 --> 00:01.500\nchanged\n",
        )
        .unwrap();

        let error = executor
            .try_reuse_existing(&item, &final_name, &TransientRunControl::default())
            .unwrap_err();
        assert_eq!(error.code, "OUTPUT_COLLISION");
        assert!(error.message.contains("does not match"));
    }

    #[test]
    fn transcript_filename_language_mismatch_fails_before_normalization() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let selected = selected_uploader();
        let (item, _context) = transcript_item(Some(selected.clone()));
        let attempt = root
            .staging_attempt_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        let raw_name = "001-Example-video-1.fr-FR.vtt";
        fs::write(attempt.path().join(raw_name), VALID_VTT).unwrap();

        let error = post_process_transcript(&attempt, &item, &selected).unwrap_err();
        assert!(error.contains("does not match selected language"));
        assert!(!attempt
            .path()
            .join(transcript_json_name(raw_name).unwrap())
            .exists());
        root.discard_attempt_lease(attempt).unwrap();
    }

    #[test]
    fn artifact_sets_are_exact_and_reject_unknown_text() {
        let media = VerifiedArtifact {
            name: "video.mp4".to_string(),
            kind: "media".to_string(),
            size_bytes: 1,
            sha256: "a".repeat(64),
        };
        let vtt = VerifiedArtifact {
            name: "video.en-US.vtt".to_string(),
            kind: "vtt".to_string(),
            size_bytes: 1,
            sha256: "b".repeat(64),
        };
        let transcript_json = VerifiedArtifact {
            name: "video.en-US.transcript.json".to_string(),
            kind: "transcript_json".to_string(),
            size_bytes: 1,
            sha256: "c".repeat(64),
        };
        let unknown_text = VerifiedArtifact {
            name: "notes.txt".to_string(),
            kind: "unsupported".to_string(),
            size_bytes: 1,
            sha256: "d".repeat(64),
        };

        assert!(validate_artifact_set(
            &YouTubeDownloadMode::VideoOnly,
            false,
            std::slice::from_ref(&media)
        )
        .is_ok());
        assert!(validate_artifact_set(
            &YouTubeDownloadMode::TranscriptOnly,
            false,
            &[vtt.clone(), transcript_json.clone()]
        )
        .is_ok());
        assert!(validate_artifact_set(
            &YouTubeDownloadMode::VideoAndTranscript,
            false,
            &[media.clone(), vtt, transcript_json]
        )
        .is_ok());
        assert!(validate_artifact_set(
            &YouTubeDownloadMode::VideoAndTranscript,
            true,
            std::slice::from_ref(&media)
        )
        .is_ok());
        assert!(validate_artifact_set(
            &YouTubeDownloadMode::VideoOnly,
            false,
            &[media, unknown_text]
        )
        .is_err());
    }

    #[test]
    fn malformed_transcript_vtt_fails_closed_without_normalized_output() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let selected = selected_uploader();
        let (item, context) = transcript_item(Some(selected.clone()));
        let executor =
            YouTubeExecutor::new_with_context(root.clone(), &transcript_request(), context)
                .unwrap();
        let attempt = root
            .staging_attempt_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        let raw_name = "001-Example-video-1.en-US.vtt";
        fs::write(attempt.path().join(raw_name), MALFORMED_VTT).unwrap();
        let error = post_process_transcript(&attempt, &item, &selected).unwrap_err();
        assert!(error.contains("WEBVTT"));
        assert!(!attempt
            .path()
            .join(transcript_json_name(raw_name).unwrap())
            .exists());
        drop(executor);
        root.discard_attempt_lease(attempt).unwrap();
    }
}
