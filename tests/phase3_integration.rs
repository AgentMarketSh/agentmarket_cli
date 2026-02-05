//! Phase 3 cross-module integration tests.
//!
//! These tests verify that the Phase 3 modules (request engine, reputation engine)
//! work together correctly with identity and config in realistic workflows.
//!
//! Tests that mutate environment variables must run with `--test-threads=1`.

use std::collections::HashSet;
use std::env;
use std::sync::Mutex;

use agentmarket::config::store::{
    AgentConfig, Config, IdentityConfig, NetworkConfig, ServicesConfig,
};
use agentmarket::engine::identity;
use agentmarket::engine::reputation::{
    compute_reputation, format_earnings_usd, format_reputation, reputation_tier, ValidationRecord,
};
use agentmarket::engine::requests::{
    dollars_to_usdc, format_price_usd, generate_secret, LocalRequest, LocalRequestStatus,
    RequestCache, RequestRole,
};
use alloy::primitives::keccak256;

/// Mutex to serialise tests that mutate environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Helper: create a temporary directory, point `AGENTMARKET_HOME` at it,
/// run the closure, then restore the previous value.
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

/// Build a `LocalRequest` with the given parameters and realistic identity data.
fn make_request(
    id: &str,
    status: LocalRequestStatus,
    role: RequestRole,
    price_usdc: u64,
    address: &str,
) -> LocalRequest {
    LocalRequest {
        request_id: id.to_string(),
        role,
        status,
        request_cid: format!("QmRequest{}", id),
        price_usdc,
        deadline: 1_700_000_000,
        response_cid: None,
        secret: None,
        secret_hash: None,
        counterparty: Some(address.to_string()),
        created_at: 1_699_000_000,
        updated_at: 1_699_000_000,
    }
}

/// Build a `ValidationRecord` helper.
fn make_record(request_id: &str, passed: bool, timestamp: u64) -> ValidationRecord {
    ValidationRecord {
        request_id: request_id.to_string(),
        passed,
        timestamp,
        validator: "0xvalidator".to_string(),
    }
}

// ===========================================================================
// 1. Request lifecycle with config
// ===========================================================================

/// Generate a keypair, build a config, then create requests and walk them
/// through the full state machine: Open -> Responded -> Validated -> Claimed.
#[test]
fn request_lifecycle_with_identity_full_path() {
    with_temp_home(|| {
        // Generate identity.
        let (_, public_key_hex, address) =
            identity::generate_keypair().expect("generate_keypair failed");

        // Build config with identity data.
        let config = Config {
            agent: AgentConfig {
                name: "lifecycle-agent".to_string(),
                description: "Tests request lifecycle".to_string(),
                version: "0.1.0".to_string(),
            },
            network: NetworkConfig::default(),
            identity: IdentityConfig {
                public_key: public_key_hex.clone(),
                agent_id: String::new(),
                ipfs_profile_cid: String::new(),
            },
            services: ServicesConfig {
                capabilities: vec!["code-review".to_string()],
                pricing_usd: 5.0,
            },
        };

        // Verify identity state is Local (not yet registered).
        let state = identity::get_identity_state(&config);
        assert_eq!(
            state,
            identity::IdentityState::Local {
                address: String::new(),
                public_key: public_key_hex.clone(),
            }
        );

        // Create a request at the agent's pricing.
        let price = dollars_to_usdc(config.services.pricing_usd);
        let mut request = make_request(
            "100",
            LocalRequestStatus::Open,
            RequestRole::Seller,
            price,
            &address,
        );

        // Walk through the state machine.
        assert!(request
            .status
            .can_transition_to(&LocalRequestStatus::Responded));
        request.status = LocalRequestStatus::Responded;
        request.response_cid = Some("QmResponse100".to_string());
        request.updated_at = 1_699_001_000;

        assert!(request
            .status
            .can_transition_to(&LocalRequestStatus::Validated));
        request.status = LocalRequestStatus::Validated;
        request.updated_at = 1_699_002_000;

        // Generate a secret for the claim step.
        let (secret_hex, hash_hex) = generate_secret();
        request.secret = Some(secret_hex);
        request.secret_hash = Some(hash_hex);

        assert!(request
            .status
            .can_transition_to(&LocalRequestStatus::Claimed));
        request.status = LocalRequestStatus::Claimed;
        request.updated_at = 1_699_003_000;

        // Save and verify persistence.
        RequestCache::save(&request).expect("save failed");
        let loaded = RequestCache::load("100").expect("load failed");

        assert_eq!(loaded.status, LocalRequestStatus::Claimed);
        assert_eq!(loaded.price_usdc, 5_000_000);
        assert!(loaded.secret.is_some());
        assert!(loaded.secret_hash.is_some());
        assert_eq!(loaded.response_cid, Some("QmResponse100".to_string()));
        assert_eq!(loaded.counterparty, Some(address));
    });
}

