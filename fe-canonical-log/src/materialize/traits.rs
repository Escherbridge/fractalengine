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
/// **The name is a claim about provenance, and the type does not enforce it.** Every field is
/// `pub`, so a value of this type is evidence of verification only when it came out of
/// [`VerifiedEnvelopeMeta::verify_from_bytes`], [`verify_envelope_meta`], or
/// [`verify_and_project_envelope`] -- the three doors that run `signing::decode_and_admit`
/// first. A value built by struct literal, or by the hazardous projection
/// [`verified_envelope_meta_from`], carries no cryptographic weight whatsoever; treating one as
/// admitted is the mistake this note exists to prevent.
///
/// The fields stay public, and the type is deliberately NOT `#[non_exhaustive]`, because
/// out-of-crate test harnesses in `fe-database` build fixture values with struct literals.
/// `capability::VerifiedCapability` closes the same seam structurally precisely because it has
/// no such consumer; until this one's literals are gone, the guarantee here is a documented
/// obligation, not a type-level one, and must not be described as anything stronger.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Decodes, verifies signature/structure/suite, and extracts materializer-facing facts.
    /// Implementers call [`verify_envelope_meta`] and add nothing; reaching for
    /// [`verified_envelope_meta_from`] here skips every check this method's name promises.
    async fn verify_envelope(
        &self,
        candidate_bytes: &[u8],
    ) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome>;

    /// Checks the operation is authorized under its presented capability (SPEC-3, injected).
    async fn verify_authorization(
        &self,
        meta: &VerifiedEnvelopeMeta,
    ) -> Result<(), AdmissionOutcome>;

    /// Checks the payload decrypts and validates against its declared schema.
    async fn verify_payload(&self, meta: &VerifiedEnvelopeMeta) -> Result<(), AdmissionOutcome>;
}

/// The exactly-once, durable log of admitted operation bytes (§3.1-§3.6). Implemented against
/// SurrealDB in `fe-database/src/canon_log/`; this crate defines only the seam.
#[async_trait]
pub trait VerifiedLogStore: Send + Sync {
    /// Appends `bytes` under `claimed_op_id`. `AlreadyPresent` (byte-identical) is success.
    ///
    /// `AppendError` currently offers only `HashMismatch` and `IntegrityConflict`, both of which
    /// are DEFINITE answers that §3.3 treats as permanently blacklisting the `op_id`. It has no
    /// indeterminate variant, so an implementer whose storage could not answer -- an I/O fault,
    /// or stored bytes that fail to decode in THIS process for a reason as innocent as a future
    /// protocol version -- has nothing honest to return and, today, reports a definite failure
    /// for an operation it never actually examined. Filed as a `fe-canonical-log` erratum:
    /// `AppendError` needs a `StorageUnavailable` variant, and until it lands an implementer
    /// MUST NOT map a local decode failure onto `IntegrityConflict`.
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

/// One deterministic reduction step (§4.1-§4.8): given the facts of an admitted operation and
/// its complete bytes, produce the projection mutation. Calling this twice with the same
/// inputs MUST produce an equal [`ProjectionMutation`] -- that equality is what makes replay
/// idempotent: a caller can rebuild projected state from verified operations plus checkpoints
/// alone by replaying `VerifiedLogStore` contents, in `ordering::deterministic_causal_order`,
/// through this trait.
///
/// **The §4 rule 8 bright line (D-CL28 gate G2).** `meta` and `envelope_bytes` are the ONLY
/// admissible inputs to a reduction. An implementation MUST NOT read an external artifact --
/// a snapshot, a segment shard, a columnar hexon, another peer's bytes -- to compute projected
/// state, because local availability of that artifact differs per peer and §4.5/§6.1 require
/// the same operation set to produce the same projection everywhere. A reduction may write a
/// deterministic *reference* to such an artifact (its content address, derived from the signed
/// envelope), and when that reference cannot itself be resolved to a verified identity it
/// returns [`ProjectionMutation::ReferencedArtifactUnavailable`] rather than guessing.
#[async_trait]
pub trait CausalMaterializer: Send + Sync {
    /// Reduces one verified operation into a projection mutation.
    async fn reduce(
        &self,
        meta: &VerifiedEnvelopeMeta,
        envelope_bytes: &[u8],
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

/// The §2.1-§2.3 validate-then-log pipeline, minus the final append: verify the envelope,
/// enforce SPEC-1 §6.2 same-verse parents, enforce the M1 author-equivocation rule, then verify
/// authorization and payload. A storage-backed caller appends only after this returns `Ok`;
/// an unresolvable parent lookup returns `MissingParent` rather than provisionally applying the
/// candidate.
pub async fn admit_candidate<V, E, P>(
    verifier: &V,
    equivocation_index: &E,
    parent_verse_lookup: &P,
    candidate_bytes: &[u8],
) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome>
where
    V: CandidateVerifier + ?Sized,
    E: EquivocationIndex + ?Sized,
    P: ParentVerseLookup + ?Sized,
{
    let meta = verifier.verify_envelope(candidate_bytes).await?;

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
    check_author_equivocation(&meta, existing)?;

    verifier.verify_authorization(&meta).await?;
    verifier.verify_payload(&meta).await?;

    Ok(meta)
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

    struct FixedVerifier(VerifiedEnvelopeMeta);

    #[async_trait]
    impl CandidateVerifier for FixedVerifier {
        async fn verify_envelope(
            &self,
            _candidate_bytes: &[u8],
        ) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome> {
            Ok(self.0.clone())
        }

        async fn verify_authorization(
            &self,
            _meta: &VerifiedEnvelopeMeta,
        ) -> Result<(), AdmissionOutcome> {
            Ok(())
        }

        async fn verify_payload(
            &self,
            _meta: &VerifiedEnvelopeMeta,
        ) -> Result<(), AdmissionOutcome> {
            Ok(())
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

        let admitted = admit_candidate(&verifier, &equivocation_index, &parent_lookup, b"unused")
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

        let outcome = admit_candidate(&verifier, &equivocation_index, &parent_lookup, b"unused")
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

        let outcome = admit_candidate(&verifier, &equivocation_index, &parent_lookup, b"unused")
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

        let outcome = admit_candidate(&verifier, &equivocation_index, &parent_lookup, b"unused")
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

        let admitted = admit_candidate(&verifier, &equivocation_index, &parent_lookup, b"unused")
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

        let first = materializer.reduce(&meta, b"bytes").await;
        let second = materializer.reduce(&meta, b"bytes").await;

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
        let outcome = blocked.reduce(&meta, b"bytes").await;
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
            blocked.reduce(&meta, b"bytes").await,
            "the unavailable outcome replays identically"
        );

        let resolved = ArtifactReferencingMaterializer { resolvable: true };
        assert!(matches!(
            resolved.reduce(&meta, b"bytes").await,
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
}
