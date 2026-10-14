//! Preview protocol (§7.1): `preview_send`, `preview_delta`, `preview_dropped`, and the SPEC-10
//! §6 `preview_kind` registry with its one registered kind, `telemetry_sample_v1`.
//!
//! This module MUST import nothing from `commit`, `subscription`, `snapshot`, or `cursor` —
//! that absence of an import path is the mechanism that makes preview frames structurally
//! disjoint from commit frames, not a runtime convention. See `src/wire/AGENTS.md`.

use thiserror::Error;

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{Author, Identifier32, Scope};

use super::error::WireError;
use super::{
    author_at, bytes_at, fixed_bytes_at, get, identifier32_at, require_exact_keys, require_no_keys,
    scope_at, scope_entry, u32_at, uint_at,
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

// ---- SPEC-10 §6 preview_kind registry; see `src/wire/AGENTS.md` §preview-kind-registry ----

/// `telemetry_sample_v1` (SPEC-10 §6 rule 1).
pub const TELEMETRY_SAMPLE_PREVIEW_KIND: u32 = 1;
/// The last value reserved for specification registration (2 to 1023).
pub const LAST_RESERVED_PREVIEW_KIND: u32 = 1023;
/// The first application/experimental value (no interop claimed).
pub const FIRST_APPLICATION_PREVIEW_KIND: u32 = 1024;

const TELEMETRY_SAMPLE_CONTEXT: &str = "telemetry_sample_v1";

/// What the registry says a non-zero `preview_kind` is (SPEC-10 §6 rule 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewKindClass {
    /// Kind 1, `telemetry_sample_v1`.
    TelemetrySampleV1,
    /// 2 to 1023 with no registering specification; never sent, dropped on receipt.
    ReservedUnregistered,
    /// 1024 and above, interpreted only by a local application handler.
    Application,
}

impl PreviewKindClass {
    /// Classifies a wire `preview_kind`; zero is the SPEC-7 decoder's own refusal.
    pub fn of(preview_kind: u32) -> Result<Self, WireError> {
        match preview_kind {
            0 => Err(WireError::ZeroPreviewKind),
            TELEMETRY_SAMPLE_PREVIEW_KIND => Ok(Self::TelemetrySampleV1),
            2..=LAST_RESERVED_PREVIEW_KIND => Ok(Self::ReservedUnregistered),
            _ => Ok(Self::Application),
        }
    }
}

/// The application preview kinds a receiver has a local handler for.
pub trait ApplicationPreviewHandlers {
    /// Whether this receiver can display application kind `preview_kind` (1024 and above).
    fn handles(&self, preview_kind: u32) -> bool;
}

/// A receiver with no application preview handler: every application kind is dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoApplicationPreviewHandlers;

impl ApplicationPreviewHandlers for NoApplicationPreviewHandlers {
    fn handles(&self, _preview_kind: u32) -> bool {
        false
    }
}

/// A preview the registry admits for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisteredPreview {
    /// A decoded `telemetry_sample_v1`.
    TelemetrySample(TelemetrySamplePreview),
    /// An application kind a local handler claimed; its data stays opaque here.
    Application {
        /// The application kind.
        preview_kind: u32,
        /// Its untrusted display data, for the handler.
        preview_data: Vec<u8>,
    },
}

/// Why a registry-aware receiver silently dropped a preview (SPEC-10 §6 rule 2): display loss,
/// never an error to the sender, never durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewRegistryDrop {
    /// A reserved kind no specification registers.
    ReservedUnregistered {
        /// The dropped kind.
        preview_kind: u32,
    },
    /// An application kind with no local handler.
    UnhandledApplication {
        /// The dropped kind.
        preview_kind: u32,
    },
}

/// The registry's decision for one received preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewRegistryDecision {
    /// Display it.
    Display(RegisteredPreview),
    /// Drop it silently.
    Drop(PreviewRegistryDrop),
}

/// Registry-aware receipt of one structurally decoded preview (a `PreviewDeltaBody`'s kind and
/// data). Kind 1 must decode as `telemetry_sample_v1` or is rejected; unregistered reserved kinds
/// and unhandled application kinds are dropped. Nothing here persists or converts a preview into
/// a reading, a batch, or hot-plane state (§6 rule 4).
pub fn decode_registered_preview(
    preview_kind: u32,
    preview_data: &[u8],
    handlers: &impl ApplicationPreviewHandlers,
) -> Result<PreviewRegistryDecision, WireError> {
    Ok(match PreviewKindClass::of(preview_kind)? {
        PreviewKindClass::TelemetrySampleV1 => {
            PreviewRegistryDecision::Display(RegisteredPreview::TelemetrySample(
                TelemetrySamplePreview::decode_preview_data(preview_data)?,
            ))
        }
        PreviewKindClass::ReservedUnregistered => {
            PreviewRegistryDecision::Drop(PreviewRegistryDrop::ReservedUnregistered {
                preview_kind,
            })
        }
        PreviewKindClass::Application if handlers.handles(preview_kind) => {
            PreviewRegistryDecision::Display(RegisteredPreview::Application {
                preview_kind,
                preview_data: preview_data.to_vec(),
            })
        }
        PreviewKindClass::Application => {
            PreviewRegistryDecision::Drop(PreviewRegistryDrop::UnhandledApplication {
                preview_kind,
            })
        }
    })
}

