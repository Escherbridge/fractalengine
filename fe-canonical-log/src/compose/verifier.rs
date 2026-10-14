//! The production `CandidateVerifier`: SPEC-1 ingress, SPEC-3 §2.4 chain verification, SPEC-2
//! §4 key lifecycle, and SPEC-1 §5.2 payload opening composed over injected seams. See
//! `src/AGENTS.md` §composed-verifiers.

use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::author_key::lineage::KeyState;
use crate::capability::{
    verify_chain_bytes, AuthorizationRequest, AuthorizationSnapshot, ChainError,
    ManagerAuthorityView, ObjectClass, Principal, Verb, VerificationOptions,
};
use crate::crypto::aead::{self, SealingKey, AEAD_TAG_LENGTH};
use crate::crypto::key_id::derive_key_id;
use crate::crypto::scope_key::ScopeKeyStore;
use crate::envelope::{Hash32, Identifier32, Scope, UnsignedEnvelope};
use crate::kind::OperationKind;
use crate::materialize::errors::{AdmissionOutcome, AuthorizationEvidenceGap};
use crate::materialize::traits::{
    verify_candidate, CandidateVerifier, PayloadFacts, VerifiedCandidate,
};
use crate::payload_aad::payload_aad_preimage;
use crate::retention::crypto_shred::ScopeKeyId;

/// Local resolution of a capability chain artifact by its `chain_id` (SPEC-3 §2.4).
///
/// A source answers only from what this process holds; `None` is "not here yet", which the
/// composed verifier quarantines, never an allow and never a refusal.
pub trait CapabilityChainSource: Send + Sync {
    /// The exact canonical chain bytes addressed by `chain_id`, if held locally.
    fn chain_bytes(&self, chain_id: &Hash32) -> Option<Vec<u8>>;
}

/// What a payload schema registry decided about one decrypted plaintext.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaVerdict {
    /// The plaintext validates against the registered schema for this kind.
    Valid,
    /// The schema is registered and the plaintext does not satisfy it.
    Invalid,
    /// `schema_hash` is not registered here.
    UnknownSchema,
    /// The operation kind has no registered schema and reduction in this build.
    UnknownKind,
}

/// The schemas and kinds this build interprets (SPEC-1 §6 rule 7, SPEC-4 §5).
pub trait PayloadSchemaRegistry: Send + Sync {
    /// Whether this build has a registered schema and reduction for a registry-interpreted kind
    /// (kind 5 and the application range, `OperationKind::requires_registered_interpretation`).
    fn interprets_kind(&self, operation_kind: u16) -> bool;

    /// Validates a decrypted plaintext against `schema_hash` for `operation_kind`.
    fn validate(
        &self,
        schema_hash: &Hash32,
        operation_kind: u16,
        plaintext: &[u8],
    ) -> SchemaVerdict;
}

/// Local resolution of a payload ciphertext by its `PayloadRef::ciphertext_hash`.
pub trait PayloadCiphertextSource: Send + Sync {
    /// The ciphertext bytes stored under `ciphertext_hash`, if held locally.
    fn ciphertext(&self, ciphertext_hash: &Hash32) -> Option<Vec<u8>>;
}

/// The SPEC-2 §4 state of an author key at a candidate's parent-induced causal point.
pub trait AuthorKeyLifecycleView: Send + Sync {
    /// The state of `author_public_key` for `scope` as seen from every one of `parents`.
    fn key_state(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        parents: &[Hash32],
    ) -> KeyState;
}

/// The wall clock SPEC-3 §2.4 step 7 checks a chain's window against.
pub trait Clock: Send + Sync {
    /// Current Unix time in milliseconds.
    fn now_ms(&self) -> u64;
}

/// The operating-system wall clock. A clock before the epoch reads as 0, which no chain window
/// contains, so a broken clock refuses rather than admits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

