use crate::artifact_downloader::{ArtifactDownloadError, ArtifactHttpClient, ArtifactHttpResponse};
use crate::auth::ValidatedLinkedInSession;
use crate::course::{CourseApiClient, CourseFetchError};
use reqwest::blocking::{Client, RequestBuilder};
use thiserror::Error;

const LINKEDIN_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:88.0) Gecko/20100101 Firefox/88.0";

pub struct AuthenticatedLinkedInClient {
    client: Client,
    cookie_header: String,
    request_headers: Vec<(String, String)>,
}

#[derive(Debug, Error)]
pub enum LiveClientError {
    #[error("li_at token is required")]
    EmptyToken,
    #[error("failed to create LinkedIn client: {0}")]
    Client(String),
}

impl AuthenticatedLinkedInClient {
    pub fn new(li_at: &str, session: &ValidatedLinkedInSession) -> Result<Self, LiveClientError> {
        let token = li_at.trim();
        if token.is_empty() {
            return Err(LiveClientError::EmptyToken);
        }

        let client = Client::builder()
            .user_agent(LINKEDIN_USER_AGENT)
            .build()
            .map_err(|error| LiveClientError::Client(error.to_string()))?;

        Ok(Self {
            client,
            cookie_header: build_cookie_header(token, &session.csrf_token),
            request_headers: session.request_headers.clone(),
        })
    }

    fn build_get_request(&self, url: &str) -> RequestBuilder {
        apply_session_headers(
            self.client.get(url).header("Cookie", &self.cookie_header),
            &self.request_headers,
        )
    }

    #[cfg(test)]
    fn session_header_names(&self) -> Vec<String> {
        self.request_headers
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    #[cfg(test)]
    fn cookie_header_for_test(&self) -> &str {
        &self.cookie_header
    }
}

impl CourseApiClient for AuthenticatedLinkedInClient {
    fn get(&mut self, url: &str) -> Result<String, CourseFetchError> {
        let response = self
            .build_get_request(url)
            .send()
            .map_err(|error| CourseFetchError::Api(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(CourseFetchError::Api(format!(
                "HTTP status {}",
                status.as_u16()
            )));
        }
        response
            .text()
            .map_err(|error| CourseFetchError::Api(error.to_string()))
    }
}

impl ArtifactHttpClient for AuthenticatedLinkedInClient {
    fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError> {
        let response = self
            .build_get_request(url)
            .send()
            .map_err(|error| ArtifactDownloadError::Network(error.to_string()))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .map_err(|error| ArtifactDownloadError::Network(error.to_string()))?
            .to_vec();

        Ok(ArtifactHttpResponse { status, bytes })
    }
}

fn apply_session_headers(
    mut request: RequestBuilder,
    headers: &[(String, String)],
) -> RequestBuilder {
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    request
}

fn build_cookie_header(li_at: &str, jsessionid: &str) -> String {
    format!("li_at={}; JSESSIONID={}", li_at.trim(), jsessionid.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_client_builds_cookie_and_session_headers_without_persisting_token() {
        let session = ValidatedLinkedInSession {
            csrf_token: "ajax:123".to_string(),
            enterprise_profile_hash: Some("urn-li-enterprise-profile".to_string()),
            request_headers: vec![
                ("Csrf-Token".to_string(), "ajax:123".to_string()),
                (
                    "x-li-identity".to_string(),
                    "urn-li-enterprise-profile".to_string(),
                ),
            ],
        };

        let client = AuthenticatedLinkedInClient::new(" li-at-token ", &session).unwrap();

        assert_eq!(
            client.cookie_header_for_test(),
            "li_at=li-at-token; JSESSIONID=ajax:123"
        );
        assert_eq!(
            client.session_header_names(),
            vec!["Csrf-Token".to_string(), "x-li-identity".to_string()]
        );
    }

    #[test]
    fn authenticated_client_rejects_empty_token_before_request_creation() {
        let session = ValidatedLinkedInSession {
            csrf_token: "ajax:123".to_string(),
            enterprise_profile_hash: None,
            request_headers: vec![("Csrf-Token".to_string(), "ajax:123".to_string())],
        };

        assert!(matches!(
            AuthenticatedLinkedInClient::new(" ", &session),
            Err(LiveClientError::EmptyToken)
        ));
    }
}
