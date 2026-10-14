//! SPEC-2 §4 author-key lifecycle as an admission seam, and the dispatcher that gives the four
//! SPEC-2 admission entrypoints their production caller. See `src/AGENTS.md`
//! §composed-verifiers.

use zeroize::Zeroizing;

use crate::author_key::admission::{
    admit_continuity_grant, admit_disavow, admit_disavow_rescind, admit_rotation, AdmissionError,
    AdmissionOutcome as LifecycleOutcome, AuthorKeyState, CapabilityAuthorityView,
    PayloadAvailability, RegisteredSchemas,
};
use crate::author_key::lineage::{CausalOperationView, KeyState, LineageIndex};
use crate::cbor::encode_canonical;
use crate::envelope::{Hash32, Scope};
use crate::materialize::traits::{AdmittedCandidate, VerifiedEnvelopeMeta};

use super::verifier::AuthorKeyLifecycleView;

/// [`AuthorKeyLifecycleView`] over an admitted rotation lineage and the caller's causal DAG.
///
/// Reads only the lineage: a §6 disavow classifies operations for the projection and never
/// refuses their admission (SPEC-2 §6 rules 4, 5 and 8), so the disavow index is not consulted.
#[derive(Debug)]
pub struct CausalLifecycleView<'a, C> {
    lineage: &'a LineageIndex,
    causal: &'a C,
}

impl<'a, C: CausalOperationView> CausalLifecycleView<'a, C> {
    /// Binds a lineage to the DAG its causal points are resolved in.
    pub fn new(lineage: &'a LineageIndex, causal: &'a C) -> Self {
        Self { lineage, causal }
    }
}

impl<C: CausalOperationView + Sync> AuthorKeyLifecycleView for CausalLifecycleView<'_, C> {
    /// The worst state any parent induces: a rotation reachable from ONE parent already puts the
    /// candidate in that rotation's causal future (§4 rule 4).
    fn key_state(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        parents: &[Hash32],
    ) -> KeyState {
        let mut resolved = KeyState::Unknown;
        for parent in parents {
            match self
                .lineage
                .active_key_at(*parent, *author_public_key, *scope, self.causal)
            {
                state @ (KeyState::Retired { .. } | KeyState::ForkedUnresolved { .. }) => {
                    return state
                }
                KeyState::Active => resolved = KeyState::Active,
                KeyState::Unknown => {}
            }
        }
        resolved
    }
}

/// Which SPEC-2 lifecycle entrypoint an admitted operation is routed to, by its schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleKind {
    /// `author-key-rotation-v1`, to `admit_rotation`.
    Rotation,
    /// `manager-suspect-key-disavow-v1`, to `admit_disavow`.
    Disavow,
    /// `manager-suspect-key-disavow-rescind-v1`, to `admit_disavow_rescind`.
    DisavowRescind,
    /// `owner-continuity-grant-v1`, to `admit_continuity_grant`.
    ContinuityGrant,
}

/// What [`LifecycleDispatcher::dispatch`] did with one admitted operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleDispatch {
    /// The operation names no SPEC-2 lifecycle schema; SPEC-2 state is untouched.
    NotLifecycle,
    /// The operation was handed to its SPEC-2 entrypoint, which answered `result`.
    Dispatched {
        /// The entrypoint it was routed to.
        kind: LifecycleKind,
        /// That entrypoint's own outcome or rejection.
        result: Result<LifecycleOutcome, AdmissionError>,
    },
}

/// Routes SPEC-1/SPEC-4-admitted operations of a SPEC-2 lifecycle schema into the SPEC-2 state
/// that [`CausalLifecycleView`] then answers from.
///
/// Takes only an [`AdmittedCandidate`], which admission alone produces, so the payload it
/// hands an entrypoint is always the authenticated, schema-validated plaintext of that exact
/// operation (`PayloadAvailability::Available`).
#[derive(Debug)]
pub struct LifecycleDispatcher<A> {
    state: AuthorKeyState,
    schemas: RegisteredSchemas,
    authority: A,
}

impl<A: CapabilityAuthorityView> LifecycleDispatcher<A> {
    /// An empty SPEC-2 state over the four registered lifecycle schemas.
    pub fn new(schemas: RegisteredSchemas, authority: A) -> Self {
        Self::with_state(AuthorKeyState::new(), schemas, authority)
    }

