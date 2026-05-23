use crate::auth::{BrowserSource, TokenCandidate};
use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use thiserror::Error;

const LINKEDIN_COOKIE_HOST: &str = ".www.linkedin.com";

#[derive(Debug, Error)]
pub enum BrowserCookieError {
    #[error("browser profile root does not exist: {0}")]
    MissingProfileRoot(String),
    #[error("failed to read browser cookie database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("failed to access browser cookie database: {0}")]
    Io(#[from] std::io::Error),
}

pub trait ChromiumCookieValueDecoder {
    fn decode(&self, encrypted_value: &[u8]) -> Option<String>;
}

pub trait DataProtector {
    fn unprotect(&self, protected_value: &[u8]) -> Option<Vec<u8>>;
}

pub struct WindowsDataProtector;

#[cfg(target_os = "windows")]
impl DataProtector for WindowsDataProtector {
    fn unprotect(&self, protected_value: &[u8]) -> Option<Vec<u8>> {
        windows_unprotect_data(protected_value)
    }
}

#[cfg(not(target_os = "windows"))]
impl DataProtector for WindowsDataProtector {
    fn unprotect(&self, _protected_value: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct ChromiumCookieDecoder {
    aes_key: Option<Vec<u8>>,
}

impl ChromiumCookieDecoder {
    pub fn from_user_data_path(user_data_path: &Path) -> Self {
        let local_state = fs::read_to_string(user_data_path.join("Local State")).ok();
        let aes_key = local_state
            .as_deref()
            .and_then(|json| decrypt_chromium_local_state_key(json, &WindowsDataProtector));

        Self { aes_key }
    }

    pub fn disabled() -> Self {
        Self { aes_key: None }
    }
}

impl ChromiumCookieValueDecoder for ChromiumCookieDecoder {
    fn decode(&self, encrypted_value: &[u8]) -> Option<String> {
        if is_modern_chromium_cookie_payload(encrypted_value) {
            return self
                .aes_key
                .as_deref()
                .and_then(|key| decrypt_chromium_aes_gcm_cookie(encrypted_value, key));
        }

        WindowsDataProtector
            .unprotect(encrypted_value)
            .and_then(|value| String::from_utf8(value).ok())
    }
}

pub struct UnsupportedEncryptedCookieDecoder;

impl ChromiumCookieValueDecoder for UnsupportedEncryptedCookieDecoder {
    fn decode(&self, _encrypted_value: &[u8]) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct BrowserCookieRoots {
    pub local_app_data: PathBuf,
    pub roaming_app_data: PathBuf,
}

impl BrowserCookieRoots {
    pub fn from_env() -> Self {
        Self {
            local_app_data: std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_default(),
            roaming_app_data: std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }
}

pub fn read_li_at_candidates(
    source: BrowserSource,
    roots: &BrowserCookieRoots,
    decoder: &impl ChromiumCookieValueDecoder,
) -> Result<Vec<TokenCandidate>, BrowserCookieError> {
    let values = match source {
        BrowserSource::Chrome | BrowserSource::Edge => read_chromium_li_at_values(
            &chromium_user_data_path_for_source(source, roots).expect("chromium source path"),
            decoder,
        )?,
        BrowserSource::Firefox => read_firefox_li_at_values(
            &roots
                .roaming_app_data
                .join("Mozilla")
                .join("Firefox")
                .join("Profiles"),
        )?,
    };

    Ok(values
        .into_iter()
        .map(|value| TokenCandidate { source, value })
        .collect())
}

pub fn chromium_user_data_path_for_source(
    source: BrowserSource,
    roots: &BrowserCookieRoots,
) -> Option<PathBuf> {
    match source {
        BrowserSource::Chrome => Some(
            roots
                .local_app_data
                .join("Google")
                .join("Chrome")
                .join("User Data"),
        ),
        BrowserSource::Edge => Some(
            roots
                .local_app_data
                .join("Microsoft")
                .join("Edge")
                .join("User Data"),
        ),
        BrowserSource::Firefox => None,
    }
}

pub fn chromium_cookie_database_paths(
    user_data_path: &Path,
) -> Result<Vec<PathBuf>, BrowserCookieError> {
    if !user_data_path.exists() {
        return Ok(Vec::new());
    }

    let mut profile_dirs = fs::read_dir(user_data_path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            path.join("Network").join("Cookies").exists() || path.join("Cookies").exists()
        })
        .collect::<Vec<_>>();

    profile_dirs.sort_by(|a, b| {
        let a_default = a
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Default"));
        let b_default = b
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Default"));
        a_default
            .cmp(&b_default)
            .reverse()
            .then_with(|| modified_desc(a, b))
    });

    let mut paths = Vec::new();
    for profile_dir in profile_dirs {
        let network = profile_dir.join("Network").join("Cookies");
        if network.exists() {
            paths.push(network);
        }

        let legacy = profile_dir.join("Cookies");
        if legacy.exists() {
            paths.push(legacy);
        }
    }

    Ok(paths)
}

pub fn firefox_cookie_database_paths(
    profiles_path: &Path,
) -> Result<Vec<PathBuf>, BrowserCookieError> {
    if !profiles_path.exists() {
        return Ok(Vec::new());
    }

    let mut profile_dirs = fs::read_dir(profiles_path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("cookies.sqlite").exists())
        .collect::<Vec<_>>();

    profile_dirs.sort_by(modified_desc);

    Ok(profile_dirs
        .into_iter()
        .map(|profile| profile.join("cookies.sqlite"))
        .collect())
}

fn read_chromium_li_at_values(
    user_data_path: &Path,
    decoder: &impl ChromiumCookieValueDecoder,
) -> Result<Vec<String>, BrowserCookieError> {
    let mut values = Vec::new();
    for db_path in chromium_cookie_database_paths(user_data_path)? {
        values.extend(read_chromium_li_at_values_from_db(&db_path, decoder)?);
    }

    Ok(distinct_non_empty(values))
}

fn read_firefox_li_at_values(profiles_path: &Path) -> Result<Vec<String>, BrowserCookieError> {
    let mut values = Vec::new();
    for db_path in firefox_cookie_database_paths(profiles_path)? {
        values.extend(read_firefox_li_at_values_from_db(&db_path)?);
    }

    Ok(distinct_non_empty(values))
}

pub fn read_chromium_li_at_values_from_db(
    db_path: &Path,
    decoder: &impl ChromiumCookieValueDecoder,
) -> Result<Vec<String>, BrowserCookieError> {
    let copy = CopiedSqliteDatabase::new(db_path)?;
    let connection =
        Connection::open_with_flags(copy.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection.prepare(
        "SELECT name, encrypted_value, value FROM cookies \
         WHERE host_key IN (?1, ?2, ?3)",
    )?;
    let rows = statement.query_map(
        params![
            LINKEDIN_COOKIE_HOST,
            LINKEDIN_COOKIE_HOST.trim_start_matches('.'),
            ".linkedin.com"
        ],
        |row| {
            let name: String = row.get(0)?;
            let encrypted_value: Option<Vec<u8>> = row.get(1)?;
            let value: Option<String> = row.get(2)?;
            Ok((name, encrypted_value, value))
        },
    )?;

    let mut values = Vec::new();
    for row in rows {
        let (name, encrypted_value, value) = row?;
        if name != "li_at" {
            continue;
        }

        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            values.push(value);
        } else if let Some(decrypted) = encrypted_value
            .as_deref()
            .filter(|value| !value.is_empty())
            .and_then(|encrypted| decoder.decode(encrypted))
        {
            values.push(decrypted);
        }
    }

    Ok(distinct_non_empty(values))
}

pub fn read_firefox_li_at_values_from_db(
    db_path: &Path,
) -> Result<Vec<String>, BrowserCookieError> {
    let copy = CopiedSqliteDatabase::new(db_path)?;
    let connection =
        Connection::open_with_flags(copy.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement =
        connection.prepare("SELECT name, value FROM moz_cookies WHERE host = ?1")?;
    let rows = statement.query_map(params![LINKEDIN_COOKIE_HOST], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut values = Vec::new();
    for row in rows {
        let (name, value) = row?;
        if name == "li_at" {
            values.push(value);
        }
    }

    Ok(distinct_non_empty(values))
}

pub fn copy_sqlite_database_files(
    source_db_path: &Path,
    destination_db_path: &Path,
) -> Result<(), BrowserCookieError> {
    fs::copy(source_db_path, destination_db_path)?;
    copy_if_exists(
        &source_db_path.with_extension("sqlite-wal"),
        &destination_db_path.with_extension("sqlite-wal"),
    )?;
    copy_if_exists(
        &source_db_path.with_extension("sqlite-shm"),
        &destination_db_path.with_extension("sqlite-shm"),
    )?;
    copy_if_exists(
        PathBuf::from(format!("{}-wal", source_db_path.display())).as_path(),
        PathBuf::from(format!("{}-wal", destination_db_path.display())).as_path(),
    )?;
    copy_if_exists(
        PathBuf::from(format!("{}-shm", source_db_path.display())).as_path(),
        PathBuf::from(format!("{}-shm", destination_db_path.display())).as_path(),
    )?;
    Ok(())
}

struct CopiedSqliteDatabase {
    _temp_dir: TempDir,
    db_path: PathBuf,
}

impl CopiedSqliteDatabase {
    fn new(source_db_path: &Path) -> Result<Self, BrowserCookieError> {
        let temp_dir = tempfile::Builder::new()
            .prefix("linkvault-cookies-")
            .tempdir()?;
        let db_path = temp_dir.path().join("Cookies");
        copy_sqlite_database_files(source_db_path, &db_path)?;
        Ok(Self {
            _temp_dir: temp_dir,
            db_path,
        })
    }

    fn path(&self) -> &Path {
        &self.db_path
    }
}

fn copy_if_exists(source: &Path, destination: &Path) -> Result<(), BrowserCookieError> {
    if source.exists() {
        fs::copy(source, destination)?;
    }
    Ok(())
}

fn distinct_non_empty(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut distinct = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            distinct.push(trimmed.to_string());
        }
    }
    distinct
}

pub fn decrypt_chromium_local_state_key(
    local_state_json: &str,
    protector: &impl DataProtector,
) -> Option<Vec<u8>> {
    let local_state = serde_json::from_str::<Value>(local_state_json).ok()?;
    let encrypted_key = local_state.pointer("/os_crypt/encrypted_key")?.as_str()?;
    let mut protected_key = BASE64.decode(encrypted_key).ok()?;
    if protected_key.starts_with(b"DPAPI") {
        protected_key.drain(..5);
    }
    protector.unprotect(&protected_key)
}

pub fn decrypt_chromium_aes_gcm_cookie(encrypted_value: &[u8], aes_key: &[u8]) -> Option<String> {
    if !is_modern_chromium_cookie_payload(encrypted_value) || aes_key.len() != 32 {
        return None;
    }

    let nonce = &encrypted_value[3..15];
    let ciphertext_and_tag = &encrypted_value[15..];
    let cipher = Aes256Gcm::new_from_slice(aes_key).ok()?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext_and_tag)
        .ok()?;
    String::from_utf8(plaintext).ok()
}

fn is_modern_chromium_cookie_payload(encrypted_value: &[u8]) -> bool {
    encrypted_value.len() >= 3 + 12 + 16
        && (encrypted_value.starts_with(b"v10") || encrypted_value.starts_with(b"v11"))
}

#[cfg(target_os = "windows")]
fn windows_unprotect_data(protected_value: &[u8]) -> Option<Vec<u8>> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: protected_value.len().try_into().ok()?,
        pbData: protected_value.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            0,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() {
        return None;
    }

    let bytes = unsafe {
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let value = slice.to_vec();
        LocalFree(output.pbData.cast());
        value
    };

    Some(bytes)
}

fn modified_desc(a: &PathBuf, b: &PathBuf) -> std::cmp::Ordering {
    let a_modified = a.metadata().and_then(|metadata| metadata.modified()).ok();
    let b_modified = b.metadata().and_then(|metadata| metadata.modified()).ok();
    b_modified.cmp(&a_modified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn reads_distinct_li_at_values_from_chromium_cookie_database_copy() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("Cookies");
        create_chromium_cookie_db(
            &db_path,
            &[
                (".www.linkedin.com", "li_at", "first-token"),
                ("www.linkedin.com", "li_at", "first-token"),
                (".linkedin.com", "li_at", "second-token"),
                (".www.linkedin.com", "other", "ignored"),
            ],
        );

        let values =
            read_chromium_li_at_values_from_db(&db_path, &UnsupportedEncryptedCookieDecoder)
                .unwrap();

        assert_eq!(values, vec!["first-token", "second-token"]);
    }

    #[test]
    fn reads_encrypted_chromium_cookie_through_decoder_adapter() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("Cookies");
        create_chromium_cookie_db_with_encrypted_value(&db_path, b"encrypted");

        let values = read_chromium_li_at_values_from_db(&db_path, &FakeDecoder).unwrap();

        assert_eq!(values, vec!["decoded-token"]);
    }

