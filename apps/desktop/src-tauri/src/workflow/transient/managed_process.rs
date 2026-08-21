//! Workflow-owned managed process port for transient external helpers.
//!
//! Provider code can select only a typed helper kind and typed arguments.  It
//! cannot submit an executable path, shell string, or generic command.  The
//! helper path and digest are resolved here from packaged resources and an
//! approved helper lock; missing or mismatched lock data fails closed.

use super::TransientRunControl;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

const EMBEDDED_HELPER_LOCK: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/third-party/youtube-helpers-lock.json"
));

// Defense in depth: a populated supply-chain lock must not silently enable
// execution before the Windows supervisor and identity-held verification land.
// Flip only with the corresponding containment, delegated-helper, and hostile
// replacement tests in the same reviewed change.
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

    #[cfg(test)]
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
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct TestManagedProcessSpec {
    pub executable: PathBuf,
    pub spec: ManagedProcessSpec,
}

#[derive(Debug)]
pub struct ManagedProcessOutput {
    pub status: ExitStatus,
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
    #[error("failed to start trusted helper: {0}")]
    Spawn(String),
    #[error("helper output was not valid UTF-8")]
    InvalidUtf8,
    #[error("helper output reader failed: {0}")]
    Reader(String),
    #[error("helper process wait failed: {0}")]
    Wait(String),
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
    run_resolved(executable, spec, control, discovery_cancel)
}

fn ensure_execution_hardened() -> Result<(), ManagedProcessError> {
    if EXECUTION_HARDENING_COMPLETE {
        return Ok(());
    }
    Err(ManagedProcessError::Integrity(
        "managed helper execution is disabled until Windows Job Object containment, identity-held verification, and delegated-helper pinning pass review"
            .to_string(),
    ))
}

#[cfg(test)]
pub fn run_test(
    test_spec: TestManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    run_resolved(
        test_spec.executable,
        test_spec.spec,
        control,
        discovery_cancel,
    )
}

fn run_resolved(
    executable: PathBuf,
    spec: ManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    let mut command = Command::new(&executable);
    command.args(&spec.args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    for variable in [
        "YTDLP_HOME",
        "XDG_CONFIG_HOME",
        "APPDATA",
        "PYTHONPATH",
        "PYTHONHOME",
        "YOUTUBE_DL_CONFIG",
    ] {
        command.env_remove(variable);
    }
    let mut child = command
        .spawn()
        .map_err(|error| ManagedProcessError::Spawn(error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ManagedProcessError::Reader("helper stdout was not piped".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ManagedProcessError::Reader("helper stderr was not piped".to_string()))?;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    let stdout_limit = spec.stdout_limit;
    let stderr_limit = spec.stderr_limit;
    thread::spawn(move || {
        let _ = stdout_tx.send(read_bounded(stdout, stdout_limit));
    });
    thread::spawn(move || {
        let _ = stderr_tx.send(read_bounded(stderr, stderr_limit));
    });

    let started = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        if control.is_some_and(TransientRunControl::is_cancelled)
            || discovery_cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            cancelled = true;
            terminate_process(&mut child);
            break child
                .wait()
                .map_err(|error| ManagedProcessError::Wait(error.to_string()))?;
        }
        if started.elapsed() >= spec.timeout {
            timed_out = true;
            terminate_process(&mut child);
            break child
                .wait()
                .map_err(|error| ManagedProcessError::Wait(error.to_string()))?;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(ManagedProcessError::Wait(error.to_string())),
        }
    };
    let (stdout_bytes, stdout_truncated) = stdout_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| ManagedProcessError::Reader(error.to_string()))?;
    let (stderr_bytes, stderr_truncated) = stderr_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| ManagedProcessError::Reader(error.to_string()))?;
    let stdout = String::from_utf8(stdout_bytes).map_err(|_| ManagedProcessError::InvalidUtf8)?;
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    Ok(ManagedProcessOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        timed_out,
        cancelled,
    })
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

fn lock_digest_for(path: &std::path::Path) -> Option<String> {
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

fn digest_file(path: &std::path::Path) -> std::io::Result<String> {
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

fn terminate_process(child: &mut std::process::Child) {
    // This slice owns and joins the direct helper child.  Descendant Job
    // Object containment remains a release-blocking follow-up until the
    // Windows native supervisor is added; do not claim tree containment yet.
    let _ = child.kill();
}

fn read_bounded<R: Read>(mut reader: R, limit: usize) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
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
            Err(_) => break,
        }
    }
    (bytes, truncated)
}
