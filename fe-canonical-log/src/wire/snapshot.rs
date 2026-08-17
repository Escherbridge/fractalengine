//! Snapshot recovery (§5.2): `snapshot_required`, `scene_snapshot`, `snapshot_ack`.

use async_trait::async_trait;

use crate::cbor::CborValue;
use crate::envelope::{Identifier32, Scope};

use super::cursor::{DurableCursor, ProjectionIdentity, SnapshotReason};
use super::error::WireError;
use super::session::SessionAuthorizationTable;
use super::subscription::SubscriptionRecord;
use super::{
    fixed_bytes_at, get, identifier32_at, require_current_session_generation, require_exact_keys,
    scope_at, scope_entry, uint_at,
};

const SNAPSHOT_REQUIRED_CONTEXT: &str = "snapshot_required";
const SCENE_SNAPSHOT_CONTEXT: &str = "scene_snapshot";
const SNAPSHOT_ACK_CONTEXT: &str = "snapshot_ack";

/// §5.2.7 `snapshot_required` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotRequiredBody {
    /// The affected subscription.
    pub subscription_id: [u8; 16],
    /// Session generation this notice was issued under.
    pub session_generation: u64,
    /// One explicit §5.2 rule 1 reason.
    pub reason: SnapshotReason,
}

impl SnapshotRequiredBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.subscription_id.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.session_generation)),
            (CborValue::Uint(2), CborValue::Uint(self.reason.to_u64())),
        ])
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2], SNAPSHOT_REQUIRED_CONTEXT)?;
        let reason_raw = uint_at(value, 2, SNAPSHOT_REQUIRED_CONTEXT)?;
        Ok(Self {
            subscription_id: fixed_bytes_at::<16>(value, 0, SNAPSHOT_REQUIRED_CONTEXT)?,
            session_generation: uint_at(value, 1, SNAPSHOT_REQUIRED_CONTEXT)?,
            reason: SnapshotReason::from_u64(reason_raw).ok_or(WireError::UnknownDiscriminant {
                context: SNAPSHOT_REQUIRED_CONTEXT,
                key: 2,
                value: reason_raw,
            })?,
        })
    }
}

/// §5.2.7 `scene_snapshot` body: a transport recovery response, not a new canonical checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneSnapshotBody {
    /// The subscription this snapshot restores.
    pub subscription_id: [u8; 16],
    /// Session generation this snapshot was authorized under.
    pub session_generation: u64,
    /// Branch the snapshot was taken from.
    pub branch_id: Identifier32,
    /// Exact authorized scope.
    pub scope: Scope,
    /// Projection identity the snapshot was read under.
    pub projection_identity: ProjectionIdentity,
    /// The durable cursor position the snapshot was read at.
    pub snapshot_cursor: DurableCursor,
    /// Materialized-view bytes authorized for this scope, opaque to the wire layer.
    pub view_bytes: Vec<u8>,
}

impl SceneSnapshotBody {
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
                CborValue::Bytes(self.branch_id.as_bytes().to_vec()),
            ),
            (CborValue::Uint(3), scope_entry(&self.scope)?),
            (CborValue::Uint(4), self.projection_identity.to_cbor()),
            (CborValue::Uint(5), self.snapshot_cursor.to_cbor()?),
            (
                CborValue::Uint(6),
                CborValue::Bytes(self.view_bytes.clone()),
            ),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3, 4, 5, 6], SCENE_SNAPSHOT_CONTEXT)?;
        Ok(Self {
            subscription_id: fixed_bytes_at::<16>(value, 0, SCENE_SNAPSHOT_CONTEXT)?,
            session_generation: uint_at(value, 1, SCENE_SNAPSHOT_CONTEXT)?,
            branch_id: identifier32_at(value, 2, SCENE_SNAPSHOT_CONTEXT)?,
            scope: scope_at(value, 3, SCENE_SNAPSHOT_CONTEXT)?,
            projection_identity: ProjectionIdentity::from_cbor(get(
                value,
                4,
                SCENE_SNAPSHOT_CONTEXT,
            )?)?,
            snapshot_cursor: DurableCursor::from_cbor(get(value, 5, SCENE_SNAPSHOT_CONTEXT)?)?,
            view_bytes: super::bytes_at(value, 6, SCENE_SNAPSHOT_CONTEXT)?.to_vec(),
        })
    }
}

