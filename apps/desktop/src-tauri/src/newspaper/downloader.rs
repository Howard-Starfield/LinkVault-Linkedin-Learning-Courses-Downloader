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
}

pub async fn download_validated_page(
    client: &NewspaperClient,
    page_url: Url,
    referer: &str,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<DownloadedPage, PageDownloadError> {
    let bytes = client.fetch_page(page_url, referer, cancelled).await?;
    let image = image::load_from_memory(&bytes)?;
    let (width, height) = image.dimensions();
    let checksum_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let part_path = sibling_part_path(destination);

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = fs::File::create(&part_path).await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    if fs::try_exists(destination).await? {
        fs::remove_file(destination).await?;
    }
    fs::rename(&part_path, destination).await?;

    Ok(DownloadedPage {
        path: destination.to_path_buf(),
        size_bytes,
        checksum_sha256,
        width,
        height,
    })
}

pub async fn validate_existing_page(path: &Path) -> Result<DownloadedPage, PageDownloadError> {
    let bytes = fs::read(path).await?;
    let image = image::load_from_memory(&bytes)?;
    let (width, height) = image.dimensions();
    Ok(DownloadedPage {
        path: path.to_path_buf(),
        size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        checksum_sha256: format!("{:x}", Sha256::digest(&bytes)),
        width,
        height,
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
}