/// Save multiple requests, filter by status, verify correct subsets returned.
#[test]
fn request_cache_filter_by_status_across_full_set() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

        // Create requests in various statuses.
        let requests = vec![
            make_request(
                "1",
                LocalRequestStatus::Open,
                RequestRole::Buyer,
                1_000_000,
                &address,
            ),
            make_request(
                "2",
                LocalRequestStatus::Open,
                RequestRole::Seller,
                2_000_000,
                &address,
            ),
            make_request(
                "3",
                LocalRequestStatus::Responded,
                RequestRole::Seller,
                3_000_000,
                &address,
            ),
            make_request(
                "4",
                LocalRequestStatus::Validated,
                RequestRole::Buyer,
                4_000_000,
                &address,
            ),
            make_request(
                "5",
                LocalRequestStatus::Claimed,
                RequestRole::Seller,
                5_000_000,
                &address,
            ),
            make_request(
                "6",
                LocalRequestStatus::Cancelled,
                RequestRole::Buyer,
                6_000_000,
                &address,
            ),
            make_request(
                "7",
                LocalRequestStatus::Expired,
                RequestRole::Seller,
                7_000_000,
                &address,
            ),
        ];

        for r in &requests {
            RequestCache::save(r).expect("save failed");
        }

        // Verify status filters.
        let open = RequestCache::load_by_status(LocalRequestStatus::Open).expect("filter open");
        assert_eq!(open.len(), 2);
        let open_ids: HashSet<String> = open.iter().map(|r| r.request_id.clone()).collect();
        assert!(open_ids.contains("1"));
        assert!(open_ids.contains("2"));

        let responded =
            RequestCache::load_by_status(LocalRequestStatus::Responded).expect("filter responded");
        assert_eq!(responded.len(), 1);
        assert_eq!(responded[0].request_id, "3");

        let validated =
            RequestCache::load_by_status(LocalRequestStatus::Validated).expect("filter validated");
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].request_id, "4");

        let claimed =
            RequestCache::load_by_status(LocalRequestStatus::Claimed).expect("filter claimed");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].request_id, "5");

        let cancelled =
            RequestCache::load_by_status(LocalRequestStatus::Cancelled).expect("filter cancelled");
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].request_id, "6");

        let expired =
            RequestCache::load_by_status(LocalRequestStatus::Expired).expect("filter expired");
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].request_id, "7");
    });
}