/// §5.2.7 `snapshot_ack` body. Authorizes no replay until the service revalidates the tuple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotAckBody {
    /// Session generation this ack was issued under.
    pub session_generation: u64,
    /// The subscription being acknowledged.
    pub subscription_id: [u8; 16],
    /// The exact snapshot cursor the client has now applied and set as its replay base.
    pub snapshot_cursor: DurableCursor,
}

impl SnapshotAckBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.session_generation)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.subscription_id.to_vec()),
            ),
            (CborValue::Uint(2), self.snapshot_cursor.to_cbor()?),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2], SNAPSHOT_ACK_CONTEXT)?;
        Ok(Self {
            session_generation: uint_at(value, 0, SNAPSHOT_ACK_CONTEXT)?,
            subscription_id: fixed_bytes_at::<16>(value, 1, SNAPSHOT_ACK_CONTEXT)?,
            snapshot_cursor: DurableCursor::from_cbor(get(value, 2, SNAPSHOT_ACK_CONTEXT)?)?,
        })
    }
}

/// Produces one fresh scope snapshot; the concrete materialized-view reader lives outside
/// this crate.
#[async_trait]
pub trait ScopeSnapshotSource: Send + Sync {
    /// Reads a fresh materialized-view snapshot for exactly this scope, returning the durable
    /// cursor position it was read at and the authorized view bytes.
    async fn snapshot(
        &self,
        branch_id: Identifier32,
        scope: &Scope,
        projection_identity: &ProjectionIdentity,
    ) -> Result<(DurableCursor, Vec<u8>), WireError>;
}

/// One subscription's D-CL15 lag-recovery outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotDispatchOutcome {
    /// A fresh snapshot for a still-authorized scope.
    Snapshot(Box<SceneSnapshotBody>),
    /// The snapshot source failed for this scope; the caller decides how to surface it.
    Failed(WireError),
}