impl<T: CapabilityChainSource + ?Sized> CapabilityChainSource for &T {
    fn chain_bytes(&self, chain_id: &Hash32) -> Option<Vec<u8>> {
        (**self).chain_bytes(chain_id)
    }
}

impl<T: PayloadSchemaRegistry + ?Sized> PayloadSchemaRegistry for &T {
    fn interprets_kind(&self, operation_kind: u16) -> bool {
        (**self).interprets_kind(operation_kind)
    }

    fn validate(
        &self,
        schema_hash: &Hash32,
        operation_kind: u16,
        plaintext: &[u8],
    ) -> SchemaVerdict {
        (**self).validate(schema_hash, operation_kind, plaintext)
    }
}

impl<T: PayloadCiphertextSource + ?Sized> PayloadCiphertextSource for &T {
    fn ciphertext(&self, ciphertext_hash: &Hash32) -> Option<Vec<u8>> {
        (**self).ciphertext(ciphertext_hash)
    }
}

impl<T: AuthorKeyLifecycleView + ?Sized> AuthorKeyLifecycleView for &T {
    fn key_state(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        parents: &[Hash32],
    ) -> KeyState {
        (**self).key_state(author_public_key, scope, parents)
    }
}

impl<T: Clock + ?Sized> Clock for &T {
    fn now_ms(&self) -> u64 {
        (**self).now_ms()
    }
}

/// The seams a [`ComposedCandidateVerifier`] composes, named so a call site reads as a map.
#[derive(Debug)]
pub struct ComposedVerifierParts<C, M, L, K, P, S, T> {
    /// Capability chain artifacts by `chain_id`.
    pub chains: C,
    /// The persistent SPEC-3 authorization view the chain's §2.4 questions are asked of.
    pub authority: M,
    /// The SPEC-2 §4 author-key lifecycle at a causal point.
    pub lifecycle: L,
    /// Scope-key custody, keyed by `(scope, epoch)`.
    pub scope_keys: K,
    /// Payload ciphertexts by hash, consulted when the caller supplies none.
    pub ciphertexts: P,
    /// The schemas and application kinds this build interprets.
    pub schemas: S,
    /// The wall clock for the chain window.
    pub clock: T,
}

/// The production [`CandidateVerifier`]: every SPEC-4 §5 admission question answered by asking
/// the injected seam itself, with a missing answer quarantined and never allowed.
///
/// Live-traffic policy only (`VerificationOptions::live()`): historical causal replay is a
/// different question with a different snapshot and does not go through this type.
#[derive(Debug)]
pub struct ComposedCandidateVerifier<C, M, L, K, P, S, T> {
    parts: ComposedVerifierParts<C, M, L, K, P, S, T>,
}

impl<C, M, L, K, P, S, T> ComposedCandidateVerifier<C, M, L, K, P, S, T>
where
    C: CapabilityChainSource,
    M: ManagerAuthorityView,
    L: AuthorKeyLifecycleView,
    K: ScopeKeyStore + Send + Sync,
    P: PayloadCiphertextSource,
    S: PayloadSchemaRegistry,
    T: Clock,
{
    /// Composes the verifier from its seams.
    pub fn new(parts: ComposedVerifierParts<C, M, L, K, P, S, T>) -> Self {
        Self { parts }
    }

    /// The composed seams, for a caller that must also read them (for example custody).
    pub fn parts(&self) -> &ComposedVerifierParts<C, M, L, K, P, S, T> {
        &self.parts
    }

    /// Releases the seams.
    pub fn into_parts(self) -> ComposedVerifierParts<C, M, L, K, P, S, T> {
        self.parts
    }

    /// The scope key whose derived `key_id` equals `key_id`, searched from the operation's exact
    /// scope outward to its verse. The `key_id` commits to `(scope, epoch, key)`, so a wrong
    /// candidate cannot match; no match is "key not held".
    fn payload_key(
        &self,
        unsigned: &UnsignedEnvelope,
        key_id: Identifier32,
    ) -> Option<&SealingKey> {
        let epoch = unsigned.capability.scope_epoch;
        enclosing_scopes(&unsigned.scope)
            .into_iter()
            .find_map(|scope| {
                let key = self
                    .parts
                    .scope_keys
                    .scope_key(&ScopeKeyId { scope, epoch })?;
                (derive_key_id(&scope, epoch, key).ok()? == key_id).then_some(key)
            })
    }
}

