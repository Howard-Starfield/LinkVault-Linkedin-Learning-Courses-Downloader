//! Resumable YouTube staging directories.
//!
//! A staging directory is named deterministically from
//! `(occurrence_id, artifact_fingerprint)`, so a resumed attempt finds the
//! `.part`/`.ytdl` files the interrupted one left and yt-dlp `--continue`
//! continues from the right byte. The artifact fingerprint already covers
//! the mode, the format-selection policy and height cap, the transcript
//! choice and the helper lock digest, so a changed request never reuses
//! another request's bytes.
//!
//! Every safety rule of ordinary staging still applies: each directory is
//! opened with no-follow guards and a recorded stable identity, content is
//! validated before reuse (flat, regular files named `media.*`), and
//! anything unexpected is deleted through the identity-held cleanup plan and
//! the attempt starts fresh.
//!
//! Exclusive use: [`StagingClaim`] is held by the lease. It combines an
//! in-process registry (two runs or attempts in this process can never
//! share a directory) with, on Windows, a lock file opened with no sharing
//! and delete-on-close (a second process cannot share it either, and a
//! crash cannot leave a stale lock behind).

use super::*;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// Directory under the output root that holds staging directories.
pub const STAGING_DIR_NAME: &str = ".lv";
/// Staging directories and lock files this app creates start with this.
pub const STAGING_NAME_PREFIX: &str = "y-";
/// Unreferenced staging older than this is swept at startup.
pub const ORPHAN_MIN_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LeaseMode {
    CreateNew,
    OpenExisting,
}

/// How a resumable staging directory was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingDisposition {
    /// No earlier directory existed.
    Fresh,
    /// An earlier directory passed validation; `preserved_bytes` are kept.
    Reused { preserved_bytes: u64 },
    /// An earlier directory had unexpected contents; it was deleted and a
    /// fresh one created.
    ReplacedUnexpected,
    /// The deterministic name could not be used safely (for example a link
    /// stands there) so a one-off unique directory was used. It is never
    /// resumed.
    Unresumable,
}

#[derive(Debug)]
pub struct ResumableStaging {
    pub lease: OutputAttemptLease,
    pub disposition: StagingDisposition,
}

fn claims() -> &'static Mutex<HashSet<String>> {
    static CLAIMS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CLAIMS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Exclusive hold on one staging directory name. Released on drop.
#[derive(Debug)]
pub struct StagingClaim {
    key: String,
    /// `.lv` guard handle and the lock file, in drop order: lock first.
    #[cfg(windows)]
    _lock: Option<File>,
    #[cfg(windows)]
    _parent_guard: Option<File>,
}

impl Drop for StagingClaim {
    fn drop(&mut self) {
        claims()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

fn claim_key(root: &ValidatedOutputRoot, name: &str) -> String {
    root.canonical_path()
        .join(STAGING_DIR_NAME)
        .join(name)
        .to_string_lossy()
        .to_lowercase()
}

/// True when a live claim covers `name` under `root`.
fn is_claimed(root: &ValidatedOutputRoot, name: &str) -> bool {
    claims()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&claim_key(root, name))
}

/// Open `<parent>/<name>.lock` exclusively and delete it when closed, but only
/// after proving the opened file really lives in `expected_parent`. If the
/// directory was swapped for a link between the check and the open, the file
/// is not followed into place: the mismatch is an error and nothing outside
/// the root is left armed for deletion by us.
#[cfg(windows)]
pub(super) fn open_verified_lock(
    parent: &Path,
    expected_parent: &Path,
    name: &str,
) -> Result<File, SafeOutputError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE,
    };
    const ERROR_SHARING_VIOLATION: i32 = 32;
    let path = parent.join(format!("{name}.lock"));
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .access_mode(FILE_GENERIC_WRITE | DELETE)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(|error| {
            if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
                SafeOutputError::StagingBusy { path: path.clone() }
            } else {
                SafeOutputError::Io(error)
            }
        })?;
    let metadata = file.metadata()?;
    if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
        return Err(SafeOutputError::UnsafeDescendant { path });
    }
    let actual = handle_final_path(&file)?;
    let in_place = actual
        .parent()
        .is_some_and(|actual_parent| same_path(actual_parent, expected_parent));
    if !in_place {
        // Only a file we just created (empty) may be removed again.
        if metadata.len() == 0 {
            let _ = mark_delete_by_handle_once(&file);
        }
        return Err(SafeOutputError::IdentityChanged { path });
    }
    // Deleted when the last handle (this one) closes, including on a crash.
    mark_delete_by_handle_once(&file)?;
    Ok(file)
}

