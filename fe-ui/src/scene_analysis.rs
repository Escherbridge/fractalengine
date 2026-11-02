//! Scene snapshot and scoped material cache for terrain reports; see src/AGENTS.md.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use fe_renderer::scene_analysis::{
    ObjectGeometry, SceneAnalysisSnapshot, SceneObjectBounds, SCENE_OBJECT_LIMIT,
};
use fe_runtime::messages::{DbCommand, DbResult};
use fe_runtime::scene_materials::{NodeMaterial, MATERIAL_BATCH_LIMIT};
use fe_sdk::primitive::PrimitiveKind;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Instant;

use crate::navigation_manager::NavigationManager;
use crate::plugin::{LocalUserRole, SpawnedNodeMarker};
use crate::verse_manager::{PrimitiveDescriptorCache, ResidentNode, VerseManager};

#[derive(Resource, Default)]
pub(crate) struct SceneMaterialCache {
    petal_id: Option<String>,
    scope: Option<String>,
    values: HashMap<String, NodeMaterial>,
    pending: Option<(String, Vec<String>, f64)>,
    retry_after: f64,
    next_sample: f64,
}

fn invalidate_changed_material(
    cache: &mut SceneMaterialCache,
    node_id: &str,
    properties: Option<&serde_json::Map<String, serde_json::Value>>,
) {
    invalidate_pending(cache, node_id);
    let Some(previous) = cache.values.get(node_id) else {
        return;
    };
    let json = properties
        .and_then(|properties| properties.get("material"))
        .map(serde_json::Value::to_string);
    let truncated = json
        .as_ref()
        .is_some_and(|json| json.len() > fe_runtime::scene_materials::MATERIAL_JSON_BYTES);
    let has_primitive = properties.is_some_and(|properties| properties.contains_key("primitive"));
    if properties.is_none()
        || previous.material_json != json.filter(|_| !truncated)
        || previous.truncated != truncated
        || previous.has_primitive != has_primitive
    {
        cache.values.remove(node_id);
        cache.next_sample = 0.0;
    }
}

fn invalidate_pending(cache: &mut SceneMaterialCache, node_id: &str) {
    if cache
        .pending
        .as_ref()
        .is_some_and(|(_, nodes, _)| nodes.iter().any(|id| id == node_id))
    {
        cache.pending = None;
        cache.next_sample = 0.0;
    }
}

fn publish(snapshot: &mut ResMut<SceneAnalysisSnapshot>, mut next: SceneAnalysisSnapshot) {
    next.revision = snapshot.revision;
    if **snapshot != next {
        next.revision = snapshot.revision.saturating_add(1);
        **snapshot = next;
    }
}

fn finite_subtree(
    root: Entity,
    transforms: &Query<&GlobalTransform>,
    bounds: &Query<&Aabb>,
    meshes: &Query<&Mesh3d>,
    children: &Query<&Children>,
) -> bool {
    let mut stack = vec![root];
    let mut visited = 0;
    while let Some(entity) = stack.pop() {
        visited += 1;
        if visited > 4096 {
            return false;
        }
        if meshes.get(entity).is_ok() && bounds.get(entity).is_err() {
            return false;
        }
        if let Ok(aabb) = bounds.get(entity) {
            if !Vec3::from(aabb.center).is_finite()
                || !Vec3::from(aabb.half_extents).is_finite()
                || Vec3::from(aabb.half_extents).min_element() < 0.0
                || !transforms
                    .get(entity)
                    .is_ok_and(|transform| transform.affine().is_finite())
            {
                return false;
            }
        }
        if let Ok(descendants) = children.get(entity) {
            if descendants.len() + stack.len() + visited > 4096 {
                return false;
            }
            stack.extend(descendants.iter());
        }
    }
    true
}

