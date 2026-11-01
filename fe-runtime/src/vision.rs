//! Vision wire contract — capture/read-camera request+reply types shared by
//! the API and Bevy threads. See src/AGENTS.md §vision-bridge.

use serde::{Deserialize, Serialize};

use crate::bridge::{bounded_bridge, BridgeReceiver, BridgeReply, BridgeRequest, BridgeSender};

// ---------------------------------------------------------------------------
// Bridge capacity + channel
// ---------------------------------------------------------------------------

/// Bounded depth of the vision request channel; a full channel is a busy
/// refusal, never a blocked async worker (mirrors `screenshot_bridge`).
pub const VISION_BRIDGE_CAPACITY: usize = 4;

pub type VisionRequestSender = BridgeSender<VisionOp, VisionResult>;
pub type VisionRequestReceiver = BridgeReceiver<VisionOp, VisionResult>;

/// A fresh bounded request channel (API holds the sender, the Bevy host the receiver).
pub fn vision_bridge() -> (VisionRequestSender, VisionRequestReceiver) {
    bounded_bridge(VISION_BRIDGE_CAPACITY)
}

// ---------------------------------------------------------------------------
// Static validation constants (spec agent_vision_20261031 §5.2)
// ---------------------------------------------------------------------------

pub const SIZE_MIN: u32 = 64;
pub const SIZE_MAX: u32 = 4096;
pub const SETTLE_MAX: u32 = 600;
pub const DEFAULT_SETTLE: u32 = 30;
pub const DEFAULT_WIDTH: u32 = 1280;
pub const DEFAULT_HEIGHT: u32 = 720;
pub const FOV_MIN_DEG: f64 = 1.0;
pub const FOV_MAX_DEG: f64 = 170.0;
/// The main camera's FOV (spec evidence §3: `FRAC_PI_4` = 45°); used when a
/// `camera` pose omits `fov_deg`.
pub const DEFAULT_FOV_DEG: f64 = 45.0;
/// Web-Mercator's latitude limit — beyond this a slippy tile has no projection.
pub const MERCATOR_LAT_MAX: f64 = 85.0511;
pub const TILE_Z_MAX: u8 = 24;
pub const ALTITUDE_MIN_M: f64 = -500.0;
pub const ALTITUDE_MAX_M: f64 = 1_000_000.0;
pub const DEFAULT_ALTITUDE_M: f64 = 300.0;
pub const DEFAULT_HEADING_DEG: f64 = 0.0;
pub const DEFAULT_GEO_PITCH_DEG: f64 = -45.0;
pub const GEO_PITCH_MIN_DEG: f64 = -90.0;
pub const GEO_PITCH_MAX_DEG: f64 = 89.0;
pub const ORBIT_DISTANCE_MAX: f64 = 1.0e7;
pub const ORBIT_PITCH_MAX_DEG: f64 = 89.0;

// ---------------------------------------------------------------------------
// Wire types (the POST body / MCP `screenshot` arguments; spec §5.2)
// ---------------------------------------------------------------------------