    /// Resumes from SPEC-2 state rebuilt elsewhere.
    pub fn with_state(state: AuthorKeyState, schemas: RegisteredSchemas, authority: A) -> Self {
        Self {
            state,
            schemas,
            authority,
        }
    }

    /// The SPEC-2 state the dispatched operations built.
    pub fn state(&self) -> &AuthorKeyState {
        &self.state
    }

    /// The SPEC-3 answers the entrypoints consult.
    pub fn authority(&self) -> &A {
        &self.authority
    }

    /// Declares a key active in `scope` with no preceding rotation (a lineage root).
    pub fn register_root_key(&mut self, scope: Scope, public_key: [u8; 32]) {
        self.state.lineage.register_root_key(scope, public_key);
    }

    /// The lifecycle entrypoint `meta`'s schema names, if any.
    pub fn lifecycle_kind(&self, meta: &VerifiedEnvelopeMeta) -> Option<LifecycleKind> {
        let schema = meta.schema_hash;
        if schema == self.schemas.rotation {
            Some(LifecycleKind::Rotation)
        } else if schema == self.schemas.disavow {
            Some(LifecycleKind::Disavow)
        } else if schema == self.schemas.disavow_rescind {
            Some(LifecycleKind::DisavowRescind)
        } else if schema == self.schemas.continuity_grant {
            Some(LifecycleKind::ContinuityGrant)
        } else {
            None
        }
    }

