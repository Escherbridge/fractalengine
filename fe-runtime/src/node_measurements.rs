//! Bounded persisted measurement projections for a selected object.

use serde::{Deserialize, Serialize};

pub const POINT_LIMIT: usize = 1024;
pub const SERIES_LIMIT: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementSeries {
    pub metric: String,
    pub units: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasurementPoint {
    pub reading_id: String,
    pub recorded_at: String,
    pub recorded_at_ms: i64,
    pub value: f64,
}

#[derive(Debug, Clone)]
pub struct NodeMeasurements {
    pub node_id: String,
    pub petal_id: String,
    pub series: Option<MeasurementSeries>,
    pub available_series: Vec<MeasurementSeries>,
    pub series_truncated: bool,
    pub points: Vec<MeasurementPoint>,
    pub truncated: bool,
    pub limit: usize,
}
