//! Safe output-root capabilities used by transient external-process jobs.
//!
//! The renderer supplies a folder selected by the user, but it never supplies
//! a helper path or an output filename.  This module validates the selected
//! root once and only exposes constrained descendants to provider code.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SafeOutputError {
    #[error("output folder must be an absolute path")]
    NotAbsolute,
    #[error("output folder is not a trusted regular directory: {path}")]
    UntrustedDirectory { path: PathBuf },
    #[error("output folder is not writable: {path}: {message}")]
    NotWritable { path: PathBuf, message: String },
    #[error("output child name is invalid")]
    InvalidChildName,
    #[error("output child escaped the validated root")]
    EscapedRoot,
    #[error("validated output identity changed: {path}")]
    IdentityChanged { path: PathBuf },
    #[error("output contains an unsafe descendant: {path}")]
    UnsafeDescendant { path: PathBuf },
    #[error("a verified output already exists at {path}")]
    OutputCollision { path: PathBuf },
    #[error("output path exceeds the supported length")]
    PathTooLong,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct ValidatedOutputRoot {
    path: PathBuf,
    canonical_path: PathBuf,
    leases: Arc<Vec<DirectoryLease>>,
}

#[derive(Debug)]
struct DirectoryLease {
    path: PathBuf,
    handle: File,
    identity: StableIdentity,
}

/// An attempt-directory capability held across helper execution and
/// publication.  The directory handle is opened without following reparse
/// points and retains the directory identity observed at admission.
///
/// Callers keep this lease alive until verification and publication complete.
#[derive(Debug)]
pub struct OutputAttemptLease {
    root: ValidatedOutputRoot,
    path: PathBuf,
    ancestor_leases: Vec<DirectoryLease>,
    identity: StableIdentity,
    handle: File,
}

/// A read-only capability for an already-published item directory.
///
/// The final directory handle is held for the lifetime of the lease.  On
/// platforms that support denying directory renames through an open handle,
/// this also prevents replacement of the leased namespace.  Other platforms
/// are protected by the stable-identity checks in [`Self::revalidate`].
#[derive(Debug)]
pub struct ExistingOutputDirectoryLease {
    root: ValidatedOutputRoot,
    path: PathBuf,
    identity: StableIdentity,
    handle: File,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableIdentity {
    volume_serial_number: u32,
    file_index: u64,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableIdentity {
    device: u64,
    inode: u64,
}

#[cfg(not(any(windows, unix)))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableIdentity;

impl ValidatedOutputRoot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn root_lease(&self) -> Result<&DirectoryLease, SafeOutputError> {
        self.leases
            .last()
            .ok_or_else(|| SafeOutputError::IdentityChanged {
                path: self.path.clone(),
            })
    }

    /// Re-open the selected root without following a Windows reparse point
    /// and compare its stable identity with the admission lease.  Callers
    /// should perform this check before and after helper-visible mutations.
    pub fn revalidate(&self) -> Result<(), SafeOutputError> {
        for lease in self.leases.iter() {
            revalidate_directory_lease(lease)?;
        }
        let canonical = self
            .path
            .canonicalize()
            .map_err(|_| SafeOutputError::IdentityChanged {
                path: self.path.clone(),
            })?;
        if canonical != self.canonical_path {
            return Err(SafeOutputError::IdentityChanged {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    /// Lease an existing final item for read-only verification and reuse.
    ///
    /// The final name is always resolved as one direct child of the validated
    /// root.  A missing child is not an error; any existing child must be a
    /// regular directory and is opened with the same no-follow/reparse guards
    /// used by staging capabilities.  Contents are checked separately through
    /// [`ExistingOutputDirectoryLease::validate_contents`].
    pub fn existing_item_lease(
        &self,
        final_name: &str,
    ) -> Result<Option<ExistingOutputDirectoryLease>, SafeOutputError> {
        validate_component(final_name)?;
        let destination = self.path.join(final_name);
        if path_utf16_len(&destination) > 240 {
            return Err(SafeOutputError::PathTooLong);
        }

        self.revalidate()?;
        let metadata = match fs::symlink_metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.revalidate()?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if is_reparse_point(&metadata) {
            return Err(SafeOutputError::UnsafeDescendant { path: destination });
        }
        if !metadata.file_type().is_dir() {
            return Err(SafeOutputError::OutputCollision { path: destination });
        }

        let handle = open_directory_guard(&destination, false, false).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::PermissionDenied
            ) {
                SafeOutputError::IdentityChanged {
                    path: destination.clone(),
                }
            } else {
                SafeOutputError::Io(error)
            }
        })?;
        let handle_metadata = handle.metadata()?;
        if is_untrusted_directory(&handle_metadata) {
            return Err(SafeOutputError::UnsafeDescendant { path: destination });
        }
        let identity = stable_identity(&handle)?;

        // Check the namespace again after opening the held handle.  This
        // closes the ordinary path-swap window and makes a reparse replacement
        // fail closed even on platforms where opening a directory follows it.
        let current_metadata = fs::symlink_metadata(&destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SafeOutputError::IdentityChanged {
                    path: destination.clone(),
                }
            } else {
                SafeOutputError::Io(error)
            }
        })?;
        if is_reparse_point(&current_metadata) {
            return Err(SafeOutputError::UnsafeDescendant { path: destination });
        }
        if !current_metadata.file_type().is_dir() {
            return Err(SafeOutputError::IdentityChanged { path: destination });
        }
        self.revalidate()?;

        let lease = ExistingOutputDirectoryLease {
            root: self.clone(),
            path: destination,
            identity,
            handle,
        };
        lease.revalidate()?;
        Ok(Some(lease))
    }