/// Save requests as buyer and seller, verify load_by_role returns correct subsets.
#[test]
fn request_cache_filter_by_role_buyer_seller() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

        let r1 = make_request(
            "10",
            LocalRequestStatus::Open,
            RequestRole::Buyer,
            1_000_000,
            &address,
        );
        let r2 = make_request(
            "11",
            LocalRequestStatus::Responded,
            RequestRole::Buyer,
            2_000_000,
            &address,
        );
        let r3 = make_request(
            "12",
            LocalRequestStatus::Claimed,
            RequestRole::Seller,
            3_000_000,
            &address,
        );
        let r4 = make_request(
            "13",
            LocalRequestStatus::Open,
            RequestRole::Seller,
            4_000_000,
            &address,
        );
        let r5 = make_request(
            "14",
            LocalRequestStatus::Validated,
            RequestRole::Validator,
            5_000_000,
            &address,
        );

        for r in [&r1, &r2, &r3, &r4, &r5] {
            RequestCache::save(r).expect("save failed");
        }

        let buyers = RequestCache::load_by_role(RequestRole::Buyer).expect("load buyers");
        assert_eq!(buyers.len(), 2);
        assert!(buyers.iter().all(|r| r.role == RequestRole::Buyer));
        let buyer_ids: HashSet<String> = buyers.iter().map(|r| r.request_id.clone()).collect();
        assert!(buyer_ids.contains("10"));
        assert!(buyer_ids.contains("11"));

        let sellers = RequestCache::load_by_role(RequestRole::Seller).expect("load sellers");
        assert_eq!(sellers.len(), 2);
        assert!(sellers.iter().all(|r| r.role == RequestRole::Seller));
        let seller_ids: HashSet<String> = sellers.iter().map(|r| r.request_id.clone()).collect();
        assert!(seller_ids.contains("12"));
        assert!(seller_ids.contains("13"));

        let validators =
            RequestCache::load_by_role(RequestRole::Validator).expect("load validators");
        assert_eq!(validators.len(), 1);
        assert_eq!(validators[0].request_id, "14");
    });
}

// ===========================================================================
// 2. Request + Reputation cross-module
// ===========================================================================

/// Create a set of requests (Claimed, Expired, Cancelled) and compute
/// reputation. 100% completion rate should yield Excellent tier.
#[test]
fn reputation_from_all_claimed_requests_yields_excellent() {
    let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

    // Simulate 10 Claimed requests with validation records.
    let records: Vec<ValidationRecord> = (0..10)
        .map(|i| make_record(&format!("r{}", i), true, 1_700_000_000 + i * 60))
        .collect();

    let total_earnings = 10 * 5_000_000u64; // 10 requests at $5 each
    let score = compute_reputation(&address, &records, total_earnings, 120);

    assert_eq!(score.score, 100.0);
    assert_eq!(score.completed_requests, 10);
    assert_eq!(score.failed_validations, 0);
    assert_eq!(score.total_earnings_usdc, 50_000_000);
    assert_eq!(reputation_tier(&score), "Excellent");
    assert_eq!(format_reputation(&score), "100.0");
    assert_eq!(format_earnings_usd(score.total_earnings_usdc), "$50.00");
}

/// Mixed completion (some Claimed, some Expired/Cancelled via failed validation)
/// should yield correct tier based on ratio.
#[test]
fn reputation_mixed_completion_correct_tier() {
    let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

    // 8 passed, 2 failed -> 80% -> Good tier
    let mut records: Vec<ValidationRecord> = (0..8)
        .map(|i| make_record(&format!("r{}", i), true, 1_700_000_000 + i * 60))
        .collect();
    records.extend((8..10).map(|i| make_record(&format!("r{}", i), false, 1_700_000_000 + i * 60)));

    let earnings = 8 * 5_000_000u64; // Only completed requests earned
    let score = compute_reputation(&address, &records, earnings, 200);

    assert!((score.score - 80.0).abs() < f64::EPSILON);
    assert_eq!(score.completed_requests, 8);
    assert_eq!(score.failed_validations, 2);
    assert_eq!(reputation_tier(&score), "Good");
    assert_eq!(format_earnings_usd(score.total_earnings_usdc), "$40.00");
}

/// 6 passed, 4 failed -> 60% -> Fair tier.
#[test]
fn reputation_fair_tier_at_sixty_percent() {
    let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

    let mut records: Vec<ValidationRecord> = (0..6)
        .map(|i| make_record(&format!("r{}", i), true, 1_700_000_000))
        .collect();
    records.extend((6..10).map(|i| make_record(&format!("r{}", i), false, 1_700_000_000)));

    let score = compute_reputation(&address, &records, 30_000_000, 300);

    assert!((score.score - 60.0).abs() < f64::EPSILON);
    assert_eq!(reputation_tier(&score), "Fair");
}

