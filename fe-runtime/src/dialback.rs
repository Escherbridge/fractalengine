//! Signed dial-back record and its acceptance rule; see `fe-sync/src/AGENTS.md` §reachability.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use serde::{Deserialize, Serialize};

/// Signature domain separator (NUL-terminated).
pub const DIALBACK_DOMAIN: &[u8] = b"fe-dialback-v1\0";

/// A record older than this is stale (AC-16).
pub const DIALBACK_MAX_AGE_MS: u64 = 10 * 60 * 1000;

/// Tolerated clock skew for a record stamped in the future.
pub const DIALBACK_FUTURE_SKEW_MS: u64 = 60 * 1000;

const SIGNATURE_LENGTH: usize = 64;

/// Result of a dial-back probe, signed by the prober for one requester and one URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialbackRecord {
    pub prober_did: String,
    pub requester_did: String,
    pub url: String,
    pub at_ms: u64,
    pub ok: bool,
    pub sig: Vec<u8>,
}

/// Why a record was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialbackRefusal {
    ProbeFailed,
    WrongRequester,
    SelfAttestation,
    UrlMismatch,
    Stale,
    FromFuture,
    MalformedProberDid,
    BadSignature,
}

fn push_field(preimage: &mut Vec<u8>, bytes: &[u8]) {
    preimage.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    preimage.extend_from_slice(bytes);
}

impl DialbackRecord {
    /// Length-prefixed canonical bytes of every field except `sig`, under the domain.
    fn signing_preimage(
        prober_did: &str,
        requester_did: &str,
        url: &str,
        at_ms: u64,
        ok: bool,
    ) -> Vec<u8> {
        let mut preimage = Vec::with_capacity(DIALBACK_DOMAIN.len() + 64 + url.len());
        preimage.extend_from_slice(DIALBACK_DOMAIN);
        push_field(&mut preimage, prober_did.as_bytes());
        push_field(&mut preimage, requester_did.as_bytes());
        push_field(&mut preimage, url.as_bytes());
        preimage.extend_from_slice(&at_ms.to_le_bytes());
        preimage.push(u8::from(ok));
        preimage
    }

    /// Signs a record as the prober; `prober_did` is derived from the key.
    pub fn sign(
        prober_key: &SigningKey,
        requester_did: &str,
        url: &str,
        at_ms: u64,
        ok: bool,
    ) -> Self {
        let prober_did = fe_identity::did_key_from_public_key(&prober_key.verifying_key());
        let preimage = Self::signing_preimage(&prober_did, requester_did, url, at_ms, ok);
        let signature = prober_key.sign(&preimage);
        Self {
            prober_did,
            requester_did: requester_did.to_string(),
            url: url.to_string(),
            at_ms,
            ok,
            sig: signature.to_bytes().to_vec(),
        }
    }

    /// Verifies the signature against the key encoded in `prober_did`.
    pub fn verify_signature(&self) -> Result<(), DialbackRefusal> {
        let verifying_key = fe_identity::public_key_from_did_key(&self.prober_did)
            .map_err(|_| DialbackRefusal::MalformedProberDid)?;
        let signature_bytes: [u8; SIGNATURE_LENGTH] = self
            .sig
            .as_slice()
            .try_into()
            .map_err(|_| DialbackRefusal::BadSignature)?;
        let preimage = Self::signing_preimage(
            &self.prober_did,
            &self.requester_did,
            &self.url,
            self.at_ms,
            self.ok,
        );
        verifying_key
            .verify(&preimage, &Signature::from_bytes(&signature_bytes))
            .map_err(|_| DialbackRefusal::BadSignature)
    }
}

/// Acceptance rule for an advertised relay: ok, bound to this advertiser and URL, fresh, third-party, signed.
pub fn accept(
    record: &DialbackRecord,
    advertiser_did: &str,
    url: &str,
    now_ms: u64,
) -> Result<(), DialbackRefusal> {
    if !record.ok {
        return Err(DialbackRefusal::ProbeFailed);
    }
    if record.requester_did != advertiser_did {
        return Err(DialbackRefusal::WrongRequester);
    }
    if record.prober_did == advertiser_did {
        return Err(DialbackRefusal::SelfAttestation);
    }
    if record.url != url {
        return Err(DialbackRefusal::UrlMismatch);
    }
    if record.at_ms > now_ms.saturating_add(DIALBACK_FUTURE_SKEW_MS) {
        return Err(DialbackRefusal::FromFuture);
    }
    if now_ms.saturating_sub(record.at_ms) > DIALBACK_MAX_AGE_MS {
        return Err(DialbackRefusal::Stale);
    }
    record.verify_signature()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn did(signing_key: &SigningKey) -> String {
        fe_identity::did_key_from_public_key(&signing_key.verifying_key())
    }

    #[test]
    fn dialback_record_refuses_self_attestation_and_stale_or_failed_probes() {
        let (prober, advertiser, stranger) = (key(1), key(2), key(3));
        let advertiser_did = did(&advertiser);
        let url = "https://relay.example.org";
        let now = 10_000_000u64;

        let good = DialbackRecord::sign(&prober, &advertiser_did, url, now - 1_000, true);
        assert_eq!(accept(&good, &advertiser_did, url, now), Ok(()));

        let self_attested = DialbackRecord::sign(&advertiser, &advertiser_did, url, now, true);
        assert_eq!(
            accept(&self_attested, &advertiser_did, url, now),
            Err(DialbackRefusal::SelfAttestation)
        );

        let failed = DialbackRecord::sign(&prober, &advertiser_did, url, now, false);
        assert_eq!(accept(&failed, &advertiser_did, url, now), Err(DialbackRefusal::ProbeFailed));

        let stale = DialbackRecord::sign(
            &prober,
            &advertiser_did,
            url,
            now - DIALBACK_MAX_AGE_MS - 1,
            true,
        );
        assert_eq!(accept(&stale, &advertiser_did, url, now), Err(DialbackRefusal::Stale));

        let future = DialbackRecord::sign(
            &prober,
            &advertiser_did,
            url,
            now + DIALBACK_FUTURE_SKEW_MS + 1,
            true,
        );
        assert_eq!(accept(&future, &advertiser_did, url, now), Err(DialbackRefusal::FromFuture));

        // A record minted for one requester cannot be replayed by another, nor for another URL.
        assert_eq!(accept(&good, &did(&stranger), url, now), Err(DialbackRefusal::WrongRequester));
        assert_eq!(
            accept(&good, &advertiser_did, "https://other.example.org", now),
            Err(DialbackRefusal::UrlMismatch)
        );

        // Tampering with a signed field breaks the signature.
        let mut tampered = good.clone();
        tampered.at_ms += 1;
        assert_eq!(accept(&tampered, &advertiser_did, url, now), Err(DialbackRefusal::BadSignature));
        let mut truncated = good;
        truncated.sig.truncate(10);
        assert_eq!(accept(&truncated, &advertiser_did, url, now), Err(DialbackRefusal::BadSignature));
    }
}
