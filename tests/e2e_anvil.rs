//! End-to-end integration tests for the full request lifecycle.
//!
//! These tests exercise the protocol from identity generation through
//! request creation, response, validation, claim, and reputation --
//! including ECIES encryption, mailbox messaging, and the hash-lock
//! secret pattern.
//!
//! **All tests are `#[ignore]` by default** because some require a local
//! Anvil instance or are intentionally slow.
//!
//! Run with:
//!   cargo test --test e2e_anvil -- --ignored --test-threads=1
//!
//! Requires:
//!   anvil running on localhost:8545  (for `e2e_identity_and_balance_check`)

use std::env;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::keccak256;

use agentmarket::chain::client::ChainClient;
use agentmarket::engine::identity;
use agentmarket::engine::reputation::{
    compute_reputation, format_earnings_usd, format_reputation, reputation_tier, ValidationRecord,
};
use agentmarket::engine::requests::{
    dollars_to_usdc, format_price_usd, generate_secret, LocalRequest, LocalRequestStatus,
    RequestCache, RequestRole,
};
use agentmarket::engine::validation::{self, HandlerOutput};
use agentmarket::ipfs::encryption;
use agentmarket::ipfs::mailbox::{self, Mailbox, MailboxMessage};

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

/// Helper: generate a random secp256k1 keypair and return
/// `(private_key_bytes, compressed_public_key_hex, checksummed_address)`.
fn random_keypair() -> (Vec<u8>, String, String) {
    identity::generate_keypair().expect("generate_keypair should succeed")
}

/// Helper: build a `LocalRequest` with the given parameters.
fn make_request(
    id: &str,
    status: LocalRequestStatus,
    role: RequestRole,
    price_usdc: u64,
    counterparty: &str,
) -> LocalRequest {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    LocalRequest {
        request_id: id.to_string(),
        role,
        status,
        request_cid: format!("QmRequest{}", id),
        price_usdc,
        deadline: now + 3600, // 1 hour from now
        response_cid: None,
        secret: None,
        secret_hash: None,
        counterparty: Some(counterparty.to_string()),
        created_at: now,
        updated_at: now,
    }
}

// ===========================================================================
// Test 1: Identity generation + balance check against Anvil
// ===========================================================================

/// Generate an identity, connect to a local Anvil instance, and verify
/// that the default Anvil account has a non-zero ETH balance.
///
/// This validates that the chain client works against a real EVM node.
/// If Anvil is not running the test handles the connection failure
/// gracefully and still passes (with a skip message).
#[tokio::test]
#[ignore]
async fn e2e_identity_and_balance_check() {
    // Step 1: Generate a fresh keypair.
    let (_private_key, public_key_hex, address_str) = random_keypair();

    // Sanity checks on the generated identity.
    assert_eq!(public_key_hex.len(), 66, "compressed pubkey = 66 hex chars");
    assert!(address_str.starts_with("0x"), "address must be 0x-prefixed");
    assert_eq!(address_str.len(), 42, "address = 42 chars");

    // Step 2: Create a ChainClient pointing at the local Anvil instance.
    let anvil_url = "http://127.0.0.1:8545";
    let client = match ChainClient::new(anvil_url).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "SKIP: could not create chain client (Anvil may not be running): {}",
                e
            );
            return;
        }
    };

    // Step 3: Check connectivity. If Anvil is not reachable, skip gracefully.
    if !client.is_connected().await {
        eprintln!("SKIP: Anvil is not reachable at {}", anvil_url);
        return;
    }

    // Step 4: Verify we can read the block number.
    let block_number = client
        .get_block_number()
        .await
        .expect("get_block_number should succeed against Anvil");
    // Anvil starts at block 0 or 1; any non-negative value is fine.
    assert!(
        block_number < u64::MAX,
        "block number should be a reasonable value"
    );

    // Step 5: Parse the generated address and query its ETH balance.
    // Anvil prefunds accounts 0..9 with 10000 ETH each. Our random key
    // will NOT be prefunded, so we query one of the well-known Anvil
    // addresses instead to prove the RPC call works.
    //
    // Anvil default account #0: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
    let anvil_account: alloy::primitives::Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        .parse()
        .expect("known Anvil address should parse");

    let balance = client
        .get_eth_balance(anvil_account)
        .await
        .expect("get_eth_balance should succeed against Anvil");

    // Anvil prefunds with 10000 ETH = 10^22 wei. The balance must be > 0.
    assert!(
        balance > alloy::primitives::U256::ZERO,
        "Anvil default account should have a non-zero balance, got: {}",
        balance
    );

    // Also query the generated address -- should be 0 since it was just created.
    let generated_addr: alloy::primitives::Address =
        address_str.parse().expect("generated address should parse");

    let generated_balance = client
        .get_eth_balance(generated_addr)
        .await
        .expect("get_eth_balance for generated address should succeed");

    assert_eq!(
        generated_balance,
        alloy::primitives::U256::ZERO,
        "freshly generated address should have zero balance"
    );
}

