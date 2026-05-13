use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateVerseRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateFractalRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatePetalRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateNodeRequest {
    /// petal_id is optional in the body when using the hierarchical path
    /// (where it comes from the URL). Required for the legacy `/api/v1/nodes` path.
    pub petal_id: Option<String>,
    pub name: String,
    pub position: Option<[f32; 3]>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTransformRequest {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

#[derive(Debug, Deserialize)]
pub struct SetPropertyRequest {
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertiesDto {
    pub node_id: String,
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertySetDto {
    pub node_id: String,
    pub key: String,
}

// ---------------------------------------------------------------------------
// Response envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Field definition (property schema) DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateFieldDefRequest {
    pub scope: String,
    #[serde(default = "default_entity_type")]
    pub entity_type: String,
    pub key: String,
    pub value_type: String,
    pub default_val: Option<serde_json::Value>,
}

fn default_entity_type() -> String {
    "node".to_string()
}

#[derive(Debug, Deserialize)]
pub struct UpdateFieldDefRequest {
    pub value_type: String,
    pub default_val: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldDefDto {
    pub field_def_id: String,
    pub scope: String,
    pub entity_type: String,
    pub key: String,
    pub value_type: String,
    pub default_val: Option<serde_json::Value>,
    pub created_by: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Response DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct VerseDto {
    pub id: String,
    pub name: String,
    pub fractals: Vec<FractalDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FractalDto {
    pub id: String,
    pub name: String,
    pub petals: Vec<PetalDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PetalDto {
    pub id: String,
    pub name: String,
    pub nodes: Vec<NodeDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeDto {
    pub id: String,
    pub name: String,
    pub petal_id: String,
    pub position: [f32; 3],
    pub has_asset: bool,
    pub asset_path: Option<String>,
    pub webpage_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransformDto {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedEntityDto {
    pub id: String,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a slice of internal hierarchy data snapshots into API DTOs.
pub fn hierarchy_to_dto(
    data: &[fe_runtime::messages::VerseHierarchyData],
) -> Vec<VerseDto> {
    data.iter()
        .map(|v| VerseDto {
            id: v.id.clone(),
            name: v.name.clone(),
            fractals: v
                .fractals
                .iter()
                .map(|f| FractalDto {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    petals: f
                        .petals
                        .iter()
                        .map(|p| PetalDto {
                            id: p.id.clone(),
                            name: p.name.clone(),
                            nodes: p.nodes.iter().map(node_to_dto).collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

/// Convert a single internal node into an API DTO.
pub fn node_to_dto(node: &fe_runtime::messages::NodeHierarchyData) -> NodeDto {
    NodeDto {
        id: node.id.clone(),
        name: node.name.clone(),
        petal_id: node.petal_id.clone(),
        position: node.position,
        has_asset: node.has_asset,
        asset_path: node.asset_path.clone(),
        webpage_url: node.webpage_url.clone(),
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Validate that a string looks like a ULID (26 alphanumeric Crockford base32 chars).
pub fn is_valid_ulid(s: &str) -> bool {
    s.len() == 26 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Validate a scope string matches the expected pattern.
pub fn is_valid_scope(s: &str) -> bool {
    // Must start with VERSE# and optionally have -FRACTAL# and -PETAL# segments
    fe_database::parse_scope(s).is_ok()
}

/// Validate a role string is one of the known values.
pub fn is_valid_role(s: &str) -> bool {
    matches!(s, "viewer" | "editor" | "manager" | "owner" | "none")
}

// ---------------------------------------------------------------------------
// Query endpoint DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub sql: String,
    #[serde(default)]
    pub vars: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct QueryResultDto {
    pub data: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct AnalyticsQueryRequest {
    pub sql: String,
    /// Optional petal_id to scope the query to a single petal's nodes.
    pub petal_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Waypoint / Track DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateWaypointRequest {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub ele: f64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MoveWaypointRequest {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub ele: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElevationPointDto {
    pub distance_m: f64,
    pub elevation_m: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackStatsDto {
    pub total_distance_m: f64,
    pub min_elevation_m: f64,
    pub max_elevation_m: f64,
    pub avg_speed_kmh: Option<f64>,
    pub duration_seconds: Option<f64>,
}
