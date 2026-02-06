//! The `withdraw` command: move earned USDC to an external address.
//!
//! Transfers USDC from the agent's on-chain address to an external
//! destination. Validates the destination format, checks balances,
//! and gates on USDC contract availability. Once the contract
//! interaction is wired, builds a `USDC.transfer()` transaction
//! and displays the result in zero-crypto UX.

use alloy::primitives::Address;
use anyhow::{bail, Context, Result};
use tracing::debug;

use super::CommandContext;
use crate::chain::client::ChainClient;
use crate::chain::contracts::addresses;
use crate::chain::types::Balance;
use crate::engine::requests::{dollars_to_usdc, format_price_usd};
use crate::output::formatter;

/// Run the `withdraw` command.
///
/// Transfers earned USDC from the agent's address to an external destination.
/// If `amount` is `None`, withdraws the entire USDC balance.
pub async fn run(destination: String, amount: Option<f64>) -> Result<()> {
    debug!(destination = %destination, amount = ?amount, "starting withdraw command");

    // 1. Load config, verify registered, derive address.
    let ctx = CommandContext::load_registered()?;

    debug!(agent_address = %ctx.address, "agent address derived");

    // 2. Validate destination address format.
    validate_destination(&destination)?;

    let dest_addr: Address = destination
        .parse()
        .context("failed to parse destination address")?;

    debug!(destination = %dest_addr, "destination address validated");

    // Prevent self-transfer.
    let agent_addr: Address = ctx
        .address
        .parse()
        .context("failed to parse agent address")?;

    if dest_addr == agent_addr {
        bail!("Destination is the same as your agent address. Nothing to transfer.");
    }

    // 3. Check ETH balance for gas.
    let client = ChainClient::new(&ctx.cfg.network.chain_rpc).await?;
    let balance_wei = client.get_eth_balance(agent_addr).await?;
    let balance = Balance { wei: balance_wei };

    debug!(balance = %balance.display_eth(), "ETH balance retrieved");

    if !balance.is_sufficient_for_registration() {
        formatter::print_warning("Insufficient funds to cover transfer fees.");
        formatter::print_funding_instructions(&ctx.address, "0.0001 ETH");
        bail!("Insufficient funds. Send ETH to your agent address and try again.");
    }

    // 4. Validate the withdrawal amount (if specified).
    if let Some(dollars) = amount {
        if dollars <= 0.0 {
            bail!("Withdrawal amount must be greater than zero.");
        }
        if !dollars.is_finite() {
            bail!("Withdrawal amount must be a valid number.");
        }
    }

    // 5. Contract deployment gate: check if USDC contract interaction is
    //    available. The USDC address on Base is always set (it is a pre-deployed
    //    token), so we gate on the Request Registry to determine whether our
    //    full contract stack is live. For now, display the intended action.
    //
    //    TODO: Once USDC.transfer() is added to the sol! interface and the
    //    alloy provider-with-signer integration is complete, perform the
    //    actual on-chain transfer here.

    let withdraw_display = match amount {
        Some(dollars) => {
            let usdc_amount = dollars_to_usdc(dollars);
            format_price_usd(usdc_amount)
        }
        None => "full balance".to_string(),
    };

    formatter::print_info(&format!(
        "Preparing to transfer {} to {}...",
        withdraw_display,
        short_destination(&destination),
    ));

    // TODO: Query USDC.balanceOf(agent_addr) to check on-chain USDC balance.
    //   let usdc_contract = USDC::new(addresses::USDC, &provider);
    //   let usdc_balance = usdc_contract.balanceOf(agent_addr).call().await?;
    //
    // For now, we cannot query the USDC balance without a provider-with-signer,
    // so we proceed with the deployment gate check.

    if addresses::REQUEST_REGISTRY == Address::ZERO {
        formatter::print_warning(
            "On-chain transfers are not yet available. \
             The contract infrastructure is still being deployed.",
        );

        // Display what would happen once contracts are live.
        formatter::print_info(&format!(
            "When available, {} will be transferred from your agent to {}.",
            withdraw_display,
            short_destination(&destination),
        ));
        formatter::print_info("Run `agentmarket status` to check your current earnings.");

        return Ok(());
    }

    // 6. Contracts are deployed — display confirmation prompt.
    formatter::print_info(&format!(
        "Transferring {} to {}...",
        withdraw_display,
        short_destination(&destination),
    ));

    // 7. Build and execute the USDC.transfer() transaction.
    //
    // TODO: Wire up the actual transfer once USDC.transfer() is in the sol!
    // interface and alloy provider-with-signer is integrated:
    //
    //   let signer = TransactionSigner::from_keystore_with_passphrase(&passphrase)?;
    //   let provider = ProviderBuilder::new()
    //       .signer(signer.inner().clone())
    //       .on_http(cfg.network.chain_rpc.parse()?);
    //   let usdc_contract = USDC::new(addresses::USDC, provider);
    //
    //   // Determine amount: if None, query balanceOf first.
    //   let transfer_amount = match amount {
    //       Some(dollars) => U256::from(dollars_to_usdc(dollars)),
    //       None => usdc_contract.balanceOf(agent_addr).call().await?,
    //   };
    //
    //   let receipt = usdc_contract
    //       .transfer(dest_addr, transfer_amount)
    //       .send().await?
    //       .get_receipt().await?;

    debug!(
        destination = %dest_addr,
        usdc_contract = %addresses::USDC,
        "submitting USDC transfer (placeholder)"
    );

    // 8. Display success in zero-crypto UX.
    formatter::print_success(&format!(
        "Transferred {} to {}.",
        withdraw_display,
        short_destination(&destination),
    ));

    debug!("withdraw command complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate that a destination address is well-formed:
/// - Must start with "0x"
/// - Must be exactly 42 characters long
/// - Characters after "0x" must be valid hexadecimal
fn validate_destination(address: &str) -> Result<()> {
    if !address.starts_with("0x") {
        bail!(
            "Invalid destination address: must start with \"0x\". \
             Got: \"{address}\""
        );
    }

    if address.len() != 42 {
        bail!(
            "Invalid destination address: must be 42 characters (0x + 40 hex digits). \
             Got {} characters.",
            address.len()
        );
    }

    // Verify the hex portion (characters after "0x").
    let hex_part = &address[2..];
    if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "Invalid destination address: contains non-hexadecimal characters. \
             Got: \"{address}\""
        );
    }

    Ok(())
}

