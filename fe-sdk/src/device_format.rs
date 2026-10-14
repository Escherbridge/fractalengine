//! R5 device format descriptors (SPEC-10 §2.2): the serde-only shape a device
//! extension declares before its payloads can be ingested. Rationale:
//! `src/AGENTS.md` §device-format.

use serde::{Deserialize, Serialize};

use crate::source_class::is_valid_format_id;

/// A declared device payload format: `format_id` + MIME + optional informative
/// JSON Schema. Bytes are never validated against `schema` in v1 (R5: "bytes
/// stored opaque") — it exists for operator/tooling documentation only. Serde-only
/// so a future plugin manifest can embed it without pulling in engine internals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceFormatDescriptor {
    /// `<namespace>/<name>@<major>` — validated by [`is_valid_format_id`].
    pub format_id: String,
    /// The MIME type every asset ingested under this format must carry.
    pub mime: String,
    /// Informative JSON Schema for the payload bytes; never enforced in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
}

impl DeviceFormatDescriptor {
    /// Whether `format_id` and `mime` are individually well-formed. Does not
    /// check uniqueness/conflict against an existing declaration — that is a
    /// DB-layer concern (`fe-database::handlers::device_payload`).
    pub fn is_well_formed(&self) -> bool {
        is_valid_format_id(&self.format_id) && !self.mime.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_descriptor_table() {
        let ok = DeviceFormatDescriptor {
            format_id: "acme.thermo/frame@1".to_string(),
            mime: "application/octet-stream".to_string(),
            schema: None,
            description: String::new(),
        };
        assert!(ok.is_well_formed());

        let bad_format_id = DeviceFormatDescriptor {
            format_id: "not-a-format-id".to_string(),
            ..ok.clone()
        };
        assert!(!bad_format_id.is_well_formed());

        let bad_mime = DeviceFormatDescriptor {
            mime: "  ".to_string(),
            ..ok
        };
        assert!(!bad_mime.is_well_formed());
    }

    #[test]
    fn serde_round_trip_keeps_schema() {
        let d = DeviceFormatDescriptor {
            format_id: "acme.thermo/frame@1".to_string(),
            mime: "application/octet-stream".to_string(),
            schema: Some(serde_json::json!({"type": "object"})),
            description: "Thermo frame v1".to_string(),
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: DeviceFormatDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }
}
