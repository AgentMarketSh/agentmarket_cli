//! Phase 1 cross-module integration tests.
//!
//! These tests verify that the Phase 1 modules (config store, encrypted keystore,
//! identity engine) work together correctly in realistic workflows and edge cases
//! not covered by unit tests or the Phase 2 integration suite.
//!
//! Tests that mutate environment variables must run with `--test-threads=1`.

use std::env;
use std::sync::Mutex;

use agentmarket::config::keystore;
use agentmarket::config::store::{self, Config};
use agentmarket::engine::identity;

/// Mutex to serialise tests that mutate environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Helper: create a temporary directory, point `AGENTMARKET_HOME` at it,
/// run the closure, then restore the previous value.
fn with_temp_home<F: FnOnce()>(f: F) {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");

    let tmp = tempfile::tempdir().expect("failed to create temp dir");
    let prev = env::var("AGENTMARKET_HOME").ok();

    env::set_var("AGENTMARKET_HOME", tmp.path());

    // Clear env overrides so they don't interfere with config round-trips.
    let prev_rpc = env::var("AGENTMARKET_RPC_URL").ok();
    let prev_ipfs_api = env::var("AGENTMARKET_IPFS_API").ok();
    let prev_ipfs_gw = env::var("AGENTMARKET_IPFS_GATEWAY").ok();
    env::remove_var("AGENTMARKET_RPC_URL");
    env::remove_var("AGENTMARKET_IPFS_API");
    env::remove_var("AGENTMARKET_IPFS_GATEWAY");

    f();

    // Restore all env vars.
    match prev_rpc {
        Some(v) => env::set_var("AGENTMARKET_RPC_URL", v),
        None => env::remove_var("AGENTMARKET_RPC_URL"),
    }
    match prev_ipfs_api {
        Some(v) => env::set_var("AGENTMARKET_IPFS_API", v),
        None => env::remove_var("AGENTMARKET_IPFS_API"),
    }
    match prev_ipfs_gw {
        Some(v) => env::set_var("AGENTMARKET_IPFS_GATEWAY", v),
        None => env::remove_var("AGENTMARKET_IPFS_GATEWAY"),
    }
    match prev {
        Some(v) => env::set_var("AGENTMARKET_HOME", v),
        None => env::remove_var("AGENTMARKET_HOME"),
    }
}

// ===========================================================================
// 1. Config store edge cases
// ===========================================================================

/// Unicode agent name and description survive a save/load round-trip.
#[test]
fn config_unicode_agent_name_and_description() {
    with_temp_home(|| {
        let mut cfg = Config::default();
        cfg.agent.name = "\u{1F916} Agent M\u{00FC}ller".to_string();
        cfg.agent.description =
            "\u{4E16}\u{754C}\u{4F60}\u{597D} \u{2014} \u{00E9}l\u{00E8}ve sp\u{00E9}cial"
                .to_string();

        store::save(&cfg).expect("save failed");
        let loaded = store::load().expect("load failed");

        assert_eq!(loaded.agent.name, cfg.agent.name);
        assert_eq!(loaded.agent.description, cfg.agent.description);
    });
}

/// Empty capabilities list round-trips correctly.
#[test]
fn config_empty_capabilities_roundtrip() {
    with_temp_home(|| {
        let mut cfg = Config::default();
        cfg.services.capabilities = vec![];

        store::save(&cfg).expect("save failed");
        let loaded = store::load().expect("load failed");

        assert!(
            loaded.services.capabilities.is_empty(),
            "empty capabilities list should persist"
        );
    });
}

/// Populated capabilities list round-trips correctly.
#[test]
fn config_populated_capabilities_roundtrip() {
    with_temp_home(|| {
        let mut cfg = Config::default();
        cfg.services.capabilities = vec![
            "code-review".to_string(),
            "testing".to_string(),
            "data-analysis".to_string(),
            "natural-language-processing".to_string(),
        ];

        store::save(&cfg).expect("save failed");
        let loaded = store::load().expect("load failed");

        assert_eq!(loaded.services.capabilities.len(), 4);
        assert_eq!(loaded.services.capabilities[0], "code-review");
        assert_eq!(
            loaded.services.capabilities[3],
            "natural-language-processing"
        );
    });
}

