//! Commit submission and acknowledgement (§4): `commit_submit`, `commit_ack`, `commit_delta`.

use async_trait::async_trait;

use crate::cbor::CborValue;
use crate::envelope::{Hash32, Identifier32, Scope};

use super::cursor::{CommittedDelta, DurableCursor, ProjectionIdentity};
use super::error::{ProtocolErrorCategory, WireError};
use super::session::SessionAuthorizationTable;
use super::subscription::SubscriptionRecord;
use super::{
    bytes_at, fixed_bytes_at, get, hash32_at, identifier32_at, require_current_session_generation,
    require_exact_keys, scope_at, scope_entry, uint_at,
};

const COMMIT_SUBMIT_CONTEXT: &str = "commit_submit";
const COMMIT_ACK_CONTEXT: &str = "commit_ack";
const COMMIT_DELTA_CONTEXT: &str = "commit_delta";

/// §4.1's NORMATIVE key table: the only body in SPEC-7 with a given integer-key grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSubmitBody {
    /// Current accepted session authorization generation.
    pub session_generation: u64,
    /// Binding that proves `append/op`.
    pub authorization_binding_id: [u8; 16],
    /// MUST equal BLAKE3 of the complete envelope bytes.
    pub claimed_op_id: Hash32,
    /// Exact signed SPEC-1 complete-envelope bytes.
    pub complete_envelope: Vec<u8>,
    /// Exact referenced artifact; `None` only for SPEC-1 no-payload operations.
    pub payload_ciphertext: Option<Vec<u8>>,
}

impl CommitSubmitBody {
    /// Encodes the §4.1 body map with its normative keys 0..4.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.session_generation)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.authorization_binding_id.to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.claimed_op_id.as_bytes().to_vec()),
            ),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.complete_envelope.clone()),
            ),
            (
                CborValue::Uint(4),
                match &self.payload_ciphertext {
                    Some(bytes) => CborValue::Bytes(bytes.clone()),
                    None => CborValue::Null,
                },
            ),
        ])
    }

    /// Decodes the §4.1 body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3, 4], COMMIT_SUBMIT_CONTEXT)?;
        let payload_slot = get(value, 4, COMMIT_SUBMIT_CONTEXT)?;
        Ok(Self {
            session_generation: uint_at(value, 0, COMMIT_SUBMIT_CONTEXT)?,
            authorization_binding_id: fixed_bytes_at::<16>(value, 1, COMMIT_SUBMIT_CONTEXT)?,
            claimed_op_id: hash32_at(value, 2, COMMIT_SUBMIT_CONTEXT)?,
            complete_envelope: bytes_at(value, 3, COMMIT_SUBMIT_CONTEXT)?.to_vec(),
            payload_ciphertext: if payload_slot.is_null() {
                None
            } else {
                Some(
                    payload_slot
                        .as_bytes()
                        .ok_or(WireError::WrongShape {
                            context: COMMIT_SUBMIT_CONTEXT,
                            key: 4,
                        })?
                        .to_vec(),
                )
            },
        })
    }
}

/// The §4.2 branch/scope/projection/cursor binding carried by `committed` and
/// `already_committed` acknowledgements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitBinding {
    /// Branch the operation committed to.
    pub branch_id: Identifier32,
    /// Exact operation scope.
    pub scope: Scope,
    /// Projection identity the operation is applied under.
    pub projection_identity: ProjectionIdentity,
    /// The durable cursor at which the operation is visible.
    pub cursor: DurableCursor,
}

/// The §4.2 acknowledgement state. Each variant serializes a DIFFERENT key set — there is no
/// shared optional-cursor field an implementation could accidentally populate for the wrong
/// state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitAckState {
    /// Admission or authorization failed; no verified append occurred.
    Rejected {
        /// One public error category.
        category: ProtocolErrorCategory,
    },
    /// Exact bytes are durably appended, but no projection cursor has committed them yet.
    /// MUST NEVER serialize a cursor, branch, scope, or projection-identity field.
    AcceptedPendingMaterialization,
    /// Applied in the named projection and visible at the supplied durable cursor.
    Committed(CommitBinding),
    /// Exact bytes were previously committed; the cursor identifies that or a later observation.
    AlreadyCommitted(CommitBinding),
}

