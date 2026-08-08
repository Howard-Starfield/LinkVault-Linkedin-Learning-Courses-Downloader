use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const DATA_DIR_NAME: &str = "LinkVaultData";
const DATA_DIR_ENV: &str = "LINKVAULT_DATA_DIR";
const DB_FILE_NAME: &str = "linkvault.sqlite3";
const TOKEN_FILE_NAME: &str = "linkvault.li_at.dpapi";

#[derive(Debug, Error)]
pub enum StoragePathError {
    #[error("LinkVault could not find the executable folder")]
    MissingExecutableFolder,
    #[error("LinkVault data folder is not writable: {path}. {message}")]
    NotWritable { path: PathBuf, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn resolve_db_path() -> Result<PathBuf, StoragePathError> {
    Ok(resolve_data_dir()?.join(DB_FILE_NAME))
}

pub fn migrate_legacy_app_data(
    legacy_data_dir: &Path,
    data_dir: &Path,
) -> Result<(), StoragePathError> {
    if paths_equal(legacy_data_dir, data_dir) {
        return Ok(());
    }

    copy_if_missing(
        &legacy_data_dir.join(DB_FILE_NAME),
        &data_dir.join(DB_FILE_NAME),
    )?;
    copy_if_missing(
        &legacy_data_dir.join(format!("{DB_FILE_NAME}-wal")),
        &data_dir.join(format!("{DB_FILE_NAME}-wal")),
    )?;
    copy_if_missing(
        &legacy_data_dir.join(format!("{DB_FILE_NAME}-shm")),
        &data_dir.join(format!("{DB_FILE_NAME}-shm")),
    )?;
    migrate_legacy_token(
        &legacy_data_dir.join(TOKEN_FILE_NAME),
        &data_dir.join(TOKEN_FILE_NAME),
    )?;
    Ok(())
}

pub fn resolve_data_dir() -> Result<PathBuf, StoragePathError> {
    if let Some(override_dir) = env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
        return ensure_writable_data_dir(PathBuf::from(override_dir));
    }

    let exe_path = env::current_exe()?;
    let data_dir = data_dir_for_exe_path(&exe_path)?;
    ensure_writable_data_dir(data_dir)
}

const CLIPPINGS_DIR_NAME: &str = "newspaper-clippings";

/// Resolve the application-managed root for canonical Newspaper clipping
/// assets. The root lives beneath the resolved LinkVaultData directory, never
/// inside a user-selected newspaper download folder, so clippings survive
/// edition deletion and World Journal reset (ADR-002, D-009).
pub fn resolve_newspaper_clippings_root() -> Result<PathBuf, StoragePathError> {
    let data_dir = resolve_data_dir()?;
    let root = data_dir.join(CLIPPINGS_DIR_NAME);
    ensure_writable_child_dir(&data_dir, root)
}

fn data_dir_for_exe_path(exe_path: &Path) -> Result<PathBuf, StoragePathError> {
    let exe_dir = exe_path
        .parent()
        .ok_or(StoragePathError::MissingExecutableFolder)?;
    Ok(exe_dir.join(DATA_DIR_NAME))
}

fn ensure_writable_data_dir(path: PathBuf) -> Result<PathBuf, StoragePathError> {
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_dir() {
            return Err(StoragePathError::NotWritable {
                path,
                message: "directory is not a trusted regular directory".to_string(),
            });
        }
    }
    fs::create_dir_all(&path).map_err(|error| StoragePathError::NotWritable {
        path: path.clone(),
        message: error.to_string(),
    })?;

    let metadata = fs::symlink_metadata(&path).map_err(|error| StoragePathError::NotWritable {
        path: path.clone(),
        message: error.to_string(),
    })?;
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_dir() {
        return Err(StoragePathError::NotWritable {
            path,
            message: "directory is not a trusted regular directory".to_string(),
        });
    }

    probe_writable_dir(&path)?;
    Ok(path)
}

