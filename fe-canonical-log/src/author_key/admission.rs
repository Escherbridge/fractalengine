//! SPEC-2 admission entrypoints: what a materializer calls after SPEC-1 envelope admission.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use super::continuity_grant::{
    AttributionIndex, AttributionLink, ContinuityGrantError, ContinuityGrantPayload,
};
use super::disavow::{DisavowError, DisavowIndex, DisavowPayload, DisavowSubject};
use super::disavow_rescind::{
    validate_rescind_authority, AuthorityLevel, RescindAuthorityError, RescindError, RescindPayload,
};
use super::lineage::{
    CausalOperationView, LineageError, LineageIndex, RotationOutcome, RotationRecord,
};
use super::payloads::{RotationPayload, RotationPayloadError, ROTATION_OPERATION_KIND};
use super::rotation_proof::{verify_rotation_proof, RotationProofError};
use crate::envelope::{EquivocationKey, Hash32, Scope, UnsignedEnvelope};

/// Whether the caller could resolve the author's capability at the operation's epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationState {
    /// The author holds a valid capability at the parent-induced epoch.
    Authorized,
    /// The author holds no valid capability at that epoch.
    Unauthorized,
    /// The capability chain artifact is not available yet.
    CapabilityChainUnavailable,
    /// The scope-epoch state is not available yet.
    EpochStateUnavailable,
}

/// Whether the caller could resolve the author's authority level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityState {
    /// The level resolved.
    Resolved(AuthorityLevel),
    /// The authorization view is not available yet.
    Unavailable,
}

/// The SPEC-3 answers SPEC-2 admission needs, without depending on SPEC-3's concrete types.
pub trait CapabilityAuthorityView {
    /// Whether `author_public_key` is authorized at `scope` for `scope_epoch`.
    fn predecessor_authorized(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorizationState;

    /// The authority level `author_public_key` holds at `scope` for `scope_epoch`.
    fn issuer_role(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorityState;

    /// Whether `author_public_key` is the Owner at `scope` for `scope_epoch`.
    fn issuer_is_owner(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        scope_epoch: u64,
    ) -> bool {
        matches!(
            self.issuer_role(author_public_key, scope, scope_epoch),
            AuthorityState::Resolved(AuthorityLevel::Owner)
        )
    }
}

/// Schema hashes for the four SPEC-2 payload schemas.
///
/// There is no `Default`: schema identity is a registry fact the caller supplies, never a
/// number this crate invents (D-CL24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredSchemas {
    /// `author-key-rotation-v1`.
    pub rotation: Hash32,
    /// `manager-suspect-key-disavow-v1`.
    pub disavow: Hash32,
    /// `manager-suspect-key-disavow-rescind-v1`.
    pub disavow_rescind: Hash32,
    /// `owner-continuity-grant-v1`.
    pub continuity_grant: Hash32,
}

/// Whether the caller obtained the decrypted payload, and if not, why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadAvailability<'a> {
    /// The decrypted, schema-validated canonical payload bytes.
    Available(&'a [u8]),
    /// The registered schema for this operation could not be resolved.
    SchemaUnavailable,
    /// The scope payload key could not be resolved, so the payload stayed encrypted.
    PayloadKeyUnavailable,
}

/// Why an operation is held rather than applied. Every variant this module reaches is a
/// retryable availability state or the §3.4 equivocation stand-off; none of them provisionally
/// applies anything.
///
/// The crate's single quarantine vocabulary, shared with SPEC-5 pre-admission holding and SPEC-6
/// receipt refusal; see `src/AGENTS.md` §unified-vocabularies. Quarantine bounds are SPEC-5's,
/// not this module's: nothing here holds a duration or a cap.
pub use crate::compose::QuarantineReason;

/// What SPEC-2 admission decided about one operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionOutcome {
    /// The operation applied to the SPEC-2 state.
    Admitted,
    /// §4.6 rotation fork: the record is retained and neither successor becomes active.
    ForkRetained {
        /// Every rotation in the fork group, in `op_id` order.
        conflicting_op_ids: Vec<Hash32>,
    },
    /// The operation is held; see the reason.
    Quarantined(QuarantineReason),
    /// This exact `op_id` was already presented and applied.
    AlreadyAdmitted,
}

