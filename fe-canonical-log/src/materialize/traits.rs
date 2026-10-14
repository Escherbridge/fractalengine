//! SPEC-4 admission facts and the store/verifier/materializer trait seams a caller composes
//! into an append-then-replay pipeline; see `src/AGENTS.md` §module-ownership and
//! `materialize/AGENTS.md`.
//!
//! Nothing here touches storage. `VerifiedLogStore` and `CausalMaterializer` are the seams a
//! later, database-backed slice implements; this module also supplies the pure orchestration
//! that needs no durable state -- [`admit_candidate`] chains envelope verification with the
//! SPEC-1 §6.2 same-verse-parent check and the M1 author-equivocation check, both against
//! caller-supplied lookups rather than a store this crate would have to own.

use async_trait::async_trait;

use crate::cbor::CborValue;
use crate::envelope::{CompleteEnvelope, EquivocationKey, Hash32, Identifier32, Scope};
use crate::materialize::errors::{AdmissionOutcome, AppendError, AppendOutcome};
use crate::signing::SigningError;

/// The materializer-facing facts extracted from one admitted envelope (SPEC-4 §2).
///
/// Amended by M2 to carry `scope`: SPEC-1 §6.2 same-verse parent checking, SPEC-2 disavow
/// scope matching, SPEC-2 scope-propagated lineage resolution, and SPEC-3 permission cells are
/// all unimplementable without it.
///
/// **Sealed against struct literals outside this crate** (`#[non_exhaustive]`): an external
/// crate obtains a value only from [`VerifiedEnvelopeMeta::verify_from_bytes`],
/// [`verify_envelope_meta`], or [`verify_and_project_envelope`] -- the doors that run
/// `signing::decode_and_admit` first -- or from the hazardous unverified projection
/// [`verified_envelope_meta_from`], which remains a public door (see its HAZARD note and
/// `materialize/AGENTS.md`). The seal therefore closes the literal door only; it does not make
/// every value proof of verification. Fields stay `pub` for reading.
///
/// An external struct literal, even with every field correctly typed, is refused:
///
/// ```compile_fail,E0639
/// use fe_canonical_log::envelope::{Hash32, Identifier32, Scope};
/// let _forged = fe_canonical_log::materialize::VerifiedEnvelopeMeta {
///     op_id: Hash32([0x01; 32]),
///     operation_kind: 100,
///     scope: Scope::verse_wide(Identifier32([0x02; 32])),
///     branch_id: Identifier32([0x03; 32]),
///     schema_hash: Hash32([0x04; 32]),
///     parents: Vec::new(),
///     author_public_key: [0x05; 32],
///     wall_ms: 1_000,
///     counter: 0,
/// };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifiedEnvelopeMeta {
    /// Content address of the complete envelope.
    pub op_id: Hash32,
    /// The raw non-zero wire operation kind; classify with `kind::OperationKind::from_u16`.
    pub operation_kind: u16,
    /// Authorization scope tuple this operation was authored under (M2).
    pub scope: Scope,
    /// Branch this operation belongs to.
    pub branch_id: Identifier32,
    /// Hash of the schema validating the decrypted intent payload.
    pub schema_hash: Hash32,
    /// Strictly ascending, unique DAG parent operation IDs.
    pub parents: Vec<Hash32>,
    /// Raw Ed25519 public key of the author.
    pub author_public_key: [u8; 32],
    /// Author's HLC wall-clock milliseconds.
    pub wall_ms: u64,
    /// Author's HLC logical counter.
    pub counter: u32,
}

impl VerifiedEnvelopeMeta {
    /// The §3.4 equivocation identity of this operation.
    pub fn equivocation_key(&self) -> EquivocationKey {
        EquivocationKey {
            author_public_key: self.author_public_key,
            wall_ms: self.wall_ms,
            counter: self.counter,
        }
    }

    /// The default constructor: verifies `candidate_bytes` through the mandatory ingress, then
    /// projects them. Reach for this, not [`verified_envelope_meta_from`].
    pub fn verify_from_bytes(candidate_bytes: &[u8]) -> Result<Self, SigningError> {
        verify_and_project_envelope(candidate_bytes).map(|(_, meta)| meta)
    }
}

/// Verifies `candidate_bytes` through `signing::decode_and_admit` and projects the result.
///
/// This is the construction path that earns the type's name. `decode_and_admit` decodes
/// canonically, asserts the received bytes are their own canonical re-encoding, checks the §3.2
/// author binding, verifies the §5.1 signature, applies the §6 structural rules, refuses every
/// non-production payload suite, and content-addresses the bytes AS RECEIVED -- so the returned
/// `op_id` is derived, never chosen by the caller.
///
/// The [`CompleteEnvelope`] is returned alongside the facts so a caller that must also append or
/// re-encode the operation does not decode it a second time, and so no second decode can
/// disagree with the one the signature was checked against.
pub fn verify_and_project_envelope(
    candidate_bytes: &[u8],
) -> Result<(CompleteEnvelope, VerifiedEnvelopeMeta), SigningError> {
    let (complete, op_id) = crate::signing::decode_and_admit(candidate_bytes)?;
    let meta = verified_envelope_meta_from(&complete, op_id);
    Ok((complete, meta))
}

/// [`verify_and_project_envelope`] with its failure mapped onto the §5.1 `InvalidEnvelope`
/// reject: the whole body a [`CandidateVerifier::verify_envelope`] implementation needs.
pub fn verify_envelope_meta(
    candidate_bytes: &[u8],
) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome> {
    VerifiedEnvelopeMeta::verify_from_bytes(candidate_bytes).map_err(|error| {
        AdmissionOutcome::InvalidEnvelope {
            reason: error.to_string(),
        }
    })
}

