//! Managed clipping asset layout, staging, promotion, containment, and
//! validation (ADR-002 asset storage, specification 02 sections 7 and 9,
//! FR-ASSET-001..003).
//!
//! All canonical paths are backend-derived from validated clipping IDs.
//! React never supplies a destination or relative path, and no managed
//! operation follows a symlink out of the clipping root.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::clipping_models::{
    validate_asset_byte_count, validate_clipping_id, ClippingError, ClippingErrorCode,
    CLIPPING_ASSET_MIME,
};

/// Canonical clipping asset file name (FR-ASSET-001).
pub const CANONICAL_FILE_NAME: &str = "clipping-v1.webp";
/// Staging suffix while a canonical asset is still being written.
pub const STAGING_PART_SUFFIX: &str = ".part";
/// Canonical relative path template (FR-ASSET-001).
pub const CANONICAL_RELATIVE_TEMPLATE: &str = "assets/<clipping-id>/clipping-v1.webp";
/// Cache schema version for derived clipping thumbnails (D-020).
pub const THUMBNAIL_CACHE_SCHEMA_VERSION: u32 = 1;

/// Directory names beneath the managed clipping root.
pub const ASSETS_DIR: &str = "assets";
pub const THUMBNAILS_DIR: &str = "thumbnails";
pub const STAGING_DIR: &str = "staging";
pub const TRASH_DIR: &str = "trash";
pub const QUARANTINE_DIR: &str = "quarantine";
pub const THUMBNAIL_VERSION_DIR: &str = "v1";

/// The managed clipping root layout beneath `LinkVaultData`.
#[derive(Clone, Debug)]
pub struct ClippingAssetLayout {
    root: PathBuf,
}

