//! Request lifecycle state machine and local cache for AgentMarket CLI.
//!
//! Manages the full lifecycle of requests from creation through settlement.
//! Each request is tracked locally as a JSON file in `~/.agentmarket/requests/`
//! and progresses through a well-defined state machine:
//!
//!   Open → Responded → Validated → Claimed (terminal)
//!   Open → Cancelled (terminal)
//!   Open / Responded / Validated → Expired (terminal)
//!
//! This module is pure business logic. It does not interact with the blockchain
//! or IPFS directly — those operations are orchestrated by the command layer.

use std::fs;
use std::path::PathBuf;

use alloy::primitives::keccak256;
use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::config::store::config_dir;

// ---------------------------------------------------------------------------
// Request status (state machine)
// ---------------------------------------------------------------------------

/// Local representation of a request's lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalRequestStatus {
    /// Request created, waiting for a response.
    Open,
    /// Response submitted, waiting for validation.
    Responded,
    /// Validation passed, ready to claim.
    Validated,
    /// Settlement complete, payment transferred.
    Claimed,
    /// Request cancelled by buyer.
    Cancelled,
    /// Past deadline without claim.
    Expired,
}

impl LocalRequestStatus {
    /// Returns `true` if transitioning from `self` to `next` is valid.
    ///
    /// Valid transitions:
    /// - `Open` → `Responded`, `Cancelled`, `Expired`
    /// - `Responded` → `Validated`, `Expired`
    /// - `Validated` → `Claimed`, `Expired`
    /// - `Claimed`, `Cancelled`, `Expired` are terminal (no outgoing transitions).
    pub fn can_transition_to(&self, next: &LocalRequestStatus) -> bool {
        matches!(
            (self, next),
            (LocalRequestStatus::Open, LocalRequestStatus::Responded)
                | (LocalRequestStatus::Open, LocalRequestStatus::Cancelled)
                | (LocalRequestStatus::Open, LocalRequestStatus::Expired)
                | (LocalRequestStatus::Responded, LocalRequestStatus::Validated)
                | (LocalRequestStatus::Responded, LocalRequestStatus::Expired)
                | (LocalRequestStatus::Validated, LocalRequestStatus::Claimed)
                | (LocalRequestStatus::Validated, LocalRequestStatus::Expired)
        )
    }
}

// ---------------------------------------------------------------------------
// Request role
// ---------------------------------------------------------------------------

/// The role this agent plays in a given request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestRole {
    Buyer,
    Seller,
    Validator,
}

// ---------------------------------------------------------------------------
// Local request
// ---------------------------------------------------------------------------

/// A locally-tracked request with all metadata needed for the CLI.
///
/// Stored as `{request_id}.json` inside `~/.agentmarket/requests/`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalRequest {
    /// On-chain request ID (stringified U256).
    pub request_id: String,
    /// Role of this agent in the request.
    pub role: RequestRole,
    /// Current status.
    pub status: LocalRequestStatus,
    /// IPFS CID of the request payload.
    pub request_cid: String,
    /// Price in USDC (6 decimals as u64).
    pub price_usdc: u64,
    /// Deadline as Unix timestamp.
    pub deadline: u64,
    /// Response CID (set when response submitted).
    pub response_cid: Option<String>,
    /// Secret S (only stored locally by the seller, never published).
    pub secret: Option<String>,
    /// Secret hash `keccak256(S)` (published on-chain).
    pub secret_hash: Option<String>,
    /// Counterparty address.
    pub counterparty: Option<String>,
    /// Creation timestamp.
    pub created_at: u64,
    /// Last updated timestamp.
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// Request cache
// ---------------------------------------------------------------------------

/// Name of the requests subdirectory inside the config directory.
const REQUESTS_DIR: &str = "requests";

/// Manages local request state in `~/.agentmarket/requests/`.
///
/// Each request is stored as a JSON file: `{request_id}.json`.
pub struct RequestCache;

