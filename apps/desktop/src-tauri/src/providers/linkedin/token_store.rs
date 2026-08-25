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

    let protected = crate::dpapi::protect_bytes(trimmed.as_bytes(), "LinkVault LinkedIn session")
        .map_err(map_dpapi_error)?;
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
    let token = String::from_utf8(unprotect_token(&protected)?)?;
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

fn map_dpapi_error(error: crate::dpapi::DpapiError) -> TokenStoreError {
    match error {
        crate::dpapi::DpapiError::UnsupportedPlatform => TokenStoreError::UnsupportedPlatform,
        crate::dpapi::DpapiError::Storage(message) => TokenStoreError::Storage(message),
    }
}

fn unprotect_token(input: &[u8]) -> Result<Vec<u8>, TokenStoreError> {
    crate::dpapi::unprotect_bytes(input).map_err(map_dpapi_error)
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
