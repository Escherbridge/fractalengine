//! Live primitive reconcile (FR-2) + texture resolution (FR-3): watches the
//! currently-selected node's inspector-loaded properties for a `primitive`
//! JSON descriptor and re-meshes/re-materializes the spawned entity in
//! place when `dims`/`kind`/`texture_ref` change, instead of a full respawn.
//! See `fe-ui/src/verse_manager/AGENTS.md` §primitives.

use bevy::prelude::*;
use fe_hexon::handlers::material::{resolve_material_textures, MaterialHandle};
use fe_hexon::registry::FsBlobStore;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_sdk::primitive::{PrimitiveDescriptor, PrimitiveKind, PRIMITIVE_PROPERTY_KEY};

use super::spawn::{
    build_primitive_mesh, build_wall_mesh, spawn_primitive_entity, spawn_wall_entity, FallbackSign,
    PrimitiveNode, WallNode,
};
use super::TextureRegistryRes;
use crate::node_manager::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{InspectorFormState, SpawnedNodeMarker};

/// FR-5: the flat node-property key a track's polyline rides on — mirrors
/// `GPX_POINTS_KEY` in `fractalengine/src/gpx_bridge.rs` (Track 1). The wall
/// reconcile reads the *same* DB-backed points the path renders from.
const GPX_POINTS_KEY: &str = "gpx_points";

/// Shared default material used when a primitive has no `texture_ref` (FR-3).
#[derive(Resource)]
pub struct PrimitiveMaterialAssets {
    pub default_material: Handle<StandardMaterial>,
}

impl FromWorld for PrimitiveMaterialAssets {
    fn from_world(world: &mut World) -> Self {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        let default_material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.6, 0.6, 0.65),
            ..default()
        });
        Self { default_material }
    }
}

/// Materialize the selected node as a primitive (FR-1, first time a
/// `primitive` descriptor is seen for it — promotes a placeholder fallback
/// sign into the real mesh) and reconcile an already-spawned [`PrimitiveNode`]
/// in place on descriptor change without despawn/respawn (FR-2).
#[allow(clippy::too_many_arguments)]
pub(super) fn reconcile_selected_primitive(
    node_mgr: Res<NodeManager>,
    nav: Res<NavigationManager>,
    inspector: Res<InspectorFormState>,
    texture_registry: Res<TextureRegistryRes>,
    mat_assets: Res<PrimitiveMaterialAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    mut primitives: Query<(&SpawnedNodeMarker, &mut PrimitiveNode, &mut Mesh3d, &mut MeshMaterial3d<StandardMaterial>)>,
    fallback_signs: Query<(Entity, &SpawnedNodeMarker, &Transform), (With<FallbackSign>, Without<PrimitiveNode>)>,
) {
    let Some(ref sel) = node_mgr.selected else { return };
    let Some(petal_id) = nav.active_petal_id.as_deref() else { return };
    let Some(raw) = inspector.node_properties.get(PRIMITIVE_PROPERTY_KEY) else { return };
    let Ok(descriptor) = PrimitiveDescriptor::from_json(raw) else { return };

    // FR-5: walls are path-driven, not dims-driven — they take a separate
    // promotion path (`promote_selected_wall`) and re-project via `wall_reconcile`
    // off Track-1 `DbResult` events, so they're excluded from the shape-primitive
    // mesh path below.
    if descriptor.kind == PrimitiveKind::Wall {
        return;
    }

    // Already-spawned primitive → reconcile in place (FR-2).
    let mut found = false;
    for (marker, mut prim, mut mesh3d, mut mat3d) in primitives.iter_mut() {
        if marker.node_id != sel.node_id || marker.petal_id != petal_id {
            continue;
        }
        found = true;
        if prim.descriptor == descriptor {
            continue; // unchanged — skip the remesh entirely
        }

        mesh3d.0 = meshes.add(build_primitive_mesh(&descriptor));

        if descriptor.texture_ref != prim.descriptor.texture_ref {
            mat3d.0 = resolve_primitive_material(
                descriptor.texture_ref.as_deref(),
                &texture_registry,
                &mat_assets,
                &mut materials,
                &mut images,
            );
        }

        prim.descriptor = descriptor.clone();
        bevy::log::debug!("Reconciled primitive node={} in place (FR-2)", marker.node_id);
    }
    if found {
        return;
    }

    // Not yet materialized as a primitive — promote from its fallback sign
    // (FR-1). Nodes without any spawned placeholder are covered on next
    // petal switch by `petal_respawn`.
    if let Some((entity, marker, transform)) =
        fallback_signs.iter().find(|(_, m, _)| m.node_id == sel.node_id && m.petal_id == petal_id)
    {
        // The fallback sign hovers 0.5 above the node's real Y (see
        // `spawn_fallback_sign`) — undo that offset for the primitive.
        let pos = [transform.translation.x, transform.translation.y - 0.5, transform.translation.z];
        let node_id = marker.node_id.clone();
        commands.entity(entity).despawn();
        let material = resolve_primitive_material(
            descriptor.texture_ref.as_deref(),
            &texture_registry,
            &mat_assets,
            &mut materials,
            &mut images,
        );
        spawn_primitive_entity(
            &mut commands,
            &mut meshes,
            &node_id,
            petal_id,
            &node_id,
            pos,
            descriptor,
            material,
        );
    }
}

