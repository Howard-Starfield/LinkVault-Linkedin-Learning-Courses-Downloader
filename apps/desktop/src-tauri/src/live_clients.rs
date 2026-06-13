use crate::artifact_downloader::{ArtifactDownloadError, ArtifactHttpClient, ArtifactHttpResponse};
use crate::auth::{LinkedInCookie, ValidatedLinkedInSession};
use crate::course::{CourseApiClient, CourseFetchError};
use reqwest::blocking::{Client, RequestBuilder};
use std::time::Duration;
use thiserror::Error;
use url::Url;

const LINKEDIN_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:88.0) Gecko/20100101 Firefox/88.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactRequestMode {
    Plain,
    Authenticated,
}

#[derive(Clone)]
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
            .cookie_store(true)
            .user_agent(LINKEDIN_USER_AGENT)
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| LiveClientError::Client(error.to_string()))?;

        Ok(Self {
            client,
            cookie_header: build_cookie_header(token, &session.cookies, &session.csrf_token),
            request_headers: session.request_headers.clone(),
        })
    }

    fn build_get_request(&self, url: &str) -> RequestBuilder {
        apply_session_headers(
            self.client.get(url).header("Cookie", &self.cookie_header),
            &self.request_headers,
        )
    }

    fn build_artifact_get_request(&self, url: &str, mode: ArtifactRequestMode) -> RequestBuilder {
        match mode {
            ArtifactRequestMode::Plain => self.client.get(url),
            ArtifactRequestMode::Authenticated => apply_session_headers(
                self.client.get(url).header("Cookie", &self.cookie_header),
                &self.request_headers,
            ),
        }
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
            .map_err(|error| CourseFetchError::Api(classify_reqwest_error(&error)))?;
        let status = response.status();
        if !status.is_success() {
            return Err(CourseFetchError::Http {
                status: status.as_u16(),
            });
        }
        response
            .text()
            .map_err(|error| CourseFetchError::Api(error.to_string()))
    }

    fn get_with_headers(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<String, CourseFetchError> {
        let response = apply_session_headers(self.build_get_request(url), headers)
            .send()
            .map_err(|error| CourseFetchError::Api(classify_reqwest_error(&error)))?;
        let status = response.status();
        if !status.is_success() {
            return Err(CourseFetchError::Http {
                status: status.as_u16(),
            });
        }
        response
            .text()
            .map_err(|error| CourseFetchError::Api(error.to_string()))
    }
}

impl ArtifactHttpClient for AuthenticatedLinkedInClient {
    fn get_bytes(&mut self, url: &str) -> Result<ArtifactHttpResponse, ArtifactDownloadError> {
        let mut last_response = None;
        for mode in artifact_request_modes_for_url(url) {
            let response = self
                .build_artifact_get_request(url, mode)
                .send()
                .map_err(|error| ArtifactDownloadError::Network(classify_reqwest_error(&error)))?;
            let status = response.status().as_u16();
            let bytes = response
                .bytes()
                .map_err(|error| ArtifactDownloadError::Network(classify_reqwest_error(&error)))?
                .to_vec();
            let artifact_response = ArtifactHttpResponse { status, bytes };
            if (200..300).contains(&status) {
                return Ok(artifact_response);
            }
            last_response = Some(artifact_response);
        }

        Ok(last_response.unwrap_or(ArtifactHttpResponse {
            status: 0,
            bytes: Vec::new(),
        }))
    }
}