fn ensure_writable_child_dir(
    trusted_parent: &Path,
    path: PathBuf,
) -> Result<PathBuf, StoragePathError> {
    let parent_metadata =
        fs::symlink_metadata(trusted_parent).map_err(|error| StoragePathError::NotWritable {
            path: trusted_parent.to_path_buf(),
            message: error.to_string(),
        })?;
    if is_symlink_or_reparse(&parent_metadata) || !parent_metadata.file_type().is_dir() {
        return Err(StoragePathError::NotWritable {
            path: trusted_parent.to_path_buf(),
            message: "data parent is not a trusted regular directory".to_string(),
        });
    }
    let canonical_parent =
        trusted_parent
            .canonicalize()
            .map_err(|error| StoragePathError::NotWritable {
                path: trusted_parent.to_path_buf(),
                message: error.to_string(),
            })?;

    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_dir() {
                return Err(StoragePathError::NotWritable {
                    path,
                    message: "managed child is not a trusted regular directory".to_string(),
                });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&path).map_err(|error| StoragePathError::NotWritable {
                path: path.clone(),
                message: error.to_string(),
            })?;
        }
        Err(error) => {
            return Err(StoragePathError::NotWritable {
                path,
                message: error.to_string(),
            });
        }
    }

    let metadata = fs::symlink_metadata(&path).map_err(|error| StoragePathError::NotWritable {
        path: path.clone(),
        message: error.to_string(),
    })?;
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_dir() {
        return Err(StoragePathError::NotWritable {
            path,
            message: "managed child is not a trusted regular directory".to_string(),
        });
    }
    let canonical_child = path
        .canonicalize()
        .map_err(|error| StoragePathError::NotWritable {
            path: path.clone(),
            message: error.to_string(),
        })?;
    if !canonical_child.starts_with(&canonical_parent) || canonical_child == canonical_parent {
        return Err(StoragePathError::NotWritable {
            path,
            message: "managed child escaped the data parent".to_string(),
        });
    }

    probe_writable_dir(&canonical_child)?;
    Ok(path)
}

