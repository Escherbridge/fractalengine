//! Per-table replication semantics (M2/F5) — see fe-database/src/AGENTS.md
//! §iot-readings and fe-sync/src/AGENTS.md §inbound-apply.
//!
//! The canonical enum lives here, at the data layer, because that is where the
//! inbound apply handler (the DB thread's enforcement point) dispatches on it;
//! fe-sync re-exports it (`fe_sync::replication::ReplicationMode`) so the
//! replication side speaks the same vocabulary without a dependency cycle
//! (fe-sync depends on fe-database, never the reverse — the same split used
//! for `RoleLevel`, see fe-database/src/AGENTS.md §rbac-policy).

/// How a table's rows replicate.
///
/// The two modes are not a tuning knob: they encode different CRDT contracts,
/// and choosing the wrong one silently corrupts a table's convergence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationMode {
    /// Mutable scene / hierarchy rows (`verse`, `fractal`, `petal`, `node`):
    /// insert-or-merge on the row key, tombstone-aware (N-4 — a delete never
    /// loses to a concurrent live write).
    Row,
    /// Append-only timeseries rows (`iot_reading`): **union CRDT** keyed by
    /// `reading_id`. A reading is an immutable fact, so re-delivery is
    /// idempotent, an existing row is never overwritten, and no entry — not
    /// even an empty `Doc::del` tombstone — ever deletes one.
    Timeseries,
}

impl ReplicationMode {
    /// The mode `table` replicates under.
    ///
    /// Unknown tables are [`ReplicationMode::Row`]; the inbound handler maps
    /// that to `NotApplicable` for tables it has no row semantics for, so a
    /// new table stays inert until it is deliberately given a mode.
    pub fn for_table(table: &str) -> Self {
        match table {
            "iot_reading" => ReplicationMode::Timeseries,
            _ => ReplicationMode::Row,
        }
    }

    /// Stable lowercase label (logs, ledger rows, diagnostics).
    pub fn as_str(&self) -> &'static str {
        match self {
            ReplicationMode::Row => "row",
            ReplicationMode::Timeseries => "timeseries",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iot_reading_replicates_as_timeseries() {
        assert_eq!(
            ReplicationMode::for_table("iot_reading"),
            ReplicationMode::Timeseries
        );
    }

    #[test]
    fn hierarchy_and_scene_tables_replicate_as_rows() {
        for table in ["verse", "fractal", "petal", "node"] {
            assert_eq!(
                ReplicationMode::for_table(table),
                ReplicationMode::Row,
                "{table} must replicate as a row table"
            );
        }
    }

    #[test]
    fn unknown_tables_default_to_row_mode() {
        for table in ["", "asset", "op_log", "__shards", "node_log"] {
            assert_eq!(ReplicationMode::for_table(table), ReplicationMode::Row);
        }
    }

    #[test]
    fn mode_labels_are_stable() {
        assert_eq!(ReplicationMode::Row.as_str(), "row");
        assert_eq!(ReplicationMode::Timeseries.as_str(), "timeseries");
    }
}
