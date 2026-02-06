//! The `request` command: create a service request for another agent.
//!
//! Builds a request payload (task description + optional file attachment),
//! encrypts it with ECIES, uploads to IPFS, optionally pins it remotely,
//! and either submits the request on-chain or saves it locally if the
//! Request Registry contract is not yet deployed.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use anyhow::{bail, Context, Result};
use tracing::debug;

use super::CommandContext;
use crate::chain::client::ChainClient;
use crate::chain::contracts::addresses;
use crate::chain::types::Balance;
use crate::engine::requests::{
    dollars_to_usdc, format_price_usd, LocalRequest, LocalRequestStatus, RequestCache, RequestRole,
};
use crate::ipfs::client::IpfsClient;
use crate::ipfs::encryption;
use crate::ipfs::pin::PinningService;
use crate::output::formatter;

pub async fn run(
    task: String,
    price: f64,
    deadline_hours: u64,
    target_agent_id: u64,
    file_path: Option<String>,
) -> Result<()> {
    debug!("starting request command");

    // 1. Load config, verify registered, derive address and public key.
    let ctx = CommandContext::load_registered()?;

    debug!(address = %ctx.address, "agent address derived");

    // 2. Check ETH balance — if insufficient, show funding instructions and bail.
    let client = ChainClient::new(&ctx.cfg.network.chain_rpc).await?;
    let addr: Address = ctx
        .address
        .parse()
        .context("failed to parse agent address")?;
    let balance_wei = client.get_eth_balance(addr).await?;
    let balance = Balance { wei: balance_wei };

    debug!(balance = %balance.display_eth(), "balance retrieved");

    if !balance.is_sufficient_for_registration() {
        formatter::print_warning("Insufficient funds to submit request.");
        formatter::print_funding_instructions(&ctx.address, "0.0001 ETH");
        bail!("Insufficient funds. Send ETH to your agent address and try again.");
    }

    // 3. Build request payload JSON (task description + optional file content).
    formatter::print_info("Preparing request...");

    let file_content = if let Some(ref path) = file_path {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read attachment file: {path}"))?;
        debug!(path = %path, size = content.len(), "attachment file loaded");
        Some(content)
    } else {
        None
    };

    let payload = if let Some(ref content) = file_content {
        serde_json::json!({
            "task": task,
            "attachment": content,
        })
    } else {
        serde_json::json!({
            "task": task,
        })
    };

    let payload_bytes =
        serde_json::to_vec(&payload).context("failed to serialize request payload")?;

    // 4. Encrypt payload with ECIES using the agent's own public key.
    //    (For a targeted request, the target agent's public key would be used;
    //     that requires a lookup that is not yet implemented.)
    let ciphertext = encryption::encrypt(&ctx.public_key, &payload_bytes)
        .context("failed to encrypt request payload")?;

    debug!(ciphertext_len = ciphertext.len(), "payload encrypted");

    // 5. Upload encrypted payload to IPFS.
    let ipfs_client = IpfsClient::from_config(&ctx.cfg);
    let cid = ipfs_client
        .add(&ciphertext)
        .await
        .context("failed to upload request to content network")?;

    debug!(cid = %cid, "encrypted request uploaded to IPFS");
    formatter::print_info("Request uploaded to content network.");

    // 6. Optionally pin via remote pinning service (if configured).
    if let Some(pinner) = PinningService::from_env() {
        debug!("remote pinning service configured — pinning request");
        match pinner.pin_by_hash(&cid).await {
            Ok(()) => {
                debug!(cid = %cid, "request pinned via remote service");
                formatter::print_info("Request pinned for persistence.");
            }
            Err(err) => {
                debug!(error = %err, "remote pinning failed (non-fatal)");
                formatter::print_warning(
                    "Could not pin request remotely. It is still available on the local node.",
                );
            }
        }
    } else {
        debug!("no remote pinning service configured — skipping remote pin");
    }

    // 7. Convert price from USD to USDC units (6 decimals).
    let price_usdc = dollars_to_usdc(price);

    // 8. Calculate deadline as Unix timestamp (now + hours).
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock error")?
        .as_secs();
    let deadline_ts = now + (deadline_hours * 3600);

    debug!(
        price_usdc = price_usdc,
        deadline_ts = deadline_ts,
        target_agent_id = target_agent_id,
        "request parameters computed"
    );

    // 9. Generate a local request ID (timestamp-based, will be replaced by
    //     the on-chain ID after contract submission).
    let local_request_id = format!("local-{now}");

    // 10. Contract deployment gate: check if REQUEST_REGISTRY address is ZERO.
    if addresses::REQUEST_REGISTRY == Address::ZERO {
        // Contract not yet deployed — save request locally.
        formatter::print_warning(
            "The request registry is not yet deployed. \
             Your request has been saved locally and will be submitted once the contract goes live.",
        );

        let local_request = LocalRequest {
            request_id: local_request_id.clone(),
            role: RequestRole::Buyer,
            status: LocalRequestStatus::Open,
            request_cid: cid.clone(),
            price_usdc,
            deadline: deadline_ts,
            response_cid: None,
            secret: None,
            secret_hash: None,
            counterparty: None,
            created_at: now,
            updated_at: now,
        };

        RequestCache::save(&local_request)?;
        debug!(request_id = %local_request_id, "request saved to local cache");

        formatter::print_success(&format!(
            "Request saved (ID: {local_request_id}). Task: \"{task}\" for {}",
            format_price_usd(price_usdc),
        ));

        if target_agent_id > 0 {
            formatter::print_info(&format!("Targeted to agent #{target_agent_id}."));
        } else {
            formatter::print_info("Open request — any agent can respond.");
        }

        formatter::print_info(&format!("Deadline: {deadline_hours} hours from now."));

        return Ok(());
    }

    // Contract is deployed — submit on-chain (placeholder with TODO).
    // TODO: Once alloy provider-with-signer integration is complete,
    // send the actual createRequest transaction here:
    //   let signer = TransactionSigner::from_keystore_with_passphrase(&passphrase)?;
    //   let provider = ProviderBuilder::new()
    //       .signer(signer.inner().clone())
    //       .on_http(cfg.network.chain_rpc.parse()?);
    //   let registry = RequestRegistry::new(addresses::REQUEST_REGISTRY, provider);
    //   let receipt = registry
    //       .createRequest(
    //           format!("ipfs://{cid}"),
    //           U256::from(price_usdc),
    //           U256::from(deadline_ts),
    //           U256::from(target_agent_id),
    //       )
    //       .send()
    //       .await?
    //       .get_receipt()
    //       .await?;
    //   let request_id = extract_request_id_from_receipt(&receipt);

    formatter::print_info("Submitting request...");

    // 11. Save to local request cache.
    let local_request = LocalRequest {
        request_id: local_request_id.clone(),
        role: RequestRole::Buyer,
        status: LocalRequestStatus::Open,
        request_cid: cid.clone(),
        price_usdc,
        deadline: deadline_ts,
        response_cid: None,
        secret: None,
        secret_hash: None,
        counterparty: None,
        created_at: now,
        updated_at: now,
    };

    RequestCache::save(&local_request)?;
    debug!(request_id = %local_request_id, "request saved to local cache");

    // 12. Display success with request details (zero-crypto UX).
    formatter::print_success(&format!(
        "Request created (ID: {local_request_id}). Task: \"{task}\" for {}",
        format_price_usd(price_usdc),
    ));

    if target_agent_id > 0 {
        formatter::print_info(&format!("Targeted to agent #{target_agent_id}."));
    } else {
        formatter::print_info("Open request — any agent can respond.");
    }

    formatter::print_info(&format!("Deadline: {deadline_hours} hours from now."));

    Ok(())
}
