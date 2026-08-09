//! The durable cursor abstraction (§3) and the branch registry seam it is issued through.

use async_trait::async_trait;

use crate::cbor::CborValue;
use crate::envelope::{Hash32, Identifier32, Scope};
use crate::frontier::SortedFrontier;

use super::error::WireError;
use super::{
    bytes_at, get, hash32_at, identifier32_at, require_exact_keys, scope_at, scope_entry, uint_at,
};

const PROJECTION_IDENTITY_CONTEXT: &str = "projection_identity";
const CURSOR_TUPLE_CONTEXT: &str = "cursor_tuple";
const DURABLE_CURSOR_CONTEXT: &str = "durable_cursor";

/// SPEC-4 materializer identity as this wire slice needs to bind it to a cursor tuple.
///
/// This is a wire-local type, not `fe_canonical_log::materialize::identity::MaterializerVersion`
/// — the two are reconciled in the serial integration wave; see `src/wire/AGENTS.md`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectionIdentity {
    /// Stable materializer identity string.
    pub materializer_id: String,
    /// Materializer version; a bump yields a distinct projection identity.
    pub version: u32,
}

impl ProjectionIdentity {
    pub(crate) fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::text(&self.materializer_id)),
            (CborValue::Uint(1), CborValue::Uint(u64::from(self.version))),
        ])
    }

    pub(crate) fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1], PROJECTION_IDENTITY_CONTEXT)?;
        let materializer_id = get(value, 0, PROJECTION_IDENTITY_CONTEXT)?
            .as_text()
            .ok_or(WireError::WrongShape {
                context: PROJECTION_IDENTITY_CONTEXT,
                key: 0,
            })?
            .to_owned();
        let version = uint_at(value, 1, PROJECTION_IDENTITY_CONTEXT)?;
        let version = u32::try_from(version).map_err(|_| WireError::WrongShape {
            context: PROJECTION_IDENTITY_CONTEXT,
            key: 1,
        })?;
        Ok(Self {
            materializer_id,
            version,
        })
    }
}

/// §3 rule 1: the tuple a durable cursor names — `(verse_id, branch_id, subscription_scope,
/// projection_identity)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CursorTuple {
    /// Verse the cursor observes.
    pub verse_id: Identifier32,
    /// Branch the cursor observes.
    pub branch_id: Identifier32,
    /// Exact scope this cursor's delivery is filtered to.
    pub subscription_scope: Scope,
    /// Projection identity this cursor's position is valid for.
    pub projection_identity: ProjectionIdentity,
}

impl CursorTuple {
    pub(crate) fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.verse_id.as_bytes().to_vec()),
            ),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.branch_id.as_bytes().to_vec()),
            ),
            (CborValue::Uint(2), scope_entry(&self.subscription_scope)?),
            (CborValue::Uint(3), self.projection_identity.to_cbor()),
        ]))
    }

    pub(crate) fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3], CURSOR_TUPLE_CONTEXT)?;
        Ok(Self {
            verse_id: identifier32_at(value, 0, CURSOR_TUPLE_CONTEXT)?,
            branch_id: identifier32_at(value, 1, CURSOR_TUPLE_CONTEXT)?,
            subscription_scope: scope_at(value, 2, CURSOR_TUPLE_CONTEXT)?,
            projection_identity: ProjectionIdentity::from_cbor(get(
                value,
                3,
                CURSOR_TUPLE_CONTEXT,
            )?)?,
        })
    }
}

/// An opaque, registry-issued durable cursor (§3).
///
/// The only sanctioned way to obtain one is [`BranchRegistry::issue_cursor`] (a provided,
/// non-overridable-in-effect method: its body is the only caller of the private constructor).
/// No public constructor exists so a peer cannot hand-roll a cursor from received bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableCursor {
    tuple: CursorTuple,
    frontier_commitment: Hash32,
    delivery_position: Vec<u8>,
}

impl DurableCursor {
    pub(crate) fn new(
        tuple: CursorTuple,
        frontier_commitment: Hash32,
        delivery_position: Vec<u8>,
    ) -> Self {
        Self {
            tuple,
            frontier_commitment,
            delivery_position,
        }
    }

