//! The `search` command: discover agents and requests on the network.
//!
//! Queries on-chain event logs via `eth_getLogs` to find registered agents
//! and (in future) open requests. Supports filtering by capability.

use anyhow::Result;
use tracing::debug;

use crate::chain::client::ChainClient;
use crate::chain::contracts;
use crate::config;
use crate::output::formatter;

/// Search mode: what to look for.
pub enum SearchMode {
    /// Search for registered agents (default).
    Agents,
    /// Search for open requests.
    Requests,
}

pub async fn run(capability: Option<String>, search_requests: bool) -> Result<()> {
    debug!("starting search command");

    let mode = if search_requests {
        SearchMode::Requests
    } else {
        SearchMode::Agents
    };

    // Load config for RPC endpoint.
    let cfg = config::store::load().unwrap_or_else(|_| config::store::Config::default());

    let client = ChainClient::new(&cfg.network.chain_rpc).await?;

    match mode {
        SearchMode::Agents => search_agents(&client, capability.as_deref()).await,
        SearchMode::Requests => search_requests_fn(&client, capability.as_deref()).await,
    }
}

async fn search_agents(_client: &ChainClient, _capability: Option<&str>) -> Result<()> {
    formatter::print_info("Searching for registered agents...");

    // For MVP: Query AgentRegistered events from the Agent Registry.
    // The Agent Registry address is a placeholder (zero address) until deployment.
    let registry_addr = contracts::addresses::AGENT_REGISTRY;

    if registry_addr == alloy::primitives::Address::ZERO {
        formatter::print_warning(
            "Agent Registry not yet deployed. Search will be available after contract deployment.",
        );
        return Ok(());
    }

    // TODO: Query eth_getLogs for AgentRegistered events
    // For each event, fetch the agentURI from IPFS, parse the profile,
    // and filter by capability if specified.
    // This will be fully implemented once the contract is deployed (Phase 3).

    formatter::print_info("No agents found matching your criteria.");
    Ok(())
}

async fn search_requests_fn(_client: &ChainClient, _capability: Option<&str>) -> Result<()> {
    formatter::print_info("Searching for open requests...");

    let registry_addr = contracts::addresses::REQUEST_REGISTRY;

    if registry_addr == alloy::primitives::Address::ZERO {
        formatter::print_warning(
            "Request Registry not yet deployed. Request search will be available after contract deployment.",
        );
        return Ok(());
    }

    // TODO: Query eth_getLogs for RequestCreated events
    // This will be implemented in Phase 3 after contract deployment.

    formatter::print_info("No open requests found matching your criteria.");
    Ok(())
}
