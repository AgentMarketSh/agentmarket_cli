//! The `fund` command: display wallet address and check balance.
//!
//! Shows the agent's wallet address for funding and reports the current
//! ETH balance. Indicates whether the agent has enough gas to register.

use anyhow::{Context, Result};
use tracing::debug;

use super::CommandContext;
use crate::chain::client::ChainClient;
use crate::chain::types::Balance;
use crate::output::formatter;

pub async fn run() -> Result<()> {
    debug!("starting fund command");

    // 1. Load config and derive address from keystore.
    let ctx = CommandContext::load_initialized()?;

    debug!(address = %ctx.address, "agent address derived");

    // 2. Display wallet address.
    formatter::print_info("Agent wallet address:");
    formatter::print_wallet_address(&ctx.address);
    println!();

    // 3. Check balance via RPC.
    let client = ChainClient::new(&ctx.cfg.network.chain_rpc).await?;

    // Parse address for alloy.
    let addr: alloy::primitives::Address = ctx
        .address
        .parse()
        .context("failed to parse agent address")?;

    let balance_wei = client.get_eth_balance(addr).await?;
    let balance = Balance { wei: balance_wei };

    // 4. Display balance and registration readiness.
    formatter::print_info(&format!("Balance: {}", balance.display_eth()));

    if balance.is_sufficient_for_registration() {
        formatter::print_success("Agent has sufficient funds for registration.");
        formatter::print_info("Run `agentmarket register` to join the network.");
    } else {
        formatter::print_warning("Insufficient funds for registration.");
        formatter::print_funding_instructions(&ctx.address, "0.0001 ETH");
    }

    Ok(())
}
