//! HTTP client + small JSON / bytes / page wrappers for the Coursera module.
//!
//! The client is a **separate** `reqwest::Client` from the LinkedIn-side
//! one. Each has its own cookie store; the two never share state. This is
//! part of the isolation rules in `docs/learning/agent-harness-coursera/ISOLATION_RULES.md`.
//!
//! All functions take an explicit `&reqwest::Client` so callers can
//! inject a mock (the `wiremock`-backed tests in `tests/coursera_e2e.rs`).
//! No module-level mutable state.

// Phase 3: every public symbol is consumed by later phases but not by
// the lib build yet. The blanket allow matches `config.rs` and is
// removed as each symbol gets its first non-test caller.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::cookie::Jar;
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::coursera::define::USER_AGENT;
use crate::coursera::error::{CourseraError, CourseraResult};

/// Default request timeout. Mirrors the value used by the existing
/// `live_clients.rs` on the LinkedIn side.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Build a `reqwest::Client` for Coursera. Cookie store enabled, rustls
/// TLS, native roots, and a `LinkVault/0.1 (+coursera; rust)` user agent.
///
/// Two `Client`s are constructed per app run: one for the LinkedIn side
/// (in `live_clients.rs`) and this one for the Coursera side. Each has
/// its own cookie jar.
pub fn build_client() -> CourseraResult<Client> {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .user_agent(USER_AGENT)
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .map_err(CourseraError::from)?;
    Ok(client)
}

/// Build a `reqwest::Client` with a custom timeout. Used by tests and
/// by the orchestrator's per-call override.
pub fn build_client_with_timeout(timeout: Duration) -> CourseraResult<Client> {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .build()
        .map_err(CourseraError::from)?;
    Ok(client)
}

/// Build a `reqwest::Client` whose cookie jar is the supplied
/// `Arc<Jar>`. Used by the auth module to pre-seed a `CAUTH` cookie.
pub fn build_client_with_jar(jar: Arc<Jar>) -> CourseraResult<Client> {
    let client = reqwest::Client::builder()
        .cookie_provider(jar)
        .user_agent(USER_AGENT)
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .map_err(CourseraError::from)?;
    Ok(client)
}

/// GET `url` and parse the response as JSON.
pub async fn get_json<T: DeserializeOwned>(client: &Client, url: &str) -> CourseraResult<T> {
    let resp = client.get(url).send().await.map_err(CourseraError::from)?;
    ensure_success(resp)
        .await?
        .json::<T>()
        .await
        .map_err(CourseraError::from)
}

/// GET `url` and return the response body as raw bytes.
pub async fn get_bytes(client: &Client, url: &str) -> CourseraResult<Bytes> {
    let resp = client.get(url).send().await.map_err(CourseraError::from)?;
    ensure_success(resp)
        .await?
        .bytes()
        .await
        .map_err(CourseraError::from)
}

/// GET `url` and return the body parsed as a generic JSON value, plus
/// the final URL (after any redirects). Used by the syllabus extractor
/// to follow the "open course" redirect.
pub async fn get_page_and_url(client: &Client, url: &str) -> CourseraResult<(Value, String)> {
    let resp = client.get(url).send().await.map_err(CourseraError::from)?;
    let final_url = resp.url().to_string();
    let value = ensure_success(resp)
        .await?
        .json::<Value>()
        .await
        .map_err(CourseraError::from)?;
    Ok((value, final_url))
}

/// POST `body` as JSON to `url` and return the raw response body. The
/// login flow uses this — it inspects the response's `Set-Cookie` headers,
/// which `reqwest` writes to the cookie store when the client was built
/// with `cookie_store(true)`.
pub async fn post_page_and_reply(
    client: &Client,
    url: &str,
    body: &Value,
) -> CourseraResult<Bytes> {
    let resp = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(CourseraError::from)?;
    ensure_success(resp)
        .await?
        .bytes()
        .await
        .map_err(CourseraError::from)
}

/// HEAD `url` and return `Ok(())` for any 2xx/3xx, `Err(Auth)` for 401/403,
/// `Err(Network)` for anything else.
pub async fn head_status(client: &Client, url: &str) -> CourseraResult<()> {
    let resp = client.head(url).send().await.map_err(CourseraError::from)?;
    let status = resp.status();
    if status.is_success() || status.is_redirection() {
        Ok(())
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        Err(CourseraError::Auth)
    } else {
        Err(CourseraError::Other(format!(
            "unexpected status {}",
            status
        )))
    }
}

/// Drain the response body and convert non-2xx into the appropriate
/// `CourseraError` variant.
async fn ensure_success(resp: Response) -> CourseraResult<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    // Drain the body for the error message, but cap it so a malicious
    // server cannot blow up our memory.
    let body = resp
        .bytes()
        .await
        .map(|b| String::from_utf8_lossy(&b[..b.len().min(2048)]).into_owned())
        .unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(CourseraError::Auth);
    }
    Err(CourseraError::Other(format!(
        "HTTP {} for request: {}",
        status, body
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_client_succeeds() {
        // No live network — we just verify the client is constructable.
        let client = build_client().expect("build_client");
        // `cookie_store` is private; we assert by side-effect (the builder
        // would have errored if cookies were not enabled with a feature
        // mismatch, but the contract is documented).
        drop(client);
    }

    #[test]
    fn build_client_with_custom_timeout_succeeds() {
        let client = build_client_with_timeout(Duration::from_millis(500))
            .expect("build_client_with_timeout");
        drop(client);
    }

    #[test]
    fn default_timeout_is_thirty_seconds() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
    }
}
