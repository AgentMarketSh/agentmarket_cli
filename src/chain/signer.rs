//! Transaction signing for AgentMarket CLI.
//!
//! Wraps alloy's [`PrivateKeySigner`] with secure key loading from the
//! encrypted keystore. Private key bytes are zeroed after signer construction
//! to minimise the window of exposure in memory.

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use anyhow::{bail, Context, Result};
use tracing::debug;

use crate::config::keystore;

// ---------------------------------------------------------------------------
// TransactionSigner
// ---------------------------------------------------------------------------

/// A transaction signer backed by a secp256k1 private key.
///
/// Constructed from the encrypted keystore or from raw key bytes. Once built,
/// the original key material is zeroed — only alloy's internal representation
/// remains.
pub struct TransactionSigner {
    signer: PrivateKeySigner,
}

impl TransactionSigner {
    /// Load the private key from the keystore and build a signer.
    ///
    /// The passphrase is obtained from the `AGENTMARKET_KEYSTORE_PASSPHRASE`
    /// environment variable, or via an interactive prompt if the env var is
    /// not set.
    pub fn from_keystore() -> Result<Self> {
        let passphrase =
            keystore::get_passphrase().context("failed to obtain keystore passphrase")?;

        Self::from_keystore_with_passphrase(&passphrase)
    }

    /// Build a signer from an explicit passphrase (useful for non-interactive
    /// contexts such as daemon mode or tests).
    pub fn from_keystore_with_passphrase(passphrase: &str) -> Result<Self> {
        debug!("loading signing key from keystore");

        let mut key_bytes =
            keystore::load_key(passphrase).context("failed to load signing key from keystore")?;

        let signer = Self::from_bytes(&mut key_bytes)?;

        debug!(address = %signer.address(), "signer loaded from keystore");
        Ok(signer)
    }

    /// Build from raw private key bytes (must be exactly 32 bytes).
    ///
    /// The input vector is zeroed after construction regardless of success or
    /// failure, preventing the raw secret from lingering in memory.
    pub fn from_bytes(key_bytes: &mut Vec<u8>) -> Result<Self> {
        let result = Self::from_bytes_inner(key_bytes);

        // Always zero the caller's bytes, even on error.
        for byte in key_bytes.iter_mut() {
            *byte = 0;
        }

        result
    }

    /// Inner helper that performs the actual construction before zeroing.
    fn from_bytes_inner(key_bytes: &[u8]) -> Result<Self> {
        if key_bytes.len() != 32 {
            bail!(
                "private key must be exactly 32 bytes, got {}",
                key_bytes.len()
            );
        }

        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("private key must be exactly 32 bytes"))?;

        let signer = PrivateKeySigner::from_bytes(&key_array.into())
            .context("failed to construct signer from private key bytes")?;

        debug!(address = %signer.address(), "transaction signer created");

        Ok(Self { signer })
    }

    /// Returns the Ethereum address derived from the signing key.
    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Returns a reference to the inner alloy signer.
    ///
    /// This is needed for provider integration (e.g. building a
    /// `SignerMiddleware` or filling transaction signatures).
    pub fn inner(&self) -> &PrivateKeySigner {
        &self.signer
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A known 32-byte private key for deterministic tests.
    /// This is the hex encoding of bytes 1..=32.
    fn test_key_bytes() -> Vec<u8> {
        vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
            0x1d, 0x1e, 0x1f, 0x20,
        ]
    }

    // -- from_bytes with valid key -------------------------------------------

    #[test]
    fn from_bytes_valid_key_creates_signer_and_zeros_input() {
        let mut key = test_key_bytes();
        let signer = TransactionSigner::from_bytes(&mut key)
            .expect("from_bytes should succeed with 32-byte key");

        // The input bytes must be zeroed after construction.
        assert!(
            key.iter().all(|&b| b == 0),
            "key bytes must be zeroed after from_bytes"
        );

        // The signer should have a valid address.
        let addr = signer.address();
        let addr_str = format!("{addr}");
        assert!(addr_str.starts_with("0x"), "address must start with 0x");
        assert_eq!(
            addr_str.len(),
            42,
            "address must be 42 characters (0x + 40 hex digits)"
        );
    }

    // -- from_bytes with invalid length --------------------------------------

    #[test]
    fn from_bytes_too_short_fails() {
        let mut key = vec![0u8; 16];
        let result = TransactionSigner::from_bytes(&mut key);
        assert!(
            result.is_err(),
            "from_bytes should reject keys shorter than 32 bytes"
        );

        // Input must still be zeroed even on failure.
        assert!(
            key.iter().all(|&b| b == 0),
            "key bytes must be zeroed even on failure"
        );
    }

    #[test]
    fn from_bytes_too_long_fails() {
        let mut key = vec![0xffu8; 64];
        let result = TransactionSigner::from_bytes(&mut key);
        assert!(
            result.is_err(),
            "from_bytes should reject keys longer than 32 bytes"
        );

        // Input must still be zeroed.
        assert!(
            key.iter().all(|&b| b == 0),
            "key bytes must be zeroed even on failure"
        );
    }

    #[test]
    fn from_bytes_empty_fails() {
        let mut key = Vec::new();
        let result = TransactionSigner::from_bytes(&mut key);
        assert!(result.is_err(), "from_bytes should reject empty input");
    }

    // -- address format ------------------------------------------------------

    #[test]
    fn address_returns_valid_format() {
        let mut key = test_key_bytes();
        let signer = TransactionSigner::from_bytes(&mut key).unwrap();
        let addr = format!("{}", signer.address());

        assert!(addr.starts_with("0x"), "address must be 0x-prefixed");
        assert_eq!(addr.len(), 42, "address must be 42 characters");

        // All characters after 0x should be valid hex.
        assert!(
            addr[2..].chars().all(|c| c.is_ascii_hexdigit()),
            "address must contain only hex digits after 0x prefix"
        );
    }

    // -- round-trip with identity::generate_keypair --------------------------

    #[test]
    fn round_trip_with_generate_keypair() {
        let (mut private_key, _public_key, expected_address) =
            crate::engine::identity::generate_keypair().expect("generate_keypair should succeed");

        let signer = TransactionSigner::from_bytes(&mut private_key)
            .expect("from_bytes should succeed with generated key");

        let signer_address = format!("{}", signer.address());
        assert_eq!(
            signer_address, expected_address,
            "signer address must match the address from generate_keypair"
        );

        // Private key bytes should be zeroed.
        assert!(
            private_key.iter().all(|&b| b == 0),
            "generated key bytes must be zeroed after from_bytes"
        );
    }

    // -- inner returns the underlying signer ---------------------------------

    #[test]
    fn inner_returns_same_address() {
        let mut key = test_key_bytes();
        let signer = TransactionSigner::from_bytes(&mut key).unwrap();

        assert_eq!(
            signer.inner().address(),
            signer.address(),
            "inner signer address must match TransactionSigner::address()"
        );
    }
}
