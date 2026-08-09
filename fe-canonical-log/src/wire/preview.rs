//! Preview protocol (§7.1): `preview_send`, `preview_delta`, `preview_dropped`.
//!
//! This module MUST import nothing from `commit`, `subscription`, `snapshot`, or `cursor` —
//! that absence of an import path is the mechanism that makes preview frames structurally
//! disjoint from commit frames, not a runtime convention. See `src/wire/AGENTS.md`.

use crate::cbor::CborValue;
use crate::envelope::{Author, Scope};

use super::error::WireError;
use super::{
    author_at, bytes_at, fixed_bytes_at, require_exact_keys, require_no_keys, scope_at,
    scope_entry, u32_at, uint_at,
};

const PREVIEW_SEND_CONTEXT: &str = "preview_send";
const PREVIEW_DELTA_CONTEXT: &str = "preview_delta";
const PREVIEW_DROPPED_CONTEXT: &str = "preview_dropped";

/// Keys permanently reserved across every preview body. No preview field may ever be assigned
/// one of these numbers; this is a second, independent barrier alongside the exact-key-set
/// check every body's `from_cbor` already performs.
const RESERVED_PREVIEW_KEYS: &[(u64, &str)] = &[
    (90, "op_id"),
    (91, "signed_envelope_bytes"),
    (92, "parent_ids"),
    (93, "branch_id"),
    (94, "hlc"),
    (95, "payload_ciphertext"),
    (96, "payload_hash"),
    (97, "checkpoint_identity"),
    (98, "durable_cursor"),
];

/// Defense in depth: rejects a preview body carrying any permanently reserved key, even one
/// smuggled in alongside a key set that would otherwise pass the exact-key check.
pub fn reject_reserved_preview_keys(value: &CborValue) -> Result<(), WireError> {
    require_no_keys(value, RESERVED_PREVIEW_KEYS)
}

/// §7.1.1 `preview_send` body. Contains none of the fields prohibited by §7.2.5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewSendBody {
    /// Current session authorization generation.
    pub session_generation: u64,
    /// The binding proving the effective `preview` verb for `scope`.
    pub authorization_binding_id: [u8; 16],
    /// Exact scope this preview applies to.
    pub scope: Scope,
    /// Sender-local monotonic sequence.
    pub preview_sequence: u64,
    /// Registered non-zero preview kind.
    pub preview_kind: u32,
    /// Expiry time, Unix milliseconds.
    pub expires_at_ms: u64,
    /// Opaque, untrusted display data.
    pub preview_data: Vec<u8>,
}

impl PreviewSendBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.session_generation)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.authorization_binding_id.to_vec()),
            ),
            (CborValue::Uint(2), scope_entry(&self.scope)?),
            (CborValue::Uint(3), CborValue::Uint(self.preview_sequence)),
            (
                CborValue::Uint(4),
                CborValue::Uint(u64::from(self.preview_kind)),
            ),
            (CborValue::Uint(5), CborValue::Uint(self.expires_at_ms)),
            (
                CborValue::Uint(6),
                CborValue::Bytes(self.preview_data.clone()),
            ),
        ]))
    }

    /// Decodes the body map, rejecting the permanently reserved keys as a second barrier.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        reject_reserved_preview_keys(value)?;
        require_exact_keys(value, &[0, 1, 2, 3, 4, 5, 6], PREVIEW_SEND_CONTEXT)?;
        let preview_kind = u32_at(value, 4, PREVIEW_SEND_CONTEXT)?;
        if preview_kind == 0 {
            return Err(WireError::ZeroPreviewKind);
        }
        Ok(Self {
            session_generation: uint_at(value, 0, PREVIEW_SEND_CONTEXT)?,
            authorization_binding_id: fixed_bytes_at::<16>(value, 1, PREVIEW_SEND_CONTEXT)?,
            scope: scope_at(value, 2, PREVIEW_SEND_CONTEXT)?,
            preview_sequence: uint_at(value, 3, PREVIEW_SEND_CONTEXT)?,
            preview_kind,
            expires_at_ms: uint_at(value, 5, PREVIEW_SEND_CONTEXT)?,
            preview_data: bytes_at(value, 6, PREVIEW_SEND_CONTEXT)?.to_vec(),
        })
    }
}

/// §7.1.2 `preview_delta` body. Not a commit acknowledgement; carries no durable cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewDeltaBody {
    /// Sender identity.
    pub sender_principal: Author,
    /// Exact authorized scope.
    pub scope: Scope,
    /// Sender-local sequence, repeated verbatim.
    pub preview_sequence: u64,
    /// Registered non-zero preview kind.
    pub preview_kind: u32,
    /// Expiry time, Unix milliseconds.
    pub expires_at_ms: u64,
    /// Opaque, untrusted display data needed for rendering.
    pub preview_data: Vec<u8>,
}

impl PreviewDeltaBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), self.sender_principal.to_cbor()),
            (CborValue::Uint(1), scope_entry(&self.scope)?),
            (CborValue::Uint(2), CborValue::Uint(self.preview_sequence)),
            (
                CborValue::Uint(3),
                CborValue::Uint(u64::from(self.preview_kind)),
            ),
            (CborValue::Uint(4), CborValue::Uint(self.expires_at_ms)),
            (
                CborValue::Uint(5),
                CborValue::Bytes(self.preview_data.clone()),
            ),
        ]))
    }

    /// Decodes the body map, rejecting the permanently reserved keys as a second barrier.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        reject_reserved_preview_keys(value)?;
        require_exact_keys(value, &[0, 1, 2, 3, 4, 5], PREVIEW_DELTA_CONTEXT)?;
        let preview_kind = u32_at(value, 3, PREVIEW_DELTA_CONTEXT)?;
        if preview_kind == 0 {
            return Err(WireError::ZeroPreviewKind);
        }
        Ok(Self {
            sender_principal: author_at(value, 0, PREVIEW_DELTA_CONTEXT)?,
            scope: scope_at(value, 1, PREVIEW_DELTA_CONTEXT)?,
            preview_sequence: uint_at(value, 2, PREVIEW_DELTA_CONTEXT)?,
            preview_kind,
            expires_at_ms: uint_at(value, 4, PREVIEW_DELTA_CONTEXT)?,
            preview_data: bytes_at(value, 5, PREVIEW_DELTA_CONTEXT)?.to_vec(),
        })
    }
}