#[cfg(windows)]
fn same_path(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        let text = path.to_string_lossy().to_lowercase();
        text.strip_prefix(r"\?\")
            .unwrap_or(&text)
            .trim_end_matches(char::from(92))
            .to_string()
    }
    normalized(left) == normalized(right)
}

/// Deterministic staging directory name for one work item.
pub fn resumable_staging_name(occurrence_id: &str, artifact_fingerprint: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"linkvault-youtube-resumable-staging-v1");
    hasher.update((occurrence_id.len() as u64).to_le_bytes());
    hasher.update(occurrence_id.as_bytes());
    hasher.update((artifact_fingerprint.len() as u64).to_le_bytes());
    hasher.update(artifact_fingerprint.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{STAGING_NAME_PREFIX}{hex}")
}

/// Whether a direct child of a resumable staging directory is something
/// yt-dlp could have written for the `media.%(ext)s` output template.
pub fn is_expected_staging_leaf(name: &str) -> bool {
    // `manifest.json` is derived output a failed finalization may have left;
    // the executor deletes it before regenerating.
    (name == "manifest.json"
        || (name.len() > "media.".len() && name.starts_with("media.")))
        && validate_component(name).is_ok()
}

/// Whether `newest_change` is old enough to sweep.
pub fn is_older_than(newest_change: SystemTime, now: SystemTime, min_age: Duration) -> bool {
    now.duration_since(newest_change)
        .map(|age| age >= min_age)
        .unwrap_or(false)
}

/// Why the sweep left an entry alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SweepKept {
    Referenced,
    InUse,
    Recent,
    NotOurs,
    Unsafe,
    CleanupFailed,
}

#[derive(Debug, Default)]
pub struct OrphanSweepReport {
    pub removed: Vec<String>,
    pub kept: Vec<(String, SweepKept)>,
}

impl ValidatedOutputRoot {
    fn claim_staging(&self, name: &str) -> Result<StagingClaim, SafeOutputError> {
        let key = claim_key(self, name);
        {
            let mut held = claims()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !held.insert(key.clone()) {
                return Err(SafeOutputError::StagingBusy {
                    path: self.path.join(STAGING_DIR_NAME).join(name),
                });
            }
        }
        // From here the claim owns the registry entry, so every early return
        // releases it through `Drop`.
        let mut claim = StagingClaim {
            key,
            #[cfg(windows)]
            _lock: None,
            #[cfg(windows)]
            _parent_guard: None,
        };
        #[cfg(windows)]
        {
            let (guard, lock) = self.open_staging_lock(name)?;
            claim._parent_guard = Some(guard);
            claim._lock = Some(lock);
        }
        #[cfg(not(windows))]
        let _ = &mut claim;
        Ok(claim)
    }

