//! IPFS-based encrypted mailbox for agent-to-agent messaging.
//!
//! Each agent has a mailbox identified by `keccak256(compressed_public_key)`.
//! Messages are encrypted with ECIES before being stored on IPFS.
//!
//! The mailbox topic is a deterministic, hex-encoded keccak256 hash derived
//! from the agent's compressed secp256k1 public key. This allows any agent to
//! compute the mailbox address of any other agent whose public key is known,
//! without requiring a central directory.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::ipfs::client::IpfsClient;
use crate::ipfs::encryption;

// ---------------------------------------------------------------------------
// MailboxMessage
// ---------------------------------------------------------------------------

/// A message in the encrypted mailbox.
///
/// Messages are serialized to JSON, encrypted with ECIES for the recipient,
/// and stored on IPFS. The sender includes their public key so the recipient
/// can verify origin and respond.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MailboxMessage {
    /// Hex-encoded sender compressed public key.
    pub sender: String,
    /// Unix timestamp when the message was created.
    pub timestamp: u64,
    /// Message type identifier (e.g., "request", "response", "notification").
    pub message_type: String,
    /// The actual message payload (arbitrary bytes, application-defined).
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Mailbox
// ---------------------------------------------------------------------------

/// Represents a mailbox: a collection of encrypted messages for an agent.
///
/// The mailbox is identified by a topic derived from `keccak256` of the
/// agent's compressed public key bytes. Any agent who knows the recipient's
/// public key can compute the topic and send messages.
pub struct Mailbox {
    /// The agent's compressed public key (hex-encoded, without `0x` prefix).
    public_key: String,
    /// The mailbox topic (`keccak256` hash of the public key bytes, hex-encoded).
    topic: String,
}

impl Mailbox {
    /// Create a new mailbox for the given public key.
    ///
    /// The topic is derived as `keccak256(hex_decode(public_key))`, producing
    /// a deterministic 32-byte identifier. The public key may be provided with
    /// or without a `0x` prefix.
    pub fn new(public_key_hex: &str) -> Result<Self> {
        let hex_str = public_key_hex.strip_prefix("0x").unwrap_or(public_key_hex);

        let public_key_bytes =
            hex::decode(hex_str).context("failed to decode public key from hex")?;

        debug!(
            public_key_len = public_key_bytes.len(),
            "computing mailbox topic from public key"
        );

        let hash = alloy::primitives::keccak256(&public_key_bytes);
        let topic = hex::encode(hash.as_slice());

        debug!(topic = %topic, "mailbox topic computed");

        Ok(Self {
            public_key: hex_str.to_string(),
            topic,
        })
    }

    /// Returns the mailbox topic (`keccak256` hash, hex-encoded).
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Returns the public key this mailbox belongs to (hex-encoded, no `0x` prefix).
    pub fn public_key(&self) -> &str {
        &self.public_key
    }
}

// ---------------------------------------------------------------------------
// Seal / Open
// ---------------------------------------------------------------------------

/// Encrypt and serialize a message for a recipient.
///
/// The message is first serialized to JSON, then encrypted with ECIES using
/// the recipient's public key. Returns the encrypted bytes ready to be stored
/// on IPFS.
pub fn seal_message(recipient_public_key_hex: &str, message: &MailboxMessage) -> Result<Vec<u8>> {
    let plaintext =
        serde_json::to_vec(message).context("failed to serialize mailbox message to JSON")?;

    debug!(
        recipient = %recipient_public_key_hex,
        plaintext_len = plaintext.len(),
        message_type = %message.message_type,
        "sealing mailbox message"
    );

    let encrypted = encryption::encrypt(recipient_public_key_hex, &plaintext)
        .context("failed to encrypt mailbox message")?;

    debug!(encrypted_len = encrypted.len(), "mailbox message sealed");

    Ok(encrypted)
}

/// Decrypt and deserialize a message using the recipient's private key.
///
/// The encrypted bytes are first decrypted with ECIES, then the resulting
/// plaintext is deserialized from JSON into a [`MailboxMessage`].
pub fn open_message(private_key_bytes: &[u8], encrypted: &[u8]) -> Result<MailboxMessage> {
    debug!(encrypted_len = encrypted.len(), "opening mailbox message");

    let plaintext = encryption::decrypt(private_key_bytes, encrypted)
        .context("failed to decrypt mailbox message")?;

    let message: MailboxMessage = serde_json::from_slice(&plaintext)
        .context("failed to deserialize mailbox message from JSON")?;

    debug!(
        message_type = %message.message_type,
        sender = %message.sender,
        "mailbox message opened"
    );

    Ok(message)
}

// ---------------------------------------------------------------------------
// Publish / Retrieve
// ---------------------------------------------------------------------------

/// Publish an encrypted message to IPFS and return the CID.
///
/// The message is sealed (encrypted) for the recipient and then uploaded to
/// IPFS via the provided client. The returned CID can be shared with the
/// recipient so they can retrieve and decrypt the message.
pub async fn publish_message(
    ipfs: &IpfsClient,
    recipient_public_key_hex: &str,
    message: &MailboxMessage,
) -> Result<String> {
    debug!(
        recipient = %recipient_public_key_hex,
        message_type = %message.message_type,
        "publishing encrypted message to IPFS"
    );

    let encrypted = seal_message(recipient_public_key_hex, message)
        .context("failed to seal message for publishing")?;

    let cid = ipfs
        .add(&encrypted)
        .await
        .context("failed to upload encrypted message to IPFS")?;

    debug!(
        cid = %cid,
        message_type = %message.message_type,
        "encrypted message published to IPFS"
    );

    Ok(cid)
}

