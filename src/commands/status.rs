use anyhow::{bail, Result};
use tracing::debug;

use crate::config;
use crate::engine::identity::{self, IdentityState};
use crate::engine::reputation;
use crate::engine::requests::{LocalRequestStatus, RequestCache};
use crate::output::formatter;

/// Run the `status` command: display agent status, earnings, and reputation.
///
/// Reads the local configuration and request cache to determine the agent's
/// current identity state (Uninitialized / Local / Registered) and displays
/// a summary including reputation score, earnings, and active request counts.
pub async fn run() -> Result<()> {
    debug!("starting status command");

    // 1. Check initialized
    if !config::store::exists()? {
        bail!("Agent not initialized. Run `agentmarket init` first.");
    }

    // 2. Load config
    let cfg = config::store::load()?;
    debug!(agent_name = %cfg.agent.name, "config loaded");

    // 3. Determine identity state
    let state = identity::get_identity_state(&cfg);
    debug!(?state, "identity state determined");

    match state {
        IdentityState::Uninitialized => {
            formatter::print_warning("Agent not initialized. Run `agentmarket init` first.");
        }
        IdentityState::Local { .. } => {
            formatter::print_info(&format!("Agent: {}", cfg.agent.name));
            formatter::print_warning(
                "Not registered on-chain. Run `agentmarket register` to join the network.",
            );
        }
        IdentityState::Registered { agent_id, .. } => {
            // Load local request cache for summary
            let all_requests = RequestCache::load_all().unwrap_or_default();
            let active = all_requests
                .iter()
                .filter(|r| {
                    matches!(
                        r.status,
                        LocalRequestStatus::Open
                            | LocalRequestStatus::Responded
                            | LocalRequestStatus::Validated
                    )
                })
                .count();
            let completed = all_requests
                .iter()
                .filter(|r| r.status == LocalRequestStatus::Claimed)
                .count();

            debug!(
                total = all_requests.len(),
                active, completed, "request cache loaded"
            );

            // Compute reputation (from local records for now).
            // In a full implementation, this would query on-chain event logs.
            let rep = reputation::compute_reputation(
                &agent_id,
                &[], // No validation records from chain yet
                0,   // earnings from chain
                0,   // avg response time
            );

            // Display status summary
            formatter::print_status(&cfg.agent.name, &agent_id, 0.0, rep.score);

            println!();
            formatter::print_info(&format!(
                "Reputation: {} ({})",
                reputation::format_reputation(&rep),
                reputation::reputation_tier(&rep)
            ));
            formatter::print_info(&format!("Active requests: {}", active));
            formatter::print_info(&format!("Completed requests: {}", completed));

            if !cfg.identity.ipfs_profile_cid.is_empty() {
                formatter::print_info(&format!(
                    "Profile: ipfs://{}",
                    cfg.identity.ipfs_profile_cid
                ));
            }
        }
    }

    debug!("status command complete");
    Ok(())
}