impl ClippingAssetLayout {
    /// Adopt a root directory. The caller resolves the production root via
    /// `app::storage::resolve_newspaper_clippings_root`; tests inject
    /// temporary directories.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn ensure_root(&self) -> Result<PathBuf, ClippingError> {
        if let Ok(metadata) = fs::symlink_metadata(&self.root) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
            }
        }
        fs::create_dir_all(&self.root)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        self.root
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))
    }

    fn ensure_dir(&self, relative: &str) -> Result<PathBuf, ClippingError> {
        let root = self.ensure_root()?;
        let dir = root.join(relative);
        if let Ok(metadata) = fs::symlink_metadata(&dir) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
            }
        }
        fs::create_dir_all(&dir)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        let canonical = dir
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        if !canonical.starts_with(&root) {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        Ok(canonical)
    }

    pub fn assets_dir(&self) -> Result<PathBuf, ClippingError> {
        self.ensure_dir(ASSETS_DIR)
    }

    pub fn staging_dir(&self) -> Result<PathBuf, ClippingError> {
        self.ensure_dir(STAGING_DIR)
    }

    pub fn trash_dir(&self) -> Result<PathBuf, ClippingError> {
        self.ensure_dir(TRASH_DIR)
    }

    pub fn quarantine_dir(&self) -> Result<PathBuf, ClippingError> {
        self.ensure_dir(QUARANTINE_DIR)
    }

    pub fn thumbnails_dir(&self) -> Result<PathBuf, ClippingError> {
        self.ensure_dir(&format!("{THUMBNAILS_DIR}/{THUMBNAIL_VERSION_DIR}"))
    }

    /// The canonical relative path stored in SQLite (FR-ASSET-001). React
    /// cannot override any part of it.
    pub fn canonical_relative_path(clipping_id: &str) -> Result<String, ClippingError> {
        if !validate_clipping_id(clipping_id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        Ok(format!("assets/{clipping_id}/{CANONICAL_FILE_NAME}"))
    }

    /// Validate a database-stored relative path before any filesystem use.
    /// Rejects absolute paths, parent components, and anything that is not
    /// the exact canonical shape (FR-ASSET-002).
    pub fn validate_relative_path(relative: &str) -> Result<(), ClippingError> {
        let invalid = || ClippingError::new(ClippingErrorCode::AssetPathInvalid);
        if relative.is_empty() {
            return Err(invalid());
        }
        let path = Path::new(relative);
        if path.is_absolute() {
            return Err(invalid());
        }
        let mut components = path.components();
        let first = components.next().ok_or_else(invalid)?;
        let Component::Normal(first) = first else {
            return Err(invalid());
        };
        if first != std::ffi::OsStr::new(ASSETS_DIR) {
            return Err(invalid());
        }
        let Some(Component::Normal(id)) = components.next() else {
            return Err(invalid());
        };
        let Some(id) = id.to_str() else {
            return Err(invalid());
        };
        if !validate_clipping_id(id) {
            return Err(invalid());
        }
        let Some(file) = components.next() else {
            return Err(invalid());
        };
        if components.next().is_some() {
            return Err(invalid());
        }
        let Component::Normal(file) = file else {
            return Err(invalid());
        };
        if file != std::ffi::OsStr::new(CANONICAL_FILE_NAME) {
            return Err(invalid());
        }
        Ok(())
    }

    fn canonical_dir_for(&self, clipping_id: &str) -> Result<PathBuf, ClippingError> {
        if !validate_clipping_id(clipping_id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        Ok(self.assets_dir()?.join(clipping_id))
    }

    /// The absolute canonical asset path for a validated clipping ID.
    pub fn canonical_path(&self, clipping_id: &str) -> Result<PathBuf, ClippingError> {
        Ok(self
            .canonical_dir_for(clipping_id)?
            .join(CANONICAL_FILE_NAME))
    }

    fn staging_dir_for(&self, clipping_id: &str) -> Result<PathBuf, ClippingError> {
        if !validate_clipping_id(clipping_id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        Ok(self.staging_dir()?.join(clipping_id))
    }

    pub fn staging_part_path(&self, clipping_id: &str) -> Result<PathBuf, ClippingError> {
        Ok(self
            .staging_dir_for(clipping_id)?
            .join(format!("{CANONICAL_FILE_NAME}{STAGING_PART_SUFFIX}")))
    }

    pub fn staging_complete_path(&self, clipping_id: &str) -> Result<PathBuf, ClippingError> {
        Ok(self.staging_dir_for(clipping_id)?.join(CANONICAL_FILE_NAME))
    }

    /// Deterministic thumbnail cache file name (D-020). Absence is not an
    /// error state; thumbnails are regenerable cache data.
    pub fn thumbnail_path(&self, clipping_id: &str) -> Result<PathBuf, ClippingError> {
        if !validate_clipping_id(clipping_id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        Ok(self
            .thumbnails_dir()?
            .join(format!("{clipping_id}-asset-1.webp")))
    }

    /// Write canonical bytes to staging using create-new semantics, then
    /// finalize the staging name (CREATE-STATE-001 steps 3-4, FR-ASSET-003).
    pub fn write_staging(&self, clipping_id: &str, bytes: &[u8]) -> Result<(), ClippingError> {
        self.write_staging_inner(clipping_id, bytes, |_| {})
    }

    fn write_staging_inner<F>(
        &self,
        clipping_id: &str,
        bytes: &[u8],
        after_create: F,
    ) -> Result<(), ClippingError>
    where
        F: FnOnce(&Path),
    {
        if !validate_asset_byte_count(bytes.len() as u64) {
            return Err(ClippingError::new(ClippingErrorCode::AssetValidationFailed));
        }
        let staging_dir = self.staging_dir_for(clipping_id)?;
        if self
            .canonical_dir_for(clipping_id)?
            .symlink_metadata()
            .is_ok()
        {
            return Err(ClippingError::new(ClippingErrorCode::AssetCollision));
        }
        if staging_dir.symlink_metadata().is_ok() {
            // A pre-existing staging directory for this ID is a collision or
            // recovery condition, never an overwrite target.
            return Err(ClippingError::new(ClippingErrorCode::AssetCollision));
        }
        fs::create_dir_all(&staging_dir)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetWriteFailed))?;
        after_create(&staging_dir);

        // Revalidate after creation so a staging-directory substitution cannot
        // redirect the create-new file open through a symlink or reparse point.
        let metadata = fs::symlink_metadata(&staging_dir)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetPathInvalid))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        let staging_root = self.staging_dir()?;
        let canonical_staging_dir = staging_dir
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetPathInvalid))?;
        if !canonical_staging_dir.starts_with(&staging_root) {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }

        let part_path = staging_dir.join(format!("{CANONICAL_FILE_NAME}{STAGING_PART_SUFFIX}"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetWriteFailed))?;
        file.write_all(bytes)
            .and_then(|_| file.flush())
            .map_err(|_| {
                let _ = fs::remove_file(&part_path);
                ClippingError::new(ClippingErrorCode::AssetWriteFailed)
            })?;
        drop(file);
        let complete_path = staging_dir.join(CANONICAL_FILE_NAME);
        fs::rename(&part_path, &complete_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetWriteFailed))?;
        Ok(())
    }

    /// Remove only the current operation's staging directory when a failure
    /// is safe to clean (CREATE crash table: DB insert failure, validation
    /// failure before row registration).
    pub fn discard_staging(&self, clipping_id: &str) {
        if let Ok(dir) = self.staging_dir_for(clipping_id) {
            if let Ok(metadata) = fs::symlink_metadata(&dir) {
                if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                    if let (Ok(canonical), Ok(staging_root)) =
                        (dir.canonicalize(), self.staging_dir())
                    {
                        if canonical.starts_with(staging_root) {
                            let _ = fs::remove_dir_all(canonical);
                        }
                    }
                }
            }
        }
    }

    /// Atomically promote `staging/<id>` to `assets/<id>` on the same volume
    /// (CREATE-STATE-003, FR-ASSET-003). An unexpected existing canonical
    /// directory is a collision, not an overwrite.
    pub fn promote_staging(&self, clipping_id: &str) -> Result<(), ClippingError> {
        let staging_dir = self.staging_dir_for(clipping_id)?;
        let canonical_dir = self.canonical_dir_for(clipping_id)?;
        let staging_metadata = staging_dir
            .symlink_metadata()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetPromotionFailed))?;
        if staging_metadata.file_type().is_symlink() || !staging_metadata.file_type().is_dir() {
            return Err(ClippingError::new(ClippingErrorCode::AssetPromotionFailed));
        }
        let staging_root = self.staging_dir()?;
        let canonical_staging_dir = staging_dir
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetPromotionFailed))?;
        if !canonical_staging_dir.starts_with(&staging_root) {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        if canonical_dir.symlink_metadata().is_ok() {
            return Err(ClippingError::new(ClippingErrorCode::AssetCollision));
        }
        if let Some(parent) = canonical_dir.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| ClippingError::new(ClippingErrorCode::AssetPromotionFailed))?;
        }
        fs::rename(&canonical_staging_dir, &canonical_dir)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetPromotionFailed))?;
        Ok(())
    }

    /// Verify a canonical asset in place: contained regular non-symlink file,
    /// expected byte count, decoded dimensions, and SHA-256 (aggregate
    /// invariant for `asset_state = ready`, CREATE-STATE-003 post-check).
    pub fn verify_canonical(
        &self,
        clipping_id: &str,
        expected_byte_count: u64,
        expected_width: u32,
        expected_height: u32,
        expected_sha256: &str,
    ) -> Result<(), ClippingError> {
        let path = self.contained_regular_file(&self.canonical_path(clipping_id)?)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        if metadata.len() != expected_byte_count {
            return Err(ClippingError::new(ClippingErrorCode::AssetValidationFailed));
        }
        let bytes =
            fs::read(&path).map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        Self::validate_canonical_bytes(
            &bytes,
            expected_byte_count,
            expected_width,
            expected_height,
            expected_sha256,
        )
    }

    pub fn verify_staging(
        &self,
        clipping_id: &str,
        expected_byte_count: u64,
        expected_width: u32,
        expected_height: u32,
        expected_sha256: &str,
    ) -> Result<(), ClippingError> {
        let path = self.contained_regular_file(&self.staging_complete_path(clipping_id)?)?;
        let bytes =
            fs::read(path).map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        Self::validate_canonical_bytes(
            &bytes,
            expected_byte_count,
            expected_width,
            expected_height,
            expected_sha256,
        )
    }

    /// Validate canonical bytes before row registration or promotion
    /// (CREATE-STATE-001 steps 5-6, D-008 decode-and-verify contract).
    pub fn validate_canonical_bytes(
        bytes: &[u8],
        expected_byte_count: u64,
        expected_width: u32,
        expected_height: u32,
        expected_sha256: &str,
    ) -> Result<(), ClippingError> {
        let invalid = || ClippingError::new(ClippingErrorCode::AssetValidationFailed);
        if !validate_asset_byte_count(bytes.len() as u64) {
            return Err(invalid());
        }
        if bytes.len() as u64 != expected_byte_count {
            return Err(invalid());
        }
        if !is_webp_container(bytes) {
            return Err(invalid());
        }
        let decoded = webp::Decoder::new(bytes).decode().ok_or_else(invalid)?;
        if decoded.width() != expected_width || decoded.height() != expected_height {
            return Err(invalid());
        }
        if sha256_hex(bytes) != expected_sha256 {
            return Err(ClippingError::new(ClippingErrorCode::AssetChecksumMismatch));
        }
        Ok(())
    }

    /// Resolve a candidate path, requiring an existing regular non-symlink
    /// file contained inside the managed root (FR-ASSET-002).
    pub fn contained_regular_file(&self, candidate: &Path) -> Result<PathBuf, ClippingError> {
        let root = self.ensure_root()?;
        let metadata = fs::symlink_metadata(candidate)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        let canonical = candidate
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetPathInvalid))?;
        if !canonical.starts_with(&root) {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        Ok(canonical)
    }

    /// Move a canonical asset directory into `trash/<id>-<nonce>`
    /// (DELETE-STATE-002). Missing assets are not an error: the note must
    /// remain deletable after explicit confirmation.
    pub fn move_canonical_to_trash(
        &self,
        clipping_id: &str,
        nonce: u128,
    ) -> Result<Option<PathBuf>, ClippingError> {
        let canonical_dir = self.canonical_dir_for(clipping_id)?;
        let metadata = match canonical_dir.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ClippingError::new(ClippingErrorCode::DeleteFailed)),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        let trash_root = self.trash_dir()?;
        let target = trash_root.join(format!("{clipping_id}-{nonce}"));
        if target.symlink_metadata().is_ok() {
            return Err(ClippingError::new(ClippingErrorCode::AssetCollision));
        }
        fs::rename(&canonical_dir, &target)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DeleteFailed))?;
        Ok(Some(target))
    }

    /// Remove a trash entry after its row deletion committed
    /// (DELETE-STATE-004). Failure is recorded by the caller for startup
    /// cleanup and never recreates the clipping.
    pub fn remove_trash_entry(&self, entry: &Path) -> Result<(), ClippingError> {
        let root = self.ensure_root()?;
        let canonical = entry
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::DeleteFailed))?;
        let trash_root = self.trash_dir()?;
        if !canonical.starts_with(&trash_root) || !canonical.starts_with(&root) {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        fs::remove_dir_all(&canonical)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DeleteFailed))
    }

    /// Move an orphaned managed directory into quarantine instead of
    /// immediate deletion (RECOVERY-004).
    pub fn quarantine_directory(
        &self,
        source_dir: &Path,
        reason: &str,
        timestamp: i64,
    ) -> Result<(), ClippingError> {
        let root = self.ensure_root()?;
        let canonical_source = source_dir
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
        if !canonical_source.starts_with(&root) {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        let quarantine_root = self.quarantine_dir()?;
        let name = source_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown");
        let safe_reason: String = reason
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        let target = quarantine_root.join(format!("{timestamp}-{safe_reason}-{name}"));
        if target.symlink_metadata().is_ok() {
            return Err(ClippingError::new(ClippingErrorCode::AssetCollision));
        }
        fs::rename(&canonical_source, &target)
            .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
        Ok(())
    }

    /// Remove a derived thumbnail cache file when present. Thumbnails are
    /// cache data; absence is success (D-020).
    pub fn remove_thumbnail(&self, clipping_id: &str) {
        if let Ok(path) = self.thumbnail_path(clipping_id) {
            if let Ok(contained) = self.contained_regular_file(&path) {
                let _ = fs::remove_file(contained);
            }
        }
    }

    pub fn remove_quarantine_entry(&self, entry: &Path) -> Result<(), ClippingError> {
        let metadata = fs::symlink_metadata(entry)
            .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        let canonical = entry
            .canonicalize()
            .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
        let quarantine_root = self.quarantine_dir()?;
        if !canonical.starts_with(&quarantine_root) {
            return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
        }
        fs::remove_dir_all(canonical)
            .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))
    }

    /// Serve-side validation for the media protocol (FR-MEDIA-001): contained
    /// regular non-symlink file with supported MIME and non-empty bytes.
    /// Returns the bytes only after every check passes.
    pub fn read_canonical_for_protocol(
        &self,
        clipping_id: &str,
    ) -> Result<(Vec<u8>, &'static str), ClippingError> {
        let path = self.contained_regular_file(&self.canonical_path(clipping_id)?)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        if metadata.len() == 0 {
            return Err(ClippingError::new(ClippingErrorCode::AssetMissing));
        }
        let bytes =
            fs::read(&path).map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        if bytes.is_empty() || !is_webp_container(&bytes) {
            return Err(ClippingError::new(ClippingErrorCode::AssetValidationFailed));
        }
        Ok((bytes, CLIPPING_ASSET_MIME))
    }

    /// Serve-side validation for derived thumbnails (FR-MEDIA-002).
    pub fn read_thumbnail_for_protocol(
        &self,
        clipping_id: &str,
    ) -> Result<(Vec<u8>, &'static str), ClippingError> {
        let path = self.contained_regular_file(&self.thumbnail_path(clipping_id)?)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        if metadata.len() == 0 {
            return Err(ClippingError::new(ClippingErrorCode::AssetMissing));
        }
        let bytes =
            fs::read(&path).map_err(|_| ClippingError::new(ClippingErrorCode::AssetMissing))?;
        if bytes.is_empty() || !is_webp_container(&bytes) {
            return Err(ClippingError::new(ClippingErrorCode::AssetMissing));
        }
        Ok((bytes, CLIPPING_ASSET_MIME))
    }
}

