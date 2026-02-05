//! Encrypted private key storage using Argon2id KDF and AES-256-GCM.
//!
//! The keystore persists an agent's private key in `~/.agentmarket/keystore.enc`
//! as a JSON file containing the salt, nonce, and ciphertext.  The encryption key
//! is derived from a user-supplied passphrase via Argon2id with hardened
//! parameters (64 MB memory, 3 iterations).

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{bail, Context, Result};
use argon2::Argon2;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::store::config_dir;

/// Current keystore format version.
const KEYSTORE_VERSION: u32 = 1;

/// Filename for the encrypted keystore within the config directory.
const KEYSTORE_FILENAME: &str = "keystore.enc";

/// Argon2id parameters.
const ARGON2_MEMORY_KIB: u32 = 64 * 1024; // 64 MB
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 1;

/// Salt length in bytes.
const SALT_LEN: usize = 16;

/// AES-256-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;

/// AES-256 key length in bytes.
const KEY_LEN: usize = 32;

/// On-disk representation of the encrypted keystore.
#[derive(Serialize, Deserialize)]
struct KeystoreFile {
    version: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// Returns the path to the keystore file.
fn keystore_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(KEYSTORE_FILENAME))
}

/// Derives a 256-bit encryption key from a passphrase and salt using Argon2id.
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    let params = argon2::Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(KEY_LEN),
    )
    .map_err(|e| anyhow::anyhow!("failed to build Argon2id parameters: {}", e))?;

    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("Argon2id key derivation failed: {}", e))?;

    Ok(key)
}

/// Encrypts private key bytes and writes the keystore file to disk.
///
/// The encryption key is derived from `passphrase` using Argon2id with a random
/// 16-byte salt.  The key bytes are then encrypted with AES-256-GCM using a
/// random 12-byte nonce.  The resulting JSON file is written with `0600`
/// permissions (owner read/write only).
pub fn save_key(key_bytes: &[u8], passphrase: &str) -> Result<()> {
    debug!("generating keystore salt and nonce");

    // Generate random salt and nonce.
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut salt);
    rng.fill_bytes(&mut nonce_bytes);

    // Derive encryption key from passphrase.
    debug!("deriving encryption key via Argon2id");
    let derived_key = derive_key(passphrase, &salt)?;

    // Encrypt.
    let cipher = Aes256Gcm::new_from_slice(&derived_key)
        .map_err(|e| anyhow::anyhow!("failed to create AES-256-GCM cipher: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, key_bytes)
        .map_err(|e| anyhow::anyhow!("AES-256-GCM encryption failed: {}", e))?;

    // Build the on-disk structure.
    let keystore = KeystoreFile {
        version: KEYSTORE_VERSION,
        salt: hex::encode(salt),
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
    };

    let json =
        serde_json::to_string_pretty(&keystore).context("failed to serialize keystore JSON")?;

    // Write file.
    let path = keystore_path()?;
    debug!("writing keystore to {}", path.display());
    fs::write(&path, json)
        .with_context(|| format!("failed to write keystore file: {}", path.display()))?;

    // Set file permissions to 0600 (owner read/write only).
    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms).with_context(|| {
            format!(
                "failed to set permissions on keystore file: {}",
                path.display()
            )
        })?;
    }

    debug!("keystore saved successfully");
    Ok(())
}

/// Reads the keystore file and decrypts the private key bytes.
///
/// The passphrase is used together with the stored salt to re-derive the
/// encryption key via Argon2id, which is then used to decrypt the ciphertext
/// with AES-256-GCM.
pub fn load_key(passphrase: &str) -> Result<Vec<u8>> {
    let path = keystore_path()?;
    debug!("loading keystore from {}", path.display());

    let json = fs::read_to_string(&path)
        .with_context(|| format!("failed to read keystore file: {}", path.display()))?;

    let keystore: KeystoreFile =
        serde_json::from_str(&json).context("failed to parse keystore JSON")?;

    if keystore.version != KEYSTORE_VERSION {
        bail!(
            "unsupported keystore version {} (expected {})",
            keystore.version,
            KEYSTORE_VERSION
        );
    }

    let salt = hex::decode(&keystore.salt).context("invalid hex in keystore salt")?;
    let nonce_bytes = hex::decode(&keystore.nonce).context("invalid hex in keystore nonce")?;
    let ciphertext =
        hex::decode(&keystore.ciphertext).context("invalid hex in keystore ciphertext")?;

    if salt.len() != SALT_LEN {
        bail!(
            "invalid salt length: expected {} bytes, got {}",
            SALT_LEN,
            salt.len()
        );
    }
    if nonce_bytes.len() != NONCE_LEN {
        bail!(
            "invalid nonce length: expected {} bytes, got {}",
            NONCE_LEN,
            nonce_bytes.len()
        );
    }

    // Derive key from passphrase + stored salt.
    debug!("deriving decryption key via Argon2id");
    let derived_key = derive_key(passphrase, &salt)?;

    // Decrypt.
    let cipher = Aes256Gcm::new_from_slice(&derived_key)
        .map_err(|e| anyhow::anyhow!("failed to create AES-256-GCM cipher: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref()).map_err(|_| {
        anyhow::anyhow!("decryption failed — wrong passphrase or corrupted keystore")
    })?;

    debug!("keystore decrypted successfully");
    Ok(plaintext)
}

/// Returns the passphrase for keystore operations.
///
/// Resolution order:
/// 1. `AGENTMARKET_KEYSTORE_PASSPHRASE` environment variable
/// 2. Interactive prompt via hidden stdin input
pub fn get_passphrase() -> Result<String> {
    if let Ok(passphrase) = std::env::var("AGENTMARKET_KEYSTORE_PASSPHRASE") {
        debug!("using passphrase from AGENTMARKET_KEYSTORE_PASSPHRASE env var");
        return Ok(passphrase);
    }

    debug!("prompting for passphrase via stdin");
    let passphrase = rpassword::prompt_password_stdout("Enter passphrase: ")
        .context("failed to read passphrase")?;

    Ok(passphrase)
}

/// Checks whether the keystore file exists on disk.
pub fn exists() -> Result<bool> {
    let path = keystore_path()?;
    Ok(path.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_save_and_load() {
        // Use a temporary directory so we don't touch the real keystore.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("AGENTMARKET_HOME", tmp.path());

        let secret = b"super-secret-private-key-32bytes!";
        let passphrase = "hunter2";

        save_key(secret, passphrase).unwrap();

        // The keystore file should now exist.
        assert!(exists().unwrap());

        // Decrypt and verify.
        let recovered = load_key(passphrase).unwrap();
        assert_eq!(recovered, secret);

        // Wrong passphrase should fail.
        let err = load_key("wrong-passphrase");
        assert!(err.is_err());

        std::env::remove_var("AGENTMARKET_HOME");
    }

    #[test]
    fn exists_returns_false_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("AGENTMARKET_HOME", tmp.path());

        assert!(!exists().unwrap());

        std::env::remove_var("AGENTMARKET_HOME");
    }
}
