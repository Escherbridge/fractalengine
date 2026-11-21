//! Declarative node geometry (`render.mesh`): a bounded list of primitive parts.
//! Strict reader with typed refusals; see `fe-sdk/src/AGENTS.md` §render-mesh.

use std::fmt;

use serde_json::{Map, Value};

use crate::primitive::{PrimitiveDescriptor, PrimitiveKind};

/// Canonical node property key storing a [`RenderMeshDescriptor`] JSON payload.
pub const RENDER_MESH_PROPERTY_KEY: &str = "render.mesh";
/// The only descriptor version this build reads.
pub const RENDER_MESH_VERSION: u64 = 1;
/// Most parts one descriptor may carry.
pub const MAX_RENDER_MESH_PARTS: usize = 32;
/// Most serialized bytes one descriptor may occupy.
pub const MAX_RENDER_MESH_BYTES: usize = 16 * 1024;
/// Smallest accepted dimension, in world units.
pub const MIN_RENDER_MESH_LENGTH: f32 = 0.01;
/// Largest accepted dimension or offset magnitude, in world units.
pub const MAX_RENDER_MESH_LENGTH: f32 = 1000.0;

const DEFAULT_COLOR: [f32; 3] = [0.8, 0.8, 0.8];
const COMMON_FIELDS: [&str; 4] = ["offset", "yaw_deg", "color", "shape"];

/// Why a `render.mesh` value was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderMeshRefusal {
    /// The value (or a part) is not a JSON object.
    NotAnObject,
    /// Serialized form exceeds [`MAX_RENDER_MESH_BYTES`].
    TooLarge {
        /// Serialized size.
        bytes: usize,
    },
    /// `v` is missing or not [`RENDER_MESH_VERSION`].
    UnsupportedVersion,
    /// `parts` is missing or empty.
    NoParts,
    /// More than [`MAX_RENDER_MESH_PARTS`] parts.
    TooManyParts {
        /// Parts present.
        count: usize,
    },
    /// A part names a shape this build cannot draw.
    UnknownShape {
        /// Part index.
        part: usize,
        /// The shape as written.
        shape: String,
    },
    /// A field this reader does not define (`part` is `None` at the top level).
    UnknownField {
        /// Part index, if inside a part.
        part: Option<usize>,
        /// The field as written.
        field: String,
    },
    /// A required field is absent.
    MissingField {
        /// Part index.
        part: usize,
        /// Field name.
        field: &'static str,
    },
    /// A number is missing, non-numeric, NaN or infinite.
    NotFinite {
        /// Part index.
        part: usize,
        /// Field name.
        field: &'static str,
    },
    /// A finite number or array length outside its allowed range.
    OutOfRange {
        /// Part index.
        part: usize,
        /// Field name.
        field: &'static str,
    },
}

impl fmt::Display for RenderMeshRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnObject => write!(f, "render.mesh must be a JSON object"),
            Self::TooLarge { bytes } => {
                write!(f, "render.mesh is {bytes} bytes, over {MAX_RENDER_MESH_BYTES}")
            }
            Self::UnsupportedVersion => write!(f, "render.mesh v must be {RENDER_MESH_VERSION}"),
            Self::NoParts => write!(f, "render.mesh needs at least one part"),
            Self::TooManyParts { count } => {
                write!(f, "render.mesh has {count} parts, over {MAX_RENDER_MESH_PARTS}")
            }
            Self::UnknownShape { part, shape } => {
                write!(f, "render.mesh part {part}: unknown shape {shape:?}")
            }
            Self::UnknownField { part: Some(part), field } => {
                write!(f, "render.mesh part {part}: unknown field {field:?}")
            }
            Self::UnknownField { part: None, field } => {
                write!(f, "render.mesh: unknown field {field:?}")
            }
            Self::MissingField { part, field } => {
                write!(f, "render.mesh part {part}: missing {field}")
            }
            Self::NotFinite { part, field } => {
                write!(f, "render.mesh part {part}: {field} is not a finite number")
            }
            Self::OutOfRange { part, field } => {
                write!(f, "render.mesh part {part}: {field} is out of range")
            }
        }
    }
}

impl std::error::Error for RenderMeshRefusal {}