    /// Create and immediately lease a unique attempt directory.
    #[allow(dead_code)]
    pub fn staging_attempt_lease(
        &self,
        occurrence_id: &str,
        artifact_fingerprint: &str,
    ) -> Result<OutputAttemptLease, SafeOutputError> {
        validate_component(occurrence_id)?;
        validate_component(artifact_fingerprint)?;
        static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);
        let attempt_name = format!(
            "attempt-{}-{}-{}",
            std::process::id(),
            now_nanos(),
            NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed)
        );
        validate_component(&attempt_name)?;
        let names = [
            ".linkvault-staging",
            "youtube",
            occurrence_id,
            artifact_fingerprint,
            attempt_name.as_str(),
        ];
        self.revalidate()?;
        let mut current = self.path.clone();
        let mut ancestor_leases = Vec::with_capacity(names.len() - 1);
        let mut final_handle = None;
        for (index, name) in names.iter().enumerate() {
            current.push(name);
            if path_utf16_len(&current) > 240 {
                return Err(SafeOutputError::PathTooLong);
            }
            let is_final = index + 1 == names.len();
            match fs::create_dir(&current) {
                Ok(()) => {}
                Err(error) if !is_final && error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            let handle = open_directory_guard(&current, is_final, false)?;
            let metadata = handle.metadata()?;
            if is_untrusted_directory(&metadata) {
                return Err(SafeOutputError::UntrustedDirectory {
                    path: current.clone(),
                });
            }
            let identity = stable_identity(&handle)?;
            if is_final {
                final_handle = Some((handle, identity));
            } else {
                ancestor_leases.push(DirectoryLease {
                    path: current.clone(),
                    handle,
                    identity,
                });
            }
        }
        let (handle, identity) = final_handle.ok_or(SafeOutputError::EscapedRoot)?;
        self.revalidate()?;
        Ok(OutputAttemptLease {
            root: self.clone(),
            path: current,
            ancestor_leases,
            identity,
            handle,
        })
    }

    /// Verify and publish a leased attempt. Windows renames the held source
    /// handle without replacement. Every destination ancestor remains guarded,
    /// so the absolute destination namespace cannot be redirected mid-rename.
    pub fn publish_attempt_lease(
        &self,
        attempt: OutputAttemptLease,
        final_name: &str,
    ) -> Result<PathBuf, SafeOutputError> {
        validate_component(final_name)?;
        if path_utf16_len(&self.path.join(final_name)) > 240 {
            return Err(SafeOutputError::PathTooLong);
        }
        if attempt.root.path != self.path
            || attempt.root.root_lease()?.identity != self.root_lease()?.identity
        {
            return Err(SafeOutputError::IdentityChanged {
                path: attempt.path().to_path_buf(),
            });
        }
        self.revalidate()?;
        attempt.revalidate()?;
        attempt.validate_contents()?;
        let destination = self.path.join(final_name);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                return Err(SafeOutputError::OutputCollision { path: destination });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        #[cfg(windows)]
        {
            rename_directory_without_replace(&attempt.handle, &destination).map_err(|error| {
                if is_destination_collision(&error) {
                    SafeOutputError::OutputCollision {
                        path: destination.clone(),
                    }
                } else {
                    SafeOutputError::Io(error)
                }
            })?;
        }

        #[cfg(not(windows))]
        fs::rename(attempt.path(), &destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                SafeOutputError::OutputCollision {
                    path: destination.clone(),
                }
            } else {
                SafeOutputError::Io(error)
            }
        })?;

        self.revalidate()?;
        let published_handle = open_directory_nofollow(&destination, false).map_err(|error| {
            SafeOutputError::IdentityChanged {
                path: if error.kind() == std::io::ErrorKind::NotFound {
                    destination.clone()
                } else {
                    destination.clone()
                },
            }
        })?;
        let published_metadata = published_handle.metadata()?;
        if is_untrusted_directory(&published_metadata)
            || stable_identity(&published_handle)? != attempt.identity
        {
            return Err(SafeOutputError::IdentityChanged { path: destination });
        }
        Ok(destination)
    }

    pub fn discard_attempt_lease(
        &self,
        attempt: OutputAttemptLease,
    ) -> Result<(), SafeOutputError> {
        if attempt.root.path != self.path
            || attempt.root.root_lease()?.identity != self.root_lease()?.identity
        {
            return Err(SafeOutputError::IdentityChanged {
                path: attempt.path.clone(),
            });
        }
        attempt.validate_contents()?;
        let mut names = Vec::new();
        for entry in fs::read_dir(attempt.path())? {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| SafeOutputError::UnsafeDescendant { path: entry.path() })?;
            validate_component(&name)?;
            names.push(name);
        }

        #[cfg(windows)]
        {
            for name in &names {
                let file = open_regular_leaf_for_delete(attempt.path(), name)?;
                mark_delete_by_handle(&file)?;
            }
            attempt.revalidate()?;
            mark_delete_by_handle(&attempt.handle)?;
        }

        #[cfg(not(windows))]
        {
            for name in &names {
                fs::remove_file(attempt.path().join(name))?;
            }
            fs::remove_dir(attempt.path())?;
        }
        Ok(())
    }
}

