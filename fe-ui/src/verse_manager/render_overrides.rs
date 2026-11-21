//! Material/shader (`render.style`), lighting/atmosphere (`render.environment`) and
//! generated geometry (`render.mesh`) reconcilers driven by committed node properties via
//! [`fe_sdk`]. See `AGENTS.md` §render-mesh.

use std::collections::{BTreeMap, HashMap};

use bevy::camera::primitives::Aabb;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use fe_renderer::camera::OrbitCameraController;
use fe_sdk::render_environment::{RenderEnvironmentDescriptor, RENDER_ENVIRONMENT_PROPERTY_KEY};
use fe_sdk::render_mesh::{RenderMeshDescriptor, RENDER_MESH_PROPERTY_KEY};
use fe_sdk::render_style::{RenderAlphaMode, RenderStyleDescriptor, RENDER_STYLE_PROPERTY_KEY};
use serde_json::Value;

use super::spawn::{build_primitive_mesh, FallbackSign, PrimitiveNode, ResidentNode};
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

const DEFAULT_CLEAR_RGB: [f32; 3] = [0.08, 0.08, 0.10];
const DEFAULT_AMBIENT_BRIGHTNESS: f32 = 200.0;
const DEFAULT_SUN_ILLUMINANCE: f32 = 5000.0;

/// Cached `render.style`, `render.environment` and `render.mesh` descriptors keyed by `node_id`.
#[derive(Resource, Default)]
pub struct RenderOverrideCache {
    styles: HashMap<String, RenderStyleDescriptor>,
    environments: BTreeMap<String, (Option<String>, RenderEnvironmentDescriptor)>,
    /// Valid `render.mesh` values; compared by value, so they never bump `revision`.
    meshes: HashMap<String, RenderMeshDescriptor>,
    revision: u64,
    applied_env_key: Option<(Option<String>, u64)>,
}

impl RenderOverrideCache {
    /// Updates cached style/environment overrides from a node's property bag.
    pub fn note_properties(&mut self, node_id: &str, petal_id: Option<&str>, properties: &Value) {
        let mut changed = false;

        let next_style = properties
            .get(RENDER_STYLE_PROPERTY_KEY)
            .and_then(RenderStyleDescriptor::from_json);
        match next_style {
            Some(style) => {
                if self.styles.get(node_id) != Some(&style) {
                    self.styles.insert(node_id.to_owned(), style);
                    changed = true;
                }
            }
            None => {
                if self.styles.remove(node_id).is_some() {
                    changed = true;
                }
            }
        }

        let next_env = properties
            .get(RENDER_ENVIRONMENT_PROPERTY_KEY)
            .and_then(RenderEnvironmentDescriptor::from_json);
        let owned_petal = petal_id.map(str::to_owned);
        match next_env {
            Some(env) => {
                let next_entry = (owned_petal, env);
                if self.environments.get(node_id) != Some(&next_entry) {
                    self.environments.insert(node_id.to_owned(), next_entry);
                    changed = true;
                }
            }
            None => {
                if self.environments.remove(node_id).is_some() {
                    changed = true;
                }
            }
        }

        if changed {
            self.revision = self.revision.saturating_add(1);
        }

        // A refused value (e.g. from a remote peer) is ignored and logged, never applied.
        let next_mesh = match properties.get(RENDER_MESH_PROPERTY_KEY) {
            None | Some(Value::Null) => None,
            Some(raw) => match RenderMeshDescriptor::parse(raw) {
                Ok(mesh) => Some(mesh),
                Err(refusal) => {
                    bevy::log::warn!("ignoring render.mesh on node {node_id}: {refusal}");
                    None
                }
            },
        };
        match next_mesh {
            Some(mesh) => {
                if self.meshes.get(node_id) != Some(&mesh) {
                    self.meshes.insert(node_id.to_owned(), mesh);
                }
            }
            None => {
                self.meshes.remove(node_id);
            }
        }
    }

    /// Removes any cached `render.mesh` descriptor for `node_id`.
    pub fn invalidate_mesh(&mut self, node_id: &str) {
        self.meshes.remove(node_id);
    }