/// Every reason SPEC-2 admission rejects an operation outright.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AdmissionError {
    /// A lifecycle operation was not a normal encrypted intent (kind 1).
    #[error("a lifecycle operation must be operation_kind 1, found {operation_kind}")]
    WrongOperationKind {
        /// The kind actually present.
        operation_kind: u16,
    },
    /// The envelope named a different registered schema than the entrypoint handles.
    #[error("envelope schema {found:?} is not the expected lifecycle schema {expected:?}")]
    WrongSchema {
        /// Schema the entrypoint handles.
        expected: Hash32,
        /// Schema the envelope named.
        found: Hash32,
    },
    /// The rotation payload was malformed or disagreed with its envelope.
    #[error("{0}")]
    RotationPayload(#[from] RotationPayloadError),
    /// The §3.2 dual proof failed.
    #[error("{0}")]
    RotationProof(#[from] RotationProofError),
    /// The §4 lineage refused the rotation.
    #[error("{0}")]
    Lineage(#[from] LineageError),
    /// The disavow payload was malformed or disagreed with its envelope.
    #[error("{0}")]
    Disavow(#[from] DisavowError),
    /// The rescind payload was malformed.
    #[error("{0}")]
    Rescind(#[from] RescindError),
    /// The rescind lacked strictly higher authority.
    #[error("{0}")]
    RescindAuthority(#[from] RescindAuthorityError),
    /// The continuity grant was malformed or its countersignature failed.
    #[error("{0}")]
    ContinuityGrant(#[from] ContinuityGrantError),
    /// The rotation predecessor held no valid capability at the parent-induced epoch.
    #[error("the rotation predecessor is not authorized at the parent-induced epoch")]
    PredecessorUnauthorized,
    /// The rotation predecessor is semantically disavowed at the parent-induced state.
    #[error("the rotation predecessor is disavowed by {disavow_op_ids:?}")]
    PredecessorDisavowed {
        /// Disavows that matched.
        disavow_op_ids: Vec<Hash32>,
    },
    /// A disavow or rescind issuer held less than Manager authority.
    #[error("a disavow requires Manager+ authority, issuer holds {issuer_authority:?}")]
    IssuerBelowManager {
        /// The level actually resolved.
        issuer_authority: AuthorityLevel,
    },
    /// A continuity grant's envelope was not signed by the Owner.
    #[error("a continuity grant must be signed by the Owner in its envelope")]
    ContinuityGrantNotOwnerSigned,
}

/// The SPEC-2 state one replica maintains. All of it is in memory; storage is the caller's.
#[derive(Clone, Debug, Default)]
pub struct AuthorKeyState {
    /// §4 lineage of admitted rotations.
    pub lineage: LineageIndex,
    /// §6 suspect-window classification.
    pub disavows: DisavowIndex,
    /// §9 attribution links, which never affect the two above.
    pub attributions: AttributionIndex,
    applied: BTreeSet<Hash32>,
    observed: BTreeMap<EquivocationKey, Hash32>,
    quarantined: BTreeMap<Hash32, QuarantineReason>,
    equivocation_locked: BTreeSet<Hash32>,
}

impl AuthorKeyState {
    /// Builds empty SPEC-2 state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The reason `op_id` is held, if it is.
    pub fn quarantine_reason(&self, op_id: Hash32) -> Option<&QuarantineReason> {
        self.quarantined.get(&op_id)
    }

    /// Reports whether `op_id` is held.
    pub fn is_quarantined(&self, op_id: Hash32) -> bool {
        self.quarantined.contains_key(&op_id)
    }

    /// Reports whether `op_id` applied to SPEC-2 state.
    pub fn is_applied(&self, op_id: Hash32) -> bool {
        self.applied.contains(&op_id)
    }

    /// Records that `op_id` applied and now owns its §3.4 identity.
    fn mark_applied(&mut self, op_id: Hash32, equivocation_key: EquivocationKey) {
        self.applied.insert(op_id);
        self.observed.insert(equivocation_key, op_id);
        self.quarantined.remove(&op_id);
    }

    /// Holds `op_id` for a retryable availability state without touching applied state.
    fn hold(&mut self, op_id: Hash32, reason: QuarantineReason) -> AdmissionOutcome {
        self.quarantined.insert(op_id, reason.clone());
        AdmissionOutcome::Quarantined(reason)
    }

    /// Holds `op_id` permanently for §3.4 equivocation and withdraws every effect it had.
    ///
    /// Only an authorized resolution operation releases this; re-presenting the bytes must
    /// not resurrect the candidate.
    fn lock_for_equivocation(&mut self, op_id: Hash32, reason: QuarantineReason) {
        self.lineage.retract_rotation(op_id);
        self.disavows.retract(op_id);
        self.attributions.retract(op_id);
        self.applied.remove(&op_id);
        self.quarantined.insert(op_id, reason);
        self.equivocation_locked.insert(op_id);
    }
}

/// The checks every SPEC-2 lifecycle operation shares, in order.
///
/// Returns `Ok(Err(outcome))` when the operation is held or already known, and
/// `Ok(Ok(payload_bytes))` when it may proceed.
#[allow(clippy::type_complexity)]
fn begin_admission<'a>(
    state: &mut AuthorKeyState,
    causal: &impl CausalOperationView,
    envelope: &UnsignedEnvelope,
    op_id: Hash32,
    expected_schema: Hash32,
    payload: PayloadAvailability<'a>,
) -> Result<Result<&'a [u8], AdmissionOutcome>, AdmissionError> {
    if envelope.operation_kind != ROTATION_OPERATION_KIND {
        return Err(AdmissionError::WrongOperationKind {
            operation_kind: envelope.operation_kind,
        });
    }
    if envelope.schema_hash != expected_schema {
        return Err(AdmissionError::WrongSchema {
            expected: expected_schema,
            found: envelope.schema_hash,
        });
    }

    if state.equivocation_locked.contains(&op_id) {
        let reason = state
            .quarantined
            .get(&op_id)
            .cloned()
            .expect("an equivocation-locked operation always carries its reason");
        return Ok(Err(AdmissionOutcome::Quarantined(reason)));
    }
    if state.applied.contains(&op_id) {
        return Ok(Err(AdmissionOutcome::AlreadyAdmitted));
    }
    let equivocation_key = envelope.equivocation_key();
    if let Some(existing) = state.observed.get(&equivocation_key).copied() {
        if existing != op_id {
            // Both candidates are locked under ONE reason naming both operations: an asymmetric
            // "the other one" reason invited a reader to treat the named side as the loser.
            let reason = QuarantineReason::AuthorEquivocation {
                equivocation_key,
                first_op_id: existing,
                second_op_id: op_id,
            };
            state.lock_for_equivocation(existing, reason.clone());
            state.lock_for_equivocation(op_id, reason.clone());
            return Ok(Err(AdmissionOutcome::Quarantined(reason)));
        }
    }

    for parent_op_id in &envelope.parents {
        if causal.parents(*parent_op_id).is_none() {
            return Ok(Err(state.hold(
                op_id,
                QuarantineReason::MissingParent {
                    missing_parents: vec![*parent_op_id],
                },
            )));
        }
        if !causal.is_admitted(*parent_op_id) {
            return Ok(Err(state.hold(
                op_id,
                QuarantineReason::ParentNotAdmitted {
                    parent_op_id: *parent_op_id,
                },
            )));
        }
    }

    match payload {
        PayloadAvailability::Available(bytes) => Ok(Ok(bytes)),
        PayloadAvailability::SchemaUnavailable => Ok(Err(state.hold(
            op_id,
            QuarantineReason::UnknownSchema {
                schema_hash: envelope.schema_hash,
            },
        ))),
        PayloadAvailability::PayloadKeyUnavailable => Ok(Err(
            state.hold(op_id, QuarantineReason::PayloadKeyUnavailable)
        )),
    }
}

/// Maps an unavailable authorization answer to its quarantine reason.
fn availability_quarantine(authorization: AuthorizationState) -> Option<QuarantineReason> {
    match authorization {
        AuthorizationState::CapabilityChainUnavailable => {
            Some(QuarantineReason::CapabilityChainUnavailable)
        }
        AuthorizationState::EpochStateUnavailable => Some(QuarantineReason::EpochStateUnavailable),
        AuthorizationState::Authorized | AuthorizationState::Unauthorized => None,
    }
}

/// Admits a §3 key rotation after SPEC-1 envelope admission has already succeeded.
///
/// Only the envelope author key and the successor proof decide the outcome; no transport
/// identity is an input here, per §2.4.
pub fn admit_rotation(
    state: &mut AuthorKeyState,
    causal: &impl CausalOperationView,
    authority: &impl CapabilityAuthorityView,
    schemas: &RegisteredSchemas,
    envelope: &UnsignedEnvelope,
    op_id: Hash32,
    payload: PayloadAvailability<'_>,
) -> Result<AdmissionOutcome, AdmissionError> {
    let payload_bytes =
        match begin_admission(state, causal, envelope, op_id, schemas.rotation, payload)? {
            Ok(bytes) => bytes,
            Err(outcome) => return Ok(outcome),
        };

    let rotation = RotationPayload::decode_canonical(payload_bytes)?;
    rotation.validate_against_envelope(envelope)?;
    verify_rotation_proof(&rotation)?;

    let authorization = authority.predecessor_authorized(
        &envelope.author.public_key,
        &envelope.scope,
        envelope.capability.scope_epoch,
    );
    if let Some(reason) = availability_quarantine(authorization) {
        return Ok(state.hold(op_id, reason));
    }
    if authorization == AuthorizationState::Unauthorized {
        return Err(AdmissionError::PredecessorUnauthorized);
    }

    let parent_op_id = rotation.statement.parent_op_id;
    let disavow_op_ids = state.disavows.matching_disavows_at(
        parent_op_id,
        causal,
        &DisavowSubject::from_envelope(envelope),
    );
    if !disavow_op_ids.is_empty() {
        return Err(AdmissionError::PredecessorDisavowed { disavow_op_ids });
    }

    let record = RotationRecord {
        op_id,
        scope: envelope.scope,
        parent_op_id,
        predecessor_public_key: rotation.statement.predecessor_public_key,
        successor_public_key: rotation.statement.successor_public_key,
    };
    let outcome = match state.lineage.record_rotation(record, causal)? {
        RotationOutcome::Applied => AdmissionOutcome::Admitted,
        RotationOutcome::Fork { conflicting_op_ids } => {
            AdmissionOutcome::ForkRetained { conflicting_op_ids }
        }
    };
    state.mark_applied(op_id, envelope.equivocation_key());
    Ok(outcome)
}

/// Admits a §6 Manager+ suspect-window disavow.
pub fn admit_disavow(
    state: &mut AuthorKeyState,
    causal: &impl CausalOperationView,
    authority: &impl CapabilityAuthorityView,
    schemas: &RegisteredSchemas,
    envelope: &UnsignedEnvelope,
    op_id: Hash32,
    payload: PayloadAvailability<'_>,
) -> Result<AdmissionOutcome, AdmissionError> {
    let payload_bytes =
        match begin_admission(state, causal, envelope, op_id, schemas.disavow, payload)? {
            Ok(bytes) => bytes,
            Err(outcome) => return Ok(outcome),
        };

    let disavow = DisavowPayload::decode_canonical(payload_bytes)?;
    disavow.validate_against_envelope(envelope)?;

    let issuer_authority = match resolve_issuer(state, authority, envelope, op_id) {
        Ok(level) => level,
        Err(outcome) => return Ok(outcome),
    };
    if issuer_authority < AuthorityLevel::Manager {
        return Err(AdmissionError::IssuerBelowManager { issuer_authority });
    }

    state.disavows.record(op_id, disavow, issuer_authority);
    state.mark_applied(op_id, envelope.equivocation_key());
    Ok(AdmissionOutcome::Admitted)
}

/// Admits a §6.1 rescind of exactly one admitted disavow.
pub fn admit_disavow_rescind(
    state: &mut AuthorKeyState,
    causal: &impl CausalOperationView,
    authority: &impl CapabilityAuthorityView,
    schemas: &RegisteredSchemas,
    envelope: &UnsignedEnvelope,
    op_id: Hash32,
    payload: PayloadAvailability<'_>,
) -> Result<AdmissionOutcome, AdmissionError> {
    let payload_bytes = match begin_admission(
        state,
        causal,
        envelope,
        op_id,
        schemas.disavow_rescind,
        payload,
    )? {
        Ok(bytes) => bytes,
        Err(outcome) => return Ok(outcome),
    };

    let rescind = RescindPayload::decode_canonical(payload_bytes)?;
    let original_issuer = match state.disavows.disavow(rescind.disavow_op_id) {
        Some(record) => record.issuer_authority,
        None => {
            return Ok(state.hold(
                op_id,
                QuarantineReason::ReferencedDisavowUnavailable {
                    disavow_op_id: rescind.disavow_op_id,
                },
            ))
        }
    };

    let rescinder = match resolve_issuer(state, authority, envelope, op_id) {
        Ok(level) => level,
        Err(outcome) => return Ok(outcome),
    };
    validate_rescind_authority(rescinder, original_issuer)?;

    state.disavows.record_rescind(op_id, rescind.disavow_op_id);
    state.mark_applied(op_id, envelope.equivocation_key());
    Ok(AdmissionOutcome::Admitted)
}

/// Admits a §9 Owner-countersigned continuity grant as an attribution link.
///
/// It touches only [`AuthorKeyState::attributions`]: §9.3 forbids any lineage, capability, or
/// disavow effect.
pub fn admit_continuity_grant(
    state: &mut AuthorKeyState,
    causal: &impl CausalOperationView,
    authority: &impl CapabilityAuthorityView,
    schemas: &RegisteredSchemas,
    envelope: &UnsignedEnvelope,
    op_id: Hash32,
    payload: PayloadAvailability<'_>,
) -> Result<AdmissionOutcome, AdmissionError> {
    let payload_bytes = match begin_admission(
        state,
        causal,
        envelope,
        op_id,
        schemas.continuity_grant,
        payload,
    )? {
        Ok(bytes) => bytes,
        Err(outcome) => return Ok(outcome),
    };

    let grant = ContinuityGrantPayload::decode_canonical(payload_bytes)?;
    grant.validate_against_envelope(envelope)?;

    if !authority.issuer_is_owner(
        &envelope.author.public_key,
        &envelope.scope,
        envelope.capability.scope_epoch,
    ) {
        return Err(AdmissionError::ContinuityGrantNotOwnerSigned);
    }

    state.attributions.record(AttributionLink {
        grant_op_id: op_id,
        verse_scope: grant.statement.verse_scope,
        lost_principal_public_key: grant.statement.lost_principal_public_key,
        new_principal_public_key: grant.statement.new_principal_public_key,
    });
    state.mark_applied(op_id, envelope.equivocation_key());
    Ok(AdmissionOutcome::Admitted)
}

/// Resolves the issuer's authority level, quarantining when the view cannot answer.
fn resolve_issuer(
    state: &mut AuthorKeyState,
    authority: &impl CapabilityAuthorityView,
    envelope: &UnsignedEnvelope,
    op_id: Hash32,
) -> Result<AuthorityLevel, AdmissionOutcome> {
    match authority.issuer_role(
        &envelope.author.public_key,
        &envelope.scope,
        envelope.capability.scope_epoch,
    ) {
        AuthorityState::Resolved(level) => Ok(level),
        AuthorityState::Unavailable => {
            Err(state.hold(op_id, QuarantineReason::CapabilityChainUnavailable))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author_key::continuity_grant::{sign_continuity_grant, ContinuityGrantStatement};
    use crate::author_key::lineage::KeyState;
    use crate::author_key::payloads::{RotationStatement, ROTATION_PROTOCOL_VERSION};
    use crate::author_key::rotation_proof::sign_successor_proof;
    use crate::author_key::test_support::{
        hash, identifier, intent_envelope, petal_scope, public_key, schemas, signing_key,
        verse_scope, FakeAuthority, FakeDag,
    };
    use crate::did_key::public_key_to_did;
    use crate::envelope::{Hlc, Identifier32};

    const BRANCH: u8 = 0x55;

    /// `genesis(0x01) -> 0x02 -> 0x03`, all admitted.
    fn dag() -> FakeDag {
        let mut dag = FakeDag::default();
        dag.insert(hash(0x01), vec![]);
        dag.insert(hash(0x02), vec![hash(0x01)]);
        dag.insert(hash(0x03), vec![hash(0x02)]);
        dag
    }

    fn seeded_state() -> AuthorKeyState {
        let mut state = AuthorKeyState::new();
        state
            .lineage
            .register_root_key(verse_scope(), public_key(1));
        state
    }

    fn rotation_statement(
        predecessor: u8,
        successor: u8,
        parent: u8,
        scope_epoch: u64,
    ) -> RotationStatement {
        RotationStatement {
            protocol_version: ROTATION_PROTOCOL_VERSION,
            scope: verse_scope(),
            branch_id: identifier(BRANCH),
            parent_op_id: hash(parent),
            scope_epoch,
            predecessor_did: public_key_to_did(&public_key(predecessor)),
            predecessor_public_key: public_key(predecessor),
            successor_did: public_key_to_did(&public_key(successor)),
            successor_public_key: public_key(successor),
        }
    }

    fn rotation_bytes(statement: &RotationStatement, signer: u8) -> Vec<u8> {
        RotationPayload {
            statement: statement.clone(),
            successor_signature: sign_successor_proof(&signing_key(signer), statement)
                .expect("sign"),
        }
        .encode_canonical()
        .expect("encode")
    }

    fn rotation_envelope(
        predecessor: u8,
        parent: u8,
        scope_epoch: u64,
        hlc: Hlc,
    ) -> UnsignedEnvelope {
        intent_envelope(
            public_key(predecessor),
            verse_scope(),
            identifier(BRANCH),
            vec![hash(parent)],
            scope_epoch,
            schemas().rotation,
            hlc,
        )
    }

    fn authority_for(keys: &[(u8, u64)]) -> FakeAuthority {
        let mut authority = FakeAuthority::default();
        for (key, epoch) in keys {
            authority.authorize(public_key(*key), *epoch);
        }
        authority
    }

    #[test]
    fn spec2_case_01_a_dual_proved_rotation_activates_its_successor() {
        let dag = dag();
        let authority = authority_for(&[(1, 0)]);
        let mut state = seeded_state();
        let statement = rotation_statement(1, 2, 0x01, 0);

        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, Hlc::new(10, 0)),
                hash(0x02),
                PayloadAvailability::Available(&rotation_bytes(&statement, 2)),
            ),
            Ok(AdmissionOutcome::Admitted)
        );
        assert_eq!(
            state
                .lineage
                .active_key_at(hash(0x02), public_key(2), verse_scope(), &dag),
            KeyState::Active
        );
    }

    #[test]
    fn spec2_case_13_admission_reads_only_the_envelope_author_and_the_successor_proof() {
        let dag = dag();
        let authority = authority_for(&[(1, 0)]);
        let mut state = seeded_state();

        // The payload claims key 9 as the predecessor while the envelope author is key 1;
        // there is no transport identity input that could vouch for the claim.
        let statement = rotation_statement(9, 2, 0x01, 0);
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, Hlc::new(10, 0)),
                hash(0x02),
                PayloadAvailability::Available(&rotation_bytes(&statement, 2)),
            ),
            Err(AdmissionError::RotationPayload(
                RotationPayloadError::PredecessorDidMismatch
            ))
        );
        assert!(state.lineage.is_empty());
    }

    #[test]
    fn spec2_case_12_a_matching_transport_key_is_not_an_input_to_admission() {
        let dag = dag();
        let authority = authority_for(&[(1, 0)]);
        let mut state = seeded_state();
        let statement = rotation_statement(1, 2, 0x01, 0);
        let mut payload = RotationPayload {
            statement: statement.clone(),
            successor_signature: sign_successor_proof(&signing_key(2), &statement).expect("sign"),
        };
        payload.successor_signature[0] ^= 0xff;

        assert!(matches!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, Hlc::new(10, 0)),
                hash(0x02),
                PayloadAvailability::Available(&payload.encode_canonical().expect("encode")),
            ),
            Err(AdmissionError::RotationProof(
                RotationProofError::ProofVerificationFailed(_)
            ))
        ));
        assert!(state.lineage.is_empty());
    }

    #[test]
    fn spec2_case_04_an_unauthorized_predecessor_does_not_activate_its_successor() {
        let dag = dag();
        let authority = FakeAuthority::default();
        let mut state = seeded_state();
        let statement = rotation_statement(1, 2, 0x01, 0);

        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, Hlc::new(10, 0)),
                hash(0x02),
                PayloadAvailability::Available(&rotation_bytes(&statement, 2)),
            ),
            Err(AdmissionError::PredecessorUnauthorized)
        );
        assert_eq!(
            state
                .lineage
                .active_key_at(hash(0x03), public_key(2), verse_scope(), &dag),
            KeyState::Unknown
        );
    }

    #[test]
    fn spec2_case_04_a_disavowed_predecessor_does_not_activate_its_successor() {
        let mut dag = dag();
        dag.insert(hash(0x0a), vec![hash(0x01)]);
        dag.insert(hash(0x0b), vec![hash(0x0a)]);
        let mut authority = authority_for(&[(1, 0), (4, 0)]);
        authority.set_role(public_key(4), AuthorityLevel::Manager);
        let mut state = seeded_state();

        let disavow = DisavowPayload {
            subject_did: public_key_to_did(&public_key(1)),
            subject_public_key: public_key(1),
            affected_scope: verse_scope(),
            first_hlc: Hlc::new(0, 0),
            last_hlc: Hlc::new(1_000, 0),
            reason_code: 1,
        };
        assert_eq!(
            admit_disavow(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &intent_envelope(
                    public_key(4),
                    verse_scope(),
                    identifier(BRANCH),
                    vec![hash(0x01)],
                    0,
                    schemas().disavow,
                    Hlc::new(5, 0),
                ),
                hash(0x0a),
                PayloadAvailability::Available(&disavow.encode_canonical().expect("encode")),
            ),
            Ok(AdmissionOutcome::Admitted)
        );

        let statement = rotation_statement(1, 2, 0x0a, 0);
        let envelope = rotation_envelope(1, 0x0a, 0, Hlc::new(10, 0));
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &envelope,
                hash(0x0b),
                PayloadAvailability::Available(&rotation_bytes(&statement, 2)),
            ),
            Err(AdmissionError::PredecessorDisavowed {
                disavow_op_ids: vec![hash(0x0a)],
            })
        );
    }

    #[test]
    fn spec2_case_05_unavailable_state_quarantines_and_never_activates_a_successor() {
        let statement = rotation_statement(1, 2, 0x01, 0);
        let bytes = rotation_bytes(&statement, 2);
        let envelope = rotation_envelope(1, 0x01, 0, Hlc::new(10, 0));

        // Missing parent.
        let mut empty_dag = FakeDag::default();
        empty_dag.insert(hash(0x99), vec![]);
        let mut state = seeded_state();
        assert_eq!(
            admit_rotation(
                &mut state,
                &empty_dag,
                &authority_for(&[(1, 0)]),
                &schemas(),
                &envelope,
                hash(0x02),
                PayloadAvailability::Available(&bytes),
            ),
            Ok(AdmissionOutcome::Quarantined(
                QuarantineReason::MissingParent {
                    missing_parents: vec![hash(0x01)],
                }
            ))
        );

        // Parent present but not admitted.
        let mut pending_dag = FakeDag::default();
        pending_dag.insert_unadmitted(hash(0x01), vec![]);
        let mut state = seeded_state();
        assert_eq!(
            admit_rotation(
                &mut state,
                &pending_dag,
                &authority_for(&[(1, 0)]),
                &schemas(),
                &envelope,
                hash(0x02),
                PayloadAvailability::Available(&bytes),
            ),
            Ok(AdmissionOutcome::Quarantined(
                QuarantineReason::ParentNotAdmitted {
                    parent_op_id: hash(0x01),
                }
            ))
        );

        // Schema unavailable, payload key unavailable, capability chain and epoch unavailable.
        let dag = dag();
        for (availability, expected) in [
            (
                PayloadAvailability::SchemaUnavailable,
                QuarantineReason::UnknownSchema {
                    schema_hash: schemas().rotation,
                },
            ),
            (
                PayloadAvailability::PayloadKeyUnavailable,
                QuarantineReason::PayloadKeyUnavailable,
            ),
        ] {
            let mut state = seeded_state();
            assert_eq!(
                admit_rotation(
                    &mut state,
                    &dag,
                    &authority_for(&[(1, 0)]),
                    &schemas(),
                    &envelope,
                    hash(0x02),
                    availability,
                ),
                Ok(AdmissionOutcome::Quarantined(expected))
            );
            assert_eq!(
                state
                    .lineage
                    .active_key_at(hash(0x03), public_key(2), verse_scope(), &dag),
                KeyState::Unknown,
                "a quarantined rotation never provisionally activates its successor"
            );
        }

        let mut chain_hidden = FakeAuthority::default();
        chain_hidden.hide_capability_chain(public_key(1));
        let mut state = seeded_state();
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &chain_hidden,
                &schemas(),
                &envelope,
                hash(0x02),
                PayloadAvailability::Available(&bytes),
            ),
            Ok(AdmissionOutcome::Quarantined(
                QuarantineReason::CapabilityChainUnavailable
            ))
        );
        assert!(state.is_quarantined(hash(0x02)));
        assert!(state.lineage.is_empty());

        let mut epoch_hidden = FakeAuthority::default();
        epoch_hidden.hide_epoch_state(public_key(1));
        let mut state = seeded_state();
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &epoch_hidden,
                &schemas(),
                &envelope,
                hash(0x02),
                PayloadAvailability::Available(&bytes),
            ),
            Ok(AdmissionOutcome::Quarantined(
                QuarantineReason::EpochStateUnavailable
            ))
        );
    }

    #[test]
    fn spec2_case_08_a_stale_epoch_rotation_is_rejected_and_the_pre_bump_one_still_verifies() {
        let dag = dag();
        // The capability view authorizes key 1 only at the bumped epoch 1.
        let authority = authority_for(&[(1, 1)]);
        let mut state = seeded_state();

        let stale_statement = rotation_statement(1, 2, 0x01, 0);
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, Hlc::new(10, 0)),
                hash(0x02),
                PayloadAvailability::Available(&rotation_bytes(&stale_statement, 2)),
            ),
            Err(AdmissionError::PredecessorUnauthorized),
            "a rotation relying on the pre-bump epoch is rejected once the bump is visible"
        );

        // The same statement remains replay-verifiable as evidence at its own epoch.
        let historic = RotationPayload::decode_canonical(&rotation_bytes(&stale_statement, 2))
            .expect("decode");
        assert_eq!(verify_rotation_proof(&historic), Ok(()));

        let current_statement = rotation_statement(1, 2, 0x01, 1);
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 1, Hlc::new(11, 0)),
                hash(0x03),
                PayloadAvailability::Available(&rotation_bytes(&current_statement, 2)),
            ),
            Ok(AdmissionOutcome::Admitted)
        );
    }

    #[test]
    fn spec2_case_07_two_concurrent_rotations_are_retained_with_neither_active() {
        let mut dag = FakeDag::default();
        dag.insert(hash(0x01), vec![]);
        dag.insert(hash(0x02), vec![hash(0x01)]);
        dag.insert(hash(0x03), vec![hash(0x01)]);
        dag.insert(hash(0x04), vec![hash(0x02), hash(0x03)]);
        let authority = authority_for(&[(1, 0)]);
        let mut state = seeded_state();

        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, Hlc::new(10, 0)),
                hash(0x02),
                PayloadAvailability::Available(&rotation_bytes(
                    &rotation_statement(1, 2, 0x01, 0),
                    2
                )),
            ),
            Ok(AdmissionOutcome::Admitted)
        );
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, Hlc::new(11, 0)),
                hash(0x03),
                PayloadAvailability::Available(&rotation_bytes(
                    &rotation_statement(1, 3, 0x01, 0),
                    3
                )),
            ),
            Ok(AdmissionOutcome::ForkRetained {
                conflicting_op_ids: vec![hash(0x02), hash(0x03)],
            })
        );

        for successor in [public_key(2), public_key(3)] {
            assert!(
                !matches!(
                    state
                        .lineage
                        .active_key_at(hash(0x04), successor, verse_scope(), &dag),
                    KeyState::Active
                ),
                "no successor of an unresolved fork may be active"
            );
        }
        assert_eq!(state.lineage.len(), 2, "both records are retained");
    }

    #[test]
    fn author_equivocation_quarantines_both_candidates_and_materializes_neither() {
        let mut dag = dag();
        dag.insert(hash(0x04), vec![hash(0x01)]);
        let authority = authority_for(&[(1, 0)]);
        let mut state = seeded_state();
        let contested = Hlc::new(10, 0);

        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rotation_envelope(1, 0x01, 0, contested),
                hash(0x02),
                PayloadAvailability::Available(&rotation_bytes(
                    &rotation_statement(1, 2, 0x01, 0),
                    2
                )),
            ),
            Ok(AdmissionOutcome::Admitted)
        );
        assert_eq!(state.lineage.len(), 1);

        // A second operation by the same author at the same HLC under a different op_id.
        let mut second = rotation_envelope(1, 0x01, 0, contested);
        second.branch_id = Identifier32([0x56; 32]);
        let outcome = admit_rotation(
            &mut state,
            &dag,
            &authority,
            &schemas(),
            &second,
            hash(0x04),
            PayloadAvailability::Available(&rotation_bytes(&rotation_statement(1, 3, 0x01, 0), 3)),
        )
        .expect("equivocation is a quarantine, not a hard error");

        assert_eq!(
            outcome,
            AdmissionOutcome::Quarantined(QuarantineReason::AuthorEquivocation {
                equivocation_key: second.equivocation_key(),
                first_op_id: hash(0x02),
                second_op_id: hash(0x04),
            })
        );
        assert_eq!(
            state.quarantine_reason(hash(0x02)),
            state.quarantine_reason(hash(0x04)),
            "both candidates carry one symmetric reason; neither is named the loser"
        );
        assert!(state.is_quarantined(hash(0x02)));
        assert!(state.is_quarantined(hash(0x04)));
        assert!(
            state.lineage.is_empty(),
            "the first candidate's effect is withdrawn; neither materializes"
        );
        assert_eq!(
            state
                .lineage
                .active_key_at(hash(0x03), public_key(1), verse_scope(), &dag),
            KeyState::Active
        );
    }

    #[test]
    fn re_presenting_one_operation_is_idempotent() {
        let dag = dag();
        let authority = authority_for(&[(1, 0)]);
        let mut state = seeded_state();
        let envelope = rotation_envelope(1, 0x01, 0, Hlc::new(10, 0));
        let bytes = rotation_bytes(&rotation_statement(1, 2, 0x01, 0), 2);

        for expected in [
            AdmissionOutcome::Admitted,
            AdmissionOutcome::AlreadyAdmitted,
        ] {
            assert_eq!(
                admit_rotation(
                    &mut state,
                    &dag,
                    &authority,
                    &schemas(),
                    &envelope,
                    hash(0x02),
                    PayloadAvailability::Available(&bytes),
                ),
                Ok(expected)
            );
        }
        assert_eq!(state.lineage.len(), 1);
    }

    #[test]
    fn a_lifecycle_operation_must_be_kind_one_with_the_registered_schema() {
        let dag = dag();
        let authority = authority_for(&[(1, 0)]);
        let mut state = seeded_state();
        let bytes = rotation_bytes(&rotation_statement(1, 2, 0x01, 0), 2);

        let mut wrong_kind = rotation_envelope(1, 0x01, 0, Hlc::new(10, 0));
        wrong_kind.operation_kind = 4;
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &wrong_kind,
                hash(0x02),
                PayloadAvailability::Available(&bytes),
            ),
            Err(AdmissionError::WrongOperationKind { operation_kind: 4 })
        );

        let mut wrong_schema = rotation_envelope(1, 0x01, 0, Hlc::new(10, 0));
        wrong_schema.schema_hash = schemas().disavow;
        assert_eq!(
            admit_rotation(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &wrong_schema,
                hash(0x02),
                PayloadAvailability::Available(&bytes),
            ),
            Err(AdmissionError::WrongSchema {
                expected: schemas().rotation,
                found: schemas().disavow,
            })
        );
    }

    #[test]
    fn spec2_case_09_a_disavow_below_manager_authority_is_rejected() {
        let dag = dag();
        let mut authority = authority_for(&[(4, 0)]);
        authority.set_role(public_key(4), AuthorityLevel::Editor);
        let mut state = seeded_state();

        let disavow = DisavowPayload {
            subject_did: public_key_to_did(&public_key(1)),
            subject_public_key: public_key(1),
            affected_scope: petal_scope(),
            first_hlc: Hlc::new(0, 0),
            last_hlc: Hlc::new(100, 0),
            reason_code: 1,
        };
        assert_eq!(
            admit_disavow(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &intent_envelope(
                    public_key(4),
                    verse_scope(),
                    identifier(BRANCH),
                    vec![hash(0x01)],
                    0,
                    schemas().disavow,
                    Hlc::new(5, 0),
                ),
                hash(0x0a),
                PayloadAvailability::Available(&disavow.encode_canonical().expect("encode")),
            ),
            Err(AdmissionError::IssuerBelowManager {
                issuer_authority: AuthorityLevel::Editor,
            })
        );
        assert!(state.disavows.is_empty());
    }

    #[test]
    fn spec2_case_05_an_unresolvable_issuer_authority_quarantines_the_disavow() {
        let dag = dag();
        let mut authority = authority_for(&[(4, 0)]);
        authority.hide_role(public_key(4));
        let mut state = seeded_state();

        let disavow = DisavowPayload {
            subject_did: public_key_to_did(&public_key(1)),
            subject_public_key: public_key(1),
            affected_scope: verse_scope(),
            first_hlc: Hlc::new(0, 0),
            last_hlc: Hlc::new(100, 0),
            reason_code: 1,
        };
        assert_eq!(
            admit_disavow(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &intent_envelope(
                    public_key(4),
                    verse_scope(),
                    identifier(BRANCH),
                    vec![hash(0x01)],
                    0,
                    schemas().disavow,
                    Hlc::new(5, 0),
                ),
                hash(0x0a),
                PayloadAvailability::Available(&disavow.encode_canonical().expect("encode")),
            ),
            Ok(AdmissionOutcome::Quarantined(
                QuarantineReason::CapabilityChainUnavailable
            ))
        );
        assert!(state.disavows.is_empty());
        assert!(state.is_quarantined(hash(0x0a)));
        assert!(!state.is_applied(hash(0x0a)));
    }

    #[test]
    fn spec2_case_14_only_strictly_higher_authority_rescinds_and_only_causally_forward() {
        let mut dag = dag();
        dag.insert(hash(0x0a), vec![hash(0x01)]);
        dag.insert(hash(0x0c), vec![hash(0x0a)]);
        let mut authority = authority_for(&[(4, 0), (5, 0), (6, 0)]);
        authority.set_role(public_key(4), AuthorityLevel::Manager);
        authority.set_role(public_key(5), AuthorityLevel::Manager);
        authority.set_role(public_key(6), AuthorityLevel::Owner);
        let mut state = seeded_state();

        let disavow = DisavowPayload {
            subject_did: public_key_to_did(&public_key(1)),
            subject_public_key: public_key(1),
            affected_scope: verse_scope(),
            first_hlc: Hlc::new(0, 0),
            last_hlc: Hlc::new(100, 0),
            reason_code: 1,
        };
        admit_disavow(
            &mut state,
            &dag,
            &authority,
            &schemas(),
            &intent_envelope(
                public_key(4),
                verse_scope(),
                identifier(BRANCH),
                vec![hash(0x01)],
                0,
                schemas().disavow,
                Hlc::new(5, 0),
            ),
            hash(0x0a),
            PayloadAvailability::Available(&disavow.encode_canonical().expect("encode")),
        )
        .expect("admit disavow");

        let rescind_payload = RescindPayload {
            disavow_op_id: hash(0x0a),
            reason_code: 2,
        }
        .encode_canonical()
        .expect("encode");
        let rescind_envelope = |issuer: u8, hlc: Hlc| {
            intent_envelope(
                public_key(issuer),
                verse_scope(),
                identifier(BRANCH),
                vec![hash(0x0a)],
                0,
                schemas().disavow_rescind,
                hlc,
            )
        };

        assert_eq!(
            admit_disavow_rescind(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rescind_envelope(5, Hlc::new(6, 0)),
                hash(0x0d),
                PayloadAvailability::Available(&rescind_payload),
            ),
            Err(AdmissionError::RescindAuthority(
                RescindAuthorityError::AuthorityNotStrictlyHigher {
                    rescinder: AuthorityLevel::Manager,
                    issuer: AuthorityLevel::Manager,
                }
            ))
        );

        assert_eq!(
            admit_disavow_rescind(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &rescind_envelope(6, Hlc::new(7, 0)),
                hash(0x0c),
                PayloadAvailability::Available(&rescind_payload),
            ),
            Ok(AdmissionOutcome::Admitted)
        );

        let subject = DisavowSubject {
            author_public_key: public_key(1),
            scope: verse_scope(),
            hlc: Hlc::new(50, 0),
        };
        assert!(state.disavows.is_disavowed_at(hash(0x0a), &dag, &subject));
        assert!(!state.disavows.is_disavowed_at(hash(0x0c), &dag, &subject));
        assert!(state.disavows.disavow(hash(0x0a)).is_some());
    }

    #[test]
    fn spec2_case_14_admission_refuses_to_rescind_an_owner_issued_disavow() {
        let mut dag = dag();
        dag.insert(hash(0x0a), vec![hash(0x01)]);
        let mut authority = authority_for(&[(6, 0), (7, 0)]);
        authority.set_role(public_key(6), AuthorityLevel::Owner);
        authority.set_role(public_key(7), AuthorityLevel::Owner);
        let mut state = seeded_state();

        let disavow = DisavowPayload {
            subject_did: public_key_to_did(&public_key(1)),
            subject_public_key: public_key(1),
            affected_scope: verse_scope(),
            first_hlc: Hlc::new(0, 0),
            last_hlc: Hlc::new(100, 0),
            reason_code: 1,
        };
        admit_disavow(
            &mut state,
            &dag,
            &authority,
            &schemas(),
            &intent_envelope(
                public_key(6),
                verse_scope(),
                identifier(BRANCH),
                vec![hash(0x01)],
                0,
                schemas().disavow,
                Hlc::new(5, 0),
            ),
            hash(0x0a),
            PayloadAvailability::Available(&disavow.encode_canonical().expect("encode")),
        )
        .expect("admit disavow");

        assert_eq!(
            admit_disavow_rescind(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &intent_envelope(
                    public_key(7),
                    verse_scope(),
                    identifier(BRANCH),
                    vec![hash(0x0a)],
                    0,
                    schemas().disavow_rescind,
                    Hlc::new(6, 0),
                ),
                hash(0x0c),
                PayloadAvailability::Available(
                    &RescindPayload {
                        disavow_op_id: hash(0x0a),
                        reason_code: 2,
                    }
                    .encode_canonical()
                    .expect("encode")
                ),
            ),
            Err(AdmissionError::RescindAuthority(
                RescindAuthorityError::OwnerIssuedDisavowIsFinal
            ))
        );
    }

    #[test]
    fn a_rescind_naming_an_unknown_disavow_is_quarantined() {
        let dag = dag();
        let mut authority = authority_for(&[(6, 0)]);
        authority.set_role(public_key(6), AuthorityLevel::Owner);
        let mut state = seeded_state();

        assert_eq!(
            admit_disavow_rescind(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &intent_envelope(
                    public_key(6),
                    verse_scope(),
                    identifier(BRANCH),
                    vec![hash(0x01)],
                    0,
                    schemas().disavow_rescind,
                    Hlc::new(6, 0),
                ),
                hash(0x0c),
                PayloadAvailability::Available(
                    &RescindPayload {
                        disavow_op_id: hash(0xee),
                        reason_code: 2,
                    }
                    .encode_canonical()
                    .expect("encode")
                ),
            ),
            Ok(AdmissionOutcome::Quarantined(
                QuarantineReason::ReferencedDisavowUnavailable {
                    disavow_op_id: hash(0xee),
                }
            ))
        );
    }

    #[test]
    fn spec2_case_13_a_continuity_grant_changes_neither_lineage_nor_disavow_state() {
        let mut dag = dag();
        dag.insert(hash(0x0a), vec![hash(0x01)]);
        dag.insert(hash(0x0e), vec![hash(0x0a)]);
        let mut authority = authority_for(&[(1, 0), (4, 0), (6, 0)]);
        authority.set_role(public_key(4), AuthorityLevel::Manager);
        authority.set_role(public_key(6), AuthorityLevel::Owner);
        let mut state = seeded_state();

        let disavow = DisavowPayload {
            subject_did: public_key_to_did(&public_key(9)),
            subject_public_key: public_key(9),
            affected_scope: verse_scope(),
            first_hlc: Hlc::new(0, 0),
            last_hlc: Hlc::new(100, 0),
            reason_code: 1,
        };
        admit_disavow(
            &mut state,
            &dag,
            &authority,
            &schemas(),
            &intent_envelope(
                public_key(4),
                verse_scope(),
                identifier(BRANCH),
                vec![hash(0x01)],
                0,
                schemas().disavow,
                Hlc::new(5, 0),
            ),
            hash(0x0a),
            PayloadAvailability::Available(&disavow.encode_canonical().expect("encode")),
        )
        .expect("admit disavow");

        let disavowed_subject = DisavowSubject {
            author_public_key: public_key(9),
            scope: verse_scope(),
            hlc: Hlc::new(50, 0),
        };
        let lineage_before: Vec<KeyState> = [public_key(1), public_key(2), public_key(9)]
            .iter()
            .map(|key| {
                state
                    .lineage
                    .active_key_at(hash(0x0e), *key, verse_scope(), &dag)
            })
            .collect();
        let disavowed_before = state.disavows.is_disavowed(&disavowed_subject);

        // The grant links the disavowed, regenerated principal 9 to the new principal 2.
        let statement = ContinuityGrantStatement {
            protocol_version: 1,
            verse_scope: verse_scope(),
            lost_principal_did: public_key_to_did(&public_key(9)),
            lost_principal_public_key: public_key(9),
            new_principal_did: public_key_to_did(&public_key(2)),
            new_principal_public_key: public_key(2),
            reason_code: 4,
        };
        let grant = ContinuityGrantPayload {
            new_principal_signature: sign_continuity_grant(&signing_key(2), &statement)
                .expect("countersign"),
            statement,
        };
        assert_eq!(
            admit_continuity_grant(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &intent_envelope(
                    public_key(6),
                    verse_scope(),
                    identifier(BRANCH),
                    vec![hash(0x0a)],
                    0,
                    schemas().continuity_grant,
                    Hlc::new(8, 0),
                ),
                hash(0x0e),
                PayloadAvailability::Available(&grant.encode_canonical().expect("encode")),
            ),
            Ok(AdmissionOutcome::Admitted)
        );

        assert_eq!(state.attributions.len(), 1);
        let lineage_after: Vec<KeyState> = [public_key(1), public_key(2), public_key(9)]
            .iter()
            .map(|key| {
                state
                    .lineage
                    .active_key_at(hash(0x0e), *key, verse_scope(), &dag)
            })
            .collect();
        assert_eq!(
            lineage_after, lineage_before,
            "an attribution link grants no lineage"
        );
        assert_eq!(
            state.disavows.is_disavowed(&disavowed_subject),
            disavowed_before,
            "an attribution link cannot rescind a disavow"
        );
        assert!(
            state.lineage.is_empty(),
            "the grant recorded no rotation at all"
        );
    }

    #[test]
    fn a_continuity_grant_not_signed_by_the_owner_is_rejected() {
        let mut dag = dag();
        dag.insert(hash(0x0e), vec![hash(0x01)]);
        let mut authority = authority_for(&[(4, 0)]);
        authority.set_role(public_key(4), AuthorityLevel::Manager);
        let mut state = seeded_state();

        let statement = ContinuityGrantStatement {
            protocol_version: 1,
            verse_scope: verse_scope(),
            lost_principal_did: public_key_to_did(&public_key(9)),
            lost_principal_public_key: public_key(9),
            new_principal_did: public_key_to_did(&public_key(2)),
            new_principal_public_key: public_key(2),
            reason_code: 4,
        };
        let grant = ContinuityGrantPayload {
            new_principal_signature: sign_continuity_grant(&signing_key(2), &statement)
                .expect("countersign"),
            statement,
        };
        assert_eq!(
            admit_continuity_grant(
                &mut state,
                &dag,
                &authority,
                &schemas(),
                &intent_envelope(
                    public_key(4),
                    verse_scope(),
                    identifier(BRANCH),
                    vec![hash(0x01)],
                    0,
                    schemas().continuity_grant,
                    Hlc::new(8, 0),
                ),
                hash(0x0e),
                PayloadAvailability::Available(&grant.encode_canonical().expect("encode")),
            ),
            Err(AdmissionError::ContinuityGrantNotOwnerSigned)
        );
        assert!(state.attributions.is_empty());
    }

    #[test]
    fn spec2_case_10_two_independent_replicas_reach_the_same_state_from_one_head() {
        let mut dag = dag();
        dag.insert(hash(0x0a), vec![hash(0x02)]);
        dag.insert(hash(0x05), vec![hash(0x0a)]);
        let mut authority = authority_for(&[(1, 0), (2, 0), (4, 0)]);
        authority.set_role(public_key(4), AuthorityLevel::Manager);

        let rotation_one = (
            rotation_envelope(1, 0x01, 0, Hlc::new(10, 0)),
            hash(0x02),
            rotation_bytes(&rotation_statement(1, 2, 0x01, 0), 2),
        );
        let disavow_payload = DisavowPayload {
            subject_did: public_key_to_did(&public_key(1)),
            subject_public_key: public_key(1),
            affected_scope: verse_scope(),
            first_hlc: Hlc::new(0, 0),
            last_hlc: Hlc::new(9, 0),
            reason_code: 1,
        };
        let disavow_operation = (
            intent_envelope(
                public_key(4),
                verse_scope(),
                identifier(BRANCH),
                vec![hash(0x02)],
                0,
                schemas().disavow,
                Hlc::new(20, 0),
            ),
            hash(0x0a),
            disavow_payload.encode_canonical().expect("encode"),
        );

        let mut left = seeded_state();
        admit_rotation(
            &mut left,
            &dag,
            &authority,
            &schemas(),
            &rotation_one.0,
            rotation_one.1,
            PayloadAvailability::Available(&rotation_one.2),
        )
        .expect("rotation");
        admit_disavow(
            &mut left,
            &dag,
            &authority,
            &schemas(),
            &disavow_operation.0,
            disavow_operation.1,
            PayloadAvailability::Available(&disavow_operation.2),
        )
        .expect("disavow");

        // The right replica sees the disavow first, quarantines it for a missing parent, and
        // replays both once the parent lands.
        let mut right = seeded_state();
        let mut partial_dag = FakeDag::default();
        partial_dag.insert(hash(0x01), vec![]);
        assert!(matches!(
            admit_disavow(
                &mut right,
                &partial_dag,
                &authority,
                &schemas(),
                &disavow_operation.0,
                disavow_operation.1,
                PayloadAvailability::Available(&disavow_operation.2),
            ),
            Ok(AdmissionOutcome::Quarantined(
                QuarantineReason::MissingParent { .. }
            ))
        ));
        admit_rotation(
            &mut right,
            &dag,
            &authority,
            &schemas(),
            &rotation_one.0,
            rotation_one.1,
            PayloadAvailability::Available(&rotation_one.2),
        )
        .expect("rotation");
        admit_disavow(
            &mut right,
            &dag,
            &authority,
            &schemas(),
            &disavow_operation.0,
            disavow_operation.1,
            PayloadAvailability::Available(&disavow_operation.2),
        )
        .expect("disavow");

        let head = hash(0x05);
        for key in [public_key(1), public_key(2)] {
            assert_eq!(
                left.lineage.active_key_at(head, key, verse_scope(), &dag),
                right.lineage.active_key_at(head, key, verse_scope(), &dag)
            );
        }
        let early = DisavowSubject {
            author_public_key: public_key(1),
            scope: verse_scope(),
            hlc: Hlc::new(5, 0),
        };
        let late = DisavowSubject {
            author_public_key: public_key(1),
            scope: verse_scope(),
            hlc: Hlc::new(10, 0),
        };
        assert!(left.disavows.is_disavowed_at(head, &dag, &early));
        assert_eq!(
            left.disavows.matching_disavows_at(head, &dag, &early),
            right.disavows.matching_disavows_at(head, &dag, &early)
        );
        assert!(!left.disavows.is_disavowed_at(head, &dag, &late));
        assert_eq!(
            left.disavows.matching_disavows_at(head, &dag, &late),
            right.disavows.matching_disavows_at(head, &dag, &late)
        );
        assert_eq!(left.lineage.len(), right.lineage.len());
        assert_eq!(left.disavows.len(), right.disavows.len());
    }
}