/// 2 passed, 8 failed -> 20% -> New tier (below 60 threshold).
#[test]
fn reputation_low_completion_new_tier() {
    let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

    let mut records: Vec<ValidationRecord> = (0..2)
        .map(|i| make_record(&format!("r{}", i), true, 1_700_000_000))
        .collect();
    records.extend((2..10).map(|i| make_record(&format!("r{}", i), false, 1_700_000_000)));

    let score = compute_reputation(&address, &records, 10_000_000, 500);

    assert!((score.score - 20.0).abs() < f64::EPSILON);
    assert_eq!(reputation_tier(&score), "New");
}

/// No records at all -> Unrated tier.
#[test]
fn reputation_no_records_unrated() {
    let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

    let score = compute_reputation(&address, &[], 0, 0);

    assert_eq!(score.score, 0.0);
    assert_eq!(reputation_tier(&score), "Unrated");
    assert_eq!(format_reputation(&score), "N/A");
}

/// Build reputation from request cache data: save requests with various
/// terminal states, derive validation records, compute reputation.
#[test]
fn reputation_summary_from_request_cache() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

        // Create 5 Claimed, 2 Expired, 1 Cancelled requests.
        let statuses = vec![
            ("c1", LocalRequestStatus::Claimed),
            ("c2", LocalRequestStatus::Claimed),
            ("c3", LocalRequestStatus::Claimed),
            ("c4", LocalRequestStatus::Claimed),
            ("c5", LocalRequestStatus::Claimed),
            ("e1", LocalRequestStatus::Expired),
            ("e2", LocalRequestStatus::Expired),
            ("x1", LocalRequestStatus::Cancelled),
        ];

        for (id, status) in &statuses {
            let r = make_request(id, status.clone(), RequestRole::Seller, 5_000_000, &address);
            RequestCache::save(&r).expect("save failed");
        }

        // Load all requests and derive validation records from them.
        let all = RequestCache::load_all().expect("load_all failed");
        assert_eq!(all.len(), 8);

        // Build validation records: Claimed = passed, Expired = failed, Cancelled = not counted.
        let records: Vec<ValidationRecord> = all
            .iter()
            .filter(|r| {
                r.status == LocalRequestStatus::Claimed || r.status == LocalRequestStatus::Expired
            })
            .map(|r| ValidationRecord {
                request_id: r.request_id.clone(),
                passed: r.status == LocalRequestStatus::Claimed,
                timestamp: r.updated_at,
                validator: "0xvalidator".to_string(),
            })
            .collect();

        assert_eq!(records.len(), 7); // 5 claimed + 2 expired

        let claimed = all
            .iter()
            .filter(|r| r.status == LocalRequestStatus::Claimed)
            .count();
        let total_earnings = claimed as u64 * 5_000_000;

        let score = compute_reputation(&address, &records, total_earnings, 150);

        // 5 passed out of 7 (5 + 2 failed) = ~71.4%
        assert_eq!(score.completed_requests, 5);
        assert_eq!(score.failed_validations, 2);
        assert!((score.score - (5.0 / 7.0 * 100.0)).abs() < 0.1);
        assert_eq!(reputation_tier(&score), "Fair"); // 71.4% >= 60
        assert_eq!(score.total_earnings_usdc, 25_000_000);
        assert_eq!(format_earnings_usd(score.total_earnings_usdc), "$25.00");
    });
}

// ===========================================================================
// 3. Secret generation + hash verification
// ===========================================================================

