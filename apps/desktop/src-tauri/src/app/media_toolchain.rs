//! Versioned, app-owned optional media-toolchain installer for LinkedVault.
//!
//! Slim sync/blocking V1 port of the Infield media-toolchain consumer. Trust,
//! staging, activation, and verified path resolution stay in one owner. Release
//! manifests must be signed by a reviewed public key pinned in
//! `TRUSTED_MANIFEST_KEYS`. Layout lives under `LinkVaultData/media-toolchain`.

use base64::Engine;
use chrono::Utc;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use zip::ZipArchive;

use crate::app::storage::{self, StoragePathError};

/// Stable public release-manifest location.
pub const MEDIA_TOOLCHAIN_MANIFEST_URL: &str =
    "https://github.com/Howard-Starfield/Infield-media-toolchain/releases/latest/download/manifest.json";

const MEDIA_TOOLCHAIN_STATE_SCHEMA_VERSION: u32 = 1;
const MEDIA_TOOLCHAIN_MANIFEST_SCHEMA_VERSION: u32 = 1;
const MEDIA_TOOLCHAIN_V1_TARGET: &str = "windows-x86_64";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 1024;
const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const MEDIA_TOOLCHAIN_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MEDIA_TOOLCHAIN_MANIFEST_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MEDIA_TOOLCHAIN_ARCHIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(20 * 60);

// Release engineering owns the production key ceremony. Never accept an
// unsigned manifest or a key supplied by that same manifest.
const TRUSTED_MANIFEST_KEYS: &[(&str, &str)] = &[(
    "infield-ed25519-2026-08",
    "J3OZsOl/8QB98szTw8+/jZQRCbFyGYJgGAZ2ipfAnPc=",
)];

type ToolchainResult<T> = Result<T, MediaToolchainError>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaToolchainComponentKind {
    YtDlp,
    Ffmpeg,
    Deno,
}

impl MediaToolchainComponentKind {
    fn health_args(&self) -> &'static [&'static str] {
        match self {
            Self::YtDlp | Self::Deno => &["--version"],
            Self::Ffmpeg => &["-version"],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaToolchainErrorCode {
    ManifestUnavailable,
    ManifestInvalid,
    SignatureMissing,
    SignatureInvalid,
    TrustedKeyMissing,
    UnsafePath,
    UnsupportedPlatform,
    DownloadFailed,
    ArchiveIntegrityFailed,
    ExtractionFailed,
    ComponentIntegrityFailed,
    HealthCheckFailed,
    ActivationFailed,
    DowngradeRejected,
    Io,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{code:?}: {message}")]
pub struct MediaToolchainError {
    pub code: MediaToolchainErrorCode,
    pub message: String,
}

impl MediaToolchainError {
    fn new(code: MediaToolchainErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<StoragePathError> for MediaToolchainError {
    fn from(error: StoragePathError) -> Self {
        Self::new(MediaToolchainErrorCode::Io, error.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaToolchainComponentStatus {
    pub kind: MediaToolchainComponentKind,
    pub available: bool,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaToolchainStatus {
    /// Strict web-media readiness: yt-dlp, Deno, and FFmpeg verified and present.
    pub ready: bool,
    pub media_core_ready: bool,
    pub web_media_ready: bool,
    pub managed_install_present: bool,
    pub active_version: Option<String>,
    pub previous_version: Option<String>,
    pub last_checked_at: Option<String>,
    pub latest_available: Option<String>,
    pub components: Vec<MediaToolchainComponentStatus>,
    pub error: Option<String>,
}

/// Paths and digests from the active install after signature + hash verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedToolchainPaths {
    pub root: PathBuf,
    pub version: String,
    pub yt_dlp: PathBuf,
    pub ffmpeg: PathBuf,
    pub deno: PathBuf,
    pub yt_dlp_sha256: String,
    pub ffmpeg_sha256: String,
    pub deno_sha256: String,
    pub yt_dlp_size_bytes: Option<u64>,
    pub ffmpeg_size_bytes: Option<u64>,
    pub deno_size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaToolchainManifest {
    pub schema_version: u32,
    pub version: String,
    pub platforms: Vec<MediaToolchainPlatformManifest>,
    pub signature: MediaToolchainManifestSignature,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaToolchainPlatformManifest {
    pub target: String,
    pub archive: MediaToolchainArchive,
    pub components: Vec<MediaToolchainComponentArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaToolchainArchive {
    pub url: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaToolchainComponentArtifact {
    pub kind: MediaToolchainComponentKind,
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaToolchainManifestSignature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
}

#[derive(Clone, Copy)]
struct RejectDuplicateJsonMembers;

impl<'de> DeserializeSeed<'de> for RejectDuplicateJsonMembers {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for RejectDuplicateJsonMembers {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object members")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element_seed(self)?.is_some() {}
        Ok(())
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut members = BTreeSet::new();
        while let Some(member) = object.next_key::<String>()? {
            if !members.insert(member.clone()) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object member `{member}`"
                )));
            }
            object.next_value_seed(self)?;
        }
        Ok(())
    }
}

fn parse_media_toolchain_manifest(bytes: &[u8]) -> ToolchainResult<MediaToolchainManifest> {
    let mut duplicate_guard = serde_json::Deserializer::from_slice(bytes);
    RejectDuplicateJsonMembers
        .deserialize(&mut duplicate_guard)
        .and_then(|()| duplicate_guard.end())
        .map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                "Media toolchain manifest is not valid strict v1 JSON",
            )
        })?;

    serde_json::from_slice(bytes).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media toolchain manifest is not valid v1 JSON",
        )
    })
}

#[derive(Serialize)]
struct UnsignedManifest<'a> {
    schema_version: u32,
    version: &'a str,
    platforms: &'a [MediaToolchainPlatformManifest],
}

impl MediaToolchainManifest {
    /// Compact JSON in fixed field order with `signature` omitted.
    pub fn canonical_signed_bytes(&self) -> ToolchainResult<Vec<u8>> {
        serde_json::to_vec(&UnsignedManifest {
            schema_version: self.schema_version,
            version: &self.version,
            platforms: &self.platforms,
        })
        .map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                format!("Serialize manifest for signature verification: {error}"),
            )
        })
    }

    pub fn platform_for_host(&self) -> ToolchainResult<&MediaToolchainPlatformManifest> {
        let target = host_target();
        self.platforms
            .iter()
            .find(|platform| platform.target == target)
            .ok_or_else(|| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::UnsupportedPlatform,
                    format!("No media-toolchain release is published for {target}"),
                )
            })
    }

    fn validate_shape(&self) -> ToolchainResult<()> {
        if self.schema_version != MEDIA_TOOLCHAIN_MANIFEST_SCHEMA_VERSION {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                format!(
                    "Unsupported media-toolchain manifest schema {}",
                    self.schema_version
                ),
            ));
        }
        validate_version(&self.version)?;
        if self.platforms.len() != 1 {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                "Media-toolchain manifest v1 must contain exactly one windows-x86_64 platform",
            ));
        }
        let platform = &self.platforms[0];
        if platform.target != MEDIA_TOOLCHAIN_V1_TARGET {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                "Media-toolchain manifest v1 must target windows-x86_64",
            ));
        }
        validate_archive_reference(&platform.archive.url)?;
        validate_sha256(&platform.archive.sha256)?;
        let mut kinds = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for component in &platform.components {
            if !kinds.insert(component.kind.clone()) {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ManifestInvalid,
                    format!("Duplicate {} component", component_name(&component.kind)),
                ));
            }
            validate_component_path(&component.kind, &component.path)?;
            if !paths.insert(&component.path) {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ManifestInvalid,
                    "Media-toolchain manifest has duplicate component paths",
                ));
            }
            validate_sha256(&component.sha256)?;
        }
        for required in [
            MediaToolchainComponentKind::YtDlp,
            MediaToolchainComponentKind::Ffmpeg,
            MediaToolchainComponentKind::Deno,
        ] {
            if !kinds.contains(&required) {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ManifestInvalid,
                    format!("Missing required {} component", component_name(&required)),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct TrustedManifestKeys {
    keys: std::collections::BTreeMap<String, [u8; 32]>,
}

impl TrustedManifestKeys {
    fn from_entries(entries: impl IntoIterator<Item = (String, [u8; 32])>) -> Self {
        Self {
            keys: entries.into_iter().collect(),
        }
    }
}

fn production_trust_store() -> TrustedManifestKeys {
    let mut entries = Vec::new();
    for (key_id, encoded) in TRUSTED_MANIFEST_KEYS {
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            continue;
        };
        let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice()) else {
            continue;
        };
        entries.push(((*key_id).to_string(), bytes));
    }
    TrustedManifestKeys::from_entries(entries)
}

