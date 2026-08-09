//! The §6 Manager+ suspect-window disavow: payload grammar and classification index.

use std::collections::BTreeMap;

use thiserror::Error;

use super::disavow_rescind::AuthorityLevel;
use super::lineage::CausalOperationView;
use super::{bytes_at, entry, require_numeric_keys, text_at, u16_at};
use crate::cbor::{decode_canonical, encode_canonical_checked, CborError, CborValue};
use crate::envelope::{EnvelopeError, Hash32, Hlc, Scope, UnsignedEnvelope};
use crate::signing::{verify_author_binding, SigningError};

const DISAVOW_CONTEXT: &str = "disavow_payload";

/// Every reason a disavow payload fails to decode or to satisfy §6.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DisavowError {
    /// A field was absent, mistyped, or the wrong width.
    #[error("disavow payload field: {0}")]
    Field(#[from] EnvelopeError),
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// `subject_did` did not bind to `subject_public_key`.
    #[error("subject DID does not bind to its public key: {0}")]
    SubjectBinding(SigningError),
    /// `last_hlc` was earlier than `first_hlc`.
    #[error("last_hlc {last:?} is earlier than first_hlc {first:?}")]
    InvertedWindow {
        /// Declared lower bound.
        first: Hlc,
        /// Declared upper bound.
        last: Hlc,
    },
    /// `affected_scope` was not the envelope scope or one of its descendants.
    #[error("affected_scope is not equal to or below the envelope scope")]
    AffectedScopeOutsideEnvelopeScope,
}

/// The §6.2 six-key disavow payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisavowPayload {
    /// Canonical DID of the suspect key.
    pub subject_did: String,
    /// Public key the subject DID encodes.
    pub subject_public_key: [u8; 32],
    /// Scope the window applies to; equal to or below the envelope scope.
    pub affected_scope: Scope,
    /// Inclusive lower bound of the suspect window.
    pub first_hlc: Hlc,
    /// Inclusive upper bound of the suspect window.
    pub last_hlc: Hlc,
    /// Registered reason code; never free-form sensitive text.
    pub reason_code: u16,
}

impl DisavowPayload {
    /// Encodes the §6.2 six-key map.
    pub fn to_cbor(&self) -> Result<CborValue, DisavowError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::text(&self.subject_did)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.subject_public_key.to_vec()),
            ),
            (CborValue::Uint(2), self.affected_scope.to_cbor()?),
            (CborValue::Uint(3), self.first_hlc.to_cbor()),
            (CborValue::Uint(4), self.last_hlc.to_cbor()),
            (
                CborValue::Uint(5),
                CborValue::Uint(u64::from(self.reason_code)),
            ),
        ]))
    }

    /// Decodes the §6.2 six-key map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, DisavowError> {
        require_numeric_keys(value, 6, DISAVOW_CONTEXT)?;
        Ok(Self {
            subject_did: text_at(value, 0, DISAVOW_CONTEXT)?,
            subject_public_key: bytes_at::<32>(value, 1, DISAVOW_CONTEXT)?,
            affected_scope: Scope::from_cbor(entry(value, 2, DISAVOW_CONTEXT)?)?,
            first_hlc: Hlc::from_cbor(entry(value, 3, DISAVOW_CONTEXT)?)?,
            last_hlc: Hlc::from_cbor(entry(value, 4, DISAVOW_CONTEXT)?)?,
            reason_code: u16_at(value, 5, DISAVOW_CONTEXT)?,
        })
    }

    /// Encodes to canonical CBOR bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, DisavowError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, DisavowError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }

    /// Enforces the §6.2 rules a payload can check on its own.
    pub fn validate(&self) -> Result<(), DisavowError> {
        verify_author_binding(&self.subject_did, &self.subject_public_key)
            .map_err(DisavowError::SubjectBinding)?;
        if self.last_hlc < self.first_hlc {
            return Err(DisavowError::InvertedWindow {
                first: self.first_hlc,
                last: self.last_hlc,
            });
        }
        Ok(())
    }

    /// Enforces §6.2 key 2 against the enclosing envelope of a RECEIVED payload.
    pub fn validate_against_envelope(
        &self,
        envelope: &UnsignedEnvelope,
    ) -> Result<(), DisavowError> {
        self.validate()?;
        if !envelope.scope.contains(&self.affected_scope) {
            return Err(DisavowError::AffectedScopeOutsideEnvelopeScope);
        }
        Ok(())
    }

    /// Reports whether `subject` falls inside this suspect window (§6.3).
    pub fn matches(&self, subject: &DisavowSubject) -> bool {
        self.subject_public_key == subject.author_public_key
            && self.affected_scope.contains(&subject.scope)
            && subject.hlc >= self.first_hlc
            && subject.hlc <= self.last_hlc
    }
}

