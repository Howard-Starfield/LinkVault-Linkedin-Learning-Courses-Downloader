//! Backend-owned clipping root registration and verification.
//!
//! Download snapshot roots are selected from a persisted newspaper batch
//! destination. A marker binds the on-disk directory to its SQLite root ID so
//! a disconnected drive or a later path reuse cannot silently redirect a
//! clipping to unrelated storage.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};
use crate::cache::open_runtime;
use serde::{Deserialize, Serialize};

use super::clipping_assets::ClippingAssetLayout;
use super::clipping_models::{ClippingError, ClippingErrorCode, ClippingRoot, ClippingRootKind};
use super::clipping_repository::{self as repository, NewClippingRoot};
use super::naming;
use super::storage::{LEGACY_CLIPPING_ROOT_ID, LEGACY_CLIPPING_ROOT_LOCATOR};

pub const SNAPSHOT_DIRECTORY_NAME: &str = "Newspaper snapshots";
pub const INTERNAL_DIRECTORY_NAME: &str = ".linkvault";
pub const ROOT_MARKER_FILE_NAME: &str = "clipping-root-v1.json";

#[derive(Clone)]
pub struct ClippingRootRegistry {
    db_path: PathBuf,
    writer: DatabaseWriter,
    legacy_layout: ClippingAssetLayout,
    registration_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootMarker {
    schema_version: u32,
    root_id: String,
}

impl ClippingRootRegistry {
    pub fn new(
        db_path: PathBuf,
        writer: DatabaseWriter,
        legacy_layout: ClippingAssetLayout,
    ) -> Self {
        Self {
            db_path,
            writer,
            legacy_layout,
            registration_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn legacy_layout(&self) -> &ClippingAssetLayout {
        &self.legacy_layout
    }

    pub fn list(&self) -> Result<Vec<ClippingRoot>, ClippingError> {
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_all_roots(&connection)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))
    }

    /// Register `<destination>/Newspaper snapshots` for future clipping
    /// creation. Callers must source `destination` from the persisted batch,
    /// never from a frontend path payload.
    pub fn register_download_destination(
        &self,
        destination: &Path,
        now: i64,
    ) -> Result<ClippingRoot, ClippingError> {
        let _registration_guard = self
            .registration_lock
            .lock()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        let destination = existing_safe_directory(destination)?;
        let requested_root = destination.join(SNAPSHOT_DIRECTORY_NAME);
        let root_created = match requested_root.symlink_metadata() {
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&requested_root) {
                    Ok(()) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                    Err(_) => {
                        return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
                    }
                }
            }
            Err(_) => {
                return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
            }
        };
        let root = existing_safe_directory(&requested_root)?;
        let locator = root.to_string_lossy().into_owned();
        let locator_key = locator_key(&root);

        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        if let Some(existing) = repository::load_root_by_locator_key(&connection, &locator_key)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
        {
            if existing.kind != ClippingRootKind::DownloadSnapshot
                || verify_marker(&root, &existing.id).is_err()
            {
                return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
            }
            return Ok(existing);
        }
        drop(connection);