    #[test]
    fn decrypts_v10_chromium_aes_gcm_cookie_payload() {
        let key = [7u8; 32];
        let encrypted = encrypt_modern_chromium_cookie_for_test(b"v10", &key, b"decoded-token");

        let decrypted = decrypt_chromium_aes_gcm_cookie(&encrypted, &key).unwrap();

        assert_eq!(decrypted, "decoded-token");
    }

    #[test]
    fn decrypts_v11_chromium_aes_gcm_cookie_payload() {
        let key = [8u8; 32];
        let encrypted = encrypt_modern_chromium_cookie_for_test(b"v11", &key, b"decoded-token");

        let decrypted = decrypt_chromium_aes_gcm_cookie(&encrypted, &key).unwrap();

        assert_eq!(decrypted, "decoded-token");
    }

    #[test]
    fn rejects_modern_chromium_cookie_payload_with_wrong_key() {
        let encrypted =
            encrypt_modern_chromium_cookie_for_test(b"v10", &[7u8; 32], b"decoded-token");

        assert!(decrypt_chromium_aes_gcm_cookie(&encrypted, &[3u8; 32]).is_none());
    }

    #[test]
    fn decrypts_chromium_local_state_key_with_dpapi_prefix() {
        let protected_key = b"wrapped-key";
        let encoded = BASE64.encode([b"DPAPI".as_slice(), protected_key].concat());
        let json = format!(r#"{{"os_crypt":{{"encrypted_key":"{encoded}"}}}}"#);
        let protector = FakeProtector {
            expected: protected_key.to_vec(),
            unprotected: vec![1u8; 32],
        };

        let key = decrypt_chromium_local_state_key(&json, &protector).unwrap();

        assert_eq!(key, vec![1u8; 32]);
    }

    #[test]
    fn reads_distinct_li_at_values_from_firefox_cookie_database_copy() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("cookies.sqlite");
        create_firefox_cookie_db(
            &db_path,
            &[
                (".www.linkedin.com", "li_at", "firefox-token"),
                (".www.linkedin.com", "li_at", "firefox-token"),
                (".linkedin.com", "li_at", "ignored-root-host"),
            ],
        );

        let values = read_firefox_li_at_values_from_db(&db_path).unwrap();

        assert_eq!(values, vec!["firefox-token"]);
    }