/// `scope`, then its petal-wide scope, then its verse-wide scope, without repeats.
fn enclosing_scopes(scope: &Scope) -> Vec<Scope> {
    let verse = Scope::verse_wide(scope.verse_id());
    let mut scopes = vec![*scope];
    if let (Some(petal), Some(_)) = (scope.petal_id(), scope.resource_id()) {
        if let Ok(petal_scope) = Scope::new(scope.verse_id(), Some(petal), None) {
            scopes.push(petal_scope);
        }
    }
    if *scope != verse {
        scopes.push(verse);
    }
    scopes
}

/// The one SPEC-2 evidence-gap quarantine for `gap`.
fn evidence_gap(gap: AuthorizationEvidenceGap) -> AdmissionOutcome {
    AdmissionOutcome::AuthorizationEvidenceUnavailable { reason: gap }
}

/// SPEC-3 §2.4: the persistent view holding no epoch for `epoch_scope` is missing evidence
/// (SPEC-2 rule 5); every other chain failure is a refusal (SPEC-4 §5 `unauthorized_operation`).
fn chain_failure(error: ChainError) -> AdmissionOutcome {
    match error {
        ChainError::EpochScopeUnknown => {
            evidence_gap(AuthorizationEvidenceGap::epoch_state_unavailable())
        }
        _ => AdmissionOutcome::UnauthorizedOperation,
    }
}

/// The §2.4 request an `append/op` of this envelope asks, every field read from the signed
/// envelope itself. Requester and author are both the envelope author: nobody else exercises it.
fn append_request(unsigned: &UnsignedEnvelope, now_ms: u64) -> AuthorizationRequest {
    let author: Principal = unsigned.author.clone();
    let payload = &unsigned.payload;
    let plaintext_bytes = if payload.is_no_payload_form() {
        0
    } else {
        payload
            .ciphertext_length
            .saturating_sub(AEAD_TAG_LENGTH as u64)
    };
    AuthorizationRequest {
        verb: Verb::Append,
        object_class: ObjectClass::Operation,
        target_scope: unsigned.scope,
        resource_id: unsigned.scope.resource_id(),
        requester: author.clone(),
        operation_author: author,
        operation_scope_epoch: unsigned.capability.scope_epoch,
        operation_kind: Some(unsigned.operation_kind),
        is_structural_operation: is_structural(unsigned.operation_kind),
        payload_bytes: Some(plaintext_bytes),
        segment_bytes: None,
        schema_hash: Some(unsigned.schema_hash),
        preview_hz: None,
        now_ms,
    }
}

/// SPEC-3 §4.7: kinds 2-4 are the SPEC-1 §6 structural operations.
fn is_structural(operation_kind: u16) -> bool {
    matches!(
        OperationKind::from_u16(operation_kind),
        Ok(OperationKind::BranchGenesis
            | OperationKind::DetachedMerge
            | OperationKind::ScopeEpochBump)
    )
}