/// Shape and dimensions of one part (world units, base resting at the part's offset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeshShape {
    /// Box: `size = [width, height, depth]`.
    Box {
        /// Width, height, depth.
        size: [f32; 3],
    },
    /// Upright cylinder.
    Cylinder {
        /// Radius.
        radius: f32,
        /// Height.
        height: f32,
    },
    /// Sphere resting on its offset.
    Sphere {
        /// Radius.
        radius: f32,
    },
    /// Flat horizontal plane: `size = [width, depth]`.
    Plane {
        /// Width, depth.
        size: [f32; 2],
    },
}

impl MeshShape {
    fn field_names(&self) -> &'static [&'static str] {
        match self {
            Self::Box { .. } | Self::Plane { .. } => &["size"],
            Self::Cylinder { .. } => &["radius", "height"],
            Self::Sphere { .. } => &["radius"],
        }
    }
}

/// One drawn part: a shape placed at `offset`, turned `yaw_deg` about +Y, coloured `color`.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshPart {
    /// Shape and dimensions.
    pub shape: MeshShape,
    /// Base-centre offset from the node origin.
    pub offset: [f32; 3],
    /// Rotation about +Y in degrees.
    pub yaw_deg: f32,
    /// Linear-ish sRGB colour, each channel clamped to `0.0..=1.0`.
    pub color: [f32; 3],
}

impl MeshPart {
    /// The existing primitive descriptor that draws this part (one mesh builder, not two).
    pub fn to_primitive(&self) -> PrimitiveDescriptor {
        let (kind, dims) = match self.shape {
            MeshShape::Box { size } => (PrimitiveKind::Cube, size.to_vec()),
            MeshShape::Cylinder { radius, height } => (PrimitiveKind::Cylinder, vec![radius, height]),
            MeshShape::Sphere { radius } => (PrimitiveKind::Sphere, vec![radius]),
            MeshShape::Plane { size } => (PrimitiveKind::Plane, size.to_vec()),
        };
        PrimitiveDescriptor { kind, dims, texture_ref: None }
    }

    /// Height from the base to the primitive's centre (Bevy primitives are centre-origin).
    pub fn centre_lift(&self) -> f32 {
        match self.shape {
            MeshShape::Box { size } => size[1] / 2.0,
            MeshShape::Cylinder { height, .. } => height / 2.0,
            MeshShape::Sphere { radius } => radius,
            MeshShape::Plane { .. } => 0.0,
        }
    }

    fn parse(part: usize, value: &Value) -> Result<Self, RenderMeshRefusal> {
        let object = value.as_object().ok_or(RenderMeshRefusal::NotAnObject)?;
        let shape_name = object
            .get("shape")
            .and_then(Value::as_str)
            .ok_or(RenderMeshRefusal::MissingField { part, field: "shape" })?;
        let shape = match shape_name {
            "box" => MeshShape::Box { size: length_array::<3>(object, part, "size")? },
            "cylinder" => MeshShape::Cylinder {
                radius: length(object, part, "radius")?,
                height: length(object, part, "height")?,
            },
            "sphere" => MeshShape::Sphere { radius: length(object, part, "radius")? },
            "plane" => MeshShape::Plane { size: length_array::<2>(object, part, "size")? },
            other => {
                return Err(RenderMeshRefusal::UnknownShape { part, shape: other.to_owned() });
            }
        };
        let own_fields = shape.field_names();
        if let Some(field) = object
            .keys()
            .find(|key| !COMMON_FIELDS.contains(&key.as_str()) && !own_fields.contains(&key.as_str()))
        {
            return Err(RenderMeshRefusal::UnknownField { part: Some(part), field: field.clone() });
        }

        let offset = match object.get("offset") {
            None => [0.0; 3],
            Some(raw) => {
                let offset = number_array::<3>(raw, part, "offset")?;
                if offset.iter().any(|v| v.abs() > MAX_RENDER_MESH_LENGTH) {
                    return Err(RenderMeshRefusal::OutOfRange { part, field: "offset" });
                }
                offset
            }
        };
        let yaw_deg = match object.get("yaw_deg") {
            None => 0.0,
            Some(raw) => number(raw).ok_or(RenderMeshRefusal::NotFinite { part, field: "yaw_deg" })?,
        };
        let color = match object.get("color") {
            None => DEFAULT_COLOR,
            Some(raw) => number_array::<3>(raw, part, "color")?.map(|channel| channel.clamp(0.0, 1.0)),
        };
        Ok(Self { shape, offset, yaw_deg, color })
    }
}

/// A validated `render.mesh` value: 1..=32 parts.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderMeshDescriptor {
    /// Parts in draw order.
    pub parts: Vec<MeshPart>,
}

