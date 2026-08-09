//! Wire-level errors and the §6.3 protocol-error category table.

use thiserror::Error;

use crate::cbor::CborError;
use crate::envelope::EnvelopeError;
use crate::frontier::FrontierError;

/// Every reason a frame or message body fails to decode, or a handler-level rule fails.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum WireError {
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// A nested SPEC-1 scope, author, or hash value failed its own grammar.
    #[error("envelope primitive: {0}")]
    Envelope(#[from] EnvelopeError),
    /// A carried [`crate::frontier::SortedFrontier`] was malformed.
    #[error("frontier: {0}")]
    Frontier(#[from] FrontierError),
    /// A value that must be a CBOR map was not one.
    #[error("{context} is not a CBOR map")]
    ExpectedMap {
        /// What was being parsed.
        context: &'static str,
    },
    /// A body carried a key its grammar does not list for the decoded state.
    #[error("{context} carries an unexpected key set")]
    UnexpectedKeys {
        /// What was being parsed.
        context: &'static str,
    },
    /// A listed key was absent.
    #[error("{context} key {key} is missing")]
    MissingKey {
        /// What was being parsed.
        context: &'static str,
        /// The absent key.
        key: u64,
    },
    /// A key held a value of the wrong CBOR shape.
    #[error("{context} key {key} has the wrong shape")]
    WrongShape {
        /// What was being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A fixed-length byte string had the wrong length.
    #[error("{context} key {key} holds {actual} byte(s), expected {expected}")]
    WrongByteLength {
        /// What was being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
        /// Length the grammar requires.
        expected: usize,
        /// Length actually present.
        actual: usize,
    },
    /// An integer discriminant did not name a known variant.
    #[error("{context} key {key} holds unknown discriminant {value}")]
    UnknownDiscriminant {
        /// What was being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
        /// The unrecognized value.
        value: u64,
    },
    /// `wire_version` was not [`crate::wire::frame::WIRE_VERSION`].
    #[error("unsupported wire_version {version}")]
    UnsupportedWireVersion {
        /// The version actually present.
        version: u64,
    },
    /// `message_type` did not name a registered type.
    #[error("unknown message_type {value}")]
    UnknownMessageType {
        /// The unrecognized value.
        value: u64,
    },
    /// The decoded frame's type did not match the type the caller expected.
    #[error("expected message_type {expected:?}, found {actual:?}")]
    UnexpectedMessageType {
        /// The type the caller's decoder was built for.
        expected: crate::wire::frame::MessageType,
        /// The type actually decoded.
        actual: crate::wire::frame::MessageType,
    },
    /// A frame's message type does not belong to the direction the caller expected.
    #[error("message_type {message_type:?} is not valid in the expected direction")]
    WrongMessageDirection {
        /// The offending type.
        message_type: crate::wire::frame::MessageType,
    },
    /// A preview body carried a key reserved for commit/cursor/subscription/snapshot semantics.
    #[error("preview body carries the forbidden field {field}")]
    ForbiddenPreviewField {
        /// Name of the forbidden field, for diagnostics only.
        field: &'static str,
    },
    /// A cursor's carried frontier commitment did not match a recomputation from its op_id set.
    #[error("cursor frontier_commitment does not match its own sorted-frontier hash")]
    InvalidFrontierCommitment,
    /// A `subscribe` reused a subscription ID with a different branch, scope, or projection.
    #[error("subscription_id already bound to a different branch, scope, or projection identity")]
    DuplicateSubscriptionBinding,
    /// An `authorize` reused a binding ID with a different chain or principal.
    #[error("authorization_binding_id already bound to a different chain or principal")]
    AuthorizationBindingCollision,
    /// A stateful frame named a session authorization generation that is no longer current;
    /// reported as [`ProtocolErrorCategory::SessionGenerationInvalid`].
    #[error("session_generation {generation} is not the current accepted generation")]
    StaleSessionGeneration {
        /// The generation the frame named.
        generation: u64,
    },
    /// A delta's exact scope lies outside the recipient's active subscription scope; reported
    /// as [`ProtocolErrorCategory::ScopeNotAuthorized`].
    #[error("delta scope is not authorized for the recipient subscription scope")]
    ScopeNotAuthorized,
    /// A delta named a branch or projection identity the recipient's subscription is not bound
    /// to; reported as [`ProtocolErrorCategory::ScopeNotAuthorized`].
    #[error("delta branch or projection identity does not match the recipient subscription")]
    DeltaNotForSubscription,
    /// A cursor did not validate against the branch registry (unknown tuple, bad commitment).
    #[error("cursor did not validate against the branch registry")]
    CursorInvalid,
    /// A preview was dropped by the per-principal, per-scope rate limiter.
    #[error("preview rate limit exceeded for this principal and scope")]
    PreviewRateLimited,
    /// `resume` or `snapshot_ack` named a subscription ID with no bound subscription.
    #[error("subscription_id names no bound subscription")]
    SubscriptionNotFound,
    /// A cursor's tuple did not match the subscription it was presented against.
    #[error("cursor tuple does not match the subscription it was presented against")]
    CursorTupleMismatch,
    /// `preview_kind` was zero; §7.1 requires a registered non-zero kind.
    #[error("preview_kind must be non-zero")]
    ZeroPreviewKind,
    /// A capability chain could not be verified: unknown, expired, or insufficient for the
    /// requested verb/object-class/scope.
    #[error("capability chain did not verify for the requested authority")]
    CapabilityNotVerified,
}

/// The §6.3 stable, public `protocol_error` categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProtocolErrorCategory {
    /// Reject the frame; no handler side effect.
    MalformedFrame,
    /// Reject the frame; no downgrade or reinterpretation.
    UnsupportedWireVersion,
    /// Reject the frame; no handler side effect.
    WrongMessageDirection,
    /// Stop the frame before commit, replay, snapshot, or preview work.
    SessionGenerationInvalid,
    /// Stop affected traffic; require replacement authority or close.
    AuthorizationRevalidationRequired,
    /// Deny without private-object existence details.
    ScopeNotAuthorized,
    /// Reject; do not append or materialize.
    InvalidOpId,
    /// Reject both claimed substitution and any projection effect.
    ConflictingOpBytes,
    /// SPEC-4 `InvalidEnvelope` admission disposition.
    InvalidEnvelope,
    /// SPEC-4 `UnauthorizedOperation` admission disposition.
    UnauthorizedOperation,
    /// SPEC-4 `InvalidPayload` admission disposition.
    InvalidPayload,
    /// Reject a non-empty candidate unless it uses the current authorized production suite.
    UnsupportedPayloadSuite,
    /// SPEC-4 `MissingParent` quarantine disposition.
    MissingParent,
    /// SPEC-4 `UnknownSchema` quarantine disposition.
    UnknownSchema,
    /// SPEC-4 `OpaquePayload` quarantine disposition.
    OpaquePayload,
    /// Report only `accepted_pending_materialization`; no cursor or delta.
    MaterializationPending,
    /// Require a valid compatible cursor or snapshot; do not approximate.
    CursorInvalid,
    /// Stop replay at a known boundary and send the recovery path.
    SnapshotRequired,
    /// Reject cursor, resume, checkpoint, or bootstrap use on a frontier/replay mismatch.
    FrontierCommitmentMismatch,
    /// Drop the preview only; never affect committed history.
    PreviewRateLimited,
}

impl ProtocolErrorCategory {
    /// Provisional wire discriminant; see `src/wire/AGENTS.md`.
    pub const fn to_u64(self) -> u64 {
        match self {
            Self::MalformedFrame => 0,
            Self::UnsupportedWireVersion => 1,
            Self::WrongMessageDirection => 2,
            Self::SessionGenerationInvalid => 3,
            Self::AuthorizationRevalidationRequired => 4,
            Self::ScopeNotAuthorized => 5,
            Self::InvalidOpId => 6,
            Self::ConflictingOpBytes => 7,
            Self::InvalidEnvelope => 8,
            Self::UnauthorizedOperation => 9,
            Self::InvalidPayload => 10,
            Self::UnsupportedPayloadSuite => 11,
            Self::MissingParent => 12,
            Self::UnknownSchema => 13,
            Self::OpaquePayload => 14,
            Self::MaterializationPending => 15,
            Self::CursorInvalid => 16,
            Self::SnapshotRequired => 17,
            Self::FrontierCommitmentMismatch => 18,
            Self::PreviewRateLimited => 19,
        }
    }

    /// Decodes the provisional wire discriminant.
    pub const fn from_u64(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::MalformedFrame,
            1 => Self::UnsupportedWireVersion,
            2 => Self::WrongMessageDirection,
            3 => Self::SessionGenerationInvalid,
            4 => Self::AuthorizationRevalidationRequired,
            5 => Self::ScopeNotAuthorized,
            6 => Self::InvalidOpId,
            7 => Self::ConflictingOpBytes,
            8 => Self::InvalidEnvelope,
            9 => Self::UnauthorizedOperation,
            10 => Self::InvalidPayload,
            11 => Self::UnsupportedPayloadSuite,
            12 => Self::MissingParent,
            13 => Self::UnknownSchema,
            14 => Self::OpaquePayload,
            15 => Self::MaterializationPending,
            16 => Self::CursorInvalid,
            17 => Self::SnapshotRequired,
            18 => Self::FrontierCommitmentMismatch,
            19 => Self::PreviewRateLimited,
            _ => return None,
        })
    }
}