    /// Returns the cached, validated `render.mesh` descriptor for `node_id`, if any.
    pub fn mesh_for(&self, node_id: &str) -> Option<&RenderMeshDescriptor> {
        self.meshes.get(node_id)
    }

    /// Removes any cached `render.style` override for `node_id`.
    pub fn invalidate_style(&mut self, node_id: &str) {
        if self.styles.remove(node_id).is_some() {
            self.revision = self.revision.saturating_add(1);
        }
    }

    /// Removes any cached `render.environment` override for `node_id`.
    pub fn invalidate_environment(&mut self, node_id: &str) {
        if self.environments.remove(node_id).is_some() {
            self.revision = self.revision.saturating_add(1);
        }
    }

    /// Removes all cached overrides for `node_id`.
    pub fn remove_node(&mut self, node_id: &str) {
        let a = self.styles.remove(node_id).is_some();
        let b = self.environments.remove(node_id).is_some();
        self.meshes.remove(node_id);
        if a || b {
            self.revision = self.revision.saturating_add(1);
        }
    }

    /// Clears all cached overrides (workspace switch / reset).
    pub fn clear(&mut self) {
        self.meshes.clear();
        if !self.styles.is_empty() || !self.environments.is_empty() {
            self.styles.clear();
            self.environments.clear();
            self.revision = self.revision.saturating_add(1);
        }
    }

    /// Returns the cached `render.style` override for `node_id`, if any.
    pub fn style_for(&self, node_id: &str) -> Option<&RenderStyleDescriptor> {
        self.styles.get(node_id)
    }

    /// Returns the deterministic active `render.environment` override for `petal_id`, if any.
    pub fn environment_for_petal(&self, petal_id: Option<&str>) -> Option<&RenderEnvironmentDescriptor> {
        let active = petal_id?;
        self.environments
            .values()
            .find(|(entry_petal, _)| entry_petal.as_deref() == Some(active))
            .map(|(_, env)| env)
    }
}

/// Per-mesh material bookkeeping so styled entities never mutate shared default handles.
#[derive(Component)]
pub struct RenderStyleState {
    owner_node_id: String,
    original_material: Handle<StandardMaterial>,
    owned_material: Handle<StandardMaterial>,
    baseline: StandardMaterial,
    base_emissive: LinearRgba,
    pulse_hz: Option<f32>,
    applied_revision: u64,
}

fn collect_subtree_entities(
    root: Entity,
    children_q: &Query<&Children>,
    out: &mut Vec<Entity>,
) {
    out.clear();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        out.push(entity);
        if let Ok(children) = children_q.get(entity) {
            for child in children.iter() {
                stack.push(child);
            }
        }
    }
}

fn apply_style_to_material(mat: &mut StandardMaterial, style: &RenderStyleDescriptor) {
    if let Some([r, g, b, a]) = style.base_color {
        mat.base_color = Color::srgba(r, g, b, a);
    }
    if let Some([r, g, b]) = style.emissive {
        mat.emissive = LinearRgba::rgb(r, g, b);
    }
    if let Some(m) = style.metallic {
        mat.metallic = m;
    }
    if let Some(r) = style.perceptual_roughness {
        mat.perceptual_roughness = r;
    }
    if let Some(unlit) = style.unlit {
        mat.unlit = unlit;
    }
    if let Some(alpha_mode) = style.alpha_mode {
        mat.alpha_mode = match alpha_mode {
            RenderAlphaMode::Opaque => AlphaMode::Opaque,
            RenderAlphaMode::Blend => AlphaMode::Blend,
            RenderAlphaMode::Mask => AlphaMode::Mask(0.5),
        };
    }
}

/// Marker on every entity generated from a node's `render.mesh` (always a leaf child).
#[derive(Component)]
pub struct RenderMeshPart;