// ===========================================================================
// Test 2: Full request lifecycle with local state
// ===========================================================================

/// Simulate the complete request lifecycle locally:
///   Open -> Responded -> Validated -> Claimed
///
/// Exercises identity generation, request cache persistence, state machine
/// transitions, secret generation, and mailbox seal/open round-trip.
#[tokio::test]
#[ignore]
async fn e2e_request_lifecycle_local_state() {
    with_temp_home(|| {
        // -- Identity setup -----------------------------------------------

        let (buyer_sk, buyer_pk, buyer_addr) = random_keypair();
        let (seller_sk, seller_pk, seller_addr) = random_keypair();

        // Verify buyer and seller are distinct.
        assert_ne!(buyer_addr, seller_addr, "buyer and seller must differ");
        assert_ne!(buyer_pk, seller_pk, "buyer and seller pubkeys must differ");

        // Verify mailbox topics are distinct.
        let buyer_mailbox = Mailbox::new(&buyer_pk).expect("buyer mailbox");
        let seller_mailbox = Mailbox::new(&seller_pk).expect("seller mailbox");
        assert_ne!(
            buyer_mailbox.topic(),
            seller_mailbox.topic(),
            "mailbox topics must differ"
        );

        // -- Step 1: Buyer creates a request (Open) -----------------------

        let price = dollars_to_usdc(5.0);
        assert_eq!(price, 5_000_000);

        let mut buyer_request = make_request(
            "lifecycle-1",
            LocalRequestStatus::Open,
            RequestRole::Buyer,
            price,
            &seller_addr,
        );
        RequestCache::save(&buyer_request).expect("save buyer request");

        // Seller also tracks the same request from their perspective.
        let mut seller_request = make_request(
            "lifecycle-1",
            LocalRequestStatus::Open,
            RequestRole::Seller,
            price,
            &buyer_addr,
        );

        // -- Step 2: Seller responds (Open -> Responded) ------------------

        assert!(buyer_request
            .status
            .can_transition_to(&LocalRequestStatus::Responded));
        assert!(seller_request
            .status
            .can_transition_to(&LocalRequestStatus::Responded));

        // Generate the seller's secret and hash.
        let (secret_hex, hash_hex) = generate_secret();
        assert_eq!(secret_hex.len(), 64, "secret hex = 64 chars");
        assert_eq!(hash_hex.len(), 66, "hash hex = 66 chars (with 0x)");

        // Verify hash matches keccak256(secret).
        let secret_bytes = hex::decode(&secret_hex).expect("valid hex");
        let recomputed_hash = keccak256(&secret_bytes);
        assert_eq!(
            hash_hex,
            format!("0x{}", hex::encode(recomputed_hash)),
            "hash must match keccak256(secret)"
        );

        buyer_request.status = LocalRequestStatus::Responded;
        buyer_request.response_cid = Some("QmSellerResponse".to_string());
        buyer_request.updated_at += 100;

        seller_request.status = LocalRequestStatus::Responded;
        seller_request.response_cid = Some("QmSellerResponse".to_string());
        seller_request.secret = Some(secret_hex.clone());
        seller_request.secret_hash = Some(hash_hex.clone());
        seller_request.updated_at += 100;

        RequestCache::save(&buyer_request).expect("save buyer responded");

        // -- Step 3: Validator validates (Responded -> Validated) ----------

        assert!(buyer_request
            .status
            .can_transition_to(&LocalRequestStatus::Validated));
        assert!(seller_request
            .status
            .can_transition_to(&LocalRequestStatus::Validated));

        // Simulate a validation handler producing a passing score.
        let handler_output = HandlerOutput {
            score: 85,
            reason: "deliverable meets requirements".to_string(),
        };
        assert!(validation::is_passing(&handler_output));

        let validation_result = validation::create_result("lifecycle-1", &handler_output);
        assert!(validation_result.passed);
        assert_eq!(validation_result.score, 85);

        buyer_request.status = LocalRequestStatus::Validated;
        buyer_request.updated_at += 100;

        seller_request.status = LocalRequestStatus::Validated;
        seller_request.updated_at += 100;

        RequestCache::save(&buyer_request).expect("save buyer validated");

        // -- Step 4: Seller claims (Validated -> Claimed) -----------------

        assert!(buyer_request
            .status
            .can_transition_to(&LocalRequestStatus::Claimed));
        assert!(seller_request
            .status
            .can_transition_to(&LocalRequestStatus::Claimed));

        buyer_request.status = LocalRequestStatus::Claimed;
        buyer_request.updated_at += 100;

        seller_request.status = LocalRequestStatus::Claimed;
        seller_request.updated_at += 100;

        RequestCache::save(&buyer_request).expect("save buyer claimed");

        // -- Verify final state -------------------------------------------

        let loaded = RequestCache::load("lifecycle-1").expect("load final state");
        assert_eq!(loaded.status, LocalRequestStatus::Claimed);
        assert_eq!(loaded.price_usdc, 5_000_000);
        assert_eq!(format_price_usd(loaded.price_usdc), "$5.00");

        // -- Verify mailbox message round-trip ----------------------------

        // Buyer sends a message to the seller via sealed mailbox.
        let buyer_msg = MailboxMessage {
            sender: buyer_pk.clone(),
            timestamp: 1_700_000_000,
            message_type: "task-assignment".to_string(),
            payload: b"Please complete task XYZ".to_vec(),
        };

        let sealed = mailbox::seal_message(&seller_pk, &buyer_msg)
            .expect("sealing message to seller should succeed");

        // Verify sealed bytes differ from plaintext.
        let plaintext_json = serde_json::to_vec(&buyer_msg).unwrap();
        assert_ne!(sealed, plaintext_json, "sealed != plaintext");

        // Seller opens the message.
        let opened = mailbox::open_message(&seller_sk, &sealed)
            .expect("opening sealed message should succeed");

        assert_eq!(opened.sender, buyer_pk);
        assert_eq!(opened.message_type, "task-assignment");
        assert_eq!(opened.payload, b"Please complete task XYZ");
        assert_eq!(opened.timestamp, 1_700_000_000);
        assert_eq!(
            opened, buyer_msg,
            "roundtrip must preserve the full message"
        );

        // Verify that the buyer cannot open the message sealed for the seller.
        let wrong_key_result = mailbox::open_message(&buyer_sk, &sealed);
        assert!(
            wrong_key_result.is_err(),
            "buyer should not be able to open a message sealed for the seller"
        );

        // -- Verify terminal state rejects further transitions ------------

        assert!(
            !loaded.status.can_transition_to(&LocalRequestStatus::Open),
            "Claimed -> Open must be invalid"
        );
        assert!(
            !loaded
                .status
                .can_transition_to(&LocalRequestStatus::Responded),
            "Claimed -> Responded must be invalid"
        );
        assert!(
            !loaded
                .status
                .can_transition_to(&LocalRequestStatus::Validated),
            "Claimed -> Validated must be invalid"
        );
        assert!(
            !loaded
                .status
                .can_transition_to(&LocalRequestStatus::Expired),
            "Claimed -> Expired must be invalid"
        );
    });
}

