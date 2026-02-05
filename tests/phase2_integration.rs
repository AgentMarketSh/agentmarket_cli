//! Phase 2 cross-module integration tests.
//!
//! These tests verify that the Phase 2 modules (identity, keystore, encryption,
//! mailbox, chain types, config) work together correctly in realistic workflows.
//!
//! Tests that mutate environment variables must run with `--test-threads=1`.

use std::env;
use std::sync::Mutex;

use agentmarket::chain::types::Balance;
use agentmarket::config::keystore;
use agentmarket::config::store::{
    AgentConfig, Config, IdentityConfig, NetworkConfig, ServicesConfig,
};
use agentmarket::engine::identity;
use agentmarket::ipfs::encryption;
use agentmarket::ipfs::mailbox::{self, MailboxMessage};
use alloy::primitives::U256;

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

// ===========================================================================
// 1. Identity -> Keystore round-trip
// ===========================================================================

/// Generate a keypair, save to keystore, load from keystore, then derive the
/// address from the loaded key and verify it matches the original.
#[test]
fn identity_keystore_roundtrip() {
    with_temp_home(|| {
        let passphrase = "integration-test-passphrase";

        // Generate keypair via identity module.
        let (private_key, original_pubkey, original_address) =
            identity::generate_keypair().expect("generate_keypair failed");

        // Save to keystore.
        keystore::save_key(&private_key, passphrase).expect("save_key failed");

        // Verify the keystore file was created.
        assert!(keystore::exists().expect("keystore::exists failed"));

        // Load from keystore.
        let recovered_key = keystore::load_key(passphrase).expect("load_key failed");
        assert_eq!(
            recovered_key, private_key,
            "recovered key must match original"
        );

        // Derive address from recovered key and verify it matches.
        let (derived_pubkey, derived_address) =
            identity::address_from_key(&recovered_key).expect("address_from_key failed");

        assert_eq!(
            derived_pubkey, original_pubkey,
            "public key derived from recovered key must match original"
        );
        assert_eq!(
            derived_address, original_address,
            "address derived from recovered key must match original"
        );
    });
}

/// Saving a key and loading with the wrong passphrase should fail.
#[test]
fn identity_keystore_wrong_passphrase_fails() {
    with_temp_home(|| {
        let (private_key, _, _) = identity::generate_keypair().expect("generate_keypair failed");

        keystore::save_key(&private_key, "correct-passphrase").expect("save_key failed");

        let result = keystore::load_key("wrong-passphrase");
        assert!(result.is_err(), "loading with wrong passphrase must fail");
    });
}

/// Saving a key twice with different passphrases should overwrite; the second
/// passphrase should be the one that works.
#[test]
fn identity_keystore_overwrite() {
    with_temp_home(|| {
        let (private_key, _, _) = identity::generate_keypair().expect("generate_keypair failed");

        keystore::save_key(&private_key, "first-passphrase").expect("save_key (1st) failed");
        keystore::save_key(&private_key, "second-passphrase").expect("save_key (2nd) failed");

        // First passphrase should no longer work.
        let result1 = keystore::load_key("first-passphrase");
        assert!(
            result1.is_err(),
            "first passphrase should fail after overwrite"
        );

        // Second passphrase should work.
        let recovered = keystore::load_key("second-passphrase").expect("second passphrase failed");
        assert_eq!(recovered, private_key);
    });
}

// ===========================================================================
// 2. Identity -> ECIES encryption round-trip
// ===========================================================================

/// Generate a keypair with the identity module, encrypt a message with the
/// public key, decrypt with the private key, and verify the plaintext matches.
#[test]
fn identity_ecies_encryption_roundtrip() {
    let (private_key, public_key_hex, _address) =
        identity::generate_keypair().expect("generate_keypair failed");

    let plaintext = b"Hello from integration test!";

    let ciphertext =
        encryption::encrypt(&public_key_hex, plaintext).expect("encryption should succeed");

    // Ciphertext should differ from plaintext.
    assert_ne!(
        ciphertext.as_slice(),
        plaintext,
        "ciphertext must differ from plaintext"
    );

    let decrypted =
        encryption::decrypt(&private_key, &ciphertext).expect("decryption should succeed");

    assert_eq!(
        decrypted.as_slice(),
        plaintext,
        "decrypted message must match original plaintext"
    );
}

/// Encrypt with one identity's public key; decrypting with a different
/// identity's private key must fail.
#[test]
fn identity_ecies_cross_key_fails() {
    let (_sk_a, pk_a, _) = identity::generate_keypair().expect("keypair A");
    let (sk_b, _pk_b, _) = identity::generate_keypair().expect("keypair B");

    let ciphertext =
        encryption::encrypt(&pk_a, b"secret payload").expect("encryption should succeed");

    let result = encryption::decrypt(&sk_b, &ciphertext);
    assert!(
        result.is_err(),
        "decrypting with wrong private key must fail"
    );
}