/// §6.3 `protocol_error` body: exactly one stable, public category. No other field — a
/// diagnostic beyond the category could disclose whether a private operation, artifact,
/// branch, or cursor exists, which §6.3 rule 3 forbids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolErrorBody {
    /// The stable public category.
    pub category: ProtocolErrorCategory,
}

impl ProtocolErrorBody {
    /// Encodes the single-key body map.
    pub fn to_cbor(&self) -> crate::cbor::CborValue {
        crate::cbor::CborValue::Map(vec![(
            crate::cbor::CborValue::Uint(0),
            crate::cbor::CborValue::Uint(self.category.to_u64()),
        )])
    }

    /// Decodes the single-key body map.
    pub fn from_cbor(value: &crate::cbor::CborValue) -> Result<Self, WireError> {
        crate::wire::require_exact_keys(value, &[0], "protocol_error")?;
        let raw = crate::wire::uint_at(value, 0, "protocol_error")?;
        Ok(Self {
            category: ProtocolErrorCategory::from_u64(raw).ok_or(
                WireError::UnknownDiscriminant {
                    context: "protocol_error",
                    key: 0,
                    value: raw,
                },
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_error_body_carries_exactly_one_key() {
        for category in [
            ProtocolErrorCategory::ScopeNotAuthorized,
            ProtocolErrorCategory::InvalidOpId,
        ] {
            let body = ProtocolErrorBody { category };
            let value = body.to_cbor();
            assert_eq!(value.as_map().expect("map").len(), 1);
            let bytes = crate::cbor::encode_canonical_checked(&value).expect("encode");
            assert_eq!(
                ProtocolErrorBody::from_cbor(
                    &crate::cbor::decode_canonical(&bytes).expect("decode")
                )
                .expect("decode"),
                body
            );
        }
    }

    #[test]
    fn every_category_round_trips_its_discriminant() {
        let all = [
            ProtocolErrorCategory::MalformedFrame,
            ProtocolErrorCategory::UnsupportedWireVersion,
            ProtocolErrorCategory::WrongMessageDirection,
            ProtocolErrorCategory::SessionGenerationInvalid,
            ProtocolErrorCategory::AuthorizationRevalidationRequired,
            ProtocolErrorCategory::ScopeNotAuthorized,
            ProtocolErrorCategory::InvalidOpId,
            ProtocolErrorCategory::ConflictingOpBytes,
            ProtocolErrorCategory::InvalidEnvelope,
            ProtocolErrorCategory::UnauthorizedOperation,
            ProtocolErrorCategory::InvalidPayload,
            ProtocolErrorCategory::UnsupportedPayloadSuite,
            ProtocolErrorCategory::MissingParent,
            ProtocolErrorCategory::UnknownSchema,
            ProtocolErrorCategory::OpaquePayload,
            ProtocolErrorCategory::MaterializationPending,
            ProtocolErrorCategory::CursorInvalid,
            ProtocolErrorCategory::SnapshotRequired,
            ProtocolErrorCategory::FrontierCommitmentMismatch,
            ProtocolErrorCategory::PreviewRateLimited,
        ];
        for category in all {
            assert_eq!(
                ProtocolErrorCategory::from_u64(category.to_u64()),
                Some(category)
            );
        }
        assert_eq!(ProtocolErrorCategory::from_u64(999), None);
    }
}