    #[test]
    fn copies_sqlite_wal_and_shm_sidecars_before_reading() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Cookies");
        let destination = temp.path().join("Copied");
        fs::write(&source, "db").unwrap();
        fs::write(temp.path().join("Cookies-wal"), "wal").unwrap();
        fs::write(temp.path().join("Cookies-shm"), "shm").unwrap();

        copy_sqlite_database_files(&source, &destination).unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "db");
        assert_eq!(
            fs::read_to_string(temp.path().join("Copied-wal")).unwrap(),
            "wal"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("Copied-shm")).unwrap(),
            "shm"
        );
    }

    #[test]
    fn chromium_paths_prefer_default_profile_network_cookie_db() {
        let temp = tempfile::tempdir().unwrap();
        let user_data = temp.path();
        let profile = user_data.join("Default").join("Network");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("Cookies"), "db").unwrap();

        let paths = chromium_cookie_database_paths(user_data).unwrap();

        assert_eq!(paths, vec![profile.join("Cookies")]);
    }

    fn create_chromium_cookie_db(path: &Path, rows: &[(&str, &str, &str)]) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE cookies (
                    host_key TEXT NOT NULL,
                    name TEXT NOT NULL,
                    encrypted_value BLOB,
                    value TEXT
                );",
            )
            .unwrap();
        for (host, name, value) in rows {
            connection
                .execute(
                    "INSERT INTO cookies (host_key, name, encrypted_value, value) VALUES (?1, ?2, X'', ?3)",
                    params![host, name, value],
                )
                .unwrap();
        }
    }

    fn create_chromium_cookie_db_with_encrypted_value(path: &Path, encrypted: &[u8]) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE cookies (
                    host_key TEXT NOT NULL,
                    name TEXT NOT NULL,
                    encrypted_value BLOB,
                    value TEXT
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cookies (host_key, name, encrypted_value, value) VALUES (?1, ?2, ?3, '')",
                params![".www.linkedin.com", "li_at", encrypted],
            )
            .unwrap();
    }

    fn create_firefox_cookie_db(path: &Path, rows: &[(&str, &str, &str)]) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE moz_cookies (
                    host TEXT NOT NULL,
                    name TEXT NOT NULL,
                    value TEXT NOT NULL
                );",
            )
            .unwrap();
        for (host, name, value) in rows {
            connection
                .execute(
                    "INSERT INTO moz_cookies (host, name, value) VALUES (?1, ?2, ?3)",
                    params![host, name, value],
                )
                .unwrap();
        }
    }

    struct FakeDecoder;

    impl ChromiumCookieValueDecoder for FakeDecoder {
        fn decode(&self, encrypted_value: &[u8]) -> Option<String> {
            assert_eq!(encrypted_value, b"encrypted");
            Some("decoded-token".to_string())
        }
    }

    struct FakeProtector {
        expected: Vec<u8>,
        unprotected: Vec<u8>,
    }

    impl DataProtector for FakeProtector {
        fn unprotect(&self, protected_value: &[u8]) -> Option<Vec<u8>> {
            assert_eq!(protected_value, self.expected.as_slice());
            Some(self.unprotected.clone())
        }
    }

    fn encrypt_modern_chromium_cookie_for_test(
        prefix: &[u8],
        key: &[u8],
        plaintext: &[u8],
    ) -> Vec<u8> {
        let nonce = [9u8; 12];
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let mut encrypted = prefix.to_vec();
        encrypted.extend_from_slice(&nonce);
        encrypted.extend(
            cipher
                .encrypt(Nonce::from_slice(&nonce), plaintext)
                .unwrap(),
        );
        encrypted
    }
}
