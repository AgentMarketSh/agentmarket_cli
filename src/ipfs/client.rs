//! IPFS HTTP API client.
//!
//! Provides a thin wrapper around the IPFS HTTP API (Kubo-compatible) and an
//! IPFS gateway for content retrieval. All network errors are returned as
//! [`anyhow::Error`] values -- the client never panics on unreachable nodes.

use anyhow::{Context, Result};
use reqwest::multipart;
use tracing::debug;

use crate::config::store::Config;

// ---------------------------------------------------------------------------
// Internal response types
// ---------------------------------------------------------------------------

/// JSON body returned by the IPFS `/api/v0/add` endpoint.
#[derive(serde::Deserialize)]
struct AddResponse {
    #[serde(rename = "Hash")]
    hash: String,
}

// ---------------------------------------------------------------------------
// IpfsClient
// ---------------------------------------------------------------------------

/// HTTP client for interacting with an IPFS node and gateway.
///
/// - `api_url` targets a local (or remote) Kubo-compatible IPFS HTTP API,
///   typically running on port 5001.
/// - `gateway_url` targets a public or private IPFS gateway used for fast
///   content retrieval. The gateway is tried first when fetching content; the
///   API endpoint is used as a fallback.
pub struct IpfsClient {
    api_url: String,
    gateway_url: String,
    http: reqwest::Client,
}

impl IpfsClient {
    /// Creates a new `IpfsClient` with explicit API and gateway URLs.
    ///
    /// URLs are stored as-is; trailing slashes are stripped for consistency.
    pub fn new(api_url: &str, gateway_url: &str) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            gateway_url: gateway_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Creates a new `IpfsClient` from the application configuration.
    ///
    /// Reads `config.network.ipfs_api` and `config.network.ipfs_gateway`.
    pub fn from_config(config: &Config) -> Self {
        Self::new(&config.network.ipfs_api, &config.network.ipfs_gateway)
    }

    /// Uploads content to IPFS via the HTTP API (`/api/v0/add`).
    ///
    /// Returns the CID (content identifier) of the newly added object.
    pub async fn add(&self, content: &[u8]) -> Result<String> {
        let url = format!("{}/api/v0/add", self.api_url);
        debug!(url = %url, size = content.len(), "adding content to IPFS");

        let part = multipart::Part::bytes(content.to_vec())
            .file_name("data")
            .mime_str("application/octet-stream")
            .context("failed to create multipart part")?;

        let form = multipart::Form::new().part("file", part);

        let response = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("failed to POST to IPFS add endpoint: {url}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "IPFS add request failed with status {status}: {body}"
            );
        }

        let add_resp: AddResponse = response
            .json()
            .await
            .context("failed to parse IPFS add response")?;

        debug!(cid = %add_resp.hash, "content added to IPFS");
        Ok(add_resp.hash)
    }

    /// Retrieves content by CID.
    ///
    /// The gateway URL is tried first (`{gateway_url}/ipfs/{cid}`). If that
    /// fails, the method falls back to the IPFS API (`/api/v0/cat?arg={cid}`).
    pub async fn cat(&self, cid: &str) -> Result<Vec<u8>> {
        // --- Attempt 1: gateway ---
        let gateway_url = format!("{}/ipfs/{}", self.gateway_url, cid);
        debug!(url = %gateway_url, "fetching content via gateway");

        match self.http.get(&gateway_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp
                    .bytes()
                    .await
                    .context("failed to read gateway response body")?;
                debug!(cid = %cid, size = bytes.len(), "content retrieved via gateway");
                return Ok(bytes.to_vec());
            }
            Ok(resp) => {
                debug!(
                    cid = %cid,
                    status = %resp.status(),
                    "gateway request returned non-success status, falling back to API"
                );
            }
            Err(err) => {
                debug!(
                    cid = %cid,
                    error = %err,
                    "gateway request failed, falling back to API"
                );
            }
        }

        // --- Attempt 2: API fallback ---
        let api_url = format!("{}/api/v0/cat?arg={}", self.api_url, cid);
        debug!(url = %api_url, "fetching content via API fallback");

        let response = self
            .http
            .post(&api_url)
            .send()
            .await
            .with_context(|| format!("failed to fetch CID {cid} from IPFS API"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "IPFS cat request failed with status {status}: {body}"
            );
        }

        let bytes = response
            .bytes()
            .await
            .context("failed to read IPFS API cat response body")?;

        debug!(cid = %cid, size = bytes.len(), "content retrieved via API");
        Ok(bytes.to_vec())
    }

    /// Pins an existing CID so the local IPFS node retains it.
    pub async fn pin(&self, cid: &str) -> Result<()> {
        let url = format!("{}/api/v0/pin/add?arg={}", self.api_url, cid);
        debug!(url = %url, cid = %cid, "pinning CID");

        let response = self
            .http
            .post(&url)
            .send()
            .await
            .with_context(|| format!("failed to pin CID {cid}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "IPFS pin request failed with status {status}: {body}"
            );
        }

        debug!(cid = %cid, "CID pinned successfully");
        Ok(())
    }

    /// Returns `true` if the IPFS API node is reachable.
    ///
    /// Sends a request to `/api/v0/id` and considers any successful HTTP
    /// response as proof of connectivity.
    pub async fn is_connected(&self) -> bool {
        let url = format!("{}/api/v0/id", self.api_url);
        debug!(url = %url, "checking IPFS connectivity");

        match self.http.post(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!("IPFS node is reachable");
                true
            }
            Ok(resp) => {
                debug!(status = %resp.status(), "IPFS node returned non-success status");
                false
            }
            Err(err) => {
                debug!(error = %err, "IPFS node is unreachable");
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trims_trailing_slashes() {
        let client = IpfsClient::new("http://localhost:5001/", "https://gw.example.com/");
        assert_eq!(client.api_url, "http://localhost:5001");
        assert_eq!(client.gateway_url, "https://gw.example.com");
    }

    #[test]
    fn from_config_uses_network_fields() {
        let config = Config::default();
        let client = IpfsClient::from_config(&config);
        assert_eq!(client.api_url, "http://localhost:5001");
        assert_eq!(client.gateway_url, "https://gateway.pinata.cloud");
    }

    #[tokio::test]
    async fn is_connected_returns_false_for_unreachable_node() {
        // Point at a port that is almost certainly not running an IPFS node.
        let client = IpfsClient::new("http://127.0.0.1:19999", "http://127.0.0.1:19998");
        assert!(!client.is_connected().await);
    }
}