/// One candidate that passed the mandatory ingress: its materializer-facing facts and the
/// complete envelope they were projected from (`materialize/AGENTS.md` §verified-candidate).
///
/// Possession is proof that `signing::decode_and_admit` ran on the candidate's bytes:
/// [`VerifiedCandidate::verify_from_bytes`] (and [`verify_candidate`], which wraps it) is the
/// only constructor outside this crate. The envelope travels with the facts because SPEC-3
/// authorization needs its `CapabilityRef` and payload verification needs its `PayloadRef` and
/// the §5.2 AAD preimage, none of which [`VerifiedEnvelopeMeta`] carries.
///
/// An external struct literal is refused:
///
/// ```compile_fail,E0639
/// # fn forge(
/// #     meta: fe_canonical_log::materialize::VerifiedEnvelopeMeta,
/// #     envelope: fe_canonical_log::envelope::CompleteEnvelope,
/// # ) {
/// let _forged = fe_canonical_log::materialize::VerifiedCandidate { meta, envelope };
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifiedCandidate {
    /// The facts a materializer reduces from.
    pub meta: VerifiedEnvelopeMeta,
    /// The complete envelope `meta` was projected from, as `decode_and_admit` returned it.
    pub envelope: CompleteEnvelope,
}

impl VerifiedCandidate {
    /// Verifies `candidate_bytes` through the mandatory ingress and keeps both halves.
    pub fn verify_from_bytes(candidate_bytes: &[u8]) -> Result<Self, SigningError> {
        verify_and_project_envelope(candidate_bytes).map(|(envelope, meta)| Self { meta, envelope })
    }
}

/// [`VerifiedCandidate::verify_from_bytes`] with its failure mapped onto the §5.1
/// `InvalidEnvelope` reject: the ingress half of a [`CandidateVerifier::verify_envelope`].
pub fn verify_candidate(candidate_bytes: &[u8]) -> Result<VerifiedCandidate, AdmissionOutcome> {
    VerifiedCandidate::verify_from_bytes(candidate_bytes).map_err(|error| {
        AdmissionOutcome::InvalidEnvelope {
            reason: error.to_string(),
        }
    })
}

/// The decrypted, schema-validated payload of ONE admitted operation, as extracted facts
/// (SPEC-4 §4 rule 8 "extracted facts"; SPEC-10 §4.4 rule 2 and §12; `materialize/AGENTS.md`
/// §payload-facts).
///
/// Values only: no artifact identifier to dereference, no ciphertext, no key, no handle. A
/// reduction reading these reads its own operation and nothing else, so G2 still holds.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PayloadFacts {
    /// The operation these facts belong to; a reduction compares it with its own `meta.op_id`.
    pub op_id: Hash32,
    /// The schema the plaintext was validated against (the envelope's own `schema_hash`).
    pub schema_hash: Hash32,
    /// The decoded canonical-CBOR plaintext.
    pub fields: CborValue,
}

impl PayloadFacts {
    /// Extracts facts from `plaintext`, the payload `candidate` committed to, AFTER the caller
    /// authenticated its decryption and validated it against `candidate.meta.schema_hash`.
    ///
    /// Refuses (`InvalidPayload`) a no-payload-form candidate and plaintext that is not
    /// canonical CBOR. It cannot see the schema, so the validation precondition is the
    /// caller's: a [`CandidateVerifier::verify_payload`] implementation calls this last.
    pub fn from_validated_plaintext(
        candidate: &VerifiedCandidate,
        plaintext: &[u8],
    ) -> Result<Self, AdmissionOutcome> {
        if candidate.envelope.unsigned.payload.is_no_payload_form() {
            return Err(AdmissionOutcome::InvalidPayload);
        }
        let fields = crate::cbor::decode_canonical(plaintext)
            .map_err(|_| AdmissionOutcome::InvalidPayload)?;
        Ok(Self {
            op_id: candidate.meta.op_id,
            schema_hash: candidate.meta.schema_hash,
            fields,
        })
    }
}

/// Projects an envelope's fields into [`VerifiedEnvelopeMeta`] **without verifying anything**.
///
/// # HAZARD — the precondition this function does not check
///
/// This is not an admission check and never was. It performs no signature check, no canonical
/// re-encoding check, no §3.2 author binding check, no §6 structural check, and no payload-suite
/// check, and it does not derive `op_id` -- it copies the one it is handed. Calling it on an
/// envelope that has not already been through `signing::decode_and_admit` mints a
/// `VerifiedEnvelopeMeta` whose name is a lie, and every downstream check that trusts the type
/// (equivocation identity, same-verse parents, authorization, ordering) then runs on
/// attacker-chosen facts.
///
/// The caller MUST have already discharged BOTH of these, for these exact bytes:
///
/// 1. `complete` is the value returned by `signing::decode_and_admit` (or by
///    [`verify_and_project_envelope`], which wraps it); and
/// 2. `op_id` is the hash that same call returned, computed over the bytes as received.
///
/// If you cannot point at the `decode_and_admit` call that produced both arguments, you are
/// using the wrong function: use [`VerifiedEnvelopeMeta::verify_from_bytes`] or
/// [`verify_envelope_meta`] instead. This door survives only for callers reading back bytes
/// this process already admitted and stored, where re-verification is redundant rather than
/// absent -- and even there, re-verifying is the safer default.
pub fn verified_envelope_meta_from(
    complete: &CompleteEnvelope,
    op_id: Hash32,
) -> VerifiedEnvelopeMeta {
    let unsigned = &complete.unsigned;
    VerifiedEnvelopeMeta {
        op_id,
        operation_kind: unsigned.operation_kind,
        scope: unsigned.scope,
        branch_id: unsigned.branch_id,
        schema_hash: unsigned.schema_hash,
        parents: unsigned.parents.clone(),
        author_public_key: unsigned.author.public_key,
        wall_ms: unsigned.hlc.wall_ms,
        counter: unsigned.hlc.counter,
    }
}