/// Retrieve and decrypt a message from IPFS by CID.
///
/// Fetches the encrypted bytes from IPFS using the given CID, then decrypts
/// and deserializes the message using the recipient's private key.
pub async fn retrieve_message(
    ipfs: &IpfsClient,
    private_key_bytes: &[u8],
    cid: &str,
) -> Result<MailboxMessage> {
    debug!(cid = %cid, "retrieving encrypted message from IPFS");

    let encrypted = ipfs
        .cat(cid)
        .await
        .with_context(|| format!("failed to fetch message from IPFS: {cid}"))?;

    let message =
        open_message(private_key_bytes, &encrypted).context("failed to open retrieved message")?;

    debug!(
        cid = %cid,
        message_type = %message.message_type,
        sender = %message.sender,
        "message retrieved and decrypted"
    );

    Ok(message)
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

    /// Helper: create a sample message for testing.
    fn sample_message(sender_hex: &str) -> MailboxMessage {
        MailboxMessage {
            sender: sender_hex.to_string(),
            timestamp: 1_700_000_000u64,
            message_type: "request".to_string(),
            payload: b"hello from agent".to_vec(),
        }
    }

    // -- mailbox topic is deterministic --------------------------------------

    #[test]
    fn mailbox_topic_is_deterministic() {
        let (_sk, pk_hex) = random_keypair();

        let mailbox_a = Mailbox::new(&pk_hex).expect("mailbox creation should succeed");
        let mailbox_b = Mailbox::new(&pk_hex).expect("mailbox creation should succeed");

        assert_eq!(
            mailbox_a.topic(),
            mailbox_b.topic(),
            "same public key must produce the same topic"
        );
        assert_eq!(
            mailbox_a.public_key(),
            mailbox_b.public_key(),
            "public key must be preserved"
        );
    }

    // -- mailbox topic differs for different keys ----------------------------

    #[test]
    fn mailbox_topic_differs_for_different_keys() {
        let (_sk1, pk1_hex) = random_keypair();
        let (_sk2, pk2_hex) = random_keypair();

        let mailbox_a = Mailbox::new(&pk1_hex).expect("mailbox creation should succeed");
        let mailbox_b = Mailbox::new(&pk2_hex).expect("mailbox creation should succeed");

        assert_ne!(
            mailbox_a.topic(),
            mailbox_b.topic(),
            "different public keys must produce different topics"
        );
    }

    // -- seal and open roundtrip ---------------------------------------------

    #[test]
    fn seal_and_open_roundtrip() {
        let (sk, pk_hex) = random_keypair();
        let message = sample_message(&pk_hex);

        let encrypted = seal_message(&pk_hex, &message).expect("sealing message should succeed");

        // Encrypted bytes must differ from the JSON plaintext.
        let plaintext = serde_json::to_vec(&message).unwrap();
        assert_ne!(
            encrypted, plaintext,
            "encrypted bytes must differ from plaintext"
        );

        let decrypted = open_message(&sk, &encrypted).expect("opening message should succeed");

        assert_eq!(decrypted.sender, message.sender);
        assert_eq!(decrypted.timestamp, message.timestamp);
        assert_eq!(decrypted.message_type, message.message_type);
        assert_eq!(decrypted.payload, message.payload);
        assert_eq!(
            decrypted, message,
            "roundtrip must preserve the full message"
        );
    }

    // -- open with wrong key fails -------------------------------------------

    #[test]
    fn open_with_wrong_key_fails() {
        let (_sk1, pk1_hex) = random_keypair();
        let (sk2, _pk2_hex) = random_keypair();

        let message = sample_message(&pk1_hex);
        let encrypted = seal_message(&pk1_hex, &message).expect("sealing message should succeed");

        let result = open_message(&sk2, &encrypted);
        assert!(
            result.is_err(),
            "opening a message with the wrong private key must fail"
        );
    }

    // -- message serialization -----------------------------------------------

    #[test]
    fn message_serialization() {
        let message = MailboxMessage {
            sender: "02abcdef1234567890".to_string(),
            timestamp: 1_700_000_000u64,
            message_type: "notification".to_string(),
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };

        let json = serde_json::to_string(&message).expect("serialization should succeed");
        let deserialized: MailboxMessage =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(deserialized, message, "serde roundtrip must be lossless");
    }

    // -- mailbox handles 0x-prefixed key -------------------------------------

    #[test]
    fn mailbox_handles_0x_prefix() {
        let (_sk, pk_hex) = random_keypair();

        let without_prefix = Mailbox::new(&pk_hex).expect("should succeed without prefix");
        let with_prefix =
            Mailbox::new(&format!("0x{}", pk_hex)).expect("should succeed with 0x prefix");

        assert_eq!(
            without_prefix.topic(),
            with_prefix.topic(),
            "0x-prefixed and bare keys must produce the same topic"
        );
    }

    // -- mailbox topic is valid hex of correct length ------------------------

    #[test]
    fn mailbox_topic_is_valid_hex() {
        let (_sk, pk_hex) = random_keypair();
        let mailbox = Mailbox::new(&pk_hex).expect("mailbox creation should succeed");

        let topic = mailbox.topic();

        // keccak256 produces 32 bytes = 64 hex characters.
        assert_eq!(topic.len(), 64, "topic must be 64 hex characters");
        assert!(hex::decode(topic).is_ok(), "topic must be valid hex");
    }
}
