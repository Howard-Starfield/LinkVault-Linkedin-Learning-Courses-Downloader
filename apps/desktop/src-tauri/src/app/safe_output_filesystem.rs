//! Safe output-root capabilities used by transient external-process jobs.
//!
//! The renderer supplies a folder selected by the user, but it never supplies
//! a helper path or an output filename.  This module validates the selected
//! root once and only exposes constrained descendants to provider code.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
}

impl ValidatedOutputRoot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create a provider-owned descendant directory beneath the validated
    /// root.  Every existing component is checked for symlink/reparse-point
    /// substitution before the path is returned.
    pub fn child_dir(&self, name: &str) -> Result<PathBuf, SafeOutputError> {
        validate_component(name)?;
        let child = self.path.join(name);
        match fs::symlink_metadata(&child) {
            Ok(metadata) => {
                if is_untrusted_directory(&metadata) {
                    return Err(SafeOutputError::UntrustedDirectory { path: child });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&child).map_err(|error| SafeOutputError::NotWritable {
                    path: child.clone(),
                    message: error.to_string(),
                })?;
            }
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata(&child)?;
        if is_untrusted_directory(&metadata) {
            return Err(SafeOutputError::UntrustedDirectory { path: child });
        }
        let canonical_child = child.canonicalize()?;
        if !canonical_child.starts_with(&self.canonical_path)
            || canonical_child == self.canonical_path
        {
            return Err(SafeOutputError::EscapedRoot);
        }
        Ok(child)
    }

    /// Create a staging directory beneath `.linkvault-staging`.  Staging is
    /// kept separate from published item directories so a failed helper never
    /// becomes a visible completed artifact.
    pub fn staging_dir(
        &self,
        occurrence_id: &str,
        artifact_fingerprint: &str,
    ) -> Result<PathBuf, SafeOutputError> {
        validate_component(occurrence_id)?;
        validate_component(artifact_fingerprint)?;
        let staging_root = self.child_dir(".linkvault-staging")?;
        let youtube_root = staging_root.join("youtube");
        ensure_directory(&youtube_root)?;
        let staging = youtube_root.join(occurrence_id).join(artifact_fingerprint);
        let occurrence_root = youtube_root.join(occurrence_id);
        ensure_directory(&occurrence_root)?;
        if path_utf16_len(&staging) > 240 {
            return Err(SafeOutputError::PathTooLong);
        }
        match fs::symlink_metadata(&staging) {
            Ok(metadata) => {
                if is_untrusted_directory(&metadata) {
                    return Err(SafeOutputError::UntrustedDirectory { path: staging });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&staging).map_err(|error| SafeOutputError::NotWritable {
                    path: staging.clone(),
                    message: error.to_string(),
                })?;
            }
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata(&staging)?;
        if is_untrusted_directory(&metadata) {
            return Err(SafeOutputError::UntrustedDirectory { path: staging });
        }
        let canonical_staging = staging.canonicalize()?;
        if !canonical_staging.starts_with(&self.canonical_path) {
            return Err(SafeOutputError::EscapedRoot);
        }
        Ok(staging)
    }

    pub fn staging_attempt_dir(
        &self,
        occurrence_id: &str,
        artifact_fingerprint: &str,
    ) -> Result<PathBuf, SafeOutputError> {
        let staging = self.staging_dir(occurrence_id, artifact_fingerprint)?;
        static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);
        let attempt_name = format!(
            "attempt-{}-{}-{}",
            std::process::id(),
            now_nanos(),
            NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed)
        );
        validate_component(&attempt_name)?;
        let attempt = staging.join(attempt_name);
        if path_utf16_len(&attempt) > 240 {
            return Err(SafeOutputError::PathTooLong);
        }
        fs::create_dir(&attempt).map_err(|error| SafeOutputError::NotWritable {
            path: attempt.clone(),
            message: error.to_string(),
        })?;
        let metadata = fs::symlink_metadata(&attempt)?;
        if is_untrusted_directory(&metadata) {
            return Err(SafeOutputError::UntrustedDirectory { path: attempt });
        }
        let canonical_attempt = attempt.canonicalize()?;
        if !canonical_attempt.starts_with(&self.canonical_path) {
            return Err(SafeOutputError::EscapedRoot);
        }
        Ok(attempt)
    }

    /// Verify that the helper-visible attempt contains only direct regular
    /// files below the validated root. A helper is not allowed to create a
    /// nested directory, symlink, junction, device or other reparse entry.
    pub fn validate_attempt_contents(&self, attempt: &Path) -> Result<(), SafeOutputError> {
        self.validate_descendant(attempt)?;
        for entry in fs::read_dir(attempt)? {
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
            self.validate_descendant(&path)?;
        }
        Ok(())
    }

    /// Atomically publish the complete attempt directory. The destination is
    /// never created in advance, so an existing item is a hard collision and
    /// no individual artifact can become visible as a partial item.
    pub fn publish_attempt(
        &self,
        attempt: &Path,
        final_name: &str,
    ) -> Result<PathBuf, SafeOutputError> {
        validate_component(final_name)?;
        let destination = self.path.join(final_name);
        if path_utf16_len(&destination) > 240 {
            return Err(SafeOutputError::PathTooLong);
        }
        let canonical_attempt = attempt.canonicalize()?;
        if !canonical_attempt.starts_with(&self.canonical_path)
            || canonical_attempt == self.canonical_path
        {
            return Err(SafeOutputError::EscapedRoot);
        }
        let attempt_metadata = fs::symlink_metadata(attempt)?;
        if is_untrusted_directory(&attempt_metadata) {
            return Err(SafeOutputError::UntrustedDirectory {
                path: attempt.to_path_buf(),
            });
        }
        self.validate_attempt_contents(attempt)?;
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(SafeOutputError::OutputCollision { path: destination });
        }
        let fingerprint_staging = attempt.parent().map(Path::to_path_buf);
        let occurrence_staging = fingerprint_staging
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        match fs::rename(attempt, &destination) {
            Ok(()) => {
                if let Some(path) = fingerprint_staging {
                    let _ = fs::remove_dir(path);
                }
                if let Some(path) = occurrence_staging {
                    let _ = fs::remove_dir(path);
                }
                Ok(destination)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(SafeOutputError::OutputCollision { path: destination })
            }
            Err(error) => {
                if fs::symlink_metadata(&destination).is_ok() {
                    Err(SafeOutputError::OutputCollision { path: destination })
                } else {
                    Err(error.into())
                }
            }
        }
    }

    pub fn discard_attempt(&self, attempt: &Path) -> Result<(), SafeOutputError> {
        let canonical_attempt = attempt.canonicalize()?;
        if !canonical_attempt.starts_with(&self.canonical_path)
            || canonical_attempt == self.canonical_path
        {
            return Err(SafeOutputError::EscapedRoot);
        }
        fs::remove_dir_all(attempt)?;
        Ok(())
    }

    fn validate_descendant(&self, path: &Path) -> Result<(), SafeOutputError> {
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(&self.canonical_path) || canonical == self.canonical_path {
            return Err(SafeOutputError::EscapedRoot);
        }
        let metadata = fs::symlink_metadata(path)?;
        if is_reparse_point(&metadata) {
            return Err(SafeOutputError::UnsafeDescendant {
                path: path.to_path_buf(),
            });
        }
        Ok(())
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
    let canonical_path = path.canonicalize()?;
    let root = ValidatedOutputRoot {
        path: path.to_path_buf(),
        canonical_path,
    };
    probe_writable_dir(root.path())?;
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

fn ensure_directory(path: &Path) -> Result<(), SafeOutputError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_untrusted_directory(&metadata) {
                return Err(SafeOutputError::UntrustedDirectory {
                    path: path.to_path_buf(),
                });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| SafeOutputError::NotWritable {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
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
        let child = root.child_dir("validated-child").unwrap();
        assert!(child.starts_with(temp.path()));
        assert!(root
            .staging_dir("occurrence-1", "artifact-1")
            .unwrap()
            .starts_with(temp.path()));
        let attempt = root
            .staging_attempt_dir("occurrence-1", "artifact-1")
            .unwrap();
        let published = root.publish_attempt(&attempt, "video").unwrap();
        assert!(published.is_dir());
        assert!(root.publish_attempt(&attempt, "video").is_err());
        assert!(root.child_dir("..").is_err());
        assert!(root.child_dir("nested/name").is_err());
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
            .staging_attempt_dir("occurrence-1", "artifact-1")
            .unwrap();
        fs::create_dir(attempt.join("nested")).unwrap();
        let result = root.publish_attempt(&attempt, "video");
        assert!(matches!(
            result,
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
        assert!(!temp.path().join("video").exists());
        assert!(attempt.exists());
    }

    #[test]
    fn existing_final_directory_is_a_collision() {
        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let first = root
            .staging_attempt_dir("occurrence-1", "artifact-1")
            .unwrap();
        fs::write(first.join("artifact.mp4"), b"complete").unwrap();
        root.publish_attempt(&first, "video").unwrap();
        let second = root
            .staging_attempt_dir("occurrence-1", "artifact-1")
            .unwrap();
        fs::write(second.join("artifact.mp4"), b"complete").unwrap();
        assert!(matches!(
            root.publish_attempt(&second, "video"),
            Err(SafeOutputError::OutputCollision { .. })
        ));
        assert_eq!(
            fs::read(temp.path().join("video").join("artifact.mp4")).unwrap(),
            b"complete"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_reparse_descendant() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let root = validate_output_root(temp.path()).unwrap();
        let attempt = root
            .staging_attempt_dir("occurrence-1", "artifact-1")
            .unwrap();
        symlink(temp.path(), attempt.join("link")).unwrap();
        assert!(matches!(
            root.validate_attempt_contents(&attempt),
            Err(SafeOutputError::UnsafeDescendant { .. })
        ));
    }
}