impl OutputAttemptLease {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-open the attempt directory without following reparse points and
    /// compare its stable identity with the identity held by this lease.
    pub fn revalidate(&self) -> Result<(), SafeOutputError> {
        for lease in &self.ancestor_leases {
            revalidate_directory_lease(lease)?;
        }
        let held_metadata = self.handle.metadata()?;
        if is_untrusted_directory(&held_metadata) {
            return Err(SafeOutputError::UnsafeDescendant {
                path: self.path.clone(),
            });
        }
        self.root.revalidate()?;
        let handle = open_directory_nofollow(&self.path, false).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::PermissionDenied
            ) {
                SafeOutputError::IdentityChanged {
                    path: self.path.clone(),
                }
            } else {
                SafeOutputError::Io(error)
            }
        })?;
        let metadata = handle.metadata()?;
        if is_untrusted_directory(&metadata) {
            return Err(SafeOutputError::UnsafeDescendant {
                path: self.path.clone(),
            });
        }
        if stable_identity(&handle)? != self.identity {
            return Err(SafeOutputError::IdentityChanged {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    /// Validate direct regular-file children through no-follow leaf handles.
    /// Nested directories and reparse points are rejected before publication.
    pub fn validate_contents(&self) -> Result<(), SafeOutputError> {
        self.revalidate()?;
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
                return Err(SafeOutputError::UnsafeDescendant { path });
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| SafeOutputError::UnsafeDescendant { path: path.clone() })?;
            validate_component(name)?;
            let _file = self.open_leaf(name)?;
        }
        self.revalidate()
    }

    /// Open one direct regular-file child without following a reparse point.
    /// The returned handle is suitable for bounded manifest or artifact reads.
    pub fn open_leaf(&self, name: &str) -> Result<File, SafeOutputError> {
        validate_component(name)?;
        self.revalidate()?;
        let file = open_regular_leaf(&self.path, name, false)?;
        self.revalidate()?;
        Ok(file)
    }

    /// Create one direct regular-file child with create-new semantics and a
    /// no-follow leaf check.  This is intended for handle-safe manifest writes.
    #[allow(dead_code)]
    pub fn create_leaf(&self, name: &str) -> Result<File, SafeOutputError> {
        validate_component(name)?;
        self.revalidate()?;
        let file = open_regular_leaf(&self.path, name, true)?;
        self.revalidate()?;
        Ok(file)
    }
}