/// Generate a secret, decode it, hash it with keccak256, and verify the
/// hash matches what generate_secret returned.
#[test]
fn secret_generation_hash_verification() {
    let (secret_hex, hash_hex) = generate_secret();

    // Decode secret to bytes.
    let secret_bytes = hex::decode(&secret_hex).expect("secret should be valid hex");
    assert_eq!(secret_bytes.len(), 32, "secret must be exactly 32 bytes");

    // Hash with keccak256.
    let computed_hash = keccak256(&secret_bytes);
    let computed_hex = format!("0x{}", hex::encode(computed_hash));

    assert_eq!(hash_hex, computed_hex, "hash must match keccak256(secret)");

    // Verify hash is exactly 32 bytes (66 hex chars with 0x prefix).
    let hash_bytes = hex::decode(&hash_hex[2..]).expect("hash should be valid hex after 0x");
    assert_eq!(hash_bytes.len(), 32, "hash must be exactly 32 bytes");
}

/// Generate multiple secrets and verify they are all unique.
#[test]
fn secret_generation_all_unique() {
    let mut secrets = HashSet::new();
    let mut hashes = HashSet::new();

    for _ in 0..20 {
        let (secret_hex, hash_hex) = generate_secret();
        assert!(
            secrets.insert(secret_hex.clone()),
            "secret collision detected: {}",
            secret_hex
        );
        assert!(
            hashes.insert(hash_hex.clone()),
            "hash collision detected: {}",
            hash_hex
        );
    }

    assert_eq!(secrets.len(), 20);
    assert_eq!(hashes.len(), 20);
}

/// Verify secret and hash byte lengths precisely.
#[test]
fn secret_and_hash_exact_byte_lengths() {
    for _ in 0..5 {
        let (secret_hex, hash_hex) = generate_secret();

        // Secret: 64 hex chars = 32 bytes, no prefix.
        assert_eq!(secret_hex.len(), 64);
        assert!(!secret_hex.starts_with("0x"));
        let secret_bytes = hex::decode(&secret_hex).expect("valid hex");
        assert_eq!(secret_bytes.len(), 32);

        // Hash: 66 hex chars = 0x + 64 = 32 bytes, with prefix.
        assert_eq!(hash_hex.len(), 66);
        assert!(hash_hex.starts_with("0x"));
        let hash_bytes = hex::decode(&hash_hex[2..]).expect("valid hex");
        assert_eq!(hash_bytes.len(), 32);
    }
}

/// Generate a secret, attach it to a request, save, load, and verify
/// the hash still verifies against the secret.
#[test]
fn secret_persisted_in_request_still_verifies() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");
        let (secret_hex, hash_hex) = generate_secret();

        let mut request = make_request(
            "secret-test",
            LocalRequestStatus::Validated,
            RequestRole::Seller,
            5_000_000,
            &address,
        );
        request.secret = Some(secret_hex.clone());
        request.secret_hash = Some(hash_hex.clone());

        RequestCache::save(&request).expect("save failed");
        let loaded = RequestCache::load("secret-test").expect("load failed");

        // Re-verify the hash from the loaded data.
        let loaded_secret = loaded.secret.expect("secret should be present");
        let loaded_hash = loaded.secret_hash.expect("hash should be present");

        let secret_bytes = hex::decode(&loaded_secret).expect("valid hex");
        let recomputed = keccak256(&secret_bytes);
        let recomputed_hex = format!("0x{}", hex::encode(recomputed));

        assert_eq!(loaded_hash, recomputed_hex, "persisted hash must verify");
        assert_eq!(loaded_secret, secret_hex);
        assert_eq!(loaded_hash, hash_hex);
    });
}

// ===========================================================================
// 4. Price conversions with request flow
// ===========================================================================

/// Create request at $5.00, verify dollars_to_usdc and format_price_usd roundtrip.
#[test]
fn price_conversion_five_dollars() {
    let price_usdc = dollars_to_usdc(5.0);
    assert_eq!(price_usdc, 5_000_000);
    assert_eq!(format_price_usd(price_usdc), "$5.00");
}

/// Create request at $0.01, verify conversion.
#[test]
fn price_conversion_one_cent() {
    let price_usdc = dollars_to_usdc(0.01);
    assert_eq!(price_usdc, 10_000);
    assert_eq!(format_price_usd(price_usdc), "$0.01");
}

