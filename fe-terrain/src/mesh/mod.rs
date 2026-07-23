// Pure elevation-grid helpers (no bevy) are always compiled + tested.
pub mod interp;
pub mod skirt;

#[cfg(feature = "render")]
pub mod marker;
#[cfg(feature = "render")]
pub mod terrain;
#[cfg(feature = "render")]
pub mod track;

// pen_curve_tool_20260722 (Phase 2): pure anchor-bezier flattener for the ribbon +
// pick polyline. Render-gated (its only callers are the render/pick paths).
#[cfg(feature = "render")]
pub mod curve;
