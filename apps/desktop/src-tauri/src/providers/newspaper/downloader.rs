use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use image::GenericImageView;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt};
use url::Url;

use super::client::{FetchError, NewspaperClient};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedPage {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub checksum_sha256: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error)]
pub enum PageDownloadError {
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error("downloaded page is not a decodable image: {0}")]
    InvalidImage(#[from] image::ImageError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("newspaper edition has not been released yet")]
    NotReleased,
}

pub async fn download_validated_page(
    client: &NewspaperClient,
    page_url: Url,
    referer: &str,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<DownloadedPage, PageDownloadError> {
    let response = client.fetch_page(page_url, referer, cancelled).await?;
    if is_unreleased_placeholder(&response.content_type, &response.bytes) {
        return Err(PageDownloadError::NotReleased);
    }
    let bytes = response.bytes;
    let decoded = decode_page_image(bytes).await?;
    let part_path = sibling_part_path(destination);

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = fs::File::create(&part_path).await?;
    file.write_all(&decoded.bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    if fs::try_exists(destination).await? {
        fs::remove_file(destination).await?;
    }
    fs::rename(&part_path, destination).await?;

    Ok(DownloadedPage {
        path: destination.to_path_buf(),
        size_bytes: decoded.size_bytes,
        checksum_sha256: decoded.checksum_sha256,
        width: decoded.width,
        height: decoded.height,
    })
}

fn is_unreleased_placeholder(content_type: &str, bytes: &[u8]) -> bool {
    let text = std::str::from_utf8(bytes)
        .ok()
        .map(str::trim)
        .unwrap_or_default();
    text.eq_ignore_ascii_case("future date")
        || (content_type.to_ascii_lowercase().starts_with("text/html")
            && text.to_ascii_lowercase().contains("future date"))
}

pub async fn validate_existing_page(path: &Path) -> Result<DownloadedPage, PageDownloadError> {
    let path = path.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = std::fs::read(&path)?;
        let decoded = inspect_page_bytes(bytes)?;
        Ok(DownloadedPage {
            path,
            size_bytes: decoded.size_bytes,
            checksum_sha256: decoded.checksum_sha256,
            width: decoded.width,
            height: decoded.height,
        })
    })
    .await
    .map_err(|error| {
        PageDownloadError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            error.to_string(),
        ))
    })?
}

struct DecodedPage {
    bytes: Vec<u8>,
    size_bytes: u64,
    checksum_sha256: String,
    width: u32,
    height: u32,
}

async fn decode_page_image(bytes: Vec<u8>) -> Result<DecodedPage, PageDownloadError> {
    tauri::async_runtime::spawn_blocking(move || inspect_page_bytes(bytes))
        .await
        .map_err(|error| {
            PageDownloadError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                error.to_string(),
            ))
        })?
}

fn inspect_page_bytes(bytes: Vec<u8>) -> Result<DecodedPage, PageDownloadError> {
    let image = image::load_from_memory(&bytes)?;
    let (width, height) = image.dimensions();
    Ok(DecodedPage {
        size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        checksum_sha256: format!("{:x}", Sha256::digest(&bytes)),
        width,
        height,
        bytes,
    })
}

pub fn sibling_part_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    destination.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use tempfile::tempdir;
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    const ONE_PIXEL_PNG: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    #[test]
    fn part_path_is_a_sibling_and_keeps_the_original_extension_visible() {
        assert_eq!(
            sibling_part_path(Path::new("C:/papers/A01.jpg")),
            PathBuf::from("C:/papers/A01.jpg.part")
        );
    }

    #[tokio::test]
    async fn valid_image_is_atomically_promoted_with_metadata() {
        let server = MockServer::start().await;
        let bytes = STANDARD.decode(ONE_PIXEL_PNG).unwrap();
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server)
            .await;
        let client = NewspaperClient::for_test(Url::parse(&server.uri()).unwrap());
        let directory = tempdir().unwrap();
        let destination = directory.path().join("A01.png");

        let result = download_validated_page(
            &client,
            Url::parse(&server.uri()).unwrap(),
            &server.uri(),
            &destination,
            &AtomicBool::new(false),
        )
        .await
        .unwrap();

        assert_eq!((result.width, result.height), (1, 1));
        assert!(destination.exists());
        assert!(!sibling_part_path(&destination).exists());
        assert_eq!(result.checksum_sha256.len(), 64);
    }

    #[tokio::test]
    async fn invalid_image_is_rejected_before_any_file_is_written() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"<html>not an image</html>"))
            .mount(&server)
            .await;
        let client = NewspaperClient::for_test(Url::parse(&server.uri()).unwrap());
        let directory = tempdir().unwrap();
        let destination = directory.path().join("A01.jpg");

        assert!(matches!(
            download_validated_page(
                &client,
                Url::parse(&server.uri()).unwrap(),
                &server.uri(),
                &destination,
                &AtomicBool::new(false),
            )
            .await,
            Err(PageDownloadError::InvalidImage(_))
        ));
        assert!(!destination.exists());
        assert!(!sibling_part_path(&destination).exists());
    }

    #[tokio::test]
    async fn future_date_placeholder_is_not_written() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_bytes(b"Future date"),
            )
            .mount(&server)
            .await;
        let client = NewspaperClient::for_test(Url::parse(&server.uri()).unwrap());
        let directory = tempdir().unwrap();
        let destination = directory.path().join("A01.jpg");

        assert!(matches!(
            download_validated_page(
                &client,
                Url::parse(&server.uri()).unwrap(),
                &server.uri(),
                &destination,
                &AtomicBool::new(false),
            )
            .await,
            Err(PageDownloadError::NotReleased)
        ));
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn existing_valid_page_can_be_revalidated_for_restart_skip() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("A01.png");
        fs::write(&destination, STANDARD.decode(ONE_PIXEL_PNG).unwrap())
            .await
            .unwrap();

        let result = validate_existing_page(&destination).await.unwrap();
        assert_eq!((result.width, result.height), (1, 1));
        assert_eq!(result.checksum_sha256.len(), 64);
    }

    #[test]
    fn image_decode_runs_on_the_blocking_pool() {
        let source = include_str!("downloader.rs");
        assert!(
            source.contains("tauri::async_runtime::spawn_blocking"),
            "image decode and hashing must not run on the async executor"
        );
        assert!(
            source.contains("decode_page_image"),
            "CPU-bound page inspection should live in a dedicated helper"
        );
    }
}