impl RenderMeshDescriptor {
    /// Strict reader: the whole value is refused if any bound is violated.
    pub fn parse(value: &Value) -> Result<Self, RenderMeshRefusal> {
        let root = value.as_object().ok_or(RenderMeshRefusal::NotAnObject)?;
        let bytes = serde_json::to_vec(value).map(|encoded| encoded.len()).unwrap_or(usize::MAX);
        if bytes > MAX_RENDER_MESH_BYTES {
            return Err(RenderMeshRefusal::TooLarge { bytes });
        }
        if let Some(field) = root.keys().find(|key| !matches!(key.as_str(), "v" | "parts")) {
            return Err(RenderMeshRefusal::UnknownField { part: None, field: field.clone() });
        }
        if root.get("v").and_then(Value::as_u64) != Some(RENDER_MESH_VERSION) {
            return Err(RenderMeshRefusal::UnsupportedVersion);
        }
        let raw_parts = root.get("parts").and_then(Value::as_array).ok_or(RenderMeshRefusal::NoParts)?;
        if raw_parts.is_empty() {
            return Err(RenderMeshRefusal::NoParts);
        }
        if raw_parts.len() > MAX_RENDER_MESH_PARTS {
            return Err(RenderMeshRefusal::TooManyParts { count: raw_parts.len() });
        }
        let parts = raw_parts
            .iter()
            .enumerate()
            .map(|(index, raw)| MeshPart::parse(index, raw))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { parts })
    }

    /// Lenient wrapper: `None` for `null` or any refusal.
    pub fn from_json(value: &Value) -> Option<Self> {
        if value.is_null() {
            return None;
        }
        Self::parse(value).ok()
    }
}

/// True for keys a manifest must list verbatim in `scene.property_scope` to write them (wildcards still cover reads).
pub fn requires_exact_scope(key: &str) -> bool {
    key == RENDER_MESH_PROPERTY_KEY
}

/// A finite JSON number as `f32`.
fn number(value: &Value) -> Option<f32> {
    let wide = value.as_f64()?;
    (wide.is_finite() && wide.abs() <= f64::from(f32::MAX)).then_some(wide as f32)
}

fn number_array<const N: usize>(
    raw: &Value,
    part: usize,
    field: &'static str,
) -> Result<[f32; N], RenderMeshRefusal> {
    let items = raw.as_array().ok_or(RenderMeshRefusal::MissingField { part, field })?;
    if items.len() != N {
        return Err(RenderMeshRefusal::OutOfRange { part, field });
    }
    let mut out = [0.0_f32; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = number(item).ok_or(RenderMeshRefusal::NotFinite { part, field })?;
    }
    Ok(out)
}

fn in_length_range(value: f32) -> bool {
    (MIN_RENDER_MESH_LENGTH..=MAX_RENDER_MESH_LENGTH).contains(&value)
}

fn length(object: &Map<String, Value>, part: usize, field: &'static str) -> Result<f32, RenderMeshRefusal> {
    let raw = object.get(field).ok_or(RenderMeshRefusal::MissingField { part, field })?;
    let value = number(raw).ok_or(RenderMeshRefusal::NotFinite { part, field })?;
    if in_length_range(value) {
        Ok(value)
    } else {
        Err(RenderMeshRefusal::OutOfRange { part, field })
    }
}

