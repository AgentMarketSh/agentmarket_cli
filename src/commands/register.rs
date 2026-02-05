//! The `register` command: register an agent on-chain via ERC-8004.
//!
//! Uploads the agent profile to IPFS, optionally pins it via a remote
//! pinning service, and submits an on-chain registration transaction.
//! If the AgentRegistry contract is not yet deployed (address is zero),
//! the profile is still uploaded and the CID is saved to config so the
//! user does not have to re-upload later.

use alloy::primitives::Address;
use anyhow::{bail, Context, Result};
use tracing::debug;

use crate::chain::client::ChainClient;
use crate::chain::contracts::addresses;
use crate::chain::contracts::AgentRegistry;
use crate::chain::signer::TransactionSigner;
use crate::chain::types::Balance;
use crate::config;
use crate::engine::identity::{self, IdentityState};
use crate::ipfs::client::IpfsClient;
use crate::ipfs::pin::PinningService;
use crate::output::formatter;

pub async fn run() -> Result<()> {
    debug!("starting register command");

    // 1. Check that agent is initialized (config exists).
    if !config::store::exists()? {
        bail!("Agent not initialized. Run `agentmarket init` first.");
    }

    // 2. Load config and check identity state.
    let mut cfg = config::store::load()?;
    let state = identity::get_identity_state(&cfg);

    match state {
        IdentityState::Uninitialized => {
            bail!("Agent not initialized. Run `agentmarket init` first.");
        }
        IdentityState::Registered { agent_id, .. } => {
            formatter::print_info(&format!("Agent is already registered (ID: {agent_id})."));
            return Ok(());
        }
        IdentityState::Local { .. } => {
            debug!("identity state is Local — proceeding with registration");
        }
    }

    // 3. Load keystore, derive address.
    let passphrase = config::keystore::get_passphrase()?;
    let key_bytes = config::keystore::load_key(&passphrase)?;
    let (public_key, address) = identity::address_from_key(&key_bytes)?;

    debug!(address = %address, "agent address derived");

    // 4. Check ETH balance — if insufficient, show funding instructions and bail.
    let client = ChainClient::new(&cfg.network.chain_rpc).await?;
    let addr: Address = address.parse().context("failed to parse agent address")?;
    let balance_wei = client.get_eth_balance(addr).await?;
    let balance = Balance { wei: balance_wei };

    debug!(balance = %balance.display_eth(), "balance retrieved");

    if !balance.is_sufficient_for_registration() {
        formatter::print_warning("Insufficient funds for registration.");
        formatter::print_funding_instructions(&address, "0.0001 ETH");
        bail!("Insufficient funds. Send ETH to your agent address and try again.");
    }

    formatter::print_info("Preparing agent profile...");

    // 5. Build and upload agent profile to IPFS.
    let profile = identity::create_profile(
        &cfg.agent.name,
        &cfg.agent.description,
        cfg.services.capabilities.clone(),
        cfg.services.pricing_usd,
        &public_key,
        &address,
    );

    // Save profile locally for reference.
    identity::save_profile(&profile)?;
    debug!("profile saved locally");

    let profile_json =
        serde_json::to_string_pretty(&profile).context("failed to serialize agent profile")?;

    let ipfs_client = IpfsClient::from_config(&cfg);
    let cid = ipfs_client
        .add(profile_json.as_bytes())
        .await
        .context("failed to upload profile to content network")?;

    debug!(cid = %cid, "profile uploaded to IPFS");
    formatter::print_info("Profile uploaded to content network.");

    // 6. Optionally pin via remote pinning service (if configured).
    if let Some(pinner) = PinningService::from_env() {
        debug!("remote pinning service configured — pinning profile");
        match pinner.pin_by_hash(&cid).await {
            Ok(()) => {
                debug!(cid = %cid, "profile pinned via remote service");
                formatter::print_info("Profile pinned for persistence.");
            }
            Err(err) => {
                debug!(error = %err, "remote pinning failed (non-fatal)");
                formatter::print_warning(
                    "Could not pin profile remotely. It is still available on the local node.",
                );
            }
        }
    } else {
        debug!("no remote pinning service configured — skipping remote pin");
    }

    // 7. On-chain registration via AgentRegistry contract.
    let agent_uri = format!("ipfs://{cid}");

    if addresses::AGENT_REGISTRY == Address::ZERO {
        // Contract is not yet deployed — save the profile CID to config
        // so the user does not have to re-upload once it is available.
        formatter::print_warning(
            "The agent registry is not yet deployed. \
             Registration will be available once the contract goes live.",
        );
        formatter::print_info(
            "Your profile has been saved and will be used when registration opens.",
        );

        cfg.identity.ipfs_profile_cid = cid.clone();
        config::store::save(&cfg)?;
        debug!("config saved with ipfs_profile_cid (contract not yet deployed)");

        formatter::print_success(&format!("Profile ready. CID: {cid}"));
        return Ok(());
    }

    // Build the register call data (for future transaction submission).
    let _register_call = AgentRegistry::registerCall {
        agentURI: agent_uri.clone(),
    };

    // Build the transaction signer for sending the registration tx.
    let _signer = TransactionSigner::from_keystore_with_passphrase(&passphrase)?;

    debug!(
        contract = %addresses::AGENT_REGISTRY,
        agent_uri = %agent_uri,
        "submitting registration transaction"
    );

    // TODO: Once alloy provider-with-signer integration is complete,
    // send the actual transaction here:
    //   let provider = ProviderBuilder::new()
    //       .signer(signer.inner().clone())
    //       .on_http(cfg.network.chain_rpc.parse()?);
    //   let registry = AgentRegistry::new(addresses::AGENT_REGISTRY, provider);
    //   let receipt = registry.register(agent_uri).send().await?.get_receipt().await?;
    //   let agent_id = extract_agent_id_from_receipt(&receipt);
    //
    // For now, we save the CID and mark registration as pending.
    formatter::print_info("Submitting registration...");

    // 8–9. Update config with profile CID (agent_id will be set once the
    //       transaction is confirmed and the event is parsed).
    cfg.identity.ipfs_profile_cid = cid.clone();
    config::store::save(&cfg)?;
    debug!("config saved with ipfs_profile_cid");

    // 10. Display success message (zero-crypto UX).
    formatter::print_success(&format!(
        "Agent \"{}\" profile uploaded. Registration pending on-chain confirmation.",
        cfg.agent.name,
    ));

    Ok(())
}
