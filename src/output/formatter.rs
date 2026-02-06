//! User-facing output formatter.
//!
//! Enforces the "zero-crypto UX" principle: no blockchain terminology
//! (wallets, gas, transactions, blocks, chains) ever reaches the user.
//! All user-facing messages are routed through the helpers in this module.
//!
//! The only exception is [`print_wallet_address`] and [`print_funding_instructions`],
//! which are used exclusively by `init` and `fund` commands where the raw
//! address must be shown so the user can send funds.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Error;

// ---------------------------------------------------------------------------
// JSON mode
// ---------------------------------------------------------------------------

static JSON_MODE: AtomicBool = AtomicBool::new(false);

/// Enable or disable JSON output mode globally.
///
/// When enabled, functions like [`print_error`] emit machine-readable JSON
/// instead of human-friendly text.
pub fn set_json_mode(enabled: bool) {
    JSON_MODE.store(enabled, Ordering::Relaxed);
}

/// Returns `true` if JSON output mode is currently active.
pub fn is_json_mode() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Success / info / warning primitives
// ---------------------------------------------------------------------------

/// Print a success message to stdout: "✓ {msg}"
pub fn print_success(msg: &str) {
    println!("\u{2713} {msg}");
}

/// Print an informational message to stdout.
pub fn print_info(msg: &str) {
    println!("{msg}");
}

/// Print a warning to stderr: "⚠ {msg}"
pub fn print_warning(msg: &str) {
    eprintln!("\u{26A0} {msg}");
}

// ---------------------------------------------------------------------------
// Error formatting
// ---------------------------------------------------------------------------

/// Translate an internal error into a human-readable message that contains
/// no blockchain or IPFS jargon.
///
/// Pattern-matching is intentionally ordered so that the most specific
/// patterns are checked first.
pub fn format_error(err: &Error) -> String {
    let msg = err.to_string();
    let lower = msg.to_lowercase();

    if lower.contains("insufficient funds") {
        "Insufficient funds. Run `agentmarket fund` to check your balance.".to_string()
    } else if lower.contains("already registered") {
        "Agent is already registered on the network.".to_string()
    } else if lower.contains("nonce") {
        "Transaction conflict. Please try again.".to_string()
    } else if lower.contains("timeout") || lower.contains("connection") {
        "Network unreachable. Check your internet connection.".to_string()
    } else if lower.contains("ipfs") {
        "Content network unavailable. Please try again later.".to_string()
    } else if lower.contains("keystore") || lower.contains("decrypt") {
        "Invalid passphrase. Please try again.".to_string()
    } else if lower.contains("not found") {
        "Request not found. Check the ID and try again.".to_string()
    } else if lower.contains("expired") {
        "Request has expired and can no longer be processed.".to_string()
    } else if lower.contains("secret") {
        "Secret key missing. Your local data may be corrupted.".to_string()
    } else if lower.contains("cancelled") {
        "Request was cancelled.".to_string()
    } else if lower.contains("validation") {
        "Validation failed. Check the handler output.".to_string()
    } else if lower.contains("permission") || lower.contains("unauthorized") {
        "Permission denied. Check your identity and try again.".to_string()
    } else if lower.contains("parse") {
        "Invalid input format. Please check your command arguments.".to_string()
    } else {
        format!("Operation failed: {msg}")
    }
}

/// Format and print an error to stderr.
///
/// In JSON mode, emits `{"error": "..."}` instead of plain text.
pub fn print_error(err: &Error) {
    if is_json_mode() {
        let message = format_error(err);
        // Escape any double-quotes or backslashes in the message for valid JSON.
        let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");
        eprintln!("{{\"error\": \"{escaped}\"}}");
    } else {
        eprintln!("{}", format_error(err));
    }
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Display an earnings amount as USD, always with two decimal places.
///
/// Example output: `$1,234.56` (no thousands separator — keeps parsing simple
/// for agent consumers; just `$1234.56`).
pub fn print_earnings(amount_usd: f64) {
    println!("${:.2}", amount_usd);
}

/// Shorten an agent ID for display purposes.
///
/// If the ID is longer than 12 characters the short form is the first 8
/// characters followed by "...". Otherwise the full ID is returned.
fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}...", &id[..8])
    } else {
        id.to_string()
    }
}

