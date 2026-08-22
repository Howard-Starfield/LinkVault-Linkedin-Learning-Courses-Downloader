//! Workflow-owned managed process port for transient external helpers.
//!
//! Provider code can select only a typed helper kind and typed arguments. It
//! cannot submit an executable path, shell string, or generic command. Helper
//! paths are resolved from packaged resources and verified against the embedded
//! lock. Windows launches are suspended, assigned to a kill-on-close Job Object,
//! connected to bounded readers, and only then resumed.

use super::TransientRunControl;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use thiserror::Error;

const EMBEDDED_HELPER_LOCK: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/third-party/youtube-helpers-lock.json"
));

// Defense in depth: a populated supply-chain lock must not silently enable
// execution before identity-held delegated-helper verification and the complete
// hostile-process/native shutdown suite pass review.
const EXECUTION_HARDENING_COMPLETE: bool = false;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperKind {
    YouTubeYtDlp,
}

#[derive(Clone, Debug)]
pub struct ManagedProcessSpec {
    helper: HelperKind,
    pub args: Vec<OsString>,
    pub stdout_limit: usize,
    pub stderr_limit: usize,
    pub timeout: Duration,
}

impl ManagedProcessSpec {
    pub fn youtube_ytdlp(
        args: Vec<OsString>,
        stdout_limit: usize,
        stderr_limit: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            helper: HelperKind::YouTubeYtDlp,
            args,
            stdout_limit,
            stderr_limit,
            timeout,
        }
    }

    #[cfg(feature = "youtube-process-test")]
    pub fn for_test(
        executable: PathBuf,
        args: Vec<OsString>,
        stdout_limit: usize,
        stderr_limit: usize,
        timeout: Duration,
    ) -> TestManagedProcessSpec {
        TestManagedProcessSpec {
            executable,
            spec: Self::youtube_ytdlp(args, stdout_limit, stderr_limit, timeout),
            fault: TestManagedProcessFault::None,
        }
    }
}

#[cfg(feature = "youtube-process-test")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestManagedProcessFault {
    None,
    BeforeJobAssignment,
    ReaderStartup,
    Resume,
}

#[cfg(feature = "youtube-process-test")]
#[derive(Clone, Debug)]
pub struct TestManagedProcessSpec {
    pub executable: PathBuf,
    pub spec: ManagedProcessSpec,
    pub fault: TestManagedProcessFault,
}

#[cfg(feature = "youtube-process-test")]
impl TestManagedProcessSpec {
    pub fn with_fault(mut self, fault: TestManagedProcessFault) -> Self {
        self.fault = fault;
        self
    }
}

#[derive(Debug)]
pub struct ManagedProcessOutput {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ManagedProcessError {
    #[error("helper integrity validation failed: {0}")]
    Integrity(String),
    #[error("helper process containment failed: {0}")]
    ProcessContainment(String),
    #[error("failed to start trusted helper: {0}")]
    Start(String),
    #[error("helper output was not valid UTF-8")]
    InvalidUtf8,
    #[error("helper output reader failed: {0}")]
    Reader(String),
    #[error("helper process wait failed: {0}")]
    Wait(String),
    #[error("managed helper execution is unsupported on this platform")]
    UnsupportedPlatform,
}

#[derive(Clone, Debug)]
pub struct HelperIdentity {
    pub digest: String,
}

pub fn helper_identity(kind: HelperKind) -> Result<HelperIdentity, ManagedProcessError> {
    ensure_execution_hardened()?;
    let (path, digest) = resolve_and_verify(kind)?;
    let _ = path;
    Ok(HelperIdentity { digest })
}

pub fn run(
    spec: ManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    ensure_execution_hardened()?;
    let (executable, _) = resolve_and_verify(spec.helper)?;
    run_resolved(
        executable,
        spec,
        control,
        discovery_cancel,
        TestFaultSelection::None,
    )
}

fn ensure_execution_hardened() -> Result<(), ManagedProcessError> {
    if EXECUTION_HARDENING_COMPLETE {
        return Ok(());
    }
    Err(ManagedProcessError::Integrity(
        "managed helper execution is disabled until identity-held verification, delegated-helper pinning, and native shutdown tests pass review"
            .to_string(),
    ))
}

#[cfg(feature = "youtube-process-test")]
pub fn run_test(
    test_spec: TestManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    let fault = match test_spec.fault {
        TestManagedProcessFault::None => TestFaultSelection::None,
        TestManagedProcessFault::BeforeJobAssignment => TestFaultSelection::BeforeJobAssignment,
        TestManagedProcessFault::ReaderStartup => TestFaultSelection::ReaderStartup,
        TestManagedProcessFault::Resume => TestFaultSelection::Resume,
    };
    run_resolved(
        test_spec.executable,
        test_spec.spec,
        control,
        discovery_cancel,
        fault,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestFaultSelection {
    None,
    BeforeJobAssignment,
    ReaderStartup,
    Resume,
}

fn run_resolved(
    executable: PathBuf,
    spec: ManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
    fault: TestFaultSelection,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    #[cfg(windows)]
    {
        windows_supervisor::run(executable, spec, control, discovery_cancel, fault)
    }
    #[cfg(not(windows))]
    {
        let _ = (executable, spec, control, discovery_cancel, fault);
        Err(ManagedProcessError::UnsupportedPlatform)
    }
}

fn resolve_and_verify(kind: HelperKind) -> Result<(PathBuf, String), ManagedProcessError> {
    let candidate = match kind {
        HelperKind::YouTubeYtDlp => packaged_candidate().ok_or_else(|| {
            ManagedProcessError::Integrity("the packaged yt-dlp helper is not present".to_string())
        })?,
    };
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ManagedProcessError::Integrity(
            "helper is not a trusted regular file".to_string(),
        ));
    }
    let expected = lock_digest_for(&candidate).ok_or_else(|| {
        ManagedProcessError::Integrity(
            "the helper lock is absent; Y0 helper validation is required before execution"
                .to_string(),
        )
    })?;
    let actual = digest_file(&candidate)
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(ManagedProcessError::Integrity(
            "helper digest does not match the approved helper lock".to_string(),
        ));
    }
    Ok((candidate, actual))
}

