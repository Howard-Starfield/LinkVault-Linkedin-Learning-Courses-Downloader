//! Backend-owned clipping root registration and verification.
//!
//! Download snapshot roots are selected from a persisted newspaper batch
//! destination. A marker binds the on-disk directory to its SQLite root ID so
//! a disconnected drive or a later path reuse cannot silently redirect a
//! clipping to unrelated storage.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};
use crate::cache::open_runtime;
use serde::{Deserialize, Serialize};

use super::clipping_assets::ClippingAssetLayout;
use super::clipping_models::{
    ClippingError, ClippingErrorCode, ClippingRoot, ClippingRootKind, ClippingRootStatus,
    ClippingRootSummary,
};
use super::clipping_repository::{self as repository, NewClippingRoot, ReconnectRootOutcome};
use super::naming;
use super::storage::{LEGACY_CLIPPING_ROOT_ID, LEGACY_CLIPPING_ROOT_LOCATOR};

pub const SNAPSHOT_DIRECTORY_NAME: &str = "Newspaper snapshots";
pub const INTERNAL_DIRECTORY_NAME: &str = ".linkvault";
pub const ROOT_MARKER_FILE_NAME: &str = "clipping-root-v1.json";
const VERIFIED_ROOT_CACHE_TTL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct ClippingRootRegistry {
    db_path: PathBuf,
    writer: DatabaseWriter,
    legacy_layout: ClippingAssetLayout,
    registration_lock: Arc<Mutex<()>>,
    probes: Arc<RootProbeCoordinator>,
}

#[derive(Clone, Copy)]
struct CachedRootProbe {
    status: ClippingRootStatus,
    checked_at: i64,
}

#[derive(Clone)]
struct CachedVerifiedRoot {
    path: PathBuf,
    valid_until: Instant,
}

#[derive(Default)]
struct RootProbeState {
    cache: HashMap<String, CachedRootProbe>,
    verified: HashMap<String, CachedVerifiedRoot>,
    in_flight: HashSet<String>,
}

#[derive(Default)]
struct RootProbeCoordinator {
    state: Mutex<RootProbeState>,
    completed: Condvar,
    /// Filesystem probes are deliberately serialized. They are user-triggered,
    /// can block on offline drives, and must never fan out without a bound.
    probe_permit: Mutex<()>,
    probe_count: std::sync::atomic::AtomicUsize,
    probe_delay: Mutex<Option<Duration>>,
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
            probes: Arc::new(RootProbeCoordinator::default()),
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

    /// Return persisted roots immediately. This never touches the filesystem;
    /// callers see `unchecked` until a user-triggered check has completed.
    pub fn list_summaries(&self) -> Result<Vec<ClippingRootSummary>, ClippingError> {
        let roots = self.list()?;
        let probes = self
            .probes
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(roots
            .iter()
            .map(|root| self.summary(root, probes.cache.get(&root.id).copied()))
            .collect())
    }

    /// Recheck one root. Concurrent checks for the same ID join the existing
    /// probe and receive its result instead of touching the drive again.
    pub fn check(&self, root_id: &str, now: i64) -> Result<ClippingRootSummary, ClippingError> {
        let root = self.load_root(root_id)?;
        let mut joined_existing = false;
        loop {
            let mut state = self
                .probes
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.in_flight.contains(root_id) {
                joined_existing = true;
                state = self
                    .probes
                    .completed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                drop(state);
                continue;
            }
            if joined_existing {
                if let Some(cached) = state.cache.get(root_id).copied() {
                    return Ok(self.summary(&root, Some(cached)));
                }
            }
            state.in_flight.insert(root_id.to_owned());
            break;
        }

        let (status, verified_path) = {
            let _permit = self
                .probes
                .probe_permit
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.probes
                .probe_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(delay) = *self
                .probes
                .probe_delay
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
            {
                std::thread::sleep(delay);
            }
            self.probe_root(&root)
        };
        let cached = CachedRootProbe {
            status,
            checked_at: now,
        };
        let mut state = self
            .probes
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.cache.insert(root.id.clone(), cached);
        if let Some(path) = verified_path {
            state.verified.insert(
                root.id.clone(),
                CachedVerifiedRoot {
                    path,
                    valid_until: Instant::now() + VERIFIED_ROOT_CACHE_TTL,
                },
            );
        } else {
            state.verified.remove(&root.id);
        }
        state.in_flight.remove(root_id);
        self.probes.completed.notify_all();
        Ok(self.summary(&root, Some(cached)))
    }