    /// The admission-time view of this dispatcher's lineage over `causal`.
    pub fn lifecycle_view<'a, C: CausalOperationView>(
        &'a self,
        causal: &'a C,
    ) -> CausalLifecycleView<'a, C> {
        CausalLifecycleView::new(&self.state.lineage, causal)
    }

    /// Hands an admitted operation of a lifecycle schema to its SPEC-2 entrypoint.
    ///
    /// Routing is by schema alone, so a lifecycle schema on the wrong kind still reaches its
    /// entrypoint and is rejected there rather than slipping past SPEC-2 as an ordinary intent.
    pub fn dispatch(
        &mut self,
        causal: &impl CausalOperationView,
        admitted: &AdmittedCandidate,
    ) -> LifecycleDispatch {
        let candidate = &admitted.candidate;
        let Some(kind) = self.lifecycle_kind(&candidate.meta) else {
            return LifecycleDispatch::NotLifecycle;
        };
        // Facts were decoded from canonical CBOR, so re-encoding restores the exact plaintext.
        let plaintext = admitted
            .payload_facts
            .as_ref()
            .map(|facts| Zeroizing::new(encode_canonical(&facts.fields)));
        let payload = match &plaintext {
            Some(bytes) => PayloadAvailability::Available(bytes),
            None => PayloadAvailability::PayloadKeyUnavailable,
        };
        let envelope = &candidate.envelope.unsigned;
        let op_id = candidate.meta.op_id;
        let state = &mut self.state;
        let authority = &self.authority;
        let schemas = &self.schemas;
        let result = match kind {
            LifecycleKind::Rotation => {
                admit_rotation(state, causal, authority, schemas, envelope, op_id, payload)
            }
            LifecycleKind::Disavow => {
                admit_disavow(state, causal, authority, schemas, envelope, op_id, payload)
            }
            LifecycleKind::DisavowRescind => {
                admit_disavow_rescind(state, causal, authority, schemas, envelope, op_id, payload)
            }
            LifecycleKind::ContinuityGrant => {
                admit_continuity_grant(state, causal, authority, schemas, envelope, op_id, payload)
            }
        };
        LifecycleDispatch::Dispatched { kind, result }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::author_key::payloads::{
        RotationPayload, RotationStatement, ROTATION_PROTOCOL_VERSION,
    };
    use crate::author_key::rotation_proof::sign_successor_proof;
    use crate::author_key::test_support::{schemas, FakeAuthority, FakeDag};
    use crate::cbor::encode_canonical_checked;
    use crate::compose::fixtures::{
        hash, public_key, signing_key, verse_scope, ComposedFixture, NoLifecycle, AUTHOR_SEED,
        BRANCH, EPOCH, SUCCESSOR_SEED,
    };
    use crate::did_key::public_key_to_did;
    use crate::materialize::errors::AdmissionOutcome;
    use crate::materialize::traits::{admit_candidate_with_facts, CandidateVerifier};

    fn rotation_plaintext(parent: Hash32) -> Vec<u8> {
        let statement = RotationStatement {
            protocol_version: ROTATION_PROTOCOL_VERSION,
            scope: verse_scope(),
            branch_id: BRANCH,
            parent_op_id: parent,
            scope_epoch: EPOCH,
            predecessor_did: public_key_to_did(&public_key(AUTHOR_SEED)),
            predecessor_public_key: public_key(AUTHOR_SEED),
            successor_did: public_key_to_did(&public_key(SUCCESSOR_SEED)),
            successor_public_key: public_key(SUCCESSOR_SEED),
        };
        let payload = RotationPayload {
            successor_signature: sign_successor_proof(&signing_key(SUCCESSOR_SEED), &statement)
                .expect("successor proof"),
            statement,
        };
        let bytes = payload.encode_canonical().expect("rotation payload");
        // The payload rides the CBOR facts path, so it must already be canonical CBOR.
        let decoded = crate::cbor::decode_canonical(&bytes).expect("canonical");
        assert_eq!(
            encode_canonical_checked(&decoded).expect("re-encode"),
            bytes
        );
        bytes
    }

    #[tokio::test]
    async fn an_operation_by_a_retired_author_key_is_unauthorized() {
        let fixture = ComposedFixture::new(EPOCH);
        let genesis = hash(0x01);
        let mut dag = FakeDag::default();
        dag.insert(genesis, vec![]);

        let mut authority = FakeAuthority::default();
        authority.authorize(public_key(AUTHOR_SEED), EPOCH);
        let mut dispatcher = LifecycleDispatcher::new(schemas(), authority);
        dispatcher.register_root_key(verse_scope(), public_key(AUTHOR_SEED));

        // 1. The author's own rotation admits through the composed verifier and is dispatched,
        //    so the lineage the verifier reads next is fed by the production path.
        let rotation = fixture.sealed(
            AUTHOR_SEED,
            verse_scope(),
            vec![genesis],
            1,
            schemas().rotation,
            &rotation_plaintext(genesis),
        );
        let admitted_rotation = {
            let verifier = fixture.verifier(EPOCH, dispatcher.lifecycle_view(&dag));
            admit_candidate_with_facts(
                &verifier,
                &fixture.no_equivocation(),
                &fixture.same_verse_parents(),
                &rotation.bytes,
                Some(&rotation.ciphertext),
            )
            .await
            .expect("the rotation itself admits")
        };
        assert_eq!(
            dispatcher.dispatch(&dag, &admitted_rotation),
            LifecycleDispatch::Dispatched {
                kind: LifecycleKind::Rotation,
                result: Ok(LifecycleOutcome::Admitted),
            }
        );
        dag.insert(rotation.op_id, vec![genesis]);

        let verifier = fixture.verifier(EPOCH, dispatcher.lifecycle_view(&dag));

        // 2. In the rotation's causal future the predecessor key is retired (§4 rule 4), even
        //    when only one of several parents reaches the rotation.
        let mut both = vec![genesis, rotation.op_id];
        both.sort();
        for parents in [vec![rotation.op_id], both] {
            let after = fixture.operation(AUTHOR_SEED, parents, 1);
            let candidate = verifier
                .verify_envelope(&after.bytes)
                .await
                .expect("ingress");
            assert_eq!(
                verifier.verify_authorization(&candidate).await,
                Err(AdmissionOutcome::UnauthorizedOperation)
            );
        }

        // 3. Causally concurrent with the rotation, the same key is not retroactively invalid
        //    (§4 rule 5): resolution is causal, never by arrival order. An ordinary operation
        //    leaves SPEC-2 state alone when dispatched.
        let concurrent = fixture.operation(AUTHOR_SEED, vec![genesis], 1);
        let admitted = admit_candidate_with_facts(
            &verifier,
            &fixture.no_equivocation(),
            &fixture.same_verse_parents(),
            &concurrent.bytes,
            Some(&concurrent.ciphertext),
        )
        .await
        .expect("a concurrent operation by the predecessor admits");
        drop(verifier);
        assert_eq!(
            dispatcher.dispatch(&dag, &admitted),
            LifecycleDispatch::NotLifecycle
        );

        // Control: with no lineage at all the same follow-up admits, so the refusal above is the
        // lineage's, not the chain's.
        let unrotated = fixture.verifier(EPOCH, NoLifecycle);
        let after = fixture.operation(AUTHOR_SEED, vec![rotation.op_id], 1);
        let candidate = unrotated
            .verify_envelope(&after.bytes)
            .await
            .expect("ingress");
        assert_eq!(unrotated.verify_authorization(&candidate).await, Ok(()));
    }
}
