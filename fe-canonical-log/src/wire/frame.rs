//! Common wire frame (§2.1) and the message-type registry (§2.3).

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};

use super::error::WireError;

/// The only supported frame version.
pub const WIRE_VERSION: u64 = 1;

/// Which side of the socket may originate a message of a given [`MessageType`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Client → service.
    ClientToService,
    /// Service → client.
    ServiceToClient,
}

/// The §2.3 registered message types, with their EXACT normative discriminants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum MessageType {
    /// `authorize`, client → service.
    Authorize = 1,
    /// `authorized`, service → client.
    Authorized = 2,
    /// `commit_submit`, client → service.
    CommitSubmit = 10,
    /// `commit_ack`, service → client.
    CommitAck = 11,
    /// `commit_delta`, service → client.
    CommitDelta = 12,
    /// `subscribe`, client → service.
    Subscribe = 13,
    /// `resume`, client → service.
    Resume = 14,
    /// `replay_complete`, service → client.
    ReplayComplete = 15,
    /// `snapshot_required`, service → client.
    SnapshotRequired = 16,
    /// `scene_snapshot`, service → client.
    SceneSnapshot = 17,
    /// `snapshot_ack`, client → service.
    SnapshotAck = 18,
    /// `authorization_revalidation_required`, service → client.
    AuthorizationRevalidationRequired = 19,
    /// `preview_send`, client → service.
    PreviewSend = 20,
    /// `preview_delta`, service → client.
    PreviewDelta = 21,
    /// `preview_dropped`, service → client.
    PreviewDropped = 22,
    /// `protocol_error`, service → client.
    ProtocolError = 255,
}

impl MessageType {
    /// Decodes the wire `u16` discriminant, rejecting any unregistered value.
    pub const fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::Authorize,
            2 => Self::Authorized,
            10 => Self::CommitSubmit,
            11 => Self::CommitAck,
            12 => Self::CommitDelta,
            13 => Self::Subscribe,
            14 => Self::Resume,
            15 => Self::ReplayComplete,
            16 => Self::SnapshotRequired,
            17 => Self::SceneSnapshot,
            18 => Self::SnapshotAck,
            19 => Self::AuthorizationRevalidationRequired,
            20 => Self::PreviewSend,
            21 => Self::PreviewDelta,
            22 => Self::PreviewDropped,
            255 => Self::ProtocolError,
            _ => return None,
        })
    }

    /// The wire `u16` discriminant.
    pub const fn as_u16(self) -> u16 {
        self as u16
    }

    /// The §2.3 direction this type is registered for.
    pub const fn direction(self) -> Direction {
        match self {
            Self::Authorize
            | Self::CommitSubmit
            | Self::Subscribe
            | Self::Resume
            | Self::SnapshotAck
            | Self::PreviewSend => Direction::ClientToService,
            Self::Authorized
            | Self::CommitAck
            | Self::CommitDelta
            | Self::ReplayComplete
            | Self::SnapshotRequired
            | Self::SceneSnapshot
            | Self::AuthorizationRevalidationRequired
            | Self::PreviewDelta
            | Self::PreviewDropped
            | Self::ProtocolError => Direction::ServiceToClient,
        }
    }
}

/// A decoded common frame: version, type, correlation ID, and an as-yet-untyped body map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// MUST be [`WIRE_VERSION`].
    pub wire_version: u64,
    /// Registered message type.
    pub message_type: MessageType,
    /// Client correlation ID; `None` only for unsolicited server messages.
    pub request_id: Option<[u8; 16]>,
    /// The exact grammar for `message_type`; unknown keys inside it are the body decoder's job.
    pub body: CborValue,
}

impl Frame {
    /// Builds a frame ready for [`encode`].
    pub fn new(message_type: MessageType, request_id: Option<[u8; 16]>, body: CborValue) -> Self {
        Self {
            wire_version: WIRE_VERSION,
            message_type,
            request_id,
            body,
        }
    }

