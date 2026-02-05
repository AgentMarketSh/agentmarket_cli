use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};
use tracing::debug;

use crate::config;
use crate::engine::identity;
use crate::output::formatter;

/// Run the `init` command: generate an agent identity and save local config.
///
/// This command works fully offline. It generates a secp256k1 keypair, saves
/// the private key in an encrypted keystore, writes the agent configuration to
/// `~/.agentmarket/config.toml`, and persists the agent profile to
/// `~/.agentmarket/profile.json`.
pub async fn run() -> Result<()> {
    debug!("starting init command");

    // 1. Check if already initialized.
    if config::store::exists()? {
        formatter::print_warning(
            "Agent already initialized. To re-initialize, delete ~/.agentmarket/ first.",
        );
        return Ok(());
    }

    // 2. Collect user input via stdin prompts.
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    let name = prompt_line(&mut reader, "Agent name: ")?;
    let description = prompt_line(&mut reader, "Description: ")?;
    let capabilities_raw = prompt_line(&mut reader, "Capabilities (comma-separated): ")?;
    let capabilities: Vec<String> = capabilities_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let price_str = prompt_line(&mut reader, "Price per task (USD): ")?;
    let pricing_usd: f64 = price_str
        .parse()
        .context("invalid price — please enter a number (e.g. 5.00)")?;

    debug!(
        name = %name,
        description = %description,
        capabilities = ?capabilities,
        pricing_usd = pricing_usd,
        "collected agent configuration"
    );

    // 3. Get keystore passphrase (with confirmation unless env var is set).
    let passphrase = config::keystore::get_passphrase()?;

    // If the passphrase came from interactive input (no env var), confirm it.
    if std::env::var("AGENTMARKET_KEYSTORE_PASSPHRASE").is_err() {
        let confirm = rpassword::prompt_password_stdout("Confirm passphrase: ")
            .context("failed to read passphrase confirmation")?;
        if passphrase != confirm {
            bail!("Passphrases do not match.");
        }
    }

    // 4. Generate keypair.
    debug!("generating agent keypair");
    let (private_key_bytes, public_key_hex, address) = identity::generate_keypair()?;
    debug!(address = %address, "keypair generated");

    // 5. Save encrypted keystore.
    debug!("saving encrypted keystore");
    config::keystore::save_key(&private_key_bytes, &passphrase)?;

    // 6. Build and save config.
    let cfg = config::store::Config {
        agent: config::store::AgentConfig {
            name: name.clone(),
            description: description.clone(),
            ..Default::default()
        },
        network: config::store::NetworkConfig::default(),
        identity: config::store::IdentityConfig {
            public_key: public_key_hex.clone(),
            agent_id: String::new(),
            ipfs_profile_cid: String::new(),
        },
        services: config::store::ServicesConfig {
            capabilities: capabilities.clone(),
            pricing_usd,
        },
    };
    debug!("saving configuration");
    config::store::save(&cfg)?;

    // 7. Build and save profile.
    let profile = identity::create_profile(
        &name,
        &description,
        capabilities,
        pricing_usd,
        &public_key_hex,
        &address,
    );
    debug!("saving agent profile");
    identity::save_profile(&profile)?;

    // 8. Display results.
    formatter::print_success("Agent identity created");
    formatter::print_success("Configuration saved to ~/.agentmarket/config.toml");
    println!();
    formatter::print_info(
        "To join the network, fund your agent's wallet with a small amount of ETH on Base:",
    );
    formatter::print_wallet_address(&address);
    println!();
    formatter::print_info("Then run `agentmarket register` to complete setup.");

    Ok(())
}

/// Print a prompt to stderr (so it appears even when stdout is redirected) and
/// read a single trimmed line from the provided reader.
fn prompt_line<R: BufRead>(reader: &mut R, prompt: &str) -> Result<String> {
    eprint!("{}", prompt);
    io::stderr().flush().context("failed to flush stderr")?;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read input")?;

    Ok(line.trim().to_string())
}