    #[cfg(windows)]
    fn open_staging_lock(&self, name: &str) -> Result<(File, File), SafeOutputError> {
        let parent = self.path.join(STAGING_DIR_NAME);
        match fs::create_dir(&parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        // Hold `.lv` open (no-follow, rename/delete denied) for as long as the
        // claim lives, then prove the handle is the real directory.
        let guard = open_directory_guard(&parent, false, false)?;
        if is_untrusted_directory(&guard.metadata()?) {
            return Err(SafeOutputError::UntrustedDirectory { path: parent });
        }
        let expected = self.canonical_path.join(STAGING_DIR_NAME);
        let lock = open_verified_lock(&parent, &expected, name)?;
        Ok((guard, lock))
    }

    /// Lease the deterministic staging directory for one work item,
    /// resuming an earlier attempt's files when they pass validation.
    pub fn resumable_staging_lease(
        &self,
        occurrence_id: &str,
        artifact_fingerprint: &str,
    ) -> Result<ResumableStaging, SafeOutputError> {
        validate_component(occurrence_id)?;
        validate_component(artifact_fingerprint)?;
        let name = resumable_staging_name(occurrence_id, artifact_fingerprint);
        let claim = self.claim_staging(&name)?;
        let existing = match self.lease_staging_dir(&name, LeaseMode::OpenExisting) {
            Ok(existing) => existing,
            // A link or other unsafe object stands at the deterministic
            // name. Never touch it; use a unique, non-resumable directory.
            Err(SafeOutputError::UnsafeDescendant { .. })
            | Err(SafeOutputError::UntrustedDirectory { .. }) => {
                let lease = self.staging_attempt_lease(occurrence_id, artifact_fingerprint)?;
                return Ok(ResumableStaging {
                    lease,
                    disposition: StagingDisposition::Unresumable,
                });
            }
            Err(error) => return Err(error),
        };
        let Some(mut existing) = existing else {
            return self.fresh_resumable(&name, claim, StagingDisposition::Fresh);
        };
        match inspect_staging(&existing) {
            Ok(preserved_bytes) => {
                existing.claim = Some(claim);
                Ok(ResumableStaging {
                    lease: existing,
                    disposition: StagingDisposition::Reused { preserved_bytes },
                })
            }
            Err(_) => {
                // Unexpected content: delete it through the identity-held
                // plan, then start over in a new directory of the same name.
                if self.discard_attempt_lease(existing).is_err() {
                    drop(claim);
                    let lease = self.staging_attempt_lease(occurrence_id, artifact_fingerprint)?;
                    return Ok(ResumableStaging {
                        lease,
                        disposition: StagingDisposition::Unresumable,
                    });
                }
                self.fresh_resumable(&name, claim, StagingDisposition::ReplacedUnexpected)
            }
        }
    }

    fn fresh_resumable(
        &self,
        name: &str,
        claim: StagingClaim,
        disposition: StagingDisposition,
    ) -> Result<ResumableStaging, SafeOutputError> {
        let mut lease = self
            .lease_staging_dir(name, LeaseMode::CreateNew)?
            .ok_or(SafeOutputError::EscapedRoot)?;
        lease.claim = Some(claim);
        Ok(ResumableStaging { lease, disposition })
    }

    /// Delete the resumable staging directory of one work item, if any.
    /// Returns `false` when nothing existed. Refuses (`StagingBusy`) while an
    /// attempt holds it.
    pub fn discard_resumable_staging(
        &self,
        occurrence_id: &str,
        artifact_fingerprint: &str,
    ) -> Result<bool, SafeOutputError> {
        validate_component(occurrence_id)?;
        validate_component(artifact_fingerprint)?;
        let name = resumable_staging_name(occurrence_id, artifact_fingerprint);
        let claim = self.claim_staging(&name)?;
        let Some(mut lease) = self.lease_staging_dir(&name, LeaseMode::OpenExisting)? else {
            return Ok(false);
        };
        lease.claim = Some(claim);
        self.discard_attempt_lease(lease)?;
        Ok(true)
    }

    /// Remove old staging directories that belong to no resumable run.
    ///
    /// Only direct children of `<root>/.lv` named `y-*` are considered. A
    /// link or junction is never followed or removed, a referenced or
    /// claimed directory is kept, and a directory is only removed when the
    /// newest change inside it is at least `min_age` old and the
    /// identity-held flat cleanup plan can delete it.
    pub fn sweep_orphan_staging(
        &self,
        referenced: &HashSet<String>,
        now: SystemTime,
        min_age: Duration,
    ) -> Result<OrphanSweepReport, SafeOutputError> {
        let mut report = OrphanSweepReport::default();
        let parent = self.path.join(STAGING_DIR_NAME);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) if !is_untrusted_directory(&metadata) => {}
            Ok(_) => return Ok(report),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(error) => return Err(error.into()),
        }
        self.revalidate()?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&parent)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_owned());
            }
        }
        names.sort();
        for name in names {
            if !name.starts_with(STAGING_NAME_PREFIX) || validate_component(&name).is_err() {
                report.kept.push((name, SweepKept::NotOurs));
                continue;
            }
            let path = parent.join(&name);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if is_reparse_point(&metadata) {
                report.kept.push((name, SweepKept::Unsafe));
                continue;
            }
            if let Some(directory_name) = name.strip_suffix(".lock") {
                if metadata.file_type().is_file()
                    && !is_claimed(self, directory_name)
                    && !referenced.contains(directory_name)
                    && metadata
                        .modified()
                        .is_ok_and(|modified| is_older_than(modified, now, min_age))
                    && fs::remove_file(&path).is_ok()
                {
                    report.removed.push(name);
                } else {
                    report.kept.push((name, SweepKept::NotOurs));
                }
                continue;
            }
            if !metadata.file_type().is_dir() {
                report.kept.push((name, SweepKept::NotOurs));
                continue;
            }
            if referenced.contains(&name) {
                report.kept.push((name, SweepKept::Referenced));
                continue;
            }
            let Ok(claim) = self.claim_staging(&name) else {
                report.kept.push((name, SweepKept::InUse));
                continue;
            };
            let newest = newest_change(&path, &metadata);
            if !newest.is_some_and(|newest| is_older_than(newest, now, min_age)) {
                report.kept.push((name, SweepKept::Recent));
                continue;
            }
            let leased = match self.lease_staging_dir(&name, LeaseMode::OpenExisting) {
                Ok(Some(mut lease)) => {
                    lease.claim = Some(claim);
                    lease
                }
                Ok(None) => continue,
                Err(_) => {
                    report.kept.push((name, SweepKept::Unsafe));
                    continue;
                }
            };
            if self.discard_attempt_lease(leased).is_ok() {
                report.removed.push(name);
            } else {
                report.kept.push((name, SweepKept::CleanupFailed));
            }
        }
        Ok(report)
    }
}