        let marker_path = marker_path(&root);
        let (root_id, marker_created) = match read_marker(&marker_path) {
            Ok(marker) if marker.schema_version == 1 && validate_root_id(&marker.root_id) => {
                (marker.root_id, false)
            }
            Ok(_) => return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let candidate_id = naming::unique_id("clipping-root");
                match write_new_marker(&root, &candidate_id) {
                    Ok(()) => (candidate_id, true),
                    Err(write_error) => match read_marker(&marker_path) {
                        Ok(marker)
                            if marker.schema_version == 1 && validate_root_id(&marker.root_id) =>
                        {
                            (marker.root_id, false)
                        }
                        _ => return Err(write_error),
                    },
                }
            }
            Err(_) => return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable)),
        };

        let new_root = NewClippingRoot {
            id: root_id.clone(),
            kind: ClippingRootKind::DownloadSnapshot,
            locator: locator.clone(),
            locator_key: locator_key.clone(),
            now,
        };
        let inserted = self.writer.execute(
            DatabaseWriteContext {
                operation: "clipping_register_download_root",
                provider: DatabaseProvider::Newspaper,
                workflow_id: None,
            },
            move |connection| repository::insert_root(connection, &new_root).map_err(Into::into),
        );
        if inserted.is_err() {
            let connection = open_runtime(&self.db_path)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            if let Some(existing) = repository::load_root_by_locator_key(&connection, &locator_key)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            {
                if existing.id == root_id && verify_marker(&root, &existing.id).is_ok() {
                    return Ok(existing);
                }
            }
            if marker_created {
                let _ = fs::remove_file(marker_path);
            }
            if root_created {
                let _ = fs::remove_dir(root.join(INTERNAL_DIRECTORY_NAME));
                let _ = fs::remove_dir(&root);
            }
            return Err(ClippingError::new(ClippingErrorCode::DatabaseWriteFailed));
        }

        Ok(ClippingRoot {
            id: root_id,
            kind: ClippingRootKind::DownloadSnapshot,
            locator,
            locator_key,
            created_at: now,
            updated_at: now,
        })
    }

    /// Resolve and verify an already-registered root. This path never creates
    /// a missing directory or marker, which keeps offline drives distinct from
    /// newly attached storage that happens to reuse the same path.
    pub fn resolve(&self, root_id: &str) -> Result<ClippingAssetLayout, ClippingError> {
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let root = repository::load_root_by_id(&connection, root_id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        match root.kind {
            ClippingRootKind::LegacyManaged => {
                if root.id != LEGACY_CLIPPING_ROOT_ID
                    || root.locator != LEGACY_CLIPPING_ROOT_LOCATOR
                {
                    return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
                }
                Ok(self.legacy_layout.clone())
            }
            ClippingRootKind::DownloadSnapshot => {
                let path = existing_safe_directory(Path::new(&root.locator))?;
                if locator_key(&path) != root.locator_key {
                    return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
                }
                verify_marker(&path, &root.id)?;
                Ok(ClippingAssetLayout::new_existing(path))
            }
        }
    }

    pub fn resolve_for_creation(
        &self,
        root_id: &str,
    ) -> Result<ClippingAssetLayout, ClippingError> {
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let root = repository::load_root_by_id(&connection, root_id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        if !root.kind.accepts_new_clippings() {
            return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
        }
        drop(connection);
        self.resolve(root_id)
    }
}

fn validate_root_id(value: &str) -> bool {
    value.starts_with("clipping-root-")
        && value.len() <= 96
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn marker_path(root: &Path) -> PathBuf {
    root.join(INTERNAL_DIRECTORY_NAME)
        .join(ROOT_MARKER_FILE_NAME)
}

fn write_new_marker(root: &Path, root_id: &str) -> Result<(), ClippingError> {
    let internal = root.join(INTERNAL_DIRECTORY_NAME);
    match internal.symlink_metadata() {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::create_dir(&internal) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => {
                    return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
                }
            }
        }
        Err(_) => {
            return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
        }
    }
    let internal = existing_safe_directory(&internal)?;
    let payload = serde_json::to_vec(&RootMarker {
        schema_version: 1,
        root_id: root_id.to_owned(),
    })
    .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
    let marker = internal.join(ROOT_MARKER_FILE_NAME);
    let part = internal.join(format!("{ROOT_MARKER_FILE_NAME}.{root_id}.part"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
    file.write_all(&payload)
        .and_then(|_| file.sync_all())
        .map_err(|_| {
            let _ = fs::remove_file(&part);
            ClippingError::new(ClippingErrorCode::AssetRootUnavailable)
        })?;
    drop(file);
    fs::rename(&part, &marker).map_err(|_| {
        let _ = fs::remove_file(&part);
        ClippingError::new(ClippingErrorCode::AssetRootUnavailable)
    })
}

fn read_marker(path: &Path) -> std::io::Result<RootMarker> {
    let metadata = fs::symlink_metadata(path)?;
    if is_symlink_or_reparse(&metadata)
        || !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > 4096
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid clipping root marker",
        ));
    }
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn verify_marker(root: &Path, expected_id: &str) -> Result<(), ClippingError> {
    let marker = read_marker(&marker_path(root))
        .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
    if marker.schema_version != 1 || marker.root_id != expected_id {
        return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
    }
    Ok(())
}

