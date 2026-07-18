// DIAG-15M: temporary render-world instance-buffer diagnostics for the 2.7 GiB bind-group crash.
use bevy::pbr::{MeshInputUniform, MeshUniform, RenderMeshInstances};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::{
    BatchedInstanceBuffers, PreprocessWorkItemBuffers,
};
use bevy::render::view::ExtractedView;
use bevy::render::{Render, RenderApp, RenderSystems};

// DIAG-15M: plugin registering the per-frame logger in the render sub-app.
pub struct Diag15MPlugin;

impl Plugin for Diag15MPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Last, diag_main_world);
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.add_systems(
            Render,
            diag_log.in_set(RenderSystems::PrepareResourcesFlush),
        );
        tracing::debug!("DIAG-15M: render-world diagnostics registered");
    }
}

// DIAG-15M: main-world census of view-producing entities (cameras/lights) with name samples.
fn diag_main_world(
    cameras: Query<(Entity, Option<&Name>), With<Camera>>,
    point_lights: Query<(), With<PointLight>>,
    spot_lights: Query<(), With<SpotLight>>,
    dir_lights: Query<(), With<DirectionalLight>>,
    scene_roots: Query<(), With<SceneRoot>>,
    mut frame: Local<u64>,
) {
    *frame += 1;
    let cam_count = cameras.iter().count();
    let sample: Vec<String> = cameras
        .iter()
        .take(4)
        .map(|(e, n)| format!("{e:?}:{}", n.map(|n| n.as_str()).unwrap_or("?")))
        .collect();
    tracing::debug!(
        "DIAG-15M-MAIN f={} cameras={} point_lights={} spot_lights={} dir_lights={} scene_roots={} cam_sample={:?}",
        *frame,
        cam_count,
        point_lights.iter().count(),
        spot_lights.iter().count(),
        dir_lights.iter().count(),
        scene_roots.iter().count(),
        sample
    );
}

// DIAG-15M: logs instance/view/buffer populations once per render frame.
fn diag_log(
    mesh_instances: Option<Res<RenderMeshInstances>>,
    buffers: Option<Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>>,
    views: Query<(), With<ExtractedView>>,
    mut frame: Local<u64>,
) {
    *frame += 1;
    let mi_len = match mesh_instances.as_deref() {
        Some(RenderMeshInstances::CpuBuilding(m)) => m.len(),
        Some(RenderMeshInstances::GpuBuilding(m)) => m.len(),
        None => usize::MAX,
    };
    let view_count = views.iter().count();
    let Some(b) = buffers.as_deref() else {
        tracing::debug!(
            "DIAG-15M f={} no BatchedInstanceBuffers; mesh_instances={} views={}",
            *frame,
            mi_len,
            view_count
        );
        return;
    };
    let cur = b.current_input_buffer.len();
    let prev = b.previous_input_buffer.len();
    tracing::debug!(
        "DIAG-15M f={} mesh_instances={} views={} input_cur={} input_prev={} input_cur_gpu_bytes={:?}",
        *frame, mi_len, view_count, cur, prev,
        b.current_input_buffer.buffer().buffer().map(|buf| buf.size())
    );
    for (tid, phase) in b.phase_instance_buffers.iter() {
        let db_len = phase.data_buffer.len();
        let db_gpu = phase.data_buffer.buffer().map(|buf| buf.size());
        let mut wi_views = 0usize;
        let mut wi_total = 0usize;
        let mut wi_max_view = 0usize;
        for (view_key, wib) in phase.work_item_buffers.iter() {
            wi_views += 1;
            let n = match wib {
                PreprocessWorkItemBuffers::Direct(v) => v.len(),
                PreprocessWorkItemBuffers::Indirect {
                    indexed,
                    non_indexed,
                    ..
                } => indexed.len() + non_indexed.len(),
            };
            wi_total += n;
            if n > wi_max_view {
                wi_max_view = n;
            }
            // DIAG-15M: per-view detail only when a view goes pathological.
            if n > 100_000 {
                tracing::debug!(
                    "DIAG-15M f={}   HOT view {:?} work_items={}",
                    *frame,
                    view_key,
                    n
                );
            }
        }
        tracing::debug!(
            "DIAG-15M f={} phase={:?} data_buffer_len={} data_buffer_gpu_bytes={:?} work_items_total={} wi_views={} wi_max_view={}",
            *frame, tid, db_len, db_gpu, wi_total, wi_views, wi_max_view
        );
    }
}