/// The operation facts §6.3 matching needs; the scope is carried, never re-derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisavowSubject {
    /// Author key that signed the operation.
    pub author_public_key: [u8; 32],
    /// Operation scope.
    pub scope: Scope,
    /// Operation wire HLC, never the packed local-storage integer.
    pub hlc: Hlc,
}

impl DisavowSubject {
    /// Reads the matching facts out of an operation envelope.
    pub fn from_envelope(envelope: &UnsignedEnvelope) -> Self {
        Self {
            author_public_key: envelope.author.public_key,
            scope: envelope.scope,
            hlc: envelope.hlc,
        }
    }
}

/// One admitted disavow plus the authority its issuer held, which §6.1 rescind checks need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisavowRecord {
    /// The admitted payload.
    pub payload: DisavowPayload,
    /// Authority the issuer held at the disavow's parent-induced epoch.
    pub issuer_authority: AuthorityLevel,
}

/// §6 classification index. It classifies operations and never deletes or rewrites evidence.
#[derive(Clone, Debug, Default)]
pub struct DisavowIndex {
    disavows: BTreeMap<Hash32, DisavowRecord>,
    rescinds: BTreeMap<Hash32, Hash32>,
}

impl DisavowIndex {
    /// Builds an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an admitted disavow, returning any record it replaced.
    pub fn record(
        &mut self,
        op_id: Hash32,
        payload: DisavowPayload,
        issuer_authority: AuthorityLevel,
    ) -> Option<DisavowRecord> {
        self.disavows.insert(
            op_id,
            DisavowRecord {
                payload,
                issuer_authority,
            },
        )
    }

    /// Records an admitted rescind of `disavow_op_id`.
    pub fn record_rescind(&mut self, rescind_op_id: Hash32, disavow_op_id: Hash32) {
        self.rescinds.insert(rescind_op_id, disavow_op_id);
    }

    /// The disavow recorded under `op_id`, if any.
    pub fn disavow(&self, op_id: Hash32) -> Option<&DisavowRecord> {
        self.disavows.get(&op_id)
    }

    /// Number of recorded disavows.
    pub fn len(&self) -> usize {
        self.disavows.len()
    }

    /// Reports whether no disavow is recorded.
    pub fn is_empty(&self) -> bool {
        self.disavows.is_empty()
    }

    /// Removes a disavow and every rescind naming it, for the §3.4 equivocation rule.
    pub fn retract(&mut self, op_id: Hash32) -> bool {
        self.rescinds
            .retain(|rescind_op_id, target| *rescind_op_id != op_id && *target != op_id);
        self.disavows.remove(&op_id).is_some()
    }

    /// Every admitted disavow matching `subject`, in `op_id` order (§6.4 monotonic union).
    pub fn matching_disavows(&self, subject: &DisavowSubject) -> Vec<Hash32> {
        self.disavows
            .iter()
            .filter(|(_, record)| record.payload.matches(subject))
            .map(|(op_id, _)| *op_id)
            .collect()
    }

    /// Reports whether any admitted disavow matches `subject`.
    pub fn is_disavowed(&self, subject: &DisavowSubject) -> bool {
        !self.matching_disavows(subject).is_empty()
    }

    /// Matching disavows that `causal_point` reaches and whose §6.1 rescind it does not.
    ///
    /// Classification is causal: a disavow the replaying head does not reach has no effect,
    /// and a rescind takes effect only in its own causal future.
    pub fn matching_disavows_at(
        &self,
        causal_point: Hash32,
        view: &impl CausalOperationView,
        subject: &DisavowSubject,
    ) -> Vec<Hash32> {
        self.matching_disavows(subject)
            .into_iter()
            .filter(|disavow_op_id| view.reaches(causal_point, *disavow_op_id))
            .filter(|disavow_op_id| {
                !self.rescinds.iter().any(|(rescind_op_id, target)| {
                    target == disavow_op_id && view.reaches(causal_point, *rescind_op_id)
                })
            })
            .collect()
    }