fn selected_manifest_url() -> ToolchainResult<&'static str> {
    require_https_url(MEDIA_TOOLCHAIN_MANIFEST_URL)?;
    Ok(MEDIA_TOOLCHAIN_MANIFEST_URL)
}

fn verify_manifest_signature(
    manifest: &MediaToolchainManifest,
    trusted_keys: &TrustedManifestKeys,
) -> ToolchainResult<()> {
    manifest.validate_shape()?;
    if manifest.signature.algorithm != "ed25519" {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::SignatureInvalid,
            "Media-toolchain manifest uses an unsupported signature algorithm",
        ));
    }
    if manifest.signature.key_id.trim().is_empty() || manifest.signature.value.trim().is_empty() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::SignatureMissing,
            "Media-toolchain manifest is missing its signature or key ID",
        ));
    }
    if !valid_key_id(&manifest.signature.key_id) {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::SignatureInvalid,
            "The media-toolchain manifest has an invalid signature key ID",
        ));
    }
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(&manifest.signature.value)
        .map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::SignatureInvalid,
                "The media-toolchain manifest signature is not valid base64",
            )
        })?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::SignatureInvalid,
            "The media-toolchain manifest signature has an invalid length",
        )
    })?;
    let Some(public_key_bytes) = trusted_keys.keys.get(&manifest.signature.key_id) else {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::TrustedKeyMissing,
            "The media-toolchain manifest is signed by a key this app does not trust",
        ));
    };
    let public_key = VerifyingKey::from_bytes(public_key_bytes).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::SignatureInvalid,
            "The configured media-toolchain trust key is invalid",
        )
    })?;
    public_key
        .verify(&manifest.canonical_signed_bytes()?, &signature)
        .map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::SignatureInvalid,
                "The media-toolchain manifest signature did not verify",
            )
        })
}

fn host_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        _ => "unsupported",
    }
}

fn component_name(component: &MediaToolchainComponentKind) -> &'static str {
    match component {
        MediaToolchainComponentKind::YtDlp => "yt-dlp",
        MediaToolchainComponentKind::Ffmpeg => "ffmpeg",
        MediaToolchainComponentKind::Deno => "deno",
    }
}

fn validate_version(version: &str) -> ToolchainResult<()> {
    let valid = version.len() <= 120
        && regex::Regex::new(
            r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$",
        )
        .expect("the manifest SemVer regex is valid")
        .is_match(version)
        && semver::Version::parse(version).is_ok();
    if valid {
        Ok(())
    } else {
        Err(MediaToolchainError::new(
            MediaToolchainErrorCode::UnsafePath,
            "Media-toolchain version is not a safe filesystem name",
        ))
    }
}