/// Roundtrip various dollar values through the conversion pipeline.
#[test]
fn price_conversion_roundtrip_various() {
    let test_cases: Vec<(f64, u64, &str)> = vec![
        (0.0, 0, "$0.00"),
        (1.0, 1_000_000, "$1.00"),
        (0.50, 500_000, "$0.50"),
        (10.0, 10_000_000, "$10.00"),
        (99.99, 99_990_000, "$99.99"),
        (100.0, 100_000_000, "$100.00"),
        (0.000001, 1, "$0.000001"),
    ];

    for (dollars, expected_usdc, expected_display) in test_cases {
        let usdc = dollars_to_usdc(dollars);
        assert_eq!(
            usdc, expected_usdc,
            "dollars_to_usdc({}) should be {}",
            dollars, expected_usdc
        );
        assert_eq!(
            format_price_usd(usdc),
            expected_display,
            "format_price_usd({}) should be '{}'",
            usdc,
            expected_display
        );
    }
}

/// Create requests at various price points, save, load, and verify
/// the price fields are preserved and display correctly.
#[test]
fn price_conversions_persisted_in_requests() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

        let prices: Vec<(f64, &str)> = vec![
            (5.0, "price-5"),
            (0.01, "price-001"),
            (100.0, "price-100"),
            (0.50, "price-050"),
        ];

        for (dollars, id) in &prices {
            let usdc = dollars_to_usdc(*dollars);
            let r = make_request(
                id,
                LocalRequestStatus::Open,
                RequestRole::Buyer,
                usdc,
                &address,
            );
            RequestCache::save(&r).expect("save failed");
        }

        // Load each and verify price.
        let loaded_5 = RequestCache::load("price-5").expect("load failed");
        assert_eq!(loaded_5.price_usdc, 5_000_000);
        assert_eq!(format_price_usd(loaded_5.price_usdc), "$5.00");

        let loaded_001 = RequestCache::load("price-001").expect("load failed");
        assert_eq!(loaded_001.price_usdc, 10_000);
        assert_eq!(format_price_usd(loaded_001.price_usdc), "$0.01");

        let loaded_100 = RequestCache::load("price-100").expect("load failed");
        assert_eq!(loaded_100.price_usdc, 100_000_000);
        assert_eq!(format_price_usd(loaded_100.price_usdc), "$100.00");

        let loaded_050 = RequestCache::load("price-050").expect("load failed");
        assert_eq!(loaded_050.price_usdc, 500_000);
        assert_eq!(format_price_usd(loaded_050.price_usdc), "$0.50");
    });
}

// ===========================================================================
// 5. Request cache persistence with temp home
// ===========================================================================

/// Save a request to cache in a temp dir, load it, verify all fields preserved.
#[test]
fn cache_persistence_all_fields_preserved() {
    with_temp_home(|| {
        let (_, public_key_hex, address) =
            identity::generate_keypair().expect("generate_keypair failed");

        let (secret_hex, hash_hex) = generate_secret();

        let request = LocalRequest {
            request_id: "persist-1".to_string(),
            role: RequestRole::Seller,
            status: LocalRequestStatus::Validated,
            request_cid: "QmRequestCidPersist".to_string(),
            price_usdc: 7_500_000,
            deadline: 1_700_100_000,
            response_cid: Some("QmResponseCidPersist".to_string()),
            secret: Some(secret_hex.clone()),
            secret_hash: Some(hash_hex.clone()),
            counterparty: Some(address.clone()),
            created_at: 1_699_000_000,
            updated_at: 1_699_050_000,
        };

        RequestCache::save(&request).expect("save failed");
        let loaded = RequestCache::load("persist-1").expect("load failed");

        assert_eq!(loaded.request_id, "persist-1");
        assert_eq!(loaded.role, RequestRole::Seller);
        assert_eq!(loaded.status, LocalRequestStatus::Validated);
        assert_eq!(loaded.request_cid, "QmRequestCidPersist");
        assert_eq!(loaded.price_usdc, 7_500_000);
        assert_eq!(loaded.deadline, 1_700_100_000);
        assert_eq!(
            loaded.response_cid,
            Some("QmResponseCidPersist".to_string())
        );
        assert_eq!(loaded.secret, Some(secret_hex));
        assert_eq!(loaded.secret_hash, Some(hash_hex));
        assert_eq!(loaded.counterparty, Some(address));
        assert_eq!(loaded.created_at, 1_699_000_000);
        assert_eq!(loaded.updated_at, 1_699_050_000);

        // Verify the public key was properly generated (sanity check).
        assert_eq!(public_key_hex.len(), 66, "compressed pubkey = 66 hex chars");
    });
}