    /// Reports whether `subject` is disavowed as seen from `causal_point`.
    pub fn is_disavowed_at(
        &self,
        causal_point: Hash32,
        view: &impl CausalOperationView,
        subject: &DisavowSubject,
    ) -> bool {
        !self
            .matching_disavows_at(causal_point, view, subject)
            .is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author_key::test_support::{
        hash, identifier, petal_scope, public_key, resource_scope, verse_scope, FakeDag,
    };
    use crate::did_key::public_key_to_did;

    fn payload(subject: u8, scope: Scope, first: Hlc, last: Hlc) -> DisavowPayload {
        DisavowPayload {
            subject_did: public_key_to_did(&public_key(subject)),
            subject_public_key: public_key(subject),
            affected_scope: scope,
            first_hlc: first,
            last_hlc: last,
            reason_code: 3,
        }
    }

    fn subject(key: u8, scope: Scope, hlc: Hlc) -> DisavowSubject {
        DisavowSubject {
            author_public_key: public_key(key),
            scope,
            hlc,
        }
    }

    #[test]
    fn disavow_payload_round_trips_canonical_bytes_byte_exactly() {
        let payload = payload(1, petal_scope(), Hlc::new(10, 0), Hlc::new(20, 5));
        let bytes = payload.encode_canonical().expect("encode");
        let decoded = DisavowPayload::decode_canonical(&bytes).expect("decode");
        assert_eq!(decoded, payload);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
    }

    #[test]
    fn disavow_payload_rejects_non_canonical_input() {
        let payload = payload(1, petal_scope(), Hlc::new(10, 0), Hlc::new(20, 5));
        let canonical = payload.encode_canonical().expect("encode");

        let mut trailing = canonical.clone();
        trailing.push(0x00);
        assert!(matches!(
            DisavowPayload::decode_canonical(&trailing),
            Err(DisavowError::Cbor(CborError::TrailingBytes { .. }))
        ));

        // `reason_code` 3 re-encoded in a one-byte argument instead of the minimal head byte.
        let mut non_minimal = Vec::new();
        for (index, byte) in canonical.iter().enumerate() {
            if index + 1 == canonical.len() {
                non_minimal.push(0x18);
            }
            non_minimal.push(*byte);
        }
        assert_eq!(
            canonical.last().copied(),
            Some(0x03),
            "the final byte is the minimally encoded reason_code"
        );
        assert!(matches!(
            DisavowPayload::decode_canonical(&non_minimal),
            Err(DisavowError::Cbor(CborError::NonMinimalArgument { .. }))
        ));
    }

    #[test]
    fn spec2_case_09_an_invalid_hlc_range_is_rejected() {
        let inverted = payload(1, petal_scope(), Hlc::new(20, 0), Hlc::new(10, 0));
        assert_eq!(
            inverted.validate(),
            Err(DisavowError::InvertedWindow {
                first: Hlc::new(20, 0),
                last: Hlc::new(10, 0),
            })
        );

        let inverted_counter = payload(1, petal_scope(), Hlc::new(20, 4), Hlc::new(20, 3));
        assert!(matches!(
            inverted_counter.validate(),
            Err(DisavowError::InvertedWindow { .. })
        ));

        let single_instant = payload(1, petal_scope(), Hlc::new(20, 4), Hlc::new(20, 4));
        assert_eq!(single_instant.validate(), Ok(()));
    }

    #[test]
    fn spec2_case_09_a_wrong_scope_disavow_is_rejected() {
        use crate::author_key::test_support::{intent_envelope, schemas};

        let envelope = intent_envelope(
            public_key(4),
            petal_scope(),
            identifier(0x55),
            vec![hash(0x01)],
            0,
            schemas().disavow,
            Hlc::new(50, 0),
        );
        let widening = payload(1, verse_scope(), Hlc::new(10, 0), Hlc::new(20, 0));
        assert_eq!(
            widening.validate_against_envelope(&envelope),
            Err(DisavowError::AffectedScopeOutsideEnvelopeScope)
        );

        let narrowing = payload(1, resource_scope(), Hlc::new(10, 0), Hlc::new(20, 0));
        assert_eq!(narrowing.validate_against_envelope(&envelope), Ok(()));
    }

    #[test]
    fn a_subject_did_that_does_not_bind_to_its_key_is_rejected() {
        let mut mismatched = payload(1, petal_scope(), Hlc::new(10, 0), Hlc::new(20, 0));
        mismatched.subject_did = public_key_to_did(&public_key(9));
        assert_eq!(
            mismatched.validate(),
            Err(DisavowError::SubjectBinding(
                SigningError::AuthorBindingMismatch
            ))
        );
    }

    #[test]
    fn spec2_case_09_a_bounded_window_marks_only_the_intended_key_scope_and_hlc() {
        let mut index = DisavowIndex::new();
        index.record(
            hash(0x0a),
            payload(1, petal_scope(), Hlc::new(10, 0), Hlc::new(20, 5)),
            AuthorityLevel::Manager,
        );

        assert!(index.is_disavowed(&subject(1, petal_scope(), Hlc::new(10, 0))));
        assert!(index.is_disavowed(&subject(1, petal_scope(), Hlc::new(20, 5))));
        assert!(
            index.is_disavowed(&subject(1, resource_scope(), Hlc::new(15, 0))),
            "a descendant scope of the affected scope matches"
        );

        assert!(
            !index.is_disavowed(&subject(2, petal_scope(), Hlc::new(15, 0))),
            "another key never matches"
        );
        assert!(
            !index.is_disavowed(&subject(1, verse_scope(), Hlc::new(15, 0))),
            "an ancestor scope of the affected scope never matches"
        );
        assert!(
            !index.is_disavowed(&subject(1, petal_scope(), Hlc::new(9, 999))),
            "an HLC before the window never matches"
        );
        assert!(
            !index.is_disavowed(&subject(1, petal_scope(), Hlc::new(20, 6))),
            "an HLC after the window never matches"
        );
    }

    #[test]
    fn overlapping_disavows_union_their_matching_sets() {
        let mut index = DisavowIndex::new();
        index.record(
            hash(0x0a),
            payload(1, petal_scope(), Hlc::new(10, 0), Hlc::new(20, 0)),
            AuthorityLevel::Manager,
        );
        index.record(
            hash(0x0b),
            payload(1, petal_scope(), Hlc::new(15, 0), Hlc::new(30, 0)),
            AuthorityLevel::Manager,
        );

        assert_eq!(
            index.matching_disavows(&subject(1, petal_scope(), Hlc::new(17, 0))),
            vec![hash(0x0a), hash(0x0b)]
        );
        assert_eq!(
            index.matching_disavows(&subject(1, petal_scope(), Hlc::new(25, 0))),
            vec![hash(0x0b)]
        );
        assert_eq!(index.len(), 2);
        assert!(!index.is_empty());
    }

    #[test]
    fn spec2_case_14_a_rescind_only_takes_effect_in_its_causal_future() {
        let mut dag = FakeDag::default();
        dag.insert(hash(0x01), vec![]);
        dag.insert(hash(0x0a), vec![hash(0x01)]);
        dag.insert(hash(0x0c), vec![hash(0x0a)]);

        let mut index = DisavowIndex::new();
        index.record(
            hash(0x0a),
            payload(1, petal_scope(), Hlc::new(10, 0), Hlc::new(20, 0)),
            AuthorityLevel::Manager,
        );
        index.record_rescind(hash(0x0c), hash(0x0a));

        let matched = subject(1, petal_scope(), Hlc::new(15, 0));
        assert!(
            index.is_disavowed_at(hash(0x0a), &dag, &matched),
            "a causal point before the rescind still classifies the operation as disavowed"
        );
        assert!(
            !index.is_disavowed_at(hash(0x0c), &dag, &matched),
            "the rescind removes the projection effect in its causal future"
        );
        assert!(
            index.is_disavowed(&matched),
            "the disavow evidence itself is retained, never deleted"
        );
        assert!(index.disavow(hash(0x0a)).is_some());
    }

    #[test]
    fn retracting_a_disavow_removes_its_classification_and_its_rescinds() {
        let mut index = DisavowIndex::new();
        index.record(
            hash(0x0a),
            payload(1, petal_scope(), Hlc::new(10, 0), Hlc::new(20, 0)),
            AuthorityLevel::Manager,
        );
        index.record_rescind(hash(0x0c), hash(0x0a));

        assert!(index.retract(hash(0x0a)));
        assert!(index.is_empty());
        assert!(!index.is_disavowed(&subject(1, petal_scope(), Hlc::new(15, 0))));
        assert!(!index.retract(hash(0x0a)));
    }
}
