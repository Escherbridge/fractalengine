//! Section-profile request/reply seam (plain data; see `src/AGENTS.md` §section-profile).

use bevy::prelude::*;

use crate::terrain_overlay::VolumeUnavailable;

/// Ask for an existing-vs-proposed section along `a -> b` (world XZ) on the active petal.
#[derive(Message, Debug, Clone)]
pub struct SectionProfileRequest {
    /// Correlation id matching the reply to the request.
    pub correlation_id: String,
    pub petal_id: String,
    /// Section start (world x, z).
    pub a: [f32; 2],
    /// Section end (world x, z).
    pub b: [f32; 2],
}

/// Reply to a [`SectionProfileRequest`]; always sent, `unavailable` explains an empty one.
#[derive(Message, Debug, Clone)]
pub struct SectionProfileResult {
    pub correlation_id: String,
    pub petal_id: String,
    /// Stations as `[distance, base_y, proposed_y]` in world units.
    pub samples: Vec<[f32; 3]>,
    /// Section length (meters when `scaled`, else world units).
    pub length: f64,
    /// Spacing between stations in world units.
    pub step: f32,
    /// Cut area (m2 when `scaled`, else world units squared).
    pub cut_area: f64,
    /// Fill area (m2 when `scaled`, else world units squared).
    pub fill_area: f64,
    /// `false` = no usable map scale: figures are world units.
    pub scaled: bool,
    /// `Some` when the section could not be measured.
    pub unavailable: Option<VolumeUnavailable>,
}
