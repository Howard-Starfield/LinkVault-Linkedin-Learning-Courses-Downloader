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
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use thiserror::Error;

const EMBEDDED_HELPER_LOCK: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/third-party/youtube-helpers-lock.json"
));

// Internal Y0-Y3 execution is admitted only after the ready lock, identity-held
// delegated-helper verification, exact output reuse, FFprobe validation, and
// the complete hostile-process/native shutdown suite have passed review.
// Public packaging remains governed by the separate Y-PUBLIC-REVIEW decision.
const EXECUTION_HARDENING_COMPLETE: bool = true;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperKind {
    YouTubeYtDlp,
    YouTubeFfprobe,
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

    pub fn youtube_ffprobe(
        args: Vec<OsString>,
        stdout_limit: usize,
        stderr_limit: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            helper: HelperKind::YouTubeFfprobe,
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

#[cfg(feature = "youtube-process-test")]
pub struct TestVerifiedExecutable {
    _verified: VerifiedExecutable,
}

#[cfg(feature = "youtube-process-test")]
pub fn lock_test_executable(
    executable: PathBuf,
    expected_digest: &str,
) -> Result<TestVerifiedExecutable, ManagedProcessError> {
    let expected_size = fs::metadata(&executable)
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?
        .len();
    open_verified_executable(executable, expected_digest, expected_digest, expected_size).map(
        |verified| TestVerifiedExecutable {
            _verified: verified,
        },
    )
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

pub(super) struct VerifiedExecutable {
    path: PathBuf,
    _file: File,
    digest: String,
    lock_digest: String,
    size: u64,
    #[cfg(windows)]
    volume_serial_number: u32,
    #[cfg(windows)]
    file_index: u64,
}

impl VerifiedExecutable {
    pub(super) fn revalidate(&self) -> Result<(), ManagedProcessError> {
        let metadata = self
            ._file
            .metadata()
            .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
        if metadata_is_reparse(&metadata) || !metadata.is_file() || metadata.len() != self.size {
            return Err(ManagedProcessError::Integrity(
                "verified helper identity changed before launch".to_string(),
            ));
        }
        let mut recheck = self
            ._file
            .try_clone()
            .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
        let digest = digest_reader(&mut recheck)
            .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
        if digest != self.digest {
            return Err(ManagedProcessError::Integrity(
                "verified helper contents changed before launch".to_string(),
            ));
        }
        #[cfg(windows)]
        {
            let (volume_serial_number, file_index) = windows_file_identity(&self._file)?;
            if volume_serial_number != self.volume_serial_number || file_index != self.file_index {
                return Err(ManagedProcessError::Integrity(
                    "verified helper file identity changed before launch".to_string(),
                ));
            }
        }
        Ok(())
    }
}

struct VerifiedHelperSet {
    ytdlp: VerifiedExecutable,
    deno: VerifiedExecutable,
    ffmpeg: VerifiedExecutable,
    ffprobe: VerifiedExecutable,
}

impl VerifiedHelperSet {
    fn lock_digest(&self) -> &str {
        &self.ytdlp.lock_digest
    }

    fn identities(&self) -> [&VerifiedExecutable; 4] {
        [&self.ytdlp, &self.deno, &self.ffmpeg, &self.ffprobe]
    }

    fn executable(&self, kind: HelperKind) -> &VerifiedExecutable {
        match kind {
            HelperKind::YouTubeYtDlp => &self.ytdlp,
            HelperKind::YouTubeFfprobe => &self.ffprobe,
        }
    }

    fn apply_controlled_args(&self, kind: HelperKind, spec: &mut ManagedProcessSpec) {
        if !matches!(kind, HelperKind::YouTubeYtDlp) {
            return;
        }
        let ffmpeg_directory = self
            .ffmpeg
            .path
            .parent()
            .expect("packaged FFmpeg has an install directory");
        let controlled = [
            OsString::from("--js-runtimes"),
            OsString::from(format!("deno:{}", self.deno.path.display())),
            OsString::from("--ffmpeg-location"),
            ffmpeg_directory.as_os_str().to_os_string(),
        ];
        spec.args.splice(0..0, controlled);
    }
}

pub fn helper_identity(kind: HelperKind) -> Result<HelperIdentity, ManagedProcessError> {
    ensure_execution_hardened()?;
    let verified = resolve_and_verify(kind)?;
    Ok(HelperIdentity {
        digest: verified.lock_digest().to_string(),
    })
}

pub fn run(
    mut spec: ManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    ensure_execution_hardened()?;
    let kind = spec.helper;
    let verified = resolve_and_verify(kind)?;
    verified.apply_controlled_args(kind, &mut spec);
    let executable = verified.executable(kind).path.clone();
    let identities = verified.identities();
    run_resolved(
        executable,
        &identities,
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
        &[],
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
    verified: &[&VerifiedExecutable],
    spec: ManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
    fault: TestFaultSelection,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    #[cfg(windows)]
    {
        windows_supervisor::run(executable, verified, spec, control, discovery_cancel, fault)
    }
    #[cfg(not(windows))]
    {
        let _ = (executable, verified, spec, control, discovery_cancel, fault);
        Err(ManagedProcessError::UnsupportedPlatform)
    }
}

fn resolve_and_verify(kind: HelperKind) -> Result<VerifiedHelperSet, ManagedProcessError> {
    if !matches!(kind, HelperKind::YouTubeYtDlp | HelperKind::YouTubeFfprobe) {
        return Err(ManagedProcessError::Integrity(
            "unsupported helper set".to_string(),
        ));
    }
    let mut locked = locked_helpers().ok_or_else(|| {
        ManagedProcessError::Integrity(
            "the helper lock is absent; Y0 helper validation is required before execution"
                .to_string(),
        )
    })?;
    let install_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .ok_or_else(|| {
            ManagedProcessError::Integrity(
                "the packaged helper directory is unavailable".to_string(),
            )
        })?;
    let lock_digest = locked.lock_digest.clone();
    let mut open = |name: &str| {
        let expected = locked.components.remove(name).ok_or_else(|| {
            ManagedProcessError::Integrity(format!("the {name} lock entry is unavailable"))
        })?;
        open_verified_executable(
            install_dir.join(format!("{name}.exe")),
            &expected.asset_digest,
            &lock_digest,
            expected.size_bytes,
        )
    };
    Ok(VerifiedHelperSet {
        ytdlp: open("yt-dlp")?,
        deno: open("deno")?,
        ffmpeg: open("ffmpeg")?,
        ffprobe: open("ffprobe")?,
    })
}

struct LockedHelperDigest {
    asset_digest: String,
    size_bytes: u64,
}

struct LockedHelpers {
    lock_digest: String,
    components: HashMap<String, LockedHelperDigest>,
}

fn locked_helpers() -> Option<LockedHelpers> {
    let value = serde_json::from_slice::<Value>(EMBEDDED_HELPER_LOCK).ok()?;
    parse_locked_helpers(&value)
}

fn parse_locked_helpers(value: &Value) -> Option<LockedHelpers> {
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(1)
        || value.get("targetTriple").and_then(Value::as_str) != Some("x86_64-pc-windows-msvc")
        || value.get("status").and_then(Value::as_str) != Some("ready")
    {
        return None;
    }
    let lock_digest = value.get("lockDigest").and_then(Value::as_str)?;
    if lock_digest.len() != 64
        || lock_digest != lock_digest.to_ascii_lowercase()
        || !lock_digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    let mut without_digest = value.clone();
    without_digest.as_object_mut()?.remove("lockDigest");
    let canonical = canonical_json(&without_digest)?;
    let mut lock_hasher = Sha256::new();
    lock_hasher.update(canonical);
    if !format!("{:x}", lock_hasher.finalize()).eq_ignore_ascii_case(lock_digest) {
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
    let mut locked = HashMap::new();
    for name in required {
        let component = components
            .iter()
            .find(|component| component.get("name").and_then(Value::as_str) == Some(name))?;
        let expected_filename = format!("{name}-x86_64-pc-windows-msvc.exe");
        if component.get("filename").and_then(Value::as_str) != Some(expected_filename.as_str()) {
            return None;
        }
        // This V1 runtime supports only the four static/standalone executable
        // records it opens and holds below. A future lock that introduces a
        // separately loaded DLL, script, or component must add an equivalent
        // identity-held runtime implementation before it can execute.
        if !component
            .get("loadedAssets")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            return None;
        }
        let asset_digest = component.get("sha256").and_then(Value::as_str)?;
        if asset_digest.len() != 64
            || asset_digest != asset_digest.to_ascii_lowercase()
            || !asset_digest
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return None;
        }
        let size_bytes = component.get("sizeBytes").and_then(Value::as_u64)?;
        if size_bytes == 0 {
            return None;
        }
        locked.insert(
            name.to_string(),
            LockedHelperDigest {
                asset_digest: asset_digest.to_string(),
                size_bytes,
            },
        );
    }
    Some(LockedHelpers {
        lock_digest: lock_digest.to_string(),
        components: locked,
    })
}

fn canonical_json(value: &Value) -> Option<Vec<u8>> {
    fn write(value: &Value, output: &mut String) -> Option<()> {
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                output.push_str(&serde_json::to_string(value).ok()?);
            }
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write(value, output)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(&serde_json::to_string(key).ok()?);
                    output.push(':');
                    write(values.get(key)?, output)?;
                }
                output.push('}');
            }
        }
        Some(())
    }

    let mut output = String::new();
    write(value, &mut output)?;
    Some(output.into_bytes())
}

