//! Bounded scene geometry for terrain impact reports; see src/AGENTS.md.

use bevy::prelude::Resource;

pub const SCENE_OBJECT_LIMIT: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectGeometry {
    AxisAlignedSolidBox,
    ConservativeWorldBounds,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneObjectBounds {
    pub node_id: String,
    pub name: String,
    pub bounds: Option<[[f32; 3]; 2]>,
    pub geometry: ObjectGeometry,
    pub selectable: bool,
    /// Exact bounded JSON for the stored engineering-material property.
    pub material_json: Option<String>,
    pub material_loaded: bool,
    pub material_truncated: bool,
}

#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct SceneAnalysisSnapshot {
    pub petal_id: Option<String>,
    pub revision: u64,
    pub objects: Vec<SceneObjectBounds>,
    pub omitted_objects: usize,
    pub excluded_non_geometric: usize,
}

#[derive(Debug, Default, Clone)]
pub struct ObjectImpactReport {
    pub scene_revision: u64,
    pub available: bool,
    pub omitted_objects: usize,
    pub excluded_non_geometric: usize,
    pub checked_objects: usize,
    pub objects: Vec<ObjectImpact>,
}

#[derive(Debug, Clone)]
pub struct ObjectImpact {
    pub object: SceneObjectBounds,
    /// Sampled changed-space overlap; bounds are not a mesh-solid calculation.
    pub overlap_m3: Option<f64>,
}