/// Newest modification time of a directory and its direct children (growing
/// `.part` files do not update the directory's own time).
fn newest_change(path: &Path, directory: &fs::Metadata) -> Option<SystemTime> {
    let mut newest = directory.modified().ok();
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) {
                newest = Some(newest.map_or(modified, |current| current.max(modified)));
            }
        }
    }
    newest
}

/// Validate an existing staging directory for reuse and total its bytes.
fn inspect_staging(lease: &OutputAttemptLease) -> Result<u64, SafeOutputError> {
    lease.validate_contents()?;
    let mut total = 0u64;
    for entry in fs::read_dir(lease.path())? {
        let entry = entry?;
        let path = entry.path();
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| SafeOutputError::UnsafeDescendant { path: path.clone() })?;
        if !is_expected_staging_leaf(&name) {
            return Err(SafeOutputError::UnsafeDescendant { path });
        }
        total = total.saturating_add(fs::symlink_metadata(&path)?.len());
    }
    Ok(total)
}

impl OutputAttemptLease {
    /// Names of the direct files in the staging directory.
    pub fn leaf_names(&self) -> Result<Vec<String>, SafeOutputError> {
        self.revalidate()?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Total size of the direct files (the bytes a resume keeps).
    pub fn leaf_bytes(&self) -> Result<u64, SafeOutputError> {
        let mut total = 0u64;
        for name in self.leaf_names()? {
            if let Ok(metadata) = fs::symlink_metadata(self.path.join(&name)) {
                if metadata.file_type().is_file() && !is_reparse_point(&metadata) {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
        Ok(total)
    }

    /// Delete one direct regular file through a no-follow delete handle.
    pub fn remove_leaf(&self, name: &str) -> Result<(), SafeOutputError> {
        validate_component(name)?;
        self.revalidate()?;
        #[cfg(windows)]
        {
            let file = open_regular_leaf_for_delete(&self.path, name)?;
            mark_delete_by_handle_once(&file)?;
        }
        #[cfg(not(windows))]
        {
            let path = self.path.join(name);
            let metadata = fs::symlink_metadata(&path)?;
            if is_reparse_point(&metadata) || !metadata.file_type().is_file() {
                return Err(SafeOutputError::UnsafeDescendant { path });
            }
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn root() -> (tempfile::TempDir, ValidatedOutputRoot) {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        (temp, root)
    }

    fn old() -> SystemTime {
        SystemTime::now() + Duration::from_secs(30 * 24 * 60 * 60)
    }

    #[test]
    fn staging_name_is_deterministic_and_separates_fingerprints() {
        let a = resumable_staging_name("occ-1", "fp-1");
        assert_eq!(a, resumable_staging_name("occ-1", "fp-1"));
        assert_ne!(a, resumable_staging_name("occ-1", "fp-2"));
        assert_ne!(a, resumable_staging_name("occ-2", "fp-1"));
        assert!(a.starts_with("y-") && !a[2..].contains('-'));
    }

    #[test]
    fn deterministic_staging_is_reused_and_its_part_files_are_preserved() {
        let (_temp, root) = root();
        let first = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        assert_eq!(first.disposition, StagingDisposition::Fresh);
        let path = first.lease.path().to_path_buf();
        fs::write(path.join("media.f137.mp4.part"), vec![7u8; 4096]).unwrap();
        fs::write(path.join("media.f137.mp4.ytdl"), b"{}").unwrap();
        drop(first);

        let second = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        assert_eq!(second.lease.path(), path.as_path());
        assert!(matches!(
            second.disposition,
            StagingDisposition::Reused { preserved_bytes } if preserved_bytes == 4098
        ));
        assert_eq!(
            fs::read(path.join("media.f137.mp4.part")).unwrap().len(),
            4096
        );
    }

    #[test]
    fn unexpected_staging_contents_cause_a_fresh_start() {
        let (_temp, root) = root();
        let first = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        let path = first.lease.path().to_path_buf();
        fs::write(path.join("media.f137.mp4.part"), b"old").unwrap();
        fs::write(path.join("surprise.txt"), b"not ours").unwrap();
        drop(first);

        let second = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        assert_eq!(second.disposition, StagingDisposition::ReplacedUnexpected);
        assert_eq!(second.lease.leaf_names().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn nested_staging_contents_are_not_trusted_and_not_followed() {
        let (temp, root) = root();
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, b"keep").unwrap();
        let first = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        let path = first.lease.path().to_path_buf();
        fs::create_dir(path.join("media.nested")).unwrap();
        drop(first);

        let second = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        // Nested content cannot be proven safe to delete, so the attempt
        // gets a one-off directory and the original is left untouched.
        assert_eq!(second.disposition, StagingDisposition::Unresumable);
        assert_ne!(second.lease.path(), path.as_path());
        assert!(path.join("media.nested").is_dir());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }

    #[test]
    fn staging_is_exclusive_until_the_lease_is_dropped() {
        let (_temp, root) = root();
        let first = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        assert!(matches!(
            root.resumable_staging_lease("occ-1", "fp-1"),
            Err(SafeOutputError::StagingBusy { .. })
        ));
        assert!(matches!(
            root.discard_resumable_staging("occ-1", "fp-1"),
            Err(SafeOutputError::StagingBusy { .. })
        ));
        // A different item is unaffected.
        let other = root.resumable_staging_lease("occ-2", "fp-1").unwrap();
        drop(other);
        drop(first);
        root.resumable_staging_lease("occ-1", "fp-1").unwrap();
    }

    #[test]
    fn a_second_validated_root_for_the_same_folder_cannot_share_staging() {
        let temp = tempdir().unwrap();
        let one = validate_output_root(temp.path()).unwrap();
        let two = validate_output_root(temp.path()).unwrap();
        let held = one.resumable_staging_lease("occ-1", "fp-1").unwrap();
        assert!(matches!(
            two.resumable_staging_lease("occ-1", "fp-1"),
            Err(SafeOutputError::StagingBusy { .. })
        ));
        drop(held);
    }

    #[test]
    fn discard_removes_the_staging_directory_for_cancel() {
        let (_temp, root) = root();
        let first = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        let path = first.lease.path().to_path_buf();
        fs::write(path.join("media.mp4.part"), b"x").unwrap();
        drop(first);
        assert!(root.discard_resumable_staging("occ-1", "fp-1").unwrap());
        assert!(!path.exists());
        assert!(!root.discard_resumable_staging("occ-1", "fp-1").unwrap());
    }

    #[test]
    fn remove_leaf_deletes_one_file() {
        let (_temp, root) = root();
        let staging = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        fs::write(staging.lease.path().join("media.f1.mp4.part"), b"a").unwrap();
        fs::write(staging.lease.path().join("media.f2.m4a.part"), b"b").unwrap();
        staging.lease.remove_leaf("media.f1.mp4.part").unwrap();
        assert_eq!(
            staging.lease.leaf_names().unwrap(),
            vec!["media.f2.m4a.part".to_string()]
        );
        assert!(staging.lease.remove_leaf("../escape").is_err());
    }

    fn make_dir(root: &ValidatedOutputRoot, name: &str, file: &str) -> PathBuf {
        let path = root.path().join(STAGING_DIR_NAME).join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(file), b"data").unwrap();
        path
    }

    #[test]
    fn orphan_sweep_removes_only_old_unreferenced_y_directories() {
        let (_temp, root) = root();
        let orphan = make_dir(&root, "y-orphan", "media.mp4.part");
        let referenced = make_dir(&root, "y-referenced", "media.mp4.part");
        let foreign = make_dir(&root, "keep-me", "media.mp4.part");
        let held = root.resumable_staging_lease("occ-9", "fp-9").unwrap();
        let held_path = held.lease.path().to_path_buf();

        let mut keep = HashSet::new();
        keep.insert("y-referenced".to_string());
        let report = root
            .sweep_orphan_staging(&keep, old(), ORPHAN_MIN_AGE)
            .unwrap();

        assert_eq!(report.removed, vec!["y-orphan".to_string()]);
        assert!(!orphan.exists());
        assert!(referenced.exists(), "referenced runs keep their staging");
        assert!(foreign.exists(), "non y- directories are never touched");
        assert!(held_path.exists(), "a directory in use is never touched");
    }

    #[test]
    fn orphan_sweep_keeps_recent_directories() {
        let (_temp, root) = root();
        let recent = make_dir(&root, "y-recent", "media.mp4.part");
        let report = root
            .sweep_orphan_staging(&HashSet::new(), SystemTime::now(), ORPHAN_MIN_AGE)
            .unwrap();
        assert!(report.removed.is_empty());
        assert!(recent.exists());
        assert!(report.kept.contains(&("y-recent".to_string(), SweepKept::Recent)));
    }

    #[cfg(windows)]
    #[test]
    fn orphan_sweep_never_follows_or_removes_a_junction() {
        let (temp, root) = root();
        let target = temp.path().join("precious");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("important.txt"), b"keep").unwrap();
        fs::create_dir_all(root.path().join(STAGING_DIR_NAME)).unwrap();
        let link = root.path().join(STAGING_DIR_NAME).join("y-junction");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J failed");

        let report = root
            .sweep_orphan_staging(&HashSet::new(), old(), ORPHAN_MIN_AGE)
            .unwrap();
        assert!(report.removed.is_empty());
        assert!(report.kept.contains(&("y-junction".to_string(), SweepKept::Unsafe)));
        assert_eq!(fs::read(target.join("important.txt")).unwrap(), b"keep");
        assert!(link.exists());
        fs::remove_dir(&link).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_at_the_deterministic_name_is_never_followed() {
        let (temp, root) = root();
        let target = temp.path().join("precious");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("important.txt"), b"keep").unwrap();
        fs::create_dir_all(root.path().join(STAGING_DIR_NAME)).unwrap();
        let link = root
            .path()
            .join(STAGING_DIR_NAME)
            .join(resumable_staging_name("occ-1", "fp-1"));
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J failed");

        let staging = root.resumable_staging_lease("occ-1", "fp-1").unwrap();
        assert_eq!(staging.disposition, StagingDisposition::Unresumable);
        assert_eq!(fs::read(target.join("important.txt")).unwrap(), b"keep");
        drop(staging);
        fs::remove_dir(&link).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_lock_opened_through_a_swapped_directory_is_refused_and_not_left_outside() {
        let (temp, root) = root();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let real = root.path().join(STAGING_DIR_NAME);
        fs::create_dir_all(&real).unwrap();
        let swapped = root.path().join("swapped-lv");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&swapped)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J failed");

        // `swapped` plays the `.lv` that was replaced after the check.
        let expected = real.canonicalize().unwrap();
        let error = open_verified_lock(&swapped, &expected, "y-test").unwrap_err();
        assert!(matches!(error, SafeOutputError::IdentityChanged { .. }), "{error}");
        assert!(
            !outside.join("y-test.lock").exists(),
            "nothing may be left behind outside the root"
        );
        fs::remove_dir(&swapped).unwrap();

        // The honest path still works and the lock disappears on close.
        let lock = open_verified_lock(&real, &expected, "y-ok").unwrap();
        assert!(real.join("y-ok.lock").exists());
        drop(lock);
        assert!(!real.join("y-ok.lock").exists());
    }

    #[test]
    fn older_than_compares_against_the_cutoff() {
        let now = SystemTime::now();
        assert!(is_older_than(
            now - Duration::from_secs(8 * 24 * 3600),
            now,
            ORPHAN_MIN_AGE
        ));
        assert!(!is_older_than(
            now - Duration::from_secs(6 * 24 * 3600),
            now,
            ORPHAN_MIN_AGE
        ));
    }
}