// ===========================================================================
// Test 3: Encrypted deliverable flow (full ECIES + hash-lock)
// ===========================================================================

/// Simulates the full encrypted deliverable exchange:
///
///   1. Buyer encrypts a task description with ECIES to seller's pubkey
///   2. Seller decrypts, "processes" it, creates a deliverable
///   3. Seller encrypts deliverable with ECIES to buyer's pubkey
///   4. Seller generates secret S, computes keccak256(S)
///   5. After claim reveals S, buyer can verify the hash-lock
///
/// This tests the entire encryption / secret / hash-lock flow end-to-end.
#[tokio::test]
#[ignore]
async fn e2e_encrypted_deliverable_flow() {
    // -- Step 1: Generate buyer and seller keypairs -----------------------

    let (buyer_sk, buyer_pk, buyer_addr) = random_keypair();
    let (seller_sk, seller_pk, seller_addr) = random_keypair();

    assert_ne!(buyer_addr, seller_addr);

    // -- Step 2: Buyer creates and encrypts task description --------------

    let task_description = b"Implement a sorting algorithm in Rust with O(n log n) complexity";

    // Buyer encrypts the task for the seller.
    let encrypted_task = encryption::encrypt(&seller_pk, task_description)
        .expect("encrypting task for seller should succeed");

    // Encrypted output must differ from plaintext.
    assert_ne!(
        encrypted_task.as_slice(),
        task_description.as_slice(),
        "ciphertext must differ from plaintext"
    );

    // Encrypted output must be larger (includes ephemeral pubkey + auth tag).
    assert!(
        encrypted_task.len() > task_description.len(),
        "ciphertext must be longer than plaintext"
    );

    // Buyer cannot decrypt their own message to the seller (wrong key).
    let buyer_decrypt_attempt = encryption::decrypt(&buyer_sk, &encrypted_task);
    assert!(
        buyer_decrypt_attempt.is_err(),
        "buyer must not be able to decrypt message encrypted for seller"
    );

    // -- Step 3: Seller decrypts and "processes" the task -----------------

    let decrypted_task = encryption::decrypt(&seller_sk, &encrypted_task)
        .expect("seller should be able to decrypt the task");

    assert_eq!(
        decrypted_task.as_slice(),
        task_description.as_slice(),
        "decrypted task must match original"
    );

    // Seller "processes" the task and creates a deliverable.
    let deliverable = format!(
        "DELIVERABLE: Merge sort implementation for task: {}",
        String::from_utf8_lossy(&decrypted_task)
    );
    let deliverable_bytes = deliverable.as_bytes();

    // -- Step 4: Seller encrypts deliverable for buyer --------------------

    let encrypted_deliverable = encryption::encrypt(&buyer_pk, deliverable_bytes)
        .expect("encrypting deliverable for buyer should succeed");

    assert_ne!(
        encrypted_deliverable.as_slice(),
        deliverable_bytes,
        "encrypted deliverable must differ from plaintext"
    );

    // Seller cannot decrypt their own message to the buyer.
    let seller_decrypt_attempt = encryption::decrypt(&seller_sk, &encrypted_deliverable);
    assert!(
        seller_decrypt_attempt.is_err(),
        "seller must not be able to decrypt message encrypted for buyer"
    );

    // -- Step 5: Seller generates secret S and computes hash-lock ---------

    let (secret_hex, hash_hex) = generate_secret();

    // Verify the hash-lock relationship.
    let secret_bytes = hex::decode(&secret_hex).expect("secret should be valid hex");
    assert_eq!(secret_bytes.len(), 32, "secret must be exactly 32 bytes");

    let computed_hash = keccak256(&secret_bytes);
    let computed_hash_hex = format!("0x{}", hex::encode(computed_hash));
    assert_eq!(
        hash_hex, computed_hash_hex,
        "hash must match keccak256(secret)"
    );

    // -- Step 6: Simulate on-chain claim revealing S ----------------------

    // After the claim transaction reveals S on-chain, anyone can verify:
    //   keccak256(S) == published_hash
    let revealed_secret_bytes = hex::decode(&secret_hex).expect("valid hex");
    let verification_hash = keccak256(&revealed_secret_bytes);
    let verification_hex = format!("0x{}", hex::encode(verification_hash));

    assert_eq!(
        verification_hex, hash_hex,
        "on-chain verification: keccak256(revealed_secret) == published_hash"
    );

    // -- Step 7: Buyer decrypts the deliverable ---------------------------

    let decrypted_deliverable = encryption::decrypt(&buyer_sk, &encrypted_deliverable)
        .expect("buyer should be able to decrypt the deliverable");

    assert_eq!(
        decrypted_deliverable.as_slice(),
        deliverable_bytes,
        "decrypted deliverable must match original"
    );

    // Verify the deliverable contains the expected content.
    let deliverable_str = String::from_utf8_lossy(&decrypted_deliverable);
    assert!(
        deliverable_str.contains("Merge sort implementation"),
        "deliverable should contain the implementation"
    );
    assert!(
        deliverable_str.contains("sorting algorithm"),
        "deliverable should reference the original task"
    );

    // -- Step 8: Test hex convenience wrappers ----------------------------

    let encrypted_hex = encryption::encrypt_hex(&seller_pk, b"hex test payload")
        .expect("encrypt_hex should succeed");

    // Must be valid hex.
    assert!(
        hex::decode(&encrypted_hex).is_ok(),
        "encrypt_hex output must be valid hex"
    );

    let decrypted_hex =
        encryption::decrypt_hex(&seller_sk, &encrypted_hex).expect("decrypt_hex should succeed");

    assert_eq!(
        decrypted_hex, b"hex test payload",
        "hex round-trip must recover original"
    );
}