fn packaged_candidate() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("binaries/yt-dlp.exe"));
            candidates.push(parent.join("resources/binaries/yt-dlp.exe"));
            candidates.push(parent.join("yt-dlp.exe"));
            candidates.push(parent.join("youtube/yt-dlp.exe"));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn lock_digest_for(path: &Path) -> Option<String> {
    let value = serde_json::from_slice::<Value>(EMBEDDED_HELPER_LOCK).ok()?;
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(1)
        || value.get("targetTriple").and_then(Value::as_str) != Some("x86_64-pc-windows-msvc")
        || value.get("status").and_then(Value::as_str) != Some("ready")
    {
        return None;
    }
    let lock_digest = value.get("lockDigest").and_then(Value::as_str)?;
    if lock_digest.len() != 64
        || !lock_digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    let mut without_digest = value.clone();
    without_digest.as_object_mut()?.remove("lockDigest");
    let canonical = serde_json::to_vec(&without_digest).ok()?;
    let mut lock_hasher = Sha256::new();
    lock_hasher.update(canonical);
    if !format!("{:x}", lock_hasher.finalize()).eq_ignore_ascii_case(lock_digest) {
        return None;
    }
    let runtime_filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if runtime_filename != "yt-dlp.exe" {
        return None;
    }
    let components = value.get("components").and_then(Value::as_array)?;
    if components.len() != 4 {
        return None;
    }
    let required = ["yt-dlp", "deno", "ffmpeg", "ffprobe"];
    if !required.iter().all(|required_name| {
        components
            .iter()
            .filter(|component| {
                component.get("name").and_then(Value::as_str) == Some(*required_name)
            })
            .count()
            == 1
    }) {
        return None;
    }
    components.iter().find_map(|component| {
        if component.get("name").and_then(Value::as_str) != Some("yt-dlp") {
            return None;
        }
        let target_filename = component
            .get("targetFilename")
            .or_else(|| component.get("filename"))
            .and_then(Value::as_str)?;
        if target_filename != "yt-dlp-x86_64-pc-windows-msvc.exe" {
            return None;
        }
        ["sha256", "assetSha256", "digest"]
            .iter()
            .find_map(|key| component.get(*key).and_then(Value::as_str))
            .filter(|digest| {
                digest.len() == 64
                    && digest
                        .chars()
                        .all(|character| character.is_ascii_hexdigit())
            })
            .map(str::to_string)
    })
}

fn digest_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_bounded<R: Read>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if bytes.len() < limit {
            let keep = (limit - bytes.len()).min(read);
            bytes.extend_from_slice(&buffer[..keep]);
            if keep < read {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    Ok((bytes, truncated))
}

#[cfg(windows)]
mod windows_supervisor;
