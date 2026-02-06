//! Agent identity management for AgentMarket CLI.
//!
//! Handles keypair generation (secp256k1), public key / Ethereum address
//! derivation, and the agent profile schema used for ERC-8004 registration.
//!
//! All cryptographic operations delegate to the `alloy` crate. No blockchain
//! or IPFS terminology is exposed beyond this module.

use std::fs;

use alloy::signers::local::PrivateKeySigner;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;
use zeroize::Zeroize;

use crate::config::store::{config_dir, Config};

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Agent profile metadata, serialised to `profile.json` and published via
/// IPFS during ERC-8004 registration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub pricing_usd: f64,
    /// Hex-encoded compressed secp256k1 public key.
    pub public_key: String,
    /// Checksummed Ethereum address (`0x`-prefixed).
    pub address: String,
    /// Profile schema version.
    pub version: String,
}

// ---------------------------------------------------------------------------
// Identity state
// ---------------------------------------------------------------------------

/// Tracks how far the agent has progressed through the identity lifecycle.
///
/// * `Uninitialized` -- no keypair has been generated yet.
/// * `Local` -- keypair exists locally but is not registered on-chain.
/// * `Registered` -- the agent has an on-chain ERC-8004 identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityState {
    Uninitialized,
    Local {
        address: String,
        public_key: String,
    },
    Registered {
        address: String,
        public_key: String,
        agent_id: String,
    },
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Name of the profile file inside the config directory.
const PROFILE_FILE: &str = "profile.json";

/// Current profile schema version.
const PROFILE_VERSION: &str = "0.1.0";

// ---------------------------------------------------------------------------
// Keypair generation
// ---------------------------------------------------------------------------

/// Generate a random secp256k1 keypair.
///
/// Returns `(private_key_bytes, hex_public_key, checksummed_address)` where:
///
/// * `private_key_bytes` is the 32-byte raw secret key.
/// * `hex_public_key` is the hex-encoded compressed public key (33 bytes,
///   no `0x` prefix).
/// * `checksummed_address` is the EIP-55 checksummed Ethereum address
///   (`0x`-prefixed).
pub fn generate_keypair() -> Result<(Vec<u8>, String, String)> {
    let signer = PrivateKeySigner::random();

    let address = format!("{}", signer.address());
    debug!(address = %address, "generated new keypair");

    // Extract private key bytes — the to_bytes() -> to_vec() chain creates an
    // intermediate GenericArray on the stack that is copied into the Vec.  We
    // cannot zero the GenericArray directly (it's a temporary), but the Vec
    // we return will be zeroed by the caller (TransactionSigner::from_bytes or
    // keystore::save_key).
    let mut raw_bytes = signer.credential().to_bytes();
    let private_key_bytes = raw_bytes.to_vec();
    // Zero the intermediate GenericArray copy on the stack.
    raw_bytes[..].zeroize();

    // Derive compressed public key from the signing key.
    let verifying_key = signer.credential().verifying_key();
    let public_key_bytes = verifying_key.to_encoded_point(true);
    let public_key_hex = hex::encode(public_key_bytes.as_bytes());

    debug!(
        public_key = %public_key_hex,
        "derived compressed public key"
    );

    Ok((private_key_bytes, public_key_hex, address))
}

/// Derive the hex-encoded compressed public key and checksummed Ethereum
/// address from raw private key bytes.
///
/// The input must be exactly 32 bytes (a secp256k1 scalar).
pub fn address_from_key(private_key_bytes: &[u8]) -> Result<(String, String)> {
    let mut key_array: [u8; 32] = private_key_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("private key must be exactly 32 bytes"))?;

    let result = (|| -> Result<(String, String)> {
        let signer = PrivateKeySigner::from_bytes(&key_array.into())
            .context("failed to construct signer from private key bytes")?;

        let address = format!("{}", signer.address());

        let verifying_key = signer.credential().verifying_key();
        let public_key_bytes = verifying_key.to_encoded_point(true);
        let public_key_hex = hex::encode(public_key_bytes.as_bytes());

        debug!(
            address = %address,
            public_key = %public_key_hex,
            "derived identity from existing key"
        );

        Ok((public_key_hex, address))
    })();

    // Zero the local copy of the key regardless of success or failure.
    key_array.zeroize();

    result
}