impl RequestCache {
    /// Returns the path to `~/.agentmarket/requests/`, creating the directory
    /// if it does not already exist.
    pub fn requests_dir() -> Result<PathBuf> {
        let dir = config_dir()?.join(REQUESTS_DIR);

        if !dir.exists() {
            debug!(path = %dir.display(), "creating requests directory");
            fs::create_dir_all(&dir).with_context(|| {
                format!("failed to create requests directory: {}", dir.display())
            })?;
        }

        Ok(dir)
    }

    /// Serialize `request` to JSON and write to
    /// `~/.agentmarket/requests/{request_id}.json`.
    pub fn save(request: &LocalRequest) -> Result<()> {
        let path = Self::requests_dir()?.join(format!("{}.json", request.request_id));
        debug!(path = %path.display(), request_id = %request.request_id, "saving request");

        let json =
            serde_json::to_string_pretty(request).context("failed to serialise request to JSON")?;

        fs::write(&path, json)
            .with_context(|| format!("failed to write request file: {}", path.display()))?;

        debug!(path = %path.display(), "request saved");
        Ok(())
    }

    /// Read and deserialize a request from
    /// `~/.agentmarket/requests/{request_id}.json`.
    pub fn load(request_id: &str) -> Result<LocalRequest> {
        let path = Self::requests_dir()?.join(format!("{}.json", request_id));
        debug!(path = %path.display(), "loading request");

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read request file: {}", path.display()))?;

        let request: LocalRequest = serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse request file: {}", path.display()))?;

        debug!(request_id = %request.request_id, "request loaded");
        Ok(request)
    }

    /// Read all request files in the requests directory.
    pub fn load_all() -> Result<Vec<LocalRequest>> {
        let dir = Self::requests_dir()?;
        debug!(path = %dir.display(), "loading all requests");

        let mut requests = Vec::new();

        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read requests directory: {}", dir.display()))?
        {
            let entry = entry.context("failed to read directory entry")?;
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let contents = fs::read_to_string(&path)
                    .with_context(|| format!("failed to read request file: {}", path.display()))?;

                let request: LocalRequest = serde_json::from_str(&contents)
                    .with_context(|| format!("failed to parse request file: {}", path.display()))?;

                requests.push(request);
            }
        }

