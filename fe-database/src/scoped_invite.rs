//! RBAC-scoped invite tokens with Ed25519 signatures.
//!
//! A [`ScopedInvite`] encodes an RBAC scope (verse, fractal, or petal level)
//! plus a role to grant upon acceptance. The invite is signed by the creator's
//! Ed25519 keypair so recipients can verify authenticity and detect tampering.
//!
//! Invite strings are JSON-serialised then URL-safe base64 encoded.  Full
//! invite links use the `fractalengine://invite/<payload>` scheme.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use fe_identity::NodeKeypair;
use serde::{Deserialize, Serialize};

use crate::role_level::RoleLevel;
use crate::scope::parse_scope;

/// A scoped invite token that encodes RBAC scope + role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedInvite {
    /// RBAC scope string: VERSE#id, VERSE#id-FRACTAL#id, or VERSE#id-FRACTAL#id-PETAL#id
    pub scope: String,
    /// Role to grant: viewer, editor, manager (NOT owner -- owner cannot be invited)
    pub role: RoleLevel,
    /// DID of the invite creator (did:key:z6Mk...)
    pub creator_did: String,
    /// Unix timestamp (seconds) after which the invite expires
    pub expiry_timestamp: u64,
    /// Hex-encoded Ed25519 signature over canonical_bytes()
    pub signature: String,
}

impl ScopedInvite {
    /// Deterministic byte representation for signing (excludes the signature field).
    ///
    /// Concatenates length-prefixed fields: scope, role (as string), creator_did,
    /// and expiry_timestamp (little-endian bytes).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        push_field(&mut buf, &self.scope);
        push_field(&mut buf, &self.role.to_string());
        push_field(&mut buf, &self.creator_did);
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

    /// Serialize to a `fractalengine://invite/<base64url-encoded-JSON>` link.
    pub fn to_invite_link(&self) -> String {
        format!("fractalengine://invite/{}", self.to_invite_string())
    }

    /// Parse from an invite link.
    ///
    /// Strips the `fractalengine://invite/` prefix, then base64-decodes and
    /// JSON-parses the payload.
    pub fn from_invite_link(link: &str) -> anyhow::Result<Self> {
        let payload = link
            .strip_prefix("fractalengine://invite/")
            .ok_or_else(|| {
                anyhow::anyhow!("invalid invite link: must start with 'fractalengine://invite/'")
            })?;
        Self::from_invite_string(payload)
    }

