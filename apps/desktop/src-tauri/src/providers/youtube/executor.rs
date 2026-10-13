use crate::app::managed_process::{
    run, run_with_stdout_lines, ManagedProcessContext, ManagedProcessError, ManagedProcessOutput,
    ManagedProcessSpec, TransientError, TransientExecutionOutcome, TransientExecutor,
    TransientItemPhase, TransientProgressUpdate, TransientRunControl, TransientWorkItem,
};
use crate::app::safe_output_filesystem::{
    direct_child_name_budget_for, validate_output_component, ExistingOutputDirectoryLease, OutputAttemptLease, SafeOutputError,
    ValidatedOutputRoot,
};
use crate::providers::youtube::download_progress::{
    parse_format_id_line, parse_formats_line, parse_pinned_formats, DownloadProgress,
    ProgressReading, FORMATS_PRINT, FORMAT_ID_PRINT, PROGRESS_TEMPLATE,
};
use crate::providers::youtube::resume::{
    classify_helper_failure, ffmpeg_stderr_proves_invalid, has_complete_merged_media,
    has_incomplete_partials, incomplete_leaves, keep_staging_after, pause_reason,
    pinned_format_unavailable,
    is_bookkeeping_leaf, is_media_leaf, stale_part_leaves, FailureKind, FORMAT_MARKER,
    TRANSCRIPT_MARKER, VERIFY_MARKER,
};
use crate::app::safe_output_filesystem::resume::{resumable_staging_name, StagingDisposition};
use crate::providers::youtube::error::YouTubeError;
use crate::providers::youtube::helper::{
    download_invocation, invocation, media_probe_invocation, output_error, MAX_RECORD_STDOUT_BYTES,
};
use crate::providers::youtube::manifest_contract::{
    artifact_fingerprint, canonical_manifest_bytes, project_manifest, ArtifactFingerprintInput,
    ManifestArtifact as StrictManifestArtifact, ManifestArtifactKind, ManifestProjectionInput,
    SelectedTranscript, FORMAT_POLICY_VERSION,
};
use crate::providers::youtube::media_verifier::{
    MediaVerificationError,
    ffmpeg_copy_probe_args, verify_ffmpeg_copy_probe, FfmpegCopyProbeOutput, MediaMode,
    PLAYBACK_COMPATIBILITY_WARNING,
};
use crate::providers::youtube::models::{
    StartYouTubeDownloadRequest, YouTubeDownloadMode, YouTubeTranscriptSource,
};
use crate::providers::youtube::transcript_normalizer::{
    normalize_vtt_json, TranscriptMetadata, TranscriptNormalizationError,
    MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES, MAX_VTT_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub(super) const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// How long a fresh attempt waits for a staging directory another attempt
/// holds: 40 x 250 ms in production, shorter under test.
const STAGING_BUSY_ATTEMPTS: u32 = if cfg!(test) { 8 } else { 40 };
const STAGING_BUSY_DELAY: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(25)
} else {
    std::time::Duration::from_millis(250)
};
/// Longest transcript failure reason carried into a user-visible warning.
const MAX_WARNING_REASON_CHARS: usize = 240;

/// Coarse per-item progress fractions. Downloading owns `0.0..=STAGE_NORMALIZING`
/// and live byte progress fills that range in, so later stages start at or
/// above it and the fraction stays monotonic.
const STAGE_START: f64 = 0.0;
const STAGE_NORMALIZING: f64 = 0.8;
const STAGE_VERIFYING: f64 = 0.85;
const STAGE_FINALIZING: f64 = 0.95;
const STAGE_DONE: f64 = 1.0;

/// Process launch seam. Production always goes through the managed-process
/// port (identity-verified helpers in a kill-on-close job); tests substitute a
/// fake so stage and publication logic run without the real helpers.
trait ProcessRunner: Send + Sync + std::fmt::Debug {
    fn run(
        &self,
        spec: ManagedProcessSpec,
        control: &TransientRunControl,
    ) -> Result<ManagedProcessOutput, ManagedProcessError>;

    /// Like [`ProcessRunner::run`], also streaming each complete stdout line to
    /// `on_line` while the helper runs.
    fn run_streaming(
        &self,
        spec: ManagedProcessSpec,
        control: &TransientRunControl,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<ManagedProcessOutput, ManagedProcessError>;
}

#[derive(Debug)]
struct ManagedProcessRunner;

impl ProcessRunner for ManagedProcessRunner {
    fn run(
        &self,
        spec: ManagedProcessSpec,
        control: &TransientRunControl,
    ) -> Result<ManagedProcessOutput, ManagedProcessError> {
        run(spec, ManagedProcessContext::Run(control))
    }

    fn run_streaming(
        &self,
        spec: ManagedProcessSpec,
        control: &TransientRunControl,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<ManagedProcessOutput, ManagedProcessError> {
        run_with_stdout_lines(spec, ManagedProcessContext::Run(control), on_line)
    }
}

/// Report a stage boundary. Bytes stay `None`; live download bytes go through
/// [`report_download_progress`].
fn report_stage(
    progress: &mut dyn FnMut(TransientProgressUpdate),
    item: &TransientWorkItem,
    phase: TransientItemPhase,
    fraction: f64,
) {
    progress(TransientProgressUpdate {
        occurrence_id: item.occurrence_id.clone(),
        phase,
        bytes_completed: None,
        bytes_total: None,
        fraction: Some(fraction),
    });
}

/// Publish live download bytes inside the download stage's fraction range.
fn report_download_progress(
    progress: &mut dyn FnMut(TransientProgressUpdate),
    item: &TransientWorkItem,
    phase: TransientItemPhase,
    reading: &ProgressReading,
) {
    progress(TransientProgressUpdate {
        occurrence_id: item.occurrence_id.clone(),
        phase,
        bytes_completed: Some(reading.bytes_completed),
        bytes_total: reading.bytes_total,
        fraction: Some(STAGE_START + (STAGE_NORMALIZING - STAGE_START) * reading.ratio),
    });
}

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
    runner: Arc<dyn ProcessRunner>,
    /// Time source for the progress throttle; tests substitute a fake.
    clock: fn() -> Instant,
    /// Staging names another unfinished run still references, asked at the
    /// moment of cleanup. A shared directory is never discarded.
    shared_staging: Option<SharedStaging>,
}

/// Answers "which staging directories do other unfinished runs use?" to the
/// callback it is given, while holding the lock that serializes new admissions,
/// so the cleanup decision and the deletion cannot race an admission.
#[derive(Clone)]
pub struct SharedStaging(pub Arc<SharedStagingFn>);

/// Calls its argument with the shared set while the admission lock is held.
pub type SharedStagingFn =
    dyn Fn(&mut dyn FnMut(&std::collections::HashSet<String>)) + Send + Sync;

impl std::fmt::Debug for SharedStaging {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SharedStaging")
    }
}

/// Immutable execution facts captured by the command layer after scan and
/// transcript inspection.  The executor never derives a transcript choice
/// from language preferences alone: transcript modes require one exact
/// occurrence-level selection and the helper-lock/source identities that were
/// used to compute the work-item fingerprint.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeExecutorContext {
    pub source_snapshot_digest: String,
    pub playlist_id: Option<String>,
    pub helper_lock_digest: String,
    pub items: Vec<YouTubeExecutorItemContext>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeExecutorItemContext {
    pub occurrence_id: String,
    pub playlist_index: Option<u32>,
    pub source_duration_seconds: Option<u64>,
    pub selected_transcript: Option<SelectedTranscript>,
}

struct AttemptCleanup {
    root: ValidatedOutputRoot,
    lease: Option<OutputAttemptLease>,
    /// Leave the directory in place on drop so a later attempt can resume.
    keep: bool,
}

impl AttemptCleanup {
    fn new(root: &ValidatedOutputRoot, lease: OutputAttemptLease) -> Self {
        Self {
            root: root.clone(),
            lease: Some(lease),
            keep: false,
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

    /// Release the lease without deleting its partial files.
    fn keep_for_resume(&mut self) {
        self.keep = true;
    }

    /// Delete the staging directory now (instead of on drop).
    fn discard_now(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self.root.discard_attempt_lease(lease);
        }
    }
}

impl Drop for AttemptCleanup {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            if !self.keep {
                let _ = self.root.discard_attempt_lease(lease);
            }
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

#[derive(Debug)]
enum FfprobeVerificationError {
    ManagedProcess(ManagedProcessError),
    Message(String),
    /// FFmpeg proved the downloaded file is not valid media.
    Corrupt(String),
}

impl From<String> for FfprobeVerificationError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

#[derive(Debug)]
enum ExistingOutputVerificationError {
    Message(String),
    Ffprobe(FfprobeVerificationError),
    /// The folder is a valid video-only output but this request requires a
    /// transcript. Carries guidance the user can act on.
    TranscriptRequired(String),
}

impl From<String> for ExistingOutputVerificationError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<FfprobeVerificationError> for ExistingOutputVerificationError {
    fn from(error: FfprobeVerificationError) -> Self {
        Self::Ffprobe(error)
    }
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
            runner: Arc::new(ManagedProcessRunner),
            clock: Instant::now,
            shared_staging: None,
        }))
    }

    /// Test constructor that substitutes the process launcher.
    #[cfg(test)]
    fn new_with_runner(
        output_root: ValidatedOutputRoot,
        request: &StartYouTubeDownloadRequest,
        context: YouTubeExecutorContext,
        runner: Arc<dyn ProcessRunner>,
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
            context: Some(context),
            runner,
            clock: Instant::now,
            shared_staging: None,
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
            runner: Arc::new(ManagedProcessRunner),
            clock: Instant::now,
            shared_staging: None,
        }))
    }

    #[cfg(test)]
    fn args_for(
        &self,
        item: &TransientWorkItem,
        output_template: &Path,
    ) -> Result<Vec<String>, String> {
        self.args_for_format(item, output_template, None)
    }

    /// Helper arguments; `pinned_format` replaces the policy's format
    /// selector with the exact selection an interrupted attempt used.
    /// The same executor, told how to ask which staging directories other
    /// unfinished runs reference.
    pub fn with_shared_staging(self: Arc<Self>, shared: SharedStaging) -> Arc<Self> {
        let mut executor = (*self).clone();
        executor.shared_staging = Some(shared);
        Arc::new(executor)
    }

    fn args_for_format(
        &self,
        item: &TransientWorkItem,
        output_template: &Path,
        pinned_format: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let mut args = vec![
            "--ignore-config".to_string(),
            "--no-plugin-dirs".to_string(),
            "--no-update".to_string(),
            "--no-cache-dir".to_string(),
            "--no-warnings".to_string(),
            "--no-playlist".to_string(),
            "--continue".to_string(),
            "--part".to_string(),
            "--retries".to_string(),
            "10".to_string(),
            "--fragment-retries".to_string(),
            "10".to_string(),
            "--file-access-retries".to_string(),
            "10".to_string(),
            "--windows-filenames".to_string(),
            "--newline".to_string(),
            "--progress-template".to_string(),
            PROGRESS_TEMPLATE.to_string(),
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
                args.push("--concurrent-fragments".to_string());
                args.push("4".to_string());
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
                args.push(pinned_format.map_or_else(|| self.format_selector(), str::to_string));
            }
            YouTubeDownloadMode::VideoOnly => {
                args.push("--concurrent-fragments".to_string());
                args.push("4".to_string());
                args.push("--format".to_string());
                args.push(pinned_format.map_or_else(|| self.format_selector(), str::to_string));
            }
        }
        if !matches!(&self.mode, YouTubeDownloadMode::TranscriptOnly) {
            // `--print` implies `--quiet`, which would drop the progress
            // records unless progress is requested explicitly, and it implies
            // `--simulate` on some yt-dlp versions, which would skip the
            // download itself.
            args.push("--progress".to_string());
            args.push("--no-simulate".to_string());
            args.push("--print".to_string());
            args.push(FORMATS_PRINT.to_string());
            args.push("--print".to_string());
            args.push(FORMAT_ID_PRINT.to_string());
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
        verify_existing_item(&lease, self, item, control, self.runner.as_ref())
            .map(Some)
            .map_err(map_existing_output_verification_error)
    }

    fn format_selector(&self) -> String {
        self.max_height.map_or_else(
            || "bestvideo*+bestaudio/best".to_string(),
            |height| format!("bestvideo[height<={height}]+bestaudio/best[height<={height}]"),
        )
    }
}

