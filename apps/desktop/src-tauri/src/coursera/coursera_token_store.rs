//! Coursera-side DPAPI token store.
//!
//! The Coursera tab stores its `CAUTH` cookie in a **separate** file
//! (`linkvault.coursera.dpapi`) from the LinkedIn `li_at` token
//! (`linkvault.li_at.dpapi`). This module is a sibling of `token_store.rs`
//! — it does not import it, and `token_store.rs` does not import this.
//! Both call the same Windows DPAPI primitive (`CryptProtectData` /
//! `CryptUnprotectData`) but each owns its own file path, its own error
//! type, and its own public surface.
//!
//! Isolation note: this module is owned by `coursera/`. It is permitted
//! to add to the additive `coursera_*` helpers in `storage.rs` for the
//! path resolution. The LinkedIn-side `token_store.rs` is not edited.

// Phase 3: every public symbol is consumed by later phases but not by
// the lib build yet. The blanket allow matches `config.rs`.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use thiserror::Error;

const COURSERA_TOKEN_DESCRIPTION: &str = "LinkVault Coursera session";

#[derive(Debug, Error)]
pub enum CourseraTokenStoreError {
    #[error("Coursera token is empty")]
    EmptyToken,
    #[error("saved Coursera token is unavailable")]
    MissingToken,
    #[error("saved Coursera token could not be decoded")]
    Decode,
    #[error("saved Coursera token storage is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("saved Coursera token storage failed: {0}")]
    Storage(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub type CourseraTokenStoreResult<T> = Result<T, CourseraTokenStoreError>;

/// `true` when the path is a regular file (i.e. a saved token exists).
/// Cheap; does not decrypt the contents.
pub fn has_saved_token(path: &Path) -> bool {
    path.is_file()
}

/// Encrypt `token` with DPAPI and write the result to `path`. The on-disk
/// representation is the base64 of the DPAPI ciphertext (so the file is
/// ASCII; debugging tools don't choke on it).
pub fn save_token(path: &Path, token: &str) -> CourseraTokenStoreResult<()> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(CourseraTokenStoreError::EmptyToken);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let protected = protect(trimmed.as_bytes())?;
    fs::write(path, BASE64.encode(&protected))?;
    Ok(())
}

/// Read the DPAPI-encrypted token from `path` and decrypt it. The trimmed
/// plaintext is returned.
pub fn load_token(path: &Path) -> CourseraTokenStoreResult<String> {
    if !path.is_file() {
        return Err(CourseraTokenStoreError::MissingToken);
    }
    let encoded = fs::read_to_string(path)?;
    let protected = BASE64
        .decode(encoded.trim())
        .map_err(|_| CourseraTokenStoreError::Decode)?;
    let bytes = unprotect(&protected)?;
    let token = String::from_utf8(bytes)?;
    let trimmed = token.trim().to_string();
    if trimmed.is_empty() {
        return Err(CourseraTokenStoreError::MissingToken);
    }
    Ok(trimmed)
}

/// Delete the file at `path`. A missing file is not an error.
pub fn clear_token(path: &Path) -> CourseraTokenStoreResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Default path for the Coursera DPAPI token file, rooted in the data
/// directory. Pure: returns `<data_dir>/linkvault.coursera.dpapi`.
///
/// Callers resolve `data_dir` via `storage::resolve_data_dir()`. This
/// function does not touch the filesystem beyond formatting the path,
/// so it is safe to call from any context.
pub fn default_token_path(data_dir: &Path) -> PathBuf {
    data_dir.join("linkvault.coursera.dpapi")
}

#[cfg(windows)]
fn protect(input: &[u8]) -> CourseraTokenStoreResult<Vec<u8>> {
    use std::ptr;
    use std::slice;
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output_blob = CRYPT_INTEGER_BLOB::default();
    let description = wide_null(COURSERA_TOKEN_DESCRIPTION);

    let ok = unsafe {
        CryptProtectData(
            &mut input_blob,
            description.as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output_blob,
        )
    };

    if ok == 0 {
        return Err(CourseraTokenStoreError::Storage(
            "Windows DPAPI could not protect the Coursera token".to_string(),
        ));
    }

    let bytes = unsafe {
        let output =
            slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec();
        LocalFree(output_blob.pbData as HLOCAL);
        output
    };
    Ok(bytes)
}

#[cfg(windows)]
fn unprotect(input: &[u8]) -> CourseraTokenStoreResult<Vec<u8>> {
    use std::ptr;
    use std::slice;
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output_blob = CRYPT_INTEGER_BLOB::default();

    let ok = unsafe {
        CryptUnprotectData(
            &mut input_blob,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output_blob,
        )
    };

    if ok == 0 {
        return Err(CourseraTokenStoreError::Storage(
            "Windows DPAPI could not unprotect the saved Coursera token".to_string(),
        ));
    }

    let bytes = unsafe {
        let output =
            slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec();
        LocalFree(output_blob.pbData as HLOCAL);
        output
    };
    Ok(bytes)
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(not(windows))]
fn protect(_input: &[u8]) -> CourseraTokenStoreResult<Vec<u8>> {
    Err(CourseraTokenStoreError::UnsupportedPlatform)
}

#[cfg(not(windows))]
fn unprotect(_input: &[u8]) -> CourseraTokenStoreResult<Vec<u8>> {
    Err(CourseraTokenStoreError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_token_path_lives_in_data_dir() {
        let data_dir = Path::new("/tmp/linkvault-data");
        let p = default_token_path(data_dir);
        assert_eq!(
            p,
            PathBuf::from("/tmp/linkvault-data/linkvault.coursera.dpapi")
        );
    }

    #[test]
    fn default_token_path_does_not_include_linkedin_name() {
        // Regression guard: the Coursera DPAPI file must never collide
        // with the LinkedIn one. If somebody refactors this to share a
        // path, the test breaks.
        let p = default_token_path(Path::new("/data"));
        assert!(!p.to_string_lossy().contains("li_at"));
    }

    #[test]
    fn has_saved_token_returns_false_for_missing() {
        let p = std::env::temp_dir().join("linkvault-coursera-missing-test.dpapi");
        let _ = clear_token(&p);
        assert!(!has_saved_token(&p));
    }

    #[test]
    fn clear_token_tolerates_missing_file() {
        let p = std::env::temp_dir().join("linkvault-coursera-clear-missing-test.dpapi");
        clear_token(&p).unwrap();
        assert!(!has_saved_token(&p));
    }

    #[test]
    fn save_token_rejects_empty_string() {
        let p = std::env::temp_dir().join("linkvault-coursera-empty-test.dpapi");
        let result = save_token(&p, "   ");
        assert!(matches!(result, Err(CourseraTokenStoreError::EmptyToken)));
    }

    #[test]
    fn load_token_errors_when_file_missing() {
        let p = std::env::temp_dir().join("linkvault-coursera-load-missing-test.dpapi");
        let _ = clear_token(&p);
        let result = load_token(&p);
        assert!(matches!(result, Err(CourseraTokenStoreError::MissingToken)));
    }

    #[cfg(windows)]
    #[test]
    fn stores_coursera_token_encrypted_without_plaintext_bytes() {
        let unique = format!(
            "linkvault-coursera-test-{}-{}.dpapi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        let token = "CAUTH-test-token-do-not-store-plain";

        save_token(&path, token).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        // The on-disk representation must not contain the plaintext token.
        assert!(!raw.contains(token));
        assert_eq!(load_token(&path).unwrap(), token);

        clear_token(&path).unwrap();
        assert!(!has_saved_token(&path));
    }
}
