use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use reqwest::{header, Client, StatusCode, Url};
use thiserror::Error;

use super::manifest::{self, Manifest, ManifestError};

pub const CHROME_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("request was cancelled")]
    Cancelled,
    #[error("edition is unavailable")]
    Unavailable,
    #[error("server returned HTTP {0}")]
    Status(u16),
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
}

#[derive(Clone)]
pub struct NewspaperClient {
    client: Client,
    origin: Url,
    backoffs: Arc<[Duration]>,
}

impl NewspaperClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_origin_and_backoffs(
            Url::parse("https://ep.worldjournal.com").expect("static origin must be valid"),
            vec![
                Duration::from_secs(1),
                Duration::from_secs(3),
                Duration::from_secs(9),
            ],
        )
    }

    fn with_origin_and_backoffs(
        origin: Url,
        backoffs: Vec<Duration>,
    ) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(CHROME_USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            origin,
            backoffs: backoffs.into(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(origin: Url) -> Self {
        Self::with_origin_and_backoffs(origin, vec![Duration::ZERO, Duration::ZERO]).unwrap()
    }

    pub fn origin(&self) -> &Url {
        &self.origin
    }

    pub async fn fetch_manifest(
        &self,
        code: &str,
        publication_date: &str,
        cancelled: &AtomicBool,
    ) -> Result<Manifest, FetchError> {
        let url = self
            .origin
            .join(&format!(
                "/pub/{}/{}-{}.json",
                code.to_ascii_lowercase(),
                code,
                publication_date
            ))
            .map_err(|_| ManifestError::InvalidPageUrl(code.to_string()))?;
        let referer = self
            .origin
            .join(&format!("/{code}/{publication_date}"))
            .map_err(|_| ManifestError::InvalidPageUrl(code.to_string()))?;
        let response = self
            .get_with_retry(url, referer.as_str(), cancelled)
            .await?;
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response.bytes().await?;
        Ok(manifest::parse_with_origin(
            &content_type,
            &body,
            &self.origin,
        )?)
    }

    pub async fn fetch_page(
        &self,
        page_url: Url,
        referer: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, FetchError> {
        let response = self.get_with_retry(page_url, referer, cancelled).await?;
        Ok(response.bytes().await?.to_vec())
    }

    async fn get_with_retry(
        &self,
        url: Url,
        referer: &str,
        cancelled: &AtomicBool,
    ) -> Result<reqwest::Response, FetchError> {
        let mut retry_index = 0;
        loop {
            if cancelled.load(Ordering::SeqCst) {
                return Err(FetchError::Cancelled);
            }

            match self
                .client
                .get(url.clone())
                .header(header::REFERER, referer)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) if response.status() == StatusCode::NOT_FOUND => {
                    return Err(FetchError::Unavailable);
                }
                Ok(response)
                    if response.status().is_server_error() && retry_index < self.backoffs.len() =>
                {
                    self.wait_or_cancel(self.backoffs[retry_index], cancelled)
                        .await?;
                    retry_index += 1;
                }
                Ok(response) => return Err(FetchError::Status(response.status().as_u16())),
                Err(error) if retry_index < self.backoffs.len() => {
                    self.wait_or_cancel(self.backoffs[retry_index], cancelled)
                        .await?;
                    retry_index += 1;
                    if cancelled.load(Ordering::SeqCst) {
                        return Err(FetchError::Cancelled);
                    }
                    if retry_index > self.backoffs.len() {
                        return Err(FetchError::Network(error));
                    }
                }
                Err(error) => return Err(FetchError::Network(error)),
            }
        }
    }

    async fn wait_or_cancel(
        &self,
        duration: Duration,
        cancelled: &AtomicBool,
    ) -> Result<(), FetchError> {
        if duration.is_zero() {
            return if cancelled.load(Ordering::SeqCst) {
                Err(FetchError::Cancelled)
            } else {
                Ok(())
            };
        }

        const SLICE: Duration = Duration::from_millis(100);
        let mut remaining = duration;
        while !remaining.is_zero() {
            if cancelled.load(Ordering::SeqCst) {
                return Err(FetchError::Cancelled);
            }
            let sleep_for = remaining.min(SLICE);
            tokio::time::sleep(sleep_for).await;
            remaining = remaining.saturating_sub(sleep_for);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn valid_manifest_sends_referer_and_parses() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pub/ny/NY-2026-07-24.json"))
            .and(header("referer", format!("{}/NY/2026-07-24", server.uri())))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "application/json").set_body_raw(
                r#"{"sessions":[{"name":"A","pages":[{"pageno":"A01","name":"Front","pagefile":"/a01.png"}]}]}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let client = NewspaperClient::for_test(Url::parse(&server.uri()).unwrap());
        let manifest = client
            .fetch_manifest("NY", "2026-07-24", &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(manifest.pages().count(), 1);
    }

    #[tokio::test]
    async fn manifest_404_is_unavailable_without_retry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        let client = NewspaperClient::for_test(Url::parse(&server.uri()).unwrap());

        assert!(matches!(
            client
                .fetch_manifest("NY", "2026-07-24", &AtomicBool::new(false))
                .await,
            Err(FetchError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn server_errors_retry_within_the_configured_budget() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let client = NewspaperClient::for_test(Url::parse(&server.uri()).unwrap());

        assert!(matches!(
            client
                .fetch_manifest("NY", "2026-07-24", &AtomicBool::new(false))
                .await,
            Err(FetchError::Status(503))
        ));
    }

    #[tokio::test]
    async fn cancellation_short_circuits_before_network_access() {
        let server = MockServer::start().await;
        let client = NewspaperClient::for_test(Url::parse(&server.uri()).unwrap());
        let cancelled = AtomicBool::new(true);

        assert!(matches!(
            client.fetch_manifest("NY", "2026-07-24", &cancelled).await,
            Err(FetchError::Cancelled)
        ));
    }
}