/// Very large pricing value round-trips without loss.
#[test]
fn config_large_pricing_usd_roundtrip() {
    with_temp_home(|| {
        let mut cfg = Config::default();
        cfg.services.pricing_usd = 999_999.99;

        store::save(&cfg).expect("save failed");
        let loaded = store::load().expect("load failed");

        assert!(
            (loaded.services.pricing_usd - 999_999.99).abs() < f64::EPSILON,
            "large pricing_usd should survive round-trip, got {}",
            loaded.services.pricing_usd
        );
    });
}

/// AGENTMARKET_RPC_URL env var overrides the file value on load.
#[test]
fn config_env_override_rpc_url_takes_precedence() {
    with_temp_home(|| {
        let cfg = Config::default();
        store::save(&cfg).expect("save failed");

        // Set an env override AFTER saving the config with defaults.
        env::set_var("AGENTMARKET_RPC_URL", "https://overridden-rpc.example.com");

        let loaded = store::load().expect("load failed");
        assert_eq!(
            loaded.network.chain_rpc, "https://overridden-rpc.example.com",
            "env var should override file value"
        );

        // The file-level values for the other fields should remain at defaults.
        assert_eq!(loaded.network.ipfs_api, "http://localhost:5001");
        assert_eq!(loaded.network.ipfs_gateway, "https://gateway.pinata.cloud");

        // Clean up for the with_temp_home restore logic.
        env::remove_var("AGENTMARKET_RPC_URL");
    });
}

/// Multiple save/load cycles preserve data integrity.
#[test]
fn config_multiple_save_load_cycles_preserve_data() {
    with_temp_home(|| {
        // Cycle 1: save and load default + name.
        let mut cfg = Config::default();
        cfg.agent.name = "cycle-1-name".to_string();
        store::save(&cfg).expect("save cycle 1 failed");

        let mut loaded = store::load().expect("load cycle 1 failed");
        assert_eq!(loaded.agent.name, "cycle-1-name");

        // Cycle 2: modify loaded config, save again, reload.
        loaded.agent.description = "cycle-2-description".to_string();
        loaded.services.pricing_usd = 42.5;
        store::save(&loaded).expect("save cycle 2 failed");

        let mut loaded2 = store::load().expect("load cycle 2 failed");
        assert_eq!(loaded2.agent.name, "cycle-1-name");
        assert_eq!(loaded2.agent.description, "cycle-2-description");
        assert!((loaded2.services.pricing_usd - 42.5).abs() < f64::EPSILON);

        // Cycle 3: add capabilities, save, reload.
        loaded2.services.capabilities = vec!["cap-a".to_string(), "cap-b".to_string()];
        loaded2.identity.public_key = "02deadbeef".to_string();
        store::save(&loaded2).expect("save cycle 3 failed");

        let loaded3 = store::load().expect("load cycle 3 failed");
        assert_eq!(loaded3.agent.name, "cycle-1-name");
        assert_eq!(loaded3.agent.description, "cycle-2-description");
        assert!((loaded3.services.pricing_usd - 42.5).abs() < f64::EPSILON);
        assert_eq!(loaded3.services.capabilities, vec!["cap-a", "cap-b"]);
        assert_eq!(loaded3.identity.public_key, "02deadbeef");
    });
}

// ===========================================================================
// 2. Keystore stress
// ===========================================================================