fn reject_downgrade(active_version: Option<&str>, candidate_version: &str) -> ToolchainResult<()> {
    let candidate = semver::Version::parse(candidate_version).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media-toolchain manifest version is not valid SemVer",
        )
    })?;
    let Some(active_version) = active_version else {
        return Ok(());
    };
    let active = semver::Version::parse(active_version).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            "The installed media-toolchain version is not valid SemVer",
        )
    })?;
    if candidate.cmp_precedence(&active) == std::cmp::Ordering::Less {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::DowngradeRejected,
            "Media toolchain rejects a lower signed version",
        ));
    }
    Ok(())
}

pub fn validate_relative_path(value: &str) -> ToolchainResult<()> {
    if value.trim().is_empty()
        || value.contains('\0')
        || value.contains('\\')
        || value.starts_with('/')
        || value
            .as_bytes()
            .get(0..2)
            .is_some_and(|prefix| prefix[0].is_ascii_alphabetic() && prefix[1] == b':')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
        || value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::UnsafePath,
            "Media-toolchain manifest contains an unsafe relative path",
        ));
    }
    Ok(())
}

fn expected_component_path(kind: &MediaToolchainComponentKind) -> &'static str {
    match kind {
        MediaToolchainComponentKind::Ffmpeg => "bin/ffmpeg.exe",
        MediaToolchainComponentKind::YtDlp => "bin/yt-dlp.exe",
        MediaToolchainComponentKind::Deno => "bin/deno.exe",
    }
}

fn validate_component_path(kind: &MediaToolchainComponentKind, value: &str) -> ToolchainResult<()> {
    validate_relative_path(value)?;
    if value != expected_component_path(kind) {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            format!(
                "Media-toolchain {} component must be at {}",
                component_name(kind),
                expected_component_path(kind)
            ),
        ));
    }
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> ToolchainResult<PathBuf> {
    validate_relative_path(relative)?;
    Ok(root.join(relative))
}

fn validate_sha256(value: &str) -> ToolchainResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media-toolchain manifest contains an invalid SHA-256 digest",
        ))
    }
}

fn valid_key_id(value: &str) -> bool {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
        .expect("the manifest key ID regex is valid")
        .is_match(value)
}

fn validate_archive_reference(value: &str) -> ToolchainResult<()> {
    if let Some(relative) = value.strip_prefix("./") {
        return validate_relative_path(relative).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                "Media-toolchain manifest contains an invalid relative archive URL",
            )
        });
    }
    require_https_url(value)
}

fn require_https_url(value: &str) -> ToolchainResult<()> {
    let parsed = reqwest::Url::parse(value).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media-toolchain manifest contains an invalid archive URL",
        )
    })?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media-toolchain downloads must use an HTTPS URL",
        ));
    }
    Ok(())
}

fn resolve_archive_url_from_manifest(
    value: &str,
    manifest_url: &str,
) -> ToolchainResult<reqwest::Url> {
    validate_archive_reference(value)?;
    require_https_url(manifest_url)?;
    let manifest_url = reqwest::Url::parse(manifest_url).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "The selected media-toolchain manifest URL is invalid",
        )
    })?;
    let resolved = if value.starts_with("./") {
        manifest_url.join(value).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                "Media-toolchain manifest contains an invalid relative archive URL",
            )
        })?
    } else {
        reqwest::Url::parse(value).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                "Media-toolchain manifest contains an invalid archive URL",
            )
        })?
    };
    if resolved.scheme() != "https" || resolved.host_str().is_none() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media-toolchain archive URLs must use HTTPS",
        ));
    }
    if !value.starts_with("./") && !same_origin(&manifest_url, &resolved) {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media-toolchain archive URLs must use the manifest origin",
        ));
    }
    Ok(resolved)
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

#[derive(Clone, Debug)]
struct MediaToolchainLayout {
    root: PathBuf,
}

impl MediaToolchainLayout {
    fn resolve() -> ToolchainResult<Self> {
        Ok(Self {
            root: storage::resolve_data_dir()?.join("media-toolchain"),
        })
    }

    #[cfg(test)]
    fn for_root(root: PathBuf) -> Self {
        Self { root }
    }

    fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    fn version_dir(&self, version: &str) -> PathBuf {
        self.versions_dir().join(version)
    }

    fn staging_dir(&self, operation_id: &str) -> PathBuf {
        self.root.join("staging").join(operation_id)
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct MediaToolchainInstallState {
    #[serde(default = "default_state_schema_version")]
    schema_version: u32,
    #[serde(default)]
    active_version: Option<String>,
    #[serde(default)]
    previous_version: Option<String>,
    #[serde(default)]
    last_checked_at: Option<String>,
    #[serde(default)]
    latest_available: Option<String>,
    #[serde(default)]
    latest_manifest_size_bytes: Option<u64>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn default_state_schema_version() -> u32 {
    MEDIA_TOOLCHAIN_STATE_SCHEMA_VERSION
}

fn read_install_state(layout: &MediaToolchainLayout) -> ToolchainResult<MediaToolchainInstallState> {
    let path = layout.state_path();
    if !path.exists() {
        return Ok(MediaToolchainInstallState::default());
    }
    let bytes = fs::read(&path).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Read media-toolchain install state: {error}"),
        )
    })?;
    let state: MediaToolchainInstallState = serde_json::from_slice(&bytes).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            "Media-toolchain install state is unreadable",
        )
    })?;
    if state.schema_version != MEDIA_TOOLCHAIN_STATE_SCHEMA_VERSION {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            "Media-toolchain install state uses an unsupported schema",
        ));
    }
    for version in [
        state.active_version.as_deref(),
        state.previous_version.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if validate_version(version).is_err() {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ActivationFailed,
                "Media-toolchain install state contains an invalid version",
            ));
        }
    }
    Ok(state)
}