/// SHA-256 over canonical bytes, lowercase hex (D-008).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .fold(String::with_capacity(64), |mut acc, byte| {
            acc.push_str(&format!("{byte:02x}"));
            acc
        })
}

/// Sniff the RIFF/WEBP container header. This is a MIME safety check, not a
/// decode; decoding happens separately through the image pipeline.
pub fn is_webp_container(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Generate a lossless WebP test fixture of the given pixel size
/// (generated bytes only; no copyrighted newspaper material ever enters the
/// repository).
#[cfg(test)]
pub(crate) fn encode_test_webp(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                (x % 256) as u8,
                (y % 256) as u8,
                ((x + y) % 256) as u8,
                255,
            ]);
        }
    }
    let encoder = webp::Encoder::from_rgba(&pixels, width, height);
    encoder.encode_lossless().to_vec()
}

#[cfg(test)]
mod tests {
    use super::encode_test_webp;
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const OTHER_ID: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

    fn temp_layout() -> (tempfile::TempDir, ClippingAssetLayout) {
        let directory = tempfile::tempdir().unwrap();
        let layout = ClippingAssetLayout::new(directory.path().join("newspaper-clippings"));
        (directory, layout)
    }

    fn valid_fixture() -> (Vec<u8>, u64, String) {
        let bytes = encode_test_webp(24, 16);
        let checksum = sha256_hex(&bytes);
        let len = bytes.len() as u64;
        (bytes, len, checksum)
    }

