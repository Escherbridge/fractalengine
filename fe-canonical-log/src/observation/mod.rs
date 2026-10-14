//! SPEC-10 observation plane, operation half: the kind-5 `observation_batch` payload codec, its
//! schema hash, its reference-only reduction, and the anchor mapping. See
//! `src/observation/AGENTS.md`; the hexon codec (B2) and the sealer/hydrator (B3) live elsewhere.

pub mod payload;
pub mod reduce;

pub use payload::{
    validate_observation_batch_plaintext, HlcRange, ObservationBatchPayload,
    ObservationBatchPayloadParts, ObservationPayloadError, SensorTimeRange, HEXON_FORMAT_VERSION,
};
pub use reduce::{
    reduce_observation_batch, ObservationBatchMaterializer, ObservationBatchReference,
    OBSERVATION_BATCH_MATERIALIZER_ID, OBSERVATION_BATCH_MATERIALIZER_VERSION,
    OBSERVATION_BATCH_ROW_BINDING, OBSERVATION_BATCH_TABLE,
};

pub use crate::kind::OBSERVATION_BATCH_OPERATION_KIND;

use fe_sdk::legacy_identifier::{legacy_identifier, LegacyIdentifierError, LegacyTable};

use crate::envelope::{Hash32, Identifier32};
use crate::kind::OperationKind;
use crate::retention::quarantine::OperationKindAvailability;

/// The NUL-terminated preimage of the kind-5 `schema_hash` (SPEC-10 §4.1 rule 3). Provisional
/// (D-CL34).
pub const OBSERVATION_BATCH_SCHEMA_DOMAIN: &[u8] = b"fe-schema-observation-batch-v1\0";

/// `BLAKE3(ASCII("fe-schema-observation-batch-v1") || 00)`: the only `schema_hash` a kind-5
/// operation may carry. Any other is `unknown_schema` (§4.1 rule 3).
pub fn observation_batch_schema_hash() -> Hash32 {
    Hash32::of(OBSERVATION_BATCH_SCHEMA_DOMAIN)
}

/// The SPEC-10 §4.5 `Identifier32` of a hot-plane node id: the payload's `anchors` entries and a
/// telemetry preview's `anchor`. Delegates to the one `fe_sdk::legacy_identifier`; no second
/// hash lives here (§4.5 rule 2).
pub fn node_anchor(node_id: &str) -> Result<Identifier32, LegacyIdentifierError> {
    legacy_identifier(LegacyTable::Node, node_id).map(Identifier32)
}

/// The kinds THIS build has a structural rule, schema codec, and deterministic reduction for:
/// SPEC-1 kinds 1 to 4 and SPEC-10 kind 5. Application kinds are never known here; a registry
/// that interprets one says so for itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpecRegisteredKinds;

impl OperationKindAvailability for SpecRegisteredKinds {
    fn is_known(&self, operation_kind: u16) -> bool {
        matches!(
            OperationKind::from_u16(operation_kind),
            Ok(kind) if !matches!(kind, OperationKind::Application(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::QuarantineReason;
    use crate::retention::quarantine::unknown_kind_promotion_ready;

    /// A kind-5 record an un-upgraded build quarantined promotes once this build knows kind 5;
    /// an application kind still does not.
    #[test]
    fn an_upgraded_build_promotes_quarantined_kind5_but_not_application_kinds() {
        let kind5 = QuarantineReason::UnknownKind {
            operation_kind: OBSERVATION_BATCH_OPERATION_KIND,
        };
        let application = QuarantineReason::UnknownKind {
            operation_kind: 0x0200,
        };
        assert!(unknown_kind_promotion_ready(&kind5, &SpecRegisteredKinds));
        assert!(!unknown_kind_promotion_ready(
            &application,
            &SpecRegisteredKinds
        ));
        assert!(!SpecRegisteredKinds.is_known(0));
    }
}
