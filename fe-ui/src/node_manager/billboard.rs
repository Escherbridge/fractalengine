//! Per-frame billboard system: keeps `Billboard`-tagged entities' flat icon
//! quads facing the viewport camera. See `fe-ui/src/node_manager/AGENTS.md`
//! §data-icons and `fe-ui/src/AGENTS.md` §data-icons. `data_icons_20260713`.

use bevy::prelude::*;

use crate::node_manager::NodeManager;
use crate::plugin::Billboard;

/// Rotate every `Billboard` entity to face the `OrbitCameraController` camera,
/// except the currently-selected node.
///
/// Cheap: one camera lookup + a per-entity rotation write, scoped to the
/// billboard-tagged set only. Copying the camera's world rotation makes the
/// quad parallel to the camera image plane (a `Rectangle` `Mesh3d` lies in its
/// local XY plane, +Z its normal — matching the camera's +Z means the quad's
/// face points down the camera's view axis, i.e. straight at the viewer). No-op
/// when there's no orbit camera (headless / pre-spawn).
///
/// MEDIUM-1: the single-point track node is a `Billboard` that can also be
/// selected and gimbal-rotated (Rotate/Move/Scale). While it's the selected
/// entity, its gimbal owns its `Transform` — so this system skips it, otherwise
/// the per-frame face-camera write would silently overwrite the gimbal rotation
/// every frame. All other billboards keep facing the camera. See
/// `node_manager/AGENTS.md` §data-icons.
pub(super) fn billboard_face_camera(
    cameras: Query<&GlobalTransform, With<fe_renderer::camera::OrbitCameraController>>,
    node_mgr: Res<NodeManager>,
    mut billboards: Query<(Entity, &mut Transform), With<Billboard>>,
) {
    let Ok(cam_gtx) = cameras.single() else { return };
    let face = cam_gtx.rotation();
    let selected = node_mgr.selected_entity();
    for (entity, mut transform) in billboards.iter_mut() {
        if selected == Some(entity) {
            continue;
        }
        transform.rotation = face;
    }
}