    #[test]
    fn canonical_relative_path_is_backend_derived_and_exact() {
        assert_eq!(
            ClippingAssetLayout::canonical_relative_path(TEST_ID).unwrap(),
            format!("assets/{TEST_ID}/clipping-v1.webp")
        );
        assert!(ClippingAssetLayout::canonical_relative_path("not-a-uuid").is_err());
    }

    #[test]
    fn relative_path_validation_rejects_absolute_parent_and_malformed_paths() {
        for candidate in [
            "",
            "C:\\outside\\file.webp",
            "/etc/passwd",
            "../assets/x/clipping-v1.webp",
            "assets/../assets/x/clipping-v1.webp",
            "assets/../../secret.webp",
            "thumbnails/v1/x.webp",
            &format!("assets/{TEST_ID}/other.webp"),
            &format!("assets/{TEST_ID}/sub/clipping-v1.webp"),
            &format!("assets/{OTHER_ID}"),
        ] {
            assert!(
                ClippingAssetLayout::validate_relative_path(candidate).is_err(),
                "accepted {candidate:?}"
            );
        }
        assert!(ClippingAssetLayout::validate_relative_path(&format!(
            "assets/{TEST_ID}/clipping-v1.webp"
        ))
        .is_ok());
    }

    #[test]
    fn staging_write_promote_and_verify_round_trip() {
        let (_directory, layout) = temp_layout();
        let (bytes, len, checksum) = valid_fixture();

        layout.write_staging(TEST_ID, &bytes).unwrap();
        assert!(layout.staging_complete_path(TEST_ID).unwrap().is_file());
        assert!(!layout.staging_part_path(TEST_ID).unwrap().exists());

        // A second staging write for the same ID collides instead of
        // overwriting (FR-ASSET-003).
        assert_eq!(
            layout.write_staging(TEST_ID, &bytes).unwrap_err().code,
            ClippingErrorCode::AssetCollision
        );

        layout.promote_staging(TEST_ID).unwrap();
        assert!(layout.canonical_path(TEST_ID).unwrap().is_file());
        assert!(!layout.staging_dir_for(TEST_ID).unwrap().exists());
        layout
            .verify_canonical(TEST_ID, len, 24, 16, &checksum)
            .unwrap();

        // Promoting again without staging fails typed; the canonical file is
        // never modified in place.
        assert_eq!(
            layout.promote_staging(TEST_ID).unwrap_err().code,
            ClippingErrorCode::AssetPromotionFailed
        );
        layout
            .verify_canonical(TEST_ID, len, 24, 16, &checksum)
            .unwrap();
    }

