//! Per-frame billboard system: keeps `Billboard`-tagged entities' flat icon
//! quads facing the viewport camera. See `fe-ui/src/node_manager/AGENTS.md`
//! §data-icons and `fe-ui/src/AGENTS.md` §data-icons. `data_icons_20260713`.

use bevy::prelude::*;

use crate::node_manager::NodeManager;
use crate::plugin::Billboard;

/// Rotate every `Billboard` entity to face the `OrbitCameraController` camera,
/// skipping the selected node — see `node_manager/AGENTS.md` §data-icons.
pub(super) fn billboard_face_camera(
    cameras: Query<&GlobalTransform, With<fe_renderer::camera::OrbitCameraController>>,
    node_mgr: Res<NodeManager>,
    mut billboards: Query<(Entity, &mut Transform), With<Billboard>>,
) {
    let Ok(cam_gtx) = cameras.single() else {
        return;
    };
    let face = cam_gtx.rotation();
    let selected = node_mgr.selected_entity();
    for (entity, mut transform) in billboards.iter_mut() {
        if selected == Some(entity) {
            continue;
        }
        transform.rotation = face;
    }
}