impl YouTubeExecutor {
    /// Download into the leased staging directory, verify, and publish.
    ///
    /// `preserved_bytes` are partial bytes an earlier attempt left in the
    /// reused staging directory; yt-dlp `--continue` picks them up. Every
    /// attempt launches yt-dlp afresh, so expired media URLs are re-extracted
    /// each time.
    fn download_and_publish(
        &self,
        item: &TransientWorkItem,
        control: &TransientRunControl,
        progress: &mut dyn FnMut(TransientProgressUpdate),
        stem: &str,
        cleanup: &mut AttemptCleanup,
        preserved_bytes: u64,
    ) -> Result<TransientExecutionOutcome, TransientError> {
        let staging = cleanup.lease().path().to_path_buf();
        if control.is_cancelled() {
            return Err(transient_error("CANCELLED", "download was cancelled"));
        }
        // Keep helper-visible leaf names compact. yt-dlp appends `.part`,
        // fragment, subtitle-language, and container suffixes while a long
        // download is in flight; the descriptive name belongs to the final
        // item directory, not the temporary media leaf.
        let output_template = staging.join("media.%(ext)s");
        let download_phase = match &self.mode {
            YouTubeDownloadMode::TranscriptOnly => TransientItemPhase::Transcript,
            _ => TransientItemPhase::Media,
        };
        report_stage(progress, item, download_phase.clone(), STAGE_START);
        cleanup
            .lease()
            .revalidate()
            .map_err(|error| transient_error("SAFE_FILESYSTEM_VIOLATION", error.to_string()))?;
        let source_duration_seconds = self
            .context
            .as_ref()
            .and_then(|context| {
                context
                    .items
                    .iter()
                    .find(|candidate| candidate.occurrence_id == item.occurrence_id)
            })
            .and_then(|context| context.source_duration_seconds);
        // Partial media is only valid for the exact format selection that
        // wrote it. A resume pins that selection; partial files with no
        // record of it cannot be trusted and are removed.
        let mut pinned: Option<String> = None;
        if preserved_bytes > 0 && !matches!(&self.mode, YouTubeDownloadMode::TranscriptOnly) {
            match read_format_marker(cleanup.lease()) {
                Some(selection) => pinned = Some(selection),
                None => {
                    // No record of the format. Unfinished downloads could be
                    // any format and are removed; complete files cannot be
                    // mixed and are kept.
                    let names = cleanup
                        .lease()
                        .leaf_names()
                        .map_err(|error| transient_error("OUTPUT_VERIFY_FAILED", error.to_string()))?;
                    if has_incomplete_partials(&names) {
                        wipe_staging(cleanup.lease())
                            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
                    }
                }
            }
        }
        if preserved_bytes > 0 {
            // Derived files of an earlier, interrupted finalization are
            // regenerated from the media; stale ones would collide.
            remove_derived_files(cleanup.lease())
                .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        }
        let kept_bytes = if preserved_bytes > 0 {
            cleanup.lease().leaf_bytes().unwrap_or(0)
        } else {
            0
        };
        if kept_bytes > 0 {
            // Show the resumed download starting from the bytes that are
            // really kept instead of from zero; yt-dlp's readings take over.
            progress(TransientProgressUpdate {
                occurrence_id: item.occurrence_id.clone(),
                phase: download_phase.clone(),
                bytes_completed: Some(kept_bytes),
                bytes_total: None,
                fraction: None,
            });
        }
        let mut download_progress = DownloadProgress::new();
        let clock = self.clock;
        // A directory that already holds the finished, merged download needs
        // verification and finalization, not yt-dlp: it must work offline and
        // must never be touched by a format re-selection.
        let skip_helper = pinned.is_some()
            && !matches!(&self.mode, YouTubeDownloadMode::TranscriptOnly)
            && {
                // A VTT is only required when the run cannot go on without a
                // transcript; an optional one may have been discarded already.
                let transcript_needed = !self.continue_without_transcript
                    && self.item_selected_transcript(item).ok().flatten().is_some();
                cleanup
                    .lease()
                    .leaf_names()
                    .is_ok_and(|names| has_complete_merged_media(&names, transcript_needed))
            };
        let (helper_output, chosen_formats) = if skip_helper {
            (None, Vec::new())
        } else {
            loop {
            let helper_args = self
                .args_for_format(item, &output_template, pinned.as_deref())
                .map_err(|message| transient_error("TRANSCRIPT_POLICY_MISSING", message))?;
            let helper_spec = if matches!(&self.mode, YouTubeDownloadMode::TranscriptOnly) {
                invocation(helper_args, MAX_RECORD_STDOUT_BYTES)
            } else {
                download_invocation(
                    helper_args,
                    MAX_RECORD_STDOUT_BYTES,
                    source_duration_seconds,
                )
            };
            // Format ids yt-dlp chose for this attempt. Partial files of any
            // other format must not be mixed in.
            let mut chosen_formats: Vec<String> = Vec::new();
            let mut selection_changed = false;
            let lease_for_purge = cleanup.lease();
            let pinned_now = pinned.clone();
            let output = self
                .runner
                .run_streaming(helper_spec, control, &mut |line| {
                    if let Some(ids) = parse_formats_line(line) {
                        let _ = purge_stale_parts(lease_for_purge, &ids);
                    }
                    if let Some(ids) = parse_format_id_line(line) {
                        let selection = ids.join("+");
                        match pinned_now.as_deref() {
                            Some(pin) if pin == selection => {}
                            Some(_) => selection_changed = true,
                            None => {
                                let _ = write_format_marker(lease_for_purge, &selection);
                            }
                        }
                        let _ = purge_stale_parts(lease_for_purge, &ids);
                        chosen_formats = ids;
                    }
                    if let Some(reading) = download_progress.observe_line(line, clock()) {
                        report_download_progress(progress, item, download_phase.clone(), &reading);
                    }
                })
                .map_err(|error| match error {
                    ManagedProcessError::HelperTemp(reason) => {
                        transient_error("HELPER_TEMP_ROOT_INVALID", reason)
                    }
                    ManagedProcessError::Integrity(reason) => {
                        transient_error("HELPER_INTEGRITY_FAILED", reason)
                    }
                    error => transient_error("HELPER_FAILED", error.to_string()),
                })?;
            if selection_changed {
                // yt-dlp printed a selection other than the recorded one, so
                // the bytes on disk may belong to a different format.
                wipe_staging(cleanup.lease())
                    .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
                return Err(transient_error(
                    "HELPER_FAILED",
                    "the video's formats changed while resuming; starting over on the next attempt",
                ));
            }
            if pinned.is_some()
                && !output.status.success()
                && !output.cancelled
                && !control.is_cancelled()
                && pinned_format_unavailable(&output.stderr)
            {
                // The recorded format no longer exists: drop every partial
                // and retry once with the normal selection.
                // Only unfinished downloads go; a completed file is never
                // deleted because a format selection failed.
                wipe_incomplete(cleanup.lease())
                    .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
                pinned = None;
                continue;
            }
            break (Some(output), chosen_formats);
            }
        };
        cleanup
            .lease()
            .revalidate()
            .map_err(|error| transient_error("SAFE_FILESYSTEM_VIOLATION", error.to_string()))?;
        if let Some(output) = &helper_output {
            if output.cancelled || control.is_cancelled() {
                return Err(transient_error("CANCELLED", "download was cancelled"));
            }
            if output.timed_out {
                return Err(transient_error("HELPER_TIMEOUT", "yt-dlp helper timed out"));
            }
            if !output.status.success() {
                return Err(helper_failure(&output.stderr, &output_error(output)));
            }
        }
        if control.is_cancelled() {
            return Err(transient_error("CANCELLED", "download was cancelled"));
        }
        purge_stale_parts(cleanup.lease(), &chosen_formats)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        // Publish the last bytes if the throttle was still holding them back.
        if let Some(reading) = download_progress.flush(clock()) {
            report_download_progress(progress, item, download_phase, &reading);
        }
        // Set when the video is kept but its transcript could not be processed:
        // the transcript error code and its reason.
        let mut transcript_failure: Option<(&'static str, String)> = None;
        if matches!(
            &self.mode,
            YouTubeDownloadMode::TranscriptOnly | YouTubeDownloadMode::VideoAndTranscript
        ) {
            let selected = self
                .item_selected_transcript(item)
                .map_err(|message| transient_error("TRANSCRIPT_POLICY_MISSING", message))?;
            if selected.is_some() {
                report_stage(
                    progress,
                    item,
                    TransientItemPhase::NormalizingTranscript,
                    STAGE_NORMALIZING,
                );
            }
            // A transcript already discarded as unusable stays discarded: the
            // video is finalized with the same warning, nothing is fetched.
            let recorded_failure = (matches!(&self.mode, YouTubeDownloadMode::VideoAndTranscript)
                && self.continue_without_transcript
                && selected.is_some())
            .then(|| read_text_marker(cleanup.lease(), TRANSCRIPT_MARKER, 600))
            .flatten();
            if let Some(reason) = recorded_failure {
                transcript_failure = Some(("TRANSCRIPT_INVALID", reason));
            } else {
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
                Err(error) => {
                    // The video is the expensive part and it downloaded; keep
                    // it when the request may continue without a transcript and
                    // the transcript content itself is the problem. Transcript-
                    // only runs, runs that require a transcript, and staging or
                    // ambiguity failures still fail with their own code.
                    if matches!(&self.mode, YouTubeDownloadMode::VideoAndTranscript)
                        && self.continue_without_transcript
                        && error.keeps_video()
                    {
                        discard_transcript_artifacts(cleanup.lease())
                            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
                        transcript_failure = Some((error.code(), error.to_string()));
                        let reason: String = error.to_string().chars().take(500).collect();
                        let _ = write_text_marker(cleanup.lease(), TRANSCRIPT_MARKER, &reason);
                    } else {
                        return Err(transient_error(error.code(), error.to_string()));
                    }
                }
            }
            }
        }
        report_stage(
            progress,
            item,
            TransientItemPhase::Verifying,
            STAGE_VERIFYING,
        );
        let (verified, initial_leaf_handles) = verify_artifacts(cleanup.lease(), false)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let artifact_set =
            validate_artifact_set(&self.mode, self.continue_without_transcript, &verified)
                .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let media_warnings = if matches!(
            &self.mode,
            YouTubeDownloadMode::VideoOnly | YouTubeDownloadMode::VideoAndTranscript
        ) {
            match verify_media_with_ffprobe(
                cleanup.lease(),
                self.item_context(item)
                    .map_err(|message| transient_error("MEDIA_POLICY_MISSING", message))?,
                &verified,
                control,
                self.runner.as_ref(),
            ) {
                Ok(warnings) => warnings,
                Err(FfprobeVerificationError::Corrupt(message)) => {
                    // FFmpeg proved the file invalid: delete just the media
                    // (and only after its verification handles are released);
                    // everything else in staging stays.
                    drop(initial_leaf_handles);
                    let media_names = verified
                        .iter()
                        .filter(|artifact| artifact.kind == "media")
                        .map(|artifact| artifact.name.clone())
                        .collect::<Vec<_>>();
                    // Last guard: a file we cannot even open is a permission or
                    // I/O problem, not proof of corruption.
                    let readable = media_names.iter().all(|name| {
                        cleanup.lease().open_leaf(name).is_ok_and(|mut file| {
                            let mut byte = [0u8; 1];
                            Read::read(&mut file, &mut byte).is_ok()
                        })
                    });
                    if !readable {
                        return Err(transient_error("MEDIA_VERIFY_FAILED", message));
                    }
                    // Media and the recorded format selection both go: a format
                    // without the required streams must not be pinned again.
                    let _ = discard_finished_download(cleanup.lease());
                    return Err(transient_error("MEDIA_CORRUPT", message));
                }
                // An I/O or process failure proves nothing about the file:
                // the download is kept so Continue only re-verifies.
                Err(other) => return Err(map_fresh_ffprobe_error(other)),
            }
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
        report_stage(
            progress,
            item,
            TransientItemPhase::Finalizing,
            STAGE_FINALIZING,
        );
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
        // Kept through verification so a crash there still resumes with the
        // media; it must not be published.
        remove_bookkeeping(cleanup.lease())
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        let destination = self
            .output_root
            .publish_attempt_lease(cleanup.take_lease(), stem)
            .map_err(safe_output_error)?;
        verify_published_artifacts(&destination, &published_expectations)
            .map_err(|message| transient_error("OUTPUT_VERIFY_FAILED", message))?;
        cleanup.disarm();
        let _ = sync_directory_best_effort(self.output_root.path());
        let mut warnings = media_warnings;
        let mut warning_messages = Vec::new();
        if let Some((code, reason)) = &transcript_failure {
            warnings.push((*code).to_string());
            warning_messages.push((
                (*code).to_string(),
                format!(
                    "Video saved; the transcript couldn't be processed: {}",
                    truncate_reason(reason)
                ),
            ));
        } else if transcript_missing {
            warnings.push(TRANSCRIPT_MISSING_WARNING.to_string());
        }
        warnings.sort();
        warnings.dedup();
        let mut outcome = if warnings.is_empty() {
            TransientExecutionOutcome::completed(artifact_kinds)
        } else {
            let mut outcome =
                TransientExecutionOutcome::warning(warnings[0].clone(), artifact_kinds);
            outcome.warnings = warnings;
            outcome
        };
        outcome.warning_messages = warning_messages;
        report_stage(progress, item, TransientItemPhase::Completed, STAGE_DONE);
        Ok(outcome)
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
        // Mark the item running right away so a queued row never looks idle
        // while the reuse check and staging setup run.
        report_stage(progress, item, TransientItemPhase::Waiting, STAGE_START);
        let stem = safe_stem_for_root(&self.output_root, item.ordinal, &item.title, &item.video_id)
            .map_err(safe_output_error)?;
        if let Some(artifact_kinds) = self.try_reuse_existing(item, &stem, control)? {
            report_stage(progress, item, TransientItemPhase::Completed, STAGE_DONE);
            // A prior run may have kept the video without its transcript (it
            // was missing or could not be processed). A published folder is
            // read-only to the executor, so the transcript cannot be added in
            // place and the video must not be downloaded again. Report the
            // gap as a warning, not as "Already saved", and say how to retry.
            let transcript_expected = matches!(&self.mode, YouTubeDownloadMode::VideoAndTranscript)
                && self
                    .item_selected_transcript(item)
                    .is_ok_and(|selected| selected.is_some());
            if transcript_expected && !artifact_kinds.iter().any(|kind| kind == "transcript_json") {
                return Ok(TransientExecutionOutcome::warning(
                    TRANSCRIPT_MISSING_WARNING,
                    artifact_kinds,
                )
                .with_warning_message(
                    TRANSCRIPT_MISSING_WARNING,
                    format!(
                        "This video was saved earlier without its transcript. To try the \
                         transcript again, delete or move the folder \"{}\" and download it again.",
                        self.output_root.path().join(&stem).display()
                    ),
                ));
            }
            return Ok(TransientExecutionOutcome::skipped_existing(artifact_kinds));
        }
        // Another attempt may still be letting go of this directory (a
        // cancelled or just-failed run on the same video). Wait briefly
        // before giving up.
        let mut busy_attempts = 0;
        let staging = loop {
            match self
                .output_root
                .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
            {
                Ok(staging) => break staging,
                Err(SafeOutputError::StagingBusy { .. })
                    if busy_attempts < STAGING_BUSY_ATTEMPTS && !control.is_cancelled() =>
                {
                    busy_attempts += 1;
                    std::thread::sleep(STAGING_BUSY_DELAY);
                }
                Err(error) => return Err(staging_error(error)),
            }
        };
        let resumable = !matches!(staging.disposition, StagingDisposition::Unresumable);
        let preserved_bytes = match staging.disposition {
            StagingDisposition::Reused { preserved_bytes } => preserved_bytes,
            _ => 0,
        };
        let mut cleanup = AttemptCleanup::new(&self.output_root, staging.lease);
        let result = self.download_and_publish(
            item,
            control,
            progress,
            &stem,
            &mut cleanup,
            preserved_bytes,
        );
        let result = match result {
            Err(error)
                if resumable
                    && matches!(error.code.as_str(), "MEDIA_VERIFY_FAILED" | "OUTPUT_VERIFY_FAILED") =>
            {
                Err(note_verification_failure(cleanup.lease(), error))
            }
            other => other,
        };
        if let Err(error) = &result {
            // A cancelled or non-transport failure removes its partial files; a
            // transport failure keeps them so the next attempt resumes from the
            // same bytes. Either way a directory another unfinished run
            // references is left alone, and the reference check and the
            // deletion happen together under the admission lock.
            let policy_keep = error.code != "CANCELLED" && keep_staging_after(&error.code);
            let name = resumable_staging_name(&item.occurrence_id, &item.artifact_fingerprint);
            let keep = if !resumable {
                false
            } else if let Some(ask) = &self.shared_staging {
                let mut keep = policy_keep;
                (ask.0)(&mut |shared| {
                    if shared.contains(&name) {
                        keep = true;
                    }
                    if !keep {
                        cleanup.discard_now();
                    }
                });
                keep
            } else {
                policy_keep
            };
            if keep {
                cleanup.keep_for_resume();
            }
        }
        result
    }
}

const TRANSCRIPT_MISSING_WARNING: &str = "TRANSCRIPT_MISSING";

/// Why transcript post-processing failed, split by what the caller may do.
#[derive(Debug)]
enum TranscriptProcessError {
    /// The transcript itself is unusable (malformed or empty VTT, normalization
    /// failure, wrong language). The downloaded video is still good.
    Content(String),
    /// The helper emitted several tracks, so no single transcript can be trusted.
    Ambiguous(String),
    /// Staging I/O or validation failed. Never a reason to keep a result.
    Staging(String),
}

impl TranscriptProcessError {
    fn code(&self) -> &'static str {
        match self {
            Self::Content(_) => "TRANSCRIPT_INVALID",
            Self::Ambiguous(_) => "TRANSCRIPT_AMBIGUOUS",
            Self::Staging(_) => "OUTPUT_VERIFY_FAILED",
        }
    }

