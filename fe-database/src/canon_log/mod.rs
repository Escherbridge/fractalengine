//! SurrealDB-backed half of the Canonical Fractal Data Log (SPEC-4, plus SPEC-3 §5 epoch
//! state). PARALLEL AND DORMANT — see `canon_log/AGENTS.md`.

pub mod admission;
pub mod append_store;
pub mod apply_marker_store;
pub mod canonical_epoch;
pub mod rebuild;
pub mod replay;

use fe_canonical_log::envelope::{Hash32, Identifier32};

/// Every way the SurrealDB substrate itself fails a canonical-log operation.
///
/// Deliberately distinct from `AdmissionOutcome` and `AppendError`: a storage fault is neither
/// a protocol reject nor a quarantine, and collapsing it into either would let an unavailable
/// database masquerade as a settled decision about a candidate.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The query or its statements failed.
    #[error("{context}: {detail}")]
    Query {
        /// What the caller was doing.
        context: String,
        /// The underlying SurrealDB message.
        detail: String,
    },
    /// A stored row could not be read back into its expected shape.
    #[error("{context}: stored row is malformed: {detail}")]
    MalformedRow {
        /// What the caller was reading.
        context: String,
        /// Why the row could not be interpreted.
        detail: String,
    },
}

impl StorageError {
    /// Builds a query failure.
    pub(crate) fn query(context: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        Self::Query {
            context: context.into(),
            detail: detail.to_string(),
        }
    }

    /// Builds a malformed-row failure.
    pub(crate) fn malformed(context: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        Self::MalformedRow {
            context: context.into(),
            detail: detail.to_string(),
        }
    }
}

/// Lowercase hex of an operation content address.
pub fn op_id_to_hex(op_id: Hash32) -> String {
    hex::encode(op_id.as_bytes())
}

/// Parses lowercase hex back into an operation content address.
pub fn op_id_from_hex(text: &str) -> Option<Hash32> {
    Some(Hash32(decode_32(text)?))
}

/// Lowercase hex of a 32-byte identifier.
pub fn identifier_to_hex(identifier: Identifier32) -> String {
    hex::encode(identifier.as_bytes())
}

/// Parses lowercase hex back into a 32-byte identifier.
pub fn identifier_from_hex(text: &str) -> Option<Identifier32> {
    Some(Identifier32(decode_32(text)?))
}

fn decode_32(text: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(text).ok()?;
    bytes.try_into().ok()
}

/// A local wall-clock stamp for diagnostic columns only.
///
/// SPEC-4 §4.1 forbids a reduction from reading a wall clock; nothing produced here is ever an
/// input to `CausalMaterializer::reduce` or to a projection root hash. It exists so an operator
/// can order local events during an incident.
pub(crate) fn local_stamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_both_thirty_two_byte_types() {
        let op_id = Hash32([0xab; 32]);
        assert_eq!(op_id_from_hex(&op_id_to_hex(op_id)), Some(op_id));

        let identifier = Identifier32([0x07; 32]);
        assert_eq!(
            identifier_from_hex(&identifier_to_hex(identifier)),
            Some(identifier)
        );
    }

    #[test]
    fn a_short_or_non_hex_string_is_not_an_identity() {
        assert_eq!(op_id_from_hex(""), None);
        assert_eq!(op_id_from_hex("ab"), None);
        assert_eq!(op_id_from_hex(&"zz".repeat(32)), None);
        assert_eq!(identifier_from_hex(&"ab".repeat(33)), None);
    }
}