/// The §4.2 `commit_ack` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitAckBody {
    /// Session generation this ack was produced under.
    pub session_generation: u64,
    /// Binding this ack correlates to.
    pub authorization_binding_id: [u8; 16],
    /// The `op_id` this ack reports on.
    pub claimed_op_id: Hash32,
    /// Disposition and, where applicable, its binding.
    pub state: CommitAckState,
}

fn encode_binding(
    entries: &mut Vec<(CborValue, CborValue)>,
    binding: &CommitBinding,
) -> Result<(), WireError> {
    entries.push((
        CborValue::Uint(5),
        CborValue::Bytes(binding.branch_id.as_bytes().to_vec()),
    ));
    entries.push((CborValue::Uint(6), scope_entry(&binding.scope)?));
    entries.push((CborValue::Uint(7), binding.projection_identity.to_cbor()));
    entries.push((CborValue::Uint(8), binding.cursor.to_cbor()?));
    Ok(())
}

fn decode_binding(value: &CborValue) -> Result<CommitBinding, WireError> {
    Ok(CommitBinding {
        branch_id: identifier32_at(value, 5, COMMIT_ACK_CONTEXT)?,
        scope: scope_at(value, 6, COMMIT_ACK_CONTEXT)?,
        projection_identity: ProjectionIdentity::from_cbor(get(value, 7, COMMIT_ACK_CONTEXT)?)?,
        cursor: DurableCursor::from_cbor(get(value, 8, COMMIT_ACK_CONTEXT)?)?,
    })
}

impl CommitAckBody {
    /// Encodes the body map. `AcceptedPendingMaterialization` emits ONLY keys 0..3; a caller
    /// can assert this structurally on the encoded bytes, not merely on a null field.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        let mut entries = vec![
            (CborValue::Uint(0), CborValue::Uint(self.session_generation)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.authorization_binding_id.to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.claimed_op_id.as_bytes().to_vec()),
            ),
        ];
        match &self.state {
            CommitAckState::Rejected { category } => {
                entries.push((CborValue::Uint(3), CborValue::Uint(0)));
                entries.push((CborValue::Uint(4), CborValue::Uint(category.to_u64())));
            }
            CommitAckState::AcceptedPendingMaterialization => {
                entries.push((CborValue::Uint(3), CborValue::Uint(1)));
            }
            CommitAckState::Committed(binding) => {
                entries.push((CborValue::Uint(3), CborValue::Uint(2)));
                encode_binding(&mut entries, binding)?;
            }
            CommitAckState::AlreadyCommitted(binding) => {
                entries.push((CborValue::Uint(3), CborValue::Uint(3)));
                encode_binding(&mut entries, binding)?;
            }
        }
        Ok(CborValue::Map(entries))
    }

    /// Decodes the body map, requiring exactly the key set its discriminant implies.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        for key in [0, 1, 2, 3] {
            if value.get_uint_key(key).is_none() {
                return Err(WireError::MissingKey {
                    context: COMMIT_ACK_CONTEXT,
                    key,
                });
            }
        }
        let session_generation = uint_at(value, 0, COMMIT_ACK_CONTEXT)?;
        let authorization_binding_id = fixed_bytes_at::<16>(value, 1, COMMIT_ACK_CONTEXT)?;
        let claimed_op_id = hash32_at(value, 2, COMMIT_ACK_CONTEXT)?;
        let discriminant = uint_at(value, 3, COMMIT_ACK_CONTEXT)?;

        let entries = value.as_map().ok_or(WireError::ExpectedMap {
            context: COMMIT_ACK_CONTEXT,
        })?;
        let state = match discriminant {
            0 => {
                if entries.len() != 5 {
                    return Err(WireError::UnexpectedKeys {
                        context: COMMIT_ACK_CONTEXT,
                    });
                }
                let category_raw = uint_at(value, 4, COMMIT_ACK_CONTEXT)?;
                CommitAckState::Rejected {
                    category: ProtocolErrorCategory::from_u64(category_raw).ok_or(
                        WireError::UnknownDiscriminant {
                            context: COMMIT_ACK_CONTEXT,
                            key: 4,
                            value: category_raw,
                        },
                    )?,
                }
            }
            1 => {
                if entries.len() != 4 {
                    return Err(WireError::UnexpectedKeys {
                        context: COMMIT_ACK_CONTEXT,
                    });
                }
                CommitAckState::AcceptedPendingMaterialization
            }
            2 => {
                if entries.len() != 8 {
                    return Err(WireError::UnexpectedKeys {
                        context: COMMIT_ACK_CONTEXT,
                    });
                }
                CommitAckState::Committed(decode_binding(value)?)
            }
            3 => {
                if entries.len() != 8 {
                    return Err(WireError::UnexpectedKeys {
                        context: COMMIT_ACK_CONTEXT,
                    });
                }
                CommitAckState::AlreadyCommitted(decode_binding(value)?)
            }
            other => {
                return Err(WireError::UnknownDiscriminant {
                    context: COMMIT_ACK_CONTEXT,
                    key: 3,
                    value: other,
                })
            }
        };

        Ok(Self {
            session_generation,
            authorization_binding_id,
            claimed_op_id,
            state,
        })
    }

    /// Reports whether the encoded body omits every binding-shaped key (5..8).
    pub fn carries_no_cursor_or_binding_fields(&self) -> bool {
        matches!(self.state, CommitAckState::AcceptedPendingMaterialization)
    }
}

