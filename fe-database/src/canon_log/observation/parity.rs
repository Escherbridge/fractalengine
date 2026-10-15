//! SPEC-10 §8 rule 4 `shadow_rebuild` comparison: the hydrated catalog against the hot rows of
//! the same membership (this node's own door accepts), field by field, values by exact bit
//! pattern; see `canon_log/AGENTS.md` §observation.

use std::collections::{BTreeMap, HashMap};

use fe_canonical_log::observation::node_anchor;

use super::hydrator::{catalog_payloads, catalog_readings, CatalogPayload, CatalogReading};
use super::{
    hydrates_observations, load_members, require_mode, resolve_members, ObservationPlaneError,
    ObservationTable, ResolvedObservation,
};
use crate::canon_log::row_exists;
use crate::migration::comparator::{
    compare_projections, CanonicalExport, ComparisonRecord, QuantizedValue,
};
use crate::migration::mode::MigrationMode;
use crate::migration::report::RunCounters;
use crate::repo::Db;

/// The SPEC-8 §6 evidence of one observation-family comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationParityReport {
    /// Field-level comparison of the sealed membership (SPEC-8 §5.3.4 record).
    pub comparison: ComparisonRecord,
    /// Every door-recorded member.
    pub members_total: u64,
    /// Members whose batch is in the verified log (the compared set).
    pub members_sealed: u64,
    /// Members that can never be sealed (`legacy_succeeded_without_candidate`).
    pub members_uncovered: u64,
    /// Members not sealed yet (an open window, or a claim whose append never landed).
    pub members_pending: u64,
}

impl ObservationParityReport {
    /// The §6.1 counters this comparison contributes: capture completeness and parity.
    pub fn run_counters(&self) -> RunCounters {
        RunCounters {
            legacy_success: self.members_total,
            legacy_success_with_candidate: self.members_sealed,
            unapproved_field_differences: self.comparison.differences.len() as u64,
            ..RunCounters::default()
        }
    }

    /// Whether this evidence alone blocks promotion (SPEC-10 §8 rules 3-4).
    pub fn blocks_promotion(&self) -> bool {
        !self.comparison.is_equal()
            || self.members_uncovered > 0
            || self.members_pending > 0
            || self.members_sealed != self.members_total
    }
}

/// Compares the hydrated catalog with the hot rows of this node's sealed membership.
///
/// `shadow_rebuild` only. Rows the hot plane holds from replication are not members, so they
/// are outside the comparison; catalog rows outside the membership are ignored likewise.
pub async fn compare_catalog_with_legacy(
    mode: MigrationMode,
    db: &Db,
    run_id: &str,
) -> Result<ObservationParityReport, ObservationPlaneError> {
    require_mode(mode, hydrates_observations)?;
    let members = load_members(db, "true", Vec::new()).await?;
    let members_total = members.len() as u64;
    let members_uncovered = members
        .iter()
        .filter(|member| member.uncovered.is_some())
        .count() as u64;

    let mut sealed = Vec::new();
    for member in &members {
        if let Some(op_hex) = &member.sealed_op_id {
            if row_exists(db, "verified_op_log", "op_id_hex", op_hex).await? {
                sealed.push(member.clone());
            }
        }
    }
    let members_sealed = sealed.len() as u64;
    let members_pending = members_total
        .saturating_sub(members_sealed)
        .saturating_sub(members_uncovered);

    let readings: HashMap<String, CatalogReading> = catalog_readings(db)
        .await?
        .into_iter()
        .map(|row| (row.reading_id.clone(), row))
        .collect();
    let payloads: HashMap<String, CatalogPayload> = catalog_payloads(db)
        .await?
        .into_iter()
        .map(|row| (row.payload_id.clone(), row))
        .collect();

    let mut legacy = CanonicalExport::empty();
    let mut shadow = CanonicalExport::empty();
    for (member, resolved) in resolve_members(db, &sealed).await? {
        let prefix = format!("{}/{}", member.table.as_str(), member.row_id);
        match resolved {
            Ok(observation) => legacy_fields(&mut legacy.fields, &prefix, &observation),
            Err(reason) => {
                legacy.fields.insert(
                    format!("{prefix}/resolve"),
                    QuantizedValue::Unquantizable { raw_debug: reason },
                );
            }
        }
        match member.table {
            ObservationTable::Reading => {
                if let Some(row) = readings.get(&member.row_id) {
                    reading_fields(&mut shadow.fields, &prefix, row);
                }
            }
            ObservationTable::Payload => {
                if let Some(row) = payloads.get(&member.row_id) {
                    payload_fields(&mut shadow.fields, &prefix, row);
                }
            }
        }
    }

    Ok(ObservationParityReport {
        comparison: compare_projections(
            run_id,
            "observation-family",
            "observation_plane",
            "observation_catalog",
            &legacy,
            &shadow,
        ),
        members_total,
        members_sealed,
        members_uncovered,
        members_pending,
    })
}

type Fields = BTreeMap<String, QuantizedValue>;

fn text(fields: &mut Fields, prefix: &str, name: &str, value: &str) {
    fields.insert(
        format!("{prefix}/{name}"),
        QuantizedValue::Text(value.to_string()),
    );
}

fn integer(fields: &mut Fields, prefix: &str, name: &str, value: i64) {
    fields.insert(format!("{prefix}/{name}"), QuantizedValue::Integer(value));
}

