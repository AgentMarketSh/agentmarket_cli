//! Remote IPFS pinning service integration.
//!
//! Provides a Pinata-compatible pinning client for ensuring content persistence
//! beyond the local IPFS node. The API key is configured via the
//! `AGENTMARKET_IPFS_PIN_KEY` environment variable.

use anyhow::{Context, Result};
use reqwest::multipart;
use tracing::debug;

/// Environment variable name for the Pinata API key.
const PIN_KEY_ENV: &str = "AGENTMARKET_IPFS_PIN_KEY";

/// Pinata API base URL.
const PINATA_API_URL: &str = "https://api.pinata.cloud";

// ---------------------------------------------------------------------------
// Internal response types
// ---------------------------------------------------------------------------

/// JSON body returned by the Pinata `/pinning/pinFileToIPFS` endpoint.
#[derive(serde::Deserialize)]
struct PinFileResponse {
    #[serde(rename = "IpfsHash")]
    ipfs_hash: String,
}

// ---------------------------------------------------------------------------
// PinningService
// ---------------------------------------------------------------------------

/// Remote pinning service client (Pinata-compatible).
///
/// Wraps the Pinata HTTP API to pin content by CID or by uploading bytes
/// directly. Authentication is via a Bearer JWT token passed in the
/// `Authorization` header.
///
/// Pinning is optional -- if no API key is configured the caller should
/// simply skip remote pinning and rely on the local IPFS node.
pub struct PinningService {
    api_key: String,
    http: reqwest::Client,
    api_url: String,
}

