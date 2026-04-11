use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// A signed invitation to join a Verse.
/// The inviter signs the canonical payload with their Ed25519 signing key.
/// The resulting sig is stored in `verse_member.invite_sig` for audit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerseInvite {
    pub verse_id: String,
    pub invitee_did: String, // did:key of the person being invited
    pub inviter_did: String, // did:key of the inviter (existing member)
    pub timestamp: String,   // ISO8601, used to prevent replay
    /// Hex-encoded Ed25519 signature over `canonical_payload()`.
    pub signature: String,
}

impl VerseInvite {
    /// Canonical payload that is signed: `"{verse_id}:{invitee_did}:{inviter_did}:{timestamp}"`.
    pub fn canonical_payload(&self) -> Vec<u8> {
        format!(
            "{}:{}:{}:{}",
            self.verse_id, self.invitee_did, self.inviter_did, self.timestamp
        )
        .into_bytes()
    }

    /// Create and sign a new invite. `signing_key` belongs to the inviter.
    pub fn sign(
        verse_id: String,
        invitee_did: String,
        inviter_did: String,
        timestamp: String,
        signing_key: &SigningKey,
    ) -> Self {
        let mut invite = VerseInvite {
            verse_id,
            invitee_did,
            inviter_did,
            timestamp,
            signature: String::new(),
        };
        let payload = invite.canonical_payload();
        let sig: Signature = signing_key.sign(&payload);
        invite.signature = hex::encode(sig.to_bytes());
        invite
    }

    /// Verify the invite signature against the inviter's verifying key.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<(), VerifyError> {
        let sig_bytes =
            hex::decode(&self.signature).map_err(|_| VerifyError::MalformedSignature)?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| VerifyError::MalformedSignature)?;
        let sig = Signature::from_bytes(&sig_arr);
        let payload = self.canonical_payload();
        verifying_key
            .verify(&payload, &sig)
            .map_err(|_| VerifyError::InvalidSignature)
    }
}

/// A signed revocation notice — removes a peer from a Verse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerseRevocation {
    pub verse_id: String,
    pub revoked_did: String, // did:key being removed
    pub revoker_did: String, // did:key of revoker (must be verse founder/admin)
    pub timestamp: String,
    pub signature: String,
}

impl VerseRevocation {
    pub fn canonical_payload(&self) -> Vec<u8> {
        format!(
            "revoke:{}:{}:{}:{}",
            self.verse_id, self.revoked_did, self.revoker_did, self.timestamp
        )
        .into_bytes()
    }

    pub fn sign(
        verse_id: String,
        revoked_did: String,
        revoker_did: String,
        timestamp: String,
        signing_key: &SigningKey,
    ) -> Self {
        let mut rev = VerseRevocation {
            verse_id,
            revoked_did,
            revoker_did,
            timestamp,
            signature: String::new(),
        };
        let payload = rev.canonical_payload();
        let sig: Signature = signing_key.sign(&payload);
        rev.signature = hex::encode(sig.to_bytes());
        rev
    }

    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<(), VerifyError> {
        let sig_bytes =
            hex::decode(&self.signature).map_err(|_| VerifyError::MalformedSignature)?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| VerifyError::MalformedSignature)?;
        let sig = Signature::from_bytes(&sig_arr);
        let payload = self.canonical_payload();
        verifying_key
            .verify(&payload, &sig)
            .map_err(|_| VerifyError::InvalidSignature)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VerifyError {
    MalformedSignature,
    InvalidSignature,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::MalformedSignature => write!(f, "malformed signature hex"),
            VerifyError::InvalidSignature => write!(f, "signature verification failed"),
        }
    }
}

impl std::error::Error for VerifyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn make_signing_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn sign_and_verify_invite_roundtrips() {
        let sk = make_signing_key();
        let vk = sk.verifying_key();
        let invite = VerseInvite::sign(
            "verse-abc".to_string(),
            "did:key:invitee".to_string(),
            "did:key:inviter".to_string(),
            "2026-03-24T00:00:00Z".to_string(),
            &sk,
        );
        assert!(invite.verify(&vk).is_ok());
    }

    #[test]
    fn tampered_invitee_fails_verification() {
        let sk = make_signing_key();
        let vk = sk.verifying_key();
        let mut invite = VerseInvite::sign(
            "verse-abc".to_string(),
            "did:key:invitee".to_string(),
            "did:key:inviter".to_string(),
            "2026-03-24T00:00:00Z".to_string(),
            &sk,
        );
        invite.invitee_did = "did:key:attacker".to_string();
        assert_eq!(invite.verify(&vk), Err(VerifyError::InvalidSignature));
    }

    #[test]
    fn tampered_verse_id_fails_verification() {
        let sk = make_signing_key();
        let vk = sk.verifying_key();
        let mut invite = VerseInvite::sign(
            "verse-abc".to_string(),
            "did:key:invitee".to_string(),
            "did:key:inviter".to_string(),
            "2026-03-24T00:00:00Z".to_string(),
            &sk,
        );
        invite.verse_id = "verse-evil".to_string();
        assert_eq!(invite.verify(&vk), Err(VerifyError::InvalidSignature));
    }

    #[test]
    fn sign_and_verify_revocation_roundtrips() {
        let sk = make_signing_key();
        let vk = sk.verifying_key();
        let rev = VerseRevocation::sign(
            "verse-abc".to_string(),
            "did:key:kicked".to_string(),
            "did:key:founder".to_string(),
            "2026-03-24T01:00:00Z".to_string(),
            &sk,
        );
        assert!(rev.verify(&vk).is_ok());
    }

    #[test]
    fn tampered_revocation_fails_verification() {
        let sk = make_signing_key();
        let vk = sk.verifying_key();
        let mut rev = VerseRevocation::sign(
            "verse-abc".to_string(),
            "did:key:kicked".to_string(),
            "did:key:founder".to_string(),
            "2026-03-24T01:00:00Z".to_string(),
            &sk,
        );
        rev.revoker_did = "did:key:imposter".to_string();
        assert_eq!(rev.verify(&vk), Err(VerifyError::InvalidSignature));
    }

    #[test]
    fn malformed_sig_hex_returns_error() {
        let sk = make_signing_key();
        let vk = sk.verifying_key();
        let mut invite = VerseInvite::sign(
            "verse-abc".to_string(),
            "did:key:invitee".to_string(),
            "did:key:inviter".to_string(),
            "2026-03-24T00:00:00Z".to_string(),
            &sk,
        );
        invite.signature = "zz".to_string();
        assert_eq!(invite.verify(&vk), Err(VerifyError::MalformedSignature));
    }

    #[test]
    fn invite_serde_roundtrip() {
        let sk = make_signing_key();
        let invite = VerseInvite::sign(
            "verse-abc".to_string(),
            "did:key:invitee".to_string(),
            "did:key:inviter".to_string(),
            "2026-03-24T00:00:00Z".to_string(),
            &sk,
        );
        let json = serde_json::to_string(&invite).expect("serialize");
        let decoded: VerseInvite = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(invite, decoded);
    }
}