    fn keeps_video(&self) -> bool {
        matches!(self, Self::Content(_))
    }
}

impl std::fmt::Display for TranscriptProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Content(message) | Self::Ambiguous(message) | Self::Staging(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl From<String> for TranscriptProcessError {
    fn from(message: String) -> Self {
        Self::Staging(message)
    }
}

fn truncate_reason(reason: &str) -> String {
    let single_line = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= MAX_WARNING_REASON_CHARS {
        return single_line;
    }
    let mut shortened = single_line
        .chars()
        .take(MAX_WARNING_REASON_CHARS)
        .collect::<String>();
    shortened.push('…');
    shortened
}

/// Remove the raw VTT and any normalized transcript JSON from a staging
/// attempt after transcript processing failed, so only the video is published.
/// Contents are re-validated first: an unsafe staging directory is a real
/// failure, not a transcript problem, and is reported to the caller.
fn discard_transcript_artifacts(attempt: &OutputAttemptLease) -> Result<(), String> {
    attempt
        .validate_contents()
        .map_err(|error| error.to_string())?;
    let mut doomed = Vec::new();
    for entry in fs::read_dir(attempt.path()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| "staging contained a non-UTF-8 name".to_string())?;
        validate_output_component(&name).map_err(|error| error.to_string())?;
        if is_vtt_name(&name) || name.to_ascii_lowercase().ends_with(".transcript.json") {
            doomed.push(name);
        }
    }
    for name in doomed {
        fs::remove_file(attempt.path().join(&name)).map_err(|error| error.to_string())?;
    }
    attempt.revalidate().map_err(|error| error.to_string())
}

