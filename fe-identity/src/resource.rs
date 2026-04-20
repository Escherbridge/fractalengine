use crate::did_key::did_key_from_public_key;
use crate::keypair::NodeKeypair;
use bevy::prelude::Resource;
use ed25519_dalek::{Signature, VerifyingKey};

/// A Bevy ECS resource representing this node's identity.
///
/// Holds the `did:key` DID string and the corresponding Ed25519 public key.
/// The private key remains inside [`NodeKeypair`] for signing operations.
#[derive(Resource)]
pub struct NodeIdentity {
    /// The `did:key:z6Mk...` identifier for this node.
    pub did: String,
    /// The Ed25519 public key corresponding to this identity.
    pub public_key: VerifyingKey,
    /// The full keypair (private key included) for signing operations.
    keypair: NodeKeypair,
}

impl NodeIdentity {
    /// Create a new [`NodeIdentity`] from a [`NodeKeypair`].
    ///
    /// Derives the `did:key` string from the keypair's public key.
    pub fn new(keypair: NodeKeypair) -> Self {
        let public_key = keypair.verifying_key();
        let did = did_key_from_public_key(&public_key);
        Self {
            did,
            public_key,
            keypair,
        }
    }

    /// The `did:key` DID string for this node.
    ///
    /// Alias for `.did` provided for backward compatibility.
    #[inline]
    pub fn did_key(&self) -> &str {
        &self.did
    }

    /// The Ed25519 verifying (public) key for this node.
    ///
    /// Alias for `.public_key` provided for backward compatibility.
    #[inline]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.public_key
    }

    /// Sign a message with this node's private key.
    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.keypair.sign(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_identity_new_produces_valid_did() {
        let kp = NodeKeypair::generate();
        let identity = NodeIdentity::new(kp);
        assert!(
            identity.did.starts_with("did:key:"),
            "did must start with 'did:key:', got: {}",
            identity.did
        );
    }

    #[test]
    fn test_did_starts_with_did_key_z6mk() {
        let kp = NodeKeypair::generate();
        let identity = NodeIdentity::new(kp);
        assert!(
            identity.did.starts_with("did:key:z6Mk"),
            "Ed25519 did:key must start with 'did:key:z6Mk', got: {}",
            identity.did
        );
    }

    #[test]
    fn test_public_key_field_matches_verifying_key() {
        let kp = NodeKeypair::generate();
        let identity = NodeIdentity::new(kp);
        assert_eq!(
            identity.public_key.to_bytes(),
            identity.verifying_key().to_bytes(),
            "public_key field must match verifying_key() accessor"
        );
    }

    #[test]
    fn test_did_key_accessor_matches_did_field() {
        let kp = NodeKeypair::generate();
        let identity = NodeIdentity::new(kp);
        assert_eq!(identity.did_key(), identity.did.as_str());
    }

    #[test]
    fn test_sign_verifies_with_public_key() {
        let kp = NodeKeypair::generate();
        let identity = NodeIdentity::new(kp);
        let msg = b"fractalengine node identity";
        let sig = identity.sign(msg);
        identity
            .public_key
            .verify_strict(msg, &sig)
            .expect("signature must verify with public_key");
    }
}