/// One deterministic reduction step, an explicit record of why a verified operation was
/// excluded from the projection, or an availability state that blocks reduction entirely. §4.6
/// requires an excluded operation's evidence to remain representable, never silently dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionMutation {
    /// Apply these statements with these bindings to the projection.
    Apply {
        /// Storage-agnostic statement strings (for example SurrealQL); the storage-backed
        /// implementer of `VerifiedLogStore`/`CausalMaterializer` interprets them.
        statements: Vec<String>,
        /// Named bindings for the statements, in this crate's own canonical value model so no
        /// storage-specific value type leaks into a pure trait seam.
        bindings: Vec<(String, CborValue)>,
    },
    /// The operation is verified and durably appended, but deliberately not projected.
    Excluded {
        /// Why this operation is excluded. Evidence, never destroyed, only never applied.
        reason: String,
    },
    /// The operation names an external artifact whose identity this process cannot currently
    /// resolve, so no deterministic reduction is possible here yet (SPEC-4 §5
    /// `referenced_artifact_unavailable`, D-CL28 gate G2).
    ///
    /// An availability state, never a decision, and the opposite of [`Self::Excluded`]: the
    /// caller MUST leave the projection position pending replay rather than advance its apply
    /// marker, and MUST NOT substitute a default, a partial value, or an empty reference. It is
    /// a distinct variant precisely so that "cannot compute yet" cannot be encoded as
    /// "deliberately not projected", which would let two peers disagree about a settled result.
    ///
    /// This is NOT the state of having failed to read an artifact's contents. §4 rule 8 forbids
    /// a reduction from reading into an external artifact to compute projected state at all; a
    /// reduction writes only a deterministic *reference*. This outcome arises when the reference
    /// itself cannot be constructed as a verified artifact identity — for example when the
    /// segment manifest binding the artifact ID and its stored length is not locally held.
    ReferencedArtifactUnavailable {
        /// The artifact the operation names.
        artifact_id: Hash32,
    },
}

/// Verifies a candidate operation, mapping every failure onto [`AdmissionOutcome`] (the §2.1-
/// §2.3 validate-then-log pipeline order). A storage-backed implementer composes this with
/// [`admit_candidate`] below and its own `VerifiedLogStore::append`.
#[async_trait]
pub trait CandidateVerifier: Send + Sync {
    /// Decodes, verifies signature/structure/suite, and keeps the verified envelope with its
    /// facts. Implementers call [`verify_candidate`] and may add only the SPEC-4 §5
    /// `unknown_kind` check ([`AdmissionOutcome::UnknownKind`], never `InvalidEnvelope`).
    async fn verify_envelope(
        &self,
        candidate_bytes: &[u8],
    ) -> Result<VerifiedCandidate, AdmissionOutcome>;

    /// Checks the operation is authorized under its presented capability (SPEC-3, injected).
    /// An unresolvable chain or epoch state is
    /// [`AdmissionOutcome::AuthorizationEvidenceUnavailable`], never a refusal.
    async fn verify_authorization(
        &self,
        candidate: &VerifiedCandidate,
    ) -> Result<(), AdmissionOutcome>;

    /// Checks the payload decrypts and validates against its declared schema, and returns its
    /// extracted facts (`None` for the no-payload form). `payload_ciphertext` is the ciphertext
    /// the caller already holds, if any; an implementer may resolve it elsewhere when `None`.
    async fn verify_payload(
        &self,
        candidate: &VerifiedCandidate,
        payload_ciphertext: Option<&[u8]>,
    ) -> Result<Option<PayloadFacts>, AdmissionOutcome>;
}

/// The exactly-once, durable log of admitted operation bytes (§3.1-§3.6). Implemented against
/// SurrealDB in `fe-database/src/canon_log/`; this crate defines only the seam.
#[async_trait]
pub trait VerifiedLogStore: Send + Sync {
    /// Appends `bytes` under `claimed_op_id`. `AlreadyPresent` (byte-identical) is success.
    ///
    /// `HashMismatch` and `IntegrityConflict` are DEFINITE answers that §3.3 treats as
    /// permanently blacklisting the `op_id`. A store that could not answer -- an I/O fault --
    /// returns the indeterminate `StorageUnavailable`, and stored bytes that fail to decode in
    /// THIS process (a future protocol version, say) return the indeterminate `NotAnEnvelope`.
    /// An implementer MUST NOT map either condition onto `IntegrityConflict`; see
    /// `src/AGENTS.md` §append-errors.
    async fn append(
        &self,
        claimed_op_id: Hash32,
        bytes: Vec<u8>,
    ) -> Result<AppendOutcome, AppendError>;

    /// The complete canonical bytes previously appended under `op_id`, if any.
    async fn get_bytes(&self, op_id: Hash32) -> Option<Vec<u8>>;

    /// The materializer-facing facts previously extracted for `op_id`, if any.
    async fn get_meta(&self, op_id: Hash32) -> Option<VerifiedEnvelopeMeta>;

    /// The direct parents of an already-appended operation, if it is present.
    async fn parents_of(&self, op_id: Hash32) -> Option<Vec<Hash32>>;
}

/// One deterministic reduction step (§4.1-§4.8): given the facts of an admitted operation, its
/// complete bytes, and its own extracted payload facts, produce the projection mutation.
/// Calling this twice with the same inputs MUST produce an equal [`ProjectionMutation`] -- that
/// equality is what makes replay idempotent: a caller can rebuild projected state from verified
/// operations plus checkpoints alone by replaying `VerifiedLogStore` contents, in
/// `ordering::deterministic_causal_order`, through this trait.
///
/// **The §4 rule 8 bright line (D-CL28 gate G2).** `meta`, `envelope_bytes`, and
/// `payload_facts` -- the admitted operation's OWN verified bytes and extracted facts -- are the
/// ONLY admissible inputs to a reduction. An implementation MUST NOT read an external artifact
/// -- a snapshot, a segment shard, a columnar hexon, another peer's bytes -- to compute
/// projected state, because local availability of that artifact differs per peer and §4.5/§6.1
/// require the same operation set to produce the same projection everywhere. A reduction may
/// write a deterministic *reference* to such an artifact (its content address, derived from the
/// signed envelope), and when that reference cannot itself be resolved to a verified identity it
/// returns [`ProjectionMutation::ReferencedArtifactUnavailable`] rather than guessing.
///
/// `payload_facts` is `None` for the no-payload form AND whenever the caller holds no decrypted
/// payload for this operation. A reduction that needs facts and receives `None` returns
/// `ReferencedArtifactUnavailable` naming the envelope's `PayloadRef::ciphertext_hash`, never
/// `Excluded` and never a default (`materialize/AGENTS.md` §payload-facts). Kinds that need no
/// payload ignore the argument.
#[async_trait]
pub trait CausalMaterializer: Send + Sync {
    /// Reduces one verified operation into a projection mutation.
    async fn reduce(
        &self,
        meta: &VerifiedEnvelopeMeta,
        envelope_bytes: &[u8],
        payload_facts: Option<&PayloadFacts>,
    ) -> ProjectionMutation;
}

