//! Daemon command — runs validate + auto-claim in a continuous loop.
//!
//! Polls the local request cache at a configurable interval, looking for:
//! - **Pending validations:** requests in `Responded` status where this agent
//!   is the `Validator`.
//! - **Claimable requests:** requests in `Validated` status where this agent
//!   is the `Seller`.
//!
//! The daemon handles graceful shutdown via `Ctrl+C` (tokio `ctrl_c`).

use alloy::primitives::Address;
use anyhow::Result;
use tokio::signal;
use tokio::time::{sleep, Duration};
use tracing::debug;

use crate::chain::contracts::addresses;
use crate::config::store;
use crate::engine::identity::{get_identity_state, IdentityState};
use crate::engine::requests::{LocalRequestStatus, RequestCache, RequestRole};
use crate::output::formatter;

pub async fn run(
    interval_secs: u64,
    handler_type: String,
    handler_path: Option<String>,
) -> Result<()> {
    // 1. Check initialized and registered
    if !store::exists()? {
        anyhow::bail!("Agent not initialized. Run `agentmarket init` first.");
    }

    let cfg = store::load()?;
    let state = get_identity_state(&cfg);

    match state {
        IdentityState::Uninitialized => {
            anyhow::bail!("Agent not initialized. Run `agentmarket init` first.");
        }
        IdentityState::Local { .. } => {
            anyhow::bail!("Agent not registered. Run `agentmarket register` first.");
        }
        IdentityState::Registered { .. } => {
            // Good to go.
        }
    }

    // 2. Print startup banner
    formatter::print_success("Daemon started");
    formatter::print_info(&format!("Poll interval: {}s", interval_secs));
    formatter::print_info(&format!("Handler: {}", handler_type));
    if let Some(ref path) = handler_path {
        formatter::print_info(&format!("Handler path: {}", path));
    }
    formatter::print_info("Press Ctrl+C to stop.");
    println!();

    // 3. Main loop
    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                formatter::print_info("Shutting down gracefully...");
                break;
            }
            _ = daemon_tick(&cfg, &handler_type, handler_path.as_deref()) => {
                // tick completed, sleep before next
            }
        }

        tokio::select! {
            _ = signal::ctrl_c() => {
                formatter::print_info("Shutting down gracefully...");
                break;
            }
            _ = sleep(Duration::from_secs(interval_secs)) => {}
        }
    }

    formatter::print_success("Daemon stopped.");
    Ok(())
}

async fn daemon_tick(
    _cfg: &store::Config,
    _handler_type: &str,
    _handler_path: Option<&str>,
) -> Result<()> {
    debug!("starting daemon tick");

    // Check for pending validations and claimable requests
    let all_requests = RequestCache::load_all().unwrap_or_default();

    // Count work items
    let pending_validations = all_requests
        .iter()
        .filter(|r| r.status == LocalRequestStatus::Responded && r.role == RequestRole::Validator)
        .count();
    let claimable = all_requests
        .iter()
        .filter(|r| r.status == LocalRequestStatus::Validated && r.role == RequestRole::Seller)
        .count();

    if pending_validations > 0 || claimable > 0 {
        formatter::print_info(&format!(
            "Found {} pending validation(s), {} claimable request(s)",
            pending_validations, claimable
        ));
    }

    // Contract deployment gate
    if addresses::REQUEST_REGISTRY == Address::ZERO {
        if pending_validations > 0 || claimable > 0 {
            formatter::print_warning(
                "Network services not yet available. Validation and claims will be processed once ready.",
            );
        }
        return Ok(());
    }

    // TODO: Process validations and claims when contract is deployed

    Ok(())
}