/// Encrypt and decrypt a large message (1 MB) to verify there are no size
/// limitations in the cross-module path.
#[test]
fn identity_ecies_large_message() {
    let (private_key, public_key_hex, _) =
        identity::generate_keypair().expect("generate_keypair failed");

    let large_plaintext = vec![0xABu8; 1024 * 1024]; // 1 MB

    let ciphertext = encryption::encrypt(&public_key_hex, &large_plaintext)
        .expect("encrypting large message should succeed");

    let decrypted = encryption::decrypt(&private_key, &ciphertext)
        .expect("decrypting large message should succeed");

    assert_eq!(
        decrypted, large_plaintext,
        "large message roundtrip must be lossless"
    );
}

/// Encrypt and decrypt via hex convenience wrappers.
#[test]
fn identity_ecies_hex_roundtrip() {
    let (private_key, public_key_hex, _) =
        identity::generate_keypair().expect("generate_keypair failed");

    let plaintext = b"hex-wrapper integration test";

    let ciphertext_hex =
        encryption::encrypt_hex(&public_key_hex, plaintext).expect("encrypt_hex should succeed");

    let decrypted =
        encryption::decrypt_hex(&private_key, &ciphertext_hex).expect("decrypt_hex should succeed");

    assert_eq!(
        decrypted.as_slice(),
        plaintext,
        "hex roundtrip must preserve plaintext"
    );
}

// ===========================================================================
// 3. Mailbox seal/open with generated keypair
// ===========================================================================

/// Generate a keypair, create a MailboxMessage, seal it with the public key,
/// open it with the private key, and verify the message matches.
#[test]
fn mailbox_seal_open_with_generated_keypair() {
    let (private_key, public_key_hex, _address) =
        identity::generate_keypair().expect("generate_keypair failed");

    let message = MailboxMessage {
        sender: public_key_hex.clone(),
        timestamp: 1_700_000_000u64,
        message_type: "request".to_string(),
        payload: b"integration test payload".to_vec(),
    };

    let sealed =
        mailbox::seal_message(&public_key_hex, &message).expect("seal_message should succeed");

    // Sealed bytes should differ from JSON plaintext.
    let json = serde_json::to_vec(&message).unwrap();
    assert_ne!(
        sealed, json,
        "sealed message must differ from plaintext JSON"
    );

    let opened = mailbox::open_message(&private_key, &sealed).expect("open_message should succeed");

    assert_eq!(opened, message, "opened message must match original");
}

/// Seal a message for one agent and attempt to open with another agent's key.
#[test]
fn mailbox_seal_open_wrong_key_fails() {
    let (_sk_a, pk_a, _) = identity::generate_keypair().expect("keypair A");
    let (sk_b, _pk_b, _) = identity::generate_keypair().expect("keypair B");

    let message = MailboxMessage {
        sender: pk_a.clone(),
        timestamp: 1_700_000_000u64,
        message_type: "notification".to_string(),
        payload: b"wrong-key test".to_vec(),
    };

    let sealed = mailbox::seal_message(&pk_a, &message).expect("seal_message should succeed");

    let result = mailbox::open_message(&sk_b, &sealed);
    assert!(
        result.is_err(),
        "opening a sealed message with the wrong key must fail"
    );
}

/// Verify that mailbox topic derivation is consistent for identity-generated keys.
#[test]
fn mailbox_topic_consistent_with_identity_key() {
    let (_, public_key_hex, _) = identity::generate_keypair().expect("generate_keypair failed");

    let mailbox_a =
        mailbox::Mailbox::new(&public_key_hex).expect("mailbox creation should succeed");
    let mailbox_b =
        mailbox::Mailbox::new(&public_key_hex).expect("mailbox creation should succeed");

    assert_eq!(
        mailbox_a.topic(),
        mailbox_b.topic(),
        "same public key must yield the same topic"
    );
    assert_eq!(mailbox_a.public_key(), public_key_hex);

    // Topic should be 64 hex chars (keccak256 = 32 bytes).
    assert_eq!(mailbox_a.topic().len(), 64);
}

/// Different identity-generated keys must produce different mailbox topics.
#[test]
fn mailbox_topics_differ_across_identities() {
    let (_, pk_a, _) = identity::generate_keypair().expect("keypair A");
    let (_, pk_b, _) = identity::generate_keypair().expect("keypair B");

    let mb_a = mailbox::Mailbox::new(&pk_a).expect("mailbox A");
    let mb_b = mailbox::Mailbox::new(&pk_b).expect("mailbox B");

    assert_ne!(
        mb_a.topic(),
        mb_b.topic(),
        "different keys must produce different topics"
    );
}