/// Why a reading value cannot become a `value_nano` (SPEC-10 §6 rule 3).
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TelemetryValueError {
    /// NaN or an infinity has no nano-unit value.
    #[error("telemetry value is not finite")]
    NonFinite,
    /// The nano-unit quantization does not fit `i64`; such a preview MUST NOT be sent.
    #[error("telemetry value overflows i64 nano-units")]
    Overflow,
}

/// Quantizes a reading value to `i64` nano-units of its units, rounded half to even (D-CL5). The
/// scale by 10^9 happens in `f64`, so the tie test applies to the scaled double.
pub fn quantize_value_nano(value: f64) -> Result<i64, TelemetryValueError> {
    if !value.is_finite() {
        return Err(TelemetryValueError::NonFinite);
    }
    let rounded = (value * 1e9).round_ties_even();
    // 2^63 is exactly representable; every double in [-2^63, 2^63) fits i64.
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&rounded) {
        return Err(TelemetryValueError::Overflow);
    }
    Ok(rounded as i64)
}

/// `telemetry_sample_v1` preview data (SPEC-10 §6 rule 3): canonical CBOR, integers only.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TelemetrySamplePreview {
    /// Key 0: SPEC-10 §4.5 identifier of the anchor node (`observation::node_anchor`).
    pub anchor: Identifier32,
    /// Key 1: non-empty metric name.
    pub metric: String,
    /// Key 2: the value in nano-units of `units` ([`quantize_value_nano`]).
    pub value_nano: i64,
    /// Key 3: units; may be empty.
    pub units: String,
    /// Key 4: sensor time, Unix milliseconds.
    pub recorded_at_ms: i64,
}

impl TelemetrySamplePreview {
    /// Encodes the five-key map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Bytes(self.anchor.0.to_vec())),
            (CborValue::Uint(1), CborValue::text(&self.metric)),
            (CborValue::Uint(2), CborValue::integer(self.value_nano)),
            (CborValue::Uint(3), CborValue::text(&self.units)),
            (CborValue::Uint(4), CborValue::integer(self.recorded_at_ms)),
        ])
    }

    /// Decodes the map: a reserved SPEC-7 operation field first, then exactly keys 0 to 4.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        reject_reserved_preview_keys(value)?;
        require_exact_keys(value, &[0, 1, 2, 3, 4], TELEMETRY_SAMPLE_CONTEXT)?;
        let metric = telemetry_text(value, 1)?;
        if metric.is_empty() {
            return Err(WireError::WrongShape {
                context: TELEMETRY_SAMPLE_CONTEXT,
                key: 1,
            });
        }
        Ok(Self {
            anchor: identifier32_at(value, 0, TELEMETRY_SAMPLE_CONTEXT)?,
            metric,
            value_nano: telemetry_int(value, 2)?,
            units: telemetry_text(value, 3)?,
            recorded_at_ms: telemetry_int(value, 4)?,
        })
    }

    /// The canonical bytes a `preview_send`/`preview_delta` carries as `preview_data`.
    pub fn encode_preview_data(&self) -> Result<Vec<u8>, WireError> {
        if self.metric.is_empty() {
            return Err(WireError::WrongShape {
                context: TELEMETRY_SAMPLE_CONTEXT,
                key: 1,
            });
        }
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes `preview_data` bytes: canonical CBOR (no floats, no duplicate keys), then the map.
    pub fn decode_preview_data(bytes: &[u8]) -> Result<Self, WireError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }
}

fn telemetry_text(value: &CborValue, key: u64) -> Result<String, WireError> {
    get(value, key, TELEMETRY_SAMPLE_CONTEXT)?
        .as_text()
        .map(str::to_owned)
        .ok_or(WireError::WrongShape {
            context: TELEMETRY_SAMPLE_CONTEXT,
            key,
        })
}