    /// Serialize to a URL-safe base64 invite string (no link prefix).
    pub fn to_invite_string(&self) -> String {
        let json = serde_json::to_string(self).expect("ScopedInvite serialization");
        URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    /// Parse from a raw base64url string.
    pub fn from_invite_string(s: &str) -> anyhow::Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s.trim())
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

/// Generate a scoped invite, signing it with the provided keypair.
///
/// # Errors
/// - Returns an error if `role` is `Owner` (owner cannot be invited).
/// - Returns an error if `scope` does not parse as a valid RBAC scope.
pub fn generate_scoped_invite(
    keypair: &NodeKeypair,
    creator_did: &str,
    scope: &str,
    role: RoleLevel,
    expiry_secs: u64,
) -> anyhow::Result<ScopedInvite> {
    if role == RoleLevel::Owner {
        anyhow::bail!("cannot create invite with Owner role");
    }

    parse_scope(scope).map_err(|e| anyhow::anyhow!("invalid scope: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut invite = ScopedInvite {
        scope: scope.to_string(),
        role,
        creator_did: creator_did.to_string(),
        expiry_timestamp: now + expiry_secs,
        signature: String::new(),
    };
    invite.sign(keypair);
    Ok(invite)
}

/// Validate a scoped invite: check signature, expiry, scope format, and role.
///
/// Derives the verifying key from `invite.creator_did` using the `did:key` format.
///
/// # Errors
/// - Invalid `creator_did` format
/// - Signature verification failure
/// - Invite has expired
/// - Invalid scope format
/// - Role is `Owner`
pub fn validate_scoped_invite(invite: &ScopedInvite) -> anyhow::Result<()> {
    // Derive verifying key from creator DID
    let vk = fe_identity::did_key::public_key_from_did_key(&invite.creator_did)?;

    // Verify signature
    if !invite.verify(&vk) {
        anyhow::bail!("invite signature verification failed");
    }

    // Check expiry
    if invite.is_expired() {
        anyhow::bail!("invite has expired");
    }

    // Validate scope format
    parse_scope(&invite.scope).map_err(|e| anyhow::anyhow!("invalid scope: {e}"))?;

    // Owner role cannot be granted via invite
    if invite.role == RoleLevel::Owner {
        anyhow::bail!("invite cannot grant Owner role");
    }

    Ok(())
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
    use crate::scope::build_scope;

    fn make_keypair_and_did() -> (NodeKeypair, String) {
        let kp = NodeKeypair::generate();
        let did = kp.to_did_key();
        (kp, did)
    }

    fn make_signed_invite(kp: &NodeKeypair, did: &str) -> ScopedInvite {
        let mut invite = ScopedInvite {
            scope: build_scope("v1", Some("f1"), None),
            role: RoleLevel::Editor,
            creator_did: did.to_string(),
            expiry_timestamp: u64::MAX,
            signature: String::new(),
        };
        invite.sign(kp);
        invite
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let (kp, did) = make_keypair_and_did();
        let invite = make_signed_invite(&kp, &did);
        let s = invite.to_invite_string();
        let parsed = ScopedInvite::from_invite_string(&s).unwrap();
        assert_eq!(parsed.scope, invite.scope);
        assert_eq!(parsed.role, invite.role);
        assert_eq!(parsed.creator_did, invite.creator_did);
        assert_eq!(parsed.expiry_timestamp, invite.expiry_timestamp);
        assert_eq!(parsed.signature, invite.signature);
    }

    #[test]
    fn sign_and_verify() {
        let (kp, did) = make_keypair_and_did();
        let invite = make_signed_invite(&kp, &did);
        assert!(invite.verify(&kp.verifying_key()));
    }

    #[test]
    fn tamper_scope_detected() {
        let (kp, did) = make_keypair_and_did();
        let mut invite = make_signed_invite(&kp, &did);
        invite.scope = build_scope("evil-verse", None, None);
        assert!(!invite.verify(&kp.verifying_key()));
    }

    #[test]
    fn tamper_role_detected() {
        let (kp, did) = make_keypair_and_did();
        let mut invite = make_signed_invite(&kp, &did);
        invite.role = RoleLevel::Manager;
        assert!(!invite.verify(&kp.verifying_key()));
    }

    #[test]
    fn expired_invite_rejected() {
        let (kp, did) = make_keypair_and_did();
        let mut invite = ScopedInvite {
            scope: build_scope("v1", None, None),
            role: RoleLevel::Viewer,
            creator_did: did,
            expiry_timestamp: 0,
            signature: String::new(),
        };
        invite.sign(&kp);
        assert!(invite.is_expired());
    }

    #[test]
    fn valid_invite_not_expired() {
        let (kp, did) = make_keypair_and_did();
        let invite = make_signed_invite(&kp, &did);
        assert!(!invite.is_expired());
    }

    #[test]
    fn owner_role_rejected() {
        let (kp, did) = make_keypair_and_did();
        let result = generate_scoped_invite(
            &kp,
            &did,
            &build_scope("v1", None, None),
            RoleLevel::Owner,
            3600,
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot create invite with Owner role")
        );
    }

    #[test]
    fn invalid_scope_rejected() {
        let (kp, did) = make_keypair_and_did();
        let result = generate_scoped_invite(&kp, &did, "bad-scope", RoleLevel::Editor, 3600);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid scope"));
    }

    #[test]
    fn invite_link_roundtrip() {
        let (kp, did) = make_keypair_and_did();
        let invite = make_signed_invite(&kp, &did);
        let link = invite.to_invite_link();
        assert!(link.starts_with("fractalengine://invite/"));
        let parsed = ScopedInvite::from_invite_link(&link).unwrap();
        assert_eq!(parsed.scope, invite.scope);
        assert_eq!(parsed.role, invite.role);
        assert_eq!(parsed.creator_did, invite.creator_did);
        assert_eq!(parsed.expiry_timestamp, invite.expiry_timestamp);
        assert_eq!(parsed.signature, invite.signature);
    }

    #[test]
    fn validate_checks_signature() {
        let (kp, did) = make_keypair_and_did();
        let mut invite = generate_scoped_invite(
            &kp,
            &did,
            &build_scope("v1", None, None),
            RoleLevel::Editor,
            3600,
        )
        .unwrap();
        // Tamper with scope after signing
        invite.scope = build_scope("tampered", None, None);
        let result = validate_scoped_invite(&invite);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("signature verification failed")
        );
    }

    #[test]
    fn validate_checks_expiry() {
        let (kp, did) = make_keypair_and_did();
        let mut invite = ScopedInvite {
            scope: build_scope("v1", None, None),
            role: RoleLevel::Viewer,
            creator_did: did,
            expiry_timestamp: 0, // already expired
            signature: String::new(),
        };
        invite.sign(&kp);
        let result = validate_scoped_invite(&invite);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expired"));
    }

    #[test]
    fn creator_did_roundtrip() {
        let (kp, did) = make_keypair_and_did();
        let invite = make_signed_invite(&kp, &did);
        // Recover the verifying key from the DID embedded in the invite
        let recovered_vk =
            fe_identity::did_key::public_key_from_did_key(&invite.creator_did).unwrap();
        assert!(invite.verify(&recovered_vk));
        assert_eq!(recovered_vk.to_bytes(), kp.verifying_key().to_bytes());
    }
}
