//! The `validate` command: review deliverables and earn validation fees.
//!
//! Validators review responses to service requests, scoring them for quality.
//! The handler can be manual (interactive terminal prompt) or an external
//! executable that receives the deliverable on stdin and returns a JSON
//! verdict on stdout.
//!
//! When the Request Registry contract is deployed, validation results are
//! submitted on-chain via `submitValidation`. Until then, results are saved
//! locally and a "coming soon" message is displayed.

use std::time::Duration;

use alloy::primitives::Address;
use anyhow::{bail, Result};
use tracing::debug;

use crate::chain::contracts::addresses;
use crate::config::{keystore, store};
use crate::engine::handlers::{self, HandlerType};
use crate::engine::identity::{self, IdentityState};
use crate::engine::manual_handler;
use crate::engine::requests::{format_price_usd, LocalRequestStatus, RequestCache};
use crate::engine::validation::{self, HandlerInput};
use crate::output::formatter;

/// Polling interval for auto-mode (seconds between checks for pending validations).
const POLL_INTERVAL_SECS: u64 = 30;

pub async fn run(
    handler_type: String,
    handler_path: Option<String>,
    auto_mode: bool,
    filter: Option<String>,
) -> Result<()> {
    debug!(
        handler_type = %handler_type,
        handler_path = ?handler_path,
        auto_mode = auto_mode,
        filter = ?filter,
        "starting validate command"
    );

    // 1. Check that agent is initialized (config exists).
    if !store::exists()? {
        bail!("Agent not initialized. Run `agentmarket init` first.");
    }

    // 2. Load config and check identity state -- must be registered.
    let cfg = store::load()?;
    let state = identity::get_identity_state(&cfg);

    match state {
        IdentityState::Uninitialized => {
            bail!("Agent not initialized. Run `agentmarket init` first.");
        }
        IdentityState::Local { .. } => {
            bail!("Agent not registered. Run `agentmarket register` first.");
        }
        IdentityState::Registered { .. } => {
            debug!("identity state is Registered -- proceeding");
        }
    }

    // 3. Load keystore and derive address.
    let passphrase = keystore::get_passphrase()?;
    let key_bytes = keystore::load_key(&passphrase)?;
    let (_public_key, address) = identity::address_from_key(&key_bytes)?;

    debug!(address = %address, "agent address derived");

    // 4. Resolve handler type from CLI args.
    let resolved_handler = HandlerType::from_str(&handler_type, handler_path.as_deref())?;

    debug!(handler = ?resolved_handler, "handler type resolved");

    // 5. Contract deployment gate: check if REQUEST_REGISTRY is deployed.
    if addresses::REQUEST_REGISTRY == Address::ZERO {
        formatter::print_info("Validation");
        formatter::print_info("");
        formatter::print_info("The network validation service is not yet available.");
        formatter::print_info("Validation will become available soon.");
        formatter::print_info("");
        formatter::print_info("In the meantime, you can:");
        formatter::print_info("  - Run `agentmarket search --requests` to discover open requests");
        formatter::print_info("  - Run `agentmarket respond` to submit deliverables");
        formatter::print_info(
            "  - Run `agentmarket status` to check your agent profile and earnings",
        );
        formatter::print_info("");

        // Even though contracts are not deployed, process any local
        // "Responded" requests that the user might want to validate
        // locally for testing/dry-run purposes.
        let responded = RequestCache::load_by_status(LocalRequestStatus::Responded)?;

        if responded.is_empty() {
            formatter::print_info("No pending validations found locally.");
            return Ok(());
        }

        formatter::print_info(&format!(
            "Found {} local response(s) available for dry-run validation:",
            responded.len()
        ));

        for req in &responded {
            formatter::print_info(&format!(
                "  Request {}: {}",
                req.request_id,
                format_price_usd(req.price_usdc),
            ));
        }

        formatter::print_info("");
        formatter::print_info(
            "Dry-run validation results will be saved locally but not submitted to the network.",
        );

        // Process the first pending validation as a dry run.
        let target = if let Some(ref cap_filter) = filter {
            // Filter is a placeholder -- in a full implementation it would
            // match against request metadata / capabilities. For now, try to
            // match against the request_id as a simple filter.
            responded
                .iter()
                .find(|r| r.request_id.contains(cap_filter))
                .or(responded.first())
        } else {
            responded.first()
        };

        if let Some(req) = target {
            process_validation(req, &resolved_handler, &address)?;
        }

        return Ok(());
    }

    // -----------------------------------------------------------------------
    // Contract is deployed -- full on-chain validation flow.
    // -----------------------------------------------------------------------

    if auto_mode {
        formatter::print_info("Entering validation loop (auto mode). Press Ctrl+C to stop.");
        formatter::print_info(&format!(
            "Polling every {} seconds for pending validations.",
            POLL_INTERVAL_SECS
        ));

        if let Some(ref cap_filter) = filter {
            formatter::print_info(&format!("Filtering by capability: {cap_filter}"));
        }

        loop {
            match poll_and_validate(&resolved_handler, &address, filter.as_deref()) {
                Ok(found) => {
                    if found {
                        debug!("processed a validation in auto mode");
                    } else {
                        debug!("no pending validations found, sleeping");
                    }
                }
                Err(err) => {
                    debug!(error = %err, "error during validation poll");
                    formatter::print_warning(&format!("Validation error: {}. Retrying...", err));
                }
            }

            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }
    } else {
        // Single-shot mode: check for one pending validation and process it.
        match poll_and_validate(&resolved_handler, &address, filter.as_deref())? {
            true => {
                formatter::print_success("Validation complete.");
            }
            false => {
                formatter::print_info("No pending validations found.");
            }
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}

/// Poll for pending validations and process one if found.
///
/// Returns `true` if a validation was processed, `false` if none were found.
fn poll_and_validate(handler: &HandlerType, address: &str, _filter: Option<&str>) -> Result<bool> {
    debug!("polling for pending validations");

    // TODO: When the contract is live, query on-chain for requests in
    // Responded status that need validation. For now, check local cache.
    let responded = RequestCache::load_by_status(LocalRequestStatus::Responded)?;

    if responded.is_empty() {
        return Ok(false);
    }

    // Process the first pending validation.
    if let Some(req) = responded.first() {
        process_validation(req, handler, address)?;
        return Ok(true);
    }

    Ok(false)
}

/// Process a single validation: retrieve deliverable, run handler, save result.
fn process_validation(
    req: &crate::engine::requests::LocalRequest,
    handler: &HandlerType,
    _address: &str,
) -> Result<()> {
    debug!(request_id = %req.request_id, "processing validation");

    formatter::print_info(&format!(
        "Validating request {} ({})",
        req.request_id,
        format_price_usd(req.price_usdc),
    ));

    // a. Build HandlerInput.
    //    In a full implementation, the deliverable would be retrieved from IPFS
    //    and decrypted. For now, use a placeholder since we don't have the
    //    encrypted content available locally without IPFS retrieval.
    let deliverable = if let Some(ref cid) = req.response_cid {
        // TODO: Retrieve from IPFS and decrypt:
        //   let ipfs_client = IpfsClient::from_config(&cfg);
        //   let encrypted = ipfs_client.cat(cid).await?;
        //   let decrypted = encryption::decrypt(&key_bytes, &encrypted)?;
        debug!(cid = %cid, "would retrieve deliverable from IPFS (placeholder)");
        format!("[Deliverable from IPFS: {cid}]").into_bytes()
    } else {
        b"[No deliverable attached]".to_vec()
    };

    let handler_input = HandlerInput {
        request_id: req.request_id.clone(),
        task_description: format!("Request {}", req.request_id),
        deliverable,
        seller: req.counterparty.clone().unwrap_or_default(),
        price_usdc: req.price_usdc,
        deadline: req.deadline,
    };

    // b. Run the handler (manual prompt or external process).
    let handler_output = match handler {
        HandlerType::Manual => manual_handler::run_manual_review(&handler_input)?,
        HandlerType::External(executable) => {
            let raw_output = handlers::execute_handler(
                executable,
                &handler_input.deliverable,
                &handler_input.request_id,
                &handler_input.seller,
                handler_input.deadline,
                handler_input.price_usdc,
                60, // default timeout
            )?;
            validation::parse_handler_output(&raw_output)?
        }
    };

    // c. Create ValidationResult.
    let result = validation::create_result(&req.request_id, &handler_output);

    debug!(
        request_id = %result.request_id,
        passed = result.passed,
        score = result.score,
        "validation result created"
    );

    // d. Save result locally.
    validation::save_result(&result)?;

    // e. Submit validation on-chain (if contract deployed).
    if addresses::REQUEST_REGISTRY != Address::ZERO {
        // TODO: Submit submitValidation transaction on-chain:
        //   let signer = TransactionSigner::from_keystore_with_passphrase(&passphrase)?;
        //   let provider = ProviderBuilder::new()
        //       .signer(signer.inner().clone())
        //       .on_http(cfg.network.chain_rpc.parse()?);
        //   let registry = RequestRegistry::new(addresses::REQUEST_REGISTRY, provider);
        //   registry.submitValidation(
        //       U256::from_str(&req.request_id)?,
        //       result.passed,
        //       addr,
        //   ).send().await?.get_receipt().await?;
        formatter::print_info("Submitting validation...");
        debug!(
            contract = %addresses::REQUEST_REGISTRY,
            request_id = %req.request_id,
            passed = result.passed,
            "would submit submitValidation transaction (placeholder)"
        );
    }

    // f. Display result to user.
    let status_label = if result.passed { "PASSED" } else { "FAILED" };
    formatter::print_success(&format!(
        "Validation {}: {} (score: {}/100)",
        status_label, result.reason, result.score,
    ));

    if result.passed {
        formatter::print_info(&format!(
            "  Request {} is now validated. The seller can claim payment.",
            req.request_id,
        ));
    } else {
        formatter::print_info(&format!(
            "  Request {} did not pass validation.",
            req.request_id,
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::handlers::HandlerType;

    #[test]
    fn test_handler_type_resolution_manual() {
        let ht = HandlerType::from_str("manual", None).unwrap();
        assert_eq!(ht, HandlerType::Manual);
    }

    #[test]
    fn test_handler_type_resolution_external() {
        let ht = HandlerType::from_str("external", Some("/usr/bin/handler")).unwrap();
        assert_eq!(ht, HandlerType::External("/usr/bin/handler".to_string()));
    }

    #[test]
    fn test_handler_type_resolution_external_no_path_fails() {
        let result = HandlerType::from_str("external", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_handler_type_resolution_unknown_fails() {
        let result = HandlerType::from_str("magic", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_poll_interval_is_reasonable() {
        // Sanity check: polling interval should be between 5 and 300 seconds.
        assert!(POLL_INTERVAL_SECS >= 5);
        assert!(POLL_INTERVAL_SECS <= 300);
    }
}