/// The §4.3 `commit_delta` body.
///
/// `#[non_exhaustive]`, so outside this crate the only ways to obtain one are
/// [`CommitDeltaBody::for_subscription`] — which applies the §4.3.3 scope filter — and
/// [`CommitDeltaBody::from_cbor`], the receiving-side decoder. A struct literal that skipped
/// the filter is not expressible in the wave-3 transport; see `src/wire/AGENTS.md`
/// §the-delta-scope-filter.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitDeltaBody {
    /// The recipient's subscription.
    pub subscription_id: [u8; 16],
    /// Session generation this delta was authorized under.
    pub session_generation: u64,
    /// The committed `op_id`.
    pub op_id: Hash32,
    /// Branch the operation committed to.
    pub branch_id: Identifier32,
    /// Exact operation scope.
    pub scope: Scope,
    /// Projection identity this delta was materialized under.
    pub projection_identity: ProjectionIdentity,
    /// The durable cursor this delta advances the recipient to.
    pub cursor: DurableCursor,
    /// Authorized projection-change summary for the recipient's subscription scope.
    pub change_summary: Vec<u8>,
}

impl CommitDeltaBody {
    /// Builds the delta one recipient is authorized to receive, or refuses to build one.
    ///
    /// Applies §4.3.3 in the only place a delta can come from: the exact operation scope MUST
    /// be contained by the recipient's active subscription scope, and the branch and projection
    /// identity MUST be the ones that subscription is bound to.
    pub fn for_subscription(
        subscription_id: [u8; 16],
        subscription: &SubscriptionRecord,
        session_generation: u64,
        delta: &CommittedDelta,
    ) -> Result<Self, WireError> {
        if !is_delta_authorized_for_subscription(&subscription.scope, &delta.scope) {
            return Err(WireError::ScopeNotAuthorized);
        }
        if subscription.branch_id != delta.branch_id
            || subscription.projection_identity != delta.projection_identity
        {
            return Err(WireError::DeltaNotForSubscription);
        }
        Ok(Self {
            subscription_id,
            session_generation,
            op_id: delta.op_id,
            branch_id: delta.branch_id,
            scope: delta.scope,
            projection_identity: delta.projection_identity.clone(),
            cursor: delta.cursor.clone(),
            change_summary: delta.change_summary.clone(),
        })
    }

    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.subscription_id.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.session_generation)),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.op_id.as_bytes().to_vec()),
            ),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.branch_id.as_bytes().to_vec()),
            ),
            (CborValue::Uint(4), scope_entry(&self.scope)?),
            (CborValue::Uint(5), self.projection_identity.to_cbor()),
            (CborValue::Uint(6), self.cursor.to_cbor()?),
            (
                CborValue::Uint(7),
                CborValue::Bytes(self.change_summary.clone()),
            ),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3, 4, 5, 6, 7], COMMIT_DELTA_CONTEXT)?;
        Ok(Self {
            subscription_id: fixed_bytes_at::<16>(value, 0, COMMIT_DELTA_CONTEXT)?,
            session_generation: uint_at(value, 1, COMMIT_DELTA_CONTEXT)?,
            op_id: hash32_at(value, 2, COMMIT_DELTA_CONTEXT)?,
            branch_id: identifier32_at(value, 3, COMMIT_DELTA_CONTEXT)?,
            scope: scope_at(value, 4, COMMIT_DELTA_CONTEXT)?,
            projection_identity: ProjectionIdentity::from_cbor(get(
                value,
                5,
                COMMIT_DELTA_CONTEXT,
            )?)?,
            cursor: DurableCursor::from_cbor(get(value, 6, COMMIT_DELTA_CONTEXT)?)?,
            change_summary: bytes_at(value, 7, COMMIT_DELTA_CONTEXT)?.to_vec(),
        })
    }
}