fn length_array<const N: usize>(
    object: &Map<String, Value>,
    part: usize,
    field: &'static str,
) -> Result<[f32; N], RenderMeshRefusal> {
    let raw = object.get(field).ok_or(RenderMeshRefusal::MissingField { part, field })?;
    let values = number_array::<N>(raw, part, field)?;
    if values.iter().all(|value| in_length_range(*value)) {
        Ok(values)
    } else {
        Err(RenderMeshRefusal::OutOfRange { part, field })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parts(count: usize) -> Value {
        let part = json!({ "shape": "box", "size": [1.0, 1.0, 1.0] });
        json!({ "v": 1, "parts": vec![part; count] })
    }

    #[test]
    fn parse_table_accepts_the_shapes_and_refuses_each_bound() {
        use RenderMeshRefusal as R;
        let valid = json!({ "v": 1, "parts": [
            { "shape": "box", "size": [10.0, 3.0, 8.0], "color": [2.0, 0.5, -1.0] },
            { "shape": "cylinder", "radius": 1.0, "height": 4.0, "offset": [1.0, 3.0, 0.0], "yaw_deg": 45.0 },
            { "shape": "sphere", "radius": 0.5 },
            { "shape": "plane", "size": [4.0, 4.0] } ] });
        let parsed = RenderMeshDescriptor::parse(&valid).expect("valid descriptor");
        assert_eq!(parsed.parts.len(), 4);
        assert_eq!(parsed.parts[0].color, [1.0, 0.5, 0.0], "channels clamp, defaults fill");
        assert_eq!(parsed.parts[0].centre_lift(), 1.5, "boxes rest on their base");
        assert_eq!(parsed.parts[1].to_primitive().kind, PrimitiveKind::Cylinder);
        assert_eq!(parsed.parts[1].to_primitive().dims, vec![1.0, 4.0]);
        assert!(RenderMeshDescriptor::parse(&parts(32)).is_ok(), "32 parts is the ceiling");
        assert!(RenderMeshDescriptor::from_json(&Value::Null).is_none());

        let rows: Vec<(&str, Value, R)> = vec![
            ("33 parts", parts(33), R::TooManyParts { count: 33 }),
            ("no parts", json!({ "v": 1, "parts": [] }), R::NoParts),
            ("wrong version", json!({ "v": 2, "parts": [] }), R::UnsupportedVersion),
            ("not an object", json!([1]), R::NotAnObject),
            ("top-level extra", json!({ "v": 1, "parts": [], "x": 1 }), R::UnknownField { part: None, field: "x".into() }),
            ("unknown shape", json!({ "v": 1, "parts": [{ "shape": "torus" }] }), R::UnknownShape { part: 0, shape: "torus".into() }),
            ("part extra", json!({ "v": 1, "parts": [{ "shape": "sphere", "radius": 1.0, "size": [1.0] }] }), R::UnknownField { part: Some(0), field: "size".into() }),
            ("missing dimension", json!({ "v": 1, "parts": [{ "shape": "cylinder", "radius": 1.0 }] }), R::MissingField { part: 0, field: "height" }),
            ("string dimension", json!({ "v": 1, "parts": [{ "shape": "sphere", "radius": "big" }] }), R::NotFinite { part: 0, field: "radius" }),
            ("too small", json!({ "v": 1, "parts": [{ "shape": "sphere", "radius": 0.001 }] }), R::OutOfRange { part: 0, field: "radius" }),
            ("too large", json!({ "v": 1, "parts": [{ "shape": "box", "size": [1.0, 1001.0, 1.0] }] }), R::OutOfRange { part: 0, field: "size" }),
            ("wrong arity", json!({ "v": 1, "parts": [{ "shape": "plane", "size": [1.0] }] }), R::OutOfRange { part: 0, field: "size" }),
            ("offset too far", json!({ "v": 1, "parts": [{ "shape": "sphere", "radius": 1.0, "offset": [0.0, 2000.0, 0.0] }] }), R::OutOfRange { part: 0, field: "offset" }),
            ("oversize bytes", json!({ "v": 1, "parts": [{ "shape": "sphere", "radius": 1.0, "color": [0.0, 0.0, 0.0] }], "pad": "x".repeat(MAX_RENDER_MESH_BYTES) }), R::TooLarge { bytes: 0 }),
        ];
        for (row, value, expected) in rows {
            let got = RenderMeshDescriptor::parse(&value).expect_err(row);
            match (&got, &expected) {
                (R::TooLarge { .. }, R::TooLarge { .. }) => {}
                _ => assert_eq!(got, expected, "row {row}"),
            }
        }
    }

    #[test]
    fn exact_scope_grant_is_required_and_wildcards_do_not_confer_it() {
        use crate::manifest_v2::{scope_allows, ScopeAccess::{Read, Write}};
        assert!(requires_exact_scope(RENDER_MESH_PROPERTY_KEY));
        assert!(!requires_exact_scope("render.style"));
        for patterns in [vec!["*"], vec!["render.*"], vec!["render.mesh*"], vec!["render.style"]] {
            assert!(!scope_allows(&patterns, RENDER_MESH_PROPERTY_KEY, Write), "{patterns:?} must not grant a render.mesh write");
        }
        assert!(scope_allows(&["*"], RENDER_MESH_PROPERTY_KEY, Read), "a wildcard still reads render.mesh");
        assert!(!scope_allows(&["render.style"], RENDER_MESH_PROPERTY_KEY, Read), "reads follow the normal rule");
        assert!(scope_allows(&["render.mesh"], RENDER_MESH_PROPERTY_KEY, Write));
        assert!(scope_allows(&["*"], "render.style", Write), "other keys keep wildcard behaviour");
    }
}