/// Looks up whether an `EquivocationKey` already names an admitted `op_id` (D-CL25, §3.4).
#[async_trait]
pub trait EquivocationIndex: Send + Sync {
    /// The op_id already admitted at this key, if any.
    async fn op_id_at(&self, key: EquivocationKey) -> Option<Hash32>;
}

/// Looks up the verse a parent operation belongs to, for the SPEC-1 §6.2 same-verse check.
#[async_trait]
pub trait ParentVerseLookup: Send + Sync {
    /// The verse a parent operation was admitted into, if it is locally known.
    async fn verse_id_of(&self, parent_op_id: Hash32) -> Option<Identifier32>;
}

/// SPEC-1 §6.2: every parent must share this operation's verse scope. A cross-verse parent is
/// a permanent structural violation (§5.1 reject), not an availability gap.
pub fn validate_same_verse_parents(
    candidate_verse_id: Identifier32,
    parents: impl IntoIterator<Item = (Hash32, Identifier32)>,
) -> Result<(), AdmissionOutcome> {
    for (parent_op_id, parent_verse_id) in parents {
        if parent_verse_id != candidate_verse_id {
            return Err(AdmissionOutcome::SameVerseParentViolation {
                parent: parent_op_id,
            });
        }
    }
    Ok(())
}

/// D-CL25 / M1: quarantine BOTH candidates when two distinct `op_id`s share an
/// `EquivocationKey`; materialize NEITHER until an authorized resolution operation runs.
pub fn check_author_equivocation(
    candidate: &VerifiedEnvelopeMeta,
    existing_op_id_at_key: Option<Hash32>,
) -> Result<(), AdmissionOutcome> {
    let Some(existing) = existing_op_id_at_key else {
        return Ok(());
    };
    if existing == candidate.op_id {
        return Ok(());
    }
    Err(AdmissionOutcome::AuthorEquivocation {
        op_id: candidate.op_id,
        conflicting_op_id: existing,
    })
}

/// One candidate [`admit_candidate_with_facts`] admitted: the verified candidate and the payload
/// facts its verifier extracted. Produced only by admission.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AdmittedCandidate {
    /// The candidate, verified through the mandatory ingress.
    pub candidate: VerifiedCandidate,
    /// Its decrypted, schema-validated payload facts; `None` for the no-payload form.
    pub payload_facts: Option<PayloadFacts>,
}

/// The §2.1-§2.3 validate-then-log pipeline, minus the final append: verify the envelope,
/// enforce SPEC-1 §6.2 same-verse parents, enforce the M1 author-equivocation rule, then verify
/// authorization and payload. A storage-backed caller appends only after this returns `Ok`;
/// an unresolvable parent lookup returns `MissingParent` rather than provisionally applying the
/// candidate.
///
/// Returns only the facts; [`admit_candidate_with_facts`] is the same pipeline keeping the
/// verified envelope and the payload facts the verifier extracted.
pub async fn admit_candidate<V, E, P>(
    verifier: &V,
    equivocation_index: &E,
    parent_verse_lookup: &P,
    candidate_bytes: &[u8],
    payload_ciphertext: Option<&[u8]>,
) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome>
where
    V: CandidateVerifier + ?Sized,
    E: EquivocationIndex + ?Sized,
    P: ParentVerseLookup + ?Sized,
{
    admit_candidate_with_facts(
        verifier,
        equivocation_index,
        parent_verse_lookup,
        candidate_bytes,
        payload_ciphertext,
    )
    .await
    .map(|admitted| admitted.candidate.meta)
}

