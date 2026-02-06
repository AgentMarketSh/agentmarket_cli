//! Base L2 RPC client.
//!
//! Provides a thin wrapper around an alloy HTTP provider for interacting with
//! the Base L2 network. Only exposes balance queries and connectivity checks
//! for now — transaction signing is deferred to T-021.

use std::time::Duration;

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use anyhow::{Context, Result};
use tracing::debug;

// ---------------------------------------------------------------------------
// ChainClient
// ---------------------------------------------------------------------------

/// Client for interacting with the Base L2 network over JSON-RPC.
///
/// Wraps an alloy [`RootProvider`] configured for HTTP transport. All public
/// methods return user-friendly error messages with no blockchain jargon (see
/// CLAUDE.md "Zero-crypto UX" constraint).
pub struct ChainClient {
    provider: RootProvider,
    rpc_url: String,
}

impl ChainClient {
    /// Create a new chain client connected to the given RPC endpoint.
    ///
    /// The URL is parsed and an HTTP provider is built via alloy's
    /// [`ProviderBuilder`] with a custom reqwest client configured with a
    /// 30-second timeout. No actual network call is made during construction
    /// — use [`is_connected`] to verify reachability.
    pub async fn new(rpc_url: &str) -> Result<Self> {
        debug!(rpc_url, "creating chain client");

        let url: reqwest::Url = rpc_url
            .parse()
            .with_context(|| format!("invalid network endpoint: {rpc_url}"))?;

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client for chain provider")?;

        let provider = ProviderBuilder::default().connect_reqwest(http_client, url);

        Ok(Self {
            provider,
            rpc_url: rpc_url.to_string(),
        })
    }

    /// Create a chain client from the loaded application configuration.
    ///
    /// Uses `config.network.chain_rpc` as the RPC endpoint.
    pub async fn from_config(config: &crate::config::store::Config) -> Result<Self> {
        Self::new(&config.network.chain_rpc).await
    }

    /// Get the ETH balance for an address, returned in wei.
    pub async fn get_eth_balance(&self, address: Address) -> Result<U256> {
        debug!(%address, "fetching balance");

        let balance = self
            .provider
            .get_balance(address)
            .await
            .context("unable to retrieve account balance — check your network connection")?;

        debug!(%address, %balance, "balance retrieved");
        Ok(balance)
    }

    /// Get the current block number from the network.
    pub async fn get_block_number(&self) -> Result<u64> {
        debug!("fetching current block number");

        let block_number = self
            .provider
            .get_block_number()
            .await
            .context("unable to reach the network — check your connection")?;

        debug!(block_number, "block number retrieved");
        Ok(block_number)
    }

    /// Check whether the client can reach the network.
    ///
    /// Attempts to fetch the current block number. Returns `true` on success,
    /// `false` on any error (network unreachable, invalid RPC URL, etc.).
    pub async fn is_connected(&self) -> bool {
        let connected = self.get_block_number().await.is_ok();
        debug!(rpc_url = %self.rpc_url, connected, "connectivity check");
        connected
    }

    /// Returns the RPC URL this client is connected to.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_with_valid_url() {
        let client = ChainClient::new("https://mainnet.base.org").await;
        assert!(client.is_ok());
        assert_eq!(client.unwrap().rpc_url(), "https://mainnet.base.org");
    }

    #[tokio::test]
    async fn new_with_invalid_url() {
        let result = ChainClient::new("not a url").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn from_config_uses_chain_rpc() {
        let config = crate::config::store::Config::default();
        let client = ChainClient::from_config(&config).await;
        assert!(client.is_ok());
        assert_eq!(client.unwrap().rpc_url(), "https://mainnet.base.org");
    }
}