/// Save multiple requests, load_all, verify count and content.
#[test]
fn cache_persistence_load_all_count_and_content() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

        let ids: Vec<&str> = vec!["multi-1", "multi-2", "multi-3", "multi-4", "multi-5"];
        for id in &ids {
            let r = make_request(
                id,
                LocalRequestStatus::Open,
                RequestRole::Buyer,
                1_000_000,
                &address,
            );
            RequestCache::save(&r).expect("save failed");
        }

        let all = RequestCache::load_all().expect("load_all failed");
        assert_eq!(all.len(), 5);

        let loaded_ids: HashSet<String> = all.iter().map(|r| r.request_id.clone()).collect();
        for id in &ids {
            assert!(
                loaded_ids.contains(*id),
                "expected to find request '{}'",
                id
            );
        }

        // Verify each request has correct data.
        for r in &all {
            assert_eq!(r.status, LocalRequestStatus::Open);
            assert_eq!(r.role, RequestRole::Buyer);
            assert_eq!(r.price_usdc, 1_000_000);
        }
    });
}

/// Delete a request, verify it is gone from load_all.
#[test]
fn cache_persistence_delete_removes_from_load_all() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

        let r1 = make_request(
            "del-1",
            LocalRequestStatus::Open,
            RequestRole::Buyer,
            1_000_000,
            &address,
        );
        let r2 = make_request(
            "del-2",
            LocalRequestStatus::Responded,
            RequestRole::Seller,
            2_000_000,
            &address,
        );
        let r3 = make_request(
            "del-3",
            LocalRequestStatus::Claimed,
            RequestRole::Seller,
            3_000_000,
            &address,
        );

        RequestCache::save(&r1).expect("save r1");
        RequestCache::save(&r2).expect("save r2");
        RequestCache::save(&r3).expect("save r3");

        // Verify all three exist.
        let all = RequestCache::load_all().expect("load_all");
        assert_eq!(all.len(), 3);

        // Delete the middle one.
        RequestCache::delete("del-2").expect("delete failed");

        // Verify it is gone.
        let result = RequestCache::load("del-2");
        assert!(result.is_err(), "deleted request should not load");

        let remaining = RequestCache::load_all().expect("load_all after delete");
        assert_eq!(remaining.len(), 2);

        let remaining_ids: HashSet<String> =
            remaining.iter().map(|r| r.request_id.clone()).collect();
        assert!(remaining_ids.contains("del-1"));
        assert!(remaining_ids.contains("del-3"));
        assert!(!remaining_ids.contains("del-2"));
    });
}

/// Delete all requests one by one, verify cache is empty at the end.
#[test]
fn cache_persistence_delete_all_leaves_empty() {
    with_temp_home(|| {
        let (_, _, address) = identity::generate_keypair().expect("generate_keypair failed");

        for i in 0..3 {
            let r = make_request(
                &format!("da-{}", i),
                LocalRequestStatus::Open,
                RequestRole::Buyer,
                1_000_000,
                &address,
            );
            RequestCache::save(&r).expect("save failed");
        }

        assert_eq!(RequestCache::load_all().expect("load_all").len(), 3);

        for i in 0..3 {
            RequestCache::delete(&format!("da-{}", i)).expect("delete failed");
        }

        let final_all = RequestCache::load_all().expect("load_all after deleting all");
        assert!(
            final_all.is_empty(),
            "cache should be empty after deleting all requests"
        );
    });
}

// ===========================================================================
// 6. Cross-module: request lifecycle + reputation + identity (end-to-end)
// ===========================================================================

