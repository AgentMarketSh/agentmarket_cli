//! ECIES encryption and decryption over secp256k1.
//!
//! Used for encrypting IPFS mailbox messages and deliverables. The same
//! secp256k1 keypair used for Ethereum transaction signing is reused for
//! ECIES encryption — no separate encryption key is needed.

use anyhow::{Context, Result};
use tracing::debug;

/// Encrypt a plaintext message for a recipient identified by their
/// compressed secp256k1 public key (33 bytes, hex-encoded).
///
/// The public key may be provided with or without a `0x` prefix. Both
/// compressed (33-byte / 66-hex-char) and uncompressed (65-byte /
/// 130-hex-char) encodings are accepted by the underlying `ecies` crate.
///
/// Returns the ECIES ciphertext as raw bytes.
pub fn encrypt(public_key_hex: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let hex_str = public_key_hex.strip_prefix("0x").unwrap_or(public_key_hex);

    let public_key_bytes = hex::decode(hex_str).context("failed to decode public key from hex")?;

    debug!(
        public_key_len = public_key_bytes.len(),
        plaintext_len = plaintext.len(),
        "encrypting message with ECIES"
    );

    let ciphertext = ecies::encrypt(&public_key_bytes, plaintext)
        .map_err(|e| anyhow::anyhow!("ECIES encryption failed: {:?}", e))?;

    debug!(
        ciphertext_len = ciphertext.len(),
        "ECIES encryption complete"
    );
    Ok(ciphertext)
}

/// Decrypt an ECIES ciphertext using the recipient's private key (32 bytes).
///
/// Returns the decrypted plaintext.
pub fn decrypt(private_key_bytes: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    debug!(
        private_key_len = private_key_bytes.len(),
        ciphertext_len = ciphertext.len(),
        "decrypting message with ECIES"
    );

    let plaintext = ecies::decrypt(private_key_bytes, ciphertext)
        .map_err(|e| anyhow::anyhow!("ECIES decryption failed: {:?}", e))?;

    debug!(plaintext_len = plaintext.len(), "ECIES decryption complete");
    Ok(plaintext)
}

/// Encrypt plaintext and return the ciphertext as a hex string.
/// Convenience wrapper around [`encrypt`].
pub fn encrypt_hex(public_key_hex: &str, plaintext: &[u8]) -> Result<String> {
    let ciphertext = encrypt(public_key_hex, plaintext)?;
    Ok(hex::encode(ciphertext))
}

/// Decrypt a hex-encoded ciphertext using the private key.
/// Convenience wrapper around [`decrypt`].
pub fn decrypt_hex(private_key_bytes: &[u8], ciphertext_hex: &str) -> Result<Vec<u8>> {
    let hex_str = ciphertext_hex.strip_prefix("0x").unwrap_or(ciphertext_hex);

    let ciphertext = hex::decode(hex_str).context("failed to decode ciphertext from hex")?;

    decrypt(private_key_bytes, &ciphertext)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;

    /// Helper: generate a random secp256k1 keypair and return
    /// `(private_key_bytes, compressed_public_key_hex)`.
    fn random_keypair() -> (Vec<u8>, String) {
        let signer = PrivateKeySigner::random();

        let private_key_bytes = signer.credential().to_bytes().to_vec();

        let verifying_key = signer.credential().verifying_key();
        let public_key_point = verifying_key.to_encoded_point(true);
        let public_key_hex = hex::encode(public_key_point.as_bytes());

        (private_key_bytes, public_key_hex)
    }

    // -- encrypt / decrypt round-trip -----------------------------------------

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let (sk, pk_hex) = random_keypair();
        let message = b"Hello, AgentMarket!";

        let ciphertext = encrypt(&pk_hex, message).expect("encryption should succeed");

        // Ciphertext must be longer than the plaintext (it includes the
        // ephemeral public key and the authentication tag).
        assert!(
            ciphertext.len() > message.len(),
            "ciphertext should be longer than plaintext"
        );

        let decrypted = decrypt(&sk, &ciphertext).expect("decryption should succeed");
        assert_eq!(
            decrypted, message,
            "decrypted message must match the original"
        );
    }

    // -- empty message --------------------------------------------------------

    #[test]
    fn encrypt_empty_message() {
        let (sk, pk_hex) = random_keypair();
        let message: &[u8] = b"";

        let ciphertext =
            encrypt(&pk_hex, message).expect("encrypting empty message should succeed");
        let decrypted = decrypt(&sk, &ciphertext).expect("decrypting empty message should succeed");

        assert_eq!(decrypted, message, "decrypted empty message must be empty");
    }

    // -- wrong key fails ------------------------------------------------------

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let (_sk1, pk1_hex) = random_keypair();
        let (sk2, _pk2_hex) = random_keypair();

        let message = b"secret payload";
        let ciphertext = encrypt(&pk1_hex, message).expect("encryption should succeed");

        let result = decrypt(&sk2, &ciphertext);
        assert!(result.is_err(), "decrypting with the wrong key must fail");
    }

    // -- hex convenience wrappers ---------------------------------------------

    #[test]
    fn encrypt_hex_roundtrip() {
        let (sk, pk_hex) = random_keypair();
        let message = b"round-trip via hex encoding";

        let ciphertext_hex = encrypt_hex(&pk_hex, message).expect("encrypt_hex should succeed");

        // The hex string must be valid hex.
        assert!(
            hex::decode(&ciphertext_hex).is_ok(),
            "encrypt_hex output must be valid hex"
        );

        let decrypted = decrypt_hex(&sk, &ciphertext_hex).expect("decrypt_hex should succeed");

        assert_eq!(
            decrypted, message,
            "hex round-trip must recover the original message"
        );
    }

    // -- invalid public key ---------------------------------------------------

    #[test]
    fn encrypt_with_invalid_pubkey_fails() {
        // An all-zero 33-byte value is not a valid compressed public key.
        let bogus_pubkey_hex = "00".repeat(33);
        let result = encrypt(&bogus_pubkey_hex, b"test");
        assert!(
            result.is_err(),
            "encrypting with an invalid public key must fail"
        );
    }

    // -- 0x-prefixed keys work ------------------------------------------------

    #[test]
    fn encrypt_with_0x_prefix() {
        let (sk, pk_hex) = random_keypair();
        let prefixed = format!("0x{}", pk_hex);
        let message = b"0x-prefix test";

        let ciphertext = encrypt(&prefixed, message).expect("encrypt with 0x prefix should work");
        let decrypted = decrypt(&sk, &ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, message);
    }

    // -- decrypt_hex with 0x-prefixed ciphertext ------------------------------

    #[test]
    fn decrypt_hex_with_0x_prefix() {
        let (sk, pk_hex) = random_keypair();
        let message = b"0x ciphertext prefix test";

        let ciphertext_hex = encrypt_hex(&pk_hex, message).expect("encrypt_hex should succeed");
        let prefixed = format!("0x{}", ciphertext_hex);

        let decrypted =
            decrypt_hex(&sk, &prefixed).expect("decrypt_hex with 0x prefix should work");
        assert_eq!(decrypted, message);
    }
}
