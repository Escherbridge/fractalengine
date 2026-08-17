//! Per-lane domain-separated associated data for SPEC-6 stored artifacts (§9.1); see
//! `src/crypto/AGENTS.md` §artifact-aad.

use crate::cbor::{encode_canonical_checked, CborValue};
use crate::envelope::EncryptionParams;
use crate::segment::artifact::{LaneClass, SealedArtifact, ARTIFACT_FORMAT_VERSION};
use crate::signing::domain_preimage;

use super::aead::ciphertext_length_for;
use super::CryptoError;

/// The §2.1.2 outer metadata a stored artifact's AEAD binds, known before its ciphertext exists.
///
/// [`SealedArtifact::associated_data`] answers the same question for an artifact that already
/// holds its ciphertext, but sealing needs the preimage *first*: the ciphertext cannot exist
/// until the AAD it authenticates does. This type closes that ordering gap, and a test pins the
/// two constructions to the same bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredArtifactAad {
    /// Outer format version; always [`ARTIFACT_FORMAT_VERSION`].
    pub format_version: u64,
    /// Which of the four §2.1 lanes the sealed body belongs to.
    pub lane_class: LaneClass,
    /// Ciphertext length the sealed outer map will declare, tag included.
    pub ciphertext_length: u64,
    /// Suite, scope key identifier, and the fresh 24-byte nonce.
    pub encryption: EncryptionParams,
}

impl StoredArtifactAad {
    /// Binds a body of `plaintext_length` bytes, adding the Poly1305 tag the seal will append.
    pub const fn for_plaintext(
        lane_class: LaneClass,
        plaintext_length: usize,
        encryption: EncryptionParams,
    ) -> Self {
        Self::for_ciphertext(
            lane_class,
            ciphertext_length_for(plaintext_length) as u64,
            encryption,
        )
    }

    /// Binds a ciphertext whose exact stored length is already known.
    pub const fn for_ciphertext(
        lane_class: LaneClass,
        ciphertext_length: u64,
        encryption: EncryptionParams,
    ) -> Self {
        Self {
            format_version: ARTIFACT_FORMAT_VERSION,
            lane_class,
            ciphertext_length,
            encryption,
        }
    }

    /// The §3.1 header-segment lane: `ASCII("fe-segment-header-v1") || 0x00 || metadata`.
    pub const fn header_segment(plaintext_length: usize, encryption: EncryptionParams) -> Self {
        Self::for_plaintext(LaneClass::HeaderSegment, plaintext_length, encryption)
    }

    /// The §3.2 payload-shard lane: `ASCII("fe-segment-payload-shard-v1") || 0x00 || metadata`.
    pub const fn payload_shard(plaintext_length: usize, encryption: EncryptionParams) -> Self {
        Self::for_plaintext(LaneClass::PayloadShard, plaintext_length, encryption)
    }

    /// The §4.1 HashSeq-node lane: `ASCII("fe-segment-hashseq-v1") || 0x00 || metadata`.
    pub const fn hashseq_node(plaintext_length: usize, encryption: EncryptionParams) -> Self {
        Self::for_plaintext(LaneClass::HashSeqNode, plaintext_length, encryption)
    }

    /// The §3.3 segment-manifest lane: `ASCII("fe-segment-manifest-v1") || 0x00 || metadata`.
    pub const fn segment_manifest(plaintext_length: usize, encryption: EncryptionParams) -> Self {
        Self::for_plaintext(LaneClass::SegmentManifest, plaintext_length, encryption)
    }

    /// The metadata of an artifact that already carries its ciphertext.
    pub fn of_sealed(sealed: &SealedArtifact) -> Self {
        Self {
            format_version: sealed.format_version,
            lane_class: sealed.lane_class,
            ciphertext_length: sealed.ciphertext_length,
            encryption: *sealed.encryption.params(),
        }
    }