    /// The `(verse_id, branch_id, subscription_scope, projection_identity)` tuple this names.
    pub fn tuple(&self) -> &CursorTuple {
        &self.tuple
    }

    /// The D-CL19 `BLAKE3(sorted frontier op_id list)` value.
    pub fn frontier_commitment(&self) -> Hash32 {
        self.frontier_commitment
    }

    /// The opaque, registry-defined delivery position; comparable only within the same tuple.
    pub fn delivery_position(&self) -> &[u8] {
        &self.delivery_position
    }

    pub(crate) fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), self.tuple.to_cbor()?),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.frontier_commitment.as_bytes().to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.delivery_position.clone()),
            ),
        ]))
    }

    pub(crate) fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2], DURABLE_CURSOR_CONTEXT)?;
        Ok(Self {
            tuple: CursorTuple::from_cbor(get(value, 0, DURABLE_CURSOR_CONTEXT)?)?,
            frontier_commitment: hash32_at(value, 1, DURABLE_CURSOR_CONTEXT)?,
            delivery_position: bytes_at(value, 2, DURABLE_CURSOR_CONTEXT)?.to_vec(),
        })
    }
}

/// The result of [`BranchRegistry::compare`] between two same-tuple cursors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorOrder {
    /// The two cursors name the same delivery position.
    Equal,
    /// `left` precedes `right`.
    LeftBeforeRight,
    /// `left` follows `right`.
    LeftAfterRight,
    /// The registry cannot order the two (different tuple, or concurrent, unresolvable state).
    Incomparable,
}

/// The §5.2 rule 1 explicit snapshot-required reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnapshotReason {
    /// Broadcast lag detected for this connection.
    BroadcastLagged,
    /// The named cursor is older than retained replay.
    CursorUnavailable,
    /// The named cursor could not be verified by the branch registry.
    CursorInvalid,
    /// The projection identity changed since the cursor was issued.
    ProjectionChanged,
    /// The requested replay interval exceeds the service's replay limit.
    ReplayLimit,
    /// Authorization changed since the cursor was issued.
    AuthorizationChanged,
}

impl SnapshotReason {
    pub(crate) const fn to_u64(self) -> u64 {
        match self {
            Self::BroadcastLagged => 0,
            Self::CursorUnavailable => 1,
            Self::CursorInvalid => 2,
            Self::ProjectionChanged => 3,
            Self::ReplayLimit => 4,
            Self::AuthorizationChanged => 5,
        }
    }

    pub(crate) const fn from_u64(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::BroadcastLagged,
            1 => Self::CursorUnavailable,
            2 => Self::CursorInvalid,
            3 => Self::ProjectionChanged,
            4 => Self::ReplayLimit,
            5 => Self::AuthorizationChanged,
            _ => return None,
        })
    }
}

/// One committed operation's authorized delivery record, as `replay_after` returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedDelta {
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
    /// Authorized projection-change summary; opaque to the wire layer.
    pub change_summary: Vec<u8>,
}

/// The result of [`BranchRegistry::replay_after`]: a complete interval, or a snapshot path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayOutcome {
    /// Every authorized committed delta after the cursor, in cursor order.
    Deltas(Vec<CommittedDelta>),
    /// Replay cannot safely continue from this cursor; the caller must snapshot.
    SnapshotRequired(SnapshotReason),
}

/// §3 rule 3: the four functions a branch registry MUST provide before a cursor is issued.
#[async_trait]
pub trait BranchRegistry: Send + Sync {
    /// Proves every field of `cursor` binds one verse, branch, scope view, projection identity,
    /// and a valid D-CL19 frontier commitment.
    async fn validate(&self, cursor: &DurableCursor) -> Result<(), WireError>;

    /// Establishes whether two same-tuple cursors are equal, ordered, or incomparable.
    async fn compare(
        &self,
        left: &DurableCursor,
        right: &DurableCursor,
    ) -> Result<CursorOrder, WireError>;

    /// Returns the complete ordered set of committed deltas after `cursor`, or a snapshot path.
    async fn replay_after(&self, cursor: &DurableCursor) -> Result<ReplayOutcome, WireError>;

