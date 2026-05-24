use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TokenStoreError {
    #[error("LinkedIn token is empty")]
    EmptyToken,
    #[error("saved LinkedIn token is unavailable")]
    MissingToken,
    #[error("saved LinkedIn token could not be decoded")]
    Decode,
    #[error("saved LinkedIn token storage is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("saved LinkedIn token storage failed: {0}")]
    Storage(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub fn has_saved_token(path: &Path) -> bool {
    path.is_file()
}

pub fn save_token(path: &Path, token: &str) -> Result<(), TokenStoreError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(TokenStoreError::EmptyToken);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let protected = protect(trimmed.as_bytes())?;
    fs::write(path, BASE64.encode(protected))?;
    Ok(())
}

pub fn load_token(path: &Path) -> Result<String, TokenStoreError> {
    if !path.is_file() {
        return Err(TokenStoreError::MissingToken);
    }

    let encoded = fs::read_to_string(path)?;
    let protected = BASE64
        .decode(encoded.trim())
        .map_err(|_| TokenStoreError::Decode)?;
    let token = String::from_utf8(unprotect(&protected)?)?;
    let trimmed = token.trim().to_string();
    if trimmed.is_empty() {
        return Err(TokenStoreError::MissingToken);
    }
    Ok(trimmed)
}

pub fn clear_token(path: &Path) -> Result<(), TokenStoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn protect(input: &[u8]) -> Result<Vec<u8>, TokenStoreError> {
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
    let description = wide_null("LinkVault LinkedIn session");

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
        return Err(TokenStoreError::Storage(
            "Windows DPAPI could not protect the token".to_string(),
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
fn unprotect(input: &[u8]) -> Result<Vec<u8>, TokenStoreError> {
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
        return Err(TokenStoreError::Storage(
            "Windows DPAPI could not unprotect the saved token".to_string(),
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
fn protect(_input: &[u8]) -> Result<Vec<u8>, TokenStoreError> {
    Err(TokenStoreError::UnsupportedPlatform)
}

#[cfg(not(windows))]
fn unprotect(_input: &[u8]) -> Result<Vec<u8>, TokenStoreError> {
    Err(TokenStoreError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_token_tolerates_missing_file() {
        let path = std::env::temp_dir().join("linkvault-missing-token.dpapi");
        clear_token(&path).unwrap();
        assert!(!has_saved_token(&path));
    }

    #[cfg(windows)]
    #[test]
    fn stores_token_encrypted_without_plaintext_bytes() {
        let unique = format!(
            "linkvault-test-{}-{}.dpapi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        let token = "li_at-test-token-do-not-store-plain";

        save_token(&path, token).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(token));
        assert_eq!(load_token(&path).unwrap(), token);

        clear_token(&path).unwrap();
        assert!(!has_saved_token(&path));
    }
}
