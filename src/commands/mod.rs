use anyhow::{bail, Result};

use crate::config;
use crate::engine::identity::{self, IdentityState};

pub mod claim;
pub mod daemon;
pub mod fund;
pub mod init;
pub mod register;
pub mod request;
pub mod respond;
pub mod search;
pub mod status;
pub mod validate;
pub mod withdraw;

/// Shared setup for commands that require an initialized and/or registered agent.
///
/// Encapsulates the repeated pattern of loading config, checking identity state,
/// loading the keystore, and deriving the agent address.
pub struct CommandContext {
    pub cfg: config::store::Config,
    pub public_key: String,
    pub address: String,
}

impl CommandContext {
    /// Load config, keystore, and derive identity. Requires agent to be registered.
    pub fn load_registered() -> Result<Self> {
        if !config::store::exists()? {
            bail!("Agent not initialized. Run `agentmarket init` first.");
        }

        let cfg = config::store::load()?;
        let state = identity::get_identity_state(&cfg);

        match state {
            IdentityState::Uninitialized => {
                bail!("Agent not initialized. Run `agentmarket init` first.");
            }
            IdentityState::Local { .. } => {
                bail!("Agent not registered. Run `agentmarket register` first.");
            }
            IdentityState::Registered { .. } => {}
        }

        let passphrase = config::keystore::get_passphrase()?;
        let key_bytes = config::keystore::load_key(&passphrase)?;
        let (public_key, address) = identity::address_from_key(&key_bytes)?;

        Ok(Self {
            cfg,
            public_key,
            address,
        })
    }

    /// Load config and keystore only (for commands that don't require registration,
    /// such as `fund`).
    pub fn load_initialized() -> Result<Self> {
        if !config::store::exists()? {
            bail!("Agent not initialized. Run `agentmarket init` first.");
        }

        let cfg = config::store::load()?;
        let passphrase = config::keystore::get_passphrase()?;
        let key_bytes = config::keystore::load_key(&passphrase)?;
        let (public_key, address) = identity::address_from_key(&key_bytes)?;

        Ok(Self {
            cfg,
            public_key,
            address,
        })
    }
}
