//! Staged-write helpers over `txn`: JSON values in, one window per tick (host flushes after `on-tick`).

use serde_json::Value;

use crate::txn::{self, TransformPatch};
use crate::{HostError, Quat, Vec3};

/// Room left in the current window, from `txn.window()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    pub ops: u32,
    pub readings: u32,
    pub bytes: u32,
}

/// What the current window still has room for.
pub fn room() -> Room {
    let info = txn::window();
    room_from(info.ops, info.max_ops, info.readings, info.max_readings, info.bytes, info.max_bytes)
}

/// Pure arithmetic behind [`room`]: limits minus use, never negative.
pub fn room_from(ops: u32, max_ops: u32, readings: u32, max_readings: u32, bytes: u32, max_bytes: u32) -> Room {
    Room {
        ops: max_ops.saturating_sub(ops),
        readings: max_readings.saturating_sub(readings),
        bytes: max_bytes.saturating_sub(bytes),
    }
}

/// Stage a property replacement (the key must be inside the manifest's `property_scope`).
pub fn set_property(node: &str, key: &str, value: &Value) -> Result<(), HostError> {
    txn::stage_set_property(node, key, &value.to_string())
}

/// Stage a position change; rotation and scale keep their stored values.
pub fn set_position(node: &str, position: [f64; 3]) -> Result<(), HostError> {
    txn::stage_set_transform(
        node,
        &TransformPatch {
            position: Some(Vec3 { x: position[0], y: position[1], z: position[2] }),
            rotation: None,
            scale: None,
        },
    )
}

/// Stage a position and rotation change (quaternion `[x, y, z, w]`).
pub fn set_pose(node: &str, position: [f64; 3], rotation: [f64; 4]) -> Result<(), HostError> {
    txn::stage_set_transform(
        node,
        &TransformPatch {
            position: Some(Vec3 { x: position[0], y: position[1], z: position[2] }),
            rotation: Some(Quat { x: rotation[0], y: rotation[1], z: rotation[2], w: rotation[3] }),
            scale: None,
        },
    )
}

/// Stage a data-node creation; the id arrives in the next tick's `commit-receipt`, by ordinal.
pub fn create_node(name: &str) -> Result<u32, HostError> {
    txn::stage_create_node(name)
}