/// Store multiple keys sequentially (overwrite). Only the last key/passphrase
/// combination should be recoverable.
#[test]
fn keystore_sequential_overwrite_keeps_only_last() {
    with_temp_home(|| {
        let (key_a, _, _) = identity::generate_keypair().expect("keypair a");
        let (key_b, _, _) = identity::generate_keypair().expect("keypair b");
        let (key_c, _, _) = identity::generate_keypair().expect("keypair c");

        keystore::save_key(&key_a, "pass-a").expect("save a");
        keystore::save_key(&key_b, "pass-b").expect("save b");
        keystore::save_key(&key_c, "pass-c").expect("save c");

        // Only the last key with its passphrase should be recoverable.
        let recovered = keystore::load_key("pass-c").expect("load with pass-c");
        assert_eq!(recovered, key_c, "should recover the last saved key");

        // Previous passphrases should fail.
        assert!(
            keystore::load_key("pass-a").is_err(),
            "old passphrase a should not work"
        );
        assert!(
            keystore::load_key("pass-b").is_err(),
            "old passphrase b should not work"
        );
    });
}

/// Passphrase containing unicode characters works correctly.
#[test]
fn keystore_unicode_passphrase() {
    with_temp_home(|| {
        let (key, _, _) = identity::generate_keypair().expect("generate_keypair");
        let passphrase = "\u{1F512}\u{00FC}ber-s\u{00E9}cr\u{00E8}t \u{4E16}\u{754C}!";

        keystore::save_key(&key, passphrase).expect("save with unicode passphrase");
        let recovered = keystore::load_key(passphrase).expect("load with unicode passphrase");
        assert_eq!(recovered, key, "unicode passphrase round-trip must succeed");
    });
}

/// Passphrase with spaces works correctly.
#[test]
fn keystore_passphrase_with_spaces() {
    with_temp_home(|| {
        let (key, _, _) = identity::generate_keypair().expect("generate_keypair");
        let passphrase = "  multiple   spaces   in passphrase  ";

        keystore::save_key(&key, passphrase).expect("save with spaces passphrase");
        let recovered = keystore::load_key(passphrase).expect("load with spaces passphrase");
        assert_eq!(recovered, key, "spaces passphrase round-trip must succeed");
    });
}

/// Keystore exists() check transitions from false to true after save.
#[test]
fn keystore_exists_before_and_after_save() {
    with_temp_home(|| {
        assert!(
            !keystore::exists().expect("exists check before save"),
            "keystore should not exist before any save"
        );

        let (key, _, _) = identity::generate_keypair().expect("generate_keypair");
        keystore::save_key(&key, "pass").expect("save_key");

        assert!(
            keystore::exists().expect("exists check after save"),
            "keystore should exist after save"
        );
    });
}

// ===========================================================================
// 3. Identity + Config integration
// ===========================================================================

/// Generate identity, save public key to config, load config, verify identity
/// state is Local.
#[test]
fn identity_generate_save_config_yields_local_state() {
    with_temp_home(|| {
        let (_, public_key_hex, _) = identity::generate_keypair().expect("generate_keypair");

        let mut config = Config::default();
        config.identity.public_key = public_key_hex.clone();

        store::save(&config).expect("save config");
        let loaded = store::load().expect("load config");

        let state = identity::get_identity_state(&loaded);
        assert_eq!(
            state,
            identity::IdentityState::Local {
                address: String::new(),
                public_key: public_key_hex,
            },
            "config with only public_key should be Local"
        );
    });
}

/// Generate identity, save config with agent_id, verify state is Registered.
#[test]
fn identity_config_with_agent_id_yields_registered_state() {
    with_temp_home(|| {
        let (_, public_key_hex, _) = identity::generate_keypair().expect("generate_keypair");

        let mut config = Config::default();
        config.identity.public_key = public_key_hex.clone();
        config.identity.agent_id = "agent-integration-test-42".to_string();

        store::save(&config).expect("save config");
        let loaded = store::load().expect("load config");

        let state = identity::get_identity_state(&loaded);
        assert_eq!(
            state,
            identity::IdentityState::Registered {
                address: String::new(),
                public_key: public_key_hex,
                agent_id: "agent-integration-test-42".to_string(),
            },
            "config with public_key + agent_id should be Registered"
        );
    });
}