/// §2.3 `preview_dropped` reasons: advisory only, never a durable acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewDroppedReason {
    /// The per-principal, per-scope rate limiter dropped this preview.
    RateLimited,
    /// The service dropped this preview under overload.
    Overloaded,
}

impl PreviewDroppedReason {
    const fn to_u64(self) -> u64 {
        match self {
            Self::RateLimited => 0,
            Self::Overloaded => 1,
        }
    }

    const fn from_u64(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::RateLimited,
            1 => Self::Overloaded,
            _ => return None,
        })
    }
}

/// §2.3 `preview_dropped` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewDroppedBody {
    /// The affected scope.
    pub scope: Scope,
    /// The sender-local sequence of the dropped preview, if known.
    pub preview_sequence: Option<u64>,
    /// Advisory reason.
    pub reason: PreviewDroppedReason,
}

impl PreviewDroppedBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        let mut entries = vec![(CborValue::Uint(0), scope_entry(&self.scope)?)];
        if let Some(sequence) = self.preview_sequence {
            entries.push((CborValue::Uint(1), CborValue::Uint(sequence)));
        }
        entries.push((CborValue::Uint(2), CborValue::Uint(self.reason.to_u64())));
        Ok(CborValue::Map(entries))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        reject_reserved_preview_keys(value)?;
        let entries = value.as_map().ok_or(WireError::ExpectedMap {
            context: PREVIEW_DROPPED_CONTEXT,
        })?;
        let has_sequence = value.get_uint_key(1).is_some();
        let expected_len = if has_sequence { 3 } else { 2 };
        if entries.len() != expected_len {
            return Err(WireError::UnexpectedKeys {
                context: PREVIEW_DROPPED_CONTEXT,
            });
        }
        let reason_raw = uint_at(value, 2, PREVIEW_DROPPED_CONTEXT)?;
        Ok(Self {
            scope: scope_at(value, 0, PREVIEW_DROPPED_CONTEXT)?,
            preview_sequence: if has_sequence {
                Some(uint_at(value, 1, PREVIEW_DROPPED_CONTEXT)?)
            } else {
                None
            },
            reason: PreviewDroppedReason::from_u64(reason_raw).ok_or(
                WireError::UnknownDiscriminant {
                    context: PREVIEW_DROPPED_CONTEXT,
                    key: 2,
                    value: reason_raw,
                },
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical_checked};
    use crate::envelope::Identifier32;

    fn sample_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    #[test]
    fn preview_send_round_trips_and_rejects_a_zero_kind() {
        let body = PreviewSendBody {
            session_generation: 1,
            authorization_binding_id: [0xaa; 16],
            scope: sample_scope(),
            preview_sequence: 42,
            preview_kind: 3,
            expires_at_ms: 1_700_000_000_000,
            preview_data: vec![9, 9],
        };
        let bytes = encode_canonical_checked(&body.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            PreviewSendBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            body
        );

        let mut zero_kind = body;
        zero_kind.preview_kind = 0;
        let bytes = encode_canonical_checked(&zero_kind.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            PreviewSendBody::from_cbor(&decode_canonical(&bytes).expect("decode")),
            Err(WireError::ZeroPreviewKind)
        );
    }

    #[test]
    fn preview_delta_round_trips() {
        let body = PreviewDeltaBody {
            sender_principal: Author::from_public_key([0x22; 32]),
            scope: sample_scope(),
            preview_sequence: 7,
            preview_kind: 1,
            expires_at_ms: 1_700_000_000_000,
            preview_data: vec![1, 2],
        };
        let bytes = encode_canonical_checked(&body.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            PreviewDeltaBody::from_cbor(&decode_canonical(&bytes).expect("decode"))
                .expect("decode"),
            body
        );
    }

    #[test]
    fn preview_dropped_round_trips_with_and_without_a_sequence() {
        for preview_sequence in [Some(5u64), None] {
            let body = PreviewDroppedBody {
                scope: sample_scope(),
                preview_sequence,
                reason: PreviewDroppedReason::RateLimited,
            };
            let bytes = encode_canonical_checked(&body.to_cbor().expect("cbor")).expect("encode");
            assert_eq!(
                PreviewDroppedBody::from_cbor(&decode_canonical(&bytes).expect("decode"))
                    .expect("decode"),
                body
            );
        }
    }

    #[test]
    fn a_reserved_key_is_rejected_even_alongside_an_otherwise_valid_body() {
        let mut entries = match (PreviewSendBody {
            session_generation: 1,
            authorization_binding_id: [0xaa; 16],
            scope: sample_scope(),
            preview_sequence: 1,
            preview_kind: 1,
            expires_at_ms: 1,
            preview_data: Vec::new(),
        })
        .to_cbor()
        .expect("cbor")
        {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries.push((CborValue::Uint(93), CborValue::Bytes(vec![0; 32])));
        let smuggled = CborValue::Map(entries);
        assert_eq!(
            PreviewSendBody::from_cbor(&smuggled),
            Err(WireError::ForbiddenPreviewField { field: "branch_id" })
        );
    }
}
