//! Native HTTP downloader.

#![allow(dead_code)] // Phase 7 — wired by Phase 8

use std::path::Path;
use std::time::Duration;
use std::{fs::File, io::Read, io::Write};

use reqwest::{blocking::Client, header};
use thiserror::Error;

use crate::coursera::client::DEFAULT_TIMEOUT;
use crate::coursera::define::USER_AGENT;

/// A progress event from a download.
#[derive(Debug, Clone)]
pub enum DownloadProgress {
    Started {
        url: String,
        total: Option<u64>,
    },
    Progress {
        url: String,
        bytes: u64,
        total: Option<u64>,
    },
    Finished {
        url: String,
        bytes: u64,
    },
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("http status {0}")]
    HttpStatus(u16),
    #[error("cancelled")]
    Cancelled,
    #[error("other: {0}")]
    Other(String),
}

impl DownloadError {
    pub fn is_retryable(&self) -> bool {
        match self {
            DownloadError::Io(_) | DownloadError::Network(_) => true,
            DownloadError::HttpStatus(s) => *s >= 500,
            DownloadError::Cancelled | DownloadError::Other(_) => false,
        }
    }
}

pub trait Downloader: Send + Sync {
    fn download(
        &self,
        url: &str,
        dest: &Path,
        on_progress: &mut dyn FnMut(DownloadProgress),
    ) -> Result<(), DownloadError>;
}

/// `reqwest`-backed downloader.
pub struct NativeDownloader {
    pub client: Client,
    pub max_attempts: u32,
    pub backoff: Duration,
}

impl Default for NativeDownloader {
    fn default() -> Self {
        Self {
            client: build_blocking_client().unwrap(),
            max_attempts: 3,
            backoff: Duration::from_millis(250),
        }
    }
}

impl NativeDownloader {
    pub fn new(_client: reqwest::Client) -> Self {
        Self {
            client: build_blocking_client().unwrap(),
            max_attempts: 3,
            backoff: Duration::from_millis(250),
        }
    }

    pub fn with_cookie_header(cookie_header: Option<String>) -> Result<Self, DownloadError> {
        Ok(Self {
            client: build_blocking_client_with_cookie(cookie_header)?,
            max_attempts: 3,
            backoff: Duration::from_millis(250),
        })
    }
}

impl Downloader for NativeDownloader {
    fn download(
        &self,
        url: &str,
        dest: &Path,
        on_progress: &mut dyn FnMut(DownloadProgress),
    ) -> Result<(), DownloadError> {
        // Two-attempt loop for transient errors.
        let mut last_err: Option<DownloadError> = None;
        for _ in 0..self.max_attempts.max(1) {
            match self.try_once(url, dest, on_progress) {
                Ok(()) => return Ok(()),
                Err(e) if e.is_retryable() => {
                    last_err = Some(e);
                    std::thread::sleep(self.backoff);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| DownloadError::Other("unknown".into())))
    }
}

impl NativeDownloader {
    fn try_once(
        &self,
        url: &str,
        dest: &Path,
        on_progress: &mut dyn FnMut(DownloadProgress),
    ) -> Result<(), DownloadError> {
        let mut resp = self.client.get(url).send()?;
        let status = resp.status();
        if !status.is_success() {
            return Err(DownloadError::HttpStatus(status.as_u16()));
        }
        let total = resp
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        on_progress(DownloadProgress::Started {
            url: url.to_string(),
            total,
        });
        let tmp = dest.with_extension("tmp");
        if let Some(parent) = tmp.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&tmp)?;
        let mut buffer = [0_u8; 64 * 1024];
        let mut written = 0_u64;
        loop {
            let n = resp.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n])?;
            written += n as u64;
            on_progress(DownloadProgress::Progress {
                url: url.to_string(),
                bytes: written,
                total,
            });
        }
        file.flush()?;
        std::fs::rename(&tmp, dest)?;
        on_progress(DownloadProgress::Finished {
            url: url.to_string(),
            bytes: written,
        });
        Ok(())
    }
}

fn build_blocking_client() -> Result<Client, DownloadError> {
    build_blocking_client_with_cookie(None)
}

fn build_blocking_client_with_cookie(
    cookie_header: Option<String>,
) -> Result<Client, DownloadError> {
    let mut headers = header::HeaderMap::new();
    if let Some(cookie_header) = cookie_header {
        let value = header::HeaderValue::from_str(&cookie_header)
            .map_err(|e| DownloadError::Other(e.to_string()))?;
        headers.insert(header::COOKIE, value);
    }
    Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .map_err(DownloadError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_error_is_retryable_for_5xx() {
        assert!(DownloadError::HttpStatus(503).is_retryable());
        assert!(!DownloadError::HttpStatus(404).is_retryable());
    }

    #[test]
    fn download_error_cancelled_is_not_retryable() {
        assert!(!DownloadError::Cancelled.is_retryable());
    }

    #[test]
    fn native_downloader_default_has_three_attempts() {
        let d = NativeDownloader::default();
        assert_eq!(d.max_attempts, 3);
    }
}
