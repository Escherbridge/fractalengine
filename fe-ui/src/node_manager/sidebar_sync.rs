//! Sidebar click → NodeManager selection resolution. A hidden node is refused
//! (D-7/D-12 shared picker rule) — see `node_manager/AGENTS.md` §visibility-pickers.

use bevy::prelude::*;

use super::NodeManager;
use crate::actions::UiManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;
use crate::visibility::{pickable, VisibilityState};

/// Toast for a sidebar click on an effectively-hidden node.
pub(super) const HIDDEN_SELECT_TOAST: &str = "That object is hidden — show it first";

pub(super) fn sync_sidebar_to_manager(
    nav: Res<NavigationManager>,
    mut manager: ResMut<NodeManager>,
    node_query: Query<(Entity, &SpawnedNodeMarker)>,
    visibility: Res<VisibilityState>,
    mut ui_mgr: ResMut<UiManager>,
    time: Res<Time>,
) {
    let Some(node_id) = manager.pending_sidebar_select.take() else {
        return;
    };

    // Consumed either way; a hidden node is never selected and the current
    // selection is left alone.
    if !pickable(&node_id, &nav, &visibility) {
        ui_mgr.show_toast(HIDDEN_SELECT_TOAST, time.elapsed_secs_f64());
        return;
    }

    let active_petal = nav.active_petal_id.as_deref();
    let matched = node_query.iter().find(|(_, m)| {
        m.node_id == node_id
            && active_petal
                .map(|pid| pid == m.petal_id.as_str())
                .unwrap_or(true)
    });

    if let Some((entity, _)) = matched {
        manager.select(entity, node_id);
    } else {
        manager.deselect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::OverrideState;

    /// The real system over one spawned node `n1` in petal `p1`, with `n0`
    /// currently selected.
    fn app_with_selection() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<NodeManager>()
            .init_resource::<UiManager>()
            .init_resource::<VisibilityState>()
            .insert_resource(NavigationManager {
                active_petal_id: Some("p1".into()),
                ..Default::default()
            })
            .add_systems(Update, sync_sidebar_to_manager);
        let n1 = app
            .world_mut()
            .spawn(SpawnedNodeMarker {
                node_id: "n1".into(),
                petal_id: "p1".into(),
            })
            .id();
        let n0 = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<NodeManager>()
            .select(n0, "n0");
        (app, n1)
    }

    fn sidebar_click(app: &mut App, node_id: &str) -> Option<String> {
        app.world_mut()
            .resource_mut::<NodeManager>()
            .pending_sidebar_select = Some(node_id.to_string());
        app.update();
        app.world()
            .resource::<NodeManager>()
            .selected
            .as_ref()
            .map(|s| s.node_id.clone())
    }

    #[test]
    fn a_visible_node_click_selects_it() {
        let (mut app, _) = app_with_selection();
        assert_eq!(sidebar_click(&mut app, "n1").as_deref(), Some("n1"));
    }

    #[test]
    fn a_hidden_node_click_is_consumed_refused_and_toasted() {
        for (label, hide) in [
            (
                "own Hide override",
                (|v| v.set_node_override("n1", OverrideState::Hide)) as fn(&mut VisibilityState),
            ),
            ("hidden petal", |v| v.toggle_petal_hidden("p1")),
            ("isolated away", |v| {
                v.set_solo(std::collections::HashSet::from(["n0".to_string()]))
            }),
        ] {
            let (mut app, _) = app_with_selection();
            hide(&mut app.world_mut().resource_mut::<VisibilityState>());
            assert_eq!(
                sidebar_click(&mut app, "n1").as_deref(),
                Some("n0"),
                "{label}: the prior selection is kept"
            );
            assert!(app
                .world()
                .resource::<NodeManager>()
                .pending_sidebar_select
                .is_none());
            let toast = app
                .world()
                .resource::<UiManager>()
                .active_toast(0.0)
                .map(|(msg, _)| msg.to_string());
            assert_eq!(toast.as_deref(), Some(HIDDEN_SELECT_TOAST), "{label}");
        }
    }
}