fn telemetry_int(value: &CborValue, key: u64) -> Result<i64, WireError> {
    get(value, key, TELEMETRY_SAMPLE_CONTEXT)?
        .as_int()
        .ok_or(WireError::WrongShape {
            context: TELEMETRY_SAMPLE_CONTEXT,
            key,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical_checked};

    fn sample() -> TelemetrySamplePreview {
        TelemetrySamplePreview {
            anchor: crate::observation::node_anchor("01HNODE0000000000000000000").expect("ascii"),
            metric: "temperature".to_string(),
            value_nano: quantize_value_nano(21.5).expect("finite"),
            units: "degC".to_string(),
            recorded_at_ms: 1_700_000_000_000,
        }
    }

    struct Handles(u32);

    impl ApplicationPreviewHandlers for Handles {
        fn handles(&self, preview_kind: u32) -> bool {
            preview_kind == self.0
        }
    }

    fn sample_entries() -> Vec<(CborValue, CborValue)> {
        match sample().to_cbor() {
            CborValue::Map(entries) => entries,
            _ => unreachable!("to_cbor builds a map"),
        }
    }

    fn sample_with(key: u64, value: CborValue) -> Vec<u8> {
        let mut entries = sample_entries();
        entries.retain(|(existing, _)| *existing != CborValue::Uint(key));
        entries.push((CborValue::Uint(key), value));
        encode_canonical_checked(&CborValue::Map(entries)).expect("encode")
    }

    /// SPEC-10 §10 test 5: kind 1 decodes, a reserved unregistered kind and an unhandled
    /// application kind are dropped, and a telemetry preview carrying a reserved SPEC-7 operation
    /// field (or any extra, missing, empty, or float-valued key) is rejected, never displayed.
    #[test]
    fn telemetry_preview_registry_accepts_kind1_drops_unregistered_and_rejects_op_fields() {
        let preview = sample();
        let data = preview.encode_preview_data().expect("encode");
        assert_eq!(
            decode_registered_preview(1, &data, &NoApplicationPreviewHandlers),
            Ok(PreviewRegistryDecision::Display(
                RegisteredPreview::TelemetrySample(preview)
            ))
        );

        for (kind, dropped) in [
            (
                2,
                PreviewRegistryDrop::ReservedUnregistered { preview_kind: 2 },
            ),
            (
                1023,
                PreviewRegistryDrop::ReservedUnregistered { preview_kind: 1023 },
            ),
            (
                1024,
                PreviewRegistryDrop::UnhandledApplication { preview_kind: 1024 },
            ),
            (
                u32::MAX,
                PreviewRegistryDrop::UnhandledApplication {
                    preview_kind: u32::MAX,
                },
            ),
        ] {
            assert_eq!(
                decode_registered_preview(kind, &data, &Handles(5_000)),
                Ok(PreviewRegistryDecision::Drop(dropped)),
                "kind {kind}"
            );
        }
        assert_eq!(
            decode_registered_preview(5_000, b"opaque", &Handles(5_000)),
            Ok(PreviewRegistryDecision::Display(
                RegisteredPreview::Application {
                    preview_kind: 5_000,
                    preview_data: b"opaque".to_vec(),
                }
            ))
        );
        assert_eq!(
            decode_registered_preview(0, &data, &NoApplicationPreviewHandlers),
            Err(WireError::ZeroPreviewKind)
        );

        // Key 2 re-encoded as a CBOR double (0xfb): a float anywhere is refused.
        let mut float_value = sample_with(2, CborValue::Uint(0x0102_0304));
        let at = float_value
            .windows(6)
            .position(|window| window == [0x02, 0x1a, 0x01, 0x02, 0x03, 0x04])
            .expect("key 2 encoding");
        float_value.splice(at + 1..at + 6, [0xfb, 0x40, 0x35, 0x80, 0, 0, 0, 0, 0]);
        let mut without_units = sample_entries();
        without_units.retain(|(key, _)| *key != CborValue::Uint(3));
        let without_units =
            encode_canonical_checked(&CborValue::Map(without_units)).expect("encode");

        for (case, bytes, expected) in [
            (
                "reserved op_id field",
                sample_with(90, CborValue::Bytes(vec![0; 32])),
                Some(WireError::ForbiddenPreviewField { field: "op_id" }),
            ),
            (
                "reserved hlc field",
                sample_with(94, CborValue::Uint(1)),
                Some(WireError::ForbiddenPreviewField { field: "hlc" }),
            ),
            ("unknown key", sample_with(5, CborValue::Uint(1)), None),
            ("missing key", without_units, None),
            ("empty metric", sample_with(1, CborValue::text("")), None),
            ("float value", float_value, None),
        ] {
            let outcome = decode_registered_preview(1, &bytes, &NoApplicationPreviewHandlers);
            match expected {
                Some(error) => assert_eq!(outcome, Err(error), "{case}"),
                None => assert!(outcome.is_err(), "{case} must be rejected"),
            }
        }
    }

    /// D-CL5 nano quantization rounds (never truncates); non-finite and out-of-range values are
    /// refused.
    #[test]
    fn telemetry_values_quantize_half_to_even_and_refuse_unsendable_values() {
        for (value, expected) in [
            (1.0, Ok(1_000_000_000)),
            (-2.25, Ok(-2_250_000_000)),
            (0.7e-9, Ok(1)),
            (-1.999_999_999_6, Ok(-2_000_000_000)),
            (f64::NAN, Err(TelemetryValueError::NonFinite)),
            (f64::INFINITY, Err(TelemetryValueError::NonFinite)),
            (9.3e9, Err(TelemetryValueError::Overflow)),
            (-9.3e9, Err(TelemetryValueError::Overflow)),
        ] {
            assert_eq!(quantize_value_nano(value), expected, "{value}");
        }
    }

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