    /// Returns the exact cursor represented by a fresh scope snapshot.
    async fn snapshot_cursor(&self, scope: &Scope) -> Result<DurableCursor, WireError>;

    /// The only sanctioned constructor for [`DurableCursor`]. Implementors receive this for
    /// free and MUST NOT reimplement cursor construction another way; see `src/wire/AGENTS.md`.
    fn issue_cursor(
        &self,
        tuple: CursorTuple,
        frontier_commitment: Hash32,
        delivery_position: Vec<u8>,
    ) -> DurableCursor {
        DurableCursor::new(tuple, frontier_commitment, delivery_position)
    }
}

/// Recomputes a D-CL19 frontier commitment from its member op_ids and checks it against
/// `claimed_commitment`, reusing [`SortedFrontier::commitment`] exclusively.
pub fn verify_frontier_commitment(
    op_ids: impl IntoIterator<Item = Hash32>,
    claimed_commitment: Hash32,
) -> Result<Hash32, WireError> {
    let frontier = SortedFrontier::try_new(op_ids)?;
    let commitment = frontier.commitment();
    if commitment != claimed_commitment {
        return Err(WireError::InvalidFrontierCommitment);
    }
    Ok(commitment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical_checked};

    fn op_id(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn sample_tuple() -> CursorTuple {
        CursorTuple {
            verse_id: Identifier32([0x11; 32]),
            branch_id: Identifier32([0x22; 32]),
            subscription_scope: Scope::verse_wide(Identifier32([0x11; 32])),
            projection_identity: ProjectionIdentity {
                materializer_id: "fe-scene".to_owned(),
                version: 1,
            },
        }
    }

    #[test]
    fn projection_identity_round_trips_and_a_version_bump_changes_it() {
        let identity = ProjectionIdentity {
            materializer_id: "fe-scene".to_owned(),
            version: 3,
        };
        let bytes = encode_canonical_checked(&identity.to_cbor()).expect("encode");
        assert_eq!(
            ProjectionIdentity::from_cbor(&decode_canonical(&bytes).expect("decode"))
                .expect("decode"),
            identity
        );
        let mut bumped = identity.clone();
        bumped.version += 1;
        assert_ne!(identity, bumped);
    }

    #[test]
    fn durable_cursor_round_trips_through_canonical_cbor() {
        let cursor = DurableCursor::new(sample_tuple(), op_id(0x99), vec![1, 2, 3]);
        let bytes = encode_canonical_checked(&cursor.to_cbor().expect("cbor")).expect("encode");
        let decoded =
            DurableCursor::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode");
        assert_eq!(decoded, cursor);
        assert_eq!(decoded.tuple(), &sample_tuple());
        assert_eq!(decoded.frontier_commitment(), op_id(0x99));
        assert_eq!(decoded.delivery_position(), &[1, 2, 3]);
    }

    #[test]
    fn frontier_commitment_is_order_independent_and_rejects_a_wrong_claim() {
        let ascending = SortedFrontier::try_new([op_id(1), op_id(2), op_id(3)]).expect("frontier");
        let shuffled_commitment =
            verify_frontier_commitment([op_id(3), op_id(1), op_id(2)], ascending.commitment())
                .expect("order-independent");
        assert_eq!(shuffled_commitment, ascending.commitment());

        assert_eq!(
            verify_frontier_commitment([op_id(1), op_id(2)], op_id(0xff)),
            Err(WireError::InvalidFrontierCommitment)
        );
    }

    #[test]
    fn snapshot_reason_round_trips_every_variant() {
        for reason in [
            SnapshotReason::BroadcastLagged,
            SnapshotReason::CursorUnavailable,
            SnapshotReason::CursorInvalid,
            SnapshotReason::ProjectionChanged,
            SnapshotReason::ReplayLimit,
            SnapshotReason::AuthorizationChanged,
        ] {
            assert_eq!(SnapshotReason::from_u64(reason.to_u64()), Some(reason));
        }
        assert_eq!(SnapshotReason::from_u64(99), None);
    }
}