/// Resolve a primitive's `texture_ref` to a `StandardMaterial` handle (FR-3):
/// look up the registry entry, load its blob(s) via `FsBlobStore`, assemble a
/// material. Falls back to the shared default when `texture_ref` is `None`
/// or unresolvable.
pub fn resolve_primitive_material(
    texture_ref: Option<&str>,
    registry: &TextureRegistryRes,
    default_assets: &PrimitiveMaterialAssets,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Handle<StandardMaterial> {
    let Some(id) = texture_ref else {
        return default_assets.default_material.clone();
    };
    let Some(entry) = registry.0.get(id) else {
        bevy::log::warn!("texture_ref '{}' not found in TextureRegistry — using default", id);
        return default_assets.default_material.clone();
    };

    let blob_store = FsBlobStore::open_default();
    let handle = MaterialHandle {
        entry_id: entry.id.clone(),
        albedo_hash: Some(entry.blob_hash.clone()),
        normal_hash: None,
        roughness_hash: None,
        ao_hash: None,
        metallic_hash: None,
        metadata: None,
    };
    let resolved = resolve_material_textures(&handle, &blob_store);

    let Some(albedo) = resolved.albedo else {
        bevy::log::warn!("texture_ref '{}' albedo blob unresolvable — using default", id);
        return default_assets.default_material.clone();
    };

    let image = Image::new(
        bevy::render::render_resource::Extent3d {
            width: albedo.width,
            height: albedo.height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        albedo.rgba,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    let image_handle = images.add(image);

    materials.add(StandardMaterial {
        base_color_texture: Some(image_handle),
        ..default()
    })
}

/// Decode a track's `gpx_points` JSON (`[[x, y, z, t], ...]`, petal-local
/// meters — the same shape `fractalengine/src/gpx_bridge.rs` writes) into the
/// `[x, y, z]` polyline `build_wall_mesh` consumes. Malformed/short entries are
/// skipped best-effort, mirroring the bridge's own `json_to_route_points`.
fn decode_gpx_points(value: &serde_json::Value) -> Vec<[f32; 3]> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let a = entry.as_array()?;
                    let x = a.first()?.as_f64()? as f32;
                    let y = a.get(1)?.as_f64()? as f32;
                    let z = a.get(2)?.as_f64()? as f32;
                    Some([x, y, z])
                })
                .collect()
        })
        .unwrap_or_default()
}

/// FR-5 (P3.2): materialize the selected node as a wall (promote from its
/// fallback sign the first time a `Wall` descriptor is seen) and request the
/// `source_path` track's points so `wall_reconcile` can fill the geometry. A
/// wall spawns with empty geometry immediately, then re-projects once the
/// Track-1 `NodePropertiesLoaded` for its source arrives.
#[allow(clippy::too_many_arguments)]
pub(super) fn promote_selected_wall(
    node_mgr: Res<NodeManager>,
    nav: Res<NavigationManager>,
    inspector: Res<InspectorFormState>,
    texture_registry: Res<TextureRegistryRes>,
    mat_assets: Res<PrimitiveMaterialAssets>,
    db_tx: Res<DbCommandSender>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    walls: Query<&SpawnedNodeMarker, With<WallNode>>,
    fallback_signs: Query<(Entity, &SpawnedNodeMarker), (With<FallbackSign>, Without<WallNode>)>,
) {
    let Some(ref sel) = node_mgr.selected else { return };
    let Some(petal_id) = nav.active_petal_id.as_deref() else { return };
    let Some(raw) = inspector.node_properties.get(PRIMITIVE_PROPERTY_KEY) else { return };
    let Ok(descriptor) = PrimitiveDescriptor::from_json(raw) else { return };
    if descriptor.kind != PrimitiveKind::Wall {
        return;
    }
    let Some(source_path) = descriptor.source_path.as_deref() else {
        bevy::log::warn!("wall node {} has no source_path — cannot extrude", sel.node_id);
        return;
    };

    // Already spawned as a wall for this petal → nothing to promote; geometry
    // updates are handled by `wall_reconcile` on the source's point changes.
    if walls
        .iter()
        .any(|m| m.node_id == sel.node_id && m.petal_id == petal_id)
    {
        return;
    }

    let height = descriptor.dims.first().copied().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(2.0);
    let material = resolve_primitive_material(
        descriptor.texture_ref.as_deref(),
        &texture_registry,
        &mat_assets,
        &mut materials,
        &mut images,
    );

    // Despawn the placeholder fallback sign if one exists for this node.
    if let Some((entity, _)) = fallback_signs
        .iter()
        .find(|(_, m)| m.node_id == sel.node_id && m.petal_id == petal_id)
    {
        commands.entity(entity).despawn();
    }

    // Spawn with empty geometry; the source's `NodePropertiesLoaded` fills it.
    spawn_wall_entity(
        &mut commands,
        &mut meshes,
        &sel.node_id,
        petal_id,
        &sel.node_id,
        source_path,
        &[],
        height,
        material,
    );

    // Ask for the source track's current points so the wall projects now
    // rather than only on the next petal-load batch.
    db_tx
        .0
        .send(DbCommand::GetNodeProperties { node_id: source_path.to_string() })
        .ok();
}