/// `window`: full UI, main camera moves and restores. `scene`: off-screen
/// render-to-texture, no UI, the user's view is untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    #[default]
    Window,
    Scene,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraPose {
    pub position: [f64; 3],
    pub target: [f64; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov_deg: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrbitPose {
    pub focus: [f64; 3],
    pub yaw_deg: f64,
    pub pitch_deg: f64,
    pub distance: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoPose {
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_deg: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TilePose {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// Exactly one of `node_id`/`petal_id` must be set — checked by `validate`,
/// not serde, so a malformed focus is a typed refusal, not a parse error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FocusPose {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub petal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigate: Option<bool>,
}

/// The wire `pose` object: exactly one of these five keys. Deliberately
/// permissive here — `validate()` is the only place cardinality is enforced,
/// so every bad-pose shape becomes a typed `VisionRefusal`, never a raw serde
/// parse error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PoseSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraPose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orbit: Option<OrbitPose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoPose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<TilePose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusPose>,
}

/// `POST /api/v1/debug/screenshot` request body (and the MCP `screenshot`
/// tool's arguments). `validate()` is the ONLY constructor of
/// `ValidCaptureSpec` — Bevy-side code never re-checks finiteness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CaptureSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<CaptureMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<PoseSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settle_frames: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<bool>,
    /// Include the app's UI panels in the image (default `true` = window
    /// mode); `false` renders only the 3D scene off-screen without moving the
    /// user's view (= scene mode). Contradicting an explicit `mode` is the
    /// typed 400 `include_ui_conflicts_with_mode` (UI-ADDENDUM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_ui: Option<bool>,
}

/// The one plain-words description of `include_ui` (MCP schema, REST docs).
pub const INCLUDE_UI_DESCRIPTION: &str = "include_ui (default true): include the app's UI panels \
     in the image; false renders only the 3D scene off-screen without moving the user's view";

/// Stable code of the uniform scope denial: an out-of-scope target and a
/// nonexistent one answer identically (fe-api AGENTS.md §existence-oracle).
pub const DENIED_CODE: &str = "not_found_or_denied";
/// Message of the uniform scope denial — byte-identical to fe-api's
/// `rest::NOT_FOUND_OR_DENIED` (asserted there).
pub const DENIED_MESSAGE: &str = "not found or not permitted";

/// A typed 400: `code` is the stable machine string (spec §5.2's table),
/// `field` is the dotted wire path when the check is field-scoped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionRefusal {
    pub code: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<&'static str>,
    pub message: String,
}

impl VisionRefusal {
    fn new(code: &'static str, field: Option<&'static str>, message: impl Into<String>) -> Self {
        Self {
            code,
            field,
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validated spec (the newtype `validate()` alone constructs)
// ---------------------------------------------------------------------------

/// Which node/petal a `focus` pose frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    Node(String),
    Petal(String),
}

/// A pose spec whose cardinality, ranges and finiteness have all been
/// checked; defaults (up, fov, altitude, heading, geo pitch, navigate) are
/// already resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidPoseSpec {
    Camera {
        position: [f64; 3],
        target: [f64; 3],
        up: [f64; 3],
        fov_deg: f64,
    },
    Orbit {
        focus: [f64; 3],
        yaw_deg: f64,
        pitch_deg: f64,
        distance: f64,
    },
    Geo {
        lat: f64,
        lon: f64,
        altitude_m: f64,
        heading_deg: f64,
        pitch_deg: f64,
    },
    Tile {
        z: u8,
        x: u32,
        y: u32,
    },
    Focus {
        target: FocusTarget,
        navigate: bool,
    },
}

/// A `CaptureSpec` that has passed every static check in spec §5.2's table.
/// No public constructor besides [`CaptureSpec::validate`] — this is the
/// "validate once" newtype (plan V1 doctrine): a NaN can never cross the
/// bridge because nothing else can build one of these.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidCaptureSpec {
    mode: CaptureMode,
    pose: Option<ValidPoseSpec>,
    width: Option<u32>,
    height: Option<u32>,
    settle_frames: u32,
    restore: Option<bool>,
}

impl ValidCaptureSpec {
    pub fn mode(&self) -> CaptureMode {
        self.mode
    }
    pub fn pose(&self) -> Option<&ValidPoseSpec> {
        self.pose.as_ref()
    }
    /// `Some` only in scene mode (window mode has no output-size parameter).
    pub fn width(&self) -> Option<u32> {
        self.width
    }
    pub fn height(&self) -> Option<u32> {
        self.height
    }
    pub fn settle_frames(&self) -> u32 {
        self.settle_frames
    }
    /// `Some` only in window mode (scene mode has nothing to restore).
    pub fn restore(&self) -> Option<bool> {
        self.restore
    }
    /// `true` iff the image includes the app UI (window mode).
    pub fn include_ui(&self) -> bool {
        self.mode == CaptureMode::Window
    }
}

/// `mode` from the optional `mode`/`include_ui` pair (row 2b).
fn resolve_mode(
    mode: Option<CaptureMode>,
    include_ui: Option<bool>,
) -> Result<CaptureMode, VisionRefusal> {
    let from_ui = include_ui.map(|ui| if ui { CaptureMode::Window } else { CaptureMode::Scene });
    match (mode, from_ui) {
        (Some(explicit), Some(implied)) if explicit != implied => Err(VisionRefusal::new(
            "include_ui_conflicts_with_mode",
            Some("include_ui"),
            "`include_ui` contradicts `mode` (include_ui true = window, false = scene) — \
             pass one of them, or matching values",
        )),
        (Some(mode), _) | (None, Some(mode)) => Ok(mode),
        (None, None) => Ok(CaptureMode::default()),
    }
}

impl CaptureSpec {
    /// The ONLY constructor of [`ValidCaptureSpec`]. Runs every check in
    /// spec §5.2's table **in table order** — the first failure wins, and
    /// that order is part of the documented contract (plan V1 step 2): an
    /// unauthenticated/un-opted caller never learns the parameter grammar
    /// from which check fires, but a given malformed body always refuses
    /// the same way.
    pub fn validate(self) -> Result<ValidCaptureSpec, VisionRefusal> {
        // Row 1: non_finite — every float present anywhere, regardless of
        // whether the pose is otherwise well-formed.
        check_all_finite(&self.pose)?;

        // Row 2: pose_must_have_exactly_one_kind (incl. focus's own
        // exactly-one-of-two-ids rule).
        let pose = check_pose_cardinality(self.pose)?;

        // Row 2b: include_ui_conflicts_with_mode — `include_ui` picks the
        // mode; an explicit contradicting `mode` fails closed (no precedence).
        let mode = resolve_mode(self.mode, self.include_ui)?;

        // Row 3: size_out_of_range.
        if let Some(w) = self.width {
            check_size_range(w, "width")?;
        }
        if let Some(h) = self.height {
            check_size_range(h, "height")?;
        }

        // Row 4: size_requires_scene_mode.
        if mode == CaptureMode::Window && (self.width.is_some() || self.height.is_some()) {
            return Err(VisionRefusal::new(
                "size_requires_scene_mode",
                None,
                "`width`/`height` are only valid in scene mode (`include_ui: false`) — a \
                 window capture (`include_ui: true`) is always the window's own size",
            ));
        }

        // Row 5: option_not_valid_in_mode (restore, then navigate).
        if mode == CaptureMode::Scene && self.restore.is_some() {
            return Err(VisionRefusal::new(
                "option_not_valid_in_mode",
                Some("restore"),
                "`restore` is only valid in window mode",
            ));
        }
        if mode == CaptureMode::Scene {
            if let Some(ValidPoseSpec::Focus { navigate: true, .. }) = &pose {
                return Err(VisionRefusal::new(
                    "option_not_valid_in_mode",
                    Some("pose.focus.navigate"),
                    "`navigate` is only valid in window mode",
                ));
            }
        }

        // Row 6: settle_out_of_range (and the mode/pose-dependent default).
        let settle_frames = match self.settle_frames {
            Some(s) if s > SETTLE_MAX => {
                return Err(VisionRefusal::new(
                    "settle_out_of_range",
                    Some("settle_frames"),
                    format!("`settle_frames` must be <= {SETTLE_MAX}, got {s}"),
                ))
            }
            Some(s) => s,
            None if mode == CaptureMode::Window && pose.is_none() => 0,
            None => DEFAULT_SETTLE,
        };

        // Rows 7-11: fov_out_of_range, orbit_out_of_range, degenerate_camera,
        // geo_out_of_range, tile_out_of_range — only the active pose kind's
        // rows can fire.
        check_pose_ranges(&pose)?;

        let (width, height) = match mode {
            CaptureMode::Scene => (
                Some(self.width.unwrap_or(DEFAULT_WIDTH)),
                Some(self.height.unwrap_or(DEFAULT_HEIGHT)),
            ),
            CaptureMode::Window => (None, None),
        };
        let restore = match mode {
            CaptureMode::Window => Some(self.restore.unwrap_or(true)),
            CaptureMode::Scene => None,
        };

        Ok(ValidCaptureSpec {
            mode,
            pose,
            width,
            height,
            settle_frames,
            restore,
        })
    }
}

fn finite(v: f64, field: &'static str) -> Result<(), VisionRefusal> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(VisionRefusal::new(
            "non_finite",
            Some(field),
            format!("`{field}` must be a finite number, got {v}"),
        ))
    }
}