fn open_verified_executable(
    path: PathBuf,
    expected_digest: &str,
    lock_digest: &str,
    expected_size: u64,
) -> Result<VerifiedExecutable, ManagedProcessError> {
    if !path.is_absolute() {
        return Err(ManagedProcessError::Integrity(
            "helper path is not absolute".to_string(),
        ));
    }
    let path_metadata = fs::symlink_metadata(&path)
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if metadata_is_reparse(&path_metadata) || !path_metadata.file_type().is_file() {
        return Err(ManagedProcessError::Integrity(
            "helper is not a trusted regular file".to_string(),
        ));
    }
    let canonical_path = path
        .canonicalize()
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_READ,
        };
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if metadata_is_reparse(&metadata) || !metadata.is_file() || metadata.len() != expected_size {
        return Err(ManagedProcessError::Integrity(
            "helper handle size does not match the approved helper lock".to_string(),
        ));
    }
    let post_open_path = path
        .canonicalize()
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if post_open_path != canonical_path {
        return Err(ManagedProcessError::Integrity(
            "helper path identity changed while it was opened".to_string(),
        ));
    }
    let actual = digest_reader(&mut file)
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if !actual.eq_ignore_ascii_case(expected_digest) {
        return Err(ManagedProcessError::Integrity(
            "helper digest does not match the approved helper lock".to_string(),
        ));
    }
    verify_pe_x86_64(&mut file)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    #[cfg(windows)]
    let (volume_serial_number, file_index) = windows_file_identity(&file)?;
    Ok(VerifiedExecutable {
        path: canonical_path,
        _file: file,
        digest: actual,
        lock_digest: lock_digest.to_string(),
        size: metadata.len(),
        #[cfg(windows)]
        volume_serial_number,
        #[cfg(windows)]
        file_index,
    })
}

fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> Result<(u32, u64), ManagedProcessError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe {
        GetFileInformationByHandle(
            file.as_raw_handle().cast(),
            &mut information as *mut BY_HANDLE_FILE_INFORMATION,
        )
    } == 0
    {
        return Err(ManagedProcessError::Integrity(
            "helper file identity is unavailable".to_string(),
        ));
    }
    let file_index = ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64;
    Ok((information.dwVolumeSerialNumber, file_index))
}

fn verify_pe_x86_64(file: &mut File) -> Result<(), ManagedProcessError> {
    let mut dos_header = [0u8; 64];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut dos_header))
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if &dos_header[..2] != b"MZ" {
        return Err(ManagedProcessError::Integrity(
            "helper is not a Windows PE executable".to_string(),
        ));
    }
    let pe_offset = u32::from_le_bytes(
        dos_header[0x3c..0x40]
            .try_into()
            .expect("fixed DOS header slice"),
    ) as u64;
    let mut pe_header = [0u8; 6];
    file.seek(SeekFrom::Start(pe_offset))
        .and_then(|_| file.read_exact(&mut pe_header))
        .map_err(|error| ManagedProcessError::Integrity(error.to_string()))?;
    if &pe_header[..4] != b"PE\0\0" || u16::from_le_bytes([pe_header[4], pe_header[5]]) != 0x8664 {
        return Err(ManagedProcessError::Integrity(
            "helper architecture is not x86_64 PE".to_string(),
        ));
    }
    Ok(())
}