fn probe_writable_dir(path: &Path) -> Result<(), StoragePathError> {
    let probe_path = path.join(format!(
        ".linkvault-write-probe-{}-{}",
        std::process::id(),
        now_nanos()
    ));
    let probe_result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)
        .and_then(|mut file| file.write_all(b"ok"));

    if let Err(error) = probe_result {
        return Err(StoragePathError::NotWritable {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }

    let _ = fs::remove_file(probe_path);
    Ok(())
}

fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
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

fn copy_if_missing(source: &Path, destination: &Path) -> Result<(), StoragePathError> {
    if !source.is_file() || destination.exists() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    Ok(())
}

fn migrate_legacy_token(source: &Path, destination: &Path) -> Result<(), StoragePathError> {
    if !source.is_file() {
        return Ok(());
    }
    if !destination.exists() {
        copy_if_missing(source, destination)?;
    }
    if destination.is_file() {
        fs::remove_file(source)?;
    }
    Ok(())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::tempdir;

    fn env_guard() -> MutexGuard<'static, ()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn data_dir_sits_beside_executable() {
        let exe_path = Path::new(r"C:\Users\howard\AppData\Local\Programs\LinkVault\LinkVault.exe");

        let data_dir = data_dir_for_exe_path(exe_path).unwrap();

        assert_eq!(
            data_dir,
            PathBuf::from(r"C:\Users\howard\AppData\Local\Programs\LinkVault\LinkVaultData")
        );
    }

    #[test]
    fn writable_data_dir_is_created_and_verified() {
        let temp = tempdir().unwrap();
        let data_dir = temp.path().join("LinkVaultData");

        let resolved = ensure_writable_data_dir(data_dir.clone()).unwrap();

        assert_eq!(resolved, data_dir);
        assert!(resolved.is_dir());
        assert!(fs::read_dir(resolved).unwrap().next().is_none());
    }

    #[test]
    fn resolved_db_path_uses_data_dir_name() {
        let exe_path = Path::new(r"C:\Users\howard\AppData\Local\Programs\LinkVault\LinkVault.exe");

        let db_path = data_dir_for_exe_path(exe_path).unwrap().join(DB_FILE_NAME);

        assert_eq!(
            db_path,
            PathBuf::from(
                r"C:\Users\howard\AppData\Local\Programs\LinkVault\LinkVaultData\linkvault.sqlite3"
            )
        );
    }

    #[test]
    fn clipping_root_resolves_beneath_data_dir_and_is_created() {
        // The resolver honors the LINKVAULT_DATA_DIR override, which is
        // process-global; serialize any test that touches it.
        let _guard = env_guard();

        let temp = tempdir().unwrap();
        let data_dir = temp.path().join("LinkVaultData");
        let expected = data_dir.join("newspaper-clippings");

        env::set_var("LINKVAULT_DATA_DIR", &data_dir);
        let resolved = resolve_newspaper_clippings_root();
        env::remove_var("LINKVAULT_DATA_DIR");

        assert_eq!(resolved.unwrap(), expected);
        assert!(expected.is_dir());
    }

    #[test]
    fn clipping_root_file_is_rejected_before_any_probe() {
        let temp = tempdir().unwrap();
        let data_dir = temp.path().join("LinkVaultData");
        fs::create_dir(&data_dir).unwrap();
        let root = data_dir.join(CLIPPINGS_DIR_NAME);
        fs::write(&root, b"not-a-directory").unwrap();

        assert!(ensure_writable_child_dir(&data_dir, root.clone()).is_err());
        assert_eq!(fs::read(root).unwrap(), b"not-a-directory");
    }

    #[test]
    fn clipping_root_junction_is_rejected_without_writing_outside_data_parent() {
        let _guard = env_guard();
        let temp = tempdir().unwrap();
        let data_dir = temp.path().join("LinkVaultData");
        let outside = temp.path().join("outside");
        fs::create_dir(&data_dir).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();
        let root = data_dir.join(CLIPPINGS_DIR_NAME);
        if !create_dir_junction(&outside, &root) {
            eprintln!("directory junction creation unavailable on this machine");
            return;
        }

        env::set_var(DATA_DIR_ENV, &data_dir);
        let resolved = resolve_newspaper_clippings_root();
        env::remove_var(DATA_DIR_ENV);

        assert!(resolved.is_err());
        assert_eq!(fs::read(outside.join("sentinel.txt")).unwrap(), b"keep-me");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    }

    #[test]
    fn clipping_root_symbolic_link_is_rejected_when_platform_permits_it() {
        let temp = tempdir().unwrap();
        let data_dir = temp.path().join("LinkVaultData");
        let outside = temp.path().join("outside-symlink");
        fs::create_dir(&data_dir).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();
        let root = data_dir.join(CLIPPINGS_DIR_NAME);
        if !create_dir_symlink(&outside, &root) {
            eprintln!("directory symlink creation unavailable on this machine");
            return;
        }

        assert!(ensure_writable_child_dir(&data_dir, root).is_err());
        assert_eq!(fs::read(outside.join("sentinel.txt")).unwrap(), b"keep-me");
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }

    #[test]
    fn data_dir_override_junction_is_rejected_before_writable_probe() {
        let _guard = env_guard();
        let temp = tempdir().unwrap();
        let outside = temp.path().join("outside-override");
        let override_link = temp.path().join("LinkVaultData-override");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();
        if !create_dir_junction(&outside, &override_link) {
            eprintln!("directory junction creation unavailable on this machine");
            return;
        }

        env::set_var(DATA_DIR_ENV, &override_link);
        let resolved = resolve_newspaper_clippings_root();
        env::remove_var(DATA_DIR_ENV);

        assert!(resolved.is_err());
        assert_eq!(fs::read(outside.join("sentinel.txt")).unwrap(), b"keep-me");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    }

    #[test]
    fn legacy_app_data_migrates_db_and_removes_legacy_token_copy() {
        let temp = tempdir().unwrap();
        let legacy = temp.path().join("legacy");
        let portable = temp.path().join("LinkVaultData");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&portable).unwrap();
        fs::write(legacy.join(DB_FILE_NAME), b"sqlite").unwrap();
        fs::write(legacy.join(TOKEN_FILE_NAME), b"encrypted-token").unwrap();

        migrate_legacy_app_data(&legacy, &portable).unwrap();

        assert_eq!(fs::read(portable.join(DB_FILE_NAME)).unwrap(), b"sqlite");
        assert_eq!(
            fs::read(portable.join(TOKEN_FILE_NAME)).unwrap(),
            b"encrypted-token"
        );
        assert!(!legacy.join(TOKEN_FILE_NAME).exists());
        assert!(legacy.join(DB_FILE_NAME).exists());
    }

    fn create_dir_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
    }

    fn create_dir_junction(target: &Path, link: &Path) -> bool {
        #[cfg(not(windows))]
        {
            let _ = (target, link);
            false
        }
        #[cfg(windows)]
        {
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
