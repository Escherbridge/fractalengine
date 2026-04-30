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
