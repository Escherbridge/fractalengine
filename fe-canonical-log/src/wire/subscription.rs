//! Subscription, resume, and replay (§5.1): `subscribe`, `resume`, `replay_complete`.

use std::collections::HashMap;

use crate::cbor::CborValue;
use crate::envelope::{Identifier32, Scope};

use super::cursor::{
    BranchRegistry, CommittedDelta, CursorTuple, DurableCursor, ProjectionIdentity, ReplayOutcome,
    SnapshotReason,
};
use super::error::WireError;
use super::session::SessionAuthorizationTable;
use super::snapshot::SnapshotRequiredBody;
use super::{
    fixed_bytes_at, get, identifier32_at, require_current_session_generation, require_exact_keys,
    scope_at, scope_entry, uint_at,
};

const SUBSCRIBE_CONTEXT: &str = "subscribe";
const RESUME_CONTEXT: &str = "resume";
const REPLAY_COMPLETE_CONTEXT: &str = "replay_complete";

/// §5.1.7 `subscribe` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscribeBody {
    /// Current session authorization generation.
    pub session_generation: u64,
    /// The binding proving the relevant SPEC-3 verb over the requested scope.
    pub authorization_binding_id: [u8; 16],
    /// Client-chosen 16-byte subscription ID.
    pub subscription_id: [u8; 16],
    /// Branch to subscribe to.
    pub branch_id: Identifier32,
    /// Exact requested scope.
    pub scope: Scope,
    /// Requested projection identity.
    pub projection_identity: ProjectionIdentity,
}

impl SubscribeBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.session_generation)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.authorization_binding_id.to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.subscription_id.to_vec()),
            ),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.branch_id.as_bytes().to_vec()),
            ),
            (CborValue::Uint(4), scope_entry(&self.scope)?),
            (CborValue::Uint(5), self.projection_identity.to_cbor()),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3, 4, 5], SUBSCRIBE_CONTEXT)?;
        Ok(Self {
            session_generation: uint_at(value, 0, SUBSCRIBE_CONTEXT)?,
            authorization_binding_id: fixed_bytes_at::<16>(value, 1, SUBSCRIBE_CONTEXT)?,
            subscription_id: fixed_bytes_at::<16>(value, 2, SUBSCRIBE_CONTEXT)?,
            branch_id: identifier32_at(value, 3, SUBSCRIBE_CONTEXT)?,
            scope: scope_at(value, 4, SUBSCRIBE_CONTEXT)?,
            projection_identity: ProjectionIdentity::from_cbor(get(value, 5, SUBSCRIBE_CONTEXT)?)?,
        })
    }
}

/// §5.1.7 `resume` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeBody {
    /// Current session authorization generation.
    pub session_generation: u64,
    /// The existing subscription being resumed.
    pub subscription_id: [u8; 16],
    /// The prior durable cursor to replay after.
    pub prior_cursor: DurableCursor,
}

impl ResumeBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.session_generation)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.subscription_id.to_vec()),
            ),
            (CborValue::Uint(2), self.prior_cursor.to_cbor()?),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2], RESUME_CONTEXT)?;
        Ok(Self {
            session_generation: uint_at(value, 0, RESUME_CONTEXT)?,
            subscription_id: fixed_bytes_at::<16>(value, 1, RESUME_CONTEXT)?,
            prior_cursor: DurableCursor::from_cbor(get(value, 2, RESUME_CONTEXT)?)?,
        })
    }
}

/// §4.3.5 `replay_complete` body: proves only that the authorized interval up to `cursor`
/// was delivered completely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayCompleteBody {
    /// The subscription this replay satisfies.
    pub subscription_id: [u8; 16],
    /// Session generation this replay was authorized under.
    pub session_generation: u64,
    /// The final durable cursor reached.
    pub cursor: DurableCursor,
}

impl ReplayCompleteBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.subscription_id.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.session_generation)),
            (CborValue::Uint(2), self.cursor.to_cbor()?),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2], REPLAY_COMPLETE_CONTEXT)?;
        Ok(Self {
            subscription_id: fixed_bytes_at::<16>(value, 0, REPLAY_COMPLETE_CONTEXT)?,
            session_generation: uint_at(value, 1, REPLAY_COMPLETE_CONTEXT)?,
            cursor: DurableCursor::from_cbor(get(value, 2, REPLAY_COMPLETE_CONTEXT)?)?,
        })
    }
}

/// One bound subscription's tuple, as `subscribe` established it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionRecord {
    /// Binding proving authority for this subscription.
    pub authorization_binding_id: [u8; 16],
    /// Branch subscribed to.
    pub branch_id: Identifier32,
    /// Exact requested scope.
    pub scope: Scope,
    /// Requested projection identity.
    pub projection_identity: ProjectionIdentity,
}

