//! Safe output-root capabilities used by transient external-process jobs.
//!
//! The renderer supplies a folder selected by the user, but it never supplies
//! a helper path or an output filename.  This module validates the selected
//! root once and only exposes constrained descendants to provider code.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const MAX_SUPPORTED_PATH_UTF16: usize = 240;
const MAX_HELPER_STAGING_DIR_UTF16: usize = 192;
const MAX_HELPER_TEMP_ROOT_UTF16: usize = MAX_SUPPORTED_PATH_UTF16 - 1 - "roaming-app-data".len();
const HELPER_TEMP_PARENT_NAME: &str = "youtube-helper";
const HELPER_TEMP_PREFIX: &str = "run-";
#[cfg(windows)]
const HELPER_TEMP_IO_ATTEMPTS: u32 = 8;
#[cfg(windows)]
const HELPER_TEMP_RETRY_DELAY: Duration = Duration::from_millis(20);
#[cfg(windows)]
const HELPER_TEMP_ABSENCE_ATTEMPTS: u32 = 20;
#[cfg(windows)]
const HELPER_TEMP_ABSENCE_RETRY_DELAY: Duration = Duration::from_millis(50);
const MAX_HELPER_CLEANUP_DEPTH: usize = 8;
const MAX_HELPER_CLEANUP_NODES: usize = 4096;
const MAX_HELPER_CLEANUP_NAME_UTF16: usize = 240;
const CREATION_FAULT_NONE: u8 = 0;
const CREATION_FAULT_BEFORE_ROOT: u8 = 1;
#[cfg(any(test, feature = "youtube-process-test"))]
const CREATION_FAULT_AFTER_ROOT: u8 = 2;
const HELPER_TEMP_CHILDREN: [(&str, HelperTempDirectory); 5] = [
    ("cache", HelperTempDirectory::Cache),
    ("deno", HelperTempDirectory::Deno),
    ("home", HelperTempDirectory::Home),
    ("local-app-data", HelperTempDirectory::LocalAppData),
    ("roaming-app-data", HelperTempDirectory::RoamingAppData),
];

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
    #[error("helper temporary workspace validation failed: {reason}")]
    HelperTemp { reason: String },
    #[cfg(windows)]
    #[error("helper temporary workspace cleanup could not be proven: {reason}")]
    HelperTempCleanupUnproven {
        reason: String,
        recovery: Box<HelperTempCleanupRecovery>,
    },
    #[cfg(windows)]
    #[error("helper temporary workspace cleanup could not be proven: {reason}")]
    HelperTempCleanupUnprovenNoRecovery { reason: String },
    #[cfg(windows)]
    #[error("staging attempt cleanup could not be proven: {reason}")]
    OutputAttemptCleanupUnproven {
        reason: String,
        recovery: Box<OutputAttemptCleanupRecovery>,
    },
    #[cfg(windows)]
    #[error("staging attempt cleanup is permanently unproven: {reason}")]
    OutputAttemptCleanupPermanentUnproven {
        reason: String,
        recovery: Box<OutputAttemptCleanupRecovery>,
    },
    #[cfg(windows)]
    #[error("staging attempt cleanup verification failed: {reason}")]
    OutputAttemptCleanup { reason: String },
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

/// The fixed helper-visible directory names.  Provider and renderer code never
/// supplies any of these paths; the managed-process port selects an enum
/// value and receives a path owned by [`HelperTempCapability`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HelperTempDirectory {
    Cache,
    Deno,
    Home,
    LocalAppData,
    RoamingAppData,
}

/// Opaque, app-owned temporary workspace for one managed-helper launch.
///
/// The parent, launch root, and all fixed children retain no-follow directory
/// handles and the stable identities observed at admission.  The capability
/// is intentionally not `Clone`: one owner must perform the final bounded
/// cleanup and report any cleanup uncertainty to the managed-process caller.
#[derive(Debug)]
pub(crate) struct HelperTempCapability {
    parent: ValidatedOutputRoot,
    root: DirectoryLease,
    children: Vec<DirectoryLease>,
    #[cfg(all(windows, feature = "youtube-process-test"))]
    test_cleanup_fault_after: Option<u64>,
}

/// Identity-held cleanup state for a helper workspace whose cleanup may need
/// to be retried after a bounded failure.
///
/// The recovery object owns the same parent, launch-root, and fixed-child
/// handles that admitted the helper.  It never reconstructs a root from a
/// caller-provided path.  Once a target has been marked for deletion, a later
/// verification may observe that exact target missing; an unmarked target
/// disappearing remains a fail-closed error.
#[cfg(windows)]
#[derive(Debug)]
pub struct HelperTempCleanupRecovery {
    parent: ValidatedOutputRoot,
    root: CleanupTarget,
    children: Vec<CleanupTarget>,
    plan: Option<Vec<CleanupTarget>>,
    root_absence_proven: bool,
    #[cfg(any(test, feature = "youtube-process-test"))]
    test_fail_after: u64,
}

/// Creation/admission failure that keeps an identity-held cleanup verifier
/// when the post-`keep` cleanup itself cannot be proven.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) enum HelperTempCreateError {
    Admission(SafeOutputError),
    CleanupUnproven {
        error: SafeOutputError,
        recovery: Box<HelperTempCleanupRecovery>,
    },
    CleanupUnprovenNoRecovery {
        error: SafeOutputError,
    },
}

#[cfg(windows)]
impl HelperTempCreateError {
    fn into_safe_output_error(self) -> SafeOutputError {
        match self {
            Self::Admission(error) => error,
            Self::CleanupUnproven { error, recovery } => {
                let _ = error;
                SafeOutputError::HelperTempCleanupUnproven {
                    reason: "helper workspace cleanup could not be proven".to_string(),
                    recovery,
                }
            }
            Self::CleanupUnprovenNoRecovery { error } => {
                let _ = error;
                SafeOutputError::HelperTempCleanupUnprovenNoRecovery {
                    reason: "helper workspace cleanup could not be proven without a retained root"
                        .to_string(),
                }
            }
        }
    }