/// Seal/open with a binary payload containing null bytes and unicode.
#[test]
fn mailbox_seal_open_binary_payload() {
    let (private_key, public_key_hex, _) =
        identity::generate_keypair().expect("generate_keypair failed");

    let binary_payload: Vec<u8> = (0..=255).collect();

    let message = MailboxMessage {
        sender: public_key_hex.clone(),
        timestamp: 42,
        message_type: "data-transfer".to_string(),
        payload: binary_payload.clone(),
    };

    let sealed = mailbox::seal_message(&public_key_hex, &message).expect("seal should succeed");
    let opened = mailbox::open_message(&private_key, &sealed).expect("open should succeed");

    assert_eq!(opened.payload, binary_payload);
    assert_eq!(opened.message_type, "data-transfer");
    assert_eq!(opened.timestamp, 42);
}

/// Seal/open with an empty payload.
#[test]
fn mailbox_seal_open_empty_payload() {
    let (private_key, public_key_hex, _) =
        identity::generate_keypair().expect("generate_keypair failed");

    let message = MailboxMessage {
        sender: public_key_hex.clone(),
        timestamp: 0,
        message_type: "ping".to_string(),
        payload: vec![],
    };

    let sealed = mailbox::seal_message(&public_key_hex, &message).expect("seal should succeed");
    let opened = mailbox::open_message(&private_key, &sealed).expect("open should succeed");

    assert_eq!(opened, message);
}

// ===========================================================================
// 4. Balance check logic
// ===========================================================================

/// Minimum registration threshold constant (0.0001 ETH = 1e14 wei).
const REGISTRATION_MIN_WEI: u128 = 100_000_000_000_000;

#[test]
fn balance_zero_is_insufficient() {
    let balance = Balance { wei: U256::ZERO };
    assert!(!balance.is_sufficient_for_registration());
    assert_eq!(balance.display_eth(), "0.0000 ETH");
}

#[test]
fn balance_one_wei_below_threshold() {
    let balance = Balance {
        wei: U256::from(REGISTRATION_MIN_WEI - 1),
    };
    assert!(!balance.is_sufficient_for_registration());
}

#[test]
fn balance_exactly_at_threshold() {
    let balance = Balance {
        wei: U256::from(REGISTRATION_MIN_WEI),
    };
    assert!(balance.is_sufficient_for_registration());
    assert_eq!(balance.display_eth(), "0.0001 ETH");
}

#[test]
fn balance_one_wei_above_threshold() {
    let balance = Balance {
        wei: U256::from(REGISTRATION_MIN_WEI + 1),
    };
    assert!(balance.is_sufficient_for_registration());
}

#[test]
fn balance_one_eth() {
    let one_eth: u128 = 1_000_000_000_000_000_000;
    let balance = Balance {
        wei: U256::from(one_eth),
    };
    assert!(balance.is_sufficient_for_registration());
    assert_eq!(balance.display_eth(), "1.0000 ETH");
}

#[test]
fn balance_fractional_display() {
    // 0.5 ETH
    let balance = Balance {
        wei: U256::from(500_000_000_000_000_000u128),
    };
    assert_eq!(balance.display_eth(), "0.5000 ETH");
}

#[test]
fn balance_large_value() {
    // 100 ETH
    let balance = Balance {
        wei: U256::from(100_000_000_000_000_000_000u128),
    };
    assert!(balance.is_sufficient_for_registration());
    assert_eq!(balance.display_eth(), "100.0000 ETH");
}

#[test]
fn balance_display_quarter_eth() {
    // 0.25 ETH = 250_000_000_000_000_000 wei
    let balance = Balance {
        wei: U256::from(250_000_000_000_000_000u128),
    };
    assert_eq!(balance.display_eth(), "0.2500 ETH");
}

// ===========================================================================
// 5. Config -> Identity state transitions
// ===========================================================================

#[test]
fn config_empty_identity_is_uninitialized() {
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

    let state = identity::get_identity_state(&config);
    assert_eq!(state, identity::IdentityState::Uninitialized);
}

#[test]
fn config_with_pubkey_only_is_local() {
    let (_, public_key_hex, _) = identity::generate_keypair().expect("generate_keypair failed");

    let config = Config {
        agent: AgentConfig::default(),
        network: NetworkConfig::default(),
        identity: IdentityConfig {
            public_key: public_key_hex.clone(),
            agent_id: String::new(),
            ipfs_profile_cid: String::new(),
        },
        services: ServicesConfig::default(),
    };

    let state = identity::get_identity_state(&config);
    assert_eq!(
        state,
        identity::IdentityState::Local {
            address: String::new(),
            public_key: public_key_hex,
        }
    );
}

