//! Configuration store for AgentMarket CLI.
//!
//! Manages reading and writing `~/.agentmarket/config.toml` (or the path
//! specified by `AGENTMARKET_HOME`). Environment variable overrides are
//! applied on every `load()` call following the precedence chain:
//!
//!   config.toml < AGENTMARKET_* env vars < CLI flags
//!
//! CLI-flag overrides are handled at the command layer, not here.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

// ---------------------------------------------------------------------------
// Config structs
// ---------------------------------------------------------------------------

/// Top-level configuration persisted in `config.toml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub agent: AgentConfig,
    pub network: NetworkConfig,
    pub identity: IdentityConfig,
    pub services: ServicesConfig,
}

/// Basic agent metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub description: String,
    pub version: String,
}

/// Network endpoints for Base L2 and IPFS.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub chain_rpc: String,
    pub ipfs_gateway: String,
    pub ipfs_api: String,
}

/// On-chain and off-chain identity references.
/// Fields are populated progressively: `public_key` after `init`,
/// `agent_id` and `ipfs_profile_cid` after `register`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityConfig {
    pub agent_id: String,
    pub ipfs_profile_cid: String,
    pub public_key: String,
}

/// Advertised capabilities and default pricing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServicesConfig {
    pub capabilities: Vec<String>,
    pub pricing_usd: f64,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: AgentConfig::default(),
            network: NetworkConfig::default(),
            identity: IdentityConfig::default(),
            services: ServicesConfig::default(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            version: "0.1.0".to_string(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            chain_rpc: "https://mainnet.base.org".to_string(),
            ipfs_gateway: "https://gateway.pinata.cloud".to_string(),
            ipfs_api: "http://localhost:5001".to_string(),
        }
    }
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            ipfs_profile_cid: String::new(),
            public_key: String::new(),
        }
    }
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            capabilities: Vec::new(),
            pricing_usd: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Name of the configuration file inside the config directory.
const CONFIG_FILE: &str = "config.toml";

/// Default directory name under the user home directory.
const DEFAULT_DIR_NAME: &str = ".agentmarket";

/// Unix permission mode for the config directory (owner-only rwx).
const DIR_PERMISSIONS: u32 = 0o700;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns the configuration directory path.
///
/// Resolution order:
/// 1. `AGENTMARKET_HOME` environment variable (if set and non-empty).
/// 2. `~/.agentmarket/` (using the `dirs` crate for home directory lookup).
///
/// The directory is created with `0700` permissions if it does not already
/// exist.
pub fn config_dir() -> Result<PathBuf> {
    let dir = match std::env::var("AGENTMARKET_HOME") {
        Ok(val) if !val.is_empty() => {
            debug!(path = %val, "using AGENTMARKET_HOME for config directory");
            PathBuf::from(val)
        }
        _ => {
            let home = dirs::home_dir()
                .context("unable to determine home directory")?;
            let path = home.join(DEFAULT_DIR_NAME);
            debug!(path = %path.display(), "using default config directory");
            path
        }
    };

    if !dir.exists() {
        debug!(path = %dir.display(), "creating config directory");
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create config directory: {}", dir.display()))?;

        let perms = fs::Permissions::from_mode(DIR_PERMISSIONS);
        fs::set_permissions(&dir, perms)
            .with_context(|| format!("failed to set permissions on {}", dir.display()))?;
    }

    Ok(dir)
}

/// Loads the configuration from `config.toml`.
///
/// After deserialisation the following environment variable overrides are
/// applied (when the variable is set and non-empty):
///
/// | Env var                    | Overrides               |
/// |----------------------------|-------------------------|
/// | `AGENTMARKET_RPC_URL`      | `network.chain_rpc`     |
/// | `AGENTMARKET_IPFS_API`     | `network.ipfs_api`      |
/// | `AGENTMARKET_IPFS_GATEWAY` | `network.ipfs_gateway`  |
pub fn load() -> Result<Config> {
    let path = config_dir()?.join(CONFIG_FILE);
    debug!(path = %path.display(), "loading config");

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;

    let mut config: Config = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file: {}", path.display()))?;

    // Apply environment variable overrides.
    apply_env_overrides(&mut config);

    debug!(?config, "config loaded");
    Ok(config)
}

/// Serialises and writes the configuration to `config.toml`.
///
/// The parent config directory is created if it does not yet exist (via
/// [`config_dir`]).
pub fn save(config: &Config) -> Result<()> {
    let path = config_dir()?.join(CONFIG_FILE);
    debug!(path = %path.display(), "saving config");

    let contents = toml::to_string_pretty(config)
        .context("failed to serialise config to TOML")?;

    fs::write(&path, contents)
        .with_context(|| format!("failed to write config file: {}", path.display()))?;

    debug!(path = %path.display(), "config saved");
    Ok(())
}