    #[test]
    fn staging_write_revalidates_containment_after_directory_creation() {
        use std::cell::Cell;

        let (directory, layout) = temp_layout();
        let outside = directory.path().join("outside-staging");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();
        let (bytes, _, _) = valid_fixture();
        let linked = Cell::new(false);

        let result = layout.write_staging_inner(TEST_ID, &bytes, |created| {
            fs::remove_dir(created).unwrap();
            if create_dir_link(&outside, created) {
                linked.set(true);
            } else {
                // The platform did not permit a directory link. Restore the
                // ordinary directory so the helper can complete safely.
                fs::create_dir(created).unwrap();
            }
        });

        if !linked.get() {
            layout.discard_staging(TEST_ID);
            eprintln!("directory link creation unavailable on this machine");
            return;
        }
        assert_eq!(
            result
                .expect_err("post-create directory escape must be rejected")
                .code,
            ClippingErrorCode::AssetPathInvalid
        );
        assert!(!outside
            .join(format!("{CANONICAL_FILE_NAME}{STAGING_PART_SUFFIX}"))
            .exists());
        assert_eq!(fs::read(outside.join("sentinel.txt")).unwrap(), b"keep-me");
    }

    #[test]
    fn promotion_refuses_to_overwrite_an_existing_canonical_directory() {
        let (_directory, layout) = temp_layout();
        let (bytes, _, _) = valid_fixture();
        layout.write_staging(TEST_ID, &bytes).unwrap();
        layout.promote_staging(TEST_ID).unwrap();
        layout.write_staging(TEST_ID, &bytes).unwrap_err(); // collision first
        layout.discard_staging(TEST_ID);
        layout.write_staging(OTHER_ID, &bytes).unwrap();
        // Force a canonical collision by pre-creating the target directory.
        let target = layout.canonical_dir_for(OTHER_ID).unwrap();
        fs::create_dir_all(&target).unwrap();
        assert_eq!(
            layout.promote_staging(OTHER_ID).unwrap_err().code,
            ClippingErrorCode::AssetCollision
        );
    }