/// Profile creation + save + load round-trip verifies all fields match config
/// values.
#[test]
fn profile_creation_matches_config_values() {
    with_temp_home(|| {
        let (_, public_key_hex, address) = identity::generate_keypair().expect("generate_keypair");

        // Set up config with meaningful values.
        let mut config = Config::default();
        config.agent.name = "Profile Test Agent".to_string();
        config.agent.description = "Integration test for profile <-> config".to_string();
        config.services.capabilities = vec!["analysis".to_string(), "summarization".to_string()];
        config.services.pricing_usd = 12.75;
        config.identity.public_key = public_key_hex.clone();

        store::save(&config).expect("save config");

        // Create profile from config values.
        let profile = identity::create_profile(
            &config.agent.name,
            &config.agent.description,
            config.services.capabilities.clone(),
            config.services.pricing_usd,
            &public_key_hex,
            &address,
        );

        // Save and reload profile.
        identity::save_profile(&profile).expect("save_profile");
        let loaded_profile = identity::load_profile().expect("load_profile");

        // Reload config too.
        let loaded_config = store::load().expect("load config");

        // Verify profile fields match config values.
        assert_eq!(loaded_profile.name, loaded_config.agent.name);
        assert_eq!(loaded_profile.description, loaded_config.agent.description);
        assert_eq!(
            loaded_profile.capabilities,
            loaded_config.services.capabilities
        );
        assert!(
            (loaded_profile.pricing_usd - loaded_config.services.pricing_usd).abs() < f64::EPSILON
        );
        assert_eq!(loaded_profile.public_key, loaded_config.identity.public_key);
        assert_eq!(loaded_profile.address, address);
        assert_eq!(loaded_profile.version, "0.1.0");
    });
}

// ===========================================================================
// 4. Init flow simulation (without interactive prompts)
// ===========================================================================

/// Simulate the full init flow: generate keypair, save keystore, create config,
/// create profile, then load everything back and verify consistency.
#[test]
fn init_flow_full_simulation() {
    with_temp_home(|| {
        let passphrase = "init-flow-test-passphrase";

        // Step 1: Verify clean state.
        assert!(!store::exists().expect("config exists before init"));
        assert!(!keystore::exists().expect("keystore exists before init"));

        // Step 2: Generate keypair.
        let (private_key, public_key_hex, address) =
            identity::generate_keypair().expect("generate_keypair");

        // Step 3: Save private key to keystore.
        keystore::save_key(&private_key, passphrase).expect("save_key");
        assert!(keystore::exists().expect("keystore should exist after save"));

        // Step 4: Build and save config.
        let mut config = Config::default();
        config.agent.name = "Init Flow Agent".to_string();
        config.agent.description = "Testing full init flow".to_string();
        config.services.capabilities = vec!["integration-testing".to_string()];
        config.services.pricing_usd = 7.50;
        config.identity.public_key = public_key_hex.clone();

        store::save(&config).expect("save config");
        assert!(store::exists().expect("config should exist after save"));

        // Step 5: Create and save profile.
        let profile = identity::create_profile(
            &config.agent.name,
            &config.agent.description,
            config.services.capabilities.clone(),
            config.services.pricing_usd,
            &public_key_hex,
            &address,
        );
        identity::save_profile(&profile).expect("save_profile");

        // Step 6: Load everything back and verify.

        // 6a: Keystore round-trip.
        let recovered_key = keystore::load_key(passphrase).expect("load_key");
        assert_eq!(recovered_key, private_key, "keystore key must match");

        // 6b: Verify address derivation from recovered key.
        let (derived_pubkey, derived_address) =
            identity::address_from_key(&recovered_key).expect("address_from_key");
        assert_eq!(derived_pubkey, public_key_hex);
        assert_eq!(derived_address, address);

        // 6c: Config round-trip.
        let loaded_config = store::load().expect("load config");
        assert_eq!(loaded_config.agent.name, "Init Flow Agent");
        assert_eq!(loaded_config.agent.description, "Testing full init flow");
        assert_eq!(
            loaded_config.services.capabilities,
            vec!["integration-testing"]
        );
        assert!((loaded_config.services.pricing_usd - 7.50).abs() < f64::EPSILON);
        assert_eq!(loaded_config.identity.public_key, public_key_hex);

        // 6d: Identity state should be Local (no agent_id yet).
        let state = identity::get_identity_state(&loaded_config);
        assert_eq!(
            state,
            identity::IdentityState::Local {
                address: String::new(),
                public_key: public_key_hex.clone(),
            }
        );

        // 6e: Profile round-trip.
        let loaded_profile = identity::load_profile().expect("load_profile");
        assert_eq!(loaded_profile.name, loaded_config.agent.name);
        assert_eq!(loaded_profile.description, loaded_config.agent.description);
        assert_eq!(
            loaded_profile.capabilities,
            loaded_config.services.capabilities
        );
        assert_eq!(loaded_profile.public_key, public_key_hex);
        assert_eq!(loaded_profile.address, address);
    });
}