impl ExistingOutputDirectoryLease {
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-open the final directory without following reparse points and
    /// compare its stable identity with the identity held by this lease.
    /// Root and final-directory identity are checked before every capability
    /// read and again by the caller after the read completes.
    pub fn revalidate(&self) -> Result<(), SafeOutputError> {
        self.root.revalidate()?;
        let held_metadata = self.handle.metadata()?;
        if is_untrusted_directory(&held_metadata) {
            return Err(SafeOutputError::UnsafeDescendant {
                path: self.path.clone(),
            });
        }
        let current_metadata = fs::symlink_metadata(&self.path).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::PermissionDenied
            ) {
                SafeOutputError::IdentityChanged {
                    path: self.path.clone(),
                }
            } else {
                SafeOutputError::Io(error)
            }
        })?;
        if is_reparse_point(&current_metadata) {
            return Err(SafeOutputError::UnsafeDescendant {
                path: self.path.clone(),
            });
        }
        if !current_metadata.file_type().is_dir() {
            return Err(SafeOutputError::IdentityChanged {
                path: self.path.clone(),
            });
        }
        let observer = open_directory_nofollow(&self.path, false).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::PermissionDenied
            ) {
                SafeOutputError::IdentityChanged {
                    path: self.path.clone(),
                }
            } else {
                SafeOutputError::Io(error)
            }
        })?;
        let observer_metadata = observer.metadata()?;
        if is_untrusted_directory(&observer_metadata) {
            return Err(SafeOutputError::UnsafeDescendant {
                path: self.path.clone(),
            });
        }
        if stable_identity(&observer)? != self.identity {
            return Err(SafeOutputError::IdentityChanged {
                path: self.path.clone(),
            });
        }
        self.root.revalidate()
    }

    /// Validate that the leased item is a flat directory of direct regular
    /// files.  Every leaf is opened through a no-follow handle before this
    /// method returns, so nested directories and reparse descendants fail
    /// closed.
    #[allow(dead_code)]
    pub fn validate_contents(&self) -> Result<(), SafeOutputError> {
        self.revalidate()?;
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
                return Err(SafeOutputError::UnsafeDescendant { path });
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| SafeOutputError::UnsafeDescendant { path: path.clone() })?;
            validate_component(name)?;
            let _file = self.open_leaf(name)?;
        }
        self.revalidate()
    }

    /// Open one direct regular-file child without following a reparse point.
    /// The returned handle is read-only and remains safe if the directory is
    /// renamed after this method returns.
    pub fn open_leaf(&self, name: &str) -> Result<File, SafeOutputError> {
        validate_component(name)?;
        self.revalidate()?;
        let file = open_regular_leaf(&self.path, name, false)?;
        self.revalidate()?;
        Ok(file)
    }
}