/// Print a table of agents (name, description).
pub fn print_agent_list(agents: &[(String, String)]) {
    if agents.is_empty() {
        println!("No agents found.");
        return;
    }

    // Determine column width for the name column.
    let name_width = agents
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(4)
        .max(4); // minimum width = "Name"

    println!("{:<width$}  Description", "Name", width = name_width);
    println!("{:<width$}  -----------", "----", width = name_width);

    for (name, description) in agents {
        println!("{:<width$}  {description}", name, width = name_width);
    }
}

/// Print a table of requests (id, description, price in USD).
pub fn print_request_list(requests: &[(String, String, f64)]) {
    if requests.is_empty() {
        println!("No requests found.");
        return;
    }

    // Compute column widths.
    let id_width = requests
        .iter()
        .map(|(id, _, _)| short_id(id).len())
        .max()
        .unwrap_or(2)
        .max(2); // minimum width = "ID"

    let desc_width = requests
        .iter()
        .map(|(_, desc, _)| desc.len())
        .max()
        .unwrap_or(11)
        .max(11); // minimum width = "Description"

    println!(
        "{:<id_w$}  {:<desc_w$}  Price",
        "ID",
        "Description",
        id_w = id_width,
        desc_w = desc_width,
    );
    println!(
        "{:<id_w$}  {:<desc_w$}  -----",
        "--",
        "-----------",
        id_w = id_width,
        desc_w = desc_width,
    );

    for (id, description, price_usd) in requests {
        println!(
            "{:<id_w$}  {:<desc_w$}  ${:.2}",
            short_id(id),
            description,
            price_usd,
            id_w = id_width,
            desc_w = desc_width,
        );
    }
}

/// Print a formatted agent status summary.
///
/// Example output:
/// ```text
/// Agent:      alice
/// ID:         0x1a2b3c...
/// Earnings:   $42.50
/// Reputation: 97.3
/// ```
pub fn print_status(name: &str, agent_id: &str, earnings: f64, reputation: f64) {
    println!("Agent:      {name}");
    println!("ID:         {}", short_id(agent_id));
    println!("Earnings:   ${:.2}", earnings);
    println!("Reputation: {:.1}", reputation);
}

/// Print a raw wallet address.
///
/// **This is the one place where a crypto-specific detail is allowed in
/// user-facing output**, used only by the `init` and `fund` commands.
pub fn print_wallet_address(address: &str) {
    println!("Address: {address}");
}