/// Bookkeeping on a node entity that currently owns generated `render.mesh` children.
#[derive(Component)]
pub struct RenderMeshState {
    descriptor: RenderMeshDescriptor,
    children: Vec<Entity>,
    /// The node's own placeholder mesh (fallback sign or `primitive`), hidden while generated geometry stands in.
    suppressed_mesh: Option<Mesh3d>,
}

/// Materializes cached `render.mesh` descriptors as leaf children of each tree-node entity,
/// rebuilding only the nodes whose descriptor changed and despawning on removal.
/// Precedence: GLTF asset > `render.mesh` > `primitive` > fallback sign (see `AGENTS.md` §render-mesh).
#[allow(clippy::type_complexity)]
pub fn reconcile_render_meshes(
    mut commands: Commands,
    cache: Res<RenderOverrideCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    owners_q: Query<(), With<RenderMeshState>>,
    mut nodes_q: Query<
        (
            Entity,
            &SpawnedNodeMarker,
            Option<&mut RenderMeshState>,
            Option<&mut Mesh3d>,
            Has<FallbackSign>,
            Has<PrimitiveNode>,
            Has<SceneRoot>,
        ),
        With<ResidentNode>,
    >,
) {
    if cache.meshes.is_empty() && owners_q.is_empty() {
        return;
    }
    for (entity, marker, mut state, mut own_mesh, is_sign, is_primitive, is_asset) in nodes_q.iter_mut() {
        // A GLTF-backed node never draws generated geometry.
        let wanted = if is_asset { None } else { cache.mesh_for(&marker.node_id) };
        let unchanged = match (wanted, state.as_deref()) {
            (None, None) => true,
            (Some(descriptor), Some(owned)) => owned.descriptor == *descriptor,
            _ => false,
        };
        if unchanged {
            // A primitive reconcile may swap a fresh mesh in; keep it as the placeholder and hide it again.
            if let (true, Some(owned), Some(mesh)) = (is_primitive, state.as_mut(), own_mesh.as_mut()) {
                if mesh.0 != Handle::default() {
                    owned.suppressed_mesh = Some(Mesh3d(std::mem::take(&mut mesh.0)));
                    commands.entity(entity).remove::<Aabb>();
                }
            }
            continue;
        }

        let placeholder = state
            .as_deref()
            .and_then(|owned| owned.suppressed_mesh.clone())
            .or_else(|| if is_sign || is_primitive { own_mesh.as_deref().cloned() } else { None });
        if let Some(owned) = state.as_deref() {
            for child in &owned.children {
                // Generated children are leaves, so a plain despawn is complete.
                commands.entity(*child).try_despawn();
            }
        }

        let Some(descriptor) = wanted else {
            commands.entity(entity).remove::<RenderMeshState>();
            if let (false, Some(placeholder)) = (is_asset, placeholder) {
                commands.entity(entity).insert(placeholder);
            }
            continue;
        };

        let mut children = Vec::with_capacity(descriptor.parts.len());
        for part in &descriptor.parts {
            let [red, green, blue] = part.color;
            let [offset_x, offset_y, offset_z] = part.offset;
            let child = commands
                .spawn((
                    Mesh3d(meshes.add(build_primitive_mesh(&part.to_primitive()))),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(red, green, blue),
                        perceptual_roughness: 0.8,
                        ..default()
                    })),
                    Transform {
                        translation: Vec3::new(offset_x, offset_y + part.centre_lift(), offset_z),
                        rotation: Quat::from_rotation_y(part.yaw_deg.to_radians()),
                        ..default()
                    },
                    ChildOf(entity),
                    RenderMeshPart,
                    Name::new("render.mesh part"),
                ))
                .id();
            children.push(child);
        }
        if placeholder.is_some() {
            // The placeholder's bounds would keep its old volume pickable; Bevy recomputes them on restore.
            commands.entity(entity).remove::<Aabb>();
            if is_primitive {
                // A primitive keeps its Mesh3d (the primitive materializer matches on it) but draws nothing.
                if let Some(mesh) = own_mesh.as_mut() {
                    if mesh.0 != Handle::default() {
                        mesh.0 = Handle::default();
                    }
                }
            } else {
                commands.entity(entity).remove::<Mesh3d>();
            }
        }
        commands.entity(entity).insert(RenderMeshState {
            descriptor: descriptor.clone(),
            children,
            suppressed_mesh: placeholder,
        });
    }
}