fn finite_triplet(v: [f64; 3], fields: [&'static str; 3]) -> Result<(), VisionRefusal> {
    finite(v[0], fields[0])?;
    finite(v[1], fields[1])?;
    finite(v[2], fields[2])?;
    Ok(())
}

fn check_all_finite(pose: &Option<PoseSpec>) -> Result<(), VisionRefusal> {
    let Some(pose) = pose else {
        return Ok(());
    };
    if let Some(camera) = &pose.camera {
        finite_triplet(
            camera.position,
            [
                "pose.camera.position.x",
                "pose.camera.position.y",
                "pose.camera.position.z",
            ],
        )?;
        finite_triplet(
            camera.target,
            [
                "pose.camera.target.x",
                "pose.camera.target.y",
                "pose.camera.target.z",
            ],
        )?;
        if let Some(up) = camera.up {
            finite_triplet(up, ["pose.camera.up.x", "pose.camera.up.y", "pose.camera.up.z"])?;
        }
        if let Some(fov_deg) = camera.fov_deg {
            finite(fov_deg, "pose.camera.fov_deg")?;
        }
    }
    if let Some(orbit) = &pose.orbit {
        finite_triplet(
            orbit.focus,
            ["pose.orbit.focus.x", "pose.orbit.focus.y", "pose.orbit.focus.z"],
        )?;
        finite(orbit.yaw_deg, "pose.orbit.yaw_deg")?;
        finite(orbit.pitch_deg, "pose.orbit.pitch_deg")?;
        finite(orbit.distance, "pose.orbit.distance")?;
    }
    if let Some(geo) = &pose.geo {
        finite(geo.lat, "pose.geo.lat")?;
        finite(geo.lon, "pose.geo.lon")?;
        if let Some(v) = geo.altitude_m {
            finite(v, "pose.geo.altitude_m")?;
        }
        if let Some(v) = geo.heading_deg {
            finite(v, "pose.geo.heading_deg")?;
        }
        if let Some(v) = geo.pitch_deg {
            finite(v, "pose.geo.pitch_deg")?;
        }
    }
    // tile has no float fields; focus has no float fields.
    Ok(())
}

fn check_pose_cardinality(pose: Option<PoseSpec>) -> Result<Option<ValidPoseSpec>, VisionRefusal> {
    let Some(pose) = pose else {
        return Ok(None);
    };
    let count = [
        pose.camera.is_some(),
        pose.orbit.is_some(),
        pose.geo.is_some(),
        pose.tile.is_some(),
        pose.focus.is_some(),
    ]
    .into_iter()
    .filter(|k| *k)
    .count();
    if count != 1 {
        return Err(VisionRefusal::new(
            "pose_must_have_exactly_one_kind",
            Some("pose"),
            format!(
                "`pose` must have exactly one of camera/orbit/geo/tile/focus, found {count}"
            ),
        ));
    }
    if let Some(camera) = pose.camera {
        return Ok(Some(ValidPoseSpec::Camera {
            position: camera.position,
            target: camera.target,
            up: camera.up.unwrap_or([0.0, 1.0, 0.0]),
            fov_deg: camera.fov_deg.unwrap_or(DEFAULT_FOV_DEG),
        }));
    }
    if let Some(orbit) = pose.orbit {
        return Ok(Some(ValidPoseSpec::Orbit {
            focus: orbit.focus,
            yaw_deg: orbit.yaw_deg,
            pitch_deg: orbit.pitch_deg,
            distance: orbit.distance,
        }));
    }
    if let Some(geo) = pose.geo {
        return Ok(Some(ValidPoseSpec::Geo {
            lat: geo.lat,
            lon: geo.lon,
            altitude_m: geo.altitude_m.unwrap_or(DEFAULT_ALTITUDE_M),
            heading_deg: geo.heading_deg.unwrap_or(DEFAULT_HEADING_DEG),
            pitch_deg: geo.pitch_deg.unwrap_or(DEFAULT_GEO_PITCH_DEG),
        }));
    }
    if let Some(tile) = pose.tile {
        return Ok(Some(ValidPoseSpec::Tile {
            z: tile.z,
            x: tile.x,
            y: tile.y,
        }));
    }
    let focus = pose
        .focus
        .expect("count == 1 and the first four kinds were None, so focus must be Some");
    let target = match (focus.node_id, focus.petal_id) {
        (Some(node_id), None) => FocusTarget::Node(node_id),
        (None, Some(petal_id)) => FocusTarget::Petal(petal_id),
        _ => {
            return Err(VisionRefusal::new(
                "pose_must_have_exactly_one_kind",
                Some("pose.focus"),
                "`focus` must have exactly one of `node_id`/`petal_id`",
            ))
        }
    };
    Ok(Some(ValidPoseSpec::Focus {
        target,
        navigate: focus.navigate.unwrap_or(false),
    }))
}

fn check_size_range(v: u32, field: &'static str) -> Result<(), VisionRefusal> {
    if (SIZE_MIN..=SIZE_MAX).contains(&v) {
        Ok(())
    } else {
        Err(VisionRefusal::new(
            "size_out_of_range",
            Some(field),
            format!("`{field}` must be within {SIZE_MIN}..={SIZE_MAX}, got {v}"),
        ))
    }
}

fn check_pose_ranges(pose: &Option<ValidPoseSpec>) -> Result<(), VisionRefusal> {
    match pose {
        Some(ValidPoseSpec::Camera {
            position,
            target,
            up,
            fov_deg,
        }) => {
            if !(FOV_MIN_DEG..=FOV_MAX_DEG).contains(fov_deg) {
                return Err(VisionRefusal::new(
                    "fov_out_of_range",
                    Some("pose.camera.fov_deg"),
                    format!("`fov_deg` must be within {FOV_MIN_DEG}..={FOV_MAX_DEG}, got {fov_deg}"),
                ));
            }
            check_degenerate_camera(*position, *target, *up)?;
        }
        Some(ValidPoseSpec::Orbit {
            pitch_deg, distance, ..
        }) => {
            if *distance <= 0.0 || *distance > ORBIT_DISTANCE_MAX {
                return Err(VisionRefusal::new(
                    "orbit_out_of_range",
                    Some("pose.orbit.distance"),
                    format!("`distance` must be within (0, {ORBIT_DISTANCE_MAX}], got {distance}"),
                ));
            }
            if pitch_deg.abs() > ORBIT_PITCH_MAX_DEG {
                return Err(VisionRefusal::new(
                    "orbit_out_of_range",
                    Some("pose.orbit.pitch_deg"),
                    format!("`pitch_deg` must be within ±{ORBIT_PITCH_MAX_DEG}, got {pitch_deg}"),
                ));
            }
        }
        Some(ValidPoseSpec::Geo {
            lat,
            lon,
            altitude_m,
            heading_deg,
            pitch_deg,
        }) => {
            if lat.abs() > MERCATOR_LAT_MAX {
                return Err(VisionRefusal::new(
                    "geo_out_of_range",
                    Some("pose.geo.lat"),
                    format!("`lat` must be within ±{MERCATOR_LAT_MAX}, got {lat}"),
                ));
            }
            if lon.abs() > 180.0 {
                return Err(VisionRefusal::new(
                    "geo_out_of_range",
                    Some("pose.geo.lon"),
                    format!("`lon` must be within ±180, got {lon}"),
                ));
            }
            if !(0.0..360.0).contains(heading_deg) {
                return Err(VisionRefusal::new(
                    "geo_out_of_range",
                    Some("pose.geo.heading_deg"),
                    format!("`heading_deg` must be within 0..360, got {heading_deg}"),
                ));
            }
            if !(GEO_PITCH_MIN_DEG..=GEO_PITCH_MAX_DEG).contains(pitch_deg) {
                return Err(VisionRefusal::new(
                    "geo_out_of_range",
                    Some("pose.geo.pitch_deg"),
                    format!(
                        "`pitch_deg` must be within {GEO_PITCH_MIN_DEG}..={GEO_PITCH_MAX_DEG}, got {pitch_deg}"
                    ),
                ));
            }
            if !(ALTITUDE_MIN_M..=ALTITUDE_MAX_M).contains(altitude_m) {
                return Err(VisionRefusal::new(
                    "geo_out_of_range",
                    Some("pose.geo.altitude_m"),
                    format!(
                        "`altitude_m` must be within {ALTITUDE_MIN_M}..={ALTITUDE_MAX_M}, got {altitude_m}"
                    ),
                ));
            }
        }
        Some(ValidPoseSpec::Tile { z, x, y }) => {
            if *z > TILE_Z_MAX {
                return Err(VisionRefusal::new(
                    "tile_out_of_range",
                    Some("pose.tile.z"),
                    format!("`z` must be <= {TILE_Z_MAX}, got {z}"),
                ));
            }
            let span = 1u64 << *z;
            if u64::from(*x) >= span || u64::from(*y) >= span {
                return Err(VisionRefusal::new(
                    "tile_out_of_range",
                    Some("pose.tile.x"),
                    format!("`x`/`y` must be < 2^z ({span}) at z={z}"),
                ));
            }
        }
        Some(ValidPoseSpec::Focus { .. }) | None => {}
    }
    Ok(())
}

/// A camera is degenerate when `position == target` (nothing to look at) or
/// when `up` is zero/parallel to the view direction (`Transform::looking_at`
/// has no valid "right" vector to build from — the smart_aim NaN crash
/// class this track exists to close).
fn check_degenerate_camera(position: [f64; 3], target: [f64; 3], up: [f64; 3]) -> Result<(), VisionRefusal> {
    let view_dir = sub3(target, position);
    if norm3(view_dir) < 1e-6 {
        return Err(VisionRefusal::new(
            "degenerate_camera",
            Some("pose.camera.target"),
            "`position` and `target` must differ",
        ));
    }
    if norm3(up) < 1e-6 {
        return Err(VisionRefusal::new(
            "degenerate_camera",
            Some("pose.camera.up"),
            "`up` must be a non-zero vector",
        ));
    }
    if norm3(cross3(up, view_dir)) < 1e-6 {
        return Err(VisionRefusal::new(
            "degenerate_camera",
            Some("pose.camera.up"),
            "`up` must not be parallel to the view direction",
        ));
    }
    Ok(())
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}

// ---------------------------------------------------------------------------
// Resolved pose + camera state (reply shapes; spec §5.2 bottom, §5.6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoseSource {
    Camera,
    Orbit,
    Geo,
    Tile,
    Focus,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ResolvedOrbit {
    pub focus: [f64; 3],
    pub yaw_deg: f64,
    pub pitch_deg: f64,
    pub distance: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AltitudeReference {
    /// `altitude_m` is measured above the resident `TerrainHeightField`.
    Terrain,
    /// No height field was resident; measured above the grounded origin elevation.
    Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ResolvedGeo {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: f64,
    pub altitude_reference: AltitudeReference,
}

/// The pose a capture actually used — echoed as the `X-FE-Capture-Pose`
/// header on REST and as a text part on MCP (spec §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPose {
    pub mode: CaptureMode,
    pub position: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
    pub fov_deg: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orbit: Option<ResolvedOrbit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<ResolvedGeo>,
    pub world_scale: f64,
    pub width: u32,
    pub height: u32,
    pub settle_frames_used: u32,
    /// `false` with `tiles_pending > 0` means the settle cap hit before
    /// residency — the capture is still returned and honest about it.
    pub settled: bool,
    pub tiles_pending: u32,
    pub restored: bool,
    pub source: PoseSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveHierarchy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verse_id: Option<String>,
    pub verse_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fractal_id: Option<String>,
    pub fractal_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub petal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionState {
    pub origin_lat: f64,
    pub origin_lon: f64,
    /// The RUNTIME-grounded elevation (`ActiveTileSource`), never the
    /// persisted petal config (spec §3: center-tile grounding overwrites it).
    pub origin_ele_grounded: f64,
    pub world_scale: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tileset_bounds: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom_range: Option<[u8; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
}

/// `GET /api/v1/debug/camera` reply (spec §5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraState {
    pub pose: ResolvedPose,
    /// `true` while a posed capture job holds the camera.
    pub locked: bool,
    pub active: ActiveHierarchy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ProjectionState>,
    pub window: WindowState,
}

// ---------------------------------------------------------------------------
// Bridge request/reply (the shared §correlated-bridge envelope; AGENTS.md
// §vision-bridge says why it is a separate CHANNEL from the screenshot one)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum VisionOp {
    Capture(ValidCaptureSpec),
    ReadCamera,
}

/// One vision request on the shared correlated-bridge envelope
/// (§correlated-bridge): correlation id, caller scope (the host refuses
/// unless it covers the active verse and any focus target — SEC-ADDENDUM),
/// the operation, and its private reply slot.
pub type VisionRequest = BridgeRequest<VisionOp, VisionResult>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionFailureClass {
    /// A typed dynamic refusal (`no_projection`, `node_hidden`, …) — maps to 422.
    Unresolvable,
    /// A posed capture already holds the camera — maps to 409.
    Busy,
    /// No window to capture from (headless/relay/minimized) — maps to 503/422.
    NoWindow,
    /// Any other Bevy-side failure — maps to 500/504.
    Failed,
    /// The caller's scope does not cover the active verse / the target, or
    /// the target does not exist — one uniform 404 ([`denied`]).
    Denied,
}

/// The uniform scope/existence denial (no existence oracle).
pub fn denied() -> VisionFailure {
    VisionFailure {
        status_class: VisionFailureClass::Denied,
        code: DENIED_CODE,
        message: DENIED_MESSAGE.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisionFailure {
    pub status_class: VisionFailureClass,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VisionOutput {
    Capture { png: Vec<u8>, pose: ResolvedPose },
    Camera(CameraState),
}

/// What a vision op produced, or the host's typed refusal/failure.
pub type VisionResult = Result<VisionOutput, VisionFailure>;

/// The vision outcome, echoing the request's correlation id.
pub type VisionReply = BridgeReply<VisionResult>;

#[cfg(test)]
mod tests {
    use super::*;

    fn camera_spec() -> CaptureSpec {
        CaptureSpec {
            mode: Some(CaptureMode::Window),
            pose: Some(PoseSpec {
                camera: Some(CameraPose {
                    position: [0.0, 1.0, 0.0],
                    target: [0.0, 0.0, 1.0],
                    up: None,
                    fov_deg: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn orbit_spec() -> CaptureSpec {
        CaptureSpec {
            mode: Some(CaptureMode::Window),
            pose: Some(PoseSpec {
                orbit: Some(OrbitPose {
                    focus: [0.0, 0.0, 0.0],
                    yaw_deg: 0.0,
                    pitch_deg: 0.0,
                    distance: 10.0,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn geo_spec() -> CaptureSpec {
        CaptureSpec {
            mode: Some(CaptureMode::Window),
            pose: Some(PoseSpec {
                geo: Some(GeoPose {
                    lat: 10.0,
                    lon: 10.0,
                    altitude_m: None,
                    heading_deg: None,
                    pitch_deg: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn tile_spec() -> CaptureSpec {
        CaptureSpec {
            mode: Some(CaptureMode::Scene),
            pose: Some(PoseSpec {
                tile: Some(TilePose { z: 10, x: 5, y: 5 }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn focus_spec(node: bool) -> CaptureSpec {
        let focus = if node {
            FocusPose {
                node_id: Some("n1".into()),
                petal_id: None,
                navigate: None,
            }
        } else {
            FocusPose {
                node_id: None,
                petal_id: Some("p1".into()),
                navigate: None,
            }
        };
        CaptureSpec {
            mode: Some(CaptureMode::Window),
            pose: Some(PoseSpec {
                focus: Some(focus),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn size_case(mode: CaptureMode, width: Option<u32>, height: Option<u32>) -> CaptureSpec {
        CaptureSpec {
            mode: Some(mode),
            width,
            height,
            ..Default::default()
        }
    }

    fn assert_refused(spec: CaptureSpec, code: &str, field: Option<&str>) {
        let err = spec.validate().expect_err("expected a refusal");
        assert_eq!(err.code, code, "message: {}", err.message);
        assert_eq!(err.field, field);
    }

    #[test]
    fn capture_spec_validation_table() {
        // non_finite — one row per float field across every pose kind.
        let mut s = camera_spec();
        s.pose.as_mut().unwrap().camera.as_mut().unwrap().position[0] = f64::NAN;
        assert_refused(s, "non_finite", Some("pose.camera.position.x"));

        let mut s = camera_spec();
        s.pose.as_mut().unwrap().camera.as_mut().unwrap().target[2] = f64::INFINITY;
        assert_refused(s, "non_finite", Some("pose.camera.target.z"));

        let mut s = camera_spec();
        s.pose.as_mut().unwrap().camera.as_mut().unwrap().up = Some([0.0, f64::NEG_INFINITY, 0.0]);
        assert_refused(s, "non_finite", Some("pose.camera.up.y"));

        let mut s = camera_spec();
        s.pose.as_mut().unwrap().camera.as_mut().unwrap().fov_deg = Some(f64::NAN);
        assert_refused(s, "non_finite", Some("pose.camera.fov_deg"));

        let mut s = orbit_spec();
        s.pose.as_mut().unwrap().orbit.as_mut().unwrap().yaw_deg = f64::NAN;
        assert_refused(s, "non_finite", Some("pose.orbit.yaw_deg"));

        let mut s = orbit_spec();
        s.pose.as_mut().unwrap().orbit.as_mut().unwrap().pitch_deg = f64::INFINITY;
        assert_refused(s, "non_finite", Some("pose.orbit.pitch_deg"));

        let mut s = orbit_spec();
        s.pose.as_mut().unwrap().orbit.as_mut().unwrap().distance = f64::NAN;
        assert_refused(s, "non_finite", Some("pose.orbit.distance"));

        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().lat = f64::NAN;
        assert_refused(s, "non_finite", Some("pose.geo.lat"));

        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().lon = f64::INFINITY;
        assert_refused(s, "non_finite", Some("pose.geo.lon"));

        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().altitude_m = Some(f64::NAN);
        assert_refused(s, "non_finite", Some("pose.geo.altitude_m"));

        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().heading_deg = Some(f64::NAN);
        assert_refused(s, "non_finite", Some("pose.geo.heading_deg"));

        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().pitch_deg = Some(f64::NAN);
        assert_refused(s, "non_finite", Some("pose.geo.pitch_deg"));

        // size_out_of_range
        assert_refused(
            size_case(CaptureMode::Scene, Some(32), None),
            "size_out_of_range",
            Some("width"),
        );
        assert_refused(
            size_case(CaptureMode::Scene, None, Some(5000)),
            "size_out_of_range",
            Some("height"),
        );

        // size_requires_scene_mode
        assert_refused(
            size_case(CaptureMode::Window, Some(640), None),
            "size_requires_scene_mode",
            None,
        );

        // option_not_valid_in_mode: restore, then navigate.
        assert_refused(
            CaptureSpec {
                mode: Some(CaptureMode::Scene),
                restore: Some(true),
                ..Default::default()
            },
            "option_not_valid_in_mode",
            Some("restore"),
        );
        let mut s = focus_spec(true);
        s.mode = Some(CaptureMode::Scene);
        s.pose.as_mut().unwrap().focus.as_mut().unwrap().navigate = Some(true);
        assert_refused(s, "option_not_valid_in_mode", Some("pose.focus.navigate"));

        // settle_out_of_range
        let mut s = camera_spec();
        s.settle_frames = Some(SETTLE_MAX + 1);
        assert_refused(s, "settle_out_of_range", Some("settle_frames"));

        // fov_out_of_range
        let mut s = camera_spec();
        s.pose.as_mut().unwrap().camera.as_mut().unwrap().fov_deg = Some(200.0);
        assert_refused(s, "fov_out_of_range", Some("pose.camera.fov_deg"));

        // orbit_out_of_range
        let mut s = orbit_spec();
        s.pose.as_mut().unwrap().orbit.as_mut().unwrap().distance = -1.0;
        assert_refused(s, "orbit_out_of_range", Some("pose.orbit.distance"));
        let mut s = orbit_spec();
        s.pose.as_mut().unwrap().orbit.as_mut().unwrap().pitch_deg = 90.0;
        assert_refused(s, "orbit_out_of_range", Some("pose.orbit.pitch_deg"));

        // degenerate_camera
        let mut s = camera_spec();
        let position = s.pose.as_ref().unwrap().camera.as_ref().unwrap().position;
        s.pose.as_mut().unwrap().camera.as_mut().unwrap().target = position;
        assert_refused(s, "degenerate_camera", Some("pose.camera.target"));
        let mut s = camera_spec();
        s.pose.as_mut().unwrap().camera.as_mut().unwrap().up = Some([0.0, 0.0, 0.0]);
        assert_refused(s, "degenerate_camera", Some("pose.camera.up"));

        // geo_out_of_range
        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().lat = 90.0;
        assert_refused(s, "geo_out_of_range", Some("pose.geo.lat"));
        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().lon = 190.0;
        assert_refused(s, "geo_out_of_range", Some("pose.geo.lon"));
        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().heading_deg = Some(400.0);
        assert_refused(s, "geo_out_of_range", Some("pose.geo.heading_deg"));
        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().pitch_deg = Some(-95.0);
        assert_refused(s, "geo_out_of_range", Some("pose.geo.pitch_deg"));
        let mut s = geo_spec();
        s.pose.as_mut().unwrap().geo.as_mut().unwrap().altitude_m = Some(2_000_000.0);
        assert_refused(s, "geo_out_of_range", Some("pose.geo.altitude_m"));

        // tile_out_of_range
        let mut s = tile_spec();
        s.pose.as_mut().unwrap().tile.as_mut().unwrap().z = 25;
        assert_refused(s, "tile_out_of_range", Some("pose.tile.z"));
        let mut s = tile_spec();
        s.pose.as_mut().unwrap().tile.as_mut().unwrap().x = 2000; // z=10 span is 1024
        assert_refused(s, "tile_out_of_range", Some("pose.tile.x"));
    }

    #[test]
    fn two_pose_keys_or_none_is_refused() {
        let two_keys: CaptureSpec = serde_json::from_value(serde_json::json!({
            "pose": {
                "camera": {"position": [0.0, 0.0, 0.0], "target": [0.0, 0.0, 1.0]},
                "orbit": {"focus": [0.0, 0.0, 0.0], "yaw_deg": 0.0, "pitch_deg": 0.0, "distance": 5.0},
            }
        }))
        .expect("parses structurally — cardinality is validate()'s job, not serde's");
        assert_refused(two_keys, "pose_must_have_exactly_one_kind", Some("pose"));

        let zero_keys: CaptureSpec = serde_json::from_value(serde_json::json!({ "pose": {} }))
            .expect("parses structurally");
        assert_refused(zero_keys, "pose_must_have_exactly_one_kind", Some("pose"));

        let both_focus_ids: CaptureSpec = serde_json::from_value(serde_json::json!({
            "pose": { "focus": { "node_id": "n1", "petal_id": "p1" } }
        }))
        .expect("parses structurally");
        assert_refused(both_focus_ids, "pose_must_have_exactly_one_kind", Some("pose.focus"));

        let neither_focus_id: CaptureSpec =
            serde_json::from_value(serde_json::json!({ "pose": { "focus": {} } }))
                .expect("parses structurally");
        assert_refused(neither_focus_id, "pose_must_have_exactly_one_kind", Some("pose.focus"));
    }

    #[test]
    fn defaults_applied_by_mode() {
        let window_no_pose = CaptureSpec::default()
            .validate()
            .expect("a bare window capture is valid");
        assert_eq!(window_no_pose.settle_frames(), 0);
        assert_eq!(window_no_pose.restore(), Some(true));
        assert_eq!(window_no_pose.width(), None);
        assert_eq!(window_no_pose.height(), None);

        let scene_no_pose = CaptureSpec {
            mode: Some(CaptureMode::Scene),
            ..Default::default()
        }
        .validate()
        .expect("a bare scene capture is valid");
        assert_eq!(scene_no_pose.settle_frames(), DEFAULT_SETTLE);
        assert_eq!(scene_no_pose.width(), Some(DEFAULT_WIDTH));
        assert_eq!(scene_no_pose.height(), Some(DEFAULT_HEIGHT));
        assert_eq!(scene_no_pose.restore(), None);

        let window_with_pose = camera_spec().validate().expect("a camera pose is valid");
        assert_eq!(window_with_pose.settle_frames(), DEFAULT_SETTLE);

        // node focus and petal focus are both valid single-kind poses.
        focus_spec(true).validate().expect("node focus is valid");
        focus_spec(false).validate().expect("petal focus is valid");
    }

    /// UI-ADDENDUM: `include_ui` x `mode` -> the resolved mode, or the typed
    /// conflict refusal (fail closed, never silent precedence).
    #[test]
    fn include_ui_and_mode_matrix() {
        use CaptureMode::{Scene, Window};
        let cases: [(Option<CaptureMode>, Option<bool>, Option<CaptureMode>); 9] = [
            (None, None, Some(Window)),
            (None, Some(true), Some(Window)),
            (None, Some(false), Some(Scene)),
            (Some(Window), None, Some(Window)),
            (Some(Scene), None, Some(Scene)),
            (Some(Window), Some(true), Some(Window)),
            (Some(Scene), Some(false), Some(Scene)),
            (Some(Window), Some(false), None),
            (Some(Scene), Some(true), None),
        ];
        for (mode, include_ui, expected) in cases {
            let outcome = CaptureSpec {
                mode,
                include_ui,
                ..Default::default()
            }
            .validate();
            match (outcome, expected) {
                (Ok(valid), Some(want)) => {
                    assert_eq!(valid.mode(), want, "{mode:?}/{include_ui:?}");
                    assert_eq!(valid.include_ui(), want == Window);
                }
                (Err(refusal), None) => {
                    assert_eq!(refusal.code, "include_ui_conflicts_with_mode");
                    assert_eq!(refusal.field, Some("include_ui"));
                }
                (outcome, expected) => {
                    panic!("{mode:?}/{include_ui:?}: got {outcome:?}, want {expected:?}")
                }
            }
        }
        // include_ui:false alone unlocks the scene-only size knobs.
        let scene: CaptureSpec = serde_json::from_value(
            serde_json::json!({ "include_ui": false, "width": 640, "height": 480 }),
        )
        .expect("parses");
        assert_eq!(scene.validate().expect("valid").width(), Some(640));
    }

    #[test]
    fn vision_bridge_is_bounded_at_its_capacity() {
        let (tx, _rx) = vision_bridge();
        let mut parked = Vec::new();
        for i in 0..VISION_BRIDGE_CAPACITY {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tx.try_send(VisionRequest {
                correlation_id: format!("c{i}"),
                caller_scope: "VERSE#v".into(),
                op: VisionOp::ReadCamera,
                reply_tx,
            })
            .expect("capacity not yet exceeded");
            parked.push(reply_rx);
        }
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        let overflow = tx.try_send(VisionRequest {
            correlation_id: "overflow".into(),
            caller_scope: "VERSE#v".into(),
            op: VisionOp::ReadCamera,
            reply_tx,
        });
        assert!(
            overflow.is_err(),
            "the bridge must refuse past its bounded capacity, never block"
        );
    }
}