// ===========================================================================
// Test 4: Multi-request reputation computation
// ===========================================================================

/// Create 5 requests with different outcomes:
///   - 3 Claimed (passed validation)
///   - 1 Expired (failed -- counts against reputation)
///   - 1 Cancelled (does not count in reputation calculation)
///
/// Reputation formula: completed / (completed + failed) * 100
///   = 3 / (3 + 1) * 100 = 75.0%
///
/// This exercises identity + requests + reputation + validation + formatting.
#[tokio::test]
#[ignore]
async fn e2e_multi_request_reputation() {
    with_temp_home(|| {
        // -- Identity setup -----------------------------------------------

        let (_sk, _pk, address) = random_keypair();

        // -- Create 5 requests with varied outcomes -----------------------

        let request_specs: Vec<(&str, LocalRequestStatus, u64)> = vec![
            ("rep-1", LocalRequestStatus::Claimed, 10_000_000), // $10
            ("rep-2", LocalRequestStatus::Claimed, 5_000_000),  // $5
            ("rep-3", LocalRequestStatus::Claimed, 15_000_000), // $15
            ("rep-4", LocalRequestStatus::Expired, 8_000_000),  // $8 (expired, no payout)
            ("rep-5", LocalRequestStatus::Cancelled, 3_000_000), // $3 (cancelled, not counted)
        ];

        for (id, status, price) in &request_specs {
            let mut request = make_request(
                id,
                LocalRequestStatus::Open,
                RequestRole::Seller,
                *price,
                &address,
            );

            // Walk through state transitions to reach the target status.
            match status {
                LocalRequestStatus::Claimed => {
                    request.status = LocalRequestStatus::Responded;
                    request.response_cid = Some(format!("QmResponse{}", id));
                    request.updated_at += 100;

                    request.status = LocalRequestStatus::Validated;
                    request.updated_at += 100;

                    let (secret, hash) = generate_secret();
                    request.secret = Some(secret);
                    request.secret_hash = Some(hash);

                    request.status = LocalRequestStatus::Claimed;
                    request.updated_at += 100;
                }
                LocalRequestStatus::Expired => {
                    request.status = LocalRequestStatus::Responded;
                    request.response_cid = Some(format!("QmResponse{}", id));
                    request.updated_at += 100;

                    // Expired after response (validation never happened or failed).
                    request.status = LocalRequestStatus::Expired;
                    request.updated_at += 5000;
                }
                LocalRequestStatus::Cancelled => {
                    // Cancelled from Open -- no validation occurs.
                    request.status = LocalRequestStatus::Cancelled;
                    request.updated_at += 50;
                }
                _ => unreachable!("test only uses Claimed, Expired, Cancelled"),
            }

            RequestCache::save(&request).expect("save request");
        }

        // -- Load all and verify counts -----------------------------------

        let all = RequestCache::load_all().expect("load_all");
        assert_eq!(all.len(), 5, "should have 5 total requests");

        let claimed_requests =
            RequestCache::load_by_status(LocalRequestStatus::Claimed).expect("filter claimed");
        assert_eq!(claimed_requests.len(), 3, "should have 3 claimed");

        let expired_requests =
            RequestCache::load_by_status(LocalRequestStatus::Expired).expect("filter expired");
        assert_eq!(expired_requests.len(), 1, "should have 1 expired");

        let cancelled_requests =
            RequestCache::load_by_status(LocalRequestStatus::Cancelled).expect("filter cancelled");
        assert_eq!(cancelled_requests.len(), 1, "should have 1 cancelled");

        // -- Derive validation records ------------------------------------
        // Claimed = passed validation, Expired = failed validation.
        // Cancelled is NOT included in reputation (no validation record).

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

        assert_eq!(
            records.len(),
            4,
            "should have 4 validation records (3 claimed + 1 expired)"
        );

        // -- Compute reputation -------------------------------------------

        // Only claimed requests earn revenue.
        let total_earnings: u64 = claimed_requests.iter().map(|r| r.price_usdc).sum();
        assert_eq!(
            total_earnings, 30_000_000,
            "total earnings = $10 + $5 + $15 = $30"
        );

        let score = compute_reputation(&address, &records, total_earnings, 120);

        // 3 passed / (3 passed + 1 failed) = 75.0%
        assert_eq!(score.completed_requests, 3, "3 completed requests");
        assert_eq!(score.failed_validations, 1, "1 failed validation");
        assert!(
            (score.score - 75.0).abs() < f64::EPSILON,
            "reputation score should be 75.0, got: {}",
            score.score
        );
        assert_eq!(
            score.total_earnings_usdc, 30_000_000,
            "total earnings = $30 USDC"
        );

        // -- Verify reputation formatting ---------------------------------

        assert_eq!(format_reputation(&score), "75.0");
        assert_eq!(reputation_tier(&score), "Fair"); // 75% >= 60, < 80

        // -- Verify earnings formatting -----------------------------------

        assert_eq!(format_earnings_usd(score.total_earnings_usdc), "$30.00");
        assert_eq!(format_price_usd(total_earnings), "$30.00");

        // Individual request prices.
        assert_eq!(format_price_usd(10_000_000), "$10.00");
        assert_eq!(format_price_usd(5_000_000), "$5.00");
        assert_eq!(format_price_usd(15_000_000), "$15.00");

        // -- Verify validation persistence --------------------------------

        // Create and save validation results for each record.
        for record in &records {
            let handler_output = HandlerOutput {
                score: if record.passed { 85 } else { 30 },
                reason: if record.passed {
                    "good work".to_string()
                } else {
                    "expired before completion".to_string()
                },
            };

            let result = validation::create_result(&record.request_id, &handler_output);
            assert_eq!(result.passed, record.passed);

            validation::save_result(&result).expect("save validation result");
        }

        let all_results = validation::load_all_results().expect("load all validation results");
        assert_eq!(
            all_results.len(),
            4,
            "should have 4 persisted validation results"
        );

        let passed_count = all_results.iter().filter(|r| r.passed).count();
        let failed_count = all_results.iter().filter(|r| !r.passed).count();
        assert_eq!(passed_count, 3, "3 passed validations");
        assert_eq!(failed_count, 1, "1 failed validation");

        // -- Cross-check: individual result load --------------------------

        for record in &records {
            let loaded =
                validation::load_result(&record.request_id).expect("load individual result");
            assert_eq!(loaded.passed, record.passed);
            assert_eq!(loaded.request_id, record.request_id);
        }
    });
}