/// FR-5 (P3.2): re-project every wall bound to a source path whenever that
/// path changes, and despawn walls whose source is deleted — driven entirely
/// by **Track 1's** existing `DbResult` lifecycle events (no new notification
/// path, no `gpx_bridge` edit):
///
/// - `NodePropertiesLoaded { node_id, properties }` — fired by Track 1's
///   petal-load batch (`request_petal_gpx_materialization`) and every live
///   path edit (`persist_and_render_points` → DB re-emits). Any wall whose
///   `source_path == node_id` rebuilds its mesh from the fresh `gpx_points`.
/// - `NodeDeleted { node_id, .. }` — any wall whose `source_path == node_id`
///   is despawned (its shape driver is gone).
pub(super) fn wall_reconcile(
    mut db_results: MessageReader<DbResult>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
    mut walls: Query<(Entity, &SpawnedNodeMarker, &WallNode, &mut Mesh3d)>,
) {
    for result in db_results.read() {
        match result {
            DbResult::NodePropertiesLoaded { node_id, properties } => {
                let Some(points_json) = properties.get(GPX_POINTS_KEY) else { continue };
                let polyline = decode_gpx_points(points_json);
                for (_, _, wall, mut mesh3d) in walls.iter_mut() {
                    if wall.source_path != *node_id {
                        continue;
                    }
                    mesh3d.0 = meshes.add(build_wall_mesh(&polyline, wall.height));
                    bevy::log::debug!(
                        "Re-projected wall (source_path={}) from {} points",
                        node_id, polyline.len()
                    );
                }
            }
            DbResult::NodeDeleted { node_id, .. } => {
                for (entity, _, wall, _) in walls.iter() {
                    if wall.source_path == *node_id {
                        commands.entity(entity).despawn();
                        bevy::log::info!("Despawned wall whose source_path {} was deleted", node_id);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::primitive::PrimitiveKind;

    #[test]
    fn descriptor_equality_gates_reconcile() {
        let a = PrimitiveDescriptor {
            kind: PrimitiveKind::Cube,
            dims: vec![1.0, 1.0, 1.0],
            texture_ref: None,
            source_path: None,
        };
        let b = a.clone();
        assert_eq!(a, b, "identical descriptors must compare equal so reconcile skips a no-op remesh");

        let mut c = a.clone();
        c.dims[0] = 2.0;
        assert_ne!(a, c, "changed dims must be detected by the reconcile diff");
    }

    #[test]
    fn wall_descriptor_change_detected_by_source_path() {
        // FR-5: changing which path drives a wall must register as a descriptor
        // change (so the reconcile machinery rebuilds against the new source).
        let a = PrimitiveDescriptor {
            kind: PrimitiveKind::Wall,
            dims: vec![3.0],
            texture_ref: None,
            source_path: Some("track-a".to_string()),
        };
        let mut b = a.clone();
        b.source_path = Some("track-b".to_string());
        assert_ne!(a, b, "a different source_path must be a detectable change");
    }

    #[test]
    fn decode_gpx_points_maps_xyz_and_skips_short_entries() {
        // `gpx_points` is `[[x, y, z, t], ...]`; the wall polyline drops `t`.
        let value = serde_json::json!([
            [0.0, 1.0, 2.0, 0.0],
            [3.0, 4.0, 5.0, 10.0],
            [6.0, 7.0], // too short — skipped
        ]);
        let pts = decode_gpx_points(&value);
        assert_eq!(pts, vec![[0.0, 1.0, 2.0], [3.0, 4.0, 5.0]]);
    }

    #[test]
    fn decode_gpx_points_empty_for_non_array() {
        assert!(decode_gpx_points(&serde_json::json!(null)).is_empty());
        assert!(decode_gpx_points(&serde_json::json!({"a": 1})).is_empty());
    }
}