fn write_install_state_atomic(
    layout: &MediaToolchainLayout,
    mut state: MediaToolchainInstallState,
) -> ToolchainResult<()> {
    state.schema_version = MEDIA_TOOLCHAIN_STATE_SCHEMA_VERSION;
    state.updated_at = Some(Utc::now().to_rfc3339());
    let bytes = serde_json::to_vec_pretty(&state).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Serialize media-toolchain install state: {error}"),
        )
    })?;
    atomic_write(&layout.state_path(), &bytes)
}

fn unique_temp_token() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> ToolchainResult<()> {
    let parent = path.parent().ok_or_else(|| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            "Media-toolchain state has no parent directory",
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Create media-toolchain state directory: {error}"),
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::Io,
                "Media-toolchain state filename is invalid",
            )
        })?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", unique_temp_token()));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file_atomic(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Atomically write media-toolchain state: {error}"),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file_atomic(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: MoveFileExW is called with NUL-terminated wide paths owned by this
    // function for the duration of the call; REPLACE_EXISTING + WRITE_THROUGH
    // are the documented flags for atomic replace on Windows.
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file_atomic(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

fn sha256_file(path: &Path) -> ToolchainResult<String> {
    let mut file = fs::File::open(path).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Open media-toolchain file for hashing: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::Io,
                format!("Read media-toolchain file for hashing: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_component_hashes(
    root: &Path,
    platform: &MediaToolchainPlatformManifest,
) -> ToolchainResult<()> {
    for component in &platform.components {
        let path = safe_join(root, &component.path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ComponentIntegrityFailed,
                format!(
                    "Media toolchain {} component is missing",
                    component_name(&component.kind)
                ),
            )
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ComponentIntegrityFailed,
                format!(
                    "Media toolchain {} component is not a file",
                    component_name(&component.kind)
                ),
            ));
        }
        if component
            .size_bytes
            .is_some_and(|expected| expected != metadata.len())
        {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ComponentIntegrityFailed,
                format!(
                    "Media toolchain {} component size did not match",
                    component_name(&component.kind)
                ),
            ));
        }
        let actual = sha256_file(&path)?;
        if actual != component.sha256 {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ComponentIntegrityFailed,
                format!(
                    "Media toolchain {} component failed its integrity check",
                    component_name(&component.kind)
                ),
            ));
        }
    }
    verify_no_extra_bin_files(root, platform)
}

fn verify_no_extra_bin_files(
    root: &Path,
    platform: &MediaToolchainPlatformManifest,
) -> ToolchainResult<()> {
    let bin = root.join("bin");
    if !bin.exists() {
        return Ok(());
    }
    let bin_metadata = fs::symlink_metadata(&bin).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ComponentIntegrityFailed,
            "Media toolchain bin directory is unreadable",
        )
    })?;
    if !bin_metadata.is_dir() || bin_metadata.file_type().is_symlink() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ComponentIntegrityFailed,
            "Media toolchain bin path is not a real directory",
        ));
    }
    let declared_paths = platform
        .components
        .iter()
        .map(|component| component.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut pending = vec![bin];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ComponentIntegrityFailed,
                format!("Read media-toolchain bin directory: {error}"),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::ComponentIntegrityFailed,
                    format!("Read media-toolchain bin entry: {error}"),
                )
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::ComponentIntegrityFailed,
                    format!("Read media-toolchain bin entry metadata: {error}"),
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ComponentIntegrityFailed,
                    "Media toolchain bin directory contains a symbolic link",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ComponentIntegrityFailed,
                    "Media toolchain bin directory contains an unsupported file type",
                ));
            }
            let relative = path
                .strip_prefix(root)
                .ok()
                .and_then(|path| path.to_str())
                .map(|path| path.replace('\\', "/"))
                .ok_or_else(|| {
                    MediaToolchainError::new(
                        MediaToolchainErrorCode::ComponentIntegrityFailed,
                        "Media toolchain bin path is not valid UTF-8",
                    )
                })?;
            if !declared_paths.contains(relative.as_str()) {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ComponentIntegrityFailed,
                    "Media toolchain archive contains an undeclared binary",
                ));
            }
        }
    }
    Ok(())
}

fn artifact_for<'a>(
    platform: &'a MediaToolchainPlatformManifest,
    kind: MediaToolchainComponentKind,
) -> ToolchainResult<&'a MediaToolchainComponentArtifact> {
    platform
        .components
        .iter()
        .find(|component| component.kind == kind)
        .ok_or_else(|| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ComponentIntegrityFailed,
                format!("Missing {} component in verified manifest", component_name(&kind)),
            )
        })
}

fn build_verified_paths(
    version_root: PathBuf,
    version: String,
    platform: &MediaToolchainPlatformManifest,
) -> ToolchainResult<VerifiedToolchainPaths> {
    let yt_dlp = artifact_for(platform, MediaToolchainComponentKind::YtDlp)?;
    let ffmpeg = artifact_for(platform, MediaToolchainComponentKind::Ffmpeg)?;
    let deno = artifact_for(platform, MediaToolchainComponentKind::Deno)?;
    Ok(VerifiedToolchainPaths {
        yt_dlp: safe_join(&version_root, &yt_dlp.path)?,
        ffmpeg: safe_join(&version_root, &ffmpeg.path)?,
        deno: safe_join(&version_root, &deno.path)?,
        yt_dlp_sha256: yt_dlp.sha256.clone(),
        ffmpeg_sha256: ffmpeg.sha256.clone(),
        deno_sha256: deno.sha256.clone(),
        yt_dlp_size_bytes: yt_dlp.size_bytes,
        ffmpeg_size_bytes: ffmpeg.size_bytes,
        deno_size_bytes: deno.size_bytes,
        root: version_root,
        version,
    })
}