/// Tracks bound subscription IDs and rejects a silent identity change (§5.1.7).
#[derive(Default)]
pub struct SubscriptionTable {
    subscriptions: HashMap<[u8; 16], SubscriptionRecord>,
}

impl SubscriptionTable {
    /// Builds an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `subscription_id` to `record`. A resubmission with an identical record is
    /// idempotent; one that changes branch, scope, or projection identity is rejected — the
    /// caller must present an explicit replacement subscription ID instead.
    pub fn bind(
        &mut self,
        subscription_id: [u8; 16],
        record: SubscriptionRecord,
    ) -> Result<(), WireError> {
        if let Some(existing) = self.subscriptions.get(&subscription_id) {
            if existing.branch_id != record.branch_id
                || existing.scope != record.scope
                || existing.projection_identity != record.projection_identity
            {
                return Err(WireError::DuplicateSubscriptionBinding);
            }
        }
        self.subscriptions.insert(subscription_id, record);
        Ok(())
    }

    /// Looks up a bound subscription's record.
    pub fn get(&self, subscription_id: [u8; 16]) -> Option<&SubscriptionRecord> {
        self.subscriptions.get(&subscription_id)
    }

    /// Removes a subscription (e.g. on explicit close or snapshot-driven replacement).
    pub fn remove(&mut self, subscription_id: [u8; 16]) -> Option<SubscriptionRecord> {
        self.subscriptions.remove(&subscription_id)
    }

    /// Iterates every bound subscription, for broadcast-lag snapshot fan-out.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8; 16], &SubscriptionRecord)> {
        self.subscriptions.iter()
    }
}

/// What resolving one `resume` request produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The complete authorized interval after the cursor, in cursor order.
    Replayed {
        /// Every authorized delta after the prior cursor, in cursor order.
        deltas: Vec<CommittedDelta>,
        /// Proof of complete delivery through the final cursor.
        replay_complete: Box<ReplayCompleteBody>,
    },
    /// Replay cannot safely continue; the client must request a fresh snapshot.
    SnapshotRequired(SnapshotRequiredBody),
}