pub fn validate_output_root(path: &Path) -> Result<ValidatedOutputRoot, SafeOutputError> {
    if !path.is_absolute() {
        return Err(SafeOutputError::NotAbsolute);
    }
    validate_root_shape(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => SafeOutputError::UntrustedDirectory {
            path: path.to_path_buf(),
        },
        _ => SafeOutputError::NotWritable {
            path: path.to_path_buf(),
            message: error.to_string(),
        },
    })?;
    if is_untrusted_directory(&metadata) {
        return Err(SafeOutputError::UntrustedDirectory {
            path: path.to_path_buf(),
        });
    }
    reject_reparse_components(path)?;
    let leases = guard_root_chain(path)?;
    let canonical_path = path.canonicalize()?;
    let root = ValidatedOutputRoot {
        path: path.to_path_buf(),
        canonical_path,
        leases: Arc::new(leases),
    };
    probe_writable_dir(root.path())?;
    root.revalidate()?;
    Ok(root)
}

pub fn validate_output_component(name: &str) -> Result<(), SafeOutputError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 240
        || name.ends_with('.')
        || name.ends_with(' ')
        || name.contains(':')
        || name
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
        || is_windows_reserved_name(name)
    {
        return Err(SafeOutputError::InvalidChildName);
    }
    Ok(())
}

fn validate_component(name: &str) -> Result<(), SafeOutputError> {
    validate_output_component(name)
}

fn guard_root_chain(path: &Path) -> Result<Vec<DirectoryLease>, SafeOutputError> {
    let mut paths = path
        .ancestors()
        .filter(|ancestor| ancestor.is_absolute())
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    paths.reverse();
    let path_count = paths.len();
    let mut leases = Vec::with_capacity(path_count);
    for (index, current) in paths.into_iter().enumerate() {
        let is_selected_root = index + 1 == path_count;
        let handle = open_directory_guard(&current, false, is_selected_root).map_err(|error| {
            SafeOutputError::NotWritable {
                path: current.clone(),
                message: error.to_string(),
            }
        })?;
        let metadata = handle.metadata()?;
        if is_untrusted_directory(&metadata) {
            return Err(SafeOutputError::UntrustedDirectory { path: current });
        }
        let identity = stable_identity(&handle)?;
        leases.push(DirectoryLease {
            path: current,
            handle,
            identity,
        });
    }
    if leases.is_empty() {
        return Err(SafeOutputError::UntrustedDirectory {
            path: path.to_path_buf(),
        });
    }
    Ok(leases)
}

fn revalidate_directory_lease(lease: &DirectoryLease) -> Result<(), SafeOutputError> {
    if is_untrusted_directory(&lease.handle.metadata()?) {
        return Err(SafeOutputError::UnsafeDescendant {
            path: lease.path.clone(),
        });
    }
    let observer = open_directory_nofollow(&lease.path, false).map_err(|_| {
        SafeOutputError::IdentityChanged {
            path: lease.path.clone(),
        }
    })?;
    if is_untrusted_directory(&observer.metadata()?)
        || stable_identity(&observer)? != lease.identity
    {
        return Err(SafeOutputError::IdentityChanged {
            path: lease.path.clone(),
        });
    }
    Ok(())
}

fn open_directory_guard(
    path: &Path,
    delete_access: bool,
    add_child_access: bool,
) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        const FILE_ADD_SUBDIRECTORY: u32 = 0x0004;
        options
            .access_mode(
                FILE_LIST_DIRECTORY
                    | FILE_READ_ATTRIBUTES
                    | if delete_access { DELETE } else { 0 }
                    | if add_child_access {
                        FILE_ADD_SUBDIRECTORY
                    } else {
                        0
                    },
            )
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let _ = (delete_access, add_child_access);
    options.open(path)
}

fn open_directory_nofollow(path: &Path, for_rename: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        const FILE_ADD_SUBDIRECTORY: u32 = 0x0004;
        options
            .access_mode(
                FILE_LIST_DIRECTORY
                    | FILE_READ_ATTRIBUTES
                    | if for_rename {
                        DELETE | FILE_ADD_SUBDIRECTORY
                    } else {
                        0
                    },
            )
            .share_mode(if for_rename {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
            })
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let _ = for_rename;
    options.open(path)
}

#[cfg(windows)]
fn open_regular_leaf_for_delete(directory: &Path, name: &str) -> Result<File, SafeOutputError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };
    let path = directory.join(name);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .access_mode(FILE_READ_ATTRIBUTES | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(&path)?;
    let metadata = file.metadata()?;
    if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
        return Err(SafeOutputError::UnsafeDescendant { path });
    }
    Ok(file)
}