fn read_verified_active(
    layout: &MediaToolchainLayout,
) -> ToolchainResult<Option<(String, MediaToolchainPlatformManifest, VerifiedToolchainPaths)>> {
    let state = read_install_state(layout)?;
    let Some(active_version) = state.active_version.clone() else {
        return Ok(None);
    };
    validate_version(&active_version)?;
    let version_root = layout.version_dir(&active_version);
    let manifest_path = version_root.join("manifest.json");
    let bytes = fs::read(&manifest_path).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ComponentIntegrityFailed,
            "The active media-toolchain install is missing its manifest",
        )
    })?;
    let manifest = parse_media_toolchain_manifest(&bytes)?;
    if manifest.version != active_version {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ComponentIntegrityFailed,
            "The active media-toolchain version does not match its manifest",
        ));
    }
    verify_manifest_signature(&manifest, &production_trust_store())?;
    let platform = manifest.platform_for_host()?.clone();
    verify_component_hashes(&version_root, &platform)?;
    let paths = build_verified_paths(version_root, active_version.clone(), &platform)?;
    Ok(Some((active_version, platform, paths)))
}

fn status_from_layout(layout: &MediaToolchainLayout) -> MediaToolchainStatus {
    let managed_install_present = layout.root.exists();
    let state = match read_install_state(layout) {
        Ok(state) => state,
        Err(error) => {
            return MediaToolchainStatus {
                ready: false,
                media_core_ready: false,
                web_media_ready: false,
                managed_install_present,
                active_version: None,
                previous_version: None,
                last_checked_at: None,
                latest_available: None,
                components: empty_component_status(),
                error: Some(error.message),
            };
        }
    };

    match read_verified_active(layout) {
        Ok(Some((_version, _platform, paths))) => {
            let components = vec![
                MediaToolchainComponentStatus {
                    kind: MediaToolchainComponentKind::YtDlp,
                    available: true,
                    path: Some(paths.yt_dlp.display().to_string()),
                },
                MediaToolchainComponentStatus {
                    kind: MediaToolchainComponentKind::Ffmpeg,
                    available: true,
                    path: Some(paths.ffmpeg.display().to_string()),
                },
                MediaToolchainComponentStatus {
                    kind: MediaToolchainComponentKind::Deno,
                    available: true,
                    path: Some(paths.deno.display().to_string()),
                },
            ];
            MediaToolchainStatus {
                ready: true,
                media_core_ready: true,
                web_media_ready: true,
                managed_install_present,
                active_version: state.active_version,
                previous_version: state.previous_version,
                last_checked_at: state.last_checked_at,
                latest_available: state.latest_available,
                components,
                error: None,
            }
        }
        Ok(None) => MediaToolchainStatus {
            ready: false,
            media_core_ready: false,
            web_media_ready: false,
            managed_install_present,
            active_version: state.active_version,
            previous_version: state.previous_version,
            last_checked_at: state.last_checked_at,
            latest_available: state.latest_available,
            components: empty_component_status(),
            error: None,
        },
        Err(error) => MediaToolchainStatus {
            ready: false,
            media_core_ready: false,
            web_media_ready: false,
            managed_install_present,
            active_version: state.active_version,
            previous_version: state.previous_version,
            last_checked_at: state.last_checked_at,
            latest_available: state.latest_available,
            components: empty_component_status(),
            error: Some(error.message),
        },
    }
}

fn empty_component_status() -> Vec<MediaToolchainComponentStatus> {
    [
        MediaToolchainComponentKind::YtDlp,
        MediaToolchainComponentKind::Ffmpeg,
        MediaToolchainComponentKind::Deno,
    ]
    .into_iter()
    .map(|kind| MediaToolchainComponentStatus {
        kind,
        available: false,
        path: None,
    })
    .collect()
}

/// Report whether a verified managed toolchain is active.
pub fn status() -> ToolchainResult<MediaToolchainStatus> {
    let layout = MediaToolchainLayout::resolve()?;
    Ok(status_from_layout(&layout))
}

/// Re-verify the active install and return helper paths plus manifest digests.
pub fn verified_component_paths() -> ToolchainResult<Option<VerifiedToolchainPaths>> {
    let layout = MediaToolchainLayout::resolve()?;
    Ok(read_verified_active(&layout)?.map(|(_, _, paths)| paths))
}

/// Download, verify, extract, health-check, and atomically activate the latest
/// signed toolchain release. Blocking; uses sync reqwest.
pub fn install_latest() -> ToolchainResult<MediaToolchainStatus> {
    let layout = MediaToolchainLayout::resolve()?;
    let operation_id = format!("install-{}", unique_temp_token());
    let stage = layout.staging_dir(&operation_id);
    let archive_path = stage.join("toolchain.zip");
    let candidate = stage.join("candidate");

    fs::create_dir_all(&stage).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Create media-toolchain staging directory: {error}"),
        )
    })?;

    let result = (|| -> ToolchainResult<()> {
        let (manifest_bytes, manifest) = fetch_verified_manifest()?;
        let installed = read_install_state(&layout)?;
        reject_downgrade(installed.active_version.as_deref(), &manifest.version)?;

        // Same version already verified: refresh state timestamps and return.
        if installed.active_version.as_deref() == Some(manifest.version.as_str()) {
            if read_verified_active(&layout)?.is_some() {
                let mut state = installed;
                state.last_checked_at = Some(Utc::now().to_rfc3339());
                state.latest_available = Some(manifest.version.clone());
                write_install_state_atomic(&layout, state)?;
                return Ok(());
            }
        }

        let platform = manifest.platform_for_host()?.clone();
        download_archive(&platform.archive, &archive_path)?;
        let actual_archive_hash = sha256_file(&archive_path)?;
        if actual_archive_hash != platform.archive.sha256 {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ArchiveIntegrityFailed,
                "The media-toolchain archive failed its integrity check",
            ));
        }
        extract_archive_safely(&archive_path, &candidate)?;
        atomic_write(&candidate.join("manifest.json"), &manifest_bytes)?;
        verify_component_hashes(&candidate, &platform)?;
        health_check_components(&candidate, &platform)?;
        activate_candidate(
            &layout,
            &manifest.version,
            &candidate,
            platform.archive.size_bytes,
        )?;
        Ok(())
    })();

    let _ = fs::remove_dir_all(&stage);
    result?;
    Ok(status_from_layout(&layout))
}

