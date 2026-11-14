//! Latest-per-metric live readings summary for one petal; see fe-database AGENTS.md §handlers petal_readings.

use serde::{Deserialize, Serialize};

/// Maximum rows in one summary.
pub const ROW_LIMIT: usize = 256;
/// Default look-back window for the count column.
pub const WINDOW_MS: i64 = 60_000;

/// Newest reading of one (node, metric) plus how many arrived in the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatestReading {
    pub node_id: String,
    pub metric: String,
    pub units: String,
    pub value: f64,
    pub recorded_at_ms: i64,
    pub count_window: u32,
}

/// Rows sorted by node then metric; `truncated` when rows or source reads were capped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PetalReadingsSummary {
    pub petal_id: String,
    pub as_of_ms: i64,
    pub rows: Vec<LatestReading>,
    pub truncated: bool,
}