// ---------------------------------------------------------------------------
// Profile helpers
// ---------------------------------------------------------------------------

/// Build a new [`AgentProfile`] with the current schema version.
pub fn create_profile(
    name: &str,
    description: &str,
    capabilities: Vec<String>,
    pricing_usd: f64,
    public_key: &str,
    address: &str,
) -> AgentProfile {
    AgentProfile {
        name: name.to_string(),
        description: description.to_string(),
        capabilities,
        pricing_usd,
        public_key: public_key.to_string(),
        address: address.to_string(),
        version: PROFILE_VERSION.to_string(),
    }
}

/// Serialise `profile` to JSON and write it to
/// `~/.agentmarket/profile.json`.
pub fn save_profile(profile: &AgentProfile) -> Result<()> {
    let path = config_dir()?.join(PROFILE_FILE);
    debug!(path = %path.display(), "saving agent profile");

    let json = serde_json::to_string_pretty(profile)
        .context("failed to serialise agent profile to JSON")?;

    fs::write(&path, json)
        .with_context(|| format!("failed to write profile file: {}", path.display()))?;

    debug!(path = %path.display(), "profile saved");
    Ok(())
}

/// Read and parse `~/.agentmarket/profile.json`.
pub fn load_profile() -> Result<AgentProfile> {
    let path = config_dir()?.join(PROFILE_FILE);
    debug!(path = %path.display(), "loading agent profile");

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read profile file: {}", path.display()))?;

    let profile: AgentProfile = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse profile file: {}", path.display()))?;

    debug!(name = %profile.name, "profile loaded");
    Ok(profile)
}

// ---------------------------------------------------------------------------
// Identity state
// ---------------------------------------------------------------------------