/// D-CL15 broadcast-lag recovery: snapshots EVERY still-authorized subscribed scope, never an
/// unsubscribed one, and never merely logs and continues from an unknown point. Generalizes
/// the per-petal lag recovery pattern at `fe-api/src/ws.rs:383-397` and `:616-642` (read, not
/// edited, by this slice).
///
/// `sessions` is required, not optional: §6 rule 3 stops a stale generation before any
/// snapshot work, so a revoked session can never be served scene bytes for its old scopes.
pub async fn snapshot_all_authorized_subscriptions<S, IsAuthorized>(
    source: &S,
    subscriptions: &super::subscription::SubscriptionTable,
    sessions: &SessionAuthorizationTable,
    session_generation: u64,
    is_still_authorized: IsAuthorized,
) -> Result<Vec<([u8; 16], SnapshotDispatchOutcome)>, WireError>
where
    S: ScopeSnapshotSource + ?Sized,
    IsAuthorized: Fn(&SubscriptionRecord) -> bool,
{
    require_current_session_generation(sessions, session_generation)?;

    let mut results = Vec::new();
    for (subscription_id, record) in subscriptions.iter() {
        if !is_still_authorized(record) {
            continue;
        }
        let outcome = source
            .snapshot(record.branch_id, &record.scope, &record.projection_identity)
            .await;
        let dispatch = match outcome {
            Ok((cursor, view_bytes)) => {
                SnapshotDispatchOutcome::Snapshot(Box::new(SceneSnapshotBody {
                    subscription_id: *subscription_id,
                    session_generation,
                    branch_id: record.branch_id,
                    scope: record.scope,
                    projection_identity: record.projection_identity.clone(),
                    snapshot_cursor: cursor,
                    view_bytes,
                }))
            }
            Err(error) => SnapshotDispatchOutcome::Failed(error),
        };
        results.push((*subscription_id, dispatch));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical_checked};
    use crate::envelope::Hash32;
    use crate::frontier::SortedFrontier;
    use crate::wire::cursor::CursorTuple;
    use crate::wire::subscription::SubscriptionTable;

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

    fn sample_cursor() -> DurableCursor {
        DurableCursor::new(
            CursorTuple {
                verse_id: Identifier32([0x11; 32]),
                branch_id: Identifier32([0x22; 32]),
                subscription_scope: sample_scope(),
                projection_identity: sample_projection(),
            },
            SortedFrontier::try_new([hash(0x01)])
                .expect("frontier")
                .commitment(),
            vec![7],
        )
    }

    #[test]
    fn every_snapshot_body_round_trips() {
        let required = SnapshotRequiredBody {
            subscription_id: [0xaa; 16],
            session_generation: 3,
            reason: SnapshotReason::BroadcastLagged,
        };
        let bytes = encode_canonical_checked(&required.to_cbor()).expect("encode");
        assert_eq!(
            SnapshotRequiredBody::from_cbor(&decode_canonical(&bytes).expect("decode"))
                .expect("decode"),
            required
        );

        let scene = SceneSnapshotBody {
            subscription_id: [0xaa; 16],
            session_generation: 3,
            branch_id: Identifier32([0x22; 32]),
            scope: sample_scope(),
            projection_identity: sample_projection(),
            snapshot_cursor: sample_cursor(),
            view_bytes: vec![1, 2, 3, 4],
        };
        let bytes = encode_canonical_checked(&scene.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            SceneSnapshotBody::from_cbor(&decode_canonical(&bytes).expect("decode"))
                .expect("decode"),
            scene
        );

        let ack = SnapshotAckBody {
            session_generation: 3,
            subscription_id: [0xaa; 16],
            snapshot_cursor: sample_cursor(),
        };
        let bytes = encode_canonical_checked(&ack.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            SnapshotAckBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            ack
        );
    }

    struct StubSource;

    #[async_trait]
    impl ScopeSnapshotSource for StubSource {
        async fn snapshot(
            &self,
            _branch_id: Identifier32,
            _scope: &Scope,
            _projection_identity: &ProjectionIdentity,
        ) -> Result<(DurableCursor, Vec<u8>), WireError> {
            Ok((sample_cursor(), vec![0xaa]))
        }
    }

    fn accepted_sessions() -> (SessionAuthorizationTable, u64) {
        let mut sessions = SessionAuthorizationTable::new();
        let generation = sessions
            .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
            .expect("accept");
        (sessions, generation)
    }

    #[tokio::test]
    async fn lag_recovery_snapshots_only_still_authorized_subscriptions() {
        let mut subscriptions = SubscriptionTable::new();
        subscriptions
            .bind(
                [1; 16],
                SubscriptionRecord {
                    authorization_binding_id: [0; 16],
                    branch_id: Identifier32([0x22; 32]),
                    scope: sample_scope(),
                    projection_identity: sample_projection(),
                },
            )
            .expect("bind authorized");
        subscriptions
            .bind(
                [2; 16],
                SubscriptionRecord {
                    authorization_binding_id: [0; 16],
                    branch_id: Identifier32([0x22; 32]),
                    scope: sample_scope(),
                    projection_identity: sample_projection(),
                },
            )
            .expect("bind unauthorized");

        let source = StubSource;
        let (sessions, generation) = accepted_sessions();
        let results = snapshot_all_authorized_subscriptions(
            &source,
            &subscriptions,
            &sessions,
            generation,
            |_record| true,
        )
        .await
        .expect("current generation");
        assert_eq!(
            results.len(),
            2,
            "every still-authorized scope is snapshotted"
        );

        let results = snapshot_all_authorized_subscriptions(
            &source,
            &subscriptions,
            &sessions,
            generation,
            |_record| false,
        )
        .await
        .expect("current generation");
        assert!(
            results.is_empty(),
            "no unauthorized scope is ever snapshotted"
        );
    }

    /// A revoked generation must stop the fan-out before the snapshot source is consulted, so a
    /// closed-over-but-still-open socket cannot be served scene bytes for its old scopes.
    #[tokio::test]
    async fn a_stale_session_generation_stops_the_snapshot_fan_out_before_the_source_is_read() {
        let mut subscriptions = SubscriptionTable::new();
        subscriptions
            .bind(
                [1; 16],
                SubscriptionRecord {
                    authorization_binding_id: [0; 16],
                    branch_id: Identifier32([0x22; 32]),
                    scope: sample_scope(),
                    projection_identity: sample_projection(),
                },
            )
            .expect("bind");

        struct PanicsIfRead;

        #[async_trait]
        impl ScopeSnapshotSource for PanicsIfRead {
            async fn snapshot(
                &self,
                _branch_id: Identifier32,
                _scope: &Scope,
                _projection_identity: &ProjectionIdentity,
            ) -> Result<(DurableCursor, Vec<u8>), WireError> {
                panic!("the snapshot source must not be read under a revoked generation");
            }
        }

        let (mut sessions, generation) = accepted_sessions();
        sessions.invalidate();

        assert_eq!(
            snapshot_all_authorized_subscriptions(
                &PanicsIfRead,
                &subscriptions,
                &sessions,
                generation,
                |_record| true,
            )
            .await,
            Err(WireError::StaleSessionGeneration { generation })
        );
    }
}
