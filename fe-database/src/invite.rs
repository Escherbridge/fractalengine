//! Verse invite tokens for P2P Mycelium Phase F.
//!
//! A [`VerseInvite`] encodes everything a remote peer needs to join a verse:
//! the namespace ID (and optionally the write-capability secret), the
//! inviter's node address, the verse owner pin (`owner_did` + the manifest's
//! `created_at`), and an ed25519 signature over all of them (RP-4 step 4, T10 —
//! see src/AGENTS.md §handlers "replicated_row").
//!
//! Invite strings are JSON serialised then URL-safe base64 encoded so they
//! can be safely pasted into chat, QR codes, or CLI arguments.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use fe_identity::NodeKeypair;
use serde::{Deserialize, Serialize};

/// A self-contained invite token for joining a verse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerseInvite {
    /// Hex-encoded namespace ID for the verse's iroh-docs replica.
    pub namespace_id: String,
    /// Hex-encoded namespace secret. `None` means read-only access.
    pub namespace_secret: Option<String>,
    /// The INVITER's node address (ed25519 public key hex) — verifies the
    /// signature; becomes the joiner's `verse_member.invited_by`, never the owner.
    pub creator_node_addr: String,
    /// The verse owner's `did:key` (the inviter's `verse.created_by`) — the
    /// joiner's owner pin, written as its local `verse.created_by`.
    pub owner_did: String,
    /// The verse manifest's `created_at`, pinned with the owner so the owner's
    /// replicated manifest matches the joiner's immutable identity fields.
    pub verse_created_at: String,
    /// Human-readable verse name.
    pub verse_name: String,
    /// ULID of the verse.
    pub verse_id: String,
    /// Unix timestamp (seconds) after which the invite is considered expired.
    pub expiry_timestamp: u64,
    /// Hex-encoded ed25519 signature over `canonical_bytes()`.
    pub signature: String,
}

impl VerseInvite {
    /// Deterministic byte representation for signing (excludes the signature field).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // Concatenate fields in a deterministic order with length prefixes
        // to prevent ambiguity attacks.
        let mut buf = Vec::new();
        push_field(&mut buf, &self.namespace_id);
        // Encode namespace_secret presence and value
        match &self.namespace_secret {
            Some(s) => {
                buf.push(1);
                push_field(&mut buf, s);
            }
            None => {
                buf.push(0);
            }
        }
        push_field(&mut buf, &self.creator_node_addr);
        push_field(&mut buf, &self.owner_did);
        push_field(&mut buf, &self.verse_created_at);
        push_field(&mut buf, &self.verse_name);
        push_field(&mut buf, &self.verse_id);
        buf.extend_from_slice(&self.expiry_timestamp.to_le_bytes());
        buf
    }

    /// Sign the invite with the given keypair, setting the `signature` field.
    pub fn sign(&mut self, keypair: &NodeKeypair) {
        let bytes = self.canonical_bytes();
        let sig = keypair.sign(&bytes);
        self.signature = hex::encode(sig.to_bytes());
    }

    /// Verify the invite signature against a known verifying key.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> bool {
        let sig_bytes = match hex::decode(&self.signature) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let sig = match Signature::from_slice(&sig_bytes) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let bytes = self.canonical_bytes();
        verifying_key.verify(&bytes, &sig).is_ok()
    }

    /// Serialize to a URL-safe base64 invite string.
    pub fn to_invite_string(&self) -> String {
        let json = serde_json::to_string(self).expect("VerseInvite serialization");
        URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    /// Parse an invite string back into a `VerseInvite`.
    ///
    /// Accepts both raw base64 and `fractalengine://invite/<base64>` links.
    pub fn from_invite_string(s: &str) -> anyhow::Result<Self> {
        let payload = s.trim();
        let payload = payload
            .strip_prefix("fractalengine://invite/")
            .unwrap_or(payload);
        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|e| anyhow::anyhow!("invalid base64: {e}"))?;
        let invite: Self =
            serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("invalid JSON: {e}"))?;
        Ok(invite)
    }

    /// Whether the invite has expired (current time >= expiry).
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now >= self.expiry_timestamp
    }
}

