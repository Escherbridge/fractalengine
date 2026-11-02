//! Local Viewer-authorized material projections; see src/AGENTS.md.

pub const MATERIAL_BATCH_LIMIT: usize = 64;
pub const MATERIAL_JSON_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMaterial {
    pub node_id: String,
    pub material_json: Option<String>,
    pub truncated: bool,
    pub has_primitive: bool,
}

#[derive(Debug, Clone)]
pub struct SceneMaterials {
    pub petal_id: String,
    pub scope: String,
    pub values: Vec<NodeMaterial>,
}