    /// Consume the failure and retain the opaque recovery object for runtime
    /// quarantine.  The error remains path-free and is suitable for the
    /// existing command-boundary mapping.
    #[allow(dead_code)]
    pub(crate) fn into_cleanup_recovery(
        self,
    ) -> Option<(SafeOutputError, HelperTempCleanupRecovery)> {
        match self {
            Self::Admission(_) => None,
            Self::CleanupUnproven { error, recovery } => Some((error, *recovery)),
            Self::CleanupUnprovenNoRecovery { .. } => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_cleanup_unproven(&self) -> bool {
        matches!(
            self,
            Self::CleanupUnproven { .. } | Self::CleanupUnprovenNoRecovery { .. }
        )
    }
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

/// Identity-held cleanup state for a staging attempt that is being discarded.
///
/// Unlike helper-temp cleanup, an attempt is deliberately flat: only direct
/// regular files are admitted to the deletion plan.  The plan owns each
/// no-follow delete handle and stable identity observed before the first
/// disposition, so retry never rediscovers a path as an authority.
#[cfg(windows)]
#[derive(Debug)]
pub struct OutputAttemptCleanupRecovery {
    root: ValidatedOutputRoot,
    ancestor_leases: Vec<DirectoryLease>,
    attempt: CleanupTarget,
    plan: Option<Vec<CleanupTarget>>,
    planning_failed: bool,
    attempt_absence_proven: bool,
    #[cfg(any(test, feature = "youtube-process-test"))]
    test_fail_after: u64,
}

/// Typed discard failure.  Both variants retain the exact attempt recovery;
/// the permanent form tells the caller to quarantine rather than treating the
/// failure as an ordinary item error.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) enum OutputAttemptDiscardError {
    Recoverable {
        error: SafeOutputError,
        recovery: Box<OutputAttemptCleanupRecovery>,
    },
    Permanent {
        error: SafeOutputError,
        recovery: Box<OutputAttemptCleanupRecovery>,
    },
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

    /// Maximum UTF-16 units available to one direct child while retaining the
    /// conservative cross-helper path ceiling used by publication.
    pub fn direct_child_name_budget(&self) -> usize {
        MAX_SUPPORTED_PATH_UTF16.saturating_sub(path_utf16_len(&self.path).saturating_add(1))
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
        if path_utf16_len(&destination) > MAX_SUPPORTED_PATH_UTF16 {
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
        let scope_name = compact_staging_scope(occurrence_id, artifact_fingerprint);
        let attempt_name = format!(
            "y-{}-{:x}-{:x}",
            scope_name,
            now_nanos(),
            NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed)
        );
        validate_component(&attempt_name)?;
        let names = [".lv", attempt_name.as_str()];
        self.revalidate()?;
        let mut current = self.path.clone();
        let mut ancestor_leases = Vec::with_capacity(names.len() - 1);
        let mut final_handle = None;
        for (index, name) in names.iter().enumerate() {
            current.push(name);
            let path_limit = if index + 1 == names.len() {
                MAX_HELPER_STAGING_DIR_UTF16
            } else {
                MAX_SUPPORTED_PATH_UTF16
            };
            if path_utf16_len(&current) > path_limit {
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
        if path_utf16_len(&self.path.join(final_name)) > MAX_SUPPORTED_PATH_UTF16 {
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

    /// Discard a staging attempt while retaining identity-held recovery on
    /// every Windows cleanup failure.  B3-B should call this API directly;
    /// the legacy wrapper below converts the typed failure to a
    /// path-free-enough safe-output error without dropping its recovery.
    #[cfg(windows)]
    pub(crate) fn discard_attempt_lease_recoverable(
        &self,
        attempt: OutputAttemptLease,
    ) -> Result<(), OutputAttemptDiscardError> {
        let mut recovery = Box::new(OutputAttemptCleanupRecovery::from_attempt(attempt));
        let ownership_error = match (self.root_lease(), recovery.root.root_lease()) {
            (Ok(root_lease), Ok(attempt_root_lease))
                if recovery.root.path == self.path
                    && attempt_root_lease.identity == root_lease.identity =>
            {
                None
            }
            _ => Some(SafeOutputError::IdentityChanged {
                path: recovery.attempt.path.clone(),
            }),
        };
        if let Some(error) = ownership_error {
            recovery.planning_failed = true;
            return Err(OutputAttemptDiscardError::Permanent { error, recovery });
        }

        match recovery.verify_cleanup() {
            Ok(()) => Ok(()),
            Err(error) if recovery.plan.is_none() || recovery.planning_failed => {
                Err(OutputAttemptDiscardError::Permanent { error, recovery })
            }
            Err(error) => Err(OutputAttemptDiscardError::Recoverable { error, recovery }),
        }
    }

    /// Compatibility wrapper for existing callers.  The error carries the
    /// retained recovery so callers cannot accidentally lose the lease by
    /// handling the result as an ordinary item failure.
    #[cfg(windows)]
    pub fn discard_attempt_lease(
        &self,
        attempt: OutputAttemptLease,
    ) -> Result<(), SafeOutputError> {
        match self.discard_attempt_lease_recoverable(attempt) {
            Ok(()) => Ok(()),
            Err(OutputAttemptDiscardError::Recoverable { error, recovery }) => {
                let _ = error;
                Err(SafeOutputError::OutputAttemptCleanupUnproven {
                    reason: "staging attempt cleanup could not be proven".to_string(),
                    recovery,
                })
            }
            Err(OutputAttemptDiscardError::Permanent { error, recovery }) => {
                let _ = error;
                Err(SafeOutputError::OutputAttemptCleanupPermanentUnproven {
                    reason: "staging attempt cleanup is permanently unproven".to_string(),
                    recovery,
                })
            }
        }
    }

    #[cfg(not(windows))]
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
        for name in &names {
            fs::remove_file(attempt.path().join(name))?;
        }
        fs::remove_dir(attempt.path())?;
        Ok(())
    }
}

impl HelperTempCapability {
    /// Create one unpredictable, exclusive helper workspace beneath the
    /// application-managed data directory.  The data directory and fixed
    /// parent are validated before the random launch directory is created;
    /// all launch directories are opened with no-follow semantics before this
    /// capability is returned.
    #[cfg(windows)]
    pub(crate) fn create() -> Result<Self, SafeOutputError> {
        Self::create_recoverable().map_err(HelperTempCreateError::into_safe_output_error)
    }

    /// Create a helper capability while preserving an identity-held cleanup
    /// verifier if post-admission cleanup cannot be proven.  Runtime callers
    /// that can quarantine and retry cleanup should use this form.
    #[cfg(windows)]
    pub(crate) fn create_recoverable() -> Result<Self, HelperTempCreateError> {
        let data_dir = crate::app::storage::resolve_data_dir().map_err(|_| {
            HelperTempCreateError::Admission(helper_temp_failure(
                "application data directory unavailable",
            ))
        })?;
        Self::create_in_recoverable(&data_dir)
    }

    /// Build a helper capability beneath an explicit application-owned parent.
    ///
    /// This seam is limited to tests and the existing Windows process-test
    /// feature so tests can use an isolated temporary parent without mutating
    /// process-wide storage or environment state.  Production callers must
    /// use [`Self::create`], which resolves the application data directory.
    #[cfg(all(windows, any(test, feature = "youtube-process-test")))]
    pub(crate) fn create_for_test(parent_path: &Path) -> Result<Self, SafeOutputError> {
        Self::create_in_recoverable(parent_path)
            .map_err(HelperTempCreateError::into_safe_output_error)
    }

    /// Test/process-fixture seam for exercising retained admission-cleanup
    /// recovery without changing process-wide application storage.
    #[cfg(all(windows, any(test, feature = "youtube-process-test")))]
    #[allow(dead_code)]
    pub(crate) fn create_for_test_recoverable(
        parent_path: &Path,
    ) -> Result<Self, HelperTempCreateError> {
        Self::create_in_recoverable(parent_path)
    }

    /// Test-only seam for proving that post-`keep` creation failures preserve
    /// the exact cleanup outcome: before root acquisition there is no safe
    /// recovery object, while after root acquisition the retained verifier
    /// remains retryable.
    #[cfg(all(windows, any(test, feature = "youtube-process-test")))]
    pub(crate) fn create_for_test_creation_failure(
        parent_path: &Path,
        after_root_acquisition: bool,
    ) -> Result<Self, SafeOutputError> {
        Self::create_in_recoverable_with_creation_fault(
            parent_path,
            if after_root_acquisition {
                CREATION_FAULT_AFTER_ROOT
            } else {
                CREATION_FAULT_BEFORE_ROOT
            },
        )
        .map_err(HelperTempCreateError::into_safe_output_error)
    }

    #[cfg(windows)]
    fn create_in_recoverable(data_dir: &Path) -> Result<Self, HelperTempCreateError> {
        Self::create_in_recoverable_with_creation_fault(data_dir, CREATION_FAULT_NONE)
    }

    #[cfg(windows)]
    fn create_in_recoverable_with_creation_fault(
        data_dir: &Path,
        creation_fault: u8,
    ) -> Result<Self, HelperTempCreateError> {
        let data_root = validate_output_root(data_dir).map_err(|error| {
            HelperTempCreateError::Admission(helper_temp_failure(&admission_failure_reason(
                "application data directory",
                &error,
            )))
        })?;
        let parent_path = data_root.path().join(HELPER_TEMP_PARENT_NAME);
        if path_utf16_len(&parent_path) > MAX_SUPPORTED_PATH_UTF16 {
            return Err(HelperTempCreateError::Admission(helper_temp_failure(
                "helper parent path is too long",
            )));
        }
        ensure_fixed_directory(&parent_path).map_err(HelperTempCreateError::Admission)?;
        data_root.revalidate().map_err(|error| {
            HelperTempCreateError::Admission(helper_temp_failure(&admission_failure_reason(
                "application data directory",
                &error,
            )))
        })?;
        let parent = validate_output_root(&parent_path).map_err(|error| {
            HelperTempCreateError::Admission(helper_temp_failure(&admission_failure_reason(
                "helper parent",
                &error,
            )))
        })?;
        parent.revalidate().map_err(|_| {
            HelperTempCreateError::Admission(helper_temp_failure("helper parent changed"))
        })?;
        ensure_same_volume(
            parent.root_lease().map_err(|_| {
                HelperTempCreateError::Admission(helper_temp_failure(
                    "helper parent identity is unavailable",
                ))
            })?,
            data_root.root_lease().map_err(|_| {
                HelperTempCreateError::Admission(helper_temp_failure(
                    "application data identity is unavailable",
                ))
            })?,
        )
        .map_err(HelperTempCreateError::Admission)?;

        let temporary = tempfile::Builder::new()
            .prefix(HELPER_TEMP_PREFIX)
            .tempdir_in(parent.path())
            .map_err(|_| {
                HelperTempCreateError::Admission(helper_temp_failure(
                    "exclusive helper launch root creation failed",
                ))
            })?;
        if path_utf16_len(temporary.path()) > MAX_HELPER_TEMP_ROOT_UTF16 {
            return Err(HelperTempCreateError::Admission(helper_temp_failure(
                "helper launch root path is too long",
            )));
        }
        let launch_path = temporary.keep();
        let mut guard = HelperTempCreationGuard::new(parent.clone());
        let result = (|| {
            // `tempdir_in` creates the directory with create-new semantics.
            // Do not recreate it or accept a replacement before opening its
            // held no-follow identity.
            if creation_fault == CREATION_FAULT_BEFORE_ROOT {
                return Err(helper_temp_failure(
                    "deterministic helper root acquisition failure",
                ));
            }
            let root = open_helper_directory(&launch_path)?;
            ensure_same_volume(
                &root,
                parent
                    .root_lease()
                    .map_err(|_| helper_temp_failure("helper parent identity is unavailable"))?,
            )?;
            guard.root = Some(root);

            #[cfg(any(test, feature = "youtube-process-test"))]
            if creation_fault == CREATION_FAULT_AFTER_ROOT {
                guard.test_fail_after = Some(0);
                return Err(helper_temp_failure(
                    "deterministic helper post-admission failure",
                ));
            }

            parent.revalidate().map_err(|_| {
                helper_temp_failure("helper parent changed during launch-root creation")
            })?;

            for (name, _) in HELPER_TEMP_CHILDREN {
                let child_path = launch_path.join(name);
                ensure_fixed_directory(&child_path)?;
                let child = open_helper_directory(&child_path)?;
                ensure_same_volume(
                    &child,
                    guard.root.as_ref().ok_or_else(|| {
                        helper_temp_failure("helper root identity is unavailable")
                    })?,
                )?;
                guard.children.push(child);
            }
            let root = guard
                .root
                .as_ref()
                .ok_or_else(|| helper_temp_failure("helper root identity is unavailable"))?;
            revalidate_directory_lease(root)?;
            for child in &guard.children {
                revalidate_directory_lease(child)?;
            }
            parent.revalidate().map_err(|_| {
                helper_temp_failure("helper parent changed before launch admission")
            })?;

            let root = guard
                .root
                .take()
                .ok_or_else(|| helper_temp_failure("helper root identity is unavailable"))?;
            let children = std::mem::take(&mut guard.children);
            Ok(Self {
                parent,
                root,
                children,
                #[cfg(all(windows, feature = "youtube-process-test"))]
                test_cleanup_fault_after: None,
            })
        })();
        match result {
            Ok(capability) => Ok(capability),
            Err(error) => {
                let recovery = guard.into_cleanup_recovery();
                match recovery {
                    Ok(mut recovery) => match recovery.verify_cleanup() {
                        Ok(()) => Err(HelperTempCreateError::Admission(error)),
                        Err(cleanup_error) => Err(HelperTempCreateError::CleanupUnproven {
                            error: cleanup_error,
                            recovery: Box::new(recovery),
                        }),
                    },
                    Err(cleanup_error) => Err(HelperTempCreateError::CleanupUnprovenNoRecovery {
                        error: cleanup_error,
                    }),
                }
            }
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn create() -> Result<Self, SafeOutputError> {
        Err(helper_temp_failure(
            "validated helper temporary workspaces are unsupported on this platform",
        ))
    }

    pub(crate) fn root_path(&self) -> &Path {
        &self.root.path
    }

    pub(crate) fn child_path(&self, directory: HelperTempDirectory) -> &Path {
        let index = match directory {
            HelperTempDirectory::Cache => 0,
            HelperTempDirectory::Deno => 1,
            HelperTempDirectory::Home => 2,
            HelperTempDirectory::LocalAppData => 3,
            HelperTempDirectory::RoamingAppData => 4,
        };
        &self.children[index].path
    }

    /// Revalidate every held parent, root, and fixed child identity.  This is
    /// deliberately separate from cleanup so the supervisor can call it at
    /// each process-containment boundary without taking ownership.
    pub(crate) fn revalidate(&self) -> Result<(), SafeOutputError> {
        self.parent.revalidate()?;
        revalidate_directory_lease(&self.root)?;
        for child in &self.children {
            revalidate_directory_lease(child)?;
            ensure_same_volume(child, &self.root)?;
        }
        Ok(())
    }

    /// Transfer this capability into identity-held cleanup state.  Callers
    /// that must retain cleanup authority after a failed attempt should keep
    /// the returned object and retry [`HelperTempCleanupRecovery::verify_cleanup`].
    #[cfg(windows)]
    pub(crate) fn cleanup_recovery(self) -> HelperTempCleanupRecovery {
        let HelperTempCapability {
            parent,
            root,
            children,
            #[cfg(all(windows, feature = "youtube-process-test"))]
            test_cleanup_fault_after,
        } = self;
        #[cfg(feature = "youtube-process-test")]
        let test_fail_after = test_cleanup_fault_after.unwrap_or(u64::MAX);
        #[cfg(all(test, not(feature = "youtube-process-test")))]
        let test_fail_after = u64::MAX;
        HelperTempCleanupRecovery {
            parent,
            root: CleanupTarget::from_directory_lease(root),
            children: children
                .into_iter()
                .map(CleanupTarget::from_directory_lease)
                .collect(),
            plan: None,
            root_absence_proven: false,
            #[cfg(any(test, feature = "youtube-process-test"))]
            test_fail_after,
        }
    }

    /// Inject one instance-local cleanup failure for the process-test seam.
    /// The failure is consumed by the identity-held recovery object and cannot
    /// affect another capability or production process.
    #[cfg(all(windows, feature = "youtube-process-test"))]
    pub(crate) fn set_cleanup_fault_after(&mut self, fail_after: u64) {
        self.test_cleanup_fault_after = Some(fail_after);
    }

    /// Boundedly remove the launch tree while all held identities remain
    /// open.  Any validation uncertainty returns an error before deletion is
    /// attempted.  No recursive standard-library removal is used here: each
    /// regular leaf and directory is opened no-follow and marked for deletion
    /// through its handle.
    #[cfg(windows)]
    pub(crate) fn cleanup(self) -> Result<(), SafeOutputError> {
        let mut recovery = self.cleanup_recovery();
        recovery.verify_cleanup()
    }

    #[cfg(not(windows))]
    pub(crate) fn cleanup(self) -> Result<(), SafeOutputError> {
        let _ = self;
        Err(helper_temp_failure(
            "validated helper temporary workspaces are unsupported on this platform",
        ))
    }
}

fn helper_temp_failure(reason: &str) -> SafeOutputError {
    SafeOutputError::HelperTemp {
        reason: reason.to_string(),
    }
}

#[cfg(windows)]
fn admission_failure_reason(label: &str, error: &SafeOutputError) -> String {
    match error {
        SafeOutputError::NotAbsolute => format!("{label} is not an absolute path"),
        SafeOutputError::UntrustedDirectory { .. } => {
            format!("{label} is not a trusted regular directory")
        }
        SafeOutputError::NotWritable { .. } => format!("{label} is not writable"),
        SafeOutputError::PathTooLong => format!("{label} path is too long"),
        SafeOutputError::InvalidChildName => format!("{label} contains an invalid path component"),
        SafeOutputError::IdentityChanged { .. } => format!("{label} changed during admission"),
        SafeOutputError::HelperTemp { reason } => reason.clone(),
        _ => format!("{label} could not be validated"),
    }
}

#[cfg(windows)]
fn is_retryable_win_io(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(
            5 | // ERROR_ACCESS_DENIED
            32 | // ERROR_SHARING_VIOLATION
            33 | // ERROR_LOCK_VIOLATION
            145 | // ERROR_DIR_NOT_EMPTY
            303 // ERROR_DELETE_PENDING
        )
    )
}

#[cfg(windows)]
fn retry_win_io<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut last = None;
    for attempt in 0..HELPER_TEMP_IO_ATTEMPTS {
        match op() {
            Ok(value) => return Ok(value),
            Err(error) if is_retryable_win_io(&error) && attempt + 1 < HELPER_TEMP_IO_ATTEMPTS => {
                last = Some(error);
                thread::sleep(HELPER_TEMP_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            "helper filesystem retry exhausted",
        )
    }))
}

#[cfg(windows)]
fn wait_marked_helper_paths_absent(plan: Option<&[CleanupTarget]>, children: &[CleanupTarget]) {
    for attempt in 0..HELPER_TEMP_IO_ATTEMPTS {
        let remaining = plan
            .into_iter()
            .flatten()
            .chain(children.iter())
            .any(|target| {
                target.marked
                    && !matches!(
                        fs::symlink_metadata(&target.path),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    )
            });
        if !remaining {
            return;
        }
        if attempt + 1 < HELPER_TEMP_IO_ATTEMPTS {
            thread::sleep(HELPER_TEMP_RETRY_DELAY);
        }
    }
}

#[cfg(windows)]
struct HelperTempCreationGuard {
    parent: ValidatedOutputRoot,
    root: Option<DirectoryLease>,
    children: Vec<DirectoryLease>,
    #[cfg(any(test, feature = "youtube-process-test"))]
    test_fail_after: Option<u64>,
}

#[cfg(windows)]
impl HelperTempCreationGuard {
    fn new(parent: ValidatedOutputRoot) -> Self {
        Self {
            parent,
            root: None,
            children: Vec::with_capacity(HELPER_TEMP_CHILDREN.len()),
            #[cfg(any(test, feature = "youtube-process-test"))]
            test_fail_after: None,
        }
    }

    /// Transfer a post-`keep` admission guard only when its launch-root
    /// identity was opened and retained.  A path without that identity is not
    /// a safe recovery token and is rejected without rediscovery.
    fn into_cleanup_recovery(self) -> Result<HelperTempCleanupRecovery, SafeOutputError> {
        let root = self
            .root
            .ok_or_else(|| helper_temp_failure("helper root identity is unavailable"))?;
        #[cfg(any(test, feature = "youtube-process-test"))]
        let test_fail_after = self.test_fail_after.unwrap_or(u64::MAX);
        Ok(HelperTempCleanupRecovery {
            parent: self.parent,
            root: CleanupTarget::from_directory_lease(root),
            children: self
                .children
                .into_iter()
                .map(CleanupTarget::from_directory_lease)
                .collect(),
            plan: None,
            root_absence_proven: false,
            #[cfg(any(test, feature = "youtube-process-test"))]
            test_fail_after,
        })
    }
}

#[cfg(windows)]
fn ensure_fixed_directory(path: &Path) -> Result<(), SafeOutputError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_untrusted_directory(&metadata) {
                return Err(helper_temp_failure(
                    "a pre-existing helper directory is a reparse point or not a directory",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Err(error) = fs::create_dir(path) {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(helper_temp_failure("helper directory creation failed"));
                }
            }
        }
        Err(_) => return Err(helper_temp_failure("helper directory admission failed")),
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| helper_temp_failure("helper directory disappeared during admission"))?;
    if is_untrusted_directory(&metadata) {
        return Err(helper_temp_failure(
            "helper directory became a reparse point during admission",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn open_helper_directory(path: &Path) -> Result<DirectoryLease, SafeOutputError> {
    if path_utf16_len(path) > MAX_SUPPORTED_PATH_UTF16 {
        return Err(helper_temp_failure("helper directory path is too long"));
    }
    let handle = retry_win_io(|| open_directory_guard(path, true, true))
        .map_err(|_| helper_temp_failure("helper directory handle could not be opened"))?;
    let metadata = handle
        .metadata()
        .map_err(|_| helper_temp_failure("helper directory identity is unavailable"))?;
    if is_untrusted_directory(&metadata) {
        return Err(helper_temp_failure(
            "helper directory handle resolves to an unsafe object",
        ));
    }
    let identity = stable_identity(&handle)
        .map_err(|_| helper_temp_failure("helper directory identity is unavailable"))?;
    let current = fs::symlink_metadata(path)
        .map_err(|_| helper_temp_failure("helper directory disappeared after opening"))?;
    if is_untrusted_directory(&current) {
        return Err(helper_temp_failure(
            "helper directory became a reparse point after opening",
        ));
    }
    Ok(DirectoryLease {
        path: path.to_path_buf(),
        handle,
        identity,
    })
}

#[cfg(windows)]
fn ensure_same_volume(
    left: &DirectoryLease,
    right: &DirectoryLease,
) -> Result<(), SafeOutputError> {
    if left.identity.volume_serial_number != right.identity.volume_serial_number {
        return Err(helper_temp_failure(
            "helper directory volume differs from the application volume",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn ensure_same_volume(
    _left: &DirectoryLease,
    _right: &DirectoryLease,
) -> Result<(), SafeOutputError> {
    Ok(())
}

#[cfg(windows)]
#[derive(Default)]
struct CleanupBudget {
    nodes: usize,
}

#[cfg(windows)]
struct CleanupEntry {
    path: PathBuf,
    handle: File,
    identity: StableIdentity,
    is_directory: bool,
}

#[cfg(windows)]
#[derive(Debug)]
struct CleanupTarget {
    path: PathBuf,
    identity: StableIdentity,
    is_directory: bool,
    handle: Option<File>,
    marked: bool,
}

#[cfg(windows)]
impl CleanupTarget {
    fn from_directory_lease(lease: DirectoryLease) -> Self {
        Self {
            path: lease.path,
            identity: lease.identity,
            is_directory: true,
            handle: Some(lease.handle),
            marked: false,
        }
    }

    fn from_entry(entry: CleanupEntry) -> Self {
        Self {
            path: entry.path,
            identity: entry.identity,
            is_directory: entry.is_directory,
            handle: Some(entry.handle),
            marked: false,
        }
    }

    fn revalidate(&self) -> Result<(), SafeOutputError> {
        if !self.is_directory {
            return Err(helper_temp_failure(
                "helper cleanup target is not a directory",
            ));
        }
        if let Some(handle) = self.handle.as_ref() {
            revalidate_directory_parts(&self.path, handle, self.identity)
        } else if self.marked {
            match fs::symlink_metadata(&self.path) {
                Ok(_) => validate_cleanup_target(self),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(SafeOutputError::Io(error)),
            }
        } else {
            Err(helper_temp_failure(
                "unmarked helper cleanup target identity is unavailable",
            ))
        }
    }
}

#[cfg(windows)]
impl HelperHeldDirectory for CleanupTarget {
    fn path(&self) -> &Path {
        &self.path
    }

    fn revalidate(&self) -> Result<(), SafeOutputError> {
        CleanupTarget::revalidate(self)
    }
}

#[cfg(windows)]
trait HelperHeldDirectory {
    fn path(&self) -> &Path;
    fn revalidate(&self) -> Result<(), SafeOutputError>;
}

#[cfg(windows)]
impl HelperHeldDirectory for DirectoryLease {
    fn path(&self) -> &Path {
        &self.path
    }

    fn revalidate(&self) -> Result<(), SafeOutputError> {
        revalidate_directory_lease(self)
    }
}

#[cfg(windows)]
impl OutputAttemptCleanupRecovery {
    fn from_attempt(attempt: OutputAttemptLease) -> Self {
        let OutputAttemptLease {
            root,
            path,
            ancestor_leases,
            identity,
            handle,
        } = attempt;
        Self {
            root,
            ancestor_leases,
            attempt: CleanupTarget {
                path,
                identity,
                is_directory: true,
                handle: Some(handle),
                marked: false,
            },
            plan: None,
            planning_failed: false,
            attempt_absence_proven: false,
            #[cfg(any(test, feature = "youtube-process-test"))]
            test_fail_after: u64::MAX,
        }
    }

    /// Verify and finish bounded flat-attempt deletion.  Planning completes
    /// before any delete disposition is issued, and a failed retry retains
    /// every unconsumed handle plus each target's marked state.
    pub(crate) fn verify_cleanup(&mut self) -> Result<(), SafeOutputError> {
        #[cfg(any(test, feature = "youtube-process-test"))]
        let mut fault = self.test_fail_after;
        #[cfg(not(any(test, feature = "youtube-process-test")))]
        let mut fault = 0u64;
        let result = self.verify_cleanup_inner(&mut fault);
        if result.is_err() && self.plan.is_none() {
            // A recovery without a complete immutable plan is permanently
            // unproven.  Retrying must not re-enumerate a potentially changed
            // namespace and silently create a new authority.
            self.planning_failed = true;
        }
        #[cfg(any(test, feature = "youtube-process-test"))]
        {
            self.test_fail_after = fault;
        }
        result
    }

    #[cfg(test)]
    pub(crate) fn set_cleanup_fault_after(&mut self, fail_after: u64) {
        self.test_fail_after = fail_after;
    }

    fn verify_cleanup_inner(&mut self, fault: &mut u64) -> Result<(), SafeOutputError> {
        if self.planning_failed && self.plan.is_none() {
            return Err(output_attempt_cleanup_failure(
                "staging attempt cleanup plan was not completed",
            ));
        }
        self.root.revalidate().map_err(|_| {
            output_attempt_cleanup_failure("validated output root changed during cleanup")
        })?;
        for ancestor in &self.ancestor_leases {
            revalidate_directory_lease(ancestor).map_err(|_| {
                output_attempt_cleanup_failure("staging ancestor changed during cleanup")
            })?;
        }

        if self.attempt.marked {
            if !self.attempt_absence_proven {
                self.verify_attempt_absent()?;
                self.attempt_absence_proven = true;
                self.plan.take();
            }
            return Ok(());
        }

        self.validate_live_attempt()?;
        if self.plan.is_none() {
            self.plan = Some(self.plan_direct_leaves()?);
        }
        self.validate_attempt_namespace()?;

        if let Some(plan) = self.plan.as_mut() {
            for target in plan.iter_mut() {
                if target.marked {
                    continue;
                }
                let handle = target.handle.as_ref().ok_or_else(|| {
                    output_attempt_cleanup_failure(
                        "unmarked staging leaf delete handle is unavailable",
                    )
                })?;
                mark_delete_for_cleanup(handle, fault).map_err(|_| {
                    output_attempt_cleanup_failure(
                        "staging attempt cleanup could not delete a verified leaf",
                    )
                })?;
                target.marked = true;
                target.handle = None;
            }
        }

        // A helper/user may have raced a new direct child into the directory
        // after planning.  Re-check the exact bounded namespace before the
        // attempt disposition; a new child is never silently deleted.
        self.validate_attempt_namespace()?;
        self.validate_live_attempt()?;
        let attempt_handle = self.attempt.handle.take().ok_or_else(|| {
            output_attempt_cleanup_failure("unmarked staging attempt handle is unavailable")
        })?;
        if mark_delete_root_for_cleanup(&attempt_handle, fault).is_err() {
            self.attempt.handle = Some(attempt_handle);
            return Err(output_attempt_cleanup_failure(
                "staging attempt cleanup could not delete the attempt directory",
            ));
        }
        drop(attempt_handle);
        self.attempt.marked = true;
        self.verify_attempt_absent()?;
        self.attempt_absence_proven = true;
        self.plan.take();
        Ok(())
    }

    fn validate_live_attempt(&self) -> Result<(), SafeOutputError> {
        let handle = self.attempt.handle.as_ref().ok_or_else(|| {
            output_attempt_cleanup_failure("unmarked staging attempt handle is unavailable")
        })?;
        let metadata = handle.metadata().map_err(|_| {
            output_attempt_cleanup_failure("staging attempt identity is unavailable")
        })?;
        if is_untrusted_directory(&metadata)
            || stable_identity(handle).map_err(|_| {
                output_attempt_cleanup_failure("staging attempt identity is unavailable")
            })? != self.attempt.identity
        {
            return Err(output_attempt_cleanup_failure(
                "staging attempt identity changed",
            ));
        }
        let current = fs::symlink_metadata(&self.attempt.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                output_attempt_cleanup_failure("unmarked staging attempt disappeared")
            } else {
                output_attempt_cleanup_failure("staging attempt namespace could not be observed")
            }
        })?;
        if is_untrusted_directory(&current) {
            return Err(output_attempt_cleanup_failure(
                "staging attempt became a reparse point",
            ));
        }
        let observer = open_directory_nofollow(&self.attempt.path, false).map_err(|error| {
            output_attempt_cleanup_failure(&format!(
                "staging attempt observer handle could not be opened: {error}"
            ))
        })?;
        let observer_metadata = observer.metadata().map_err(|_| {
            output_attempt_cleanup_failure("staging attempt observer metadata unavailable")
        })?;
        if is_untrusted_directory(&observer_metadata) {
            return Err(output_attempt_cleanup_failure(
                "staging attempt became a reparse point",
            ));
        }
        let current_identity = stable_identity(&observer).map_err(|_| {
            output_attempt_cleanup_failure("staging attempt observer identity unavailable")
        })?;
        if current_identity != self.attempt.identity {
            return Err(output_attempt_cleanup_failure(
                "staging attempt identity changed",
            ));
        }
        Ok(())
    }

    fn plan_direct_leaves(&self) -> Result<Vec<CleanupTarget>, SafeOutputError> {
        let mut entries = Vec::new();
        let mut budget = CleanupBudget::default();
        for entry in fs::read_dir(&self.attempt.path).map_err(|_| {
            output_attempt_cleanup_failure("staging attempt directory could not be enumerated")
        })? {
            budget.nodes = budget.nodes.saturating_add(1);
            if budget.nodes > MAX_HELPER_CLEANUP_NODES {
                return Err(output_attempt_cleanup_failure(
                    "staging attempt cleanup node limit exceeded",
                ));
            }
            let entry = entry.map_err(|_| {
                output_attempt_cleanup_failure("staging attempt directory entry was unreadable")
            })?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| {
                output_attempt_cleanup_failure("staging attempt entry name is not UTF-8")
            })?;
            if name.encode_utf16().count() > MAX_HELPER_CLEANUP_NAME_UTF16 {
                return Err(output_attempt_cleanup_failure(
                    "staging attempt entry name is too long",
                ));
            }
            validate_component(name).map_err(|_| {
                output_attempt_cleanup_failure("staging attempt entry name is invalid")
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| output_attempt_cleanup_failure("staging attempt entry disappeared"))?;
            if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
                return Err(output_attempt_cleanup_failure(
                    "staging attempt contains a non-regular direct child",
                ));
            }
            entries.push((name.to_owned(), path));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));

        let mut plan = Vec::with_capacity(entries.len());
        for (name, path) in entries {
            let handle = open_regular_leaf_for_delete(&self.attempt.path, &name)?;
            let identity = stable_identity(&handle).map_err(|_| {
                output_attempt_cleanup_failure("staging attempt leaf identity is unavailable")
            })?;
            let current = fs::symlink_metadata(&path)
                .map_err(|_| output_attempt_cleanup_failure("staging attempt leaf disappeared"))?;
            if is_reparse_point(&current) || !current.file_type().is_file() {
                return Err(output_attempt_cleanup_failure(
                    "staging attempt leaf became unsafe",
                ));
            }
            let observer = open_regular_leaf_observer(&self.attempt.path, &name)?;
            if stable_identity(&observer).map_err(|_| {
                output_attempt_cleanup_failure("staging attempt leaf identity is unavailable")
            })? != identity
            {
                return Err(output_attempt_cleanup_failure(
                    "staging attempt leaf identity changed",
                ));
            }
            plan.push(CleanupTarget {
                path,
                identity,
                is_directory: false,
                handle: Some(handle),
                marked: false,
            });
        }
        Ok(plan)
    }

    fn validate_attempt_namespace(&self) -> Result<(), SafeOutputError> {
        let plan = self.plan.as_ref().ok_or_else(|| {
            output_attempt_cleanup_failure("staging attempt cleanup plan is unavailable")
        })?;
        let mut seen = vec![false; plan.len()];
        let mut budget = CleanupBudget::default();
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.attempt.path).map_err(|_| {
            output_attempt_cleanup_failure("staging attempt directory could not be enumerated")
        })? {
            budget.nodes = budget.nodes.saturating_add(1);
            if budget.nodes > MAX_HELPER_CLEANUP_NODES {
                return Err(output_attempt_cleanup_failure(
                    "staging attempt cleanup node limit exceeded",
                ));
            }
            entries.push(entry.map_err(|_| {
                output_attempt_cleanup_failure("staging attempt directory entry was unreadable")
            })?);
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let index = plan
                .iter()
                .position(|target| target.path == path)
                .ok_or_else(|| {
                    output_attempt_cleanup_failure("staging attempt contains an unexpected child")
                })?;
            seen[index] = true;
            let target = &plan[index];
            validate_attempt_leaf_target(target)?;
        }
        for (index, target) in plan.iter().enumerate() {
            if !target.marked && !seen[index] {
                return Err(output_attempt_cleanup_failure(
                    "unmarked staging attempt leaf disappeared",
                ));
            }
        }
        Ok(())
    }

    fn verify_attempt_absent(&self) -> Result<(), SafeOutputError> {
        match fs::symlink_metadata(&self.attempt.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(output_attempt_cleanup_failure(
                "staging attempt absence could not be verified",
            )),
            Ok(metadata) => {
                if is_reparse_point(&metadata) || !metadata.file_type().is_dir() {
                    return Err(output_attempt_cleanup_failure(
                        "staging attempt was replaced by an unsafe object",
                    ));
                }
                let observer =
                    open_directory_nofollow(&self.attempt.path, false).map_err(|_| {
                        output_attempt_cleanup_failure(
                            "staging attempt identity could not be observed",
                        )
                    })?;
                if stable_identity(&observer).map_err(|_| {
                    output_attempt_cleanup_failure("staging attempt identity could not be observed")
                })? != self.attempt.identity
                {
                    return Err(output_attempt_cleanup_failure(
                        "staging attempt was replaced before absence was proven",
                    ));
                }
                Err(output_attempt_cleanup_failure(
                    "staging attempt remained visible after deletion",
                ))
            }
        }
    }
}

#[cfg(windows)]
fn validate_attempt_leaf_target(target: &CleanupTarget) -> Result<(), SafeOutputError> {
    let metadata = match fs::symlink_metadata(&target.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && target.marked => {
            return Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(output_attempt_cleanup_failure(
                "unmarked staging attempt leaf disappeared",
            ))
        }
        Err(_) => {
            return Err(output_attempt_cleanup_failure(
                "staging attempt leaf could not be observed",
            ))
        }
    };
    if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
        return Err(output_attempt_cleanup_failure(
            "staging attempt leaf became unsafe",
        ));
    }
    let parent = target.path.parent().ok_or_else(|| {
        output_attempt_cleanup_failure("staging attempt leaf parent is unavailable")
    })?;
    let name = target
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| output_attempt_cleanup_failure("staging attempt leaf name is invalid"))?;
    let observer = open_regular_leaf_observer(parent, name)?;
    if stable_identity(&observer).map_err(|_| {
        output_attempt_cleanup_failure("staging attempt leaf identity is unavailable")
    })? != target.identity
    {
        return Err(output_attempt_cleanup_failure(
            "staging attempt leaf identity changed",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn output_attempt_cleanup_failure(reason: &str) -> SafeOutputError {
    SafeOutputError::OutputAttemptCleanup {
        reason: reason.to_string(),
    }
}

#[cfg(windows)]
impl HelperTempCleanupRecovery {
    /// Revalidate and, when safe, finish deletion of the retained helper
    /// workspace.  A failure leaves this object and all unconsumed handles
    /// intact so the owner can retry without rediscovering the root.
    pub(crate) fn verify_cleanup(&mut self) -> Result<(), SafeOutputError> {
        #[cfg(any(test, feature = "youtube-process-test"))]
        let mut fault = self.test_fail_after;
        #[cfg(not(any(test, feature = "youtube-process-test")))]
        let mut fault = 0u64;
        let result = self.verify_cleanup_inner(&mut fault);
        #[cfg(any(test, feature = "youtube-process-test"))]
        {
            self.test_fail_after = fault;
        }
        result
    }

    fn verify_cleanup_inner(&mut self, fault: &mut u64) -> Result<(), SafeOutputError> {
        self.parent.revalidate().map_err(|_| {
            helper_temp_failure("application data directory changed during cleanup")
        })?;
        self.root.revalidate()?;
        if self.root.marked {
            if !self.root_absence_proven {
                self.verify_root_absent()?;
                self.root_absence_proven = true;
                self.plan.take();
            }
            return Ok(());
        }
        for child in &self.children {
            if !child.marked {
                child.revalidate()?;
                ensure_same_volume_target(child, &self.root)?;
            } else {
                validate_cleanup_target(child)?;
            }
        }

        if self.plan.is_none() {
            let mut entries = Vec::new();
            let mut budget = CleanupBudget::default();
            collect_helper_cleanup(
                &self.root.path,
                0,
                &mut budget,
                &mut entries,
                &self.children,
            )?;
            self.plan = Some(entries.into_iter().map(CleanupTarget::from_entry).collect());
        }

        self.validate_plan()?;

        if let Some(plan) = self.plan.as_mut() {
            for target in plan.iter_mut() {
                if target.marked {
                    continue;
                }
                let handle = target.handle.as_ref().ok_or_else(|| {
                    helper_temp_failure("unmarked helper cleanup target handle is unavailable")
                })?;
                mark_delete_for_cleanup(handle, fault).map_err(|_| {
                    helper_temp_failure(
                        "helper workspace cleanup could not delete a verified entry",
                    )
                })?;
                target.marked = true;
                target.handle = None;
            }
        }
        for child in &mut self.children {
            if child.marked {
                continue;
            }
            let handle = child.handle.as_ref().ok_or_else(|| {
                helper_temp_failure("unmarked fixed helper child handle is unavailable")
            })?;
            mark_delete_for_cleanup(handle, fault).map_err(|_| {
                helper_temp_failure("helper workspace cleanup could not delete a fixed child")
            })?;
            child.marked = true;
            child.handle = None;
        }

        wait_marked_helper_paths_absent(self.plan.as_deref(), &self.children);

        self.parent.revalidate().map_err(|_| {
            helper_temp_failure("application data directory changed during cleanup")
        })?;
        self.root.revalidate()?;
        let root_handle = self.root.handle.take().ok_or_else(|| {
            helper_temp_failure("unmarked helper cleanup root handle is unavailable")
        })?;
        if mark_delete_root_for_cleanup(&root_handle, fault).is_err() {
            self.root.handle = Some(root_handle);
            return Err(helper_temp_failure(
                "helper workspace cleanup could not delete the launch root",
            ));
        }
        drop(root_handle);
        self.root.marked = true;
        self.verify_root_absent()?;
        self.root_absence_proven = true;
        // Keep the marked plan through child/root failures so a same-name
        // replacement can never become a fresh target on retry.  Once the
        // root disposition succeeds and its namespace is proven absent, no
        // further retry can be needed.
        self.plan.take();
        Ok(())
    }

    fn verify_root_absent(&self) -> Result<(), SafeOutputError> {
        let mut last = helper_temp_failure("helper workspace root remained visible after deletion");
        for attempt in 0..HELPER_TEMP_ABSENCE_ATTEMPTS {
            match self.observe_root_absence() {
                Ok(()) => return Ok(()),
                Err(error) => {
                    last = error;
                    if attempt + 1 < HELPER_TEMP_ABSENCE_ATTEMPTS {
                        thread::sleep(HELPER_TEMP_ABSENCE_RETRY_DELAY);
                    }
                }
            }
        }
        Err(last)
    }

    fn observe_root_absence(&self) -> Result<(), SafeOutputError> {
        match fs::symlink_metadata(&self.root.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(helper_temp_failure(
                    "helper workspace root absence could not be verified",
                ))
            }
            Ok(metadata) => {
                if is_reparse_point(&metadata) || !metadata.file_type().is_dir() {
                    return Err(helper_temp_failure(
                        "helper workspace root was replaced by an unsafe object",
                    ));
                }
            }
        }
        let observer = open_directory_nofollow(&self.root.path, false).map_err(|_| {
            helper_temp_failure("helper workspace root identity could not be observed")
        })?;
        let metadata = observer.metadata().map_err(|_| {
            helper_temp_failure("helper workspace root identity could not be observed")
        })?;
        let identity = stable_identity(&observer).map_err(|_| {
            helper_temp_failure("helper workspace root identity could not be observed")
        })?;
        if is_untrusted_directory(&metadata) || identity != self.root.identity {
            return Err(helper_temp_failure(
                "helper workspace root identity changed before absence was proven",
            ));
        }
        Err(helper_temp_failure(
            "helper workspace root remained visible after deletion",
        ))
    }

    fn validate_plan(&self) -> Result<(), SafeOutputError> {
        let plan = self
            .plan
            .as_ref()
            .ok_or_else(|| helper_temp_failure("helper cleanup plan is unavailable"))?;
        for target in plan {
            validate_cleanup_target(target)?;
        }
        for child in &self.children {
            validate_cleanup_target(child)?;
        }
        let mut budget = CleanupBudget::default();
        validate_cleanup_namespace(&self.root.path, 0, &mut budget, plan, &self.children)?;
        Ok(())
    }
}

#[cfg(windows)]
fn collect_helper_cleanup<H: HelperHeldDirectory>(
    directory: &Path,
    depth: usize,
    budget: &mut CleanupBudget,
    plan: &mut Vec<CleanupEntry>,
    held_children: &[H],
) -> Result<(), SafeOutputError> {
    if depth > MAX_HELPER_CLEANUP_DEPTH {
        return Err(helper_temp_failure("helper cleanup depth limit exceeded"));
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|_| helper_temp_failure("helper cleanup directory could not be enumerated"))?
    {
        budget.nodes = budget.nodes.saturating_add(1);
        if budget.nodes > MAX_HELPER_CLEANUP_NODES {
            return Err(helper_temp_failure("helper cleanup node limit exceeded"));
        }
        entries.push(
            entry.map_err(|_| {
                helper_temp_failure("helper cleanup directory entry was unreadable")
            })?,
        );
    }
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| helper_temp_failure("helper cleanup entry name is not UTF-8"))?;
        if name.encode_utf16().count() > MAX_HELPER_CLEANUP_NAME_UTF16 {
            return Err(helper_temp_failure("helper cleanup entry name is too long"));
        }
        validate_component(name)
            .map_err(|_| helper_temp_failure("helper cleanup entry name is invalid"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| helper_temp_failure("helper cleanup entry disappeared"))?;
        if is_reparse_point(&metadata) {
            return Err(helper_temp_failure(
                "helper cleanup encountered a reparse point",
            ));
        }
        if metadata.file_type().is_dir() {
            if depth == 0 {
                if let Some(held) = held_children.iter().find(|child| child.path() == path) {
                    held.revalidate()?;
                    collect_helper_cleanup(&path, depth + 1, budget, plan, held_children)?;
                    continue;
                }
            }
            let handle = open_directory_guard(&path, true, true)
                .map_err(|_| helper_temp_failure("helper cleanup directory handle failed"))?;
            let handle_metadata = handle
                .metadata()
                .map_err(|_| helper_temp_failure("helper cleanup directory identity failed"))?;
            if is_untrusted_directory(&handle_metadata) {
                return Err(helper_temp_failure(
                    "helper cleanup directory handle is unsafe",
                ));
            }
            let identity = stable_identity(&handle)
                .map_err(|_| helper_temp_failure("helper cleanup directory identity failed"))?;
            let observer = open_directory_nofollow(&path, false)
                .map_err(|_| helper_temp_failure("helper cleanup directory was replaced"))?;
            if is_untrusted_directory(
                &observer
                    .metadata()
                    .map_err(|_| helper_temp_failure("helper cleanup directory identity failed"))?,
            ) || stable_identity(&observer)
                .map_err(|_| helper_temp_failure("helper cleanup directory identity failed"))?
                != identity
            {
                return Err(helper_temp_failure(
                    "helper cleanup directory identity changed",
                ));
            }
            collect_helper_cleanup(&path, depth + 1, budget, plan, held_children)?;
            plan.push(CleanupEntry {
                path,
                handle,
                identity,
                is_directory: true,
            });
        } else if metadata.file_type().is_file() {
            let handle = open_regular_leaf_for_delete(directory, name)?;
            let identity = stable_identity(&handle)
                .map_err(|_| helper_temp_failure("helper cleanup file identity failed"))?;
            plan.push(CleanupEntry {
                path,
                handle,
                identity,
                is_directory: false,
            });
        } else {
            return Err(helper_temp_failure(
                "helper cleanup encountered an unsupported filesystem object",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn revalidate_directory_parts(
    path: &Path,
    handle: &File,
    identity: StableIdentity,
) -> Result<(), SafeOutputError> {
    if is_untrusted_directory(&handle.metadata()?) {
        return Err(SafeOutputError::UnsafeDescendant {
            path: path.to_path_buf(),
        });
    }
    let observer =
        open_directory_nofollow(path, false).map_err(|_| SafeOutputError::IdentityChanged {
            path: path.to_path_buf(),
        })?;
    if is_untrusted_directory(&observer.metadata()?) || stable_identity(&observer)? != identity {
        return Err(SafeOutputError::IdentityChanged {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_same_volume_target(
    left: &CleanupTarget,
    right: &CleanupTarget,
) -> Result<(), SafeOutputError> {
    if left.identity.volume_serial_number != right.identity.volume_serial_number {
        return Err(helper_temp_failure(
            "helper directory volume differs from the application volume",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_cleanup_target(target: &CleanupTarget) -> Result<(), SafeOutputError> {
    let metadata = match fs::symlink_metadata(&target.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && target.marked => {
            return Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(helper_temp_failure(
                "unmarked helper cleanup target disappeared",
            ));
        }
        Err(error) => return Err(SafeOutputError::Io(error)),
    };
    if is_reparse_point(&metadata) {
        return Err(helper_temp_failure(
            "helper cleanup encountered a reparse point",
        ));
    }
    if target.is_directory != metadata.file_type().is_dir() {
        return Err(helper_temp_failure("helper cleanup target type changed"));
    }
    let observed = if target.is_directory {
        open_directory_nofollow(&target.path, false)
            .map_err(|_| helper_temp_failure("helper cleanup directory identity failed"))?
    } else {
        let parent = target
            .path
            .parent()
            .ok_or_else(|| helper_temp_failure("helper cleanup file parent is unavailable"))?;
        let name = target
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| helper_temp_failure("helper cleanup file name is invalid"))?;
        open_regular_leaf_observer(parent, name)?
    };
    let observed_metadata = observed.metadata()?;
    if target.is_directory && is_untrusted_directory(&observed_metadata) {
        return Err(helper_temp_failure(
            "helper cleanup target is not a directory",
        ));
    }
    if target.is_directory != observed_metadata.file_type().is_dir()
        || is_reparse_point(&observed_metadata)
        || stable_identity(&observed)? != target.identity
    {
        return Err(helper_temp_failure(
            "helper cleanup target identity changed",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_cleanup_namespace(
    directory: &Path,
    depth: usize,
    budget: &mut CleanupBudget,
    plan: &[CleanupTarget],
    children: &[CleanupTarget],
) -> Result<(), SafeOutputError> {
    if depth > MAX_HELPER_CLEANUP_DEPTH {
        return Err(helper_temp_failure("helper cleanup depth limit exceeded"));
    }
    let entries = fs::read_dir(directory)
        .map_err(|_| helper_temp_failure("helper cleanup directory could not be enumerated"))?;
    for entry in entries {
        let entry = entry
            .map_err(|_| helper_temp_failure("helper cleanup directory entry was unreadable"))?;
        budget.nodes = budget.nodes.saturating_add(1);
        if budget.nodes > MAX_HELPER_CLEANUP_NODES {
            return Err(helper_temp_failure("helper cleanup node limit exceeded"));
        }
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| helper_temp_failure("helper cleanup entry name is not UTF-8"))?;
        if name.encode_utf16().count() > MAX_HELPER_CLEANUP_NAME_UTF16 {
            return Err(helper_temp_failure("helper cleanup entry name is too long"));
        }
        validate_component(name)
            .map_err(|_| helper_temp_failure("helper cleanup entry name is invalid"))?;
        let path = entry.path();
        let expected = plan
            .iter()
            .find(|target| target.path == path)
            .or_else(|| children.iter().find(|target| target.path == path))
            .ok_or_else(|| helper_temp_failure("helper cleanup found an unplanned entry"))?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| helper_temp_failure("helper cleanup entry disappeared"))?;
        if is_reparse_point(&metadata) {
            return Err(helper_temp_failure(
                "helper cleanup encountered a reparse point",
            ));
        }
        if expected.is_directory != metadata.file_type().is_dir() {
            return Err(helper_temp_failure("helper cleanup entry type changed"));
        }
        if metadata.file_type().is_dir() {
            validate_cleanup_namespace(&path, depth + 1, budget, plan, children)?;
        }
    }
    Ok(())
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
        const FILE_DELETE_CHILD: u32 = 0x0040;
        options
            .access_mode(
                FILE_LIST_DIRECTORY
                    | FILE_READ_ATTRIBUTES
                    | if delete_access { DELETE } else { 0 }
                    | if add_child_access {
                        FILE_ADD_SUBDIRECTORY | FILE_DELETE_CHILD
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
fn open_regular_leaf_observer(directory: &Path, name: &str) -> Result<File, SafeOutputError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let path = directory.join(name);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options
        .open(&path)
        .map_err(|_| helper_temp_failure("helper cleanup file identity could not be observed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| helper_temp_failure("helper cleanup file identity could not be observed"))?;
    if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
        return Err(SafeOutputError::UnsafeDescendant { path });
    }
    Ok(file)
}

#[cfg(windows)]
fn mark_delete_by_handle_once(file: &File) -> std::io::Result<()> {
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

#[cfg(windows)]
fn mark_delete_for_cleanup(file: &File, fault: &mut u64) -> std::io::Result<()> {
    consume_cleanup_fault(fault)?;
    mark_delete_helper_entry(file)
}

#[cfg(windows)]
fn mark_delete_root_for_cleanup(file: &File, fault: &mut u64) -> std::io::Result<()> {
    consume_cleanup_fault(fault)?;
    mark_delete_root_by_posix_handle(file)
}

#[cfg(windows)]
fn consume_cleanup_fault(fault: &mut u64) -> std::io::Result<()> {
    #[cfg(any(test, feature = "youtube-process-test"))]
    {
        if *fault == 0 {
            *fault = u64::MAX;
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "deterministic helper cleanup test failure",
            ));
        }
        if *fault != u64::MAX {
            *fault = (*fault).saturating_sub(1);
        }
    }
    #[cfg(not(any(test, feature = "youtube-process-test")))]
    let _ = fault;
    Ok(())
}

#[cfg(windows)]
fn mark_delete_root_by_posix_handle(file: &File) -> std::io::Result<()> {
    mark_delete_helper_entry(file)
}

#[cfg(windows)]
fn mark_delete_helper_entry(file: &File) -> std::io::Result<()> {
    match mark_delete_root_by_posix_handle_once(file) {
        Ok(()) => Ok(()),
        Err(_) => retry_win_io(|| mark_delete_by_handle_once(file)),
    }
}

#[cfg(windows)]
fn mark_delete_root_by_posix_handle_once(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfoEx, SetFileInformationByHandle, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX,
    };
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    let result = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle().cast(),
            FileDispositionInfoEx,
            (&info as *const FILE_DISPOSITION_INFO_EX).cast(),
            u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO_EX>())
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

fn compact_staging_scope(occurrence_id: &str, artifact_fingerprint: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((occurrence_id.len() as u64).to_le_bytes());
    hasher.update(occurrence_id.as_bytes());
    hasher.update((artifact_fingerprint.len() as u64).to_le_bytes());
    hasher.update(artifact_fingerprint.as_bytes());
    let digest = hasher.finalize();
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn validate_root_shape(path: &Path) -> Result<(), SafeOutputError> {
    for component in path.components() {
        match component {
            Component::ParentDir | Component::CurDir => {
                return Err(SafeOutputError::UntrustedDirectory {
                    path: path.to_path_buf(),
                });
            }
            #[cfg(windows)]
            Component::Prefix(prefix) => {
                use std::path::Prefix;
                if !matches!(prefix.kind(), Prefix::Disk(_)) {
                    return Err(SafeOutputError::UntrustedDirectory {
                        path: path.to_path_buf(),
                    });
                }
            }
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                validate_component(&name)?;
            }
            Component::RootDir => {}
            #[cfg(not(windows))]
            Component::Prefix(_) => {}
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
    static NEXT_PROBE: AtomicU64 = AtomicU64::new(1);
    let probe = path.join(format!(
        ".linkvault-youtube-write-probe-{}-{}-{}",
        std::process::id(),
        now_nanos(),
        NEXT_PROBE.fetch_add(1, Ordering::Relaxed)
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
    fn compact_staging_namespace_supports_deep_output_roots() {
        let temp = tempdir().unwrap();
        let base_units = path_utf16_len(temp.path()) + 1;
        let component_units = 150usize.saturating_sub(base_units).max(1);
        let deep_root = temp.path().join("r".repeat(component_units));
        fs::create_dir(&deep_root).unwrap();
        let root = validate_output_root(&deep_root).unwrap();
        let attempt = root
            .staging_attempt_lease(&"o".repeat(64), &"f".repeat(64))
            .unwrap();
        assert!(path_utf16_len(attempt.path()) <= MAX_HELPER_STAGING_DIR_UTF16);
        root.discard_attempt_lease(attempt).unwrap();
    }

    #[test]
    fn rejects_relative_file_and_parent_dir_roots() {
        assert!(validate_output_root(Path::new("relative")).is_err());
        let temp = tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, b"not-a-directory").unwrap();
        assert!(validate_output_root(&file).is_err());
        let with_parent = temp.path().join("child").join("..").join("sibling");
        let _ = fs::create_dir_all(temp.path().join("sibling"));
        assert!(
            validate_output_root(&with_parent).is_err(),
            "output roots must reject ParentDir components before canonicalize"
        );
        #[cfg(windows)]
        {
            let traversal = PathBuf::from(r"C:\Users\Public\Videos\..\..\Windows");
            assert!(
                validate_root_shape(&traversal).is_err(),
                "shape validation must reject .. components"
            );
        }
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
            Err(SafeOutputError::OutputAttemptCleanupPermanentUnproven { .. })
        ));
        assert!(attempt_path.exists());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }

    #[cfg(windows)]
    #[test]
    fn staging_attempt_recovery_retries_after_partial_leaf_deletion() {
        let temp = tempdir().unwrap();
        let sentinel = temp.path().join("outside-sentinel.txt");
        fs::write(&sentinel, b"keep").unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let attempt_path = attempt.path().to_path_buf();
        fs::write(attempt.path().join("a.partial"), b"a").unwrap();
        fs::write(attempt.path().join("b.partial"), b"b").unwrap();

        let mut recovery = OutputAttemptCleanupRecovery::from_attempt(attempt);
        recovery.set_cleanup_fault_after(1);
        assert!(recovery.verify_cleanup().is_err());
        let plan = recovery.plan.as_ref().unwrap();
        assert_eq!(plan.iter().filter(|target| target.marked).count(), 1);
        assert!(plan.iter().any(|target| !target.marked));
        recovery.set_cleanup_fault_after(u64::MAX);
        recovery.verify_cleanup().unwrap();
        assert!(!attempt_path.exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }

    #[cfg(windows)]
    #[test]
    fn staging_attempt_recovery_retries_root_disposition_with_extra_handle() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let attempt_path = attempt.path().to_path_buf();
        let external = open_directory_nofollow(&attempt_path, false).unwrap();

        let mut recovery = OutputAttemptCleanupRecovery::from_attempt(attempt);
        recovery.set_cleanup_fault_after(0);
        assert!(recovery.verify_cleanup().is_err());
        assert!(recovery.plan.is_some());
        assert!(!recovery.attempt.marked);
        assert!(recovery.attempt.handle.is_some());

        recovery.set_cleanup_fault_after(u64::MAX);
        recovery.verify_cleanup().unwrap();
        assert!(!attempt_path.exists());
        drop(external);
    }

    #[cfg(windows)]
    #[test]
    fn staging_attempt_recovery_rejects_missing_or_replaced_unmarked_leaf() {
        for replacement in [false, true] {
            let temp = tempdir().unwrap();
            let sentinel = temp.path().join("outside-sentinel.txt");
            fs::write(&sentinel, b"keep").unwrap();
            let root = validate_output_root(temp.path()).unwrap();
            let attempt = root
                .staging_attempt_lease("occurrence-1", "artifact-1")
                .unwrap();
            let attempt_path = attempt.path().to_path_buf();
            let leaf = attempt.path().join("artifact.partial");
            fs::write(&leaf, b"original").unwrap();
            let mut recovery = OutputAttemptCleanupRecovery::from_attempt(attempt);
            recovery.set_cleanup_fault_after(0);
            recovery.verify_cleanup().unwrap_err();
            let target = recovery
                .plan
                .as_mut()
                .unwrap()
                .iter_mut()
                .find(|target| !target.marked)
                .unwrap();
            target.handle.take();
            fs::remove_file(&leaf).unwrap();
            if replacement {
                fs::write(&leaf, b"replacement").unwrap();
            }
            recovery.set_cleanup_fault_after(u64::MAX);
            assert!(recovery.verify_cleanup().is_err());
            assert!(attempt_path.exists());
            assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
        }
    }

    #[cfg(windows)]
    #[test]
    fn staging_attempt_recovery_rejects_unexpected_new_child() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let attempt_path = attempt.path().to_path_buf();
        fs::write(attempt.path().join("known.partial"), b"known").unwrap();
        let mut recovery = OutputAttemptCleanupRecovery::from_attempt(attempt);
        recovery.set_cleanup_fault_after(0);
        recovery.verify_cleanup().unwrap_err();
        fs::write(&attempt_path.join("unexpected.partial"), b"must survive").unwrap();
        recovery.set_cleanup_fault_after(u64::MAX);
        assert!(recovery.verify_cleanup().is_err());
        assert!(attempt_path.join("unexpected.partial").exists());
        assert!(attempt_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn staging_attempt_plan_bound_is_permanent_and_never_replanned() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_lease("occurrence-1", "artifact-1")
            .unwrap();
        let attempt_path = attempt.path().to_path_buf();
        for index in 0..=MAX_HELPER_CLEANUP_NODES {
            fs::write(
                attempt.path().join(format!("node-{index:04}.partial")),
                b"must survive",
            )
            .unwrap();
        }

        let error = root
            .discard_attempt_lease_recoverable(attempt)
            .expect_err("over-bound attempt must not be discarded");
        let mut recovery = match error {
            OutputAttemptDiscardError::Permanent { recovery, .. } => *recovery,
            OutputAttemptDiscardError::Recoverable { .. } => {
                panic!("incomplete attempt plan was classified as recoverable")
            }
        };
        assert!(recovery.plan.is_none());
        assert!(recovery.planning_failed);
        fs::write(attempt_path.join("late-child.partial"), b"must survive").unwrap();
        assert!(recovery.verify_cleanup().is_err());
        assert!(recovery.plan.is_none());

        drop(recovery);
        for entry in fs::read_dir(&attempt_path).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::remove_dir(attempt_path).unwrap();
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
        let staging = output.join(".lv");
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

    #[cfg(windows)]
    #[test]
    fn helper_temp_admission_keeps_a_path_free_reason() {
        let error = HelperTempCapability::create_for_test(Path::new("relative"))
            .expect_err("a relative parent must fail closed");
        match error {
            SafeOutputError::HelperTemp { reason } => {
                assert!(
                    reason.contains("absolute") || reason.contains("trusted"),
                    "admission reason should describe the failure: {reason}"
                );
                assert!(
                    !reason.contains('\\') && !reason.contains('/'),
                    "admission reason must not include a filesystem path: {reason}"
                );
            }
            other => panic!("expected HelperTemp, got {other}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn helper_capabilities_use_distinct_exclusive_roots_and_contained_children() {
        let temp = tempdir().unwrap();
        let first = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let second = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let first_root = first.root_path().to_path_buf();
        let second_root = second.root_path().to_path_buf();

        assert_ne!(first_root, second_root);
        assert!(first_root.starts_with(temp.path()));
        assert!(second_root.starts_with(temp.path()));
        first.revalidate().unwrap();
        second.revalidate().unwrap();

        for (_, directory) in HELPER_TEMP_CHILDREN {
            let first_child = first.child_path(directory);
            let second_child = second.child_path(directory);
            assert!(first_child.starts_with(&first_root));
            assert!(second_child.starts_with(&second_root));
            assert_eq!(first_child.parent(), Some(first_root.as_path()));
            assert_eq!(second_child.parent(), Some(second_root.as_path()));
            assert_ne!(first_child, second_child);
        }

        first.cleanup().unwrap();
        second.cleanup().unwrap();
        assert!(!first_root.exists());
        assert!(!second_root.exists());
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_removes_a_bounded_tree_without_touching_parent_sentinel() {
        let temp = tempdir().unwrap();
        let sentinel = temp.path().join("outside-sentinel.txt");
        fs::write(&sentinel, b"keep").unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        let nested = capability
            .child_path(HelperTempDirectory::Cache)
            .join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("artifact.bin"), b"temporary").unwrap();
        fs::write(root.join("root-marker.txt"), b"temporary").unwrap();

        capability.cleanup().unwrap();

        assert!(!root.exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_proves_root_absence_with_a_compatible_external_handle() {
        let temp = tempdir().unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        let external = open_directory_nofollow(&root, false).unwrap();
        let mut recovery = capability.cleanup_recovery();

        recovery.verify_cleanup().unwrap();
        assert!(matches!(
            fs::symlink_metadata(&root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ));

        drop(external);
    }

    #[cfg(windows)]
    #[test]
    fn helper_creation_failure_preserves_or_rejects_cleanup_recovery_by_identity() {
        let temp = tempdir().unwrap();
        let result = HelperTempCapability::create_for_test_creation_failure(temp.path(), true);
        let mut recovery = match result {
            Err(SafeOutputError::HelperTempCleanupUnproven { recovery, .. }) => *recovery,
            Err(SafeOutputError::HelperTemp { reason }) => {
                panic!("post-root failure was incorrectly reported as admission: {reason}")
            }
            Err(SafeOutputError::HelperTempCleanupUnprovenNoRecovery { .. }) => {
                panic!("post-root failure lost the retained cleanup root")
            }
            Err(error) => panic!("unexpected post-root creation error: {error}"),
            Ok(_) => panic!("the deterministic post-root failure did not fire"),
        };

        assert!(recovery.root.handle.is_some());
        recovery.verify_cleanup().unwrap();
        assert!(!recovery.root.path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn helper_creation_failure_before_root_acquisition_is_unrecoverable() {
        let temp = tempdir().unwrap();
        let result = HelperTempCapability::create_for_test_creation_failure(temp.path(), false);
        assert!(matches!(
            result,
            Err(SafeOutputError::HelperTempCleanupUnprovenNoRecovery { .. })
        ));

        let parent = temp.path().join(HELPER_TEMP_PARENT_NAME);
        for entry in fs::read_dir(parent).unwrap() {
            fs::remove_dir_all(entry.unwrap().path()).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn held_helper_directory_handles_deny_root_and_child_rename_replacement() {
        let temp = tempdir().unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        let child = capability
            .child_path(HelperTempDirectory::Cache)
            .to_path_buf();
        let moved_root = temp.path().join("moved-helper-root");
        let moved_child = root.join("moved-cache");

        assert!(fs::rename(&root, &moved_root).is_err());
        assert!(fs::rename(&child, &moved_child).is_err());
        assert!(!moved_root.exists());
        assert!(!moved_child.exists());
        capability.revalidate().unwrap();
        capability.cleanup().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_rejects_nested_reparse_and_preserves_outside_sentinel() {
        use std::os::windows::fs::symlink_dir;

        let temp = tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let sentinel = outside.join("sentinel.txt");
        fs::write(&sentinel, b"must survive").unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        let nested = capability
            .child_path(HelperTempDirectory::Cache)
            .join("nested");
        fs::create_dir(&nested).unwrap();
        let link = nested.join("escape");
        match symlink_dir(&outside, &link) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    || error.kind() == std::io::ErrorKind::Unsupported
                    || matches!(error.raw_os_error(), Some(5 | 50 | 1314)) =>
            {
                eprintln!("skipping reparse cleanup test: symlink privilege unavailable: {error}");
                fs::remove_dir(&nested).unwrap();
                capability.cleanup().unwrap();
                return;
            }
            Err(error) => panic!("creating the reparse fixture failed unexpectedly: {error}"),
        }

        let error = capability
            .cleanup()
            .expect_err("reparse descendants must fail closed before deletion");
        assert!(matches!(error, SafeOutputError::HelperTemp { .. }));
        assert!(root.exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"must survive");
        fs::remove_dir(&link).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_recovery_retries_after_leaf_child_and_root_failures() {
        for (fail_after, leaf_count) in [(1u64, 2usize), (0, 0), (5, 0)] {
            let temp = tempdir().unwrap();
            let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
            let root = capability.root_path().to_path_buf();
            for index in 0..leaf_count {
                fs::write(root.join(format!("leaf-{index}.tmp")), b"partial").unwrap();
            }

            let mut recovery = capability.cleanup_recovery();
            recovery.test_fail_after = fail_after;
            assert!(matches!(
                recovery.verify_cleanup(),
                Err(SafeOutputError::HelperTemp { .. })
            ));
            assert!(root.exists());

            recovery.verify_cleanup().unwrap();
            drop(recovery);
            assert!(!root.exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_recovery_allows_marked_missing_but_rejects_unmarked_missing() {
        let temp = tempdir().unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        fs::write(root.join("a-leaf.tmp"), b"partial").unwrap();
        fs::write(root.join("b-leaf.tmp"), b"partial").unwrap();

        let mut recovery = capability.cleanup_recovery();
        recovery.test_fail_after = 1;
        recovery.verify_cleanup().unwrap_err();

        let marked_path = recovery
            .plan
            .as_ref()
            .unwrap()
            .iter()
            .find(|target| target.marked)
            .unwrap()
            .path
            .clone();
        if marked_path.exists() {
            fs::remove_file(&marked_path).unwrap();
        }
        recovery.verify_cleanup().unwrap();
        drop(recovery);
        assert!(!root.exists());

        let temp = tempdir().unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        fs::write(root.join("unmarked-a.tmp"), b"partial").unwrap();
        fs::write(root.join("unmarked-b.tmp"), b"partial").unwrap();
        let mut recovery = capability.cleanup_recovery();
        recovery.test_fail_after = 1;
        recovery.verify_cleanup().unwrap_err();
        let target = recovery
            .plan
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|target| !target.marked)
            .unwrap();
        let unmarked_path = target.path.clone();
        target.handle = None;
        fs::remove_file(&unmarked_path).unwrap();
        assert!(matches!(
            recovery.verify_cleanup(),
            Err(SafeOutputError::HelperTemp { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_recovery_rejects_same_name_replacement_after_child_or_root_failure() {
        for fail_after in [1u64, 6u64] {
            let temp = tempdir().unwrap();
            let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
            let root = capability.root_path().to_path_buf();
            let leaf = root.join("replacement.tmp");
            fs::write(&leaf, b"original").unwrap();
            let mut recovery = capability.cleanup_recovery();
            recovery.test_fail_after = fail_after;
            recovery.verify_cleanup().unwrap_err();

            let marked_path = recovery
                .plan
                .as_ref()
                .unwrap()
                .iter()
                .find(|target| target.marked)
                .unwrap()
                .path
                .clone();
            if marked_path.exists() {
                fs::remove_file(&marked_path).unwrap();
            }
            fs::write(&marked_path, b"replacement").unwrap();
            assert!(matches!(
                recovery.verify_cleanup(),
                Err(SafeOutputError::HelperTemp { .. })
            ));
            assert!(root.exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_recovery_never_rediscovers_a_lost_root_handle() {
        let temp = tempdir().unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        let mut recovery = capability.cleanup_recovery();
        recovery.root.handle = None;

        assert!(matches!(
            recovery.verify_cleanup(),
            Err(SafeOutputError::HelperTemp { .. })
        ));
        assert!(root.exists());
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_recovery_rejects_fixed_child_reparse_then_can_retry() {
        use std::os::windows::fs::symlink_dir;

        let temp = tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        let nested = capability
            .child_path(HelperTempDirectory::Cache)
            .join("nested");
        fs::create_dir(&nested).unwrap();
        let link = nested.join("escape");
        match symlink_dir(&outside, &link) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    || error.kind() == std::io::ErrorKind::Unsupported
                    || matches!(error.raw_os_error(), Some(5 | 50 | 1314)) =>
            {
                eprintln!(
                    "skipping fixed-child reparse test: symlink privilege unavailable: {error}"
                );
                fs::remove_dir(&nested).unwrap();
                capability.cleanup().unwrap();
                return;
            }
            Err(error) => panic!("creating the reparse fixture failed unexpectedly: {error}"),
        }

        let mut recovery = capability.cleanup_recovery();
        assert!(matches!(
            recovery.verify_cleanup(),
            Err(SafeOutputError::HelperTemp { .. })
        ));
        assert!(root.exists());
        fs::remove_dir(&link).unwrap();
        recovery.verify_cleanup().unwrap();
        drop(recovery);
        assert!(!root.exists());
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_node_bound_rejects_before_retaining_unbounded_plan() {
        let temp = tempdir().unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        for index in 0..=MAX_HELPER_CLEANUP_NODES {
            fs::write(root.join(format!("node-{index:04}.tmp")), b"must survive").unwrap();
        }

        let mut recovery = capability.cleanup_recovery();
        let error = recovery
            .verify_cleanup()
            .expect_err("cleanup must reject an over-bound direct entry set");
        assert!(matches!(error, SafeOutputError::HelperTemp { .. }));
        assert!(recovery.plan.is_none());
        let direct_entries = fs::read_dir(&root).unwrap().count();
        assert!(direct_entries >= MAX_HELPER_CLEANUP_NODES + 1);

        for entry in fs::read_dir(&root).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy().starts_with("node-") {
                fs::remove_file(entry.path()).unwrap();
            }
        }
        recovery.verify_cleanup().unwrap();
        drop(recovery);
        assert!(!root.exists());
    }

    #[cfg(windows)]
    #[test]
    fn helper_cleanup_depth_bound_fails_closed_without_mutating_tree() {
        let temp = tempdir().unwrap();
        let capability = HelperTempCapability::create_for_test(temp.path()).unwrap();
        let root = capability.root_path().to_path_buf();
        let sentinel = root.join("root-sentinel.txt");
        fs::write(&sentinel, b"must survive").unwrap();
        let mut deepest = capability
            .child_path(HelperTempDirectory::Cache)
            .to_path_buf();
        for index in 0..MAX_HELPER_CLEANUP_DEPTH {
            deepest = deepest.join(format!("depth-{index}"));
            fs::create_dir(&deepest).unwrap();
        }
        fs::write(deepest.join("leaf.txt"), b"must survive").unwrap();

        let error = capability
            .cleanup()
            .expect_err("cleanup must reject a tree deeper than its bound");
        assert!(matches!(error, SafeOutputError::HelperTemp { .. }));
        assert!(root.exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"must survive");
        assert_eq!(fs::read(deepest.join("leaf.txt")).unwrap(), b"must survive");
    }
}
