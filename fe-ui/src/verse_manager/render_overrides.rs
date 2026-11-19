//! Material/shader (`render.style`) and lighting/atmosphere (`render.environment`)
//! reconcilers driven by committed node properties via [`fe_sdk`].

use std::collections::{BTreeMap, HashMap};

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use fe_renderer::camera::OrbitCameraController;
use fe_sdk::render_environment::{RenderEnvironmentDescriptor, RENDER_ENVIRONMENT_PROPERTY_KEY};
use fe_sdk::render_style::{RenderAlphaMode, RenderStyleDescriptor, RENDER_STYLE_PROPERTY_KEY};
use serde_json::Value;

use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

const DEFAULT_CLEAR_RGB: [f32; 3] = [0.08, 0.08, 0.10];
const DEFAULT_AMBIENT_BRIGHTNESS: f32 = 200.0;
const DEFAULT_SUN_ILLUMINANCE: f32 = 5000.0;

/// Cached `render.style` and `render.environment` descriptors keyed by `node_id`.
#[derive(Resource, Default)]
pub struct RenderOverrideCache {
    styles: HashMap<String, RenderStyleDescriptor>,
    environments: BTreeMap<String, (Option<String>, RenderEnvironmentDescriptor)>,
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
        if a || b {
            self.revision = self.revision.saturating_add(1);
        }
    }

    /// Clears all cached overrides (workspace switch / reset).
    pub fn clear(&mut self) {
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
    }
}
