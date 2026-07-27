//! First-class viewport brush gesture; see `AGENTS.md` §brush-tool.

use bevy::prelude::*;

use super::router::{ClickArbiter, ClickPriority, PointerPhase};
use crate::actions::terrain_proposal::{
    petal_map_enabled, SculptOpKind, SculptToolState, MAX_BRUSH_DABS_PER_STROKE,
};
use crate::actions::{UiAction, UiManager};
use crate::geometry::meters_to_world;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::terrain_map::PetalMapState;

const DAB_SPACING_RADIUS_FACTOR: f32 = 0.5;

#[derive(Debug, Clone)]
struct BrushSnapshot {
    radius: f32,
    strength: f32,
    op: String,
    target_height: Option<f32>,
    delta: Option<f32>,
    material: String,
}

impl BrushSnapshot {
    fn from_state(state: &SculptToolState, world_scale: f64) -> Self {
        let (target_height, delta) = match state.op {
            SculptOpKind::Level => (
                Some(meters_to_world(state.target_height, world_scale)),
                None,
            ),
            SculptOpKind::Raise | SculptOpKind::Lower => {
                (None, Some(meters_to_world(state.delta, world_scale)))
            }
            SculptOpKind::Smooth => (None, None),
        };
        Self {
            radius: meters_to_world(state.radius, world_scale),
            strength: state.strength,
            op: state.op.to_snake().to_string(),
            target_height,
            delta,
            material: if state.material.trim().is_empty() {
                "earth".to_string()
            } else {
                state.material.trim().to_string()
            },
        }
    }
}

#[derive(Debug)]
struct ActiveStroke {
    petal_id: String,
    samples: Vec<[f32; 2]>,
    settings: BrushSnapshot,
}

#[derive(Resource, Default)]
pub(super) struct BrushGesture {
    active: Option<ActiveStroke>,
}

fn ground_hit(ray: Ray3d) -> Option<[f32; 2]> {
    let direction = *ray.direction;
    if direction.y.abs() <= f32::EPSILON {
        return None;
    }
    let t = -ray.origin.y / direction.y;
    if !(t.is_finite() && t >= 0.0) {
        return None;
    }
    let hit = ray.origin + direction * t;
    (hit.x.is_finite() && hit.z.is_finite()).then_some([hit.x, hit.z])
}

fn append_spaced_samples(samples: &mut Vec<[f32; 2]>, point: [f32; 2], spacing: f32) {
    if samples.is_empty() {
        samples.push(point);
        return;
    }
    if !(spacing.is_finite() && spacing > 0.0) {
        return;
    }
    while samples.len() < MAX_BRUSH_DABS_PER_STROKE {
        let last = *samples.last().expect("checked non-empty");
        let dx = point[0] - last[0];
        let dz = point[1] - last[1];
        let distance = dx.hypot(dz);
        if !(distance.is_finite() && distance >= spacing) {
            break;
        }
        samples.push([
            last[0] + dx / distance * spacing,
            last[1] + dz / distance * spacing,
        ]);
    }
}