    /// The §2.1.2 outer map with the ciphertext (key 4) omitted.
    pub fn metadata_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.format_version)),
            (
                CborValue::Uint(1),
                CborValue::Uint(self.lane_class.to_wire()),
            ),
            (CborValue::Uint(2), CborValue::Uint(self.ciphertext_length)),
            (CborValue::Uint(3), self.encryption.to_cbor()),
        ])
    }

    /// `ASCII(lane domain) || 0x00 || canonical outer metadata` (§9.1).
    pub fn preimage(&self) -> Result<Vec<u8>, CryptoError> {
        let metadata = encode_canonical_checked(&self.metadata_cbor())?;
        Ok(domain_preimage(self.lane_class.aad_domain(), &metadata))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::crypto::aead::{open, seal, FreshNonce, SealingKey, SEALING_KEY_LENGTH};
    use crate::envelope::{Identifier32, NONCE_LENGTH, PRODUCTION_SUITE_ID};
    use crate::segment::artifact::EncryptionDescriptor;

    const EVERY_LANE: [LaneClass; 4] = [
        LaneClass::HeaderSegment,
        LaneClass::PayloadShard,
        LaneClass::HashSeqNode,
        LaneClass::SegmentManifest,
    ];

    fn key_id() -> Identifier32 {
        Identifier32([0x77; 32])
    }

    fn scope_key() -> SealingKey {
        SealingKey::from_bytes([0x5a; SEALING_KEY_LENGTH])
    }

    fn params() -> EncryptionParams {
        EncryptionParams {
            suite_id: PRODUCTION_SUITE_ID,
            key_id: key_id(),
            nonce: [0x88; NONCE_LENGTH],
        }
    }

    #[test]
    fn every_lane_prefixes_its_own_nul_terminated_domain() {
        for lane in EVERY_LANE {
            let aad = StoredArtifactAad::for_plaintext(lane, 32, params());
            let preimage = aad.preimage().expect("preimage");
            let domain = lane.aad_domain();
            assert!(domain.ends_with(&[0]), "{lane:?} domain lacks its NUL byte");
            assert!(preimage.starts_with(domain), "{lane:?} preimage prefix");
        }
    }

    #[test]
    fn no_lane_domain_is_a_prefix_of_another() {
        for outer in EVERY_LANE {
            for inner in EVERY_LANE {
                if outer == inner {
                    continue;
                }
                assert!(
                    !outer.aad_domain().starts_with(inner.aad_domain()),
                    "{inner:?} is a prefix of {outer:?}"
                );
            }
        }
    }

    #[test]
    fn the_four_lanes_produce_four_distinct_preimages() {
        let mut seen = std::collections::BTreeSet::new();
        for lane in EVERY_LANE {
            let preimage = StoredArtifactAad::for_plaintext(lane, 32, params())
                .preimage()
                .expect("preimage");
            assert!(seen.insert(preimage), "{lane:?} collided with another lane");
        }
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn a_header_segment_aad_cannot_open_a_payload_shard_ciphertext() {
        let body = b"payload shard body";
        let nonce = FreshNonce::draw(key_id()).expect("nonce");
        let encryption = nonce.params();
        let shard_aad = StoredArtifactAad::payload_shard(body.len(), encryption)
            .preimage()
            .expect("shard aad");
        let sealed = seal(&scope_key(), nonce, &shard_aad, body).expect("seal");

        // The header lane binds the same metadata under a different domain, and the shard's
        // own ciphertext length: only the domain differs, and that alone must be fatal.
        let header_aad = StoredArtifactAad::for_ciphertext(
            LaneClass::HeaderSegment,
            sealed.ciphertext.len() as u64,
            encryption,
        )
        .preimage()
        .expect("header aad");
        assert_ne!(header_aad, shard_aad);
        assert_eq!(
            open(
                &scope_key(),
                &sealed.encryption,
                &header_aad,
                &sealed.ciphertext
            ),
            Err(CryptoError::AuthenticationFailed)
        );
        assert_eq!(
            open(
                &scope_key(),
                &sealed.encryption,
                &shard_aad,
                &sealed.ciphertext
            )
            .expect("open"),
            body.to_vec()
        );
    }

    #[test]
    fn a_different_declared_ciphertext_length_cannot_open_the_artifact() {
        let body = b"hashseq node body";
        let nonce = FreshNonce::draw(key_id()).expect("nonce");
        let encryption = nonce.params();
        let aad = StoredArtifactAad::hashseq_node(body.len(), encryption)
            .preimage()
            .expect("aad");
        let sealed = seal(&scope_key(), nonce, &aad, body).expect("seal");

        let lying = StoredArtifactAad::for_ciphertext(
            LaneClass::HashSeqNode,
            sealed.ciphertext.len() as u64 + 1,
            encryption,
        )
        .preimage()
        .expect("aad");
        assert_eq!(
            open(&scope_key(), &sealed.encryption, &lying, &sealed.ciphertext),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn the_preseal_construction_matches_the_segment_slices_sealed_artifact_aad() {
        for lane in EVERY_LANE {
            let body = b"sealed body bytes";
            let descriptor = EncryptionDescriptor(params());
            let sealed = SealedArtifact::seal(lane, descriptor, body.to_vec()).expect("sealed");

            assert_eq!(
                StoredArtifactAad::of_sealed(&sealed)
                    .preimage()
                    .expect("crypto aad"),
                sealed.associated_data().expect("segment aad"),
                "{lane:?} diverged from segment::artifact"
            );
            assert_eq!(
                StoredArtifactAad::for_ciphertext(lane, body.len() as u64, params()),
                StoredArtifactAad::of_sealed(&sealed)
            );
        }
    }

    #[test]
    fn for_plaintext_declares_the_length_the_seal_actually_produces() {
        let body = b"nineteen byte body!";
        let nonce = FreshNonce::draw(key_id()).expect("nonce");
        let encryption = nonce.params();
        let declared = StoredArtifactAad::segment_manifest(body.len(), encryption);
        let sealed = seal(
            &scope_key(),
            nonce,
            &declared.preimage().expect("aad"),
            body,
        )
        .expect("seal");
        assert_eq!(declared.ciphertext_length, sealed.ciphertext.len() as u64);
    }
}
