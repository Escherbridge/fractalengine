//! Terrain splat view: synthesized 3D splats as an alternative to the mesh
//! renderer. Pure synthesis + view-mode parsing are always compiled; the Bevy
//! plugin is render-gated. See `src/splat/AGENTS.md`.

pub mod bake;
pub mod format;
pub mod synth;
pub mod view_mode;

#[cfg(feature = "render")]
pub mod render;

pub use bake::{bake_splat_coverage, bake_splat_coverage_within, TileFootprint};
pub use format::BakedSplatBuffer;
pub use synth::{synthesize_splats, SplatBuffer, TileSatellite};
pub use view_mode::{view_mode_from_terrain_json, TerrainViewMode};

#[cfg(feature = "render")]
pub use render::{
    bake_splat_mesh, make_soft_disc_image, SplatAssets, SplatChunk, SplatConfig, SplatPlugin,
    TerrainViewModeMsg,
};
