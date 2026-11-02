//! Bounded Petal declarations; see src/AGENTS.md (petal-requirements).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Supported catalog content kinds.
pub const HEXON_TYPES: [&str; 12] = [
    "Scene",
    "Model",
    "Material",
    "Skybox",
    "Terrain",
    "TerrainTileset",
    "GpxCollection",
    "Surface",
    "Sound",
    "VisualLayer",
    "Theme",
    "Bundle",
];

/// One declared content dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestHexonEntry {
    /// Catalog identifier, never a filesystem path.
    pub hexon_id: String,
    /// One supported catalog content kind.
    pub hexon_type: String,
    /// Whether the declaration marks this dependency as required.
    pub required: bool,
}

/// Persisted declarations, independent of residency settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PetalRequirements {
    /// At most 256 uniquely identified catalog dependencies.
    #[serde(default)]
    pub hexons: Vec<ManifestHexonEntry>,
}

impl PetalRequirements {
    /// Check the common writer and strictest-reader bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.hexons.len() > 256
            || serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > 65_536
        {
            return Err("Requirements exceed 256 entries or 64 KiB".into());
        }
        let mut ids = std::collections::HashSet::new();
        for entry in &self.hexons {
            if entry.hexon_id.len() > 128
                || !crate::safe_id::is_safe_path_id(&entry.hexon_id)
                || !HEXON_TYPES.contains(&entry.hexon_type.as_str())
                || !ids.insert(&entry.hexon_id)
            {
                return Err(
                    "Requirements contain an invalid or duplicate ID or unsupported type".into(),
                );
            }
        }
        Ok(())
    }

    /// Decode current or explicit legacy documents without dropping hidden fields.
    pub fn from_document(document: Option<&Value>) -> Result<Self, String> {
        let Some(document) = document else {
            return Ok(Self::default());
        };
        if serde_json::to_vec(document)
            .map_err(|e| e.to_string())?
            .len()
            > 65_536
        {
            return Err("Requirements document exceeds 64 KiB".into());
        }
        let object = document
            .as_object()
            .ok_or("Requirements must be an object")?;
        if object
            .keys()
            .any(|key| !["hexons", "render_distance", "fallback"].contains(&key.as_str()))
        {
            return Err("Requirements contain unknown fields; refusing a lossy edit".into());
        }
        if let Some(distance) = object.get("render_distance") {
            if !distance
                .as_f64()
                .is_some_and(|v| v.is_finite() && (0.0..=50_000.0).contains(&v))
            {
                return Err("Invalid legacy requirements render distance".into());
            }
        }
        if object
            .get("fallback")
            .is_some_and(|value| value.as_str() != Some("sign"))
        {
            return Err("Unsupported legacy requirements fallback".into());
        }
        let mut current = object.clone();
        current.remove("render_distance");
        current.remove("fallback");
        let result: Self =
            serde_json::from_value(Value::Object(current)).map_err(|e| e.to_string())?;
        result.validate()?;
        Ok(result)
    }

    /// Replace declarations while preserving explicitly supported legacy metadata.
    pub fn update_document(&self, previous: Option<&Value>) -> Result<Value, String> {
        self.validate()?;
        Self::from_document(previous)?;
        let mut document = previous
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        document.insert(
            "hexons".into(),
            serde_json::to_value(&self.hexons).map_err(|e| e.to_string())?,
        );
        let document = Value::Object(document);
        Self::from_document(Some(&document))?;
        Ok(document)
    }
}

/// An authorized readback and its exact compare-and-set baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct PetalRequirementsSnapshot {
    /// The exact requested Petal identity.
    pub petal_id: String,
    /// Stored ancestry at the fenced read or commit.
    pub scope: String,
    /// Validated declarations for editing.
    pub requirements: PetalRequirements,
    /// Raw validated document retained for lossless legacy edits and CAS.
    pub document: Option<Value>,
    /// Advisory Manager ability from the same authority snapshot.
    pub can_manage: bool,
}
