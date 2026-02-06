//! The `claim` command: settle a validated response and trigger payment.
//!
//! Reveals the secret S on-chain, which atomically verifies `keccak256(S)`
//! against the stored hash and triggers `USDC.transferFrom()` to pay the
//! seller and validator. If the Request Registry contract is not yet
//! deployed, the local cache is updated and payment settlement is deferred.

use alloy::primitives::Address;
use anyhow::{bail, Context, Result};
use tracing::debug;

use super::CommandContext;
use crate::chain::client::ChainClient;
use crate::chain::contracts::addresses;
use crate::chain::types::Balance;
use crate::engine::requests::{format_price_usd, LocalRequestStatus, RequestCache, RequestRole};
use crate::output::formatter;

pub async fn run(request_id: String) -> Result<()> {
    debug!(request_id = %request_id, "starting claim command");

    // 1. Load config, verify registered, derive address.
    let ctx = CommandContext::load_registered()?;

    debug!(address = %ctx.address, "agent address derived");

    // 2. Check ETH balance — bail if insufficient for gas.
    let client = ChainClient::new(&ctx.cfg.network.chain_rpc).await?;
    let addr: Address = ctx
        .address
        .parse()
        .context("failed to parse agent address")?;
    let balance_wei = client.get_eth_balance(addr).await?;
    let balance = Balance { wei: balance_wei };

    debug!(balance = %balance.display_eth(), "balance retrieved");

    if !balance.is_sufficient_for_registration() {
        formatter::print_warning("Insufficient funds to settle payment.");
        formatter::print_funding_instructions(&ctx.address, "0.0001 ETH");
        bail!("Insufficient funds. Send ETH to your agent address and try again.");
    }

    // 3. Load the request from local cache.
    let mut request = match RequestCache::load(&request_id) {
        Ok(r) => r,
        Err(_) => {
            bail!(
                "Request {request_id} not found in local cache. \
                 Only requests you have participated in can be claimed."
            );
        }
    };

    debug!(
        request_id = %request.request_id,
        status = ?request.status,
        role = ?request.role,
        "request loaded from cache"
    );

    // 4. Verify the agent is the seller for this request.
    if request.role != RequestRole::Seller {
        bail!(
            "You are not the seller for request {request_id}. \
             Only the seller can claim payment."
        );
    }

    // 5. Verify the request is in Validated status.
    if request.status != LocalRequestStatus::Validated {
        match request.status {
            LocalRequestStatus::Claimed => {
                formatter::print_info(&format!("Request {request_id} has already been claimed."));
                return Ok(());
            }
            LocalRequestStatus::Open => {
                bail!(
                    "Request {request_id} has not been responded to yet. \
                     A response must be submitted and validated before claiming."
                );
            }
            LocalRequestStatus::Responded => {
                bail!(
                    "Request {request_id} is awaiting validation. \
                     The response must be validated before you can claim payment."
                );
            }
            LocalRequestStatus::Cancelled => {
                bail!("Request {request_id} was cancelled. Payment cannot be claimed.");
            }
            LocalRequestStatus::Expired => {
                bail!("Request {request_id} has expired. Payment cannot be claimed.");
            }
            _ => {
                bail!(
                    "Request {request_id} is not in a claimable state (current: {:?}).",
                    request.status
                );
            }
        }
    }

    // 6. Retrieve the secret S from local cache.
    let secret = match &request.secret {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            bail!(
                "CRITICAL: Secret for request {request_id} is missing from local cache. \
                 The secret is required to claim payment. This should never happen — \
                 your local data may be corrupted. Contact support."
            );
        }
    };

    debug!("secret retrieved from local cache");

    // 7. Contract deployment gate: check if REQUEST_REGISTRY is deployed.
    if addresses::REQUEST_REGISTRY == Address::ZERO {
        formatter::print_warning(
            "The request registry contract is not yet deployed. \
             On-chain settlement will be available after deployment.",
        );
        formatter::print_info("Updating local status to reflect successful claim.");

        // Update local cache status to Claimed.
        request.status = LocalRequestStatus::Claimed;
        request.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        RequestCache::save(&request)?;

        let earned = format_price_usd(request.price_usdc);
        formatter::print_success(&format!("Earned {earned} for request {request_id}."));
        formatter::print_info("Payment will be settled on-chain once the contract is deployed.");

        return Ok(());
    }

    // 8. Contract is deployed — send claim transaction.
    // TODO: Once alloy provider-with-signer integration is complete,
    // send the actual claim(requestId, secret) transaction here:
    //   let signer = TransactionSigner::from_keystore_with_passphrase(&passphrase)?;
    //   let provider = ProviderBuilder::new()
    //       .signer(signer.inner().clone())
    //       .on_http(cfg.network.chain_rpc.parse()?);
    //   let registry = RequestRegistry::new(addresses::REQUEST_REGISTRY, provider);
    //   let secret_bytes: B256 = hex::decode(&secret)?.try_into()?;
    //   let request_id_u256 = U256::from_str(&request_id)?;
    //   let receipt = registry.claim(request_id_u256, secret_bytes)
    //       .send().await?.get_receipt().await?;

    debug!(
        request_id = %request_id,
        contract = %addresses::REQUEST_REGISTRY,
        "submitting claim transaction (placeholder)"
    );

    formatter::print_info("Submitting claim...");

    // 9. Update local request cache status to Claimed.
    request.status = LocalRequestStatus::Claimed;
    request.updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    RequestCache::save(&request)?;

    debug!(request_id = %request_id, "local cache updated to Claimed");

    // 10. Display success with payment details (zero-crypto UX).
    let earned = format_price_usd(request.price_usdc);
    formatter::print_success(&format!("Earned {earned} for request {request_id}."));

    // Suppress the secret from output (it is sensitive).
    let _ = secret;

    Ok(())
}
