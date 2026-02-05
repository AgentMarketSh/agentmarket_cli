//! The `fund` command: display wallet address and check balance.
//!
//! Shows the agent's wallet address for funding and reports the current
//! ETH balance. Indicates whether the agent has enough gas to register.

use anyhow::{bail, Context, Result};
use tracing::debug;

use crate::chain::client::ChainClient;
use crate::chain::types::Balance;
use crate::config;
use crate::engine::identity;
use crate::output::formatter;

pub async fn run() -> Result<()> {
    debug!("starting fund command");

    // 1. Check that agent is initialized (config exists).
    if !config::store::exists()? {
        bail!("Agent not initialized. Run `agentmarket init` first.");
    }

    // 2. Load config and derive address from keystore.
    let cfg = config::store::load()?;
    let passphrase = config::keystore::get_passphrase()?;
    let key_bytes = config::keystore::load_key(&passphrase)?;
    let (_public_key, address) = identity::address_from_key(&key_bytes)?;

    debug!(address = %address, "agent address derived");

    // 3. Display wallet address.
    formatter::print_info("Agent wallet address:");
    formatter::print_wallet_address(&address);
    println!();

    // 4. Check balance via RPC.
    let client = ChainClient::new(&cfg.network.chain_rpc).await?;

    // Parse address for alloy.
    let addr: alloy::primitives::Address =
        address.parse().context("failed to parse agent address")?;

    let balance_wei = client.get_eth_balance(addr).await?;
    let balance = Balance { wei: balance_wei };

    // 5. Display balance and registration readiness.
    formatter::print_info(&format!("Balance: {}", balance.display_eth()));

    if balance.is_sufficient_for_registration() {
        formatter::print_success("Agent has sufficient funds for registration.");
        formatter::print_info("Run `agentmarket register` to join the network.");
    } else {
        formatter::print_warning("Insufficient funds for registration.");
        formatter::print_funding_instructions(&address, "0.0001 ETH");
    }

    Ok(())
}