    #[test]
    fn canonical_validation_rejects_wrong_dimensions_checksum_and_non_webp() {
        let (bytes, len, checksum) = valid_fixture();
        assert_eq!(
            ClippingAssetLayout::validate_canonical_bytes(&bytes, len, 999, 16, &checksum)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetValidationFailed
        );
        assert_eq!(
            ClippingAssetLayout::validate_canonical_bytes(&bytes, len, 24, 16, &"0".repeat(64))
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetChecksumMismatch
        );
        assert_eq!(
            ClippingAssetLayout::validate_canonical_bytes(
                b"RIFFxxxxWEBPvp8 ",
                16,
                24,
                16,
                &checksum
            )
            .unwrap_err()
            .code,
            ClippingErrorCode::AssetValidationFailed
        );
        assert_eq!(
            ClippingAssetLayout::validate_canonical_bytes(&bytes, len + 1, 24, 16, &checksum)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetValidationFailed
        );
    }

    #[test]
    fn trash_move_and_cleanup_remove_only_managed_entries() {
        let (directory, layout) = temp_layout();
        let outside = directory.path().join("outside-sentinel");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();

        let (bytes, _len, _checksum) = valid_fixture();
        layout.write_staging(TEST_ID, &bytes).unwrap();
        layout.promote_staging(TEST_ID).unwrap();

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let trash_entry = layout
            .move_canonical_to_trash(TEST_ID, nonce)
            .unwrap()
            .expect("canonical directory existed");
        assert!(!layout.canonical_path(TEST_ID).unwrap().exists());
        assert!(trash_entry.join(CANONICAL_FILE_NAME).is_file());

        // Missing assets are not an error during confirmed deletion.
        assert!(layout
            .move_canonical_to_trash(TEST_ID, nonce + 1)
            .unwrap()
            .is_none());

        layout.remove_trash_entry(&trash_entry).unwrap();
        assert!(!trash_entry.exists());

        // The sentinel outside the managed root was never touched.
        assert_eq!(fs::read(outside.join("sentinel.txt")).unwrap(), b"keep-me");
    }

