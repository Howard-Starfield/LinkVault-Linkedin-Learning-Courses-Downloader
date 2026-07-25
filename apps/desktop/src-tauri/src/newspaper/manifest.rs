use serde::Deserialize;
use thiserror::Error;
use url::Url;

const WORLD_JOURNAL_ORIGIN: &str = "https://ep.worldjournal.com";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    #[serde(default)]
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub name: Option<String>,
    #[serde(default)]
    pub pages: Vec<Page>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Page {
    pub pageno: String,
    pub name: Option<String>,
    pub pagefile: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest content type is not JSON")]
    InvalidContentType,
    #[error("manifest body is HTML")]
    HtmlBody,
    #[error("manifest JSON is malformed: {0}")]
    Malformed(String),
    #[error("manifest has no pages")]
    Empty,
    #[error("manifest contains an invalid page URL: {0}")]
    InvalidPageUrl(String),
}

impl Manifest {
    pub fn pages(&self) -> impl Iterator<Item = &Page> {
        self.sessions
            .iter()
            .flat_map(|session| session.pages.iter())
    }
}

pub fn parse(content_type: &str, body: &[u8]) -> Result<Manifest, ManifestError> {
    let origin = Url::parse(WORLD_JOURNAL_ORIGIN).expect("static origin must be valid");
    parse_with_origin(content_type, body, &origin)
}

pub fn parse_with_origin(
    content_type: &str,
    body: &[u8],
    origin: &Url,
) -> Result<Manifest, ManifestError> {
    if !content_type
        .to_ascii_lowercase()
        .contains("application/json")
    {
        return Err(ManifestError::InvalidContentType);
    }

    let first = body
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    if first != Some(b'{') {
        return Err(ManifestError::HtmlBody);
    }

    let manifest: Manifest = serde_json::from_slice(body)
        .map_err(|error| ManifestError::Malformed(error.to_string()))?;
    let pages: Vec<&Page> = manifest.pages().collect();
    if pages.is_empty() {
        return Err(ManifestError::Empty);
    }

    for page in pages {
        resolve_page_url_with_origin(&page.pagefile, origin)?;
    }
    Ok(manifest)
}

pub fn manifest_url(code: &str, publication_date: &str) -> Result<Url, ManifestError> {
    if code.len() != 2 || !code.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(ManifestError::InvalidPageUrl(code.to_string()));
    }

    Url::parse(&format!(
        "{WORLD_JOURNAL_ORIGIN}/pub/{}/{}-{}.json",
        code.to_ascii_lowercase(),
        code,
        publication_date
    ))
    .map_err(|_| ManifestError::InvalidPageUrl(code.to_string()))
}

pub fn referer_url(code: &str, publication_date: &str) -> Result<Url, ManifestError> {
    Url::parse(&format!("{WORLD_JOURNAL_ORIGIN}/{code}/{publication_date}"))
        .map_err(|_| ManifestError::InvalidPageUrl(code.to_string()))
}

pub fn resolve_page_url(pagefile: &str) -> Result<Url, ManifestError> {
    let origin = Url::parse(WORLD_JOURNAL_ORIGIN).expect("static origin must be valid");
    resolve_page_url_with_origin(pagefile, &origin)
}

pub fn resolve_page_url_with_origin(pagefile: &str, origin: &Url) -> Result<Url, ManifestError> {
    let url = origin
        .join(pagefile)
        .map_err(|_| ManifestError::InvalidPageUrl(pagefile.to_string()))?;

    if url.scheme() != origin.scheme()
        || url.host_str() != origin.host_str()
        || url.port_or_known_default() != origin.port_or_known_default()
    {
        return Err(ManifestError::InvalidPageUrl(pagefile.to_string()));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &[u8] = br#"{
        "sessions": [{
            "name": "A",
            "pages": [{"pageno":"A01","name":"Front","pagefile":"/pages/a01.jpg"}]
        }]
    }"#;

    #[test]
    fn valid_manifest_parses() {
        let manifest = parse("application/json; charset=utf-8", VALID).unwrap();
        assert_eq!(manifest.pages().count(), 1);
    }

    #[test]
    fn content_type_html_and_lying_html_body_are_distinct_errors() {
        assert_eq!(
            parse("text/html", b"<html></html>"),
            Err(ManifestError::InvalidContentType)
        );
        assert_eq!(
            parse("application/json", b" <!doctype html>"),
            Err(ManifestError::HtmlBody)
        );
    }

    #[test]
    fn malformed_and_empty_manifests_are_rejected() {
        assert!(matches!(
            parse("application/json", b"{"),
            Err(ManifestError::Malformed(_))
        ));
        assert_eq!(
            parse("application/json", br#"{"sessions":[]}"#),
            Err(ManifestError::Empty)
        );
    }

    #[test]
    fn page_urls_cannot_escape_the_world_journal_https_origin() {
        assert!(resolve_page_url("/pages/a01.jpg").is_ok());
        assert!(resolve_page_url("https://evil.example/a01.jpg").is_err());
        assert!(resolve_page_url("http://ep.worldjournal.com/a01.jpg").is_err());
    }

    #[test]
    fn manifest_and_referer_urls_follow_the_verified_site_pattern() {
        assert_eq!(
            manifest_url("NY", "2026-07-24").unwrap().as_str(),
            "https://ep.worldjournal.com/pub/ny/NY-2026-07-24.json"
        );
        assert_eq!(
            referer_url("NY", "2026-07-24").unwrap().as_str(),
            "https://ep.worldjournal.com/NY/2026-07-24"
        );
    }
}
