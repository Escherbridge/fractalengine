// DIAG-15M: render-world instance-buffer diagnostics for the 2.7 GiB bind-group crash, promoted
// to the D-74 B3 closed loop (`RenderPressure`). See `fe-runtime/src/AGENTS.md` §render-pressure.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bevy::pbr::{MeshInputUniform, MeshUniform, RenderMeshInstances};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::{
    BatchedInstanceBuffers, PreprocessWorkItemBuffers,
};
use bevy::render::view::ExtractedView;
use bevy::render::{Render, RenderApp, RenderSystems};

/// Largest per-binding GPU instance-buffer demand in bytes, shared by the main
/// and render worlds (one `Arc`): the render world stores, the residency ledger
/// reads. See `fe-runtime/src/AGENTS.md` §render-pressure.
#[derive(Resource, Clone, Debug, Default)]
pub struct RenderPressure(pub Arc<AtomicU64>);

impl RenderPressure {
    /// Latest measured bytes (`0` = unmeasured / no GPU preprocessing).
    pub fn bytes(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// Publish a new measurement.
    pub fn store(&self, bytes: u64) {
        self.0.store(bytes, Ordering::Relaxed);
    }
}

/// Tile-residency counters the host binary mirrors from fe-terrain each frame
/// so fe-ui (no fe-terrain dependency) can show them. See AGENTS.md §render-pressure.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResidencyStats {
    /// Streamed tile bundles resident in RAM.
    pub tile_bundles: usize,
    /// Tile bytes resident in RAM (installed hexon caches + streamed bundles).
    pub tile_bytes: u64,
}

/// Largest single instance-buffer binding the render world currently needs:
/// `max(input_len × size_of::<MeshInputUniform>, max_phase(data_len × size_of::<MeshUniform>))`.
/// Pure so the per-binding (not summed) rule is unit-testable.
pub fn instance_pressure_bytes(
    input_len: usize,
    phase_data_lens: impl IntoIterator<Item = usize>,
) -> u64 {
    let input = (input_len as u64).saturating_mul(std::mem::size_of::<MeshInputUniform>() as u64);
    phase_data_lens
        .into_iter()
        .map(|len| (len as u64).saturating_mul(std::mem::size_of::<MeshUniform>() as u64))
        .fold(input, u64::max)
}

// DIAG-15M: plugin registering the per-frame logger in the render sub-app.
pub struct Diag15MPlugin;

impl Plugin for Diag15MPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderPressure>();
        app.add_systems(Last, diag_main_world);
    }

    fn finish(&self, app: &mut App) {
        // Same Arc in both worlds: the render world writes, the main world reads.
        let pressure = app.world().resource::<RenderPressure>().clone();
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.insert_resource(pressure);
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

// DIAG-15M: logs instance/view/buffer populations once per render frame and
// publishes the per-binding byte demand into `RenderPressure` (B3 closed loop).
fn diag_log(
    mesh_instances: Option<Res<RenderMeshInstances>>,
    buffers: Option<Res<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>>,
    views: Query<(), With<ExtractedView>>,
    pressure: Option<Res<RenderPressure>>,
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
    if let Some(pressure) = pressure.as_deref() {
        pressure.store(instance_pressure_bytes(
            cur,
            b.phase_instance_buffers
                .values()
                .map(|phase| phase.data_buffer.len()),
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The pressure is the largest single binding, never the sum: the 2 GiB
    /// limit is per bind group entry, so three 600 MB phases are not 1.8 GB of risk.
    #[test]
    fn pressure_is_the_largest_binding_not_the_sum() {
        let uniform = std::mem::size_of::<MeshUniform>() as u64;
        let input = std::mem::size_of::<MeshInputUniform>() as u64;
        assert_eq!(instance_pressure_bytes(0, []), 0);
        assert_eq!(instance_pressure_bytes(10, []), 10 * input);
        assert_eq!(
            instance_pressure_bytes(0, [100, 300, 200]),
            300 * uniform,
            "max phase, not 600 × MeshUniform"
        );
        assert_eq!(
            instance_pressure_bytes(1_000, [1]),
            (1_000 * input).max(uniform)
        );
        assert_eq!(
            instance_pressure_bytes(usize::MAX, []),
            u64::MAX,
            "saturates instead of wrapping"
        );
    }

    #[test]
    fn render_pressure_clones_share_one_cell() {
        let main_world = RenderPressure::default();
        let render_world = main_world.clone();
        render_world.store(42);
        assert_eq!(main_world.bytes(), 42);
    }
}