/// Applies cached `render.style` descriptors onto resident node meshes (including GLTF children)
/// and restores original materials when overrides are deleted.
pub fn reconcile_render_styles(
    mut commands: Commands,
    cache: Res<RenderOverrideCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    nodes_q: Query<(Entity, &SpawnedNodeMarker)>,
    children_q: Query<&Children>,
    mut styled_q: Query<(Entity, &mut RenderStyleState, &mut MeshMaterial3d<StandardMaterial>)>,
    mut restored_q: Query<&mut MeshMaterial3d<StandardMaterial>, Without<RenderStyleState>>,
) {
    // 1. Restore any mesh entities whose owning node no longer has a style override.
    for (mesh_entity, state, mut mat_comp) in styled_q.iter_mut() {
        if cache.style_for(&state.owner_node_id).is_none() {
            mat_comp.0 = state.original_material.clone();
            commands.entity(mesh_entity).remove::<RenderStyleState>();
        }
    }

    if cache.styles.is_empty() {
        return;
    }

    let mut mesh_entities = Vec::new();
    for (root, resident) in nodes_q.iter() {
        let Some(style) = cache.style_for(&resident.node_id) else {
            continue;
        };
        collect_subtree_entities(root, &children_q, &mut mesh_entities);
        for &mesh_entity in &mesh_entities {
            if let Ok((_, mut state, _)) = styled_q.get_mut(mesh_entity) {
                if state.applied_revision != cache.revision {
                    if let Some(asset) = materials.get_mut(&state.owned_material) {
                        *asset = state.baseline.clone();
                        apply_style_to_material(asset, style);
                        state.base_emissive = asset.emissive;
                    }
                    state.pulse_hz = style.pulse_hz.filter(|&hz| hz > 0.0);
                    state.applied_revision = cache.revision;
                }
                continue;
            }

            let Ok(mut mat_comp) = restored_q.get_mut(mesh_entity) else {
                continue;
            };
            let Some(baseline) = materials.get(&mat_comp.0).cloned() else {
                continue;
            };
            let mut updated = baseline.clone();
            apply_style_to_material(&mut updated, style);
            let base_emissive = updated.emissive;
            let owned_handle = materials.add(updated);
            let original_handle = mat_comp.0.clone();
            mat_comp.0 = owned_handle.clone();
            commands.entity(mesh_entity).insert(RenderStyleState {
                owner_node_id: resident.node_id.clone(),
                original_material: original_handle,
                owned_material: owned_handle,
                baseline,
                base_emissive,
                pulse_hz: style.pulse_hz.filter(|&hz| hz > 0.0),
                applied_revision: cache.revision,
            });
        }
    }
}

/// Modulates emissive intensity smoothly for nodes whose `render.style` specifies `pulse_hz > 0`.
pub fn pulse_render_styles_system(
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    query: Query<(&RenderStyleState, &MeshMaterial3d<StandardMaterial>)>,
) {
    let elapsed = time.elapsed_secs();
    for (state, mat_comp) in query.iter() {
        let Some(hz) = state.pulse_hz else {
            continue;
        };
        if hz <= 0.0 {
            continue;
        }
        let Some(mat) = materials.get_mut(&mat_comp.0) else {
            continue;
        };
        let wave = 0.5 + 0.5 * (elapsed * hz * std::f32::consts::TAU).sin();
        let gain = 0.25 + 0.75 * wave;
        mat.emissive = state.base_emissive * gain;
    }
}