/// Double-init guard: once config exists, exists() returns true.
#[test]
fn double_init_guard_config_exists() {
    with_temp_home(|| {
        assert!(
            !store::exists().expect("exists before init"),
            "config should not exist in fresh temp dir"
        );

        // First init: create and save config.
        let config = Config::default();
        store::save(&config).expect("save config");

        assert!(
            store::exists().expect("exists after first init"),
            "config should exist after first save"
        );

        // Second call still returns true.
        assert!(
            store::exists().expect("exists on second check"),
            "config should still exist on repeated check"
        );
    });
}

/// Double-init guard for keystore: exists() transitions correctly.
#[test]
fn double_init_guard_keystore_exists() {
    with_temp_home(|| {
        assert!(
            !keystore::exists().expect("keystore exists before init"),
            "keystore should not exist in fresh temp dir"
        );

        let (key, _, _) = identity::generate_keypair().expect("generate_keypair");
        keystore::save_key(&key, "pass1").expect("first save");

        assert!(
            keystore::exists().expect("keystore exists after first save"),
            "keystore should exist after first save"
        );

        // Overwrite with second key.
        let (key2, _, _) = identity::generate_keypair().expect("generate_keypair 2");
        keystore::save_key(&key2, "pass2").expect("second save");

        assert!(
            keystore::exists().expect("keystore exists after overwrite"),
            "keystore should still exist after overwrite"
        );
    });
}

/// Simulate init then register transition: Local -> Registered via config update.
#[test]
fn init_then_register_state_transition() {
    with_temp_home(|| {
        let passphrase = "transition-test";
        let (private_key, public_key_hex, address) =
            identity::generate_keypair().expect("generate_keypair");

        // Init: save keystore + config with public_key only.
        keystore::save_key(&private_key, passphrase).expect("save_key");

        let mut config = Config::default();
        config.agent.name = "Transition Agent".to_string();
        config.identity.public_key = public_key_hex.clone();
        store::save(&config).expect("save config (init)");

        // Verify Local state.
        let loaded = store::load().expect("load after init");
        assert_eq!(
            identity::get_identity_state(&loaded),
            identity::IdentityState::Local {
                address: String::new(),
                public_key: public_key_hex.clone(),
            }
        );

        // Simulate register: update config with agent_id and profile CID.
        let mut updated = loaded;
        updated.identity.agent_id = "agent-8004-token-id".to_string();
        updated.identity.ipfs_profile_cid = "QmSimulatedProfileCid".to_string();
        store::save(&updated).expect("save config (register)");

        // Verify Registered state persists through save/load.
        let final_config = store::load().expect("load after register");
        assert_eq!(
            identity::get_identity_state(&final_config),
            identity::IdentityState::Registered {
                address: String::new(),
                public_key: public_key_hex.clone(),
                agent_id: "agent-8004-token-id".to_string(),
            }
        );

        // All other fields should still be intact.
        assert_eq!(final_config.agent.name, "Transition Agent");
        assert_eq!(
            final_config.identity.ipfs_profile_cid,
            "QmSimulatedProfileCid"
        );

        // Keystore should still work.
        let recovered = keystore::load_key(passphrase).expect("load_key after register");
        let (derived_pk, derived_addr) =
            identity::address_from_key(&recovered).expect("address_from_key");
        assert_eq!(derived_pk, public_key_hex);
        assert_eq!(derived_addr, address);
    });
}
