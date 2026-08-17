//! SPEC-8 §5.3 cutover comparator: deterministic, quantization-only field
//! diffing. No float-tolerance path exists anywhere in this module — see
//! `fe-database/src/migration/AGENTS.md` §comparator.

use std::collections::{BTreeMap, BTreeSet};

/// A canonical value after explicit quantization (§2.4, §5.3.3). No float
/// variant exists on purpose: a legacy display float MUST be quantized before
/// comparison, never compared with tolerance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuantizedValue {
    Integer(i64),
    Text(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Tombstoned,
    /// A legacy value that could not be losslessly quantized. Always a
    /// difference — even against another `Unquantizable` with identical
    /// `raw_debug` text (§5.3.3): an unquantizable or stale value is never
    /// masked as equal.
    Unquantizable {
        raw_debug: String,
    },
}

/// A deterministic canonical export: sorted field-path -> quantized value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CanonicalExport {
    pub fields: BTreeMap<String, QuantizedValue>,
}

impl CanonicalExport {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Hex BLAKE3 root over the sorted field map — the export's deterministic
    /// identity (§5.2.3).
    pub fn root_hex(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        for (path, value) in &self.fields {
            hasher.update(path.as_bytes());
            hasher.update(&[0u8]);
            hasher.update(&quantized_value_bytes(value));
            hasher.update(&[0u8]);
        }
        hex::encode(hasher.finalize().as_bytes())
    }
}

fn quantized_value_bytes(value: &QuantizedValue) -> Vec<u8> {
    match value {
        QuantizedValue::Integer(i) => format!("int:{i}").into_bytes(),
        QuantizedValue::Text(s) => format!("text:{s}").into_bytes(),
        QuantizedValue::Bytes(b) => {
            let mut v = b"bytes:".to_vec();
            v.extend_from_slice(b);
            v
        }
        QuantizedValue::Bool(b) => format!("bool:{b}").into_bytes(),
        QuantizedValue::Tombstoned => b"tombstoned".to_vec(),
        QuantizedValue::Unquantizable { raw_debug } => {
            format!("unquantizable:{raw_debug}").into_bytes()
        }
    }
}

/// How one field path's legacy/shadow pair compares (§5.3.1-§5.3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldDifferenceClass {
    MissingFromShadow,
    MissingFromLegacy,
    ValueMismatch,
    /// Either side was `Unquantizable` — always a difference, never masked
    /// (§5.3.3).
    AlwaysDiffersUnquantizable,
}

/// One machine-readable field difference (§5.3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDifference {
    pub field_path: String,
    pub legacy_value: Option<QuantizedValue>,
    pub shadow_value: Option<QuantizedValue>,
    pub class: FieldDifferenceClass,
}

/// The full machine-readable comparison record for one candidate (§5.3.4: run,
/// correlation, scope, candidate ID, legacy export hash, shadow export hash,
/// field path, and classification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonRecord {
    pub run_id: String,
    pub caller_correlation_id: String,
    pub scope: String,
    pub candidate_id: String,
    pub legacy_export_hash: String,
    pub shadow_export_hash: String,
    pub differences: Vec<FieldDifference>,
}

impl ComparisonRecord {
    pub fn is_equal(&self) -> bool {
        self.differences.is_empty()
    }
}

/// Compare two canonical exports field-by-field. Quantization-only
/// normalization: no float-tolerance path exists anywhere in this function
/// (§5.3.3). Any `Unquantizable` value is always reported as a difference,
/// even against another `Unquantizable` with identical debug text.
pub fn compare_projections(
    run_id: &str,
    caller_correlation_id: &str,
    scope: &str,
    candidate_id: &str,
    legacy: &CanonicalExport,
    shadow: &CanonicalExport,
) -> ComparisonRecord {
    let mut differences = Vec::new();
    let mut all_paths: BTreeSet<&String> = legacy.fields.keys().collect();
    all_paths.extend(shadow.fields.keys());

    for path in all_paths {
        let legacy_value = legacy.fields.get(path);
        let shadow_value = shadow.fields.get(path);

        match (legacy_value, shadow_value) {
            (Some(l), Some(s)) => {
                let either_unquantizable = matches!(l, QuantizedValue::Unquantizable { .. })
                    || matches!(s, QuantizedValue::Unquantizable { .. });
                if either_unquantizable {
                    differences.push(FieldDifference {
                        field_path: path.clone(),
                        legacy_value: Some(l.clone()),
                        shadow_value: Some(s.clone()),
                        class: FieldDifferenceClass::AlwaysDiffersUnquantizable,
                    });
                } else if l != s {
                    differences.push(FieldDifference {
                        field_path: path.clone(),
                        legacy_value: Some(l.clone()),
                        shadow_value: Some(s.clone()),
                        class: FieldDifferenceClass::ValueMismatch,
                    });
                }
            }
            (Some(l), None) => differences.push(FieldDifference {
                field_path: path.clone(),
                legacy_value: Some(l.clone()),
                shadow_value: None,
                class: FieldDifferenceClass::MissingFromShadow,
            }),
            (None, Some(s)) => differences.push(FieldDifference {
                field_path: path.clone(),
                legacy_value: None,
                shadow_value: Some(s.clone()),
                class: FieldDifferenceClass::MissingFromLegacy,
            }),
            (None, None) => unreachable!("path came from one of the two field maps"),
        }
    }

    ComparisonRecord {
        run_id: run_id.to_string(),
        caller_correlation_id: caller_correlation_id.to_string(),
        scope: scope.to_string(),
        candidate_id: candidate_id.to_string(),
        legacy_export_hash: legacy.root_hex(),
        shadow_export_hash: shadow.root_hex(),
        differences,
    }
}

/// Compare a full history of sequential prefixes (§5.3.5): a compensating later
/// sequence cannot conceal an earlier divergent reduction, so every prefix —
/// not just the final state — gets its own comparison record.
pub fn compare_prefix_history(
    run_id: &str,
    caller_correlation_id: &str,
    scope: &str,
    candidate_id: &str,
    legacy_prefixes: &[CanonicalExport],
    shadow_prefixes: &[CanonicalExport],
) -> Vec<ComparisonRecord> {
    legacy_prefixes
        .iter()
        .zip(shadow_prefixes.iter())
        .enumerate()
        .map(|(i, (legacy, shadow))| {
            compare_projections(
                run_id,
                &format!("{caller_correlation_id}#prefix-{i}"),
                scope,
                candidate_id,
                legacy,
                shadow,
            )
        })
        .collect()
}