    /// Rebind an existing root to the exact selected snapshot directory. The
    /// directory and its marker must already exist; reconnect never scans,
    /// creates, repairs, or moves user files.
    pub fn reconnect(
        &self,
        root_id: &str,
        selected_snapshot_directory: &Path,
        now: i64,
    ) -> Result<ClippingRootSummary, ClippingError> {
        let _registration_guard = self
            .registration_lock
            .lock()
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        let current = self.load_root(root_id)?;
        if current.kind != ClippingRootKind::DownloadSnapshot {
            return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
        }
        let selected = existing_safe_directory(selected_snapshot_directory)?;
        if selected.file_name().and_then(|name| name.to_str()) != Some(SNAPSHOT_DIRECTORY_NAME) {
            return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
        }
        verify_marker(&selected, root_id)?;
        let locator = selected.to_string_lossy().into_owned();
        let new_locator_key = locator_key(&selected);
        let expected_locator_key = current.locator_key.clone();
        let root_id_owned = root_id.to_owned();
        let outcome = self.writer.execute(
            DatabaseWriteContext {
                operation: "clipping_reconnect_download_root",
                provider: DatabaseProvider::Newspaper,
                workflow_id: None,
            },
            move |connection| {
                repository::reconnect_download_root(
                    connection,
                    &root_id_owned,
                    &expected_locator_key,
                    &locator,
                    &new_locator_key,
                    now,
                )
                .map_err(Into::into)
            },
        );
        let updated = match outcome {
            Ok(ReconnectRootOutcome::Updated(root)) => root,
            Ok(ReconnectRootOutcome::NotFoundOrChanged)
            | Ok(ReconnectRootOutcome::LocatorOwnedByOther) => {
                return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
            }
            Err(_) => return Err(ClippingError::new(ClippingErrorCode::DatabaseWriteFailed)),
        };
        let cached = CachedRootProbe {
            status: ClippingRootStatus::Connected,
            checked_at: now,
        };
        let mut state = self
            .probes
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.cache.insert(updated.id.clone(), cached);
        state.verified.insert(
            updated.id.clone(),
            CachedVerifiedRoot {
                path: selected,
                valid_until: Instant::now() + VERIFIED_ROOT_CACHE_TTL,
            },
        );
        Ok(self.summary(&updated, Some(cached)))
    }

    /// Verify immediately before an OS-open action and return the backend-only
    /// path. Commands must never serialize this path to React.
    pub fn verified_open_path(&self, root_id: &str) -> Result<PathBuf, ClippingError> {
        let root = self.load_root(root_id)?;
        Ok(self.resolve_root_fresh(&root)?.root().to_path_buf())
    }

    /// Bypass the short media cache after a read failure so an unplugged or
    /// moved snapshot root is not mistaken for a corrupt clipping asset.
    pub(crate) fn verify_fresh_for_integrity(&self, root_id: &str) -> Result<(), ClippingError> {
        let root = self.load_root(root_id)?;
        self.resolve_root_fresh(&root).map(|_| ())
    }