/// Returns `true` if a `config.toml` file already exists in the config
/// directory.
pub fn exists() -> Result<bool> {
    let path = config_dir()?.join(CONFIG_FILE);
    Ok(path.exists())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Applies `AGENTMARKET_*` environment variable overrides to the loaded
/// configuration. Only non-empty values are applied.
fn apply_env_overrides(config: &mut Config) {
    if let Ok(val) = std::env::var("AGENTMARKET_RPC_URL") {
        if !val.is_empty() {
            debug!(chain_rpc = %val, "overriding network.chain_rpc from AGENTMARKET_RPC_URL");
            config.network.chain_rpc = val;
        }
    }

    if let Ok(val) = std::env::var("AGENTMARKET_IPFS_API") {
        if !val.is_empty() {
            debug!(ipfs_api = %val, "overriding network.ipfs_api from AGENTMARKET_IPFS_API");
            config.network.ipfs_api = val;
        }
    }

    if let Ok(val) = std::env::var("AGENTMARKET_IPFS_GATEWAY") {
        if !val.is_empty() {
            debug!(ipfs_gateway = %val, "overriding network.ipfs_gateway from AGENTMARKET_IPFS_GATEWAY");
            config.network.ipfs_gateway = val;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    /// Mutex to serialise tests that mutate environment variables.
    /// `cargo test` runs tests in parallel by default, and env vars are
    /// process-global state, so we must hold a lock while touching them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: create a temporary directory and point `AGENTMARKET_HOME` at it
    /// for the duration of the closure. Restores (or removes) the env var
    /// afterwards. Acquires `ENV_LOCK` to prevent parallel env var mutation.
    fn with_temp_home<F: FnOnce(&PathBuf)>(f: F) {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let prev = env::var("AGENTMARKET_HOME").ok();

        env::set_var("AGENTMARKET_HOME", tmp.path());
        f(&tmp.path().to_path_buf());

        match prev {
            Some(v) => env::set_var("AGENTMARKET_HOME", v),
            None => env::remove_var("AGENTMARKET_HOME"),
        }
    }

    #[test]
    fn config_dir_creates_directory() {
        with_temp_home(|dir| {
            // Point at a nested path that does not exist yet.
            let sub = dir.join("nested");
            env::set_var("AGENTMARKET_HOME", &sub);

            let result = config_dir().expect("config_dir failed");
            assert_eq!(result, sub);
            assert!(sub.exists());

            // Check permissions are 0700.
            let meta = fs::metadata(&sub).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, DIR_PERMISSIONS);
        });
    }

    #[test]
    fn save_and_load_roundtrip() {
        with_temp_home(|_dir| {
            // Clear env overrides so they don't interfere.
            env::remove_var("AGENTMARKET_RPC_URL");
            env::remove_var("AGENTMARKET_IPFS_API");
            env::remove_var("AGENTMARKET_IPFS_GATEWAY");

            let mut cfg = Config::default();
            cfg.agent.name = "test-agent".to_string();
            cfg.agent.description = "A test agent".to_string();
            cfg.services.capabilities = vec!["code-review".to_string(), "testing".to_string()];
            cfg.services.pricing_usd = 5.0;
            cfg.identity.public_key = "0xabc123".to_string();

            save(&cfg).expect("save failed");
            assert!(exists().expect("exists failed"));

            let loaded = load().expect("load failed");
            assert_eq!(loaded.agent.name, "test-agent");
            assert_eq!(loaded.agent.description, "A test agent");
            assert_eq!(loaded.agent.version, "0.1.0");
            assert_eq!(loaded.network.chain_rpc, "https://mainnet.base.org");
            assert_eq!(loaded.network.ipfs_gateway, "https://gateway.pinata.cloud");
            assert_eq!(loaded.network.ipfs_api, "http://localhost:5001");
            assert_eq!(loaded.identity.public_key, "0xabc123");
            assert_eq!(loaded.identity.agent_id, "");
            assert_eq!(loaded.services.capabilities.len(), 2);
            assert!((loaded.services.pricing_usd - 5.0).abs() < f64::EPSILON);
        });
    }

    #[test]
    fn env_overrides_applied() {
        with_temp_home(|_dir| {
            // Save a default config first.
            save(&Config::default()).expect("save failed");

            // Set env overrides.
            env::set_var("AGENTMARKET_RPC_URL", "https://custom-rpc.example.com");
            env::set_var("AGENTMARKET_IPFS_API", "http://custom-ipfs:5001");
            env::set_var("AGENTMARKET_IPFS_GATEWAY", "https://custom-gw.example.com");

            let loaded = load().expect("load failed");
            assert_eq!(loaded.network.chain_rpc, "https://custom-rpc.example.com");
            assert_eq!(loaded.network.ipfs_api, "http://custom-ipfs:5001");
            assert_eq!(loaded.network.ipfs_gateway, "https://custom-gw.example.com");

            // Clean up.
            env::remove_var("AGENTMARKET_RPC_URL");
            env::remove_var("AGENTMARKET_IPFS_API");
            env::remove_var("AGENTMARKET_IPFS_GATEWAY");
        });
    }

    #[test]
    fn exists_returns_false_when_no_file() {
        with_temp_home(|_dir| {
            assert!(!exists().expect("exists failed"));
        });
    }

    #[test]
    fn default_config_values() {
        let cfg = Config::default();

        assert_eq!(cfg.agent.name, "");
        assert_eq!(cfg.agent.version, "0.1.0");
        assert_eq!(cfg.network.chain_rpc, "https://mainnet.base.org");
        assert_eq!(cfg.network.ipfs_gateway, "https://gateway.pinata.cloud");
        assert_eq!(cfg.network.ipfs_api, "http://localhost:5001");
        assert_eq!(cfg.identity.agent_id, "");
        assert_eq!(cfg.identity.ipfs_profile_cid, "");
        assert_eq!(cfg.identity.public_key, "");
        assert!(cfg.services.capabilities.is_empty());
        assert!((cfg.services.pricing_usd - 0.0).abs() < f64::EPSILON);
    }
}
