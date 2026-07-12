//! Terrain splat view: synthesized 3D splats as an alternative to the mesh
//! renderer. Pure synthesis + view-mode parsing are always compiled; the Bevy
//! plugin is render-gated. See `src/splat/AGENTS.md`.

pub mod interpolate;
pub mod synth;
pub mod view_mode;

#[cfg(feature = "render")]
pub mod render;

pub use interpolate::{
    augment_splat_buffer_coverage, augment_splat_buffer_if_needed, augment_splat_buffer_within,
    splat_needs_interpolation, TileFootprint,
};
pub use synth::{synthesize_splats, SplatBuffer, TileSatellite};
pub use view_mode::{view_mode_from_terrain_json, TerrainViewMode};

#[cfg(feature = "render")]
pub use render::{
    bake_splat_mesh, make_soft_disc_image, SplatAssets, SplatChunk, SplatConfig, SplatPlugin,
    TerrainViewModeMsg,
};