    /// Rejects a frame whose type does not belong to `expected` direction.
    pub fn require_direction(&self, expected: Direction) -> Result<(), WireError> {
        if self.message_type.direction() != expected {
            return Err(WireError::WrongMessageDirection {
                message_type: self.message_type,
            });
        }
        Ok(())
    }

    /// Rejects a frame whose type is not exactly `expected`.
    pub fn require_type(&self, expected: MessageType) -> Result<(), WireError> {
        if self.message_type != expected {
            return Err(WireError::UnexpectedMessageType {
                expected,
                actual: self.message_type,
            });
        }
        Ok(())
    }
}

/// Encodes the four-key §2.1 common frame.
pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>, WireError> {
    let request_id = match frame.request_id {
        Some(bytes) => CborValue::Bytes(bytes.to_vec()),
        None => CborValue::Null,
    };
    let value = CborValue::Map(vec![
        (CborValue::Uint(0), CborValue::Uint(frame.wire_version)),
        (
            CborValue::Uint(1),
            CborValue::Uint(u64::from(frame.message_type.as_u16())),
        ),
        (CborValue::Uint(2), request_id),
        (CborValue::Uint(3), frame.body.clone()),
    ]);
    Ok(encode_canonical_checked(&value)?)
}

/// Decodes and validates the §2.1 common frame: version, known type, well-formed correlation ID.
///
/// Direction and exact message type are the caller's job via [`Frame::require_direction`] and
/// [`Frame::require_type`], because only the caller knows which role it is playing.
pub fn decode_frame(bytes: &[u8]) -> Result<Frame, WireError> {
    const CONTEXT: &str = "frame";
    let value = decode_canonical(bytes)?;
    let entries = value
        .as_map()
        .ok_or(WireError::ExpectedMap { context: CONTEXT })?;
    if entries.len() != 4 {
        return Err(WireError::UnexpectedKeys { context: CONTEXT });
    }
    for key in 0..4u64 {
        if value.get_uint_key(key).is_none() {
            return Err(WireError::MissingKey {
                context: CONTEXT,
                key,
            });
        }
    }

    let wire_version =
        value
            .get_uint_key(0)
            .and_then(CborValue::as_uint)
            .ok_or(WireError::WrongShape {
                context: CONTEXT,
                key: 0,
            })?;
    if wire_version != WIRE_VERSION {
        return Err(WireError::UnsupportedWireVersion {
            version: wire_version,
        });
    }

    let message_type_raw =
        value
            .get_uint_key(1)
            .and_then(CborValue::as_uint)
            .ok_or(WireError::WrongShape {
                context: CONTEXT,
                key: 1,
            })?;
    let message_type = u16::try_from(message_type_raw)
        .ok()
        .and_then(MessageType::from_u16)
        .ok_or(WireError::UnknownMessageType {
            value: message_type_raw,
        })?;

    let request_id_slot = value.get_uint_key(2).expect("checked present above");
    let request_id = if request_id_slot.is_null() {
        None
    } else {
        let raw = request_id_slot.as_bytes().ok_or(WireError::WrongShape {
            context: CONTEXT,
            key: 2,
        })?;
        Some(
            <[u8; 16]>::try_from(raw).map_err(|_| WireError::WrongByteLength {
                context: CONTEXT,
                key: 2,
                expected: 16,
                actual: raw.len(),
            })?,
        )
    };

    let body = value
        .get_uint_key(3)
        .expect("checked present above")
        .clone();

    Ok(Frame {
        wire_version,
        message_type,
        request_id,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_type_round_trips_its_discriminant() {
        let all = [
            MessageType::Authorize,
            MessageType::Authorized,
            MessageType::CommitSubmit,
            MessageType::CommitAck,
            MessageType::CommitDelta,
            MessageType::Subscribe,
            MessageType::Resume,
            MessageType::ReplayComplete,
            MessageType::SnapshotRequired,
            MessageType::SceneSnapshot,
            MessageType::SnapshotAck,
            MessageType::AuthorizationRevalidationRequired,
            MessageType::PreviewSend,
            MessageType::PreviewDelta,
            MessageType::PreviewDropped,
            MessageType::ProtocolError,
        ];
        for message_type in all {
            assert_eq!(
                MessageType::from_u16(message_type.as_u16()),
                Some(message_type)
            );
        }
        assert_eq!(MessageType::from_u16(3), None);
    }

    #[test]
    fn frame_round_trips_with_and_without_a_request_id() {
        let with_id = Frame::new(
            MessageType::Subscribe,
            Some([0xab; 16]),
            CborValue::Map(vec![(CborValue::Uint(0), CborValue::Uint(7))]),
        );
        let encoded = encode_frame(&with_id).expect("encode");
        assert_eq!(decode_frame(&encoded).expect("decode"), with_id);

        let without_id = Frame::new(MessageType::ProtocolError, None, CborValue::Map(Vec::new()));
        let encoded = encode_frame(&without_id).expect("encode");
        assert_eq!(decode_frame(&encoded).expect("decode"), without_id);
    }

    #[test]
    fn decode_rejects_an_unsupported_wire_version() {
        let value = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(2)),
            (CborValue::Uint(1), CborValue::Uint(1)),
            (CborValue::Uint(2), CborValue::Null),
            (CborValue::Uint(3), CborValue::Map(Vec::new())),
        ]);
        let bytes = encode_canonical_checked(&value).expect("encode");
        assert_eq!(
            decode_frame(&bytes),
            Err(WireError::UnsupportedWireVersion { version: 2 })
        );
    }

    #[test]
    fn decode_rejects_an_unknown_message_type() {
        let value = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(1)),
            (CborValue::Uint(1), CborValue::Uint(3)),
            (CborValue::Uint(2), CborValue::Null),
            (CborValue::Uint(3), CborValue::Map(Vec::new())),
        ]);
        let bytes = encode_canonical_checked(&value).expect("encode");
        assert_eq!(
            decode_frame(&bytes),
            Err(WireError::UnknownMessageType { value: 3 })
        );
    }

    #[test]
    fn decode_rejects_an_extra_or_missing_top_level_key() {
        let mut entries = vec![
            (CborValue::Uint(0), CborValue::Uint(1)),
            (CborValue::Uint(1), CborValue::Uint(1)),
            (CborValue::Uint(2), CborValue::Null),
            (CborValue::Uint(3), CborValue::Map(Vec::new())),
        ];
        entries.push((CborValue::Uint(4), CborValue::Uint(0)));
        let bytes = encode_canonical_checked(&CborValue::Map(entries)).expect("encode");
        assert_eq!(
            decode_frame(&bytes),
            Err(WireError::UnexpectedKeys { context: "frame" })
        );

        let truncated = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(1)),
            (CborValue::Uint(1), CborValue::Uint(1)),
            (CborValue::Uint(2), CborValue::Null),
        ]);
        let bytes = encode_canonical_checked(&truncated).expect("encode");
        assert_eq!(
            decode_frame(&bytes),
            Err(WireError::UnexpectedKeys { context: "frame" })
        );
    }

    #[test]
    fn require_direction_and_type_reject_mismatches() {
        let frame = Frame::new(MessageType::CommitSubmit, None, CborValue::Map(Vec::new()));
        assert!(frame.require_direction(Direction::ClientToService).is_ok());
        assert_eq!(
            frame.require_direction(Direction::ServiceToClient),
            Err(WireError::WrongMessageDirection {
                message_type: MessageType::CommitSubmit
            })
        );
        assert!(frame.require_type(MessageType::CommitSubmit).is_ok());
        assert_eq!(
            frame.require_type(MessageType::Subscribe),
            Err(WireError::UnexpectedMessageType {
                expected: MessageType::Subscribe,
                actual: MessageType::CommitSubmit,
            })
        );
    }
}