    #[test]
    fn symlinked_canonical_asset_is_rejected_without_reading_outside_bytes() {
        let (directory, layout) = temp_layout();
        let outside = directory.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.webp");
        fs::write(&secret, b"outside-bytes").unwrap();

        let canonical_dir = layout.canonical_dir_for(TEST_ID).unwrap();
        fs::create_dir_all(canonical_dir.parent().unwrap()).unwrap();
        let link = layout.canonical_path(TEST_ID).unwrap();
        let linked = create_file_link(&secret, &link);
        if !linked {
            // Symlink creation requires Windows Developer Mode or Unix
            // permissions; the containment and metadata checks below still
            // prove the escape is rejected for directory links.
            eprintln!("symlink creation unavailable; skipping file symlink case");
            return;
        }
        let error = layout
            .read_canonical_for_protocol(TEST_ID)
            .expect_err("symlinked asset must be rejected");
        assert!(matches!(
            error.code,
            ClippingErrorCode::AssetPathInvalid | ClippingErrorCode::AssetMissing
        ));
        assert_eq!(fs::read(&secret).unwrap(), b"outside-bytes");
    }

    #[test]
    fn directory_escape_through_reparse_point_is_contained() {
        let (directory, layout) = temp_layout();
        let outside = directory.path().join("outside-dir");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();

        let assets_dir = layout.assets_dir().unwrap();
        let link_path = assets_dir.join(OTHER_ID);
        let linked = create_dir_link(&outside, &link_path);
        if !linked {
            eprintln!("directory link creation unavailable on this machine");
            return;
        }
        let candidate = link_path.join("sentinel.txt");
        let error = layout
            .contained_regular_file(&candidate)
            .expect_err("paths escaping the root must be rejected");
        assert_eq!(error.code, ClippingErrorCode::AssetPathInvalid);
        assert_eq!(fs::read(outside.join("sentinel.txt")).unwrap(), b"keep-me");
    }