/// Resolves one `resume`: revalidates the session generation, then the subscription and cursor
/// tuple, then either replays the complete authorized interval or returns an explicit snapshot
/// path. Never truncates an interval and never approximates an unavailable or incomparable
/// cursor.
///
/// `sessions` is required, not optional: §6 rule 3 stops a stale generation before any replay
/// work, and no caller may skip that by omitting an argument.
pub async fn resolve_resume<R: BranchRegistry + ?Sized>(
    registry: &R,
    subscriptions: &SubscriptionTable,
    sessions: &SessionAuthorizationTable,
    body: &ResumeBody,
) -> Result<ResumeOutcome, WireError> {
    require_current_session_generation(sessions, body.session_generation)?;

    let record = subscriptions
        .get(body.subscription_id)
        .ok_or(WireError::SubscriptionNotFound)?;

    let expected_tuple = CursorTuple {
        verse_id: record.scope.verse_id(),
        branch_id: record.branch_id,
        subscription_scope: record.scope,
        projection_identity: record.projection_identity.clone(),
    };
    if body.prior_cursor.tuple() != &expected_tuple {
        return Ok(ResumeOutcome::SnapshotRequired(SnapshotRequiredBody {
            subscription_id: body.subscription_id,
            session_generation: body.session_generation,
            reason: SnapshotReason::CursorInvalid,
        }));
    }

    if registry.validate(&body.prior_cursor).await.is_err() {
        return Ok(ResumeOutcome::SnapshotRequired(SnapshotRequiredBody {
            subscription_id: body.subscription_id,
            session_generation: body.session_generation,
            reason: SnapshotReason::CursorInvalid,
        }));
    }

    match registry.replay_after(&body.prior_cursor).await? {
        ReplayOutcome::Deltas(deltas) => {
            let final_cursor = deltas
                .last()
                .map(|delta| delta.cursor.clone())
                .unwrap_or_else(|| body.prior_cursor.clone());
            Ok(ResumeOutcome::Replayed {
                deltas,
                replay_complete: Box::new(ReplayCompleteBody {
                    subscription_id: body.subscription_id,
                    session_generation: body.session_generation,
                    cursor: final_cursor,
                }),
            })
        }
        ReplayOutcome::SnapshotRequired(reason) => {
            Ok(ResumeOutcome::SnapshotRequired(SnapshotRequiredBody {
                subscription_id: body.subscription_id,
                session_generation: body.session_generation,
                reason,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical_checked};
    use crate::envelope::Hash32;
    use crate::frontier::SortedFrontier;
    use crate::wire::test_support::InMemoryBranchRegistry;

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn sample_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    fn sample_projection() -> ProjectionIdentity {
        ProjectionIdentity {
            materializer_id: "fe-scene".to_owned(),
            version: 1,
        }
    }

    fn sample_tuple() -> CursorTuple {
        CursorTuple {
            verse_id: Identifier32([0x11; 32]),
            branch_id: Identifier32([0x22; 32]),
            subscription_scope: sample_scope(),
            projection_identity: sample_projection(),
        }
    }

    #[test]
    fn subscribe_and_resume_and_replay_complete_round_trip() {
        let subscribe = SubscribeBody {
            session_generation: 1,
            authorization_binding_id: [0xaa; 16],
            subscription_id: [0xbb; 16],
            branch_id: Identifier32([0x22; 32]),
            scope: sample_scope(),
            projection_identity: sample_projection(),
        };
        let bytes = encode_canonical_checked(&subscribe.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            SubscribeBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            subscribe
        );

        let cursor = DurableCursor::new(
            sample_tuple(),
            SortedFrontier::try_new([hash(0x01)])
                .expect("frontier")
                .commitment(),
            vec![1],
        );
        let resume = ResumeBody {
            session_generation: 2,
            subscription_id: [0xbb; 16],
            prior_cursor: cursor.clone(),
        };
        let bytes = encode_canonical_checked(&resume.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            ResumeBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            resume
        );

        let replay_complete = ReplayCompleteBody {
            subscription_id: [0xbb; 16],
            session_generation: 2,
            cursor,
        };
        let bytes =
            encode_canonical_checked(&replay_complete.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            ReplayCompleteBody::from_cbor(&decode_canonical(&bytes).expect("decode"))
                .expect("decode"),
            replay_complete
        );
    }

    #[test]
    fn subscription_table_rejects_a_silent_identity_change() {
        let mut table = SubscriptionTable::new();
        let record = SubscriptionRecord {
            authorization_binding_id: [0xaa; 16],
            branch_id: Identifier32([0x22; 32]),
            scope: sample_scope(),
            projection_identity: sample_projection(),
        };
        table.bind([1; 16], record.clone()).expect("first bind");
        assert_eq!(
            table.bind([1; 16], record.clone()),
            Ok(()),
            "identical rebind is idempotent"
        );

        let mut changed = record;
        changed.branch_id = Identifier32([0x33; 32]);
        assert_eq!(
            table.bind([1; 16], changed),
            Err(WireError::DuplicateSubscriptionBinding)
        );
    }

    fn accepted_sessions() -> (SessionAuthorizationTable, u64) {
        let mut sessions = SessionAuthorizationTable::new();
        let generation = sessions
            .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
            .expect("accept");
        (sessions, generation)
    }

    #[tokio::test]
    async fn resume_with_an_unknown_subscription_is_rejected() {
        let registry = InMemoryBranchRegistry::new();
        let subscriptions = SubscriptionTable::new();
        let (sessions, generation) = accepted_sessions();
        let body = ResumeBody {
            session_generation: generation,
            subscription_id: [0xff; 16],
            prior_cursor: DurableCursor::new(sample_tuple(), Hash32([0; 32]), Vec::new()),
        };
        assert_eq!(
            resolve_resume(&registry, &subscriptions, &sessions, &body).await,
            Err(WireError::SubscriptionNotFound)
        );
    }

    /// A revoked generation must stop replay ahead of the subscription lookup, so a stale
    /// resume cannot even learn whether a subscription ID exists.
    #[tokio::test]
    async fn a_stale_session_generation_stops_resume_before_the_subscription_is_looked_up() {
        let registry = InMemoryBranchRegistry::new();
        let mut subscriptions = SubscriptionTable::new();
        subscriptions
            .bind(
                [1; 16],
                SubscriptionRecord {
                    authorization_binding_id: [0xaa; 16],
                    branch_id: Identifier32([0x22; 32]),
                    scope: sample_scope(),
                    projection_identity: sample_projection(),
                },
            )
            .expect("bind");

        let (mut sessions, generation) = accepted_sessions();
        sessions.invalidate();

        let body = ResumeBody {
            session_generation: generation,
            subscription_id: [1; 16],
            prior_cursor: DurableCursor::new(sample_tuple(), Hash32([0; 32]), Vec::new()),
        };
        assert_eq!(
            resolve_resume(&registry, &subscriptions, &sessions, &body).await,
            Err(WireError::StaleSessionGeneration { generation })
        );
    }
}
