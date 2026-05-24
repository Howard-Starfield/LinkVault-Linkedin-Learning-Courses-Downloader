use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use thiserror::Error;

const LINKEDIN_LEARNING_HOME: &str = "https://www.linkedin.com/learning";
const LINKEDIN_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:88.0) Gecko/20100101 Firefox/88.0";

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
pub enum BrowserSource {
    Chrome,
    Edge,
    Firefox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCandidate {
    pub source: BrowserSource,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedInCookie {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedInHomeResponse {
    pub html: String,
    pub cookies: Vec<LinkedInCookie>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidatedLinkedInSession {
    pub csrf_token: String,
    pub enterprise_profile_hash: Option<String>,
    pub request_headers: Vec<(String, String)>,
    #[serde(skip)]
    pub cookies: Vec<LinkedInCookie>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenValidationError {
    #[error("li_at token is required")]
    EmptyToken,
    #[error("LinkedIn returned a trial prompt for this account")]
    TrialPrompt,
    #[error("LinkedIn session did not include JSESSIONID")]
    MissingSessionCookie,
    #[error("no valid browser token candidates were found")]
    NoValidBrowserToken,
    #[error("failed to fetch LinkedIn Learning home: {0}")]
    HomeFetch(String),
}

pub trait LinkedInHomeClient {
    fn get_learning_home(
        &mut self,
        li_at: &str,
    ) -> Result<LinkedInHomeResponse, TokenValidationError>;
}

pub struct ReqwestLinkedInHomeClient {
    client: reqwest::blocking::Client,
}

impl ReqwestLinkedInHomeClient {
    pub fn new() -> Result<Self, TokenValidationError> {
        let client = reqwest::blocking::Client::builder()
            .cookie_store(true)
            .user_agent(LINKEDIN_USER_AGENT)
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| TokenValidationError::HomeFetch(error.to_string()))?;

        Ok(Self { client })
    }
}

impl LinkedInHomeClient for ReqwestLinkedInHomeClient {
    fn get_learning_home(
        &mut self,
        li_at: &str,
    ) -> Result<LinkedInHomeResponse, TokenValidationError> {
        let response = self
            .client
            .get(LINKEDIN_LEARNING_HOME)
            .header(reqwest::header::COOKIE, format!("li_at={li_at}"))
            .send()
            .map_err(|error| TokenValidationError::HomeFetch(error.to_string()))?;

        let cookies = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(parse_set_cookie)
            .collect();

        let html = response
            .text()
            .map_err(|error| TokenValidationError::HomeFetch(error.to_string()))?;

        Ok(LinkedInHomeResponse { html, cookies })
    }
}

pub fn validate_li_at_with_client(
    li_at: &str,
    client: &mut impl LinkedInHomeClient,
) -> Result<ValidatedLinkedInSession, TokenValidationError> {
    let token = li_at.trim();
    if token.is_empty() {
        return Err(TokenValidationError::EmptyToken);
    }

    let response = client.get_learning_home(token)?;
    let html = decode_linkedin_html(&response.html);
    if has_trial_prompt(&html) {
        return Err(TokenValidationError::TrialPrompt);
    }

    let csrf_token = response
        .cookies
        .iter()
        .find(|cookie| cookie.name.eq_ignore_ascii_case("JSESSIONID"))
        .map(|cookie| cookie.value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or(TokenValidationError::MissingSessionCookie)?;

    let enterprise_profile_hash = extract_enterprise_profile_hash(&html);
    let mut request_headers = vec![("Csrf-Token".to_string(), csrf_token.clone())];
    if let Some(hash) = &enterprise_profile_hash {
        request_headers.push(("x-li-identity".to_string(), hash.clone()));
    }

    Ok(ValidatedLinkedInSession {
        csrf_token,
        enterprise_profile_hash,
        request_headers,
        cookies: response.cookies,
    })
}

pub fn select_first_valid_browser_token(
    candidates: &[TokenCandidate],
    client: &mut impl LinkedInHomeClient,
) -> Result<(TokenCandidate, ValidatedLinkedInSession), TokenValidationError> {
    for candidate in distinct_non_empty_candidates(candidates) {
        if let Ok(session) = validate_li_at_with_client(&candidate.value, client) {
            return Ok((candidate, session));
        }
    }

    Err(TokenValidationError::NoValidBrowserToken)
}

pub fn has_trial_prompt(html: &str) -> bool {
    let normalized = html.to_lowercase();
    normalized
        .find("nav__button-tertiary")
        .and_then(|start| normalized[start..].find("start free trial"))
        .is_some()
}

pub fn extract_enterprise_profile_hash(html: &str) -> Option<String> {
    let decoded = decode_linkedin_html(html);
    let marker = "\"enterpriseProfileHash\":\"";
    let start = decoded.find(marker)? + marker.len();
    let rest = &decoded[start..];
    let end = rest.find('"')?;
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn distinct_non_empty_candidates(candidates: &[TokenCandidate]) -> Vec<TokenCandidate> {
    let mut seen = HashSet::new();
    let mut distinct = Vec::new();

    for candidate in candidates {
        let trimmed = candidate.value.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }

        distinct.push(TokenCandidate {
            source: candidate.source,
            value: trimmed.to_string(),
        });
    }

    distinct
}

fn decode_linkedin_html(html: &str) -> String {
    html.replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x22;", "\"")
        .replace("&amp;", "&")
}

fn parse_set_cookie(header: &str) -> Option<LinkedInCookie> {
    let first = header.split(';').next()?;
    let (name, value) = first.split_once('=')?;
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() || value.is_empty() {
        None
    } else {
        Some(LinkedInCookie {
            name: name.to_string(),
            value: value.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_li_at_requires_jsessionid_and_sets_csrf_header() {
        let mut client = FakeHomeClient::new(vec![Ok(home(
            "<html><script>\"enterpriseProfileHash\":\"urn-li-enterprise-profile\"</script></html>",
            &[("JSESSIONID", "ajax:123")],
        ))]);

        let session = validate_li_at_with_client(" li-at-token ", &mut client).unwrap();

        assert_eq!(client.seen_tokens, vec!["li-at-token"]);
        assert_eq!(session.csrf_token, "ajax:123");
        assert_eq!(
            session.cookies,
            vec![LinkedInCookie {
                name: "JSESSIONID".to_string(),
                value: "ajax:123".to_string()
            }]
        );
        assert_eq!(
            session.request_headers,
            vec![
                ("Csrf-Token".to_string(), "ajax:123".to_string()),
                (
                    "x-li-identity".to_string(),
                    "urn-li-enterprise-profile".to_string()
                )
            ]
        );
    }

    #[test]
    fn trial_prompt_rejects_token_even_with_session_cookie() {
        let mut client = FakeHomeClient::new(vec![Ok(home(
            "<a class=\"NAV__BUTTON-TERTIARY other-class\">\r\nStart free trial</a>",
            &[("JSESSIONID", "ajax:123")],
        ))]);

        let error = validate_li_at_with_client("li-at-token", &mut client).unwrap_err();

        assert_eq!(error, TokenValidationError::TrialPrompt);
    }

    #[test]
    fn missing_jsessionid_rejects_token() {
        let mut client = FakeHomeClient::new(vec![Ok(home("<html>signed in shell</html>", &[]))]);

        let error = validate_li_at_with_client("li-at-token", &mut client).unwrap_err();

        assert_eq!(error, TokenValidationError::MissingSessionCookie);
    }

    #[test]
    fn enterprise_profile_hash_supports_html_encoded_bootstrap_json() {
        let html = "<script>{&quot;enterpriseProfileHash&quot;:&quot;urn-li-enterprise-profile&quot;}</script>";

        assert_eq!(
            extract_enterprise_profile_hash(html),
            Some("urn-li-enterprise-profile".to_string())
        );
    }

    #[test]
    fn browser_token_selection_tries_distinct_candidates_and_returns_first_valid() {
        let candidates = vec![
            TokenCandidate {
                source: BrowserSource::Chrome,
                value: "expired-token".to_string(),
            },
            TokenCandidate {
                source: BrowserSource::Chrome,
                value: "expired-token".to_string(),
            },
            TokenCandidate {
                source: BrowserSource::Edge,
                value: "valid-token".to_string(),
            },
        ];
        let mut client = FakeHomeClient::new(vec![
            Ok(home("<html></html>", &[])),
            Ok(home("<html></html>", &[("JSESSIONID", "ajax:456")])),
        ]);

        let (candidate, session) =
            select_first_valid_browser_token(&candidates, &mut client).unwrap();

        assert_eq!(client.seen_tokens, vec!["expired-token", "valid-token"]);
        assert_eq!(candidate.source, BrowserSource::Edge);
        assert_eq!(candidate.value, "valid-token");
        assert_eq!(session.csrf_token, "ajax:456");
    }

    #[test]
    fn browser_token_selection_reports_empty_when_no_candidate_validates() {
        let candidates = vec![TokenCandidate {
            source: BrowserSource::Firefox,
            value: "expired-token".to_string(),
        }];
        let mut client = FakeHomeClient::new(vec![Ok(home("<html></html>", &[]))]);

        let error = select_first_valid_browser_token(&candidates, &mut client).unwrap_err();

        assert_eq!(error, TokenValidationError::NoValidBrowserToken);
    }

    #[test]
    fn parses_jsessionid_from_set_cookie_header() {
        let cookie =
            parse_set_cookie("JSESSIONID=ajax:789; Path=/; Domain=.linkedin.com; Secure").unwrap();

        assert_eq!(
            cookie,
            LinkedInCookie {
                name: "JSESSIONID".to_string(),
                value: "ajax:789".to_string()
            }
        );
    }

    fn home(html: &str, cookies: &[(&str, &str)]) -> LinkedInHomeResponse {
        LinkedInHomeResponse {
            html: html.to_string(),
            cookies: cookies
                .iter()
                .map(|(name, value)| LinkedInCookie {
                    name: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
        }
    }

    struct FakeHomeClient {
        responses: Vec<Result<LinkedInHomeResponse, TokenValidationError>>,
        seen_tokens: Vec<String>,
    }

    impl FakeHomeClient {
        fn new(responses: Vec<Result<LinkedInHomeResponse, TokenValidationError>>) -> Self {
            Self {
                responses: responses.into_iter().rev().collect(),
                seen_tokens: Vec::new(),
            }
        }
    }

    impl LinkedInHomeClient for FakeHomeClient {
        fn get_learning_home(
            &mut self,
            li_at: &str,
        ) -> Result<LinkedInHomeResponse, TokenValidationError> {
            self.seen_tokens.push(li_at.to_string());
            self.responses.pop().unwrap_or_else(|| {
                Err(TokenValidationError::HomeFetch(
                    "fake response queue exhausted".to_string(),
                ))
            })
        }
    }
}
