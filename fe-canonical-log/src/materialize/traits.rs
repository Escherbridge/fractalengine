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

/// The materializer-facing facts extracted from one admitted envelope (SPEC-4 §2).
///
/// Amended by M2 to carry `scope`: SPEC-1 §6.2 same-verse parent checking, SPEC-2 disavow
/// scope matching, SPEC-2 scope-propagated lineage resolution, and SPEC-3 permission cells are
/// all unimplementable without it.
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
}

/// Builds [`VerifiedEnvelopeMeta`] from an envelope already admitted by
/// `signing::decode_and_admit`. This function verifies nothing itself -- it is a pure
/// projection of already-verified fields; the caller supplies the `op_id` computed over the
/// received bytes.
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

/// One deterministic reduction step, or an explicit record of why a verified operation was
/// excluded from the projection. §4.6 requires an excluded operation's evidence to remain
/// representable, never silently dropped.
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
}

/// Verifies a candidate operation, mapping every failure onto [`AdmissionOutcome`] (the §2.1-
/// §2.3 validate-then-log pipeline order). A storage-backed implementer composes this with
/// [`admit_candidate`] below and its own `VerifiedLogStore::append`.
#[async_trait]
pub trait CandidateVerifier: Send + Sync {
    /// Decodes, verifies signature/structure/suite, and extracts materializer-facing facts.
    /// Implementers use `signing::decode_and_admit` plus [`verified_envelope_meta_from`].
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
    /// Appends `bytes` under `claimed_op_id`. `AlreadyPresent` (byte-identical) is success;
    /// `HashMismatch`/`IntegrityConflict` are the only failure modes (§3.1).
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

/// One deterministic reduction step (§4.1-§4.7): given the facts of an admitted operation and
/// its complete bytes, produce the projection mutation. Calling this twice with the same
/// inputs MUST produce an equal [`ProjectionMutation`] -- that equality is what makes replay
/// idempotent: a caller can rebuild projected state from verified operations plus checkpoints
/// alone by replaying `VerifiedLogStore` contents, in `ordering::deterministic_causal_order`,
/// through this trait.
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
}