#[cfg(windows)]
fn mark_delete_by_handle(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let result = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle().cast(),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO>())
                .expect("disposition metadata size fits u32"),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn open_regular_leaf(
    directory: &Path,
    name: &str,
    create_new: bool,
) -> Result<File, SafeOutputError> {
    let path = directory.join(name);
    // Windows uses FILE_FLAG_OPEN_REPARSE_POINT below.  On Unix, perform the
    // equivalent path-side checks before and after opening so a direct leaf
    // read cannot knowingly follow an existing symlink.  The held directory
    // lease and the post-open check close the ordinary replacement window.
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
            return Err(SafeOutputError::UnsafeDescendant { path });
        }
    }
    let mut options = OpenOptions::new();
    options
        .read(!create_new)
        .write(create_new)
        .create_new(create_new);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_DELETE,
            FILE_SHARE_READ,
        };
        options
            // Parent-directory publication requires children to share delete.
            // Write sharing remains denied, so a verified handle cannot be
            // modified in place while the caller retains it through publish.
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN);
    }
    let file = options.open(&path).map_err(|error| {
        if create_new && error.kind() == std::io::ErrorKind::AlreadyExists {
            SafeOutputError::OutputCollision { path: path.clone() }
        } else {
            SafeOutputError::Io(error)
        }
    })?;
    let metadata = file.metadata()?;
    if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
        return Err(SafeOutputError::UnsafeDescendant { path });
    }
    let path_metadata = fs::symlink_metadata(&path)?;
    if is_reparse_point(&path_metadata) || !path_metadata.file_type().is_file() {
        return Err(SafeOutputError::UnsafeDescendant { path });
    }
    Ok(file)
}

fn stable_identity(file: &File) -> std::io::Result<StableIdentity> {
    #[cfg(windows)]
    {
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
            return Err(std::io::Error::last_os_error());
        }
        return Ok(StableIdentity {
            volume_serial_number: information.dwVolumeSerialNumber,
            file_index: ((information.nFileIndexHigh as u64) << 32)
                | information.nFileIndexLow as u64,
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        return Ok(StableIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        });
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = file;
        Ok(StableIdentity)
    }
}

#[cfg(windows)]
#[allow(dead_code)]
fn rename_directory_without_replace(attempt: &File, destination: &Path) -> std::io::Result<()> {
    use std::alloc::{alloc, dealloc, Layout};
    use std::mem::{align_of, offset_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, FILE_RENAME_INFO, FILE_RENAME_INFO_0,
    };

    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let offset = offset_of!(FILE_RENAME_INFO, FileName);
    let size = offset
        .checked_add(
            name.len()
                .saturating_add(1)
                .saturating_mul(std::mem::size_of::<u16>()),
        )
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "name too long"))?;
    let size_u32 = u32::try_from(size)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "name too long"))?;
    let layout = Layout::from_size_align(size, align_of::<FILE_RENAME_INFO>()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid rename layout")
    })?;
    let info = unsafe { alloc(layout).cast::<FILE_RENAME_INFO>() };
    if info.is_null() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::OutOfMemory,
            "rename metadata allocation failed",
        ));
    }
    unsafe {
        ptr::write_bytes(info.cast::<u8>(), 0, size);
        (*info).Anonymous = FILE_RENAME_INFO_0 {
            ReplaceIfExists: false,
        };
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = u32::try_from(name.len() * std::mem::size_of::<u16>())
            .expect("validated output component length fits u32");
        ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
    }
    let result = unsafe {
        SetFileInformationByHandle(
            attempt.as_raw_handle().cast(),
            FileRenameInfo,
            info.cast(),
            size_u32,
        )
    };
    unsafe { dealloc(info.cast::<u8>(), layout) };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
#[allow(dead_code)]
fn is_destination_collision(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(80) | Some(183) // ERROR_FILE_EXISTS / ERROR_ALREADY_EXISTS
    ) || error.kind() == std::io::ErrorKind::AlreadyExists
}

fn path_utf16_len(path: &Path) -> usize {
    path.as_os_str().to_string_lossy().encode_utf16().count()
}