#[test]
fn config_with_pubkey_and_agent_id_is_registered() {
    let (_, public_key_hex, _) = identity::generate_keypair().expect("generate_keypair failed");

    let config = Config {
        agent: AgentConfig::default(),
        network: NetworkConfig::default(),
        identity: IdentityConfig {
            public_key: public_key_hex.clone(),
            agent_id: "agent-99".to_string(),
            ipfs_profile_cid: "QmTestCid".to_string(),
        },
        services: ServicesConfig::default(),
    };

    let state = identity::get_identity_state(&config);
    assert_eq!(
        state,
        identity::IdentityState::Registered {
            address: String::new(),
            public_key: public_key_hex,
            agent_id: "agent-99".to_string(),
        }
    );
}

/// Verify full lifecycle: Uninitialized -> Local -> Registered by progressively
/// filling in the config.
#[test]
fn config_identity_lifecycle_transitions() {
    let (_, public_key_hex, _) = identity::generate_keypair().expect("generate_keypair failed");

    // Phase 1: Uninitialized.
    let mut config = Config::default();
    assert_eq!(
        identity::get_identity_state(&config),
        identity::IdentityState::Uninitialized
    );

    // Phase 2: After init, set public_key -> Local.
    config.identity.public_key = public_key_hex.clone();
    assert_eq!(
        identity::get_identity_state(&config),
        identity::IdentityState::Local {
            address: String::new(),
            public_key: public_key_hex.clone(),
        }
    );

    // Phase 3: After register, set agent_id -> Registered.
    config.identity.agent_id = "agent-42".to_string();
    config.identity.ipfs_profile_cid = "QmSomeCid".to_string();
    assert_eq!(
        identity::get_identity_state(&config),
        identity::IdentityState::Registered {
            address: String::new(),
            public_key: public_key_hex,
            agent_id: "agent-42".to_string(),
        }
    );
}

/// Verify that config save/load preserves identity state across disk round-trip.
#[test]
fn config_identity_state_persists_through_save_load() {
    with_temp_home(|| {
        // Clear env overrides so they don't interfere with config load.
        env::remove_var("AGENTMARKET_RPC_URL");
        env::remove_var("AGENTMARKET_IPFS_API");
        env::remove_var("AGENTMARKET_IPFS_GATEWAY");

        let (_, public_key_hex, _) = identity::generate_keypair().expect("generate_keypair failed");

        let mut config = Config::default();
        config.identity.public_key = public_key_hex.clone();
        config.identity.agent_id = "agent-7".to_string();

        agentmarket::config::store::save(&config).expect("save failed");
        let loaded = agentmarket::config::store::load().expect("load failed");

        let state = identity::get_identity_state(&loaded);
        assert_eq!(
            state,
            identity::IdentityState::Registered {
                address: String::new(),
                public_key: public_key_hex,
                agent_id: "agent-7".to_string(),
            }
        );
    });
}

// ===========================================================================
// 6. Full integration: identity -> keystore -> encryption -> mailbox
// ===========================================================================

/// End-to-end: generate identity, persist key in keystore, recover key,
/// seal a mailbox message with the identity's public key, decrypt with the
/// recovered private key.
#[test]
fn full_identity_keystore_mailbox_flow() {
    with_temp_home(|| {
        let passphrase = "e2e-passphrase";

        // Step 1: Generate identity.
        let (private_key, public_key_hex, address) =
            identity::generate_keypair().expect("generate_keypair failed");

        // Step 2: Persist private key.
        keystore::save_key(&private_key, passphrase).expect("save_key failed");

        // Step 3: Recover private key from keystore.
        let recovered = keystore::load_key(passphrase).expect("load_key failed");

        // Step 4: Verify identity round-trip.
        let (derived_pk, derived_addr) =
            identity::address_from_key(&recovered).expect("address_from_key failed");
        assert_eq!(derived_pk, public_key_hex);
        assert_eq!(derived_addr, address);

        // Step 5: Create and seal a mailbox message using the public key.
        let message = MailboxMessage {
            sender: public_key_hex.clone(),
            timestamp: 1_700_001_000u64,
            message_type: "response".to_string(),
            payload: b"full integration test deliverable".to_vec(),
        };

        let sealed = mailbox::seal_message(&public_key_hex, &message).expect("seal_message failed");

        // Step 6: Open the message with the recovered private key.
        let opened = mailbox::open_message(&recovered, &sealed).expect("open_message failed");

        assert_eq!(opened, message, "full flow message round-trip must match");
    });
}