fn locator_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn existing_safe_directory(path: &Path) -> Result<PathBuf, ClippingError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_dir() {
        return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
    }
    path.canonicalize()
        .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))
}

#[cfg(windows)]
fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::initialize_database;
    use crate::app::database_diagnostics::DatabaseDiagnostics;

    fn fixture() -> (
        tempfile::TempDir,
        ClippingRootRegistry,
        DatabaseWriter,
        PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let diagnostics = DatabaseDiagnostics::default();
        let writer = DatabaseWriter::start(db_path.clone(), diagnostics).unwrap();
        let legacy = ClippingAssetLayout::new(temp.path().join("legacy-clippings"));
        let registry = ClippingRootRegistry::new(db_path, writer.clone(), legacy);
        let destination = temp.path().join("downloads");
        fs::create_dir(&destination).unwrap();
        (temp, registry, writer, destination)
    }

    #[test]
    fn clipping_root_registration_is_idempotent_and_marker_bound() {
        let (_temp, registry, writer, destination) = fixture();
        let first = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        let second = registry
            .register_download_destination(&destination, 200)
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.kind, ClippingRootKind::DownloadSnapshot);
        assert!(first.locator.ends_with(SNAPSHOT_DIRECTORY_NAME));
        let layout = registry.resolve(&first.id).unwrap();
        assert_eq!(
            layout.root().canonicalize().unwrap(),
            PathBuf::from(first.locator)
        );
        writer.shutdown().unwrap();
    }

    #[test]
    fn concurrent_registration_adopts_one_marker_and_root_row() {
        let (_temp, registry, writer, destination) = fixture();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let callers: Vec<_> = (0..8)
            .map(|index| {
                let registry = registry.clone();
                let destination = destination.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.register_download_destination(&destination, 100 + index)
                })
            })
            .collect();
        let roots: Vec<_> = callers
            .into_iter()
            .map(|caller| caller.join().unwrap().unwrap())
            .collect();
        assert!(roots.iter().all(|root| root.id == roots[0].id));
        assert_eq!(registry.list().unwrap().len(), 2);
        writer.shutdown().unwrap();
    }

    #[test]
    fn clipping_root_resolve_does_not_recreate_offline_or_reused_path() {
        let (_temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        let path = PathBuf::from(&root.locator);
        fs::remove_dir_all(&path).unwrap();
        assert_eq!(
            registry.resolve(&root.id).unwrap_err().code,
            ClippingErrorCode::AssetRootUnavailable
        );
        assert!(!path.exists());

        fs::create_dir(&path).unwrap();
        assert_eq!(
            registry.resolve(&root.id).unwrap_err().code,
            ClippingErrorCode::AssetRootUnavailable
        );
        assert!(!marker_path(&path).exists());
        assert_eq!(
            registry
                .register_download_destination(&destination, 200)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetRootUnavailable
        );
        writer.shutdown().unwrap();
    }

    #[test]
    fn clipping_root_resolve_rejects_marker_substitution() {
        let (_temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        let path = PathBuf::from(&root.locator);
        fs::write(
            marker_path(&path),
            br#"{"schema_version":1,"root_id":"clipping-root-forged-0"}"#,
        )
        .unwrap();
        assert_eq!(
            registry.resolve(&root.id).unwrap_err().code,
            ClippingErrorCode::AssetRootUnavailable
        );
        writer.shutdown().unwrap();
    }
}