/// Applies the active petal's `render.environment` descriptor to scene ambient/directional
/// lighting, viewport clear color, and camera fog.
pub fn reconcile_render_environment(
    mut commands: Commands,
    mut cache: ResMut<RenderOverrideCache>,
    nav: Option<Res<NavigationManager>>,
    mut ambient: Option<ResMut<GlobalAmbientLight>>,
    mut clear: Option<ResMut<ClearColor>>,
    mut dir_lights: Query<(&mut DirectionalLight, &mut Transform)>,
    cameras: Query<Entity, With<OrbitCameraController>>,
) {
    let active_petal = nav.as_ref().and_then(|n| n.active_petal_id.clone());
    let next_key = Some((active_petal.clone(), cache.revision));
    if cache.applied_env_key == next_key {
        return;
    }
    if !dir_lights.is_empty() && !cameras.is_empty() {
        cache.applied_env_key = next_key;
    }

    let env = cache.environment_for_petal(active_petal.as_deref()).cloned();

    if let Some(ref mut ambient_res) = ambient {
        ambient_res.color = env
            .as_ref()
            .and_then(|e| e.ambient_color)
            .map(|[r, g, b]| Color::srgb(r, g, b))
            .unwrap_or(Color::WHITE);
        ambient_res.brightness = env
            .as_ref()
            .and_then(|e| e.ambient_brightness)
            .unwrap_or(DEFAULT_AMBIENT_BRIGHTNESS);
    }

    if let Some(ref mut clear_res) = clear {
        let [r, g, b] = env
            .as_ref()
            .and_then(|e| e.clear_color)
            .unwrap_or(DEFAULT_CLEAR_RGB);
        clear_res.0 = Color::srgb(r, g, b);
    }

    let sun_color = env
        .as_ref()
        .and_then(|e| e.sun_color)
        .map(|[r, g, b]| Color::srgb(r, g, b))
        .unwrap_or(Color::WHITE);
    let sun_illuminance = env
        .as_ref()
        .and_then(|e| e.sun_illuminance)
        .unwrap_or(DEFAULT_SUN_ILLUMINANCE);
    let default_rot = Quat::from_euler(EulerRot::XYZ, -0.8, 0.5, 0.0);

    for (mut light, mut tx) in dir_lights.iter_mut() {
        light.color = sun_color;
        light.illuminance = sun_illuminance;
        if let Some([dx, dy, dz]) = env.as_ref().and_then(|e| e.sun_direction) {
            let dir = Vec3::new(dx, dy, dz).normalize_or_zero();
            if dir.length_squared() > 0.0 {
                let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
                tx.look_to(dir, up);
            } else {
                tx.rotation = default_rot;
            }
        } else {
            tx.rotation = default_rot;
        }
    }

    let fog_density = env.as_ref().and_then(|e| e.fog_density).unwrap_or(0.0);
    let fog_color = env.as_ref().and_then(|e| e.fog_color);
    if fog_density > 0.0 || fog_color.is_some() {
        let [r, g, b] = fog_color.unwrap_or(DEFAULT_CLEAR_RGB);
        let alpha = if fog_density <= 0.0 { 0.0 } else { 1.0 };
        let density = fog_density.max(0.0);
        for cam in cameras.iter() {
            commands.entity(cam).insert(DistanceFog {
                color: Color::srgba(r, g, b, alpha),
                directional_light_color: Color::NONE,
                directional_light_exponent: 8.0,
                falloff: FogFalloff::Exponential { density },
            });
        }
    } else {
        for cam in cameras.iter() {
            commands.entity(cam).remove::<DistanceFog>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cache_tracks_and_invalidates_style_and_environment() {
        let mut cache = RenderOverrideCache::default();
        let props = json!({
            "render.style": {
                "base_color": [0.9, 0.1, 0.2, 1.0],
                "emissive": [2.0, 0.2, 0.0],
                "pulse_hz": 1.5
            },
            "render.environment": {
                "ambient_brightness": 350.0,
                "sun_illuminance": 12000.0
            }
        });

        cache.note_properties("node-1", Some("petal-a"), &props);
        let rev_after_insert = cache.revision;
        assert!(cache.style_for("node-1").is_some());
        assert!(cache.environment_for_petal(Some("petal-a")).is_some());
        assert!(cache.environment_for_petal(Some("petal-b")).is_none());

        // Idempotent repeat does not bump revision.
        cache.note_properties("node-1", Some("petal-a"), &props);
        assert_eq!(cache.revision, rev_after_insert);

        cache.invalidate_style("node-1");
        assert!(cache.style_for("node-1").is_none());
        assert!(cache.revision > rev_after_insert);
        assert!(cache.environment_for_petal(Some("petal-a")).is_some());

        // render.mesh: valid value cached, refused value (unknown shape) ignored like absence.
        let mesh = |shape: &str| json!({ "render.mesh": { "v": 1, "parts": [{ "shape": shape, "radius": 1.0 }] } });
        cache.note_properties("node-1", Some("petal-a"), &mesh("sphere"));
        assert_eq!(cache.mesh_for("node-1").map(|m| m.parts.len()), Some(1));
        cache.note_properties("node-1", Some("petal-a"), &mesh("cone"));
        assert!(cache.mesh_for("node-1").is_none(), "refused render.mesh is ignored");
    }

    #[test]
    fn reconcile_clones_shared_material_per_styled_entity_and_restores() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>();
        app.init_resource::<RenderOverrideCache>();

        let shared = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.5, 0.5),
                ..default()
            });

        let n1 = app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: "n1".into(),
                    petal_id: "p1".into(),
                },
                MeshMaterial3d(shared.clone()),
            ))
            .id();
        let n2 = app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: "n2".into(),
                    petal_id: "p1".into(),
                },
                MeshMaterial3d(shared.clone()),
            ))
            .id();

        app.world_mut()
            .resource_mut::<RenderOverrideCache>()
            .note_properties(
                "n1",
                Some("p1"),
                &json!({
                    "render.style": {
                        "base_color": [1.0, 0.0, 0.0, 1.0],
                        "metallic": 0.9
                    }
                }),
            );
        app.add_systems(Update, reconcile_render_styles);
        app.update();

        let h1 = app.world().get::<MeshMaterial3d<StandardMaterial>>(n1).unwrap().0.clone();
        let h2 = app.world().get::<MeshMaterial3d<StandardMaterial>>(n2).unwrap().0.clone();
        assert_ne!(h1, shared, "styled node must receive an owned material handle");
        assert_eq!(h2, shared, "sibling node keeps the untouched shared handle");

        let mats = app.world().resource::<Assets<StandardMaterial>>();
        assert_eq!(mats.get(&shared).unwrap().base_color, Color::srgb(0.5, 0.5, 0.5));
        assert_eq!(mats.get(&h1).unwrap().metallic, 0.9);

        // Removing the override restores the original shared handle.
        app.world_mut()
            .resource_mut::<RenderOverrideCache>()
            .invalidate_style("n1");
        app.update();
        let h1_after = app.world().get::<MeshMaterial3d<StandardMaterial>>(n1).unwrap().0.clone();
        assert_eq!(h1_after, shared);
        assert!(app.world().get::<RenderStyleState>(n1).is_none());

        // render.mesh journey on n2: materialize, rebuild on change, refuse 33 parts, despawn on removal.
        app.world_mut().entity_mut(n1).insert(ResidentNode);
        app.world_mut().entity_mut(n2).insert(ResidentNode);
        app.init_resource::<Assets<Mesh>>();
        app.add_systems(Update, reconcile_render_meshes);
        app.add_systems(PostUpdate, bevy::camera::visibility::calculate_bounds);
        let part_count = |app: &mut App, owner: Entity| {
            let world = app.world_mut();
            let mut parts = world.query_filtered::<&ChildOf, With<RenderMeshPart>>();
            parts.iter(world).filter(|child| child.parent() == owner).count()
        };
        let mesh_props = |count: usize| {
            json!({ "render.mesh": { "v": 1, "parts": vec![json!({ "shape": "box", "size": [2.0, 3.0, 2.0] }); count] } })
        };
        let note = |app: &mut App, props: &Value| {
            app.world_mut().resource_mut::<RenderOverrideCache>().note_properties("n2", Some("p1"), props);
        };

        note(&mut app, &mesh_props(2));
        app.update();
        assert_eq!(part_count(&mut app, n2), 2, "descriptor materializes one child per part");
        assert_eq!(part_count(&mut app, n1), 0, "sibling node is untouched");
        let first_children: Vec<Entity> = app.world().get::<RenderMeshState>(n2).unwrap().children.clone();

        note(&mut app, &mesh_props(3));
        app.update();
        assert_eq!(part_count(&mut app, n2), 3, "changed descriptor rebuilds only n2's children");
        assert!(first_children.iter().all(|old| app.world().get_entity(*old).is_err()), "old children despawned");

        note(&mut app, &mesh_props(33));
        app.update();
        assert_eq!(part_count(&mut app, n2), 0, "33 parts is refused, so nothing is drawn");

        note(&mut app, &mesh_props(1));
        app.update();
        assert_eq!(part_count(&mut app, n2), 1);
        app.world_mut().resource_mut::<RenderOverrideCache>().invalidate_mesh("n2");
        app.update();
        assert_eq!(part_count(&mut app, n2), 0, "removal despawns the generated children");
        assert!(app.world().get::<RenderMeshState>(n2).is_none());

        // Precedence: render.mesh hides a primitive's own mesh, survives a primitive re-mesh, and restores it on removal.
        let cube = fe_sdk::primitive::PrimitiveDescriptor {
            kind: fe_sdk::primitive::PrimitiveKind::Cube,
            dims: vec![1.0, 1.0, 1.0],
            texture_ref: None,
        };
        let primitive_mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(build_primitive_mesh(&cube));
        app.world_mut().entity_mut(n2).insert((PrimitiveNode { descriptor: cube.clone() }, Mesh3d(primitive_mesh.clone())));
        let drawn = |app: &App| app.world().get::<Mesh3d>(n2).map(|mesh| mesh.0.clone());
        app.update();
        assert!(app.world().get::<Aabb>(n2).is_some(), "the primitive starts with pickable bounds");
        note(&mut app, &mesh_props(1));
        app.update();
        assert_eq!(part_count(&mut app, n2), 1);
        assert_eq!(drawn(&app), Some(Handle::default()), "the primitive keeps Mesh3d but draws nothing");
        assert!(app.world().get::<Aabb>(n2).is_none(), "the hidden primitive volume is not pickable");
        let remeshed = app.world_mut().resource_mut::<Assets<Mesh>>().add(build_primitive_mesh(&cube));
        app.world_mut().get_mut::<Mesh3d>(n2).unwrap().0 = remeshed.clone();
        app.update();
        assert_eq!(drawn(&app), Some(Handle::default()), "a primitive re-mesh is hidden again");
        assert!(app.world().get::<Aabb>(n2).is_none(), "and its bounds stay removed");
        app.world_mut().resource_mut::<RenderOverrideCache>().invalidate_mesh("n2");
        app.update();
        assert_eq!(drawn(&app), Some(remeshed), "removal restores the latest primitive mesh");
        assert!(app.world().get::<Aabb>(n2).is_some(), "restored bounds make the primitive pickable again");

        // Precedence: a GLTF-backed node drops generated parts; ribbons and stamps never receive any.
        note(&mut app, &mesh_props(1));
        app.update();
        assert_eq!(part_count(&mut app, n2), 1);
        app.world_mut().entity_mut(n2).insert(bevy::scene::SceneRoot::default());
        app.update();
        assert_eq!(part_count(&mut app, n2), 0, "GLTF asset > render.mesh: owned parts despawn");
        assert!(app.world().get::<RenderMeshState>(n2).is_none());
        let ribbon = app
            .world_mut()
            .spawn(SpawnedNodeMarker { node_id: "ribbon".into(), petal_id: "p1".into() })
            .id();
        app.world_mut().resource_mut::<RenderOverrideCache>().note_properties("ribbon", Some("p1"), &mesh_props(1));
        app.update();
        assert_eq!(part_count(&mut app, ribbon), 0, "a marker without ResidentNode is not a tree node");
    }
}