fn classify_reqwest_error(error: &reqwest::Error) -> String {
    let mut labels = Vec::new();
    if error.is_timeout() {
        labels.push("timeout");
    }
    if error.is_connect() {
        labels.push("connect");
    }
    if error.is_redirect() {
        labels.push("redirect");
    }
    if error.is_request() {
        labels.push("request");
    }
    if error.is_body() {
        labels.push("body");
    }
    if error.is_decode() {
        labels.push("decode");
    }
    if labels.is_empty() {
        labels.push("unknown");
    }
    labels.join("+")
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

fn build_cookie_header(li_at: &str, cookies: &[LinkedInCookie], jsessionid: &str) -> String {
    let mut header_parts = vec![format!("li_at={}", li_at.trim())];
    let mut has_jsessionid = false;

    for cookie in cookies {
        let name = cookie.name.trim();
        let value = cookie.value.trim();
        if name.is_empty() || value.is_empty() || name.eq_ignore_ascii_case("li_at") {
            continue;
        }
        if name.eq_ignore_ascii_case("JSESSIONID") {
            has_jsessionid = true;
        }
        header_parts.push(format!("{name}={value}"));
    }

    if !has_jsessionid {
        header_parts.push(format!("JSESSIONID={}", jsessionid.trim()));
    }

    header_parts.join("; ")
}

fn artifact_request_modes_for_url(url: &str) -> Vec<ArtifactRequestMode> {
    if is_linkedin_host(url) {
        vec![ArtifactRequestMode::Authenticated]
    } else {
        vec![ArtifactRequestMode::Plain]
    }
}

fn is_linkedin_host(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_ascii_lowercase()))
        .is_some_and(|host| host == "linkedin.com" || host.ends_with(".linkedin.com"))
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
            cookies: vec![
                LinkedInCookie {
                    name: "JSESSIONID".to_string(),
                    value: "ajax:123".to_string(),
                },
                LinkedInCookie {
                    name: "bcookie".to_string(),
                    value: "browser-cookie".to_string(),
                },
            ],
        };

        let client = AuthenticatedLinkedInClient::new(" li-at-token ", &session).unwrap();

        assert_eq!(
            client.cookie_header_for_test(),
            "li_at=li-at-token; JSESSIONID=ajax:123; bcookie=browser-cookie"
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
            cookies: Vec::new(),
        };

        assert!(matches!(
            AuthenticatedLinkedInClient::new(" ", &session),
            Err(LiveClientError::EmptyToken)
        ));
    }

    #[test]
    fn non_linkedin_artifact_downloads_stay_plain() {
        let session = ValidatedLinkedInSession {
            csrf_token: "ajax:123".to_string(),
            enterprise_profile_hash: None,
            request_headers: vec![("Csrf-Token".to_string(), "ajax:123".to_string())],
            cookies: vec![LinkedInCookie {
                name: "JSESSIONID".to_string(),
                value: "ajax:123".to_string(),
            }],
        };
        let client = AuthenticatedLinkedInClient::new("li-at-token", &session).unwrap();

        let cdn_request = client
            .build_artifact_get_request(
                "https://files3.lynda.com/exercise.zip",
                ArtifactRequestMode::Plain,
            )
            .build()
            .unwrap();

        assert!(cdn_request.headers().get("Cookie").is_none());
        assert!(cdn_request.headers().get("Csrf-Token").is_none());
    }

    #[test]
    fn linkedin_artifact_urls_use_authenticated_request_directly() {
        assert_eq!(
            artifact_request_modes_for_url("https://www.linkedin.com/ambry/?x=1"),
            vec![ArtifactRequestMode::Authenticated]
        );
        assert_eq!(
            artifact_request_modes_for_url("https://files3.lynda.com/exercise.zip"),
            vec![ArtifactRequestMode::Plain]
        );
    }

    #[test]
    fn authenticated_artifact_retry_mode_sends_session_headers() {
        let session = ValidatedLinkedInSession {
            csrf_token: "ajax:123".to_string(),
            enterprise_profile_hash: None,
            request_headers: vec![("Csrf-Token".to_string(), "ajax:123".to_string())],
            cookies: vec![LinkedInCookie {
                name: "JSESSIONID".to_string(),
                value: "ajax:123".to_string(),
            }],
        };
        let client = AuthenticatedLinkedInClient::new("li-at-token", &session).unwrap();

        let request = client
            .build_artifact_get_request(
                "https://www.linkedin.com/ambry/?x=1",
                ArtifactRequestMode::Authenticated,
            )
            .build()
            .unwrap();

        assert!(request.headers().get("Cookie").is_some());
        assert!(request.headers().get("Csrf-Token").is_some());
    }
}
