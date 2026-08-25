//! Windows DPAPI protect/unprotect primitive.
//!
//! Provider token stores own paths, filenames, and error types. This module
//! only encrypts and decrypts byte buffers.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DpapiError {
    #[error("DPAPI storage is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("DPAPI operation failed: {0}")]
    Storage(String),
}

pub fn protect_bytes(plaintext: &[u8], description: &str) -> Result<Vec<u8>, DpapiError> {
    protect(plaintext, description)
}

pub fn unprotect_bytes(ciphertext: &[u8]) -> Result<Vec<u8>, DpapiError> {
    unprotect(ciphertext)
}

#[cfg(windows)]
fn protect(input: &[u8], description: &str) -> Result<Vec<u8>, DpapiError> {
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
    let description = wide_null(description);

    let ok = unsafe {
        // SAFETY: input_blob points at `input` for the duration of the call;
        // description is a NUL-terminated UTF-16 buffer; output_blob is written
        // by CryptProtectData and freed with LocalFree below.
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
        return Err(DpapiError::Storage(
            "Windows DPAPI could not protect the buffer".to_string(),
        ));
    }

    let bytes = unsafe {
        // SAFETY: CryptProtectData succeeded and output_blob.pbData is a
        // LocalAlloc buffer of cbData bytes.
        let output =
            slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec();
        LocalFree(output_blob.pbData as HLOCAL);
        output
    };
    Ok(bytes)
}

#[cfg(windows)]
fn unprotect(input: &[u8]) -> Result<Vec<u8>, DpapiError> {
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
        // SAFETY: input_blob points at `input` for the duration of the call;
        // output_blob is written by CryptUnprotectData and freed with LocalFree.
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
        return Err(DpapiError::Storage(
            "Windows DPAPI could not unprotect the buffer".to_string(),
        ));
    }

    let bytes = unsafe {
        // SAFETY: CryptUnprotectData succeeded and output_blob.pbData is a
        // LocalAlloc buffer of cbData bytes.
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
fn protect(_input: &[u8], _description: &str) -> Result<Vec<u8>, DpapiError> {
    Err(DpapiError::UnsupportedPlatform)
}

#[cfg(not(windows))]
fn unprotect(_input: &[u8]) -> Result<Vec<u8>, DpapiError> {
    Err(DpapiError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn round_trips_bytes_without_leaving_plaintext_in_ciphertext() {
        let plaintext = b"linkvault-dpapi-roundtrip";
        let ciphertext = protect_bytes(plaintext, "LinkVault test").unwrap();
        assert!(!ciphertext.windows(plaintext.len()).any(|w| w == plaintext));
        assert_eq!(unprotect_bytes(&ciphertext).unwrap(), plaintext);
    }

    #[cfg(not(windows))]
    #[test]
    fn protect_is_unsupported_off_windows() {
        assert!(matches!(
            protect_bytes(b"secret", "test"),
            Err(DpapiError::UnsupportedPlatform)
        ));
    }
}