fn bytes(fields: &mut Fields, prefix: &str, name: &str, value: Vec<u8>) {
    fields.insert(format!("{prefix}/{name}"), QuantizedValue::Bytes(value));
}

fn hex_bytes(text: &str) -> QuantizedValue {
    hex::decode(text).map_or_else(
        |_| QuantizedValue::Unquantizable {
            raw_debug: format!("not hex: {text}"),
        },
        QuantizedValue::Bytes,
    )
}

/// The hot row's compared fields (SPEC-10 §8 rule 4), anchor and source re-derived exactly as
/// the sealer derives them.
fn legacy_fields(fields: &mut Fields, prefix: &str, observation: &ResolvedObservation) {
    let anchor = node_anchor(observation.node_id()).map(|anchor| anchor.0.to_vec());
    match anchor {
        Ok(anchor) => bytes(fields, prefix, "anchor", anchor),
        Err(error) => {
            fields.insert(
                format!("{prefix}/anchor"),
                QuantizedValue::Unquantizable {
                    raw_debug: error.to_string(),
                },
            );
        }
    }
    match observation.source().encode_canonical() {
        Ok(source) => bytes(fields, prefix, "source", source),
        Err(error) => {
            fields.insert(
                format!("{prefix}/source"),
                QuantizedValue::Unquantizable {
                    raw_debug: error.to_string(),
                },
            );
        }
    }
    match observation {
        ResolvedObservation::Reading { hot, .. } => {
            text(fields, prefix, "anchor_node_id", &hot.node_id);
            text(fields, prefix, "metric", &hot.metric);
            fields.insert(
                format!("{prefix}/value"),
                QuantizedValue::f64_bit_pattern(hot.value),
            );
            text(fields, prefix, "units", &hot.units);
            text(fields, prefix, "recorded_at", &hot.recorded_at);
            integer(fields, prefix, "recorded_at_ms", hot.recorded_at_ms);
            integer(fields, prefix, "hlc_wall_ms", hot.hlc_timestamp >> 16);
            integer(fields, prefix, "hlc_counter", hot.hlc_timestamp & 0xFFFF);
            if let Some(payload_id) = &hot.payload_id {
                text(fields, prefix, "payload_id", payload_id);
            }
        }
        ResolvedObservation::Payload {
            hot,
            asset_content_hashes,
            ..
        } => {
            text(fields, prefix, "anchor_node_id", &hot.node_id);
            text(fields, prefix, "format_id", &hot.format_id);
            text(fields, prefix, "recorded_at", &hot.recorded_at);
            integer(fields, prefix, "recorded_at_ms", hot.recorded_at_ms);
            integer(fields, prefix, "hlc_wall_ms", hot.hlc_timestamp >> 16);
            integer(fields, prefix, "hlc_counter", hot.hlc_timestamp & 0xFFFF);
            text(fields, prefix, "asset_ids", &hot.asset_ids.join("\n"));
            bytes(
                fields,
                prefix,
                "asset_content_hashes",
                asset_content_hashes.concat(),
            );
        }
    }
}

fn reading_fields(fields: &mut Fields, prefix: &str, row: &CatalogReading) {
    fields.insert(format!("{prefix}/anchor"), hex_bytes(&row.anchor));
    fields.insert(format!("{prefix}/source"), hex_bytes(&row.source));
    text(fields, prefix, "anchor_node_id", &row.anchor_node_id);
    text(fields, prefix, "metric", &row.metric);
    fields.insert(
        format!("{prefix}/value"),
        u64::from_str_radix(&row.value_bits, 16).map_or_else(
            |_| QuantizedValue::Unquantizable {
                raw_debug: format!("value_bits {}", row.value_bits),
            },
            |bits| QuantizedValue::f64_bit_pattern(f64::from_bits(bits)),
        ),
    );
    text(fields, prefix, "units", &row.units);
    text(fields, prefix, "recorded_at", &row.recorded_at);
    integer(fields, prefix, "recorded_at_ms", row.recorded_at_ms);
    integer(fields, prefix, "hlc_wall_ms", row.hlc_wall_ms);
    integer(fields, prefix, "hlc_counter", row.hlc_counter);
    if let Some(payload_id) = &row.payload_id {
        text(fields, prefix, "payload_id", payload_id);
    }
}

fn payload_fields(fields: &mut Fields, prefix: &str, row: &CatalogPayload) {
    fields.insert(format!("{prefix}/anchor"), hex_bytes(&row.anchor));
    fields.insert(format!("{prefix}/source"), hex_bytes(&row.source));
    text(fields, prefix, "anchor_node_id", &row.anchor_node_id);
    text(fields, prefix, "format_id", &row.format_id);
    text(fields, prefix, "recorded_at", &row.recorded_at);
    integer(fields, prefix, "recorded_at_ms", row.recorded_at_ms);
    integer(fields, prefix, "hlc_wall_ms", row.hlc_wall_ms);
    integer(fields, prefix, "hlc_counter", row.hlc_counter);
    text(fields, prefix, "asset_ids", &row.asset_ids.join("\n"));
    fields.insert(
        format!("{prefix}/asset_content_hashes"),
        hex_bytes(&row.asset_content_hashes.concat()),
    );
}