/// Push a length-prefixed field into the canonical buffer.
fn push_field(buf: &mut Vec<u8>, s: &str) {
    let len = s.len() as u32;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_invite(keypair: &NodeKeypair) -> VerseInvite {
        let mut invite = VerseInvite {
            namespace_id: "aabbccdd".to_string(),
            namespace_secret: Some("secrethex".to_string()),
            creator_node_addr: "node123".to_string(),
            owner_did: "did:key:z6MkOwner".to_string(),
            verse_created_at: "2026-10-15T00:00:00Z".to_string(),
            verse_name: "Test Verse".to_string(),
            verse_id: "verse-ulid-001".to_string(),
            expiry_timestamp: u64::MAX, // far future
            signature: String::new(),
        };
        invite.sign(keypair);
        invite
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let kp = NodeKeypair::generate();
        let invite = make_invite(&kp);
        let s = invite.to_invite_string();
        let parsed = VerseInvite::from_invite_string(&s).unwrap();
        assert_eq!(parsed.verse_id, invite.verse_id);
        assert_eq!(parsed.namespace_id, invite.namespace_id);
        assert_eq!(parsed.namespace_secret, invite.namespace_secret);
        assert_eq!(parsed.verse_name, invite.verse_name);
        assert_eq!(parsed.signature, invite.signature);
    }

    #[test]
    fn sign_and_verify() {
        let kp = NodeKeypair::generate();
        let invite = make_invite(&kp);
        assert!(invite.verify(&kp.verifying_key()));
    }

    #[test]
    fn tamper_detection() {
        let kp = NodeKeypair::generate();
        let mut invite = make_invite(&kp);
        // Tamper with the verse name
        invite.verse_name = "Evil Verse".to_string();
        assert!(!invite.verify(&kp.verifying_key()));

        use crate::join_invite::{JoinInvitation, JoinManifest, JoinRole, JOIN_PREFIX};
        let recipient = NodeKeypair::generate().to_did_key();
        let manifest = JoinManifest {
            verse_id: "addressed-verse".into(),
            name: "Addressed".into(),
            created_by: kp.to_did_key(),
            created_at: "2026-11-02T00:00:00Z".into(),
            namespace_id: hex::encode(crate::derive_namespace_id(&[23u8; 32])),
            default_access: "none".into(),
            ts_mode: "mirror".into(),
            ts_replication_factor: 1,
            ts_bucket_width_ms: 86_400_000,
        };
        // Native ticket grammar fixture; its controlled address has no live listener.
        for role in [JoinRole::Viewer, JoinRole::Editor, JoinRole::Manager] {
            let secret = iroh_docs::NamespaceSecret::from_bytes(&[23u8; 32]);
            let capability = if role == JoinRole::Viewer {
                iroh_docs::Capability::Read(secret.id())
            } else {
                iroh_docs::Capability::Write(secret)
            };
            let peer = iroh::EndpointAddr::new(kp.to_iroh_secret().public())
                .with_ip_addr("127.0.0.1:40000".parse().unwrap());
            let ticket = iroh_docs::DocTicket::new(capability, vec![peer]).to_string();
            let addressed = JoinInvitation::issue_owner(
                &kp,
                recipient.clone(),
                "VERSE#addressed-verse".into(),
                role,
                1,
                manifest.clone(),
                ticket,
                100,
                3600,
            )
            .unwrap();
            let link = addressed.to_link().unwrap();
            let verified = JoinInvitation::parse_owner(&link, &recipient, 101).unwrap();
            assert_eq!(verified.role(), role);
            assert_eq!(verified.manifest(), &manifest);
            assert!(!format!("{verified:?}").contains(verified.ticket()));
            assert!(JoinInvitation::parse_owner(&link, &kp.to_did_key(), 101).is_err());
            assert!(JoinInvitation::parse_owner(&link, &recipient, 3700).is_err());
            for field in [
                "recipient_did",
                "scope",
                "ticket",
                "nonce",
                "issuer_did",
                "manifest",
            ] {
                let bytes = URL_SAFE_NO_PAD
                    .decode(link.strip_prefix(JOIN_PREFIX).unwrap())
                    .unwrap();
                let mut altered: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                altered[field] = serde_json::json!("changed");
                let altered = format!(
                    "{JOIN_PREFIX}{}",
                    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&altered).unwrap())
                );
                assert!(
                    JoinInvitation::parse_owner(&altered, &recipient, 101).is_err(),
                    "{field}"
                );
            }
            assert!(JoinInvitation::parse_owner(
                &link.replacen("/v1/", "/v2/", 1),
                &recipient,
                101
            )
            .is_err());
            assert!(JoinInvitation::parse_owner(&(link.clone() + "="), &recipient, 101).is_err());
        }
    }

    /// T10: the owner pin is inside the signed bytes — swapping the owner (or
    /// the pinned manifest timestamp) after signing fails verification.
    #[test]
    fn a_tampered_invite_owner_did_fails_signature_verification() {
        let kp = NodeKeypair::generate();
        let mut invite = make_invite(&kp);
        invite.owner_did = "did:key:z6MkAttacker".to_string();
        assert!(!invite.verify(&kp.verifying_key()));

        let mut invite = make_invite(&kp);
        invite.verse_created_at = "2030-01-01T00:00:00Z".to_string();
        assert!(!invite.verify(&kp.verifying_key()));

        // Round-tripped through the invite string, the untampered pin still verifies.
        let parsed = VerseInvite::from_invite_string(&make_invite(&kp).to_invite_string()).unwrap();
        assert_eq!(parsed.owner_did, "did:key:z6MkOwner");
        assert!(parsed.verify(&kp.verifying_key()));
    }

    #[test]
    fn tampered_namespace_secret_detected() {
        let kp = NodeKeypair::generate();
        let mut invite = make_invite(&kp);
        invite.namespace_secret = None; // strip write cap
        assert!(!invite.verify(&kp.verifying_key()));
    }

    #[test]
    fn read_only_invite_roundtrip() {
        let kp = NodeKeypair::generate();
        let mut invite = VerseInvite {
            namespace_id: "aabb".to_string(),
            namespace_secret: None,
            creator_node_addr: "node1".to_string(),
            owner_did: "did:key:z6MkOwner".to_string(),
            verse_created_at: "2026-10-15T00:00:00Z".to_string(),
            verse_name: "RO Verse".to_string(),
            verse_id: "v-ro".to_string(),
            expiry_timestamp: u64::MAX,
            signature: String::new(),
        };
        invite.sign(&kp);
        let s = invite.to_invite_string();
        let parsed = VerseInvite::from_invite_string(&s).unwrap();
        assert!(parsed.verify(&kp.verifying_key()));
        assert!(parsed.namespace_secret.is_none());
    }

    #[test]
    fn expiry_check() {
        let kp = NodeKeypair::generate();
        let mut invite = VerseInvite {
            namespace_id: "aa".to_string(),
            namespace_secret: None,
            creator_node_addr: "n".to_string(),
            owner_did: "did:key:z6MkOwner".to_string(),
            verse_created_at: "2026-10-15T00:00:00Z".to_string(),
            verse_name: "Exp".to_string(),
            verse_id: "v-exp".to_string(),
            expiry_timestamp: 0, // already expired
            signature: String::new(),
        };
        invite.sign(&kp);
        assert!(invite.is_expired());

        invite.expiry_timestamp = u64::MAX;
        // Need to re-sign since expiry changed
        invite.sign(&kp);
        assert!(!invite.is_expired());
    }

    #[test]
    fn invalid_base64_rejected() {
        assert!(VerseInvite::from_invite_string("not-valid!!!").is_err());
    }

    #[test]
    fn invalid_json_rejected() {
        let encoded = URL_SAFE_NO_PAD.encode(b"not json");
        assert!(VerseInvite::from_invite_string(&encoded).is_err());
    }
}
