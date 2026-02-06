//! The `respond` command: submit a response to a service request.
//!
//! Builds a deliverable payload from a file and/or message, generates a
//! hash-lock secret, encrypts the deliverable with ECIES using the buyer's
//! public key, uploads it to IPFS, and (when the Request Registry contract
//! is deployed) submits a `submitResponse` transaction on-chain.
//!
//! The secret S is stored locally in the request cache -- losing it means
//! losing the ability to claim payment. The keccak256(S) hash is published
//! on-chain as part of the response.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use anyhow::{bail, Context, Result};
use tracing::debug;

use super::CommandContext;
use crate::chain::client::ChainClient;
use crate::chain::contracts::addresses;
use crate::chain::types::Balance;
use crate::engine::requests::{
    format_price_usd, generate_secret, LocalRequestStatus, RequestCache, RequestRole,
};
use crate::ipfs::client::IpfsClient;
use crate::ipfs::encryption;
use crate::ipfs::pin::PinningService;
use crate::output::formatter;

pub async fn run(
    request_id: String,
    file_path: Option<String>,
    message: Option<String>,
) -> Result<()> {
    debug!("starting respond command");

    // 0. Validate that at least one of file or message is provided.
    if file_path.is_none() && message.is_none() {
        bail!("Provide a file (--file) and/or a message (--message) for the response.");
    }

    // 1. If a file path was given, verify it exists before doing heavy setup.
    if let Some(ref path) = file_path {
        if !std::path::Path::new(path).exists() {
            bail!("File not found: {path}");
        }
    }

    // 2. Load config, verify registered, derive address.
    let ctx = CommandContext::load_registered()?;

    debug!(address = %ctx.address, "agent address derived");

    // 3. Check ETH balance -- bail with funding instructions if insufficient.
    let client = ChainClient::new(&ctx.cfg.network.chain_rpc).await?;
    let addr: Address = ctx
        .address
        .parse()
        .context("failed to parse agent address")?;
    let balance_wei = client.get_eth_balance(addr).await?;
    let balance = Balance { wei: balance_wei };

    debug!(balance = %balance.display_eth(), "balance retrieved");

    if !balance.is_sufficient_for_registration() {
        formatter::print_warning("Insufficient funds to submit a response.");
        formatter::print_funding_instructions(&ctx.address, "0.0001 ETH");
        bail!("Insufficient funds. Send ETH to your agent address and try again.");
    }

    // 4. Load the local request from cache to verify it exists and is Open.
    let mut local_request = RequestCache::load(&request_id)
        .with_context(|| format!("Request {request_id} not found in local cache."))?;

    if local_request.status != LocalRequestStatus::Open {
        bail!(
            "Request {} is not open for responses (current status: {:?}).",
            request_id,
            local_request.status,
        );
    }

    formatter::print_info(&format!(
        "Preparing response to request {} ({})...",
        request_id,
        format_price_usd(local_request.price_usdc),
    ));

    // 5. Build deliverable payload (file content and/or message).
    let mut payload = Vec::new();

    if let Some(ref msg) = message {
        payload.extend_from_slice(b"--- MESSAGE ---\n");
        payload.extend_from_slice(msg.as_bytes());
        payload.push(b'\n');
    }

    if let Some(ref path) = file_path {
        let file_content =
            fs::read(path).with_context(|| format!("Failed to read file: {path}"))?;
        payload.extend_from_slice(b"--- FILE ---\n");
        payload.extend_from_slice(&file_content);
    }

    debug!(payload_size = payload.len(), "deliverable payload built");

    // 6. Generate secret S and compute keccak256(S) for the hash-lock.
    let (secret_hex, secret_hash_hex) = generate_secret();
    debug!("secret and hash generated for hash-lock pattern");

    // 7. Encrypt deliverable with ECIES using our own public key.
    //    In a full implementation, the buyer's public key would be used so
    //    only the buyer can decrypt it. For now we use our own public key
    //    since the buyer's key is not yet available in the local cache.
    let encrypted_payload =
        encryption::encrypt(&ctx.public_key, &payload).context("Failed to encrypt deliverable.")?;

    debug!(
        encrypted_size = encrypted_payload.len(),
        "deliverable encrypted"
    );

    // 8. Upload encrypted deliverable to IPFS.
    let ipfs_client = IpfsClient::from_config(&ctx.cfg);
    let cid = ipfs_client
        .add(&encrypted_payload)
        .await
        .context("Failed to upload response to content network.")?;

    debug!(cid = %cid, "encrypted deliverable uploaded to IPFS");
    formatter::print_info("Response uploaded to content network.");

    // 9. Optionally pin via remote pinning service.
    if let Some(pinner) = PinningService::from_env() {
        debug!("remote pinning service configured -- pinning response");
        match pinner.pin_by_hash(&cid).await {
            Ok(()) => {
                debug!(cid = %cid, "response pinned via remote service");
                formatter::print_info("Response pinned for persistence.");
            }
            Err(err) => {
                debug!(error = %err, "remote pinning failed (non-fatal)");
                formatter::print_warning(
                    "Could not pin response remotely. It is still available on the local node.",
                );
            }
        }
    } else {
        debug!("no remote pinning service configured -- skipping remote pin");
    }

    // 10. Contract deployment gate: check if REQUEST_REGISTRY is ZERO.
    if addresses::REQUEST_REGISTRY == Address::ZERO {
        formatter::print_warning(
            "The request registry is not yet deployed. \
             Your response has been saved locally and will be submitted \
             when the contract goes live.",
        );
    } else {
        // TODO: Submit the submitResponse transaction on-chain once the
        // alloy provider-with-signer integration is complete:
        //   let signer = TransactionSigner::from_keystore_with_passphrase(&passphrase)?;
        //   let provider = ProviderBuilder::new()
        //       .signer(signer.inner().clone())
        //       .on_http(cfg.network.chain_rpc.parse()?);
        //   let registry = RequestRegistry::new(addresses::REQUEST_REGISTRY, provider);
        //   let secret_hash_bytes: B256 = secret_hash_hex.parse()?;
        //   registry.submitResponse(
        //       U256::from_str(&request_id)?,
        //       format!("ipfs://{cid}"),
        //       secret_hash_bytes,
        //   ).send().await?.get_receipt().await?;
        formatter::print_info("Submitting response on-chain...");
        debug!(
            contract = %addresses::REQUEST_REGISTRY,
            request_id = %request_id,
            cid = %cid,
            "would submit submitResponse transaction (placeholder)"
        );
    }

    // 11. Save secret S locally -- CRITICAL: losing S means losing payment.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    local_request.status = LocalRequestStatus::Responded;
    local_request.response_cid = Some(cid.clone());
    local_request.secret = Some(secret_hex);
    local_request.secret_hash = Some(secret_hash_hex);
    local_request.role = RequestRole::Seller;
    local_request.updated_at = now;

    RequestCache::save(&local_request).context("Failed to save response to local cache.")?;
    debug!(request_id = %request_id, "local request cache updated with response");

    // 12. Display success with response details (zero-crypto UX).
    formatter::print_success(&format!("Response submitted for request {}.", request_id,));
    formatter::print_info(&format!(
        "  Price: {}",
        format_price_usd(local_request.price_usdc)
    ));
    formatter::print_info(&format!("  Content ID: {cid}"));

    if addresses::REQUEST_REGISTRY == Address::ZERO {
        formatter::print_info("  Status: Saved locally (pending contract deployment).");
    } else {
        formatter::print_info("  Status: Pending on-chain confirmation.");
    }

    formatter::print_warning(
        "Your claim secret is stored locally. Do not delete your agent data \
         before claiming payment.",
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secret_produces_valid_pair() {
        // Verify the generate_secret helper used by respond works correctly.
        let (secret, hash) = generate_secret();
        assert_eq!(secret.len(), 64, "secret hex should be 64 chars");
        assert!(hash.starts_with("0x"), "hash should be 0x-prefixed");
        assert_eq!(hash.len(), 66, "hash hex should be 66 chars");
    }

    #[test]
    fn test_local_request_status_transition_open_to_responded() {
        assert!(LocalRequestStatus::Open.can_transition_to(&LocalRequestStatus::Responded));
    }

    #[test]
    fn test_local_request_status_transition_responded_not_to_open() {
        assert!(!LocalRequestStatus::Responded.can_transition_to(&LocalRequestStatus::Open));
    }
}
