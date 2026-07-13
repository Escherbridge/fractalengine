//! Per-frame billboard system: keeps `Billboard`-tagged entities' flat icon
//! quads facing the viewport camera. See `fe-ui/src/node_manager/AGENTS.md`
//! §data-icons and `fe-ui/src/AGENTS.md` §data-icons. `data_icons_20260713`.

use bevy::prelude::*;

use crate::plugin::Billboard;

/// Rotate every `Billboard` entity to face the `OrbitCameraController` camera.
///
/// Cheap: one camera lookup + a per-entity rotation write, scoped to the
/// billboard-tagged set only. Copying the camera's world rotation makes the
/// quad parallel to the camera image plane (a `Rectangle` `Mesh3d` lies in its
/// local XY plane, +Z its normal — matching the camera's +Z means the quad's
/// face points down the camera's view axis, i.e. straight at the viewer). No-op
/// when there's no orbit camera (headless / pre-spawn).
pub(super) fn billboard_face_camera(
    cameras: Query<&GlobalTransform, With<fe_renderer::camera::OrbitCameraController>>,
    mut billboards: Query<&mut Transform, With<Billboard>>,
) {
    let Ok(cam_gtx) = cameras.single() else { return };
    let face = cam_gtx.rotation();
    for mut transform in billboards.iter_mut() {
        transform.rotation = face;
    }
}