fn stroke_action(stroke: ActiveStroke) -> Option<UiAction> {
    (!stroke.samples.is_empty()).then_some(UiAction::SculptBrushStroke {
        petal_id: stroke.petal_id,
        centers: stroke.samples,
        radius: stroke.settings.radius,
        strength: stroke.settings.strength,
        op: stroke.settings.op,
        target_height: stroke.settings.target_height,
        delta: stroke.settings.delta,
        material: stroke.settings.material,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_brush_interaction(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    tool: Res<ToolState>,
    nav: Res<NavigationManager>,
    petal_map: Res<PetalMapState>,
    time: Res<Time>,
    mut sculpt: ResMut<SculptToolState>,
    mut gesture: ResMut<BrushGesture>,
    mut ui_mgr: ResMut<UiManager>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    let brush_active = tool.active_tool == Tool::Brush;
    let cancel = keyboard.just_pressed(KeyCode::Escape)
        || mouse.just_pressed(MouseButton::Right)
        || !brush_active;
    if cancel {
        gesture.active = None;
        return;
    }

    if gesture
        .active
        .as_ref()
        .is_some_and(|stroke| nav.active_petal_id.as_deref() != Some(stroke.petal_id.as_str()))
    {
        gesture.active = None;
        return;
    }

    if arbiter.phase() == PointerPhase::Hover {
        gesture.active = None;
        return;
    }

    let hit = arbiter.ray().and_then(ground_hit);
    match arbiter.phase() {
        PointerPhase::Press => {
            let Some(center) = hit else { return };
            if !arbiter.claim(ClickPriority::Brush) {
                return;
            }
            let Some(petal_id) = nav.active_petal_id.as_deref() else {
                bevy::log::warn!("Brush ignored — no active petal");
                ui_mgr.show_toast(
                    "Navigate to a petal before using Brush",
                    time.elapsed_secs_f64(),
                );
                return;
            };
            if !petal_map_enabled(&petal_map, petal_id) {
                bevy::log::warn!("Brush ignored — active petal has no enabled map");
                ui_mgr.show_toast(
                    "Assign an enabled map to this petal before using Brush",
                    time.elapsed_secs_f64(),
                );
                return;
            }
            sculpt.sanitize_numeric_state();
            gesture.active = Some(ActiveStroke {
                petal_id: petal_id.to_string(),
                samples: vec![center],
                settings: BrushSnapshot::from_state(&sculpt, petal_map.world_scale),
            });
        }
        PointerPhase::Hold => {
            if let (Some(stroke), Some(center)) = (gesture.active.as_mut(), hit) {
                append_spaced_samples(
                    &mut stroke.samples,
                    center,
                    stroke.settings.radius * DAB_SPACING_RADIUS_FACTOR,
                );
            }
        }
        PointerPhase::Release => {
            let Some(mut stroke) = gesture.active.take() else {
                return;
            };
            if let Some(center) = hit {
                append_spaced_samples(
                    &mut stroke.samples,
                    center,
                    stroke.settings.radius * DAB_SPACING_RADIUS_FACTOR,
                );
            }
            if let Some(action) = stroke_action(stroke) {
                ui_mgr.push_action(action);
            }
        }
        PointerPhase::Hover => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(op: &str) -> BrushSnapshot {
        BrushSnapshot {
            radius: 2.0,
            strength: 0.25,
            op: op.to_string(),
            target_height: Some(7.0),
            delta: Some(3.0),
            material: "earth".to_string(),
        }
    }

    #[test]
    fn sampling_is_distance_based_not_frame_based() {
        let mut samples = vec![[0.0, 0.0]];
        append_spaced_samples(&mut samples, [0.4, 0.0], 1.0);
        append_spaced_samples(&mut samples, [3.2, 0.0], 1.0);
        assert_eq!(
            samples,
            vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]
        );
    }

    #[test]
    fn release_payload_batches_all_samples_once() {
        let action = stroke_action(ActiveStroke {
            petal_id: "petal-1".to_string(),
            samples: vec![[1.0, 2.0], [2.0, 2.0]],
            settings: snapshot("level"),
        })
        .expect("non-empty stroke");
        match action {
            UiAction::SculptBrushStroke {
                petal_id,
                centers,
                radius,
                strength,
                target_height,
                ..
            } => {
                assert_eq!(petal_id, "petal-1");
                assert_eq!(centers, vec![[1.0, 2.0], [2.0, 2.0]]);
                assert_eq!(radius, 2.0);
                assert_eq!(strength, 0.25);
                assert_eq!(target_height, Some(7.0));
            }
            other => panic!("unexpected brush payload: {other:?}"),
        }
    }

    #[test]
    fn snapshot_converts_meter_controls_to_world_units() {
        let state = SculptToolState {
            radius: 2.0,
            target_height: 7.0,
            delta: 3.0,
            op: SculptOpKind::Level,
            ..Default::default()
        };
        let level = BrushSnapshot::from_state(&state, 0.001);
        assert!((level.radius - 0.002).abs() < 1e-7);
        assert!((level.target_height.unwrap() - 0.007).abs() < 1e-7);

        let raise = BrushSnapshot::from_state(
            &SculptToolState {
                op: SculptOpKind::Raise,
                ..state
            },
            0.001,
        );
        assert!((raise.delta.unwrap() - 0.003).abs() < 1e-7);
    }

    #[test]
    fn canceled_gesture_drops_samples_without_actions() {
        let mut gesture = BrushGesture {
            active: Some(ActiveStroke {
                petal_id: "p".to_string(),
                samples: vec![[1.0, 1.0]],
                settings: snapshot("raise"),
            }),
        };
        gesture.active = None;
        assert!(gesture.active.is_none());
    }
}