fn fetch_verified_manifest() -> ToolchainResult<(Vec<u8>, MediaToolchainManifest)> {
    let manifest_url = selected_manifest_url()?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(MEDIA_TOOLCHAIN_CONNECT_TIMEOUT)
        .timeout(MEDIA_TOOLCHAIN_MANIFEST_REQUEST_TIMEOUT)
        .build()
        .map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestUnavailable,
                format!("Create media-toolchain HTTP client: {error}"),
            )
        })?;
    let response = client.get(manifest_url).send().map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestUnavailable,
            "Could not download the media-toolchain manifest",
        )
    })?;
    if !response.status().is_success() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestUnavailable,
            "The media-toolchain manifest server returned an error",
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES)
    {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ManifestInvalid,
            "Media-toolchain manifest exceeds the safety limit",
        ));
    }
    let mut bytes = Vec::new();
    let mut reader = response;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestUnavailable,
                "Could not read the media-toolchain manifest",
            )
        })?;
        if read == 0 {
            break;
        }
        if (bytes.len() as u64).saturating_add(read as u64) > MAX_MANIFEST_BYTES {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ManifestInvalid,
                "Media-toolchain manifest exceeds the safety limit",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let manifest = parse_media_toolchain_manifest(&bytes)?;
    verify_manifest_signature(&manifest, &production_trust_store())?;
    Ok((bytes, manifest))
}

fn download_archive(archive: &MediaToolchainArchive, destination: &Path) -> ToolchainResult<()> {
    let archive_url = resolve_archive_url_from_manifest(&archive.url, selected_manifest_url()?)?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(MEDIA_TOOLCHAIN_CONNECT_TIMEOUT)
        .timeout(MEDIA_TOOLCHAIN_ARCHIVE_REQUEST_TIMEOUT)
        .build()
        .map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::DownloadFailed,
                format!("Create media-toolchain archive client: {error}"),
            )
        })?;
    let response = client.get(archive_url).send().map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::DownloadFailed,
            "Could not download media-toolchain archive",
        )
    })?;
    if !response.status().is_success() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::DownloadFailed,
            "The media-toolchain download server returned an error",
        ));
    }
    let total = response.content_length().or(archive.size_bytes);
    if total.is_some_and(|bytes| bytes > MAX_ARCHIVE_BYTES) {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::DownloadFailed,
            "Media-toolchain archive exceeds the safety limit",
        ));
    }
    let mut file = fs::File::create(destination).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Create media-toolchain archive staging file: {error}"),
        )
    })?;
    let mut reader = response;
    let mut buffer = [0_u8; 64 * 1024];
    let mut downloaded = 0_u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::DownloadFailed,
                "Media-toolchain download was interrupted",
            )
        })?;
        if read == 0 {
            break;
        }
        downloaded = downloaded.saturating_add(read as u64);
        if downloaded > MAX_ARCHIVE_BYTES {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::DownloadFailed,
                "Media-toolchain archive exceeds the safety limit",
            ));
        }
        file.write_all(&buffer[..read]).map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::Io,
                format!("Write media-toolchain archive: {error}"),
            )
        })?;
    }
    file.sync_all().map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::Io,
            format!("Flush media-toolchain archive: {error}"),
        )
    })?;
    if archive
        .size_bytes
        .is_some_and(|expected| expected != downloaded)
    {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::DownloadFailed,
            "Media-toolchain archive size did not match its manifest",
        ));
    }
    Ok(())
}