/// Full end-to-end flow: generate identity, create requests through full
/// lifecycle, derive reputation from completed requests.
#[test]
fn full_request_lifecycle_to_reputation() {
    with_temp_home(|| {
        // Step 1: Generate identity.
        let (_, public_key_hex, address) =
            identity::generate_keypair().expect("generate_keypair failed");

        // Step 2: Build config.
        let config = Config {
            agent: AgentConfig {
                name: "e2e-agent".to_string(),
                description: "End-to-end test agent".to_string(),
                version: "0.1.0".to_string(),
            },
            network: NetworkConfig::default(),
            identity: IdentityConfig {
                public_key: public_key_hex.clone(),
                agent_id: "agent-e2e".to_string(),
                ipfs_profile_cid: "QmProfile".to_string(),
            },
            services: ServicesConfig {
                capabilities: vec!["testing".to_string()],
                pricing_usd: 10.0,
            },
        };

        assert_eq!(
            identity::get_identity_state(&config),
            identity::IdentityState::Registered {
                address: String::new(),
                public_key: public_key_hex,
                agent_id: "agent-e2e".to_string(),
            }
        );

        let price = dollars_to_usdc(config.services.pricing_usd);
        assert_eq!(price, 10_000_000);

        // Step 3: Create and process requests through lifecycle.
        // 3 requests: 2 will complete (Claimed), 1 will expire.
        for i in 0..3 {
            let mut r = make_request(
                &format!("e2e-{}", i),
                LocalRequestStatus::Open,
                RequestRole::Seller,
                price,
                &address,
            );

            // Transition to Responded.
            r.status = LocalRequestStatus::Responded;
            r.response_cid = Some(format!("QmResponse{}", i));
            r.updated_at += 1000;

            if i < 2 {
                // Complete: Validated -> Claimed.
                r.status = LocalRequestStatus::Validated;
                r.updated_at += 1000;

                let (secret, hash) = generate_secret();
                r.secret = Some(secret);
                r.secret_hash = Some(hash);

                r.status = LocalRequestStatus::Claimed;
                r.updated_at += 1000;
            } else {
                // Expire.
                r.status = LocalRequestStatus::Expired;
                r.updated_at += 5000;
            }

            RequestCache::save(&r).expect("save failed");
        }

        // Step 4: Load all requests and verify statuses.
        let all = RequestCache::load_all().expect("load_all failed");
        assert_eq!(all.len(), 3);

        let claimed = RequestCache::load_by_status(LocalRequestStatus::Claimed).expect("claimed");
        assert_eq!(claimed.len(), 2);

        let expired = RequestCache::load_by_status(LocalRequestStatus::Expired).expect("expired");
        assert_eq!(expired.len(), 1);

        // Step 5: Derive validation records from request data.
        let records: Vec<ValidationRecord> = all
            .iter()
            .filter(|r| {
                r.status == LocalRequestStatus::Claimed || r.status == LocalRequestStatus::Expired
            })
            .map(|r| ValidationRecord {
                request_id: r.request_id.clone(),
                passed: r.status == LocalRequestStatus::Claimed,
                timestamp: r.updated_at,
                validator: "0xvalidator".to_string(),
            })
            .collect();

        let total_earnings = claimed.len() as u64 * price;

        // Step 6: Compute and verify reputation.
        let score = compute_reputation(&address, &records, total_earnings, 120);

        // 2 passed, 1 failed -> 66.7%
        assert_eq!(score.completed_requests, 2);
        assert_eq!(score.failed_validations, 1);
        assert!((score.score - (2.0 / 3.0 * 100.0)).abs() < 0.1);
        assert_eq!(reputation_tier(&score), "Fair"); // 66.7% >= 60
        assert_eq!(score.total_earnings_usdc, 20_000_000); // 2 * $10
        assert_eq!(format_earnings_usd(score.total_earnings_usdc), "$20.00");
        assert_eq!(format_price_usd(price), "$10.00");
    });
}