/// Print funding instructions including the wallet address and the amount
/// of funds required.
///
/// Like [`print_wallet_address`], this is one of the few places where raw
/// crypto details are intentionally exposed to the user.
pub fn print_funding_instructions(address: &str, needed: &str) {
    println!("Your agent needs funding to continue.");
    println!("Address: {address}");
    println!("Amount needed: {needed}");
    println!();
    println!("Send the required amount to the address above, then retry your command.");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    // -- format_error ---------------------------------------------------------

    #[test]
    fn test_format_error_insufficient_funds() {
        let err = anyhow!("execution reverted: insufficient funds for transfer");
        assert_eq!(
            format_error(&err),
            "Insufficient funds. Run `agentmarket fund` to check your balance."
        );
    }

    #[test]
    fn test_format_error_nonce() {
        let err = anyhow!("nonce too low");
        assert_eq!(
            format_error(&err),
            "Transaction conflict. Please try again."
        );
    }

    #[test]
    fn test_format_error_timeout() {
        let err = anyhow!("request timeout after 30s");
        assert_eq!(
            format_error(&err),
            "Network unreachable. Check your internet connection."
        );
    }

    #[test]
    fn test_format_error_connection() {
        let err = anyhow!("connection refused");
        assert_eq!(
            format_error(&err),
            "Network unreachable. Check your internet connection."
        );
    }

    #[test]
    fn test_format_error_ipfs_lower() {
        let err = anyhow!("ipfs daemon not running");
        assert_eq!(
            format_error(&err),
            "Content network unavailable. Please try again later."
        );
    }

    #[test]
    fn test_format_error_ipfs_upper() {
        let err = anyhow!("IPFS pin failed");
        assert_eq!(
            format_error(&err),
            "Content network unavailable. Please try again later."
        );
    }

    #[test]
    fn test_format_error_keystore() {
        let err = anyhow!("failed to open keystore");
        assert_eq!(format_error(&err), "Invalid passphrase. Please try again.");
    }

    #[test]
    fn test_format_error_decrypt() {
        let err = anyhow!("could not decrypt payload");
        assert_eq!(format_error(&err), "Invalid passphrase. Please try again.");
    }

    #[test]
    fn test_format_error_already_registered() {
        let err = anyhow!("token already registered in registry");
        assert_eq!(
            format_error(&err),
            "Agent is already registered on the network."
        );
    }

    #[test]
    fn test_format_error_not_found() {
        let err = anyhow!("request not found");
        assert_eq!(
            format_error(&err),
            "Request not found. Check the ID and try again."
        );
    }

    #[test]
    fn test_format_error_expired() {
        let err = anyhow!("request has expired");
        assert_eq!(
            format_error(&err),
            "Request has expired and can no longer be processed."
        );
    }

    #[test]
    fn test_format_error_secret() {
        let err = anyhow!("secret missing from local cache");
        assert_eq!(
            format_error(&err),
            "Secret key missing. Your local data may be corrupted."
        );
    }

    #[test]
    fn test_format_error_cancelled() {
        let err = anyhow!("request was cancelled by the buyer");
        assert_eq!(format_error(&err), "Request was cancelled.");
    }

    #[test]
    fn test_format_error_validation() {
        let err = anyhow!("validation failed: handler returned non-zero exit code");
        assert_eq!(
            format_error(&err),
            "Validation failed. Check the handler output."
        );
    }

    #[test]
    fn test_format_error_permission() {
        let err = anyhow!("permission denied for this operation");
        assert_eq!(
            format_error(&err),
            "Permission denied. Check your identity and try again."
        );
    }

    #[test]
    fn test_format_error_unauthorized() {
        let err = anyhow!("unauthorized: caller is not the owner");
        assert_eq!(
            format_error(&err),
            "Permission denied. Check your identity and try again."
        );
    }

    #[test]
    fn test_format_error_parse() {
        let err = anyhow!("failed to parse input as JSON");
        assert_eq!(
            format_error(&err),
            "Invalid input format. Please check your command arguments."
        );
    }

    #[test]
    fn test_format_error_default() {
        let err = anyhow!("something unexpected");
        assert_eq!(format_error(&err), "Operation failed: something unexpected");
    }

    // -- short_id -------------------------------------------------------------

    #[test]
    fn test_short_id_long() {
        assert_eq!(short_id("0x1a2b3c4d5e6f7890"), "0x1a2b3c...");
    }

    #[test]
    fn test_short_id_short() {
        assert_eq!(short_id("abcdef"), "abcdef");
    }

    #[test]
    fn test_short_id_exactly_12() {
        assert_eq!(short_id("abcdefghijkl"), "abcdefghijkl");
    }

    #[test]
    fn test_short_id_13() {
        assert_eq!(short_id("abcdefghijklm"), "abcdefgh...");
    }

    // -- JSON mode ------------------------------------------------------------

    #[test]
    fn test_json_mode_default_off() {
        // Reset to known state.
        set_json_mode(false);
        assert!(!is_json_mode());
    }

    #[test]
    fn test_json_mode_toggle() {
        set_json_mode(true);
        assert!(is_json_mode());
        set_json_mode(false);
        assert!(!is_json_mode());
    }
}