fn extract_archive_safely(archive_path: &Path, destination: &Path) -> ToolchainResult<()> {
    if destination.exists() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ExtractionFailed,
            "Media-toolchain extraction destination is not fresh",
        ));
    }
    fs::create_dir_all(destination).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ExtractionFailed,
            format!("Create media-toolchain extraction directory: {error}"),
        )
    })?;
    let file = fs::File::open(archive_path).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ExtractionFailed,
            format!("Open media-toolchain archive: {error}"),
        )
    })?;
    let mut archive = ZipArchive::new(file).map_err(|_| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ExtractionFailed,
            "Media-toolchain archive is not a valid ZIP file",
        )
    })?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ExtractionFailed,
            "Media-toolchain archive contains too many files",
        ));
    }
    let mut extracted_bytes = 0_u64;
    let mut extracted_paths = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                "Could not read a media-toolchain archive entry",
            )
        })?;
        let raw_name = entry.name().to_string();
        let name = if entry.is_dir() {
            raw_name.trim_end_matches('/')
        } else {
            raw_name.as_str()
        };
        validate_relative_path(name).map_err(|_| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                "Media-toolchain archive contains an unsafe path",
            )
        })?;
        if !extracted_paths.insert(name.to_string()) {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                "Media-toolchain archive contains duplicate paths",
            ));
        }
        if entry.is_symlink() {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                "Media-toolchain archive contains a symbolic link",
            ));
        }
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            let expected_type = if entry.is_dir() { 0o040000 } else { 0o100000 };
            if file_type != 0 && file_type != expected_type {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ExtractionFailed,
                    "Media-toolchain archive contains an unsupported file type",
                ));
            }
        }
        let target = safe_join(destination, name)?;
        if entry.is_dir() {
            if target.exists()
                && !fs::symlink_metadata(&target)
                    .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
                    .unwrap_or(false)
            {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ExtractionFailed,
                    "Media-toolchain archive has a file-directory path conflict",
                ));
            }
            fs::create_dir_all(&target).map_err(|error| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::ExtractionFailed,
                    format!("Create media-toolchain archive directory: {error}"),
                )
            })?;
            continue;
        }
        if extracted_bytes.saturating_add(entry.size()) > MAX_EXTRACTED_BYTES {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                "Media-toolchain archive expands beyond the safety limit",
            ));
        }
        let parent = target.parent().ok_or_else(|| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                "Media-toolchain archive entry has no parent",
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                format!("Create media-toolchain archive parent: {error}"),
            )
        })?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|error| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::ExtractionFailed,
                    format!("Create media-toolchain extracted file: {error}"),
                )
            })?;
        let mut entry_bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer).map_err(|error| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::ExtractionFailed,
                    format!("Extract media-toolchain archive entry: {error}"),
                )
            })?;
            if read == 0 {
                break;
            }
            entry_bytes = entry_bytes.saturating_add(read as u64);
            extracted_bytes = extracted_bytes.saturating_add(read as u64);
            if entry_bytes > entry.size() || extracted_bytes > MAX_EXTRACTED_BYTES {
                return Err(MediaToolchainError::new(
                    MediaToolchainErrorCode::ExtractionFailed,
                    "Media-toolchain archive expands beyond the safety limit",
                ));
            }
            output.write_all(&buffer[..read]).map_err(|error| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::ExtractionFailed,
                    format!("Write media-toolchain extracted file: {error}"),
                )
            })?;
        }
        if entry_bytes != entry.size() {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                "Media-toolchain archive entry size is inconsistent",
            ));
        }
        output.sync_all().map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ExtractionFailed,
                format!("Flush media-toolchain extracted file: {error}"),
            )
        })?;
    }
    Ok(())
}

fn health_check_components(
    root: &Path,
    platform: &MediaToolchainPlatformManifest,
) -> ToolchainResult<()> {
    for component in &platform.components {
        let path = safe_join(root, &component.path)?;
        let mut child = Command::new(&path)
            .args(component.kind.health_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::HealthCheckFailed,
                    format!(
                        "Could not start media-toolchain {}",
                        component_name(&component.kind)
                    ),
                )
            })?;
        let status = wait_with_timeout(&mut child, HEALTH_CHECK_TIMEOUT).map_err(|timed_out| {
            if timed_out {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::HealthCheckFailed,
                    format!(
                        "Media-toolchain {} health check timed out",
                        component_name(&component.kind)
                    ),
                )
            } else {
                MediaToolchainError::new(
                    MediaToolchainErrorCode::HealthCheckFailed,
                    format!(
                        "Could not wait for media-toolchain {}",
                        component_name(&component.kind)
                    ),
                )
            }
        })?;
        if !status.success() {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::HealthCheckFailed,
                format!(
                    "Media-toolchain {} health check failed",
                    component_name(&component.kind)
                ),
            ));
        }
    }
    Ok(())
}

/// Returns `Ok(status)` on exit, `Err(true)` on timeout, `Err(false)` on wait I/O error.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, bool> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(true);
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => return Err(false),
        }
    }
}