fn axis_aligned(transform: &GlobalTransform) -> bool {
    let matrix = transform.affine().matrix3;
    let axes = [
        Vec3::from(matrix.x_axis),
        Vec3::from(matrix.y_axis),
        Vec3::from(matrix.z_axis),
    ];
    axes.into_iter().all(|axis| {
        let Some(axis) = axis.try_normalize() else {
            return false;
        };
        axis.abs()
            .to_array()
            .into_iter()
            .filter(|value| *value > 1e-5)
            .count()
            == 1
    })
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SceneGeometry<'w, 's> {
    roots: Query<
        'w,
        's,
        (
            Entity,
            &'static SpawnedNodeMarker,
            Option<&'static crate::verse_manager::spawn::FallbackSign>,
            Option<&'static crate::verse_manager::PrimitiveNode>,
        ),
        With<ResidentNode>,
    >,
    transforms: Query<'w, 's, &'static GlobalTransform>,
    bounds: Query<'w, 's, &'static Aabb>,
    meshes: Query<'w, 's, &'static Mesh3d>,
    children: Query<'w, 's, &'static Children>,
}

pub(crate) fn collect(
    nav: Res<NavigationManager>,
    role: Res<LocalUserRole>,
    visibility: Res<crate::visibility::VisibilityState>,
    hierarchy: Res<VerseManager>,
    primitives: Res<PrimitiveDescriptorCache>,
    sender: Res<fe_runtime::app::DbCommandSender>,
    report: Res<crate::panels::volume_report_panel::VolumeReportState>,
    mut replies: MessageReader<DbResult>,
    mut cache: ResMut<SceneMaterialCache>,
    mut snapshot: ResMut<SceneAnalysisSnapshot>,
    geometry: SceneGeometry,
) {
    static CLOCK: OnceLock<Instant> = OnceLock::new();
    let now = CLOCK.get_or_init(Instant::now).elapsed().as_secs_f64();
    let petal = hierarchy
        .verses
        .iter()
        .find(|verse| Some(verse.id.as_str()) == nav.active_verse_id.as_deref())
        .and_then(|verse| {
            verse
                .fractals
                .iter()
                .find(|fractal| Some(fractal.id.as_str()) == nav.active_fractal_id.as_deref())
        })
        .and_then(|fractal| {
            fractal
                .petals
                .iter()
                .find(|petal| Some(petal.id.as_str()) == nav.active_petal_id.as_deref())
        });
    let petal_id = nav
        .active_petal_id
        .as_deref()
        .filter(|_| petal.is_some() && role.role.is_some_and(|role| role.can_view()));
    let scope = petal_id.and_then(|_| nav.active_scope());
    if cache.petal_id.as_deref() != petal_id || cache.scope != scope {
        *cache = SceneMaterialCache {
            petal_id: petal_id.map(str::to_owned),
            scope: scope.clone(),
            ..Default::default()
        };
    }
    let Some(petal_id) = petal_id else {
        cache.values.clear();
        cache.pending = None;
        replies.read().for_each(|_| {});
        publish(&mut snapshot, SceneAnalysisSnapshot::default());
        return;
    };
    for reply in replies.read() {
        match reply {
            DbResult::SceneMaterialsLoaded {
                materials,
                correlation_id,
            } => {
                let accepted = cache.pending.as_ref().is_some_and(|(id, nodes, deadline)| {
                    id == correlation_id
                        && now <= *deadline
                        && materials.petal_id == petal_id
                        && Some(materials.scope.as_str()) == scope.as_deref()
                        && materials.values.len() == nodes.len()
                        && materials
                            .values
                            .iter()
                            .all(|value| nodes.contains(&value.node_id))
                        && materials
                            .values
                            .iter()
                            .map(|value| &value.node_id)
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                            == nodes.len()
                });
                if accepted {
                    for value in &materials.values {
                        cache.values.insert(value.node_id.clone(), value.clone());
                    }
                    cache.pending = None;
                    cache.next_sample = 0.0;
                }
            }
            DbResult::RequestFailed { correlation_id, .. }
                if cache
                    .pending
                    .as_ref()
                    .is_some_and(|(id, _, _)| id == correlation_id) =>
            {
                cache.pending = None;
                cache.retry_after = now + 5.0;
            }
            DbResult::NodePropertySet { node_id, key, .. }
            | DbResult::NodePropertyDeleted { node_id, key, .. }
                if key == "material" || key == "primitive" =>
            {
                invalidate_pending(&mut cache, node_id);
                cache.values.remove(node_id);
                cache.next_sample = 0.0;
            }
            DbResult::NodeMaterialized { node, .. } => invalidate_changed_material(
                &mut cache,
                &node.node.node_id,
                node.properties.as_object(),
            ),
            DbResult::NodeMutationCommitted { receipt, .. } => invalidate_changed_material(
                &mut cache,
                &receipt.node_id,
                Some(&receipt.state.properties),
            ),
            DbResult::HierarchyLoaded { .. } | DbResult::ElevatedQueryResult { .. } => {
                cache.pending = None;
                cache.values.clear();
                cache.next_sample = 0.0;
            }
            DbResult::NodePropertiesLoaded {
                node_id,
                properties,
                ..
            } => invalidate_changed_material(&mut cache, node_id, properties.as_object()),
            _ => {}
        }
    }
    if cache
        .pending
        .as_ref()
        .is_some_and(|(_, _, deadline)| now > *deadline)
    {
        cache.pending = None;
        cache.retry_after = now + 5.0;
    }
    if now < cache.next_sample {
        return;
    }
    if !report.has_selection() {
        publish(&mut snapshot, SceneAnalysisSnapshot::default());
        return;
    }
    cache.next_sample = now + 0.25;
    let Some(petal) = petal else {
        publish(&mut snapshot, SceneAnalysisSnapshot::default());
        return;
    };
    let mut next = SceneAnalysisSnapshot {
        petal_id: Some(petal_id.to_owned()),
        omitted_objects: petal.nodes.len().saturating_sub(SCENE_OBJECT_LIMIT),
        ..Default::default()
    };
    let mut missing_materials = Vec::new();
    let wanted: HashSet<&str> = petal
        .nodes
        .iter()
        .take(SCENE_OBJECT_LIMIT)
        .map(|node| node.id.as_str())
        .collect();
    cache.values.retain(|id, _| wanted.contains(id.as_str()));
    let mut roots = HashMap::new();
    for root in geometry.roots.iter().filter(|(_, marker, _, _)| {
        marker.petal_id == petal_id && wanted.contains(marker.node_id.as_str())
    }) {
        roots
            .entry(root.1.node_id.as_str())
            .and_modify(|entry| *entry = None)
            .or_insert(Some(root));
    }
    for node in petal.nodes.iter().take(SCENE_OBJECT_LIMIT) {
        let material = cache.values.get(&node.id);
        if material.is_none() && missing_materials.len() < MATERIAL_BATCH_LIMIT {
            missing_materials.push(node.id.clone());
        }
        let primitive = primitives.get(&node.id);
        if !node.has_asset
            && node.asset_path.is_none()
            && primitive.is_none()
            && material.is_some_and(|material| !material.has_primitive)
        {
            next.excluded_non_geometric += 1;
            continue;
        }
        let root = roots.get(node.id.as_str()).copied().flatten();
        let bounds = root
            .filter(|(entity, _, fallback, _)| {
                fallback.is_none()
                    && finite_subtree(
                        *entity,
                        &geometry.transforms,
                        &geometry.bounds,
                        &geometry.meshes,
                        &geometry.children,
                    )
            })
            .and_then(|(entity, _, _, _)| {
                crate::gimbal::subtree_world_aabb(
                    entity,
                    &geometry.transforms,
                    &geometry.bounds,
                    &geometry.children,
                )
            })
            .filter(|(min, max)| {
                min.is_finite() && max.is_finite() && (max - min).min_element() > 0.0
            })
            .map(|(min, max)| [min.to_array(), max.to_array()]);
        let solid_box = root.is_some_and(|(entity, _, _, primitive)| {
            primitive.is_some_and(|primitive| primitive.descriptor.kind == PrimitiveKind::Cube)
                && geometry.transforms.get(entity).is_ok_and(axis_aligned)
        });
        next.objects.push(SceneObjectBounds {
            node_id: node.id.clone(),
            name: node.name.chars().take(256).collect(),
            bounds,
            geometry: if bounds.is_none() {
                ObjectGeometry::Unavailable
            } else if solid_box {
                ObjectGeometry::AxisAlignedSolidBox
            } else {
                ObjectGeometry::ConservativeWorldBounds
            },
            selectable: crate::visibility::pickable(&node.id, &nav, &visibility),
            material_json: material.and_then(|material| material.material_json.clone()),
            material_loaded: material.is_some(),
            material_truncated: material.is_some_and(|material| material.truncated),
        });
    }
    publish(&mut snapshot, next);
    if cache.pending.is_none() && !missing_materials.is_empty() && now >= cache.retry_after {
        let id = crate::actions::next_ui_query_id("scene-materials");
        if sender
            .0
            .try_send(DbCommand::ReadSceneMaterials {
                petal_id: petal_id.to_owned(),
                expected_scope: scope.unwrap_or_default(),
                node_ids: missing_materials.clone(),
                correlation_id: id.clone(),
            })
            .is_ok()
        {
            cache.pending = Some((id, missing_materials, now + 30.0));
        } else {
            bevy::log::warn!("DB queue unavailable for scene material projection");
            cache.retry_after = now + 5.0;
        }
    }
}