/// Determine the current [`IdentityState`] from the persisted configuration.
///
/// * If `identity.public_key` is empty the agent is `Uninitialized`.
/// * If `identity.agent_id` is empty the identity is `Local` only.
/// * Otherwise the identity is fully `Registered`.
pub fn get_identity_state(config: &Config) -> IdentityState {
    if config.identity.public_key.is_empty() {
        debug!("identity state: Uninitialized");
        IdentityState::Uninitialized
    } else if config.identity.agent_id.is_empty() {
        debug!(
            address = %config.identity.public_key,
            "identity state: Local"
        );
        IdentityState::Local {
            address: String::new(), // address is derived, not stored in config separately
            public_key: config.identity.public_key.clone(),
        }
    } else {
        debug!(
            agent_id = %config.identity.agent_id,
            "identity state: Registered"
        );
        IdentityState::Registered {
            address: String::new(),
            public_key: config.identity.public_key.clone(),
            agent_id: config.identity.agent_id.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::store::{
        AgentConfig, Config, IdentityConfig, NetworkConfig, ServicesConfig,
    };
    use std::env;
    use std::sync::Mutex;

    /// Mutex to serialise tests that mutate environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: create a temporary directory and point `AGENTMARKET_HOME` at it
    /// for the duration of the closure.
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let prev = env::var("AGENTMARKET_HOME").ok();

        env::set_var("AGENTMARKET_HOME", tmp.path());
        f();

        match prev {
            Some(v) => env::set_var("AGENTMARKET_HOME", v),
            None => env::remove_var("AGENTMARKET_HOME"),
        }
    }

    // -- Keypair generation ---------------------------------------------------

    #[test]
    fn test_generate_keypair() {
        let (private_key, public_key, address) =
            generate_keypair().expect("keypair generation failed");

        // Private key must be 32 bytes (secp256k1 scalar).
        assert_eq!(private_key.len(), 32, "private key must be 32 bytes");

        // Public key must be non-empty hex string (compressed = 33 bytes = 66 hex chars).
        assert!(!public_key.is_empty(), "public key must not be empty");
        assert_eq!(
            public_key.len(),
            66,
            "compressed public key hex should be 66 chars"
        );

        // Address must be 0x-prefixed and 42 characters long.
        assert!(address.starts_with("0x"), "address must start with 0x");
        assert_eq!(address.len(), 42, "address must be 42 characters");

        // Verify we can round-trip: derive the same address from the private key.
        let (derived_pk, derived_addr) =
            address_from_key(&private_key).expect("address_from_key failed");
        assert_eq!(derived_pk, public_key);
        assert_eq!(derived_addr, address);
    }

    #[test]
    fn test_generate_keypair_uniqueness() {
        let (key_a, _, addr_a) = generate_keypair().expect("keypair a");
        let (key_b, _, addr_b) = generate_keypair().expect("keypair b");

        assert_ne!(key_a, key_b, "two random keys should differ");
        assert_ne!(addr_a, addr_b, "two random addresses should differ");
    }

    // -- Identity state -------------------------------------------------------

    #[test]
    fn test_identity_state() {
        // Uninitialized: public_key is empty.
        let config = Config {
            agent: AgentConfig::default(),
            network: NetworkConfig::default(),
            identity: IdentityConfig {
                public_key: String::new(),
                agent_id: String::new(),
                ipfs_profile_cid: String::new(),
            },
            services: ServicesConfig::default(),
        };
        assert_eq!(get_identity_state(&config), IdentityState::Uninitialized);

        // Local: public_key set, agent_id empty.
        let config = Config {
            identity: IdentityConfig {
                public_key: "02abc123".to_string(),
                agent_id: String::new(),
                ipfs_profile_cid: String::new(),
            },
            ..config.clone()
        };
        assert_eq!(
            get_identity_state(&config),
            IdentityState::Local {
                address: String::new(),
                public_key: "02abc123".to_string(),
            }
        );

        // Registered: both public_key and agent_id set.
        let config = Config {
            identity: IdentityConfig {
                public_key: "02abc123".to_string(),
                agent_id: "agent-42".to_string(),
                ipfs_profile_cid: "Qm...".to_string(),
            },
            ..config.clone()
        };
        assert_eq!(
            get_identity_state(&config),
            IdentityState::Registered {
                address: String::new(),
                public_key: "02abc123".to_string(),
                agent_id: "agent-42".to_string(),
            }
        );
    }

    // -- Profile roundtrip ----------------------------------------------------

    #[test]
    fn test_profile_roundtrip() {
        with_temp_home(|| {
            let profile = create_profile(
                "test-agent",
                "An agent for testing",
                vec!["code-review".to_string(), "testing".to_string()],
                5.0,
                "02abcdef1234567890",
                "0x1234567890abcdef1234567890abcdef12345678",
            );

            assert_eq!(profile.version, PROFILE_VERSION);

            save_profile(&profile).expect("save_profile failed");
            let loaded = load_profile().expect("load_profile failed");

            assert_eq!(loaded.name, "test-agent");
            assert_eq!(loaded.description, "An agent for testing");
            assert_eq!(loaded.capabilities, vec!["code-review", "testing"]);
            assert!((loaded.pricing_usd - 5.0).abs() < f64::EPSILON);
            assert_eq!(loaded.public_key, "02abcdef1234567890");
            assert_eq!(loaded.address, "0x1234567890abcdef1234567890abcdef12345678");
            assert_eq!(loaded.version, "0.1.0");
        });
    }

    // -- address_from_key -----------------------------------------------------

    #[test]
    fn test_address_from_key_invalid_length() {
        let short = vec![0u8; 16]; // too short
        let result = address_from_key(&short);
        assert!(result.is_err(), "should reject keys that are not 32 bytes");
    }

    // -- create_profile -------------------------------------------------------

    #[test]
    fn test_create_profile_sets_version() {
        let p = create_profile("n", "d", vec![], 0.0, "pk", "addr");
        assert_eq!(p.version, "0.1.0");
    }
}