fn activate_candidate(
    layout: &MediaToolchainLayout,
    version: &str,
    candidate: &Path,
    manifest_size_bytes: Option<u64>,
) -> ToolchainResult<()> {
    validate_version(version)?;
    let destination = layout.version_dir(version);
    if !candidate.is_dir() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            "Media-toolchain candidate is incomplete",
        ));
    }
    let mut state = read_install_state(layout)?;
    fs::create_dir_all(layout.versions_dir()).map_err(|error| {
        MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            format!("Create media-toolchain versions directory: {error}"),
        )
    })?;
    let replacement = candidate
        .parent()
        .ok_or_else(|| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ActivationFailed,
                "Media-toolchain candidate has no staging directory",
            )
        })?
        .join("replaced-version");

    if replacement.exists() {
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            "Media-toolchain staging directory is not fresh",
        ));
    }
    let replaced_existing = if destination.exists() {
        let metadata = fs::symlink_metadata(&destination).map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ActivationFailed,
                format!("Inspect existing media-toolchain version: {error}"),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(MediaToolchainError::new(
                MediaToolchainErrorCode::ActivationFailed,
                "Existing media-toolchain version is a symbolic link",
            ));
        }
        fs::rename(&destination, &replacement).map_err(|error| {
            MediaToolchainError::new(
                MediaToolchainErrorCode::ActivationFailed,
                format!("Stage existing media-toolchain version: {error}"),
            )
        })?;
        true
    } else {
        false
    };
    if let Err(error) = fs::rename(candidate, &destination) {
        if replaced_existing {
            let _ = fs::rename(&replacement, &destination);
        }
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            format!("Activate media-toolchain version: {error}"),
        ));
    }
    let old_active = state.active_version.clone();
    state.active_version = Some(version.to_string());
    if old_active.as_deref() != Some(version) {
        state.previous_version = old_active;
    }
    state.latest_available = Some(version.to_string());
    state.latest_manifest_size_bytes = manifest_size_bytes;
    state.last_checked_at = Some(Utc::now().to_rfc3339());
    if let Err(error) = write_install_state_atomic(layout, state) {
        let mut recovery_failures = Vec::new();
        if let Err(restore_error) = fs::rename(&destination, candidate) {
            recovery_failures.push(format!("restore candidate: {restore_error}"));
        }
        if replaced_existing {
            if let Err(restore_error) = fs::rename(&replacement, &destination) {
                recovery_failures.push(format!("restore previous version: {restore_error}"));
            }
        }
        let recovery_detail = if recovery_failures.is_empty() {
            String::new()
        } else {
            format!("; recovery also failed ({})", recovery_failures.join(", "))
        };
        return Err(MediaToolchainError::new(
            MediaToolchainErrorCode::ActivationFailed,
            format!(
                "Activate media-toolchain state: {}{}",
                error.message, recovery_detail
            ),
        ));
    }
    // Best-effort cleanup of the displaced prior same-version tree.
    if replaced_existing {
        let _ = fs::remove_dir_all(&replacement);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn sample_component(
        kind: MediaToolchainComponentKind,
        path: &str,
        bytes: &[u8],
    ) -> MediaToolchainComponentArtifact {
        MediaToolchainComponentArtifact {
            kind,
            path: path.to_string(),
            sha256: digest_hex(bytes),
            size_bytes: Some(bytes.len() as u64),
        }
    }

    fn unsigned_manifest() -> MediaToolchainManifest {
        MediaToolchainManifest {
            schema_version: 1,
            version: "2026.8.2".to_string(),
            platforms: vec![MediaToolchainPlatformManifest {
                target: MEDIA_TOOLCHAIN_V1_TARGET.to_string(),
                archive: MediaToolchainArchive {
                    url: "https://github.com/Howard-Starfield/Infield-media-toolchain/releases/latest/download/toolchain.zip".to_string(),
                    sha256: "a".repeat(64),
                    size_bytes: None,
                },
                components: vec![
                    sample_component(MediaToolchainComponentKind::YtDlp, "bin/yt-dlp.exe", b"yt"),
                    sample_component(
                        MediaToolchainComponentKind::Ffmpeg,
                        "bin/ffmpeg.exe",
                        b"ffmpeg",
                    ),
                    sample_component(MediaToolchainComponentKind::Deno, "bin/deno.exe", b"deno"),
                ],
            }],
            signature: MediaToolchainManifestSignature {
                algorithm: "ed25519".to_string(),
                key_id: "test-key".to_string(),
                value: String::new(),
            },
        }
    }

    #[test]
    fn canonical_signed_bytes_omit_signature_and_nulls_in_fixed_order() {
        let bytes =
            String::from_utf8(unsigned_manifest().canonical_signed_bytes().unwrap()).unwrap();
        assert!(!bytes.contains("\"signature\""));
        assert!(!bytes.contains("null"));
        assert!(bytes.starts_with(
            "{\"schema_version\":1,\"version\":\"2026.8.2\",\"platforms\":"
        ));
        assert!(bytes.contains("\"kind\":\"yt_dlp\""));
        assert!(bytes.contains("\"kind\":\"ffmpeg\""));
    }

    #[test]
    fn validate_relative_path_rejects_traversal() {
        for path in ["../deno", "/deno", "C:\\deno", "bin/../../deno", "./deno", ".."] {
            assert!(
                validate_relative_path(path).is_err(),
                "{path} should be rejected"
            );
        }
        assert!(validate_relative_path("bin/deno").is_ok());
        assert!(validate_relative_path("licenses/.notice").is_ok());
    }

    #[test]
    fn validate_version_accepts_semver_and_rejects_unsafe_names() {
        assert!(validate_version("1.2.3").is_ok());
        assert!(validate_version("2026.8.2").is_ok());
        assert!(validate_version("1.2.3-rc.1").is_ok());
        assert!(validate_version("../1.2.3").is_err());
        assert!(validate_version("1.2").is_err());
        assert!(validate_version("").is_err());
        assert!(validate_version(&"9".repeat(121)).is_err());
    }

    #[test]
    fn strict_parser_rejects_duplicate_keys() {
        let serialized = serde_json::to_string(&unsigned_manifest()).unwrap();
        let duplicate = serialized.replacen(
            "\"version\":\"2026.8.2\"",
            "\"version\":\"2026.8.1\",\"version\":\"2026.8.2\"",
            1,
        );
        assert_eq!(
            parse_media_toolchain_manifest(duplicate.as_bytes())
                .unwrap_err()
                .code,
            MediaToolchainErrorCode::ManifestInvalid
        );
    }

    #[test]
    fn production_trust_pin_matches_reviewed_key() {
        assert_eq!(TRUSTED_MANIFEST_KEYS.len(), 1);
        let trust = production_trust_store();
        let key = trust
            .keys
            .get("infield-ed25519-2026-08")
            .expect("production signing key should be available");
        assert_eq!(
            base64::engine::general_purpose::STANDARD.encode(key),
            "J3OZsOl/8QB98szTw8+/jZQRCbFyGYJgGAZ2ipfAnPc="
        );
    }

    #[test]
    fn same_origin_archive_resolution_rejects_cross_origin() {
        let manifest_url = MEDIA_TOOLCHAIN_MANIFEST_URL;
        assert!(resolve_archive_url_from_manifest(
            "https://evil.example/toolchain.zip",
            manifest_url
        )
        .is_err());
        let relative = resolve_archive_url_from_manifest("./toolchain.zip", manifest_url).unwrap();
        assert_eq!(
            relative.as_str(),
            "https://github.com/Howard-Starfield/Infield-media-toolchain/releases/latest/download/toolchain.zip"
        );
    }

    #[test]
    fn layout_uses_versions_and_state_under_root() {
        let layout = MediaToolchainLayout::for_root(PathBuf::from("C:/tmp/media-toolchain"));
        assert_eq!(
            layout.state_path(),
            PathBuf::from("C:/tmp/media-toolchain/state.json")
        );
        assert_eq!(
            layout.version_dir("1.0.0"),
            PathBuf::from("C:/tmp/media-toolchain/versions/1.0.0")
        );
    }
}