impl PinningService {
    /// Creates a new pinning service client.
    ///
    /// `api_key` is the Pinata JWT (or legacy API key) used for
    /// authentication.
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            http: reqwest::Client::new(),
            api_url: PINATA_API_URL.to_string(),
        }
    }

    /// Creates a pinning service client from the `AGENTMARKET_IPFS_PIN_KEY`
    /// environment variable.
    ///
    /// Returns `None` if the variable is not set or is empty, since remote
    /// pinning is optional.
    pub fn from_env() -> Option<Self> {
        match std::env::var(PIN_KEY_ENV) {
            Ok(key) if !key.is_empty() => {
                debug!("pinning service configured from environment");
                Some(Self::new(&key))
            }
            _ => {
                debug!("no pinning service API key found in environment");
                None
            }
        }
    }

    /// Returns `true` if a pinning service API key is configured via the
    /// environment.
    pub fn is_configured() -> bool {
        std::env::var(PIN_KEY_ENV)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Pins a CID by hash (Pinata "pin by hash" endpoint).
    ///
    /// Sends `POST /pinning/pinByHash` with `{"hashToPin": "<cid>"}`.
    /// Pinata will fetch the content from the IPFS network and pin it on
    /// their infrastructure.
    pub async fn pin_by_hash(&self, cid: &str) -> Result<()> {
        let url = format!("{}/pinning/pinByHash", self.api_url);
        debug!(url = %url, cid = %cid, "pinning CID by hash");

        let body = serde_json::json!({ "hashToPin": cid });

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("failed to POST to pin-by-hash endpoint: {url}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("pin-by-hash request failed with status {status}: {body}");
        }

        debug!(cid = %cid, "CID pinned by hash successfully");
        Ok(())
    }

    /// Pins content directly by uploading bytes (Pinata "pin file to IPFS").
    ///
    /// Sends `POST /pinning/pinFileToIPFS` as a multipart form with the file
    /// content attached. Returns the CID of the pinned content.
    pub async fn pin_bytes(&self, content: &[u8], name: &str) -> Result<String> {
        let url = format!("{}/pinning/pinFileToIPFS", self.api_url);
        debug!(url = %url, name = %name, size = content.len(), "pinning bytes directly");

        let part = multipart::Part::bytes(content.to_vec())
            .file_name(name.to_string())
            .mime_str("application/octet-stream")
            .context("failed to create multipart part")?;

        let form = multipart::Form::new().part("file", part);

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("failed to POST to pin-file endpoint: {url}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("pin-file request failed with status {status}: {body}");
        }

        let pin_resp: PinFileResponse = response
            .json()
            .await
            .context("failed to parse pin-file response")?;

        debug!(cid = %pin_resp.ipfs_hash, name = %name, "content pinned successfully");
        Ok(pin_resp.ipfs_hash)
    }

    /// Checks if the pinning service is reachable and the API key is valid.
    ///
    /// Sends `GET /data/testAuthentication`. Returns `true` if the server
    /// responds with a success status code.
    pub async fn test_authentication(&self) -> Result<bool> {
        let url = format!("{}/data/testAuthentication", self.api_url);
        debug!(url = %url, "testing pinning service authentication");

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .with_context(|| format!("failed to reach pinning service auth endpoint: {url}"))?;

        let ok = response.status().is_success();
        debug!(authenticated = ok, "pinning service auth check complete");
        Ok(ok)
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Overrides the API base URL (useful for testing against a mock server).
    #[cfg(test)]
    fn with_api_url(mut self, url: &str) -> Self {
        self.api_url = url.trim_end_matches('/').to_string();
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_configured_returns_false_when_env_not_set() {
        // Temporarily remove the env var if it happens to be set.
        let prev = std::env::var(PIN_KEY_ENV).ok();
        std::env::remove_var(PIN_KEY_ENV);

        assert!(!PinningService::is_configured());

        // Restore previous value if it existed.
        if let Some(val) = prev {
            std::env::set_var(PIN_KEY_ENV, val);
        }
    }

    #[test]
    fn is_configured_returns_false_for_empty_key() {
        let prev = std::env::var(PIN_KEY_ENV).ok();
        std::env::set_var(PIN_KEY_ENV, "");

        assert!(!PinningService::is_configured());

        // Restore.
        match prev {
            Some(val) => std::env::set_var(PIN_KEY_ENV, val),
            None => std::env::remove_var(PIN_KEY_ENV),
        }
    }

    #[test]
    fn from_env_returns_none_when_env_not_set() {
        let prev = std::env::var(PIN_KEY_ENV).ok();
        std::env::remove_var(PIN_KEY_ENV);

        assert!(PinningService::from_env().is_none());

        if let Some(val) = prev {
            std::env::set_var(PIN_KEY_ENV, val);
        }
    }

    #[test]
    fn from_env_returns_some_when_key_set() {
        let prev = std::env::var(PIN_KEY_ENV).ok();
        std::env::set_var(PIN_KEY_ENV, "test-jwt-token");

        let svc = PinningService::from_env();
        assert!(svc.is_some());
        assert_eq!(svc.unwrap().api_key, "test-jwt-token");

        // Restore.
        match prev {
            Some(val) => std::env::set_var(PIN_KEY_ENV, val),
            None => std::env::remove_var(PIN_KEY_ENV),
        }
    }

    #[test]
    fn new_creates_client_with_correct_api_key() {
        let svc = PinningService::new("my-secret-key");
        assert_eq!(svc.api_key, "my-secret-key");
        assert_eq!(svc.api_url, PINATA_API_URL);
    }

    #[tokio::test]
    async fn test_authentication_returns_false_for_invalid_key() {
        // Use a dummy key against the real Pinata endpoint. The API should
        // return a non-success status (401), which we interpret as `false`.
        let svc = PinningService::new("invalid-key-for-testing");

        match svc.test_authentication().await {
            Ok(authenticated) => assert!(!authenticated),
            // A network error is also acceptable in CI environments that lack
            // outbound HTTPS access -- the important thing is that we do not
            // panic or return `Ok(true)`.
            Err(_) => {}
        }
    }

    #[test]
    fn with_api_url_overrides_base() {
        let svc = PinningService::new("key").with_api_url("http://localhost:9999/");
        assert_eq!(svc.api_url, "http://localhost:9999");
    }
}
