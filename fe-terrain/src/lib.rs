pub mod api_ext;
pub mod config;
pub mod extension;
pub mod format;
pub mod gpx;
pub mod iot;
pub mod lod_ring;
pub mod mesh;
pub mod petal_binding;
pub mod projection;
pub mod ruler;
pub mod scale;
pub mod sculpt;
pub mod simplify;
pub mod splat;
pub mod terrain_proposal;
pub mod tiles;

#[cfg(feature = "render")]
pub mod layers;

#[cfg(feature = "render")]
pub mod ruler_plugin;

#[cfg(feature = "render")]
pub mod terrain_plugin;

use serde::{Deserialize, Serialize};

/// A command to create or update a node in the database.
/// Mirrors DbCommand patterns but stays independent of fe-database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbCommand {
    pub petal_id: String,
    pub name: String,
    pub position: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,
}

/// A node snapshot for export back to GPX.
/// Mirrors fe-format::ExportNode but only carries terrain-relevant fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportNode {
    pub node_id: String,
    pub name: String,
    pub position: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ExportNode>,
}