#[async_trait]
impl<C, M, L, K, P, S, T> CandidateVerifier for ComposedCandidateVerifier<C, M, L, K, P, S, T>
where
    C: CapabilityChainSource,
    M: ManagerAuthorityView,
    L: AuthorKeyLifecycleView,
    K: ScopeKeyStore + Send + Sync,
    P: PayloadCiphertextSource,
    S: PayloadSchemaRegistry,
    T: Clock,
{
    async fn verify_envelope(
        &self,
        candidate_bytes: &[u8],
    ) -> Result<VerifiedCandidate, AdmissionOutcome> {
        let candidate = verify_candidate(candidate_bytes)?;
        let operation_kind = candidate.meta.operation_kind;
        match OperationKind::from_u16(operation_kind) {
            Ok(kind)
                if kind.requires_registered_interpretation()
                    && !self.parts.schemas.interprets_kind(operation_kind) =>
            {
                Err(AdmissionOutcome::UnknownKind { operation_kind })
            }
            Ok(_) => Ok(candidate),
            Err(error) => Err(AdmissionOutcome::InvalidEnvelope {
                reason: error.to_string(),
            }),
        }
    }

    async fn verify_authorization(
        &self,
        candidate: &VerifiedCandidate,
    ) -> Result<(), AdmissionOutcome> {
        let unsigned = &candidate.envelope.unsigned;
        let chain_id = unsigned.capability.chain_id;
        let chain_bytes = self.parts.chains.chain_bytes(&chain_id).ok_or_else(|| {
            evidence_gap(AuthorizationEvidenceGap::capability_chain_unavailable())
        })?;

        let request = append_request(unsigned, self.parts.clock.now_ms());
        let verified = verify_chain_bytes(
            &chain_bytes,
            &request,
            &self.parts.authority,
            &AuthorizationSnapshot {
                epoch_causally_valid_for_operation: false,
            },
            &VerificationOptions::live(),
        )
        .map_err(chain_failure)?;
        // A source answering for one id with another chain's bytes proves nothing about this one.
        if verified.chain_id != chain_id {
            return Err(AdmissionOutcome::UnauthorizedOperation);
        }

        match self.parts.lifecycle.key_state(
            &unsigned.author.public_key,
            &unsigned.scope,
            &unsigned.parents,
        ) {
            // SPEC-2 §4 rule 4 (retired on this causal path) and rule 6 (fork: neither side is
            // active); `Unknown` is a key no lineage evidence reaches, whose authority is the
            // chain just verified (§4 rule 2).
            KeyState::Retired { .. } | KeyState::ForkedUnresolved { .. } => {
                Err(AdmissionOutcome::UnauthorizedOperation)
            }
            KeyState::Active | KeyState::Unknown => Ok(()),
        }
    }

    async fn verify_payload(
        &self,
        candidate: &VerifiedCandidate,
        payload_ciphertext: Option<&[u8]>,
    ) -> Result<Option<PayloadFacts>, AdmissionOutcome> {
        let unsigned = &candidate.envelope.unsigned;
        let payload = &unsigned.payload;
        let Some(encryption) = payload.encryption else {
            // The no-payload form commits to the empty ciphertext; bytes beside it are a lie.
            return match payload_ciphertext {
                Some(bytes) if !bytes.is_empty() => Err(AdmissionOutcome::InvalidPayload),
                _ => Ok(None),
            };
        };

        let (ciphertext, supplied) = match payload_ciphertext {
            Some(bytes) => (Cow::Borrowed(bytes), true),
            None => match self.parts.ciphertexts.ciphertext(&payload.ciphertext_hash) {
                Some(bytes) => (Cow::Owned(bytes), false),
                None => {
                    return Err(AdmissionOutcome::OpaquePayload {
                        op_id: candidate.meta.op_id,
                    })
                }
            },
        };
        if ciphertext.len() as u64 != payload.ciphertext_length
            || Hash32::of(&ciphertext) != payload.ciphertext_hash
        {
            // Supplied bytes are the candidate's own claim; a local store answering with the
            // wrong bytes says nothing about the candidate, so the payload stays opaque.
            return Err(if supplied {
                AdmissionOutcome::InvalidPayload
            } else {
                AdmissionOutcome::OpaquePayload {
                    op_id: candidate.meta.op_id,
                }
            });
        }

        let key = self
            .payload_key(unsigned, encryption.key_id)
            .ok_or_else(|| evidence_gap(AuthorizationEvidenceGap::payload_key_unavailable()))?;
        let aad = payload_aad_preimage(unsigned).map_err(|_| AdmissionOutcome::InvalidPayload)?;
        let plaintext = Zeroizing::new(
            aead::open(key, &encryption, &aad, &ciphertext)
                .map_err(|_| AdmissionOutcome::InvalidPayload)?,
        );

        match self.parts.schemas.validate(
            &unsigned.schema_hash,
            unsigned.operation_kind,
            &plaintext,
        ) {
            SchemaVerdict::Valid => {
                PayloadFacts::from_validated_plaintext(candidate, &plaintext).map(Some)
            }
            SchemaVerdict::Invalid => Err(AdmissionOutcome::InvalidPayload),
            SchemaVerdict::UnknownSchema => Err(AdmissionOutcome::UnknownSchema {
                schema_hash: unsigned.schema_hash,
            }),
            SchemaVerdict::UnknownKind => Err(AdmissionOutcome::UnknownKind {
                operation_kind: unsigned.operation_kind,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::compose::fixtures::{
        hash, petal_scope, verse_scope, ChainShelf, ComposedFixture, FixedClock, ManagerView,
        NoCiphertexts, NoLifecycle, SchemaTable, AUTHOR_SEED, EPOCH, NOW_MS, SCHEMA,
    };
    use crate::crypto::scope_key::InMemoryScopeKeyStore;
    use crate::materialize::errors::AdmissionDisposition;
    use crate::materialize::traits::admit_candidate_with_facts;

    #[tokio::test]
    async fn a_signed_operation_with_a_live_chain_and_a_decryptable_payload_is_admitted() {
        let fixture = ComposedFixture::new(EPOCH);
        let operation = fixture.operation(AUTHOR_SEED, vec![hash(0x01)], 1);
        let verifier = fixture.verifier(EPOCH, NoLifecycle);

        let admitted = admit_candidate_with_facts(
            &verifier,
            &fixture.no_equivocation(),
            &fixture.same_verse_parents(),
            &operation.bytes,
            Some(&operation.ciphertext),
        )
        .await
        .expect("a live chain, an active author, and an openable payload admit");
        assert_eq!(admitted.candidate.meta.op_id, operation.op_id);
        let facts = admitted
            .payload_facts
            .expect("a payload-bearing operation has facts");
        assert_eq!(facts.op_id, operation.op_id);
        assert_eq!(facts.schema_hash, SCHEMA);
        assert_eq!(facts.fields, operation.plaintext_fields);

        // Each payload failure lands in its own SPEC-4 §5 row: a tampered byte the candidate
        // itself supplied is a reject, absent bytes are opaque, and an absent key is a gap.
        let candidate = verifier
            .verify_envelope(&operation.bytes)
            .await
            .expect("ingress");
        let mut tampered = operation.ciphertext.clone();
        tampered[0] ^= 0x01;
        assert_eq!(
            verifier.verify_payload(&candidate, Some(&tampered)).await,
            Err(AdmissionOutcome::InvalidPayload)
        );
        assert_eq!(
            verifier.verify_payload(&candidate, None).await,
            Err(AdmissionOutcome::OpaquePayload {
                op_id: operation.op_id
            })
        );
        let keyless = fixture.verifier_with_keys(EPOCH, NoLifecycle, InMemoryScopeKeyStore::new());
        assert_eq!(
            keyless
                .verify_payload(&candidate, Some(&operation.ciphertext))
                .await,
            Err(AdmissionOutcome::AuthorizationEvidenceUnavailable {
                reason: AuthorizationEvidenceGap::payload_key_unavailable()
            })
        );
    }

    #[tokio::test]
    async fn the_same_operation_is_refused_after_its_scope_epoch_is_bumped() {
        let fixture = ComposedFixture::new(EPOCH);
        let operation = fixture.operation(AUTHOR_SEED, vec![hash(0x01)], 1);

        let live = fixture.verifier(EPOCH, NoLifecycle);
        let candidate = live
            .verify_envelope(&operation.bytes)
            .await
            .expect("ingress");
        assert_eq!(live.verify_authorization(&candidate).await, Ok(()));

        // The persistent view moved to EPOCH + 1: the chain still verifies byte-for-byte, but
        // §2.4 step 9 now refuses it, and the refusal is a reject, not a retryable quarantine.
        let bumped = fixture.verifier(EPOCH + 1, NoLifecycle);
        let refused = bumped.verify_authorization(&candidate).await;
        assert_eq!(refused, Err(AdmissionOutcome::UnauthorizedOperation));
        assert_eq!(
            refused.unwrap_err().disposition(),
            AdmissionDisposition::Reject
        );
    }

    #[tokio::test]
    async fn a_candidate_the_verifier_cannot_yet_judge_is_quarantined_with_the_missing_evidence_named(
    ) {
        let fixture = ComposedFixture::new(EPOCH);
        let operation = fixture.operation(AUTHOR_SEED, vec![hash(0x01)], 1);
        let unregistered_kind = fixture.operation(AUTHOR_SEED, vec![hash(0x01)], 0x0200);

        // No chain held locally: SPEC-2 rule 5 quarantine, never a reject (erratum E-W4-1).
        let chainless = ComposedCandidateVerifier::new(ComposedVerifierParts {
            chains: ChainShelf::default(),
            authority: ManagerView::at_epoch(EPOCH),
            lifecycle: NoLifecycle,
            scope_keys: fixture.scope_keys(),
            ciphertexts: NoCiphertexts,
            schemas: SchemaTable::kind_one_only(),
            clock: FixedClock(NOW_MS),
        });
        let candidate = chainless
            .verify_envelope(&operation.bytes)
            .await
            .expect("ingress");
        let chain_gap = chainless.verify_authorization(&candidate).await;
        assert_eq!(
            chain_gap,
            Err(AdmissionOutcome::AuthorizationEvidenceUnavailable {
                reason: AuthorizationEvidenceGap::capability_chain_unavailable()
            })
        );

        // The view records no epoch for the chain's epoch scope at all.
        let epochless = fixture.verifier_with_view(ManagerView::without_epoch_state(), NoLifecycle);
        let epoch_gap = epochless.verify_authorization(&candidate).await;
        assert_eq!(
            epoch_gap,
            Err(AdmissionOutcome::AuthorizationEvidenceUnavailable {
                reason: AuthorizationEvidenceGap::epoch_state_unavailable()
            })
        );

        // A validly signed kind this build has no rule, schema, and reduction for (SPEC-4 §5
        // `unknown_kind`): quarantined at the envelope, never `InvalidEnvelope`.
        let verifier = fixture.verifier(EPOCH, NoLifecycle);
        let unknown_kind = verifier.verify_envelope(&unregistered_kind.bytes).await;
        assert_eq!(
            unknown_kind,
            Err(AdmissionOutcome::UnknownKind {
                operation_kind: 0x0200
            })
        );

        for outcome in [chain_gap, epoch_gap, unknown_kind.map(|_| ())] {
            assert!(
                outcome.expect_err("held").is_quarantine(),
                "missing evidence is a quarantine"
            );
        }
    }

    #[tokio::test]
    async fn a_payload_key_held_at_the_verse_opens_a_petal_scoped_operation() {
        // The fixture's scope key lives at verse scope; this operation is petal-scoped, so the
        // key is found only by walking outward and matching the committed `key_id`.
        let fixture = ComposedFixture::new(EPOCH);
        let operation = fixture.operation_in(AUTHOR_SEED, petal_scope(), vec![hash(0x01)], 1);
        let verifier = fixture.verifier(EPOCH, NoLifecycle);
        let candidate = verifier
            .verify_envelope(&operation.bytes)
            .await
            .expect("ingress");
        assert!(verifier
            .verify_payload(&candidate, Some(&operation.ciphertext))
            .await
            .expect("opens")
            .is_some());
        assert_ne!(operation.scope, verse_scope());
    }
}
