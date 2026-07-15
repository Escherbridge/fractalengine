//! Live primitive reconcile (FR-2) + texture resolution (FR-3): watches the
//! currently-selected node's inspector-loaded properties for a `primitive`
//! JSON descriptor and re-meshes/re-materializes the spawned entity in
//! place when `dims`/`kind`/`texture_ref` change, instead of a full respawn.
//! See `fe-ui/src/verse_manager/AGENTS.md` §primitives.

use bevy::prelude::*;
use fe_hexon::handlers::material::{resolve_material_textures, MaterialHandle};
use fe_hexon::registry::FsBlobStore;
use fe_sdk::primitive::{PrimitiveDescriptor, PRIMITIVE_PROPERTY_KEY};

use super::spawn::{build_primitive_mesh, spawn_primitive_entity, FallbackSign, PrimitiveNode};
use super::TextureRegistryRes;
use crate::node_manager::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{InspectorFormState, SpawnedNodeMarker};

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
    // (FR-1 selection path). Petal-wide coverage without selection lives in
    // `primitive_materialize::materialize_cached_primitives`.
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
        };
        let b = a.clone();
        assert_eq!(a, b, "identical descriptors must compare equal so reconcile skips a no-op remesh");

        let mut c = a.clone();
        c.dims[0] = 2.0;
        assert_ne!(a, c, "changed dims must be detected by the reconcile diff");
    }
}
