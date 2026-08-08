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
    let root = resolve_data_dir()?.join(CLIPPINGS_DIR_NAME);
    ensure_writable_data_dir(root)
}

fn data_dir_for_exe_path(exe_path: &Path) -> Result<PathBuf, StoragePathError> {
    let exe_dir = exe_path
        .parent()
        .ok_or(StoragePathError::MissingExecutableFolder)?;
    Ok(exe_dir.join(DATA_DIR_NAME))
}

fn ensure_writable_data_dir(path: PathBuf) -> Result<PathBuf, StoragePathError> {
    fs::create_dir_all(&path).map_err(|error| StoragePathError::NotWritable {
        path: path.clone(),
        message: error.to_string(),
    })?;

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
            path,
            message: error.to_string(),
        });
    }

    let _ = fs::remove_file(probe_path);
    Ok(path)
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
    use tempfile::tempdir;

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
        use std::sync::{Mutex, OnceLock};

        // The resolver honors the LINKVAULT_DATA_DIR override, which is
        // process-global; serialize any test that touches it.
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

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
}