/// What running the SPEC-4 log-first pipeline against one exact candidate produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineResult {
    /// A public disposition short of a verified append (admission/authorization failure).
    Rejected(ProtocolErrorCategory),
    /// A different byte string already exists for this `op_id`. MUST NOT substitute either
    /// string into a projection.
    IntegrityConflict,
    /// Exact bytes are durably appended, no projection cursor has committed them yet.
    AcceptedPendingMaterialization,
    /// Applied in the named projection and visible at the supplied durable cursor.
    Committed(CommitBinding),
    /// Exact bytes were already committed; identical resubmission returns this.
    AlreadyCommitted(CommitBinding),
}

/// The DB-free half of SPEC-4 commit admission: derive `op_id`, then admit/append/materialize
/// or dedup one exact candidate. The concrete implementation lives outside this crate.
#[async_trait]
pub trait CanonicalCommitPipeline: Send + Sync {
    /// Derives `op_id` from exact bytes. The pipeline's own authority — never the client's
    /// [`CommitSubmitBody::claimed_op_id`].
    fn derive_op_id(&self, complete_envelope_bytes: &[u8]) -> Hash32;

    /// Runs the SPEC-4 pipeline for one exact candidate, deduplicating by `op_id` and exact
    /// bytes and rejecting a differing byte string for an existing `op_id` as
    /// [`PipelineResult::IntegrityConflict`].
    async fn submit_candidate(
        &self,
        complete_envelope_bytes: &[u8],
        payload_ciphertext: Option<&[u8]>,
    ) -> PipelineResult;
}

/// Pure composition of one [`PipelineResult`] into a [`CommitAckBody`] — no I/O, directly
/// testable for the load-bearing "no cursor while pending" and "reject a claimed mismatch
/// before any append" properties.
pub fn build_commit_ack(
    session_generation: u64,
    authorization_binding_id: [u8; 16],
    claimed_op_id: Hash32,
    result: PipelineResult,
) -> CommitAckBody {
    let state = match result {
        PipelineResult::Rejected(category) => CommitAckState::Rejected { category },
        PipelineResult::IntegrityConflict => CommitAckState::Rejected {
            category: ProtocolErrorCategory::ConflictingOpBytes,
        },
        PipelineResult::AcceptedPendingMaterialization => {
            CommitAckState::AcceptedPendingMaterialization
        }
        PipelineResult::Committed(binding) => CommitAckState::Committed(binding),
        PipelineResult::AlreadyCommitted(binding) => CommitAckState::AlreadyCommitted(binding),
    };
    CommitAckBody {
        session_generation,
        authorization_binding_id,
        claimed_op_id,
        state,
    }
}