fn digest_reader(file: &mut File) -> std::io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
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

#[cfg(test)]
mod tests {
    use super::{canonical_json, parse_locked_helpers};
    use sha2::{Digest, Sha256};

    fn seal_lock(value: &mut serde_json::Value) {
        let mut without_digest = value.clone();
        without_digest.as_object_mut().unwrap().remove("lockDigest");
        value["lockDigest"] = serde_json::Value::String(format!(
            "{:x}",
            Sha256::digest(canonical_json(&without_digest).unwrap())
        ));
    }

    #[test]
    fn canonical_lock_json_sorts_nested_object_keys() {
        let value = serde_json::json!({
            "z": [{ "b": 2, "a": 1 }],
            "a": "value"
        });
        assert_eq!(
            canonical_json(&value).unwrap(),
            br#"{"a":"value","z":[{"a":1,"b":2}]}"#
        );
    }

    #[test]
    fn ready_lock_requires_all_exact_sidecar_source_names() {
        let components = ["yt-dlp", "deno", "ffmpeg", "ffprobe"]
            .into_iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "filename": format!("{name}-x86_64-pc-windows-msvc.exe"),
                    "sha256": "a".repeat(64),
                    "sizeBytes": 1,
                    "loadedAssets": []
                })
            })
            .collect::<Vec<_>>();
        let mut value = serde_json::json!({
            "schemaVersion": 1,
            "targetTriple": "x86_64-pc-windows-msvc",
            "status": "ready",
            "lockDigest": null,
            "components": components
        });
        seal_lock(&mut value);
        let locked = parse_locked_helpers(&value).expect("valid ready lock");
        assert_eq!(locked.components.len(), 4);

        value["components"][0]["filename"] = serde_json::json!("yt-dlp.exe");
        seal_lock(&mut value);
        assert!(parse_locked_helpers(&value).is_none());

        value["components"][0]["filename"] = serde_json::json!("yt-dlp-x86_64-pc-windows-msvc.exe");
        value["components"][0]["loadedAssets"] = serde_json::json!([{
            "filename": "unheld.dll",
            "sha256": "b".repeat(64),
            "sizeBytes": 1
        }]);
        seal_lock(&mut value);
        assert!(parse_locked_helpers(&value).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn delegated_helper_paths_are_app_owned_and_deterministic() {
        use super::{open_verified_executable, ManagedProcessSpec, VerifiedHelperSet};
        use std::ffi::OsString;
        use std::fs;
        use std::time::Duration;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let source = std::env::current_exe().unwrap();
        let bytes = fs::read(&source).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let open = |name: &str| {
            let path = temp.path().join(format!("{name}.exe"));
            fs::write(&path, &bytes).unwrap();
            open_verified_executable(path, &digest, &"b".repeat(64), bytes.len() as u64).unwrap()
        };
        let helpers = VerifiedHelperSet {
            ytdlp: open("yt-dlp"),
            deno: open("deno"),
            ffmpeg: open("ffmpeg"),
            ffprobe: open("ffprobe"),
        };
        let mut spec = ManagedProcessSpec::youtube_ytdlp(
            vec![OsString::from("--ignore-config")],
            1024,
            1024,
            Duration::from_secs(1),
        );
        helpers.apply_controlled_args(super::HelperKind::YouTubeYtDlp, &mut spec);
        assert_eq!(spec.args[0], "--js-runtimes");
        assert!(spec.args[1].to_string_lossy().starts_with("deno:"));
        assert_eq!(spec.args[2], "--ffmpeg-location");
        assert_eq!(spec.args[4], "--ignore-config");
    }
}

#[cfg(windows)]
mod windows_supervisor;
