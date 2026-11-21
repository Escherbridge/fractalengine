//! Portal `focus_node` / `select_node` / `tour_nodes` consumer; see `fe-renderer/src/AGENTS.md` §camera-tour.

use bevy::prelude::*;
use bevy::window::RequestRedraw;
use fe_renderer::camera::{CameraTour, MAX_TOUR_STOPS};
use fe_webview::portal_bundle::{PortalIntent, UiIntent};

use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::plugin::{CameraFocusTarget, SpawnedNodeMarker};
use crate::visibility::{pickable, VisibilityState};

/// World position of a visible, spawned node of `petal_id`; `None` when unknown, out of petal or hidden.
fn resolve_node(
    node_id: &str,
    petal_id: &str,
    nav: &NavigationManager,
    visibility: &VisibilityState,
    spawned: &Query<(&SpawnedNodeMarker, &GlobalTransform)>,
) -> Option<Vec3> {
    if !pickable(node_id, nav, visibility) {
        return None;
    }
    spawned
        .iter()
        .find(|(marker, _)| marker.node_id == node_id && marker.petal_id == petal_id)
        .map(|(_, transform)| transform.translation())
}

/// Turns accepted portal intents into a focus request, a selection request or a camera tour (local only).
#[allow(clippy::too_many_arguments)]
pub fn apply_portal_intents(
    mut intents: MessageReader<PortalIntent>,
    nav: Res<NavigationManager>,
    visibility: Res<VisibilityState>,
    spawned: Query<(&SpawnedNodeMarker, &GlobalTransform)>,
    mut node_manager: ResMut<NodeManager>,
    mut focus: ResMut<CameraFocusTarget>,
    mut tour: ResMut<CameraTour>,
    mut last_petal: Local<Option<String>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    if last_petal.as_deref() != nav.active_petal_id.as_deref() {
        tour.cancel();
        *last_petal = nav.active_petal_id.clone();
    }
    for intent in intents.read() {
        if nav.active_petal_id.as_deref() != Some(intent.petal_id.as_str()) {
            warn!("portal {}: {} dropped, petal {} is not active", intent.package_id, intent.intent.as_str(), intent.petal_id);
            continue;
        }
        match (intent.intent, &intent.tour) {
            (UiIntent::TourNodes, Some(request)) => {
                let mut stops = [Vec3::ZERO; MAX_TOUR_STOPS];
                let mut count = 0;
                for node_id in request.node_ids.iter().take(MAX_TOUR_STOPS) {
                    match resolve_node(node_id, &intent.petal_id, &nav, &visibility, &spawned) {
                        Some(position) => {
                            stops[count] = position;
                            count += 1;
                        }
                        None => warn!("portal {}: tour skips unknown or hidden node {node_id}", intent.package_id),
                    }
                }
                if count == 0 {
                    warn!("portal {}: tour_nodes dropped, no visible node in the active petal", intent.package_id);
                    continue;
                }
                tour.start(&stops[..count], request.speed_mm_s as f32 / 1000.0, request.dwell_ms as f32 / 1000.0);
                redraw.write(RequestRedraw);
            }
            (UiIntent::FocusNode, _) | (UiIntent::SelectNode, _) => {
                if resolve_node(&intent.node_id, &intent.petal_id, &nav, &visibility, &spawned).is_none() {
                    warn!("portal {}: {} dropped, unknown or hidden node {}", intent.package_id, intent.intent.as_str(), intent.node_id);
                } else if intent.intent == UiIntent::FocusNode {
                    focus.target = Some((intent.node_id.clone(), [0.0; 3]));
                    redraw.write(RequestRedraw);
                } else if node_manager.is_dragging() || node_manager.gesture_active {
                    // A page never aborts the user's own drag.
                    warn!("portal {}: select_node dropped, a drag is in progress", intent.package_id);
                } else {
                    node_manager.request_select(&intent.node_id);
                    redraw.write(RequestRedraw);
                }
            }
            (UiIntent::TourNodes, None) => warn!("portal {}: tour_nodes without a tour dropped", intent.package_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_webview::portal_bundle::PortalTour;

    fn intent(kind: UiIntent, petal: &str, node: &str, tour: Option<PortalTour>) -> PortalIntent {
        PortalIntent { intent: kind, node_id: node.into(), petal_id: petal.into(), package_id: "org.example.pkg".into(), tour }
    }

    /// Focus, select and tour intents reach their targets and wake the loop; another petal, an unknown node, a drag
    /// in progress and a petal change do not.
    #[test]
    fn portal_intents_reach_camera_and_selection_only_in_the_active_petal() {
        let mut app = App::new();
        app.add_message::<PortalIntent>()
            .add_message::<RequestRedraw>()
            .init_resource::<NavigationManager>()
            .init_resource::<VisibilityState>()
            .init_resource::<NodeManager>()
            .init_resource::<CameraFocusTarget>()
            .init_resource::<CameraTour>()
            .add_systems(Update, apply_portal_intents);
        app.world_mut().resource_mut::<NavigationManager>().navigate_to_petal("p1");
        for (node, petal, x) in [("a", "p1", 10.0), ("b", "p1", 20.0), ("c", "p2", 30.0)] {
            app.world_mut().spawn((
                SpawnedNodeMarker { node_id: node.into(), petal_id: petal.into() },
                GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
            ));
        }
        let send = |app: &mut App, message: PortalIntent| {
            app.world_mut().write_message(message);
            app.update();
        };

        let woke = |app: &App| app.world().resource::<Messages<RequestRedraw>>().iter_current_update_messages().count() > 0;
        send(&mut app, intent(UiIntent::FocusNode, "p1", "a", None));
        assert_eq!(app.world().resource::<CameraFocusTarget>().target.as_ref().map(|(id, _)| id.as_str()), Some("a"));
        assert!(woke(&app), "a focus wakes the reactive loop");
        app.world_mut().resource_mut::<CameraFocusTarget>().target = None;

        send(&mut app, intent(UiIntent::FocusNode, "p2", "c", None));
        send(&mut app, intent(UiIntent::FocusNode, "p1", "missing", None));
        send(&mut app, intent(UiIntent::FocusNode, "p1", "c", None));
        assert!(app.world().resource::<CameraFocusTarget>().target.is_none(), "other petal, unknown and out-of-petal nodes are dropped");
        assert!(!woke(&app), "a dropped intent wakes nothing");

        // A select while the user's gesture is live is dropped, so a page cannot abort the drag.
        app.world_mut().resource_mut::<NodeManager>().gesture_active = true;
        send(&mut app, intent(UiIntent::SelectNode, "p1", "b", None));
        assert_eq!(app.world().resource::<NodeManager>().pending_sidebar_select, None, "no selection during a drag");
        assert!(!woke(&app));
        app.world_mut().resource_mut::<NodeManager>().gesture_active = false;
        send(&mut app, intent(UiIntent::SelectNode, "p1", "b", None));
        assert_eq!(app.world().resource::<NodeManager>().pending_sidebar_select.as_deref(), Some("b"));

        let tour = PortalTour { node_ids: vec!["a".into(), "ghost".into(), "b".into()], speed_mm_s: 30_000, dwell_ms: 1_000 };
        send(&mut app, intent(UiIntent::TourNodes, "p1", "a", Some(tour.clone())));
        assert!(woke(&app), "a started tour runs without waiting for input");
        let tour_state = app.world().resource::<CameraTour>();
        assert!(tour_state.is_active());
        assert_eq!(tour_state.pending_stops(), &[Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0)], "ordered, unknown skipped");

        // A tour naming only out-of-petal nodes starts nothing; a petal change cancels a running one.
        app.world_mut().resource_mut::<CameraTour>().cancel();
        let foreign = PortalTour { node_ids: vec!["c".into()], ..tour };
        send(&mut app, intent(UiIntent::TourNodes, "p1", "c", Some(foreign)));
        assert!(!app.world().resource::<CameraTour>().is_active());
        app.world_mut().resource_mut::<CameraTour>().start(&[Vec3::X], 10.0, 0.0);
        app.world_mut().resource_mut::<NavigationManager>().navigate_to_petal("p2");
        app.update();
        assert!(!app.world().resource::<CameraTour>().is_active(), "petal change cancels the tour");

        // The two copies of the tour bounds agree (fe-webview does not depend on fe-renderer).
        use fe_renderer::camera::{TOUR_DWELL_S, TOUR_SPEED_MPS};
        use fe_webview::portal_bundle::limits;
        assert_eq!(limits::TOUR_MAX_NODES, MAX_TOUR_STOPS);
        assert_eq!((limits::TOUR_SPEED_MPS.0 as f32, limits::TOUR_SPEED_MPS.1 as f32), TOUR_SPEED_MPS);
        assert_eq!((limits::TOUR_DWELL_S.0 as f32, limits::TOUR_DWELL_S.1 as f32), TOUR_DWELL_S);
    }
}