    fn load_root(&self, root_id: &str) -> Result<ClippingRoot, ClippingError> {
        if root_id != LEGACY_CLIPPING_ROOT_ID && !validate_root_id(root_id) {
            return Err(ClippingError::new(ClippingErrorCode::AssetRootUnavailable));
        }
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_root_by_id(&connection, root_id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))
    }

    fn summary(&self, root: &ClippingRoot, probe: Option<CachedRootProbe>) -> ClippingRootSummary {
        let display_path = match root.kind {
            ClippingRootKind::LegacyManaged => user_display_path(self.legacy_layout.root()),
            ClippingRootKind::DownloadSnapshot => user_display_locator(&root.locator),
        };
        ClippingRootSummary {
            root_id: root.id.clone(),
            kind: root.kind.as_sql().to_owned(),
            display_path,
            status: probe
                .map(|probe| probe.status)
                .unwrap_or(ClippingRootStatus::Unchecked),
            last_checked_at: probe.map(|probe| probe.checked_at),
        }
    }

    fn probe_root(&self, root: &ClippingRoot) -> (ClippingRootStatus, Option<PathBuf>) {
        match root.kind {
            ClippingRootKind::LegacyManaged => {
                match fs::symlink_metadata(self.legacy_layout.root()) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        (ClippingRootStatus::Offline, None)
                    }
                    Ok(metadata)
                        if !is_symlink_or_reparse(&metadata) && metadata.file_type().is_dir() =>
                    {
                        (
                            ClippingRootStatus::Connected,
                            Some(self.legacy_layout.root().to_path_buf()),
                        )
                    }
                    _ => (ClippingRootStatus::MarkerMismatch, None),
                }
            }
            ClippingRootKind::DownloadSnapshot => {
                let locator = Path::new(&root.locator);
                match fs::symlink_metadata(locator) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        (ClippingRootStatus::Offline, None)
                    }
                    Err(_) => (ClippingRootStatus::Offline, None),
                    Ok(metadata)
                        if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_dir() =>
                    {
                        (ClippingRootStatus::MarkerMismatch, None)
                    }
                    Ok(_) => match locator.canonicalize() {
                        Ok(path)
                            if locator_key(&path) == root.locator_key
                                && verify_marker(&path, &root.id).is_ok() =>
                        {
                            (ClippingRootStatus::Connected, Some(path))
                        }
                        _ => (ClippingRootStatus::MarkerMismatch, None),
                    },
                }
            }
        }
    }

    fn resolve_root_fresh(
        &self,
        root: &ClippingRoot,
    ) -> Result<ClippingAssetLayout, ClippingError> {
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
        let root = self.load_root(root_id)?;
        if root.kind == ClippingRootKind::DownloadSnapshot {
            let cached = self
                .probes
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .verified
                .get(root_id)
                .cloned();
            if let Some(cached) = cached {
                if cached.valid_until > Instant::now()
                    && locator_key(&cached.path) == root.locator_key
                {
                    return Ok(ClippingAssetLayout::new_existing(cached.path));
                }
            }
        }
        let layout = self.resolve_root_fresh(&root)?;
        if root.kind == ClippingRootKind::DownloadSnapshot {
            self.probes
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .verified
                .insert(
                    root.id,
                    CachedVerifiedRoot {
                        path: layout.root().to_path_buf(),
                        valid_until: Instant::now() + VERIFIED_ROOT_CACHE_TTL,
                    },
                );
        }
        Ok(layout)
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
        self.resolve_root_fresh(&root)
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

fn user_display_path(path: &Path) -> String {
    user_display_locator(&path.to_string_lossy())
}

fn user_display_locator(locator: &str) -> String {
    locator
        .strip_prefix(r"\\?\UNC\")
        .map(|path| format!(r"\\{path}"))
        .or_else(|| locator.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| locator.to_owned())
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

    #[test]
    fn clipping_root_summaries_are_non_blocking_until_explicitly_checked() {
        let (_temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        fs::remove_dir_all(&root.locator).unwrap();

        let summaries = registry.list_summaries().unwrap();
        let summary = summaries
            .iter()
            .find(|summary| summary.root_id == root.id)
            .unwrap();
        assert_eq!(summary.status, ClippingRootStatus::Unchecked);
        assert_eq!(summary.last_checked_at, None);

        let checked = registry.check(&root.id, 200).unwrap();
        assert_eq!(checked.status, ClippingRootStatus::Offline);
        assert_eq!(checked.last_checked_at, Some(200));
        assert!(!Path::new(&root.locator).exists());
        writer.shutdown().unwrap();
    }

    #[test]
    fn clipping_root_operations_reject_unbounded_or_malformed_ids_before_lookup() {
        let (_temp, registry, writer, _destination) = fixture();
        for invalid in ["../Newspaper snapshots", &"x".repeat(10_000)] {
            assert_eq!(
                registry.check(invalid, 100).unwrap_err().code,
                ClippingErrorCode::AssetRootUnavailable
            );
            assert_eq!(
                registry.verified_open_path(invalid).unwrap_err().code,
                ClippingErrorCode::AssetRootUnavailable
            );
        }
        writer.shutdown().unwrap();
    }

    #[test]
    fn concurrent_clipping_root_checks_share_one_bounded_probe() {
        let (_temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        *registry
            .probes
            .probe_delay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Duration::from_millis(100));
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let callers: Vec<_> = (0..8)
            .map(|index| {
                let registry = registry.clone();
                let root_id = root.id.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.check(&root_id, 200 + index).unwrap()
                })
            })
            .collect();
        let summaries: Vec<_> = callers
            .into_iter()
            .map(|caller| caller.join().unwrap())
            .collect();
        assert!(summaries
            .iter()
            .all(|summary| summary.status == ClippingRootStatus::Connected));
        assert!(summaries
            .iter()
            .all(|summary| summary.last_checked_at == summaries[0].last_checked_at));
        assert_eq!(
            registry
                .probes
                .probe_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        writer.shutdown().unwrap();
    }

    #[test]
    fn clipping_root_check_distinguishes_connected_and_marker_mismatch() {
        let (_temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        assert_eq!(
            registry.check(&root.id, 200).unwrap().status,
            ClippingRootStatus::Connected
        );
        fs::write(
            marker_path(Path::new(&root.locator)),
            br#"{"schema_version":1,"root_id":"clipping-root-wrong"}"#,
        )
        .unwrap();
        assert_eq!(
            registry.check(&root.id, 300).unwrap().status,
            ClippingRootStatus::MarkerMismatch
        );
        writer.shutdown().unwrap();
    }

    #[test]
    fn clipping_root_reconnect_requires_exact_existing_marker_bound_directory() {
        let (temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        let moved_parent = temp.path().join("replacement-drive");
        fs::create_dir(&moved_parent).unwrap();
        let moved_root = moved_parent.join(SNAPSHOT_DIRECTORY_NAME);
        fs::rename(&root.locator, &moved_root).unwrap();
        assert_eq!(
            registry.check(&root.id, 150).unwrap().status,
            ClippingRootStatus::Offline
        );

        let wrong_level = moved_root.join("edition");
        fs::create_dir(&wrong_level).unwrap();
        assert_eq!(
            registry
                .reconnect(&root.id, &wrong_level, 190)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetRootUnavailable
        );

        let empty_parent = temp.path().join("empty-drive");
        fs::create_dir(&empty_parent).unwrap();
        let empty_root = empty_parent.join(SNAPSHOT_DIRECTORY_NAME);
        fs::create_dir(&empty_root).unwrap();
        assert_eq!(
            registry
                .reconnect(&root.id, &empty_root, 191)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetRootUnavailable
        );
        fs::create_dir(empty_root.join(INTERNAL_DIRECTORY_NAME)).unwrap();
        fs::write(
            marker_path(&empty_root),
            serde_json::to_vec(&RootMarker {
                schema_version: 1,
                root_id: "wrong-root-id".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            registry
                .reconnect(&root.id, &empty_root, 192)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetRootUnavailable
        );

        let reconnected = registry.reconnect(&root.id, &moved_root, 200).unwrap();
        assert_eq!(reconnected.status, ClippingRootStatus::Connected);
        assert_eq!(reconnected.display_path, moved_root.to_string_lossy());
        assert_eq!(
            registry.resolve(&root.id).unwrap().root(),
            moved_root.canonicalize().unwrap()
        );
        writer.shutdown().unwrap();
    }

    #[test]
    fn clipping_root_reconnect_writer_failure_preserves_old_locator() {
        let (temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        let moved_parent = temp.path().join("replacement-drive");
        fs::create_dir(&moved_parent).unwrap();
        let moved_root = moved_parent.join(SNAPSHOT_DIRECTORY_NAME);
        fs::rename(&root.locator, &moved_root).unwrap();
        writer.shutdown().unwrap();

        assert_eq!(
            registry
                .reconnect(&root.id, &moved_root, 200)
                .unwrap_err()
                .code,
            ClippingErrorCode::DatabaseWriteFailed
        );
        let stored = registry
            .list()
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == root.id)
            .unwrap();
        assert_eq!(stored.locator, root.locator);
        assert_eq!(stored.updated_at, root.updated_at);
    }

    #[test]
    fn clipping_root_reconnect_same_path_is_idempotent() {
        let (_temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        let reconnected = registry
            .reconnect(&root.id, Path::new(&root.locator), 200)
            .unwrap();
        assert_eq!(reconnected.status, ClippingRootStatus::Connected);
        let stored = registry
            .list()
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == root.id)
            .unwrap();
        assert_eq!(stored.locator, root.locator);
        assert_eq!(stored.locator_key, root.locator_key);
        assert_eq!(stored.updated_at, 200);
        writer.shutdown().unwrap();
    }

    #[test]
    fn clipping_root_reconnect_rejects_duplicate_locator_without_touching_sentinel() {
        let (temp, registry, writer, first_destination) = fixture();
        let second_destination = temp.path().join("second-downloads");
        fs::create_dir(&second_destination).unwrap();
        let first = registry
            .register_download_destination(&first_destination, 100)
            .unwrap();
        let second = registry
            .register_download_destination(&second_destination, 110)
            .unwrap();
        let second_path = PathBuf::from(&second.locator);
        fs::write(
            marker_path(&second_path),
            serde_json::to_vec(&RootMarker {
                schema_version: 1,
                root_id: first.id.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let sentinel = temp.path().join("outside-sentinel.bin");
        let sentinel_bytes = b"must remain byte-identical";
        fs::write(&sentinel, sentinel_bytes).unwrap();

        assert_eq!(
            registry
                .reconnect(&first.id, &second_path, 200)
                .unwrap_err()
                .code,
            ClippingErrorCode::AssetRootUnavailable
        );
        assert_eq!(fs::read(&sentinel).unwrap(), sentinel_bytes);
        let roots = registry.list().unwrap();
        assert_eq!(
            roots
                .iter()
                .find(|root| root.id == first.id)
                .unwrap()
                .locator,
            first.locator
        );
        assert_eq!(
            roots
                .iter()
                .find(|root| root.id == second.id)
                .unwrap()
                .locator,
            second.locator
        );
        writer.shutdown().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn clipping_root_reconnect_rejects_junction_before_touching_target() {
        let (temp, registry, writer, destination) = fixture();
        let root = registry
            .register_download_destination(&destination, 100)
            .unwrap();
        let target_parent = temp.path().join("junction-target");
        let link_parent = temp.path().join("junction-link");
        fs::create_dir(&target_parent).unwrap();
        fs::create_dir(&link_parent).unwrap();
        let target = target_parent.join(SNAPSHOT_DIRECTORY_NAME);
        fs::rename(&root.locator, &target).unwrap();
        let sentinel = target.join("sentinel.bin");
        let sentinel_bytes = b"junction target must remain unchanged";
        fs::write(&sentinel, sentinel_bytes).unwrap();
        let link = link_parent.join(SNAPSHOT_DIRECTORY_NAME);
        let output = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction fixture must be available"
        );

        assert_eq!(
            registry.reconnect(&root.id, &link, 200).unwrap_err().code,
            ClippingErrorCode::AssetRootUnavailable
        );
        assert_eq!(fs::read(&sentinel).unwrap(), sentinel_bytes);
        assert_eq!(
            registry
                .list()
                .unwrap()
                .into_iter()
                .find(|candidate| candidate.id == root.id)
                .unwrap()
                .locator,
            root.locator
        );
        writer.shutdown().unwrap();
    }
}