/// §4.3.3: whether a committed delta at `delta_scope` may be delivered to a subscriber whose
/// active exact scope is `subscription_scope`. Reuses [`Scope::contains`] exclusively; a
/// missing/absent scope is never treated as permission to broadcast.
///
/// [`CommitDeltaBody::for_subscription`] is its production caller; the predicate stays public so
/// a fan-out loop can pre-filter recipients before building any body.
pub fn is_delta_authorized_for_subscription(
    subscription_scope: &Scope,
    delta_scope: &Scope,
) -> bool {
    subscription_scope.contains(delta_scope)
}

/// Handles one `commit_submit`: revalidates the session generation, rejects a claimed `op_id`
/// mismatch BEFORE calling the pipeline, then delegates admission to `pipeline`.
pub async fn handle_commit_submit<P: CanonicalCommitPipeline + ?Sized>(
    pipeline: &P,
    sessions: &SessionAuthorizationTable,
    body: &CommitSubmitBody,
) -> CommitAckBody {
    if require_current_session_generation(sessions, body.session_generation).is_err() {
        return build_commit_ack(
            body.session_generation,
            body.authorization_binding_id,
            body.claimed_op_id,
            PipelineResult::Rejected(ProtocolErrorCategory::SessionGenerationInvalid),
        );
    }

    let derived_op_id = pipeline.derive_op_id(&body.complete_envelope);
    if derived_op_id != body.claimed_op_id {
        return build_commit_ack(
            body.session_generation,
            body.authorization_binding_id,
            body.claimed_op_id,
            PipelineResult::Rejected(ProtocolErrorCategory::InvalidOpId),
        );
    }

    let result = pipeline
        .submit_candidate(&body.complete_envelope, body.payload_ciphertext.as_deref())
        .await;
    build_commit_ack(
        body.session_generation,
        body.authorization_binding_id,
        body.claimed_op_id,
        result,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical_checked};
    use crate::frontier::SortedFrontier;

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn sample_projection() -> ProjectionIdentity {
        ProjectionIdentity {
            materializer_id: "fe-scene".to_owned(),
            version: 1,
        }
    }

    fn sample_binding() -> CommitBinding {
        CommitBinding {
            branch_id: Identifier32([0x22; 32]),
            scope: Scope::verse_wide(Identifier32([0x11; 32])),
            projection_identity: sample_projection(),
            cursor: DurableCursor::new(
                super::super::cursor::CursorTuple {
                    verse_id: Identifier32([0x11; 32]),
                    branch_id: Identifier32([0x22; 32]),
                    subscription_scope: Scope::verse_wide(Identifier32([0x11; 32])),
                    projection_identity: sample_projection(),
                },
                SortedFrontier::try_new([hash(0x77)])
                    .expect("frontier")
                    .commitment(),
                vec![9, 9, 9],
            ),
        }
    }

    #[test]
    fn commit_submit_round_trips_with_and_without_a_payload() {
        for payload_ciphertext in [Some(vec![1, 2, 3]), None] {
            let body = CommitSubmitBody {
                session_generation: 3,
                authorization_binding_id: [0xaa; 16],
                claimed_op_id: hash(0x55),
                complete_envelope: vec![9, 8, 7],
                payload_ciphertext,
            };
            let bytes = encode_canonical_checked(&body.to_cbor()).expect("encode");
            assert_eq!(
                CommitSubmitBody::from_cbor(&decode_canonical(&bytes).expect("decode"))
                    .expect("decode"),
                body
            );
        }
    }

    #[test]
    fn accepted_pending_materialization_carries_no_binding_or_cursor_key() {
        let ack = CommitAckBody {
            session_generation: 1,
            authorization_binding_id: [0xaa; 16],
            claimed_op_id: hash(0x55),
            state: CommitAckState::AcceptedPendingMaterialization,
        };
        let value = ack.to_cbor().expect("cbor");
        let entries = value.as_map().expect("map");
        assert_eq!(entries.len(), 4, "only session/binding/op_id/state keys");
        for forbidden_key in [5u64, 6, 7, 8] {
            assert!(
                value.get_uint_key(forbidden_key).is_none(),
                "key {forbidden_key} must be absent, not null"
            );
        }
        assert!(ack.carries_no_cursor_or_binding_fields());

        let bytes = encode_canonical_checked(&value).expect("encode");
        assert_eq!(
            CommitAckBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            ack
        );
    }

    #[test]
    fn committed_and_already_committed_always_carry_a_full_binding() {
        for state in [
            CommitAckState::Committed(sample_binding()),
            CommitAckState::AlreadyCommitted(sample_binding()),
        ] {
            let ack = CommitAckBody {
                session_generation: 1,
                authorization_binding_id: [0xaa; 16],
                claimed_op_id: hash(0x55),
                state,
            };
            let value = ack.to_cbor().expect("cbor");
            for required_key in [5u64, 6, 7, 8] {
                assert!(value.get_uint_key(required_key).is_some());
            }
            let bytes = encode_canonical_checked(&value).expect("encode");
            assert_eq!(
                CommitAckBody::from_cbor(&decode_canonical(&bytes).expect("decode"))
                    .expect("decode"),
                ack
            );
        }
    }

    #[test]
    fn rejected_round_trips_and_carries_only_a_category() {
        let ack = CommitAckBody {
            session_generation: 1,
            authorization_binding_id: [0xaa; 16],
            claimed_op_id: hash(0x55),
            state: CommitAckState::Rejected {
                category: ProtocolErrorCategory::InvalidOpId,
            },
        };
        let value = ack.to_cbor().expect("cbor");
        assert_eq!(value.as_map().expect("map").len(), 5);
        let bytes = encode_canonical_checked(&value).expect("encode");
        assert_eq!(
            CommitAckBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            ack
        );
    }

    fn sample_subscription(scope: Scope) -> SubscriptionRecord {
        SubscriptionRecord {
            authorization_binding_id: [0xaa; 16],
            branch_id: Identifier32([0x22; 32]),
            scope,
            projection_identity: sample_projection(),
        }
    }

    fn sample_committed_delta(scope: Scope) -> CommittedDelta {
        CommittedDelta {
            op_id: hash(0x66),
            branch_id: Identifier32([0x22; 32]),
            scope,
            projection_identity: sample_projection(),
            cursor: sample_binding().cursor,
            change_summary: vec![1, 1, 2, 3],
        }
    }

    #[test]
    fn commit_delta_round_trips() {
        let scope = Scope::verse_wide(Identifier32([0x11; 32]));
        let body = CommitDeltaBody::for_subscription(
            [0xcc; 16],
            &sample_subscription(scope),
            4,
            &sample_committed_delta(scope),
        )
        .expect("authorized delta");
        let bytes = encode_canonical_checked(&body.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            CommitDeltaBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            body
        );
    }

    #[test]
    fn a_delta_outside_the_subscription_scope_cannot_be_built() {
        let verse_wide = Scope::verse_wide(Identifier32([0x11; 32]));
        let petal_scope = Scope::new(
            Identifier32([0x11; 32]),
            Some(Identifier32([0x22; 32])),
            None,
        )
        .expect("petal scope");

        // A verse-wide subscription is authorized for a petal-scoped delta inside that verse.
        assert!(CommitDeltaBody::for_subscription(
            [0xcc; 16],
            &sample_subscription(verse_wide),
            4,
            &sample_committed_delta(petal_scope),
        )
        .is_ok());

        // The reverse — a broader delta to a narrower subscription — is refused outright.
        assert_eq!(
            CommitDeltaBody::for_subscription(
                [0xcc; 16],
                &sample_subscription(petal_scope),
                4,
                &sample_committed_delta(verse_wide),
            ),
            Err(WireError::ScopeNotAuthorized)
        );

        // A different verse never authorizes a broadcast.
        assert_eq!(
            CommitDeltaBody::for_subscription(
                [0xcc; 16],
                &sample_subscription(verse_wide),
                4,
                &sample_committed_delta(Scope::verse_wide(Identifier32([0x99; 32]))),
            ),
            Err(WireError::ScopeNotAuthorized)
        );
    }

    #[test]
    fn a_delta_for_another_branch_or_projection_cannot_be_built() {
        let scope = Scope::verse_wide(Identifier32([0x11; 32]));

        let mut other_branch = sample_committed_delta(scope);
        other_branch.branch_id = Identifier32([0x77; 32]);
        assert_eq!(
            CommitDeltaBody::for_subscription(
                [0xcc; 16],
                &sample_subscription(scope),
                4,
                &other_branch
            ),
            Err(WireError::DeltaNotForSubscription)
        );

        let mut other_projection = sample_committed_delta(scope);
        other_projection.projection_identity.version += 1;
        assert_eq!(
            CommitDeltaBody::for_subscription(
                [0xcc; 16],
                &sample_subscription(scope),
                4,
                &other_projection
            ),
            Err(WireError::DeltaNotForSubscription)
        );
    }

    struct ScriptedPipeline {
        derived: Hash32,
        result: PipelineResult,
    }

    #[async_trait]
    impl CanonicalCommitPipeline for ScriptedPipeline {
        fn derive_op_id(&self, _complete_envelope_bytes: &[u8]) -> Hash32 {
            self.derived
        }

        async fn submit_candidate(
            &self,
            _complete_envelope_bytes: &[u8],
            _payload_ciphertext: Option<&[u8]>,
        ) -> PipelineResult {
            self.result.clone()
        }
    }

    #[tokio::test]
    async fn a_claimed_op_id_mismatch_is_rejected_before_the_pipeline_is_consulted() {
        let pipeline = ScriptedPipeline {
            derived: hash(0x01),
            result: PipelineResult::Committed(sample_binding()),
        };
        let mut sessions = SessionAuthorizationTable::new();
        let generation = sessions
            .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
            .expect("accept");

        let body = CommitSubmitBody {
            session_generation: generation,
            authorization_binding_id: [1; 16],
            claimed_op_id: hash(0x02),
            complete_envelope: vec![1, 2, 3],
            payload_ciphertext: None,
        };
        let ack = handle_commit_submit(&pipeline, &sessions, &body).await;
        assert_eq!(
            ack.state,
            CommitAckState::Rejected {
                category: ProtocolErrorCategory::InvalidOpId
            }
        );
    }

    #[tokio::test]
    async fn a_stale_session_generation_is_rejected_before_the_pipeline_is_consulted() {
        let pipeline = ScriptedPipeline {
            derived: hash(0x02),
            result: PipelineResult::Committed(sample_binding()),
        };
        let sessions = SessionAuthorizationTable::new();
        let body = CommitSubmitBody {
            session_generation: 999,
            authorization_binding_id: [1; 16],
            claimed_op_id: hash(0x02),
            complete_envelope: vec![1, 2, 3],
            payload_ciphertext: None,
        };
        let ack = handle_commit_submit(&pipeline, &sessions, &body).await;
        assert_eq!(
            ack.state,
            CommitAckState::Rejected {
                category: ProtocolErrorCategory::SessionGenerationInvalid
            }
        );
    }

    #[tokio::test]
    async fn a_differing_byte_string_for_an_existing_op_id_is_rejected_as_conflicting() {
        let pipeline = ScriptedPipeline {
            derived: hash(0x02),
            result: PipelineResult::IntegrityConflict,
        };
        let mut sessions = SessionAuthorizationTable::new();
        let generation = sessions
            .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
            .expect("accept");
        let body = CommitSubmitBody {
            session_generation: generation,
            authorization_binding_id: [1; 16],
            claimed_op_id: hash(0x02),
            complete_envelope: vec![9, 9, 9],
            payload_ciphertext: None,
        };
        let ack = handle_commit_submit(&pipeline, &sessions, &body).await;
        assert_eq!(
            ack.state,
            CommitAckState::Rejected {
                category: ProtocolErrorCategory::ConflictingOpBytes
            }
        );
    }
}