/// [`admit_candidate`], keeping the [`VerifiedCandidate`] and its [`PayloadFacts`].
pub async fn admit_candidate_with_facts<V, E, P>(
    verifier: &V,
    equivocation_index: &E,
    parent_verse_lookup: &P,
    candidate_bytes: &[u8],
    payload_ciphertext: Option<&[u8]>,
) -> Result<AdmittedCandidate, AdmissionOutcome>
where
    V: CandidateVerifier + ?Sized,
    E: EquivocationIndex + ?Sized,
    P: ParentVerseLookup + ?Sized,
{
    let candidate = verifier.verify_envelope(candidate_bytes).await?;
    let meta = &candidate.meta;

    let mut parent_verse_ids = Vec::with_capacity(meta.parents.len());
    for parent_op_id in &meta.parents {
        match parent_verse_lookup.verse_id_of(*parent_op_id).await {
            Some(verse_id) => parent_verse_ids.push((*parent_op_id, verse_id)),
            None => {
                return Err(AdmissionOutcome::MissingParent {
                    missing: *parent_op_id,
                })
            }
        }
    }
    validate_same_verse_parents(meta.scope.verse_id(), parent_verse_ids)?;

    let existing = equivocation_index.op_id_at(meta.equivocation_key()).await;
    check_author_equivocation(meta, existing)?;

    verifier.verify_authorization(&candidate).await?;
    let payload_facts = verifier
        .verify_payload(&candidate, payload_ciphertext)
        .await?;

    Ok(AdmittedCandidate {
        candidate,
        payload_facts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn sample_meta(
        op_id_filler: u8,
        verse_filler: u8,
        parents: Vec<Hash32>,
    ) -> VerifiedEnvelopeMeta {
        VerifiedEnvelopeMeta {
            op_id: hash(op_id_filler),
            operation_kind: 1,
            scope: Scope::verse_wide(identifier(verse_filler)),
            branch_id: identifier(0xaa),
            schema_hash: hash(0xbb),
            parents,
            author_public_key: [0xcc; 32],
            wall_ms: 1_000,
            counter: 0,
        }
    }

    /// Answers a fixed meta for any bytes. In-crate only: the struct literal pairs `meta` with an
    /// unrelated signed envelope, which is exactly what the seal forbids outside this crate.
    struct FixedVerifier(VerifiedEnvelopeMeta);

    #[async_trait]
    impl CandidateVerifier for FixedVerifier {
        async fn verify_envelope(
            &self,
            _candidate_bytes: &[u8],
        ) -> Result<VerifiedCandidate, AdmissionOutcome> {
            Ok(VerifiedCandidate {
                meta: self.0.clone(),
                envelope: signed_sample().0,
            })
        }

        async fn verify_authorization(
            &self,
            _candidate: &VerifiedCandidate,
        ) -> Result<(), AdmissionOutcome> {
            Ok(())
        }

        async fn verify_payload(
            &self,
            _candidate: &VerifiedCandidate,
            _payload_ciphertext: Option<&[u8]>,
        ) -> Result<Option<PayloadFacts>, AdmissionOutcome> {
            Ok(None)
        }
    }

    struct FixedEquivocationIndex(Option<Hash32>);

    #[async_trait]
    impl EquivocationIndex for FixedEquivocationIndex {
        async fn op_id_at(&self, _key: EquivocationKey) -> Option<Hash32> {
            self.0
        }
    }

    struct FixedParentVerseLookup(HashMap<Hash32, Identifier32>);

    #[async_trait]
    impl ParentVerseLookup for FixedParentVerseLookup {
        async fn verse_id_of(&self, parent_op_id: Hash32) -> Option<Identifier32> {
            self.0.get(&parent_op_id).copied()
        }
    }

    #[tokio::test]
    async fn admit_candidate_succeeds_when_parents_share_the_verse_and_no_equivocation_exists() {
        let parent = hash(0x01);
        let meta = sample_meta(0x10, 0x99, vec![parent]);
        let verifier = FixedVerifier(meta.clone());
        let equivocation_index = FixedEquivocationIndex(None);
        let mut parents = HashMap::new();
        parents.insert(parent, identifier(0x99));
        let parent_lookup = FixedParentVerseLookup(parents);

        let admitted = admit_candidate(
            &verifier,
            &equivocation_index,
            &parent_lookup,
            b"unused",
            None,
        )
        .await
        .expect("admission");
        assert_eq!(admitted, meta);
    }

    #[tokio::test]
    async fn admit_candidate_rejects_a_cross_verse_parent() {
        let parent = hash(0x01);
        let meta = sample_meta(0x10, 0x99, vec![parent]);
        let verifier = FixedVerifier(meta.clone());
        let equivocation_index = FixedEquivocationIndex(None);
        let mut parents = HashMap::new();
        parents.insert(parent, identifier(0x77));
        let parent_lookup = FixedParentVerseLookup(parents);

        let outcome = admit_candidate(
            &verifier,
            &equivocation_index,
            &parent_lookup,
            b"unused",
            None,
        )
        .await
        .expect_err("cross-verse parent must be rejected");
        assert_eq!(
            outcome,
            AdmissionOutcome::SameVerseParentViolation { parent }
        );
        assert!(outcome.is_reject());
    }

    #[tokio::test]
    async fn admit_candidate_quarantines_on_a_missing_parent() {
        let parent = hash(0x01);
        let meta = sample_meta(0x10, 0x99, vec![parent]);
        let verifier = FixedVerifier(meta);
        let equivocation_index = FixedEquivocationIndex(None);
        let parent_lookup = FixedParentVerseLookup(HashMap::new());

        let outcome = admit_candidate(
            &verifier,
            &equivocation_index,
            &parent_lookup,
            b"unused",
            None,
        )
        .await
        .expect_err("unresolvable parent must quarantine");
        assert_eq!(outcome, AdmissionOutcome::MissingParent { missing: parent });
        assert!(outcome.is_quarantine());
    }

    #[tokio::test]
    async fn admit_candidate_quarantines_both_op_ids_on_author_equivocation() {
        let meta = sample_meta(0x10, 0x99, vec![]);
        let conflicting = hash(0x20);
        let verifier = FixedVerifier(meta.clone());
        let equivocation_index = FixedEquivocationIndex(Some(conflicting));
        let parent_lookup = FixedParentVerseLookup(HashMap::new());

        let outcome = admit_candidate(
            &verifier,
            &equivocation_index,
            &parent_lookup,
            b"unused",
            None,
        )
        .await
        .expect_err("equivocation must quarantine");
        assert_eq!(
            outcome,
            AdmissionOutcome::AuthorEquivocation {
                op_id: meta.op_id,
                conflicting_op_id: conflicting,
            }
        );
        assert!(outcome.is_quarantine());
    }

    #[tokio::test]
    async fn admit_candidate_does_not_flag_equivocation_when_the_existing_op_id_is_itself() {
        let meta = sample_meta(0x10, 0x99, vec![]);
        let verifier = FixedVerifier(meta.clone());
        let equivocation_index = FixedEquivocationIndex(Some(meta.op_id));
        let parent_lookup = FixedParentVerseLookup(HashMap::new());

        let admitted = admit_candidate(
            &verifier,
            &equivocation_index,
            &parent_lookup,
            b"unused",
            None,
        )
        .await
        .expect("re-admitting the same op_id is not equivocation");
        assert_eq!(admitted, meta);
    }

    struct RecordingMaterializer {
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl CausalMaterializer for RecordingMaterializer {
        async fn reduce(
            &self,
            meta: &VerifiedEnvelopeMeta,
            _envelope_bytes: &[u8],
            _payload_facts: Option<&PayloadFacts>,
        ) -> ProjectionMutation {
            *self.calls.lock().expect("lock") += 1;
            ProjectionMutation::Apply {
                statements: vec![format!("-- reduce {:?}", meta.op_id)],
                bindings: vec![(
                    "op_id".to_string(),
                    CborValue::Bytes(meta.op_id.as_bytes().to_vec()),
                )],
            }
        }
    }

    #[tokio::test]
    async fn causal_materializer_reduce_is_pure_and_replays_identically() {
        let meta = sample_meta(0x30, 0x40, vec![]);
        let materializer = RecordingMaterializer {
            calls: Mutex::new(0),
        };

        let first = materializer.reduce(&meta, b"bytes", None).await;
        let second = materializer.reduce(&meta, b"bytes", None).await;

        assert_eq!(
            first, second,
            "replaying the same operation must reduce identically"
        );
        assert_eq!(*materializer.calls.lock().expect("lock"), 2);
    }

    /// Gate G2: a materializer that cannot resolve a referenced artifact has a state to say so,
    /// and that state is not confusable with a deliberate exclusion. Determinism is preserved
    /// because the outcome depends only on the operation, never on what this peer happens to
    /// hold: the same materializer returns the same unavailable outcome on every replay.
    struct ArtifactReferencingMaterializer {
        resolvable: bool,
    }

    #[async_trait]
    impl CausalMaterializer for ArtifactReferencingMaterializer {
        async fn reduce(
            &self,
            meta: &VerifiedEnvelopeMeta,
            _envelope_bytes: &[u8],
            _payload_facts: Option<&PayloadFacts>,
        ) -> ProjectionMutation {
            if !self.resolvable {
                return ProjectionMutation::ReferencedArtifactUnavailable {
                    artifact_id: meta.schema_hash,
                };
            }
            // Only a reference is written. The artifact's CONTENTS are never read to compute
            // this mutation -- that is the §4 rule 8 bright line.
            ProjectionMutation::Apply {
                statements: vec!["-- reference only".to_string()],
                bindings: vec![(
                    "artifact_id".to_string(),
                    CborValue::Bytes(meta.schema_hash.as_bytes().to_vec()),
                )],
            }
        }
    }

    #[tokio::test]
    async fn referenced_artifact_unavailable_is_expressible_and_distinct_from_exclusion() {
        let meta = sample_meta(0x50, 0x60, vec![]);

        let blocked = ArtifactReferencingMaterializer { resolvable: false };
        let outcome = blocked.reduce(&meta, b"bytes", None).await;
        assert_eq!(
            outcome,
            ProjectionMutation::ReferencedArtifactUnavailable {
                artifact_id: meta.schema_hash,
            }
        );
        assert_ne!(
            outcome,
            ProjectionMutation::Excluded {
                reason: "referenced artifact unavailable".to_string(),
            },
            "an availability state must not be representable as a settled exclusion"
        );
        assert_eq!(
            outcome,
            blocked.reduce(&meta, b"bytes", None).await,
            "the unavailable outcome replays identically"
        );

        let resolved = ArtifactReferencingMaterializer { resolvable: true };
        assert!(matches!(
            resolved.reduce(&meta, b"bytes", None).await,
            ProjectionMutation::Apply { .. }
        ));
    }

    #[test]
    fn verified_envelope_meta_from_projects_the_admitted_envelope_fields() {
        use crate::envelope::{
            Author, CapabilityRef, Hlc, PayloadRef, UnsignedEnvelope, PROTOCOL_VERSION,
        };
        use crate::signing::sign_envelope;
        use ed25519_dalek::SigningKey;

        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 2,
            scope: Scope::verse_wide(identifier(0x11)),
            author: Author::from_public_key(signing_key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: hash(0x33),
                scope_epoch: 5,
            },
            schema_hash: hash(0x44),
            branch_id: identifier(0x55),
            parents: Vec::new(),
            hlc: Hlc::new(42, 7),
            payload: PayloadRef::empty(),
        };
        let complete = sign_envelope(&signing_key, &unsigned).expect("sign");
        let bytes = complete.encode_canonical().expect("encode");
        let op_id = crate::signing::op_id(&bytes);

        let meta = verified_envelope_meta_from(&complete, op_id);
        assert_eq!(meta.op_id, op_id);
        assert_eq!(meta.operation_kind, 2);
        assert_eq!(meta.scope, unsigned.scope);
        assert_eq!(meta.branch_id, unsigned.branch_id);
        assert_eq!(meta.schema_hash, unsigned.schema_hash);
        assert_eq!(meta.parents, unsigned.parents);
        assert_eq!(meta.author_public_key, unsigned.author.public_key);
        assert_eq!(meta.wall_ms, 42);
        assert_eq!(meta.counter, 7);
    }

    /// Builds a signed, admissible envelope and its canonical bytes.
    fn signed_sample() -> (crate::envelope::CompleteEnvelope, Vec<u8>) {
        use crate::envelope::{
            Author, CapabilityRef, Hlc, PayloadRef, UnsignedEnvelope, PROTOCOL_VERSION,
        };
        use crate::signing::sign_envelope;
        use ed25519_dalek::SigningKey;

        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 2,
            scope: Scope::verse_wide(identifier(0x11)),
            author: Author::from_public_key(signing_key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: hash(0x33),
                scope_epoch: 5,
            },
            schema_hash: hash(0x44),
            branch_id: identifier(0x55),
            parents: Vec::new(),
            hlc: Hlc::new(42, 7),
            payload: PayloadRef::empty(),
        };
        let complete = sign_envelope(&signing_key, &unsigned).expect("sign");
        let bytes = complete.encode_canonical().expect("encode");
        (complete, bytes)
    }

    #[test]
    fn verify_from_bytes_derives_the_op_id_rather_than_accepting_one() {
        let (complete, bytes) = signed_sample();

        let meta = VerifiedEnvelopeMeta::verify_from_bytes(&bytes).expect("admissible bytes");
        assert_eq!(meta.op_id, crate::signing::op_id(&bytes));
        assert_eq!(meta, verified_envelope_meta_from(&complete, meta.op_id));

        let (returned_envelope, same_meta) =
            verify_and_project_envelope(&bytes).expect("admissible bytes");
        assert_eq!(returned_envelope, complete);
        assert_eq!(same_meta, meta);

        assert_eq!(verify_envelope_meta(&bytes), Ok(meta));
    }

    #[test]
    fn the_projection_mints_meta_for_an_envelope_the_verifying_door_refuses() {
        use crate::envelope::Hlc;

        // A forged envelope: the HLC is moved after signing, so the signature no longer covers
        // the bytes. The projection cannot tell; the verifying door refuses outright.
        let (mut forged, _) = signed_sample();
        forged.unsigned.hlc = Hlc::new(43, 7);
        let forged_bytes = forged.encode_canonical().expect("encode");
        let forged_op_id = crate::signing::op_id(&forged_bytes);

        let minted = verified_envelope_meta_from(&forged, forged_op_id);
        assert_eq!(
            minted.wall_ms, 43,
            "the projection happily reports a field no signature covers"
        );

        assert_eq!(
            VerifiedEnvelopeMeta::verify_from_bytes(&forged_bytes),
            Err(SigningError::SignatureVerificationFailed),
        );
        assert_eq!(
            verify_envelope_meta(&forged_bytes),
            Err(AdmissionOutcome::InvalidEnvelope {
                reason: SigningError::SignatureVerificationFailed.to_string(),
            }),
            "the verifying door maps a forged envelope onto the §5.1 reject"
        );
        assert!(verify_envelope_meta(&forged_bytes)
            .expect_err("forged")
            .is_reject());
    }

    #[test]
    fn the_verifying_door_refuses_bytes_that_are_not_their_own_canonical_re_encoding() {
        let (_, bytes) = signed_sample();
        let mut trailing = bytes.clone();
        trailing.push(0x00);

        assert!(
            VerifiedEnvelopeMeta::verify_from_bytes(&trailing).is_err(),
            "trailing bytes are never admissible"
        );
        assert!(VerifiedEnvelopeMeta::verify_from_bytes(&bytes[..bytes.len() - 1]).is_err());
    }

    /// A payload-bearing kind-100 envelope whose payload is `plaintext` sealed under
    /// `scope_key` with the §5.2 AAD, and the ciphertext the envelope commits to.
    fn signed_payload_sample(
        scope_key: &crate::crypto::aead::SealingKey,
        plaintext: &[u8],
    ) -> (Vec<u8>, Vec<u8>) {
        use crate::crypto::aead::{seal, FreshNonce};
        use crate::envelope::{
            Author, CapabilityRef, Hlc, PayloadRef, UnsignedEnvelope, PROTOCOL_VERSION,
        };
        use crate::payload_aad::payload_aad_preimage;
        use crate::signing::sign_envelope;
        use ed25519_dalek::SigningKey;

        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let mut unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 100,
            scope: Scope::verse_wide(identifier(0x11)),
            author: Author::from_public_key(signing_key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: hash(0x33),
                scope_epoch: 5,
            },
            schema_hash: hash(0x44),
            branch_id: identifier(0x55),
            parents: Vec::new(),
            hlc: Hlc::new(42, 7),
            // §5.2: the AAD binds the encryption map but not the ciphertext hash or length, so
            // the placeholder reference is replaced after sealing without changing the AAD.
            payload: PayloadRef {
                encryption: Some(nonce.params()),
                ..PayloadRef::empty()
            },
        };
        let aad = payload_aad_preimage(&unsigned).expect("aad");
        let sealed = seal(scope_key, nonce, &aad, plaintext).expect("seal");
        unsigned.payload = sealed.payload_ref();
        let bytes = sign_envelope(&signing_key, &unsigned)
            .expect("sign")
            .encode_canonical()
            .expect("encode");
        (bytes, sealed.ciphertext)
    }

    /// Opens the payload the way a production verifier must -- hash, length, AEAD under the
    /// §5.2 AAD -- and extracts facts only after it authenticates. The schema check is a fixed
    /// "map with key 0" stand-in for the registry W4-3 composes.
    struct DecryptingVerifier {
        scope_key: crate::crypto::aead::SealingKey,
    }

    #[async_trait]
    impl CandidateVerifier for DecryptingVerifier {
        async fn verify_envelope(
            &self,
            candidate_bytes: &[u8],
        ) -> Result<VerifiedCandidate, AdmissionOutcome> {
            verify_candidate(candidate_bytes)
        }

        async fn verify_authorization(
            &self,
            _candidate: &VerifiedCandidate,
        ) -> Result<(), AdmissionOutcome> {
            Ok(())
        }

        async fn verify_payload(
            &self,
            candidate: &VerifiedCandidate,
            payload_ciphertext: Option<&[u8]>,
        ) -> Result<Option<PayloadFacts>, AdmissionOutcome> {
            let payload = &candidate.envelope.unsigned.payload;
            let Some(encryption) = payload.encryption else {
                return Ok(None);
            };
            let ciphertext = payload_ciphertext.ok_or(AdmissionOutcome::OpaquePayload {
                op_id: candidate.meta.op_id,
            })?;
            if ciphertext.len() as u64 != payload.ciphertext_length
                || Hash32::of(ciphertext) != payload.ciphertext_hash
            {
                return Err(AdmissionOutcome::InvalidPayload);
            }
            let aad = crate::payload_aad::payload_aad_preimage(&candidate.envelope.unsigned)
                .map_err(|_| AdmissionOutcome::InvalidPayload)?;
            let plaintext =
                crate::crypto::aead::open(&self.scope_key, &encryption, &aad, ciphertext)
                    .map_err(|_| AdmissionOutcome::InvalidPayload)?;
            let facts = PayloadFacts::from_validated_plaintext(candidate, &plaintext)?;
            match &facts.fields {
                CborValue::Map(entries)
                    if entries.iter().any(|(key, _)| *key == CborValue::Uint(0)) =>
                {
                    Ok(Some(facts))
                }
                _ => Err(AdmissionOutcome::InvalidPayload),
            }
        }
    }

    /// A kind-100 reduction that needs its payload's field 0: it writes `op_id` plus that one
    /// extracted value, and nothing derived from any artifact.
    struct FactReadingMaterializer;

    #[async_trait]
    impl CausalMaterializer for FactReadingMaterializer {
        async fn reduce(
            &self,
            meta: &VerifiedEnvelopeMeta,
            envelope_bytes: &[u8],
            payload_facts: Option<&PayloadFacts>,
        ) -> ProjectionMutation {
            let Some(facts) = payload_facts.filter(|facts| facts.op_id == meta.op_id) else {
                // No decrypted payload here: a pending availability state naming the payload
                // artifact the operation's own signed bytes commit to -- never an exclusion.
                let envelope = CompleteEnvelope::decode_canonical(envelope_bytes)
                    .expect("admitted bytes decode");
                return ProjectionMutation::ReferencedArtifactUnavailable {
                    artifact_id: envelope.unsigned.payload.ciphertext_hash,
                };
            };
            let field_zero = match &facts.fields {
                CborValue::Map(entries) => entries
                    .iter()
                    .find(|(key, _)| *key == CborValue::Uint(0))
                    .map(|(_, value)| value.clone()),
                _ => None,
            }
            .expect("schema-validated facts carry field 0");
            ProjectionMutation::Apply {
                statements: vec!["-- reference row: op_id + field 0".to_string()],
                bindings: vec![
                    (
                        "op_id".to_string(),
                        CborValue::Bytes(meta.op_id.as_bytes().to_vec()),
                    ),
                    ("field_zero".to_string(), field_zero),
                ],
            }
        }
    }

    #[tokio::test]
    async fn reduce_reads_its_own_payload_facts_and_stays_reference_only() {
        let scope_key = crate::crypto::aead::SealingKey::from_bytes([0x5a; 32]);
        let plaintext = crate::cbor::encode_canonical(&CborValue::Map(vec![(
            CborValue::Uint(0),
            CborValue::Text("node-a".to_string()),
        )]));
        let (bytes, ciphertext) = signed_payload_sample(&scope_key, &plaintext);
        let verifier = DecryptingVerifier { scope_key };
        let no_equivocation = FixedEquivocationIndex(None);
        let no_parents = FixedParentVerseLookup(HashMap::new());

        // Admission decrypts and validates once, and hands the facts on.
        let admitted = admit_candidate_with_facts(
            &verifier,
            &no_equivocation,
            &no_parents,
            &bytes,
            Some(&ciphertext),
        )
        .await
        .expect("a decryptable, schema-valid payload admits");
        let meta = admitted.candidate.meta.clone();
        let facts = admitted
            .payload_facts
            .expect("payload-bearing kind has facts");
        assert_eq!(facts.op_id, meta.op_id);
        assert_eq!(facts.schema_hash, meta.schema_hash);

        // The reduction reads them through the widened signature and writes a reference row
        // carrying only the operation's own extracted value -- identically on every replay.
        let materializer = FactReadingMaterializer;
        let mutation = materializer.reduce(&meta, &bytes, Some(&facts)).await;
        assert_eq!(
            mutation,
            ProjectionMutation::Apply {
                statements: vec!["-- reference row: op_id + field 0".to_string()],
                bindings: vec![
                    (
                        "op_id".to_string(),
                        CborValue::Bytes(meta.op_id.as_bytes().to_vec()),
                    ),
                    (
                        "field_zero".to_string(),
                        CborValue::Text("node-a".to_string()),
                    ),
                ],
            }
        );
        assert_eq!(
            mutation,
            materializer.reduce(&meta, &bytes, Some(&facts)).await
        );

        // Without facts the same reduction is pending on the payload artifact, not excluded.
        let pending = materializer.reduce(&meta, &bytes, None).await;
        assert_eq!(
            pending,
            ProjectionMutation::ReferencedArtifactUnavailable {
                artifact_id: Hash32::of(&ciphertext),
            }
        );

        // Admission without the ciphertext is the §5 opaque_payload quarantine, so facts are
        // never minted from nothing.
        let opaque = admit_candidate(&verifier, &no_equivocation, &no_parents, &bytes, None)
            .await
            .expect_err("no ciphertext, no facts");
        assert_eq!(
            opaque,
            AdmissionOutcome::OpaquePayload { op_id: meta.op_id }
        );
        assert!(opaque.is_quarantine());
    }

    #[test]
    fn payload_facts_refuse_the_no_payload_form_and_non_canonical_plaintext() {
        let (_, bytes) = signed_sample();
        let candidate = VerifiedCandidate::verify_from_bytes(&bytes).expect("admissible");
        assert_eq!(
            PayloadFacts::from_validated_plaintext(&candidate, &[0xa0]),
            Err(AdmissionOutcome::InvalidPayload),
            "a no-payload operation has no payload to extract facts from"
        );

        let scope_key = crate::crypto::aead::SealingKey::from_bytes([0x5a; 32]);
        let (payload_bytes, _) = signed_payload_sample(&scope_key, &[0xa0]);
        let payload_candidate =
            VerifiedCandidate::verify_from_bytes(&payload_bytes).expect("admissible");
        // 0x18 0x01: the integer 1 in a non-shortest encoding -- not canonical CBOR.
        assert_eq!(
            PayloadFacts::from_validated_plaintext(&payload_candidate, &[0x18, 0x01]),
            Err(AdmissionOutcome::InvalidPayload)
        );
        assert!(PayloadFacts::from_validated_plaintext(&payload_candidate, &[0xa0]).is_ok());
    }
}