        debug!(count = requests.len(), "loaded all requests");
        Ok(requests)
    }

    /// Read all requests and filter by status.
    pub fn load_by_status(status: LocalRequestStatus) -> Result<Vec<LocalRequest>> {
        let all = Self::load_all()?;
        let filtered: Vec<LocalRequest> = all.into_iter().filter(|r| r.status == status).collect();

        debug!(count = filtered.len(), ?status, "loaded requests by status");
        Ok(filtered)
    }

    /// Read all requests and filter by role.
    pub fn load_by_role(role: RequestRole) -> Result<Vec<LocalRequest>> {
        let all = Self::load_all()?;
        let filtered: Vec<LocalRequest> = all.into_iter().filter(|r| r.role == role).collect();

        debug!(count = filtered.len(), ?role, "loaded requests by role");
        Ok(filtered)
    }

    /// Remove a request file from the cache.
    pub fn delete(request_id: &str) -> Result<()> {
        let path = Self::requests_dir()?.join(format!("{}.json", request_id));
        debug!(path = %path.display(), "deleting request");

        fs::remove_file(&path)
            .with_context(|| format!("failed to delete request file: {}", path.display()))?;

        debug!(path = %path.display(), "request deleted");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers: secret generation
// ---------------------------------------------------------------------------

/// Generate a random 32-byte secret and its keccak256 hash.
///
/// Returns `(secret_hex, hash_hex)` where:
/// - `secret_hex` is 64 hex characters (32 bytes, no prefix)
/// - `hash_hex` is 66 hex characters (32 bytes, `0x`-prefixed)
pub fn generate_secret() -> (String, String) {
    let mut rng = rand::thread_rng();
    let mut secret_bytes = [0u8; 32];
    rng.fill(&mut secret_bytes);

    let secret_hex = hex::encode(secret_bytes);
    let hash = keccak256(secret_bytes);
    let hash_hex = format!("0x{}", hex::encode(hash));

    debug!("generated secret and hash");
    (secret_hex, hash_hex)
}

// ---------------------------------------------------------------------------
// Helpers: price formatting
// ---------------------------------------------------------------------------

/// Convert a USDC amount (6 decimals) to a human-readable dollar string.
///
/// # Examples
///
/// ```
/// # use agentmarket::engine::requests::format_price_usd;
/// assert_eq!(format_price_usd(5_000_000), "$5.00");
/// assert_eq!(format_price_usd(0), "$0.00");
/// assert_eq!(format_price_usd(1), "$0.000001");
/// ```
pub fn format_price_usd(usdc_amount: u64) -> String {
    let dollars = usdc_amount / 1_000_000;
    let cents = usdc_amount % 1_000_000;

    if cents == 0 {
        format!("${dollars}.00")
    } else {
        // Determine how many trailing zeros to trim, but keep at least 2 decimal places.
        let cents_str = format!("{:06}", cents);
        let trimmed = cents_str.trim_end_matches('0');
        let decimals = if trimmed.len() < 2 {
            &cents_str[..2]
        } else {
            trimmed
        };
        format!("${dollars}.{decimals}")
    }
}

/// Convert a dollar amount to USDC atomic units (6 decimals).
///
/// # Examples
///
/// ```
/// # use agentmarket::engine::requests::dollars_to_usdc;
/// assert_eq!(dollars_to_usdc(5.0), 5_000_000);
/// assert_eq!(dollars_to_usdc(0.5), 500_000);
/// ```
pub fn dollars_to_usdc(dollars: f64) -> u64 {
    (dollars * 1_000_000.0).round() as u64
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

    /// Build a sample `LocalRequest` for testing.
    fn sample_request(id: &str, status: LocalRequestStatus, role: RequestRole) -> LocalRequest {
        LocalRequest {
            request_id: id.to_string(),
            role,
            status,
            request_cid: "QmTestCid123".to_string(),
            price_usdc: 5_000_000,
            deadline: 1_700_000_000,
            response_cid: None,
            secret: None,
            secret_hash: None,
            counterparty: None,
            created_at: 1_699_000_000,
            updated_at: 1_699_000_000,
        }
    }

    // -- State transition tests -----------------------------------------------

    #[test]
    fn test_valid_transitions() {
        // Open → Responded, Cancelled, Expired
        assert!(LocalRequestStatus::Open.can_transition_to(&LocalRequestStatus::Responded));
        assert!(LocalRequestStatus::Open.can_transition_to(&LocalRequestStatus::Cancelled));
        assert!(LocalRequestStatus::Open.can_transition_to(&LocalRequestStatus::Expired));

        // Responded → Validated, Expired
        assert!(LocalRequestStatus::Responded.can_transition_to(&LocalRequestStatus::Validated));
        assert!(LocalRequestStatus::Responded.can_transition_to(&LocalRequestStatus::Expired));

        // Validated → Claimed, Expired
        assert!(LocalRequestStatus::Validated.can_transition_to(&LocalRequestStatus::Claimed));
        assert!(LocalRequestStatus::Validated.can_transition_to(&LocalRequestStatus::Expired));
    }

    #[test]
    fn test_invalid_transitions() {
        // Terminal states cannot transition.
        assert!(!LocalRequestStatus::Claimed.can_transition_to(&LocalRequestStatus::Open));
        assert!(!LocalRequestStatus::Claimed.can_transition_to(&LocalRequestStatus::Responded));
        assert!(!LocalRequestStatus::Claimed.can_transition_to(&LocalRequestStatus::Validated));
        assert!(!LocalRequestStatus::Claimed.can_transition_to(&LocalRequestStatus::Expired));
        assert!(!LocalRequestStatus::Claimed.can_transition_to(&LocalRequestStatus::Cancelled));

        assert!(!LocalRequestStatus::Cancelled.can_transition_to(&LocalRequestStatus::Open));
        assert!(!LocalRequestStatus::Cancelled.can_transition_to(&LocalRequestStatus::Responded));
        assert!(!LocalRequestStatus::Cancelled.can_transition_to(&LocalRequestStatus::Expired));

        assert!(!LocalRequestStatus::Expired.can_transition_to(&LocalRequestStatus::Open));
        assert!(!LocalRequestStatus::Expired.can_transition_to(&LocalRequestStatus::Responded));
        assert!(!LocalRequestStatus::Expired.can_transition_to(&LocalRequestStatus::Claimed));

        // Skip-ahead transitions are invalid.
        assert!(!LocalRequestStatus::Open.can_transition_to(&LocalRequestStatus::Validated));
        assert!(!LocalRequestStatus::Open.can_transition_to(&LocalRequestStatus::Claimed));
        assert!(!LocalRequestStatus::Responded.can_transition_to(&LocalRequestStatus::Claimed));
        assert!(!LocalRequestStatus::Responded.can_transition_to(&LocalRequestStatus::Cancelled));

        // Self-transitions are invalid.
        assert!(!LocalRequestStatus::Open.can_transition_to(&LocalRequestStatus::Open));
        assert!(!LocalRequestStatus::Responded.can_transition_to(&LocalRequestStatus::Responded));
        assert!(!LocalRequestStatus::Validated.can_transition_to(&LocalRequestStatus::Validated));

        // Backward transitions are invalid.
        assert!(!LocalRequestStatus::Responded.can_transition_to(&LocalRequestStatus::Open));
        assert!(!LocalRequestStatus::Validated.can_transition_to(&LocalRequestStatus::Open));
        assert!(!LocalRequestStatus::Validated.can_transition_to(&LocalRequestStatus::Responded));
    }

    // -- generate_secret ------------------------------------------------------

    #[test]
    fn test_generate_secret_lengths() {
        let (secret_hex, hash_hex) = generate_secret();

        // Secret: 32 bytes = 64 hex characters, no prefix.
        assert_eq!(secret_hex.len(), 64, "secret hex should be 64 chars");
        assert!(
            !secret_hex.starts_with("0x"),
            "secret should not have 0x prefix"
        );

        // Hash: 32 bytes = 64 hex characters + "0x" prefix = 66 chars.
        assert_eq!(hash_hex.len(), 66, "hash hex should be 66 chars");
        assert!(hash_hex.starts_with("0x"), "hash should have 0x prefix");
    }

    #[test]
    fn test_generate_secret_hash_matches() {
        let (secret_hex, hash_hex) = generate_secret();

        // Decode the secret back to bytes, hash it, and compare.
        let secret_bytes = hex::decode(&secret_hex).expect("secret should be valid hex");
        let expected_hash = keccak256(&secret_bytes);
        let expected_hex = format!("0x{}", hex::encode(expected_hash));

        assert_eq!(
            hash_hex, expected_hex,
            "hash should match keccak256(secret)"
        );
    }

    #[test]
    fn test_generate_secret_uniqueness() {
        let (secret_a, _) = generate_secret();
        let (secret_b, _) = generate_secret();
        assert_ne!(secret_a, secret_b, "two random secrets should differ");
    }

    // -- format_price_usd -----------------------------------------------------

    #[test]
    fn test_format_price_usd() {
        assert_eq!(format_price_usd(0), "$0.00");
        assert_eq!(format_price_usd(1_000_000), "$1.00");
        assert_eq!(format_price_usd(5_000_000), "$5.00");
        assert_eq!(format_price_usd(5_500_000), "$5.50");
        assert_eq!(format_price_usd(5_550_000), "$5.55");
        assert_eq!(format_price_usd(5_123_456), "$5.123456");
        assert_eq!(format_price_usd(100_000_000), "$100.00");
        assert_eq!(format_price_usd(1), "$0.000001");
        assert_eq!(format_price_usd(10), "$0.00001");
        assert_eq!(format_price_usd(100), "$0.0001");
        assert_eq!(format_price_usd(1_000), "$0.001");
        assert_eq!(format_price_usd(10_000), "$0.01");
        assert_eq!(format_price_usd(100_000), "$0.10");
        assert_eq!(format_price_usd(500_000), "$0.50");
        assert_eq!(format_price_usd(999_999), "$0.999999");
    }

    // -- dollars_to_usdc ------------------------------------------------------

    #[test]
    fn test_dollars_to_usdc() {
        assert_eq!(dollars_to_usdc(0.0), 0);
        assert_eq!(dollars_to_usdc(1.0), 1_000_000);
        assert_eq!(dollars_to_usdc(5.0), 5_000_000);
        assert_eq!(dollars_to_usdc(5.5), 5_500_000);
        assert_eq!(dollars_to_usdc(0.01), 10_000);
        assert_eq!(dollars_to_usdc(100.0), 100_000_000);
        assert_eq!(dollars_to_usdc(0.000001), 1);
    }

    #[test]
    fn test_dollars_to_usdc_roundtrip() {
        let amounts: Vec<u64> = vec![0, 1_000_000, 5_500_000, 100_000_000, 10_000, 500_000];
        for amount in amounts {
            let dollars = amount as f64 / 1_000_000.0;
            let roundtripped = dollars_to_usdc(dollars);
            assert_eq!(
                roundtripped, amount,
                "round-trip failed for amount {amount}"
            );
        }
    }

    // -- RequestCache save/load round-trip ------------------------------------

    #[test]
    fn test_cache_save_load_roundtrip() {
        with_temp_home(|| {
            let request = sample_request("42", LocalRequestStatus::Open, RequestRole::Buyer);

            RequestCache::save(&request).expect("save failed");
            let loaded = RequestCache::load("42").expect("load failed");

            assert_eq!(loaded.request_id, "42");
            assert_eq!(loaded.status, LocalRequestStatus::Open);
            assert_eq!(loaded.role, RequestRole::Buyer);
            assert_eq!(loaded.request_cid, "QmTestCid123");
            assert_eq!(loaded.price_usdc, 5_000_000);
            assert_eq!(loaded.deadline, 1_700_000_000);
            assert_eq!(loaded.response_cid, None);
            assert_eq!(loaded.secret, None);
            assert_eq!(loaded.secret_hash, None);
            assert_eq!(loaded.counterparty, None);
            assert_eq!(loaded.created_at, 1_699_000_000);
            assert_eq!(loaded.updated_at, 1_699_000_000);
        });
    }

    #[test]
    fn test_cache_save_load_with_optional_fields() {
        with_temp_home(|| {
            let mut request =
                sample_request("99", LocalRequestStatus::Responded, RequestRole::Seller);
            request.response_cid = Some("QmResponseCid".to_string());
            request.secret = Some("deadbeef".repeat(8));
            request.secret_hash = Some("0xabcdef".to_string());
            request.counterparty = Some("0x1234".to_string());

            RequestCache::save(&request).expect("save failed");
            let loaded = RequestCache::load("99").expect("load failed");

            assert_eq!(loaded.response_cid, Some("QmResponseCid".to_string()));
            assert_eq!(loaded.secret, Some("deadbeef".repeat(8)));
            assert_eq!(loaded.secret_hash, Some("0xabcdef".to_string()));
            assert_eq!(loaded.counterparty, Some("0x1234".to_string()));
        });
    }

    // -- RequestCache load_all ------------------------------------------------

    #[test]
    fn test_cache_load_all() {
        with_temp_home(|| {
            let r1 = sample_request("1", LocalRequestStatus::Open, RequestRole::Buyer);
            let r2 = sample_request("2", LocalRequestStatus::Responded, RequestRole::Seller);
            let r3 = sample_request("3", LocalRequestStatus::Claimed, RequestRole::Validator);

            RequestCache::save(&r1).expect("save r1");
            RequestCache::save(&r2).expect("save r2");
            RequestCache::save(&r3).expect("save r3");

            let all = RequestCache::load_all().expect("load_all failed");
            assert_eq!(all.len(), 3);

            let ids: Vec<String> = all.iter().map(|r| r.request_id.clone()).collect();
            assert!(ids.contains(&"1".to_string()));
            assert!(ids.contains(&"2".to_string()));
            assert!(ids.contains(&"3".to_string()));
        });
    }

    #[test]
    fn test_cache_load_all_empty() {
        with_temp_home(|| {
            let all = RequestCache::load_all().expect("load_all failed");
            assert!(all.is_empty());
        });
    }

    // -- RequestCache load_by_status ------------------------------------------

    #[test]
    fn test_cache_load_by_status() {
        with_temp_home(|| {
            let r1 = sample_request("1", LocalRequestStatus::Open, RequestRole::Buyer);
            let r2 = sample_request("2", LocalRequestStatus::Open, RequestRole::Seller);
            let r3 = sample_request("3", LocalRequestStatus::Responded, RequestRole::Seller);
            let r4 = sample_request("4", LocalRequestStatus::Claimed, RequestRole::Buyer);

            RequestCache::save(&r1).expect("save r1");
            RequestCache::save(&r2).expect("save r2");
            RequestCache::save(&r3).expect("save r3");
            RequestCache::save(&r4).expect("save r4");

            let open = RequestCache::load_by_status(LocalRequestStatus::Open)
                .expect("load_by_status Open");
            assert_eq!(open.len(), 2);
            assert!(open.iter().all(|r| r.status == LocalRequestStatus::Open));

            let responded = RequestCache::load_by_status(LocalRequestStatus::Responded)
                .expect("load_by_status Responded");
            assert_eq!(responded.len(), 1);
            assert_eq!(responded[0].request_id, "3");

            let claimed = RequestCache::load_by_status(LocalRequestStatus::Claimed)
                .expect("load_by_status Claimed");
            assert_eq!(claimed.len(), 1);
            assert_eq!(claimed[0].request_id, "4");

            let cancelled = RequestCache::load_by_status(LocalRequestStatus::Cancelled)
                .expect("load_by_status Cancelled");
            assert!(cancelled.is_empty());
        });
    }

    // -- RequestCache load_by_role --------------------------------------------

    #[test]
    fn test_cache_load_by_role() {
        with_temp_home(|| {
            let r1 = sample_request("1", LocalRequestStatus::Open, RequestRole::Buyer);
            let r2 = sample_request("2", LocalRequestStatus::Open, RequestRole::Buyer);
            let r3 = sample_request("3", LocalRequestStatus::Responded, RequestRole::Seller);
            let r4 = sample_request("4", LocalRequestStatus::Claimed, RequestRole::Validator);

            RequestCache::save(&r1).expect("save r1");
            RequestCache::save(&r2).expect("save r2");
            RequestCache::save(&r3).expect("save r3");
            RequestCache::save(&r4).expect("save r4");

            let buyers =
                RequestCache::load_by_role(RequestRole::Buyer).expect("load_by_role Buyer");
            assert_eq!(buyers.len(), 2);
            assert!(buyers.iter().all(|r| r.role == RequestRole::Buyer));

            let sellers =
                RequestCache::load_by_role(RequestRole::Seller).expect("load_by_role Seller");
            assert_eq!(sellers.len(), 1);
            assert_eq!(sellers[0].request_id, "3");

            let validators =
                RequestCache::load_by_role(RequestRole::Validator).expect("load_by_role Validator");
            assert_eq!(validators.len(), 1);
            assert_eq!(validators[0].request_id, "4");
        });
    }

    // -- RequestCache delete --------------------------------------------------

    #[test]
    fn test_cache_delete() {
        with_temp_home(|| {
            let request = sample_request("42", LocalRequestStatus::Open, RequestRole::Buyer);
            RequestCache::save(&request).expect("save failed");

            // Verify it exists.
            let loaded = RequestCache::load("42");
            assert!(loaded.is_ok());

            // Delete it.
            RequestCache::delete("42").expect("delete failed");

            // Verify it no longer exists.
            let loaded = RequestCache::load("42");
            assert!(loaded.is_err());
        });
    }

    #[test]
    fn test_cache_delete_nonexistent() {
        with_temp_home(|| {
            let result = RequestCache::delete("nonexistent");
            assert!(
                result.is_err(),
                "deleting a nonexistent request should fail"
            );
        });
    }
}