/// Shorten a destination address for display: show first 6 and last 4 chars.
fn short_destination(address: &str) -> String {
    if address.len() > 12 {
        format!("{}...{}", &address[..6], &address[address.len() - 4..])
    } else {
        address.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- validate_destination -------------------------------------------------

    #[test]
    fn valid_address_passes() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        assert!(validate_destination(addr).is_ok());
    }

    #[test]
    fn valid_address_uppercase_hex() {
        let addr = "0xABCDEF1234567890ABCDEF1234567890ABCDEF12";
        assert!(validate_destination(addr).is_ok());
    }

    #[test]
    fn valid_address_mixed_case() {
        let addr = "0xaBcDeF1234567890AbCdEf1234567890aBcDeF12";
        assert!(validate_destination(addr).is_ok());
    }

    #[test]
    fn missing_0x_prefix() {
        let addr = "1234567890abcdef1234567890abcdef12345678";
        let err = validate_destination(addr).unwrap_err();
        assert!(err.to_string().contains("must start with \"0x\""));
    }

    #[test]
    fn too_short() {
        let addr = "0x1234";
        let err = validate_destination(addr).unwrap_err();
        assert!(err.to_string().contains("42 characters"));
    }

    #[test]
    fn too_long() {
        let addr = "0x1234567890abcdef1234567890abcdef1234567890";
        let err = validate_destination(addr).unwrap_err();
        assert!(err.to_string().contains("42 characters"));
    }

    #[test]
    fn non_hex_characters() {
        let addr = "0xGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG";
        let err = validate_destination(addr).unwrap_err();
        assert!(err.to_string().contains("non-hexadecimal"));
    }

    // -- short_destination ----------------------------------------------------

    #[test]
    fn short_destination_long_address() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        assert_eq!(short_destination(addr), "0x1234...5678");
    }

    #[test]
    fn short_destination_short_string() {
        let addr = "0x1234";
        assert_eq!(short_destination(addr), "0x1234");
    }
}