fn validate_root_shape(path: &Path) -> Result<(), SafeOutputError> {
    for component in path.components() {
        #[cfg(windows)]
        if let Component::Prefix(prefix) = component {
            use std::path::Prefix;
            if !matches!(prefix.kind(), Prefix::Disk(_)) {
                return Err(SafeOutputError::UntrustedDirectory {
                    path: path.to_path_buf(),
                });
            }
        }
        if let Component::Normal(name) = component {
            let name = name.to_string_lossy();
            validate_component(&name)?;
        }
    }
    Ok(())
}

fn is_windows_reserved_name(name: &str) -> bool {
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base[3..]
                .chars()
                .all(|character| character.is_ascii_digit())
            && base[3..]
                .parse::<u8>()
                .is_ok_and(|number| (1..=9).contains(&number)))
}

fn reject_reparse_components(path: &Path) -> Result<(), SafeOutputError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            _ => {
                current.push(component.as_os_str());
                let metadata = fs::symlink_metadata(&current)?;
                if is_untrusted_directory(&metadata) {
                    return Err(SafeOutputError::UntrustedDirectory { path: current });
                }
            }
        }
    }
    Ok(())
}

fn probe_writable_dir(path: &Path) -> Result<(), SafeOutputError> {
    let probe = path.join(format!(
        ".linkvault-youtube-write-probe-{}",
        std::process::id()
    ));
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|mut file| file.write_all(b"ok"));
    if let Err(error) = result {
        return Err(SafeOutputError::NotWritable {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
    let _ = fs::remove_file(probe);
    Ok(())
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn is_untrusted_directory(metadata: &fs::Metadata) -> bool {
    if !metadata.file_type().is_dir() {
        return true;
    }
    is_reparse_point(metadata)
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn validates_root_and_confines_children() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        assert!(attempt.path().starts_with(temp.path()));
        let published = root.publish_attempt_lease(attempt, "video").unwrap();
        assert!(published.is_dir());
        assert!(root.staging_attempt_lease("..", "artifact-1").is_err());
        assert!(root
            .staging_attempt_lease("nested/name", "artifact-1")
            .is_err());
    }

    #[test]
    fn rejects_relative_and_file_roots() {
        assert!(validate_output_root(Path::new("relative")).is_err());
        let temp = tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, b"not-a-directory").unwrap();
        assert!(validate_output_root(&file).is_err());
    }

    #[test]
    fn rejects_reserved_ads_and_device_child_names() {
        for name in [
            "CON",
            "NUL.txt",
            "COM1",
            "LPT9.log",
            "video:stream",
            "trail.",
            "trail ",
        ] {
            assert!(validate_output_component(name).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn unsafe_attempt_is_not_published() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let attempt_path = attempt.path().to_path_buf();
        fs::create_dir(attempt.path().join("nested")).unwrap();
        let result = root.publish_attempt_lease(attempt, "video");
        assert!(matches!(
            result,
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
        assert!(!temp.path().join("video").exists());
        assert!(attempt_path.exists());
    }

    #[test]
    fn existing_final_directory_is_a_collision() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let first = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        fs::write(first.path().join("artifact.mp4"), b"complete").unwrap();
        root.publish_attempt_lease(first, "video").unwrap();
        let second = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        fs::write(second.path().join("artifact.mp4"), b"complete").unwrap();
        assert!(matches!(
            root.publish_attempt_lease(second, "video"),
            Err(SafeOutputError::OutputCollision { .. })
        ));
        assert_eq!(
            fs::read(temp.path().join("video").join("artifact.mp4")).unwrap(),
            b"complete"
        );
    }

    #[test]
    fn existing_item_lease_reports_absent_final_directory() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();

        assert!(root.existing_item_lease("missing-video").unwrap().is_none());
    }

    #[test]
    fn existing_item_lease_reads_valid_flat_item() {
        use std::io::Read;

        let temp = tempdir().unwrap();
        let final_path = temp.path().join("video");
        fs::create_dir(&final_path).unwrap();
        fs::write(final_path.join("artifact.txt"), b"complete").unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let lease = root.existing_item_lease("video").unwrap().unwrap();

        assert_eq!(lease.path(), final_path);
        lease.validate_contents().unwrap();
        let mut artifact = lease.open_leaf("artifact.txt").unwrap();
        let mut bytes = Vec::new();
        artifact.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"complete");
    }

    #[test]
    fn existing_item_lease_rejects_nested_content() {
        let temp = tempdir().unwrap();
        let final_path = temp.path().join("video");
        fs::create_dir(&final_path).unwrap();
        fs::create_dir(final_path.join("nested")).unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let lease = root.existing_item_lease("video").unwrap().unwrap();

        assert!(matches!(
            lease.validate_contents(),
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn existing_item_lease_rejects_reparse_content() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let final_path = temp.path().join("video");
        fs::create_dir(&final_path).unwrap();
        symlink(temp.path(), final_path.join("link")).unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let lease = root.existing_item_lease("video").unwrap().unwrap();

        assert!(matches!(
            lease.validate_contents(),
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
        assert!(matches!(
            lease.open_leaf("link"),
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn existing_item_lease_detects_replacement_by_identity() {
        let temp = tempdir().unwrap();
        let final_path = temp.path().join("video");
        fs::create_dir(&final_path).unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let lease = root.existing_item_lease("video").unwrap().unwrap();

        fs::rename(&final_path, temp.path().join("moved-video")).unwrap();
        fs::create_dir(&final_path).unwrap();
        assert!(matches!(
            lease.revalidate(),
            Err(SafeOutputError::IdentityChanged { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn existing_item_lease_denies_final_directory_rename() {
        let temp = tempdir().unwrap();
        let final_path = temp.path().join("video");
        fs::create_dir(&final_path).unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let lease = root.existing_item_lease("video").unwrap().unwrap();

        assert!(fs::rename(&final_path, temp.path().join("moved-video")).is_err());
        lease.revalidate().unwrap();
    }

    #[test]
    fn leased_attempt_uses_stable_identity_and_no_follow_leaf_handles() {
        use std::io::{Read, Write};

        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        fs::write(attempt.path().join("artifact.txt"), b"complete").unwrap();
        {
            let mut manifest = attempt.create_leaf("manifest.json").unwrap();
            manifest.write_all(br#"{"schemaVersion":1}"#).unwrap();
            manifest.sync_all().unwrap();
        }
        let mut manifest = attempt.open_leaf("manifest.json").unwrap();
        let mut bytes = Vec::new();
        manifest.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, br#"{"schemaVersion":1}"#);
        drop(manifest);
        attempt.validate_contents().unwrap();
        let published = root.publish_attempt_lease(attempt, "leased-video").unwrap();
        assert_eq!(
            fs::read(published.join("artifact.txt")).unwrap(),
            b"complete"
        );
    }

    #[test]
    fn discard_removes_only_a_validated_flat_attempt() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let attempt_path = attempt.path().to_path_buf();
        fs::write(attempt.path().join("artifact.txt"), b"partial").unwrap();
        root.discard_attempt_lease(attempt).unwrap();
        assert!(!attempt_path.exists());
    }

    #[test]
    fn discard_fails_closed_on_nested_content() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let attempt_path = attempt.path().to_path_buf();
        fs::create_dir(attempt.path().join("nested")).unwrap();
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, b"keep").unwrap();
        assert!(matches!(
            root.discard_attempt_lease(attempt),
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
        assert!(attempt_path.exists());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }

    #[cfg(windows)]
    #[test]
    fn held_root_and_attempt_chain_deny_namespace_renames() {
        let temp = tempdir().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let root = validate_output_root(&output).unwrap();
        assert!(fs::rename(&output, temp.path().join("moved-output")).is_err());

        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let staging = output.join(".linkvault-staging");
        assert!(fs::rename(&staging, output.join("swapped-staging")).is_err());
        assert!(fs::rename(
            attempt.path(),
            attempt.path().with_file_name("replaced-attempt")
        )
        .is_err());
        attempt.revalidate().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_reparse_descendant() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        symlink(temp.path(), attempt.path().join("link")).unwrap();
        assert!(matches!(
            attempt.validate_contents(),
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
    }
}