fn verify_existing_item(
    lease: &ExistingOutputDirectoryLease,
    executor: &YouTubeExecutor,
    item: &TransientWorkItem,
    control: &TransientRunControl,
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>, ExistingOutputVerificationError> {
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
            return Err(format!("existing output contains an incomplete artifact: {name}").into());
        }
        let held = hash_existing_regular_file(lease, &name)?;
        artifacts.push(held.artifact.clone());
        handles.push(held);
    }
    artifacts.sort_by(|left, right| left.name.cmp(&right.name));
    if artifacts.is_empty() {
        return Err("existing output has no artifacts".to_string().into());
    }
    // Structure is checked as if a transcript were optional; a request that
    // requires one is enforced below, once the folder is proven valid, so the
    // user gets guidance instead of a generic "not an exact match".
    let artifact_set = validate_artifact_set(&executor.mode, true, &artifacts)?;
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
        return Err(
            "existing manifest is empty, oversized, or not a regular file"
                .to_string()
                .into(),
        );
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    manifest
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let parsed: crate::providers::youtube::manifest_contract::YouTubeArtifactManifest =
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("existing manifest is invalid: {error}"))?;
    if parsed != expected || bytes != expected_bytes {
        return Err(
            "existing manifest does not match the current verified projection"
                .to_string()
                .into(),
        );
    }
    if artifact_set.has_media {
        verify_existing_media_with_ffprobe(
            lease,
            executor
                .item_context(item)
                .map_err(|message| message.to_string())?,
            &artifacts,
            control,
            runner,
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
    if !artifact_set.has_transcript
        && matches!(&executor.mode, YouTubeDownloadMode::VideoAndTranscript)
        && !executor.continue_without_transcript
    {
        return Err(ExistingOutputVerificationError::TranscriptRequired(
            format!(
                "The video is already saved in \"{}\" without its transcript, and this download \
             requires one. Delete or move that folder to download it again, or allow \
             downloads to continue without a transcript.",
                lease.path().display()
            ),
        ));
    }
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
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>, FfprobeVerificationError> {
    let media = artifacts
        .iter()
        .filter(|artifact| artifact.kind == "media")
        .collect::<Vec<_>>();
    if media.len() != 1 {
        return Err("media execution must produce exactly one media artifact"
            .to_string()
            .into());
    }
    attempt.revalidate().map_err(|error| error.to_string())?;
    let media_path = attempt.path().join(&media[0].name);
    let input_path = media_path.to_string_lossy().into_owned();
    let output = runner
        .run(
            media_probe_invocation(
                ffmpeg_copy_probe_args(&input_path, MediaMode::VideoAndAudio),
                media[0].size_bytes,
            ),
            control,
        )
        .map_err(FfprobeVerificationError::ManagedProcess)?;
    attempt.revalidate().map_err(|error| error.to_string())?;
    if output.cancelled || control.is_cancelled() {
        return Err("media verification was cancelled".to_string().into());
    }
    if output.timed_out {
        return Err("The FFmpeg media check timed out".to_string().into());
    }
    if output.stdout_truncated {
        return Err("The FFmpeg media check output exceeded the safety limit"
            .to_string()
            .into());
    }
    if !output.status.success() {
        let message = "The downloaded file failed the FFmpeg media check: it could not be read"
            .to_string();
        // Only FFmpeg's own words about the data prove corruption; a permission
        // or I/O failure proves nothing and must not cost the download.
        return Err(if ffmpeg_stderr_proves_invalid(&output.stderr) {
            FfprobeVerificationError::Corrupt(message)
        } else {
            FfprobeVerificationError::Message(message)
        });
    }
    let verified = verify_ffmpeg_copy_probe(
        &FfmpegCopyProbeOutput {
            progress: &output.stdout,
            log: &output.stderr,
            log_truncated: output.stderr_truncated,
            input_path: &input_path,
        },
        MediaMode::VideoAndAudio,
        context.source_duration_seconds.map(|value| value as f64),
    )
    .map_err(|error| {
        if matches!(
            error,
            MediaVerificationError::NoFramesCopied
                | MediaVerificationError::MissingVideoStream
                | MediaVerificationError::MissingAudioStream
        ) {
            FfprobeVerificationError::Corrupt(error.to_string())
        } else {
            FfprobeVerificationError::Message(error.to_string())
        }
    })?;
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
    runner: &dyn ProcessRunner,
) -> Result<(), FfprobeVerificationError> {
    let media = artifacts
        .iter()
        .filter(|artifact| artifact.kind == "media")
        .collect::<Vec<_>>();
    if media.len() != 1 {
        return Err("media reuse requires exactly one media artifact"
            .to_string()
            .into());
    }
    lease.revalidate().map_err(|error| error.to_string())?;
    let media_path = lease.path().join(&media[0].name);
    let input_path = media_path.to_string_lossy().into_owned();
    let output = runner
        .run(
            media_probe_invocation(
                ffmpeg_copy_probe_args(&input_path, MediaMode::VideoAndAudio),
                media[0].size_bytes,
            ),
            control,
        )
        .map_err(FfprobeVerificationError::ManagedProcess)?;
    lease.revalidate().map_err(|error| error.to_string())?;
    if output.cancelled || control.is_cancelled() {
        return Err("media reuse verification was cancelled".to_string().into());
    }
    if output.timed_out {
        return Err("The FFmpeg media check of the existing file timed out".to_string().into());
    }
    if output.stdout_truncated {
        return Err(
            "The FFmpeg media check output for the existing file exceeded the safety limit"
                .to_string()
                .into(),
        );
    }
    if !output.status.success() {
        return Err("The existing file failed the FFmpeg media check: it could not be read"
            .to_string()
            .into());
    }
    verify_ffmpeg_copy_probe(
        &FfmpegCopyProbeOutput {
            progress: &output.stdout,
            log: &output.stderr,
            log_truncated: output.stderr_truncated,
            input_path: &input_path,
        },
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
        // Resume bookkeeping, not an artifact; removed just before publication.
        if is_bookkeeping_leaf(name) {
            continue;
        }
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
) -> Result<bool, TranscriptProcessError> {
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
            return Err(format!("staging contained an incomplete artifact: {name}").into());
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
        return Err(TranscriptProcessError::Ambiguous(format!(
            "transcript helper emitted multiple VTT tracks: {}",
            vtt_names.join(", ")
        )));
    }
    let vtt_name = vtt_names
        .pop()
        .ok_or_else(|| "transcript VTT selection unexpectedly empty".to_string())?;
    if !transcript_filename_matches_language(&vtt_name, selected) {
        return Err(TranscriptProcessError::Content(format!(
            "transcript VTT filename does not match selected language {}: {vtt_name}",
            selected.language_tag
        )));
    }
    let json_name = transcript_json_name(&vtt_name)?;
    let mut vtt = attempt
        .open_leaf(&vtt_name)
        .map_err(|error| error.to_string())?;
    let vtt_len = vtt.metadata().map_err(|error| error.to_string())?.len();
    if vtt_len == 0 {
        return Err(TranscriptProcessError::Content(format!(
            "transcript VTT is empty: {vtt_name}"
        )));
    }
    if vtt_len > MAX_VTT_BYTES as u64 {
        return Err(TranscriptProcessError::Content(format!(
            "{vtt_name} exceeds the {MAX_VTT_BYTES}-byte safety limit"
        )));
    }
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
    .map_err(|error| match error {
        // Our own metadata or serializer failing says nothing about the track.
        TranscriptNormalizationError::EmptyMetadata { .. }
        | TranscriptNormalizationError::Serialization { .. } => {
            TranscriptProcessError::Staging(transcript_normalization_error(error))
        }
        error => TranscriptProcessError::Content(transcript_normalization_error(error)),
    })?;
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
        MAX_NORMALIZED_TRANSCRIPT_JSON_BYTES as u64,
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

#[cfg(test)]
fn safe_stem(ordinal: u32, title: &str, video_id: &str) -> String {
    safe_stem_with_budget(ordinal, title, video_id, usize::MAX)
        .expect("the default YouTube stem budget must fit its fixed identity")
}

fn safe_stem_for_root(
    output_root: &ValidatedOutputRoot,
    ordinal: u32,
    title: &str,
    video_id: &str,
) -> Result<String, SafeOutputError> {
    safe_stem_with_budget(
        ordinal,
        title,
        video_id,
        output_root.direct_child_name_budget(),
    )
}

/// Final item directory name the executor publishes under, computed from the
/// stored output root without admitting (or writing to) it. Playback uses this
/// to find an item folder from durable run state instead of scanning.
pub(super) fn published_item_name(
    root_path: &Path,
    ordinal: u32,
    title: &str,
    video_id: &str,
) -> Result<String, SafeOutputError> {
    safe_stem_with_budget(
        ordinal,
        title,
        video_id,
        direct_child_name_budget_for(root_path),
    )
}

fn safe_stem_with_budget(
    ordinal: u32,
    title: &str,
    video_id: &str,
    max_units: usize,
) -> Result<String, SafeOutputError> {
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
    let ordinal_label = format!("{ordinal:03}");
    let fixed_units =
        ordinal_label.encode_utf16().count() + safe_video_id.encode_utf16().count() + 2;
    let title_budget = max_units.saturating_sub(fixed_units).min(80);
    if title_budget == 0 {
        return Err(SafeOutputError::PathTooLong);
    }
    let fixed_utf8 = ordinal_label.len() + safe_video_id.len() + 2;
    let title_utf8_budget = 240usize.saturating_sub(fixed_utf8);
    if title_utf8_budget == 0 {
        return Err(SafeOutputError::PathTooLong);
    }

    let mut stem = sanitize_title_stem(title);
    if stem.is_empty() {
        stem = "youtube-video".to_string();
    }

    let mut used_video_fallback = stem == "video";
    loop {
        stem = trim_stem_edges(&truncate_title(&stem, title_budget, title_utf8_budget));
        if stem.is_empty() {
            if used_video_fallback {
                return Err(SafeOutputError::PathTooLong);
            }
            used_video_fallback = true;
            stem = "video".to_string();
            continue;
        }
        let full = format!("{ordinal_label}-{stem}-{safe_video_id}");
        if full.encode_utf16().count() <= max_units && validate_output_component(&full).is_ok() {
            return Ok(full);
        }
        let mut chars = stem.chars().collect::<Vec<_>>();
        if chars.len() <= 1 {
            if used_video_fallback {
                return Err(SafeOutputError::PathTooLong);
            }
            used_video_fallback = true;
            stem = "video".to_string();
            continue;
        }
        chars.pop();
        stem = chars.into_iter().collect();
    }
}

fn sanitize_title_stem(title: &str) -> String {
    let mut sanitized = String::with_capacity(title.len());
    let mut last_was_space = false;
    let mut last_was_underscore = false;
    for character in title.chars() {
        if character.is_control() || is_windows_filename_illegal(character) {
            continue;
        }
        let mapped = if character.is_whitespace() {
            ' '
        } else if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
            character
        } else {
            continue;
        };
        if mapped == ' ' {
            if last_was_space || sanitized.is_empty() {
                continue;
            }
            last_was_space = true;
            last_was_underscore = false;
            sanitized.push(' ');
            continue;
        }
        if mapped == '_' {
            if last_was_underscore {
                continue;
            }
            last_was_underscore = true;
            last_was_space = false;
            sanitized.push('_');
            continue;
        }
        last_was_space = false;
        last_was_underscore = false;
        sanitized.push(mapped);
    }
    trim_stem_edges(&sanitized)
}

fn is_windows_filename_illegal(character: char) -> bool {
    matches!(
        character,
        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
    )
}

fn trim_stem_edges(value: &str) -> String {
    value
        .trim_matches(|character: char| {
            character == '.' || character == ' ' || character.is_whitespace()
        })
        .to_string()
}

fn truncate_title(value: &str, max_utf16: usize, max_utf8: usize) -> String {
    let mut units = 0;
    let mut bytes = 0;
    value
        .chars()
        .take_while(|character| {
            let width = character.len_utf16();
            let size = character.len_utf8();
            if units + width > max_utf16 || bytes + size > max_utf8 {
                false
            } else {
                units += width;
                bytes += size;
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

fn map_fresh_ffprobe_error(error: FfprobeVerificationError) -> TransientError {
    match error {
        FfprobeVerificationError::ManagedProcess(ManagedProcessError::HelperTemp(reason)) => {
            transient_error("HELPER_TEMP_ROOT_INVALID", reason)
        }
        FfprobeVerificationError::ManagedProcess(ManagedProcessError::Integrity(reason)) => {
            transient_error("HELPER_INTEGRITY_FAILED", reason)
        }
        FfprobeVerificationError::ManagedProcess(error) => {
            transient_error("MEDIA_VERIFY_FAILED", error.to_string())
        }
        FfprobeVerificationError::Message(message) => {
            transient_error("MEDIA_VERIFY_FAILED", message)
        }
        FfprobeVerificationError::Corrupt(message) => transient_error("MEDIA_CORRUPT", message),
    }
}

fn map_existing_output_verification_error(
    error: ExistingOutputVerificationError,
) -> TransientError {
    match error {
        ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::ManagedProcess(
            ManagedProcessError::HelperTemp(reason),
        )) => transient_error("HELPER_TEMP_ROOT_INVALID", reason),
        ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::ManagedProcess(
            ManagedProcessError::Integrity(reason),
        )) => transient_error("HELPER_INTEGRITY_FAILED", reason),
        ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::ManagedProcess(
            error,
        )) => existing_output_collision_error(error.to_string()),
        ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::Message(message))
        | ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::Corrupt(message))
        | ExistingOutputVerificationError::Message(message) => {
            existing_output_collision_error(message)
        }
        ExistingOutputVerificationError::TranscriptRequired(message) => {
            transient_error("EXISTING_OUTPUT_MISSING_TRANSCRIPT", message)
        }
    }
}

fn existing_output_collision_error(message: impl Into<String>) -> TransientError {
    transient_error(
        "OUTPUT_COLLISION",
        format!(
            "existing output is not an exact verified match: {}",
            message.into()
        ),
    )
}

/// Map a staging-lease failure; a directory another attempt holds is its
/// own code so nothing mistakes it for a bad output folder.
fn staging_error(error: SafeOutputError) -> TransientError {
    match error {
        SafeOutputError::StagingBusy { .. } => transient_error(
            "STAGING_BUSY",
            "Another download is already using this video's temporary folder.",
        ),
        other => safe_output_error(other),
    }
}

/// A failed helper run, classified so a full disk or a removed video gets its
/// own code and wording instead of a generic helper failure.
fn helper_failure(stderr: &str, summary: &str) -> TransientError {
    // Classified from yt-dlp's own diagnostics; only the fixed summary is
    // shown, because the raw text can carry signed media URLs.
    match classify_helper_failure(stderr) {
        FailureKind::DiskFull => transient_error("DISK_FULL", pause_reason("DISK_FULL", summary)),
        FailureKind::VideoUnavailable => {
            transient_error("VIDEO_UNAVAILABLE", pause_reason("VIDEO_UNAVAILABLE", summary))
        }
        FailureKind::Other => transient_error("HELPER_FAILED", summary),
    }
}

/// The format selection recorded in a reused staging directory, if it is
/// present and plain enough to hand back to yt-dlp.
fn read_format_marker(lease: &OutputAttemptLease) -> Option<String> {
    let selection = read_text_marker(lease, FORMAT_MARKER, 256)?;
    parse_pinned_formats(&selection).map(|_| selection)
}

fn read_text_marker(lease: &OutputAttemptLease, name: &str, max_bytes: u64) -> Option<String> {
    let mut file = lease.open_leaf(name).ok()?;
    let mut text = String::new();
    Read::read_to_string(&mut Read::take(&mut file, max_bytes), &mut text).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn write_text_marker(lease: &OutputAttemptLease, name: &str, text: &str) -> Result<(), String> {
    let _ = lease.remove_leaf(name);
    let mut file = lease.create_leaf(name).map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}

fn write_format_marker(lease: &OutputAttemptLease, selection: &str) -> Result<(), String> {
    write_text_marker(lease, FORMAT_MARKER, selection)
}

/// Remove every bookkeeping marker; none of them is ever published.
fn remove_bookkeeping(lease: &OutputAttemptLease) -> Result<(), String> {
    for name in lease.leaf_names().map_err(|error| error.to_string())? {
        if is_bookkeeping_leaf(&name) {
            lease.remove_leaf(&name).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Throw away a finished download (media and the markers that describe it) so
/// the next attempt fetches it again with the normal format selection. Subtitles stay.
fn discard_finished_download(lease: &OutputAttemptLease) -> Result<(), String> {
    for name in lease.leaf_names().map_err(|error| error.to_string())? {
        if is_media_leaf(&name) || is_bookkeeping_leaf(&name) {
            lease.remove_leaf(&name).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Remember which check failed on a finished download. The same failure twice
/// in a row means retrying the check cannot help, so the download is discarded
/// and fetched again instead of looping.
fn note_verification_failure(lease: &OutputAttemptLease, error: TransientError) -> TransientError {
    if read_text_marker(lease, VERIFY_MARKER, 64).as_deref() == Some(error.code.as_str()) {
        let _ = discard_finished_download(lease);
        return transient_error(
            "MEDIA_CORRUPT",
            "Verification failed twice in a row, so the download was removed and will be fetched again.",
        );
    }
    let _ = write_text_marker(lease, VERIFY_MARKER, &error.code);
    error
}

/// Delete files derived from the media (normalized transcripts, manifest) so a
/// resumed attempt regenerates them instead of colliding with leftovers.
fn remove_derived_files(lease: &OutputAttemptLease) -> Result<(), String> {
    for name in lease.leaf_names().map_err(|error| error.to_string())? {
        if name == "manifest.json" || name.to_ascii_lowercase().ends_with(".transcript.json") {
            lease.remove_leaf(&name).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Remove only the unfinished-download files; completed files stay.
fn wipe_incomplete(lease: &OutputAttemptLease) -> Result<(), String> {
    let names = lease.leaf_names().map_err(|error| error.to_string())?;
    for name in incomplete_leaves(&names) {
        lease.remove_leaf(&name).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Remove every file in the staging directory (partials of unknown or
/// outdated provenance); the directory itself stays leased.
fn wipe_staging(lease: &OutputAttemptLease) -> Result<(), String> {
    for name in lease.leaf_names().map_err(|error| error.to_string())? {
        lease.remove_leaf(&name).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Delete staging files that belong to formats other than `chosen`.
fn purge_stale_parts(lease: &OutputAttemptLease, chosen: &[String]) -> Result<(), String> {
    if chosen.is_empty() {
        return Ok(());
    }
    let names = lease.leaf_names().map_err(|error| error.to_string())?;
    for stale in stale_part_leaves(&names, chosen) {
        lease.remove_leaf(&stale).map_err(|error| error.to_string())?;
    }
    Ok(())
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

    const HELPER_TEMP_ERROR_MESSAGE: &str =
        "the helper temporary workspace could not be validated or cleaned";

    #[test]
    fn fresh_ffprobe_helper_temp_maps_to_typed_transient_error() {
        let error = map_fresh_ffprobe_error(FfprobeVerificationError::ManagedProcess(
            ManagedProcessError::HelperTemp(HELPER_TEMP_ERROR_MESSAGE.to_string()),
        ));

        assert_eq!(error.code, "HELPER_TEMP_ROOT_INVALID");
        assert_eq!(error.message, HELPER_TEMP_ERROR_MESSAGE);
    }

    #[test]
    fn helper_integrity_failures_keep_their_code_through_media_verification() {
        let fresh = map_fresh_ffprobe_error(FfprobeVerificationError::ManagedProcess(
            ManagedProcessError::Integrity("ffmpeg hash mismatch".to_string()),
        ));
        assert_eq!(fresh.code, "HELPER_INTEGRITY_FAILED");
        let reuse = map_existing_output_verification_error(
            ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::ManagedProcess(
                ManagedProcessError::Integrity("x".to_string()),
            )),
        );
        assert_eq!(reuse.code, "HELPER_INTEGRITY_FAILED");
        let ordinary =
            map_fresh_ffprobe_error(FfprobeVerificationError::Message("bad media".to_string()));
        assert_eq!(ordinary.code, "MEDIA_VERIFY_FAILED");
    }

    #[test]
    fn existing_ffprobe_helper_temp_maps_to_typed_transient_error() {
        let error = map_existing_output_verification_error(
            ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::ManagedProcess(
                ManagedProcessError::HelperTemp(HELPER_TEMP_ERROR_MESSAGE.to_string()),
            )),
        );

        assert_eq!(error.code, "HELPER_TEMP_ROOT_INVALID");
        assert_eq!(error.message, HELPER_TEMP_ERROR_MESSAGE);
    }

    #[test]
    fn fresh_ffprobe_managed_process_errors_keep_media_fallback_code() {
        let error = map_fresh_ffprobe_error(FfprobeVerificationError::ManagedProcess(
            ManagedProcessError::Start("test failure".to_string()),
        ));

        assert_eq!(error.code, "MEDIA_VERIFY_FAILED");
        assert!(error.message.contains("test failure"));
    }

    #[test]
    fn existing_ffprobe_managed_process_errors_keep_reuse_fallback_code() {
        let error = map_existing_output_verification_error(
            ExistingOutputVerificationError::Ffprobe(FfprobeVerificationError::ManagedProcess(
                ManagedProcessError::Start("test failure".to_string()),
            )),
        );

        assert_eq!(error.code, "OUTPUT_COLLISION");
        assert!(error
            .message
            .contains("existing output is not an exact verified match"));
        assert!(error.message.contains("test failure"));
    }

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
        for required in [
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-cache-dir",
            "--continue",
            "--part",
            "--windows-filenames",
            "--concurrent-fragments",
        ] {
            assert!(args.iter().any(|argument| argument == required));
        }
        for (option, expected) in [
            ("--retries", "10"),
            ("--fragment-retries", "10"),
            ("--file-access-retries", "10"),
            ("--concurrent-fragments", "4"),
        ] {
            let index = args.iter().position(|argument| argument == option).unwrap();
            assert_eq!(args[index + 1], expected);
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
        let compact_scope = staging.parent().unwrap();
        assert_eq!(fs::read_dir(compact_scope).unwrap().count(), 0);
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

    #[test]
    fn final_stem_shrinks_to_fit_a_deep_output_root() {
        let temp = tempdir().unwrap();
        let base_units = temp.path().to_string_lossy().encode_utf16().count() + 1;
        let component_units = 180usize.saturating_sub(base_units).max(1);
        let deep_root = temp.path().join("r".repeat(component_units));
        fs::create_dir(&deep_root).unwrap();
        let root = validate_output_root(&deep_root).unwrap();
        let stem = safe_stem_for_root(
            &root,
            1,
            &"A very long descriptive title ".repeat(12),
            "video-1",
        )
        .unwrap();
        assert!(
            deep_root
                .join(stem)
                .to_string_lossy()
                .encode_utf16()
                .count()
                <= 240
        );
    }

    #[test]
    fn safe_stem_keeps_cjk_title_characters() {
        let stem = safe_stem(1, "日本語タイトル", "f7TNxVmRrtc");
        assert_eq!(stem, "001-日本語タイトル-f7TNxVmRrtc");
        assert!(stem.contains("日本語タイトル"));
        assert!(!stem.contains("___"));
        validate_output_component(&stem).unwrap();
    }

    #[test]
    fn safe_stem_does_not_replace_cjk_with_underscore_runs() {
        let stem = safe_stem(1, "这是一个中文视频标题", "f7TNxVmRrtc");
        assert_eq!(stem, "001-这是一个中文视频标题-f7TNxVmRrtc");
        let underscore_run = stem.chars().filter(|character| *character == '_').count();
        assert_eq!(underscore_run, 0);
        validate_output_component(&stem).unwrap();
    }

    #[test]
    fn safe_stem_strips_illegal_characters_and_collapses_separators() {
        let stem = safe_stem(2, r#"Foo<>:"/\|?*   Bar___Baz. 🎬 中文"#, "abc-id");
        assert_eq!(stem, "002-Foo Bar_Baz. 中文-abc-id");
        validate_output_component(&stem).unwrap();
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
        assert!(error
            .to_string()
            .contains("does not match selected language"));
        assert!(error.keeps_video());
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
        assert!(error.to_string().contains("WEBVTT"));
        assert_eq!(error.code(), "TRANSCRIPT_INVALID");
        assert!(!attempt
            .path()
            .join(transcript_json_name(raw_name).unwrap())
            .exists());
        drop(executor);
        root.discard_attempt_lease(attempt).unwrap();
    }

    // ---- Execute-level tests with a fake process launcher -----------------

    use std::cell::Cell;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// One step of the stdout a fake yt-dlp download prints.
    #[derive(Clone, Debug)]
    enum ScriptStep {
        Line(String),
        AdvanceClockMs(u64),
    }

    thread_local! {
        static CLOCK_BASE: Instant = Instant::now();
        static CLOCK_OFFSET_MS: Cell<u64> = const { Cell::new(0) };
    }

    /// Deterministic per-thread clock for the progress throttle; the fake
    /// runner advances it, so no test sleeps.
    fn test_clock() -> Instant {
        CLOCK_BASE.with(|base| *base) + Duration::from_millis(CLOCK_OFFSET_MS.with(Cell::get))
    }

    fn advance_test_clock(millis: u64) {
        CLOCK_OFFSET_MS.with(|offset| offset.set(offset.get() + millis));
    }

    /// Stands in for yt-dlp and FFmpeg: the download call writes the media
    /// file and, when the request asks for subtitles, a VTT with the chosen
    /// content into the staging directory; the probe call reports a clean copy.
    #[derive(Debug)]
    struct FakeRunner {
        vtt: &'static str,
        downloads: AtomicUsize,
        script: Vec<ScriptStep>,
        /// Also write a second language track, as a misbehaving helper might.
        second_vtt: bool,
        /// The next download leaves this partial file and fails with this
        /// stderr, like a connection that dropped mid-download.
        fail_next: std::sync::Mutex<Option<(String, usize, String)>>,
        /// Files already in staging when a download starts: `(name, bytes)`.
        seen_at_start: std::sync::Mutex<Vec<(String, u64)>>,
        /// Stdout lines the planned failure prints before it fails.
        fail_lines: std::sync::Mutex<Vec<String>>,
        /// Arguments of every download call.
        seen_args: std::sync::Mutex<Vec<Vec<String>>>,
        /// Refuse any pinned `--format` as no longer available.
        reject_pinned: std::sync::atomic::AtomicBool,
        /// Make the next media check fail: 1 = timed out, 2 = unreadable file.
        probe_failure: std::sync::atomic::AtomicUsize,
        /// Cancel the run while the next download is in flight.
        cancel_download: std::sync::atomic::AtomicBool,
        /// Every download call fails as if YouTube were unreachable.
        download_unreachable: std::sync::atomic::AtomicBool,
        /// stderr of a failing media check.
        probe_stderr: std::sync::Mutex<String>,
    }

    impl FakeRunner {
        fn new(vtt: &'static str) -> Arc<Self> {
            Self::with_script(vtt, Vec::new())
        }

        fn with_script(vtt: &'static str, script: Vec<ScriptStep>) -> Arc<Self> {
            Arc::new(Self {
                vtt,
                downloads: AtomicUsize::new(0),
                script,
                second_vtt: false,
                fail_next: std::sync::Mutex::new(None),
                seen_at_start: std::sync::Mutex::new(Vec::new()),
                fail_lines: std::sync::Mutex::new(Vec::new()),
                seen_args: std::sync::Mutex::new(Vec::new()),
                reject_pinned: std::sync::atomic::AtomicBool::new(false),
                probe_failure: std::sync::atomic::AtomicUsize::new(0),
                cancel_download: std::sync::atomic::AtomicBool::new(false),
                download_unreachable: std::sync::atomic::AtomicBool::new(false),
                probe_stderr: std::sync::Mutex::new(String::new()),
            })
        }

        fn with_second_vtt(vtt: &'static str) -> Arc<Self> {
            Arc::new(Self {
                vtt,
                downloads: AtomicUsize::new(0),
                script: Vec::new(),
                second_vtt: true,
                fail_next: std::sync::Mutex::new(None),
                seen_at_start: std::sync::Mutex::new(Vec::new()),
                fail_lines: std::sync::Mutex::new(Vec::new()),
                seen_args: std::sync::Mutex::new(Vec::new()),
                reject_pinned: std::sync::atomic::AtomicBool::new(false),
                probe_failure: std::sync::atomic::AtomicUsize::new(0),
                cancel_download: std::sync::atomic::AtomicBool::new(false),
                download_unreachable: std::sync::atomic::AtomicBool::new(false),
                probe_stderr: std::sync::Mutex::new(String::new()),
            })
        }

        fn execute(
            &self,
            spec: ManagedProcessSpec,
            on_line: &mut dyn FnMut(&str),
        ) -> Result<ManagedProcessOutput, ManagedProcessError> {
            let args = spec
                .args
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let (stdout, stderr) = if args.iter().any(|argument| argument == "--ignore-config") {
                self.downloads.fetch_add(1, Ordering::SeqCst);
                let template = arg_after(&args, "--output").expect("download has --output");
                let staging = Path::new(&template).parent().expect("template parent");
                {
                    let mut seen = self.seen_at_start.lock().unwrap();
                    seen.clear();
                    for entry in fs::read_dir(staging).unwrap() {
                        let entry = entry.unwrap();
                        seen.push((
                            entry.file_name().to_string_lossy().into_owned(),
                            entry.metadata().unwrap().len(),
                        ));
                    }
                }
                self.seen_args.lock().unwrap().push(args.clone());
                if self.download_unreachable.load(Ordering::SeqCst) {
                    return Ok(ManagedProcessOutput {
                        status: std::process::ExitStatus::from_raw(1),
                        stdout: String::new(),
                        stderr: "ERROR: unable to download webpage: network is unreachable".to_string(),
                        stdout_truncated: false,
                        stderr_truncated: false,
                        timed_out: false,
                        cancelled: false,
                    });
                }
                if self.reject_pinned.load(Ordering::SeqCst) {
                    let format = arg_after(&args, "--format").unwrap_or_default();
                    if !format.starts_with("bestvideo") {
                        return Ok(ManagedProcessOutput {
                            status: std::process::ExitStatus::from_raw(1),
                            stdout: String::new(),
                            stderr: "ERROR: Requested format is not available".to_string(),
                            stdout_truncated: false,
                            stderr_truncated: false,
                            timed_out: false,
                            cancelled: false,
                        });
                    }
                }
                let planned_failure = self.fail_next.lock().unwrap().take();
                if let Some((part_name, part_bytes, stderr)) = planned_failure {
                    for line in self.fail_lines.lock().unwrap().iter() {
                        on_line(line);
                    }
                    fs::write(staging.join(part_name), vec![9u8; part_bytes]).unwrap();
                    return Ok(ManagedProcessOutput {
                        status: std::process::ExitStatus::from_raw(1),
                        stdout: String::new(),
                        stderr,
                        stdout_truncated: false,
                        stderr_truncated: false,
                        timed_out: false,
                        cancelled: false,
                    });
                }
                if !args.iter().any(|argument| argument == "--skip-download") {
                    // A completed download consumes the stream it was resuming.
                    let _ = fs::remove_file(staging.join("media.f137.mp4.part"));
                    let _ = fs::remove_file(staging.join("media.mp4.part"));
                    fs::write(staging.join("media.mp4"), b"media bytes").unwrap();
                }
                if args.iter().any(|argument| argument == "--write-subs") {
                    fs::write(staging.join("media.en-US.vtt"), self.vtt).unwrap();
                    if self.second_vtt {
                        fs::write(staging.join("media.fr-FR.vtt"), self.vtt).unwrap();
                    }
                }
                for step in &self.script {
                    match step {
                        ScriptStep::Line(line) => on_line(line),
                        ScriptStep::AdvanceClockMs(millis) => advance_test_clock(*millis),
                    }
                }
                (String::new(), String::new())
            } else {
                let failure = self.probe_failure.swap(0, Ordering::SeqCst);
                if failure != 0 {
                    return Ok(ManagedProcessOutput {
                        status: std::process::ExitStatus::from_raw(if failure == 2 { 1 } else { 0 }),
                        stdout: String::new(),
                        stderr: self.probe_stderr.lock().unwrap().clone(),
                        stdout_truncated: false,
                        stderr_truncated: false,
                        timed_out: failure == 1,
                        cancelled: false,
                    });
                }
                let input = arg_after(&args, "-i").expect("probe has -i");
                (
                    "frame=10\nout_time_us=1000000\nprogress=end\n".to_string(),
                    format!(
                        "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from '{input}':\n\
                         Output #0, null, to 'pipe:':\n  \
                         Stream #0:0: Video: h264, yuv420p\n  \
                         Stream #0:1: Audio: aac, 48000 Hz\n"
                    ),
                )
            };
            Ok(ManagedProcessOutput {
                status: std::process::ExitStatus::from_raw(0),
                stdout,
                stderr,
                stdout_truncated: false,
                stderr_truncated: false,
                timed_out: false,
                cancelled: false,
            })
        }
    }

    fn arg_after(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|argument| argument == flag)
            .and_then(|index| args.get(index + 1).cloned())
    }

    impl ProcessRunner for FakeRunner {
        fn run(
            &self,
            spec: ManagedProcessSpec,
            _control: &TransientRunControl,
        ) -> Result<ManagedProcessOutput, ManagedProcessError> {
            self.execute(spec, &mut |_| {})
        }

        fn run_streaming(
            &self,
            spec: ManagedProcessSpec,
            control: &TransientRunControl,
            on_line: &mut dyn FnMut(&str),
        ) -> Result<ManagedProcessOutput, ManagedProcessError> {
            if self.cancel_download.swap(false, Ordering::SeqCst) {
                control.request_cancel();
            }
            self.execute(spec, on_line)
        }
    }

    fn item_for_mode(
        mode: &YouTubeDownloadMode,
        selected: Option<SelectedTranscript>,
    ) -> (TransientWorkItem, YouTubeExecutorContext) {
        let (mut item, mut context) = transcript_item(selected.clone());
        item.artifact_fingerprint = artifact_fingerprint(&ArtifactFingerprintInput {
            occurrence_id: item.occurrence_id.clone(),
            video_id: item.video_id.clone(),
            mode: mode.clone(),
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: None,
            selected_transcript: selected,
            helper_lock_digest: context.helper_lock_digest.clone(),
        })
        .unwrap();
        context.items[0].source_duration_seconds = None;
        (item, context)
    }

    fn executor_with_fake(
        root: &ValidatedOutputRoot,
        mode: YouTubeDownloadMode,
        continue_without_transcript: bool,
        runner: Arc<FakeRunner>,
    ) -> (Arc<YouTubeExecutor>, TransientWorkItem) {
        let (item, context) = item_for_mode(&mode, Some(selected_uploader()));
        let mut request = transcript_request();
        request.mode = mode;
        request.continue_without_transcript = continue_without_transcript;
        let executor =
            YouTubeExecutor::new_with_runner(root.clone(), &request, context, runner).unwrap();
        (executor, item)
    }

    fn run_collecting(
        executor: &YouTubeExecutor,
        item: &TransientWorkItem,
    ) -> (
        Result<TransientExecutionOutcome, TransientError>,
        Vec<(TransientItemPhase, f64)>,
    ) {
        let mut stages = Vec::new();
        let control = TransientRunControl::default();
        let result = TransientExecutor::execute(executor, item, &control, &mut |update| {
            stages.push((update.phase, update.fraction.unwrap_or(-1.0)));
        });
        (result, stages)
    }

    fn published_files(temp: &Path) -> Vec<String> {
        let mut names = Vec::new();
        for entry in fs::read_dir(temp).unwrap() {
            let entry = entry.unwrap();
            if !entry.file_type().unwrap().is_dir() {
                continue;
            }
            for child in fs::read_dir(entry.path()).unwrap() {
                names.push(child.unwrap().file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        names
    }

    fn staging_dirs(temp: &Path) -> Vec<String> {
        let lv = temp.join(".lv");
        if !lv.exists() {
            return Vec::new();
        }
        let mut names = fs::read_dir(lv)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("y-") && !name.ends_with(".lock"))
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn an_interrupted_attempt_keeps_its_part_file_and_the_next_attempt_resumes_from_it() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        *runner.fail_lines.lock().unwrap() = vec!["LVFORMATID 137".to_string()];
        *runner.fail_next.lock().unwrap() = Some((
            "media.f137.mp4.part".to_string(),
            5000,
            "ERROR: unable to download video data: HTTP Error 403: Forbidden".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );

        let (first, _) = run_collecting(&executor, &item);
        assert_eq!(first.unwrap_err().code, "HELPER_FAILED");
        let kept = staging_dirs(temp.path());
        assert_eq!(kept.len(), 1, "the partial download must be kept: {kept:?}");
        let part = temp.path().join(".lv").join(&kept[0]).join("media.f137.mp4.part");
        assert_eq!(fs::metadata(&part).unwrap().len(), 5000);

        let (second, stages) = run_collecting(&executor, &item);
        second.expect("the resumed attempt completes");
        let mut at_start = runner.seen_at_start.lock().unwrap().clone();
        at_start.sort();
        assert_eq!(
            at_start,
            vec![
                ("media.f137.mp4.part".to_string(), 5000u64),
                ("media.lvformat".to_string(), 3u64),
            ],
            "the resumed attempt must start with the earlier bytes in place"
        );
        assert!(
            staging_dirs(temp.path()).is_empty(),
            "a published item leaves no staging behind"
        );
        assert!(stages.iter().any(|(phase, _)| *phase == TransientItemPhase::Media));
    }

    #[test]
    fn a_resumed_attempt_reports_the_preserved_bytes_before_yt_dlp_speaks() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        *runner.fail_lines.lock().unwrap() = vec!["LVFORMATID 137".to_string()];
        *runner.fail_next.lock().unwrap() = Some((
            "media.f137.mp4.part".to_string(),
            7000,
            "ERROR: network is unreachable".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap_err();

        let control = TransientRunControl::default();
        let mut bytes = Vec::new();
        TransientExecutor::execute(&*executor, &item, &control, &mut |update| {
            bytes.push(update.bytes_completed);
        })
        .unwrap();
        assert!(bytes.contains(&Some(7000 + 3)), "{bytes:?}");
    }

    #[test]
    fn a_full_disk_is_classified_and_keeps_the_partial_download() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        *runner.fail_next.lock().unwrap() = Some((
            "media.f137.mp4.part".to_string(),
            100,
            "OSError: [Errno 28] No space left on device".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            runner,
        );
        let error = run_collecting(&executor, &item).0.unwrap_err();
        assert_eq!(error.code, "DISK_FULL");
        assert_eq!(staging_dirs(temp.path()).len(), 1);
    }

    #[test]
    fn a_removed_video_is_classified_as_unavailable() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        *runner.fail_next.lock().unwrap() = Some((
            "media.f137.mp4.part".to_string(),
            1,
            "ERROR: [youtube] abc: Video unavailable. This video has been removed".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            runner,
        );
        let error = run_collecting(&executor, &item).0.unwrap_err();
        assert_eq!(error.code, "VIDEO_UNAVAILABLE");
    }

    #[test]
    fn a_failure_after_the_download_finished_discards_the_staging() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(MALFORMED_VTT);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            false,
            runner,
        );
        let error = run_collecting(&executor, &item).0.unwrap_err();
        assert_eq!(error.code, "TRANSCRIPT_INVALID");
        assert!(
            staging_dirs(temp.path()).is_empty(),
            "resuming would only repeat this failure"
        );
    }

    #[test]
    fn a_changed_format_removes_the_old_formats_partial_files() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::with_script(
            VALID_VTT,
            vec![ScriptStep::Line("LVFORMATS 137, 140".to_string())],
        );
        *runner.fail_next.lock().unwrap() = Some((
            "media.f299.mp4.part".to_string(),
            4096,
            "ERROR: connection reset".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap_err();

        run_collecting(&executor, &item)
            .0
            .expect("stale parts of format 299 must not block the new format");
        let files = published_files(temp.path());
        assert!(
            !files.iter().any(|name| name.contains("f299")),
            "stale part published: {files:?}"
        );
    }

    fn format_of(args: &[String]) -> Option<String> {
        arg_after(args, "--format")
    }

    #[test]
    fn a_merged_download_resumes_pinned_to_the_exact_formats_it_started_with() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        *runner.fail_lines.lock().unwrap() = vec!["LVFORMATID 137+140".to_string()];
        *runner.fail_next.lock().unwrap() = Some((
            "media.f137.mp4.part".to_string(),
            2048,
            "ERROR: connection reset".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap_err();

        run_collecting(&executor, &item).0.expect("resumed");

        let seen = runner.seen_args.lock().unwrap().clone();
        assert_eq!(format_of(&seen[1]).as_deref(), Some("137+140"), "{seen:?}");
        assert!(
            !published_files(temp.path()).iter().any(|name| name == "media.lvformat"),
            "the format marker must not be published"
        );
    }

    #[test]
    fn a_single_file_download_resumes_pinned_to_its_format_too() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        // A single format prints no requested_formats; its partial has no id.
        *runner.fail_lines.lock().unwrap() = vec!["LVFORMATID 18".to_string()];
        *runner.fail_next.lock().unwrap() = Some((
            "media.mp4.part".to_string(),
            2048,
            "ERROR: connection reset".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap_err();

        run_collecting(&executor, &item).0.expect("resumed");

        let seen = runner.seen_args.lock().unwrap().clone();
        assert_eq!(format_of(&seen[1]).as_deref(), Some("18"), "{seen:?}");
        assert!(seen[1].iter().any(|argument| argument == FORMAT_ID_PRINT));
    }

    #[test]
    fn partial_media_with_no_recorded_format_is_discarded_not_continued() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        *runner.fail_next.lock().unwrap() = Some((
            "media.mp4.part".to_string(),
            4096,
            "ERROR: connection reset".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap_err();

        run_collecting(&executor, &item).0.expect("fresh start");

        assert!(
            runner.seen_at_start.lock().unwrap().is_empty(),
            "bytes of unknown provenance must not be handed to yt-dlp"
        );
        let seen = runner.seen_args.lock().unwrap().clone();
        assert!(format_of(&seen[1]).unwrap().starts_with("bestvideo"));
    }

    #[test]
    fn an_unavailable_pinned_format_wipes_the_partials_and_retries_with_normal_selection() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        *runner.fail_lines.lock().unwrap() = vec!["LVFORMATID 137+140".to_string()];
        *runner.fail_next.lock().unwrap() = Some((
            "media.f137.mp4.part".to_string(),
            2048,
            "ERROR: connection reset".to_string(),
        ));
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap_err();
        runner.reject_pinned.store(true, Ordering::SeqCst);

        run_collecting(&executor, &item).0.expect("falls back");

        let seen = runner.seen_args.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "failed attempt, refused pin, fresh retry");
        assert_eq!(format_of(&seen[1]).as_deref(), Some("137+140"));
        assert!(format_of(&seen[2]).unwrap().starts_with("bestvideo"));
        assert!(
            !runner
                .seen_at_start
                .lock()
                .unwrap()
                .iter()
                .any(|(name, _)| name.contains(".part")),
            "the retry starts with no unfinished download"
        );
    }

    #[test]
    fn a_verification_io_failure_keeps_the_downloaded_media_and_resume_does_not_wipe_it() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::with_script(
            VALID_VTT,
            vec![ScriptStep::Line("LVFORMATID 137+140".to_string())],
        );
        runner.probe_failure.store(1, Ordering::SeqCst);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );

        let error = run_collecting(&executor, &item).0.unwrap_err();
        assert_eq!(error.code, "MEDIA_VERIFY_FAILED");
        let kept = staging_dirs(temp.path());
        assert_eq!(kept.len(), 1, "the finished download must be kept");
        let files = fs::read_dir(temp.path().join(".lv").join(&kept[0]))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(files.contains(&"media.mp4".to_string()), "{files:?}");
        assert!(
            files.contains(&FORMAT_MARKER.to_string()),
            "the marker stays until publication: {files:?}"
        );

        // Continue with YouTube unreachable: the finished download goes
        // straight to verification and publishes without invoking yt-dlp.
        runner.download_unreachable.store(true, Ordering::SeqCst);
        run_collecting(&executor, &item).0.expect("re-verified");
        assert_eq!(
            runner.seen_args.lock().unwrap().len(),
            1,
            "yt-dlp must not be launched again for a finished download"
        );
        let published = published_files(temp.path());
        assert!(published.contains(&"media.mp4".to_string()), "{published:?}");
        assert!(!published.contains(&FORMAT_MARKER.to_string()), "{published:?}");
        assert!(staging_dirs(temp.path()).is_empty());
    }

    #[test]
    fn complete_media_without_a_format_marker_is_kept_but_partials_are_not() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        runner.probe_failure.store(1, Ordering::SeqCst);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap_err();

        run_collecting(&executor, &item).0.expect("re-verified");

        assert!(
            runner
                .seen_at_start
                .lock()
                .unwrap()
                .iter()
                .any(|(name, _)| name == "media.mp4"),
            "complete files cannot be mixed across formats, so they stay"
        );
    }

    #[test]
    fn media_that_ffmpeg_proves_corrupt_is_deleted_but_the_rest_of_staging_stays() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::with_script(
            VALID_VTT,
            vec![ScriptStep::Line("LVFORMATID 18".to_string())],
        );
        runner.probe_failure.store(2, Ordering::SeqCst);
        *runner.probe_stderr.lock().unwrap() =
            "x.mp4: Invalid data found when processing input".to_string();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );

        let error = run_collecting(&executor, &item).0.unwrap_err();

        assert_eq!(error.code, "MEDIA_CORRUPT");
        let kept = staging_dirs(temp.path());
        assert_eq!(kept.len(), 1);
        let files = fs::read_dir(temp.path().join(".lv").join(&kept[0]))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!files.contains(&"media.mp4".to_string()), "{files:?}");
        assert!(
            !files.contains(&FORMAT_MARKER.to_string()),
            "media and its pin go together: {files:?}"
        );
        assert!(files.contains(&"media.en-US.vtt".to_string()), "subtitles stay: {files:?}");
        run_collecting(&executor, &item).0.expect("downloads again");
    }

    #[test]
    fn cancelling_an_attempt_keeps_staging_that_a_paused_run_shares_and_removes_the_rest() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        for shared in [true, false] {
            let runner = FakeRunner::new(VALID_VTT);
            runner.cancel_download.store(true, Ordering::SeqCst);
            *runner.fail_lines.lock().unwrap() = vec!["LVFORMATID 137".to_string()];
            let (executor, item) = executor_with_fake(
                &root,
                YouTubeDownloadMode::VideoAndTranscript,
                true,
                Arc::clone(&runner),
            );
            let name = resumable_staging_name(&item.occurrence_id, &item.artifact_fingerprint);
            let executor = executor.with_shared_staging(SharedStaging(Arc::new(move |decide| {
                if shared {
                    decide(&std::collections::HashSet::from([name.clone()]))
                } else {
                    decide(&std::collections::HashSet::new())
                }
            })));
            // Partials left by a paused run that uses the same directory.
            let held = root
                .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
                .unwrap();
            fs::write(held.lease.path().join("media.f137.mp4.part"), b"paused run partial")
                .unwrap();
            fs::write(held.lease.path().join(FORMAT_MARKER), b"137").unwrap();
            drop(held);

            let error = run_collecting(&executor, &item).0.unwrap_err();

            assert_eq!(error.code, "CANCELLED");
            assert_eq!(
                staging_dirs(temp.path()).len(),
                usize::from(shared),
                "shared={shared}: the directory survives only when another run needs it"
            );
            if !shared {
                continue;
            }
            // Leave a clean slate for the next iteration.
            root.discard_resumable_staging(&item.occurrence_id, &item.artifact_fingerprint)
                .unwrap();
        }
    }

    #[test]
    fn an_unexplained_ffmpeg_failure_keeps_the_media() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::with_script(
            VALID_VTT,
            vec![ScriptStep::Line("LVFORMATID 18".to_string())],
        );
        runner.probe_failure.store(2, Ordering::SeqCst);
        *runner.probe_stderr.lock().unwrap() =
            "x.mp4: Permission denied".to_string();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );

        let error = run_collecting(&executor, &item).0.unwrap_err();

        assert_eq!(error.code, "MEDIA_VERIFY_FAILED", "access problems prove nothing");
        let kept = staging_dirs(temp.path());
        let files = fs::read_dir(temp.path().join(".lv").join(&kept[0]))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(files.contains(&"media.mp4".to_string()), "{files:?}");
    }

    #[test]
    fn an_unavailable_pin_removes_only_unfinished_downloads_never_a_completed_file() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        runner.reject_pinned.store(true, Ordering::SeqCst);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        let held = root
            .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        fs::write(held.lease.path().join(FORMAT_MARKER), b"137+140").unwrap();
        fs::write(held.lease.path().join("media.f137.mp4.part"), b"half").unwrap();
        fs::write(held.lease.path().join("media.en-US.vtt"), VALID_VTT).unwrap();
        fs::write(held.lease.path().join("media.mp4"), b"finished earlier").unwrap();
        drop(held);

        run_collecting(&executor, &item).0.expect("falls back");

        let at_retry = runner.seen_at_start.lock().unwrap().clone();
        assert!(
            at_retry.iter().any(|(name, _)| name == "media.mp4"),
            "the completed file survives the failed pin: {at_retry:?}"
        );
        assert!(!at_retry.iter().any(|(name, _)| name.ends_with(".part")));
    }

    #[test]
    fn a_fresh_attempt_waits_for_a_staging_directory_that_is_briefly_held() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(VALID_VTT);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        let held = root
            .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            drop(held);
        });

        let result = run_collecting(&executor, &item).0;
        releaser.join().unwrap();

        result.expect("the brief hold is waited out, not failed");
    }

    #[test]
    fn a_staging_directory_that_stays_held_ends_as_staging_busy() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            FakeRunner::new(VALID_VTT),
        );
        let _held = root
            .resumable_staging_lease(&item.occurrence_id, &item.artifact_fingerprint)
            .unwrap();
        assert_eq!(run_collecting(&executor, &item).0.unwrap_err().code, "STAGING_BUSY");
    }

    #[test]
    fn an_optional_transcript_discarded_earlier_does_not_force_yt_dlp_again() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::with_script(
            MALFORMED_VTT,
            vec![ScriptStep::Line("LVFORMATID 18".to_string())],
        );
        runner.probe_failure.store(1, Ordering::SeqCst);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        let error = run_collecting(&executor, &item).0.unwrap_err();
        assert_eq!(error.code, "MEDIA_VERIFY_FAILED");

        // Offline Continue: the malformed VTT was discarded, yet nothing is
        // fetched and the warning survives.
        runner.download_unreachable.store(true, Ordering::SeqCst);
        let outcome = run_collecting(&executor, &item).0.expect("finalized offline");

        assert_eq!(runner.seen_args.lock().unwrap().len(), 1);
        assert_eq!(outcome.warnings, vec!["TRANSCRIPT_INVALID".to_string()]);
        let published = published_files(temp.path());
        assert!(published.contains(&"media.mp4".to_string()), "{published:?}");
        assert!(
            !published.iter().any(|name| is_bookkeeping_leaf(name)),
            "no marker may be published: {published:?}"
        );
    }

    #[test]
    fn a_missing_required_stream_is_structural_so_the_media_and_its_pin_are_dropped() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::with_script(
            VALID_VTT,
            vec![ScriptStep::Line("LVFORMATID 137".to_string())],
        );
        runner.probe_failure.store(2, Ordering::SeqCst);
        *runner.probe_stderr.lock().unwrap() =
            "Stream map '0:a:0' matches no streams.".to_string();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );

        let error = run_collecting(&executor, &item).0.unwrap_err();

        assert_eq!(error.code, "MEDIA_CORRUPT");
        let kept = staging_dirs(temp.path());
        let files = fs::read_dir(temp.path().join(".lv").join(&kept[0]))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!files.contains(&"media.mp4".to_string()), "{files:?}");
        assert!(
            !files.contains(&FORMAT_MARKER.to_string()),
            "the format without the stream must not be pinned again: {files:?}"
        );
        // The next Continue downloads again with the normal selection.
        run_collecting(&executor, &item).0.expect("re-downloaded");
        let seen = runner.seen_args.lock().unwrap().clone();
        assert!(format_of(&seen[1]).unwrap().starts_with("bestvideo"));
    }

    #[test]
    fn the_same_verification_failure_twice_in_a_row_discards_the_download_instead_of_looping() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::with_script(
            VALID_VTT,
            vec![ScriptStep::Line("LVFORMATID 18".to_string())],
        );
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        runner.probe_failure.store(1, Ordering::SeqCst);
        assert_eq!(
            run_collecting(&executor, &item).0.unwrap_err().code,
            "MEDIA_VERIFY_FAILED"
        );
        // Continue: finished download, no yt-dlp, the same check fails again.
        runner.probe_failure.store(1, Ordering::SeqCst);
        let second = run_collecting(&executor, &item).0.unwrap_err();
        assert_eq!(second.code, "MEDIA_CORRUPT", "{second:?}");
        assert!(second.message.contains("twice"));
        assert_eq!(runner.seen_args.lock().unwrap().len(), 1);

        // The next Continue fetches it again and publishes.
        run_collecting(&executor, &item).0.expect("fetched again");
        assert_eq!(runner.seen_args.lock().unwrap().len(), 2);
        assert!(staging_dirs(temp.path()).is_empty());
    }

    #[test]
    fn video_and_transcript_keeps_the_video_when_the_transcript_is_malformed() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(MALFORMED_VTT);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );

        let (result, _) = run_collecting(&executor, &item);

        let outcome = result.expect("the video must survive a bad transcript");
        assert_eq!(
            outcome.state,
            crate::app::managed_process::TransientItemState::CompletedWithWarnings
        );
        assert_eq!(outcome.warnings, vec!["TRANSCRIPT_INVALID".to_string()]);
        let message = &outcome
            .warning_messages
            .iter()
            .find(|(code, _)| code == "TRANSCRIPT_INVALID")
            .expect("the warning carries a human message")
            .1;
        assert!(
            message.starts_with("Video saved; the transcript couldn't be processed: "),
            "unexpected message: {message}"
        );
        assert!(message.contains("WEBVTT"), "reason missing: {message}");
        assert_eq!(outcome.published_artifact_kinds, vec!["media".to_string()]);
        let files = published_files(temp.path());
        assert!(files.contains(&"media.mp4".to_string()), "{files:?}");
        assert!(files.contains(&"manifest.json".to_string()), "{files:?}");
        assert!(
            !files
                .iter()
                .any(|name| name.ends_with(".vtt") || name.ends_with(".transcript.json")),
            "transcript artifacts must not be published: {files:?}"
        );
    }

    #[test]
    fn rerun_reuses_a_video_that_was_kept_without_its_transcript() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(MALFORMED_VTT);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&runner),
        );
        run_collecting(&executor, &item).0.unwrap();
        assert_eq!(runner.downloads.load(Ordering::SeqCst), 1);

        let (second, _) = run_collecting(&executor, &item);

        let outcome = second.expect("the kept video is reused, not failed");
        // Not "Already saved": the transcript the user asked for is missing.
        assert_eq!(
            outcome.state,
            crate::app::managed_process::TransientItemState::CompletedWithWarnings
        );
        assert_eq!(outcome.warnings, vec!["TRANSCRIPT_MISSING".to_string()]);
        let message = &outcome.warning_messages[0].1;
        assert!(
            message.contains("delete or move the folder"),
            "the warning must say how to retry: {message}"
        );
        assert_eq!(outcome.published_artifact_kinds, vec!["media".to_string()]);
        assert_eq!(
            runner.downloads.load(Ordering::SeqCst),
            1,
            "the video must not be downloaded again"
        );
    }

    #[test]
    fn rerun_with_a_now_working_transcript_still_does_not_redownload_or_rewrite_the_folder() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            FakeRunner::new(MALFORMED_VTT),
        );
        run_collecting(&executor, &item).0.unwrap();
        let before = published_files(temp.path());
        // The same item again, with a helper that would now return a good VTT.
        let fixed_runner = FakeRunner::new(VALID_VTT);
        let (retry, retry_item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            Arc::clone(&fixed_runner),
        );

        let outcome = run_collecting(&retry, &retry_item).0.unwrap();

        assert_eq!(fixed_runner.downloads.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.warnings, vec!["TRANSCRIPT_MISSING".to_string()]);
        assert_eq!(published_files(temp.path()), before);
    }

    #[test]
    fn required_transcript_rerun_of_a_video_only_folder_explains_how_to_continue() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (kept, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            FakeRunner::new(MALFORMED_VTT),
        );
        run_collecting(&kept, &item).0.unwrap();
        let strict_runner = FakeRunner::new(VALID_VTT);
        let (strict, strict_item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            false,
            Arc::clone(&strict_runner),
        );

        let error = run_collecting(&strict, &strict_item)
            .0
            .expect_err("a required transcript cannot be satisfied by a video-only folder");

        assert_eq!(error.code, "EXISTING_OUTPUT_MISSING_TRANSCRIPT");
        assert!(
            error.message.contains("Delete or move that folder"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("without a transcript"),
            "{}",
            error.message
        );
        assert_eq!(strict_runner.downloads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn empty_transcript_keeps_the_video_with_the_transcript_error_code() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            FakeRunner::new(""),
        );

        let outcome = run_collecting(&executor, &item).0.unwrap();

        assert_eq!(outcome.warnings, vec!["TRANSCRIPT_INVALID".to_string()]);
        assert!(outcome.warning_messages[0].1.contains("empty"));
        assert_eq!(outcome.published_artifact_kinds, vec!["media".to_string()]);
    }

    #[test]
    fn multiple_transcript_tracks_fail_with_their_own_code_instead_of_keeping_the_video() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            FakeRunner::with_second_vtt(VALID_VTT),
        );

        let error = run_collecting(&executor, &item)
            .0
            .expect_err("an ambiguous transcript is not a content failure");

        assert_eq!(error.code, "TRANSCRIPT_AMBIGUOUS");
        assert!(published_files(temp.path()).is_empty());
    }

    #[test]
    fn only_transcript_content_failures_keep_the_video() {
        assert!(TranscriptProcessError::Content("bad".to_string()).keeps_video());
        assert!(!TranscriptProcessError::Ambiguous("two".to_string()).keeps_video());
        assert!(!TranscriptProcessError::Staging("io".to_string()).keeps_video());
        assert_eq!(
            TranscriptProcessError::Staging("io".to_string()).code(),
            "OUTPUT_VERIFY_FAILED"
        );
    }

    #[test]
    fn truncate_reason_leaves_short_reasons_and_collapses_whitespace() {
        assert_eq!(
            truncate_reason("bad   header\n at line 3"),
            "bad header at line 3"
        );
        let exactly = "a".repeat(MAX_WARNING_REASON_CHARS);
        assert_eq!(truncate_reason(&exactly), exactly);
    }

    #[test]
    fn truncate_reason_cuts_long_multibyte_text_on_character_boundaries() {
        // 3-byte, 4-byte and combining characters: a byte-based cut would panic
        // or split a character.
        for unit in ["字幕", "🎬", "e\u{301}"] {
            let reason = unit.repeat(MAX_WARNING_REASON_CHARS + 5);
            let shortened = truncate_reason(&reason);
            assert!(shortened.ends_with('…'));
            assert_eq!(shortened.chars().count(), MAX_WARNING_REASON_CHARS + 1);
            assert!(reason.starts_with(shortened.trim_end_matches('…')));
        }
    }

    #[test]
    fn truncate_reason_cuts_one_character_over_the_limit() {
        let reason = "字".repeat(MAX_WARNING_REASON_CHARS + 1);
        let shortened = truncate_reason(&reason);
        assert_eq!(shortened.chars().count(), MAX_WARNING_REASON_CHARS + 1);
        assert_eq!(
            shortened.trim_end_matches('…').chars().count(),
            MAX_WARNING_REASON_CHARS
        );
    }

    #[test]
    fn transcript_only_malformed_transcript_still_fails() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(MALFORMED_VTT);
        let (executor, item) =
            executor_with_fake(&root, YouTubeDownloadMode::TranscriptOnly, true, runner);

        let (result, _) = run_collecting(&executor, &item);

        let error = result.expect_err("transcript-only has nothing to keep");
        assert_eq!(error.code, "TRANSCRIPT_INVALID");
        assert!(published_files(temp.path()).is_empty());
    }

    #[test]
    fn required_transcript_still_fails_a_video_and_transcript_run() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let runner = FakeRunner::new(MALFORMED_VTT);
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            false,
            runner,
        );

        let (result, _) = run_collecting(&executor, &item);

        assert_eq!(
            result.expect_err("transcript is required").code,
            "TRANSCRIPT_INVALID"
        );
        assert!(published_files(temp.path()).is_empty());
    }

    #[test]
    fn successful_video_and_transcript_run_reports_each_stage_in_order() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            true,
            FakeRunner::new(VALID_VTT),
        );

        let (result, stages) = run_collecting(&executor, &item);

        let outcome = result.unwrap();
        assert_eq!(
            outcome.state,
            crate::app::managed_process::TransientItemState::Completed
        );
        let phases = stages
            .iter()
            .map(|(phase, _)| phase.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            phases,
            vec![
                TransientItemPhase::Waiting,
                TransientItemPhase::Media,
                TransientItemPhase::NormalizingTranscript,
                TransientItemPhase::Verifying,
                TransientItemPhase::Finalizing,
                TransientItemPhase::Completed,
            ]
        );
        assert!(
            stages.windows(2).all(|pair| pair[0].1 <= pair[1].1),
            "fractions must never go backwards: {stages:?}"
        );
        assert_eq!(stages.last().map(|stage| stage.1), Some(1.0));
    }

    #[test]
    fn successful_transcript_only_run_reports_transcript_stages() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_fake(
            &root,
            YouTubeDownloadMode::TranscriptOnly,
            false,
            FakeRunner::new(VALID_VTT),
        );

        let (result, stages) = run_collecting(&executor, &item);

        result.unwrap();
        let phases = stages
            .into_iter()
            .map(|(phase, _)| phase)
            .collect::<Vec<_>>();
        assert_eq!(
            phases,
            vec![
                TransientItemPhase::Waiting,
                TransientItemPhase::Transcript,
                TransientItemPhase::NormalizingTranscript,
                TransientItemPhase::Verifying,
                TransientItemPhase::Finalizing,
                TransientItemPhase::Completed,
            ]
        );
    }

    fn stage_reviewed_helpers_beside_current_exe() -> bool {
        let binaries = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries");
        let names = ["yt-dlp", "deno", "ffmpeg"];
        const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
        if !names.iter().all(|name| {
            binaries
                .join(format!("{name}-{TARGET_TRIPLE}.exe"))
                .is_file()
        }) {
            return false;
        }
        let Some(install) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        else {
            return false;
        };
        for name in names {
            let source = binaries.join(format!("{name}-{TARGET_TRIPLE}.exe"));
            let destination = install.join(format!("{name}.exe"));
            if destination.is_file() {
                continue;
            }
            if std::fs::hard_link(&source, &destination).is_err()
                && std::fs::copy(&source, &destination).is_err()
            {
                return false;
            }
        }
        true
    }

    #[test]
    #[ignore = "requires reviewed helpers in src-tauri/binaries and network access"]
    fn live_transcript_only_download_writes_raw_vtt_and_normalized_json() {
        use crate::app::managed_process::{helper_identity, DiscoveryOperation, HelperKind};
        use crate::providers::youtube::manifest_contract::select_transcript;
        use crate::providers::youtube::models::{ScanYouTubeSourceRequest, YouTubePlaylistMode};
        use crate::providers::youtube::scan::scan_source;
        use crate::providers::youtube::transcript_inspection::inspect_transcripts;
        use crate::providers::youtube::transcript_normalizer::NormalizedTranscript;

        assert!(
            stage_reviewed_helpers_beside_current_exe(),
            "run npm --prefix apps/desktop run fetch:youtube-helpers first"
        );
        let helper = helper_identity(HelperKind::YouTubeYtDlp)
            .expect("reviewed helpers must pass identity verification");
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let operation = DiscoveryOperation::new();
        let plan = scan_source(
            &ScanYouTubeSourceRequest {
                client_operation_id: "live-transcript-scan-01".to_string(),
                url: "https://www.youtube.com/watch?v=jNQXAC9IVRw".to_string(),
                playlist_mode: Some(YouTubePlaylistMode::Video),
            },
            &operation,
        )
        .expect("yt-dlp scan of the reviewed public fixture video must succeed");
        let planned = plan
            .items
            .first()
            .cloned()
            .expect("the fixture video must produce one occurrence");
        let inspected = inspect_transcripts(&planned, &operation)
            .expect("transcript inspection must succeed for the fixture video");
        let selected = select_transcript(
            &inspected.tracks,
            Some("en"),
            &[],
            true,
            &["live_chat".to_string()],
        )
        .expect("the fixture video must expose an English caption track");
        let artifact_fingerprint = artifact_fingerprint(&ArtifactFingerprintInput {
            occurrence_id: planned.public.occurrence_id.clone(),
            video_id: planned.public.video_id.clone(),
            mode: YouTubeDownloadMode::TranscriptOnly,
            format_policy_version: FORMAT_POLICY_VERSION,
            max_height: None,
            selected_transcript: Some(selected.clone()),
            helper_lock_digest: helper.digest.clone(),
        })
        .expect("live fingerprint must be constructible");
        let item = TransientWorkItem {
            occurrence_id: planned.public.occurrence_id.clone(),
            artifact_fingerprint,
            video_id: planned.public.video_id.clone(),
            ordinal: planned.public.ordinal,
            title: planned.public.title.clone(),
            source_url: planned.source_url.clone(),
        };
        let context = YouTubeExecutorContext {
            source_snapshot_digest: plan.source_snapshot_digest.clone(),
            playlist_id: plan.response.playlist_id.clone(),
            helper_lock_digest: helper.digest,
            items: vec![YouTubeExecutorItemContext {
                occurrence_id: planned.public.occurrence_id.clone(),
                playlist_index: None,
                source_duration_seconds: planned.public.duration_seconds,
                selected_transcript: Some(selected.clone()),
            }],
        };
        let mut request = transcript_request();
        request.preferred_language = Some("en".to_string());
        request.allow_automatic_captions = true;
        request.selected_occurrence_ids = vec![planned.public.occurrence_id.clone()];
        let executor = YouTubeExecutor::new_with_context(root.clone(), &request, context).unwrap();
        let control = TransientRunControl::default();
        let outcome = TransientExecutor::execute(executor.as_ref(), &item, &control, &mut |_| {})
            .expect("transcript-only download must complete");
        assert!(
            matches!(
                outcome.state,
                crate::app::managed_process::TransientItemState::Completed
                    | crate::app::managed_process::TransientItemState::CompletedWithWarnings
            ),
            "unexpected live outcome: {:?}",
            outcome
        );
        assert!(
            outcome
                .published_artifact_kinds
                .iter()
                .any(|kind| kind == "vtt"),
            "published kinds missing raw VTT: {:?}",
            outcome.published_artifact_kinds
        );
        assert!(
            outcome
                .published_artifact_kinds
                .iter()
                .any(|kind| kind == "transcript_json"),
            "published kinds missing normalized transcript JSON: {:?}",
            outcome.published_artifact_kinds
        );

        let mut vtt_path = None;
        let mut json_path = None;
        for entry in fs::read_dir(temp.path()).unwrap() {
            let entry = entry.unwrap();
            if !entry.file_type().unwrap().is_dir() {
                continue;
            }
            for child in fs::read_dir(entry.path()).unwrap() {
                let child = child.unwrap();
                let name = child.file_name();
                let name = name.to_string_lossy();
                if name
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("vtt"))
                {
                    vtt_path = Some(child.path());
                }
                if name.ends_with(".transcript.json") {
                    json_path = Some(child.path());
                }
            }
        }
        let vtt_path = vtt_path.expect("published directory must contain the raw VTT");
        let json_path =
            json_path.expect("published directory must contain normalized transcript JSON");
        assert!(
            transcript_filename_matches_language(
                vtt_path.file_name().unwrap().to_str().unwrap(),
                &selected
            ),
            "raw VTT name {} does not match selected language {}",
            vtt_path.display(),
            selected.language_tag
        );
        let raw_vtt = fs::read(&vtt_path).unwrap();
        assert!(raw_vtt.starts_with(b"WEBVTT") || raw_vtt.starts_with(b"\xEF\xBB\xBFWEBVTT"));
        let normalized: NormalizedTranscript =
            serde_json::from_slice(&fs::read(&json_path).unwrap()).unwrap();
        assert_eq!(normalized.schema_version, 1);
        assert_eq!(normalized.provider, "youtube");
        assert_eq!(normalized.video_id, item.video_id);
        assert_eq!(normalized.language_tag, selected.language_tag);
        assert!(
            !normalized.cues.is_empty(),
            "normalized transcript has no cues"
        );
        assert!(normalized.cues.iter().all(|cue| cue.end_ms >= cue.start_ms));
    }
    // ---- Live download progress ------------------------------------------

    fn line(text: impl Into<String>) -> ScriptStep {
        ScriptStep::Line(text.into())
    }

    struct Recorded {
        at: Instant,
        update: TransientProgressUpdate,
    }

    fn run_recording(
        executor: &YouTubeExecutor,
        item: &TransientWorkItem,
    ) -> (
        Result<TransientExecutionOutcome, TransientError>,
        Vec<Recorded>,
    ) {
        let mut recorded = Vec::new();
        let control = TransientRunControl::default();
        let result = TransientExecutor::execute(executor, item, &control, &mut |update| {
            recorded.push(Recorded {
                at: test_clock(),
                update,
            });
        });
        (result, recorded)
    }

    fn executor_with_script(
        root: &ValidatedOutputRoot,
        mode: YouTubeDownloadMode,
        script: Vec<ScriptStep>,
    ) -> (Arc<YouTubeExecutor>, TransientWorkItem) {
        let (mut executor, item) =
            executor_with_fake(root, mode, true, FakeRunner::with_script(VALID_VTT, script));
        Arc::get_mut(&mut executor)
            .expect("the test owns the only executor handle")
            .clock = test_clock;
        (executor, item)
    }

    fn byte_updates(recorded: &[Recorded]) -> Vec<&Recorded> {
        recorded
            .iter()
            .filter(|entry| entry.update.bytes_completed.is_some())
            .collect()
    }

    /// Video stream (1000 bytes over 400 ms) followed by an audio stream
    /// (200 bytes over 1 s), with lines the parser must ignore mixed in.
    fn two_stream_script() -> Vec<ScriptStep> {
        let mut script = vec![
            line("[download] Destination: media.f160.mp4"),
            line("LVFORMATS 160, 139"),
            line("noise LVPROGRESS 1 2 3 4"),
        ];
        for step in 1..=40u64 {
            script.push(line(format!("LVPROGRESS {} 1000 NA 160", step * 25)));
            script.push(ScriptStep::AdvanceClockMs(10));
        }
        script.push(line("LVPROGRESS 0 NA NA 139"));
        for step in 1..=20u64 {
            script.push(line(format!("LVPROGRESS {} 200 NA 139", step * 10)));
            script.push(ScriptStep::AdvanceClockMs(50));
        }
        script.push(line("[Merger] Merging formats into media.mp4"));
        script
    }

    #[test]
    fn download_progress_is_monotonic_throttled_and_carries_bytes() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_script(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            two_stream_script(),
        );

        let (result, recorded) = run_recording(&executor, &item);

        result.unwrap();
        let fractions = recorded
            .iter()
            .map(|entry| entry.update.fraction.unwrap())
            .collect::<Vec<_>>();
        assert!(
            fractions.windows(2).all(|pair| pair[0] <= pair[1]),
            "fractions must never go backwards: {fractions:?}"
        );
        let bytes = byte_updates(&recorded);
        assert!(
            bytes.len() > 3,
            "expected live updates, got {}",
            bytes.len()
        );
        assert!(
            bytes.len() < 12,
            "61 progress lines over 1.4 s must be throttled, got {}",
            bytes.len()
        );
        assert!(bytes
            .iter()
            .all(|entry| entry.update.phase == TransientItemPhase::Media));
        assert!(bytes
            .iter()
            .all(|entry| entry.update.fraction.unwrap() <= STAGE_NORMALIZING));
        // At most four per second: consecutive live updates are 250 ms apart,
        // except the final flush that publishes whatever the throttle held.
        let spaced = &bytes[..bytes.len() - 1];
        assert!(
            spaced
                .windows(2)
                .all(|pair| pair[1].at.duration_since(pair[0].at) >= Duration::from_millis(250)),
            "live updates were published faster than four per second"
        );
        let last = bytes.last().unwrap();
        assert_eq!(last.update.bytes_completed, Some(1200));
        assert_eq!(last.update.bytes_total, Some(1200));
        assert_eq!(last.update.fraction, Some(STAGE_NORMALIZING));
        // The transcript stage follows without the fraction dropping.
        let next = recorded
            .iter()
            .position(|entry| entry.update.phase == TransientItemPhase::NormalizingTranscript)
            .unwrap();
        assert!(recorded[next].update.fraction.unwrap() >= STAGE_NORMALIZING);
    }

    #[test]
    fn download_progress_lands_in_the_run_snapshot() {
        use crate::providers::youtube::live::{apply_item_progress, initial_snapshot};

        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let (executor, item) = executor_with_script(
            &root,
            YouTubeDownloadMode::VideoAndTranscript,
            two_stream_script(),
        );
        let (result, recorded) = run_recording(&executor, &item);
        result.unwrap();

        let mut snapshot = initial_snapshot(
            "run".to_string(),
            "submission".to_string(),
            "plan".to_string(),
            std::slice::from_ref(&item),
        );
        let mut applied = 0;
        for entry in recorded
            .iter()
            .take_while(|entry| entry.update.phase != TransientItemPhase::NormalizingTranscript)
        {
            apply_item_progress(
                &mut snapshot,
                &item,
                entry.update.phase.clone(),
                entry.update.bytes_completed,
                entry.update.bytes_total,
                entry.update.fraction,
            );
            applied += 1;
        }

        assert_eq!(snapshot.revision, 1 + applied);
        assert_eq!(snapshot.progress.bytes_completed, Some(1200));
        assert_eq!(snapshot.progress.bytes_total, Some(1200));
        assert_eq!(snapshot.progress.fraction, Some(STAGE_NORMALIZING));
    }

    #[test]
    fn unknown_totals_publish_bytes_without_a_total_or_fraction_jump() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let mut script = Vec::new();
        for step in 1..=3u64 {
            script.push(line(format!("LVPROGRESS {} NA NA 18", step * 5000)));
            script.push(ScriptStep::AdvanceClockMs(300));
        }
        let (executor, item) =
            executor_with_script(&root, YouTubeDownloadMode::VideoAndTranscript, script);

        let (result, recorded) = run_recording(&executor, &item);

        result.unwrap();
        let bytes = byte_updates(&recorded);
        assert_eq!(bytes.len(), 3);
        assert!(bytes.iter().all(|entry| entry.update.bytes_total.is_none()));
        assert!(bytes
            .iter()
            .all(|entry| entry.update.fraction == Some(STAGE_START)));
        assert_eq!(bytes[2].update.bytes_completed, Some(15000));
    }

    #[test]
    fn transcript_only_subtitle_records_and_garbage_publish_no_bytes() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let script = vec![
            line("LVPROGRESS 440 NA 440 NA"),
            line("LVPROGRESS 440 440 NA NA"),
            line("LVPROGRESS"),
            line("not progress at all"),
        ];
        let (executor, item) =
            executor_with_script(&root, YouTubeDownloadMode::TranscriptOnly, script);

        let (result, recorded) = run_recording(&executor, &item);

        result.unwrap();
        assert!(byte_updates(&recorded).is_empty());
    }

    #[test]
    fn helper_arguments_request_prefixed_progress_records() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        for (mode, expects_print) in [
            (YouTubeDownloadMode::VideoAndTranscript, true),
            (YouTubeDownloadMode::TranscriptOnly, false),
        ] {
            let (executor, item) =
                executor_with_fake(&root, mode, true, FakeRunner::new(VALID_VTT));
            let args = executor
                .args_for(&item, Path::new("C:/staging/media.%(ext)s"))
                .unwrap();
            assert_eq!(
                arg_after(&args, "--progress-template").as_deref(),
                Some(PROGRESS_TEMPLATE)
            );
            assert!(args.iter().any(|argument| argument == "--newline"));
            assert_eq!(
                arg_after(&args, "--print").as_deref(),
                expects_print.then_some(FORMATS_PRINT)
            );
            assert_eq!(
                args.iter().any(|argument| argument == "--progress"),
                expects_print
            );
            assert_eq!(
                args.iter().any(|argument| argument == "--no-simulate"),
                expects_print
            );
        }
        assert!(PROGRESS_TEMPLATE.starts_with("download:LVPROGRESS "));
    }
}