    #[test]
    fn protocol_reads_reject_missing_empty_and_directory_targets() {
        let (_directory, layout) = temp_layout();
        assert_eq!(
            layout
                .read_canonical_for_protocol(TEST_ID)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetMissing
        );

        let (bytes, _, _) = valid_fixture();
        layout.write_staging(TEST_ID, &bytes).unwrap();
        layout.promote_staging(TEST_ID).unwrap();
        let (served, mime) = layout.read_canonical_for_protocol(TEST_ID).unwrap();
        assert_eq!(served, bytes);
        assert_eq!(mime, CLIPPING_ASSET_MIME);

        // Replace the canonical file with an empty file: serve fails safe.
        let canonical = layout.canonical_path(TEST_ID).unwrap();
        fs::write(&canonical, []).unwrap();
        assert!(layout.read_canonical_for_protocol(TEST_ID).is_err());
    }

    #[test]
    fn thumbnail_paths_are_deterministic_and_cache_scoped() {
        let (_directory, layout) = temp_layout();
        let path = layout.thumbnail_path(TEST_ID).unwrap();
        assert!(path.ends_with(format!("thumbnails/v1/{TEST_ID}-asset-1.webp")));
        // Absence is not an error for cache data.
        layout.remove_thumbnail(TEST_ID);
        assert_eq!(
            layout
                .read_thumbnail_for_protocol(TEST_ID)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetMissing
        );
    }

    #[test]
    fn quarantine_moves_only_contained_directories() {
        let (directory, layout) = temp_layout();
        let staging = layout.staging_dir().unwrap();
        let orphan = staging.join(OTHER_ID);
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join(CANONICAL_FILE_NAME), b"orphan").unwrap();

        layout
            .quarantine_directory(&orphan, "stale staging", 1_700_000_000)
            .unwrap();
        assert!(!orphan.exists());
        let quarantine = layout.quarantine_dir().unwrap();
        let moved: Vec<_> = fs::read_dir(&quarantine)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(moved.len(), 1);
        assert!(moved[0]
            .file_name()
            .to_string_lossy()
            .starts_with("1700000000-stale-staging-"));

        // Quarantine never reaches outside the managed root.
        let outside = directory.path().join("outside-quarantine");
        fs::create_dir_all(&outside).unwrap();
        assert!(layout.quarantine_directory(&outside, "x", 1).is_err());
    }

    /// Create a file symlink/junction-style link in a platform-appropriate
    /// way. Returns false when the platform denies the privilege.
    fn create_file_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_file(target, link).is_ok() {
                return true;
            }
            // Fall back to a hardlink-free junction-style attempt via mklink.
            std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
    }

    fn create_dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return true;
            }
            std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
    }
}
