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
    node_query: Query<(
        Entity,
        &SpawnedNodeMarker,
        Option<&crate::verse_manager::PathAssetInstance>,
    )>,
    stamps: Res<crate::actions::asset::StampInteractionState>,
    visibility: Res<VisibilityState>,
    mut ui_mgr: ResMut<UiManager>,
    time: Res<crate::notice::NoticeClock>,
    mut transforms: Query<&mut Transform>,
) {
    use super::{SelectionMode, SelectionTarget};
    let mut navigation = None;
    let request = if let Some(request) = manager.pending_selection.take() {
        request
    } else if let Some(node_id) = manager.pending_sidebar_select.take() {
        manager.pending_navigation_select = None;
        super::SelectionRequest {
            petal_id: nav.active_petal_id.clone().unwrap_or_default(),
            node_ids: vec![node_id],
            mode: SelectionMode::Replace,
            visible_order: Vec::new(),
            clicked_row: None,
        }
    } else if let Some(pending) = manager.pending_navigation_select.take() {
        if nav.active_petal_id.as_deref() != Some(pending.petal_id.as_str()) {
            return;
        }
        let request = super::SelectionRequest {
            petal_id: pending.petal_id.clone(),
            node_ids: vec![pending.node_id.clone()],
            mode: SelectionMode::Replace,
            visible_order: Vec::new(),
            clicked_row: None,
        };
        navigation = Some(pending);
        request
    } else {
        return;
    };
    if nav.active_petal_id.as_deref() != Some(request.petal_id.as_str()) {
        return;
    }
    let ids = manager.requested_ids(&request);
    let mut targets = Vec::new();
    let mut hidden = 0;
    let mut unavailable = 0;
    for node_id in &ids {
        if !pickable(node_id, &nav, &visibility) {
            hidden += 1;
            continue;
        }
        let stamp_alias = stamps.promoted_stamp_for_node(node_id);
        let matched = node_query.iter().find(|(_, marker, instance)| {
            marker.petal_id == request.petal_id
                && if let Some(stamp) = stamp_alias {
                    instance.is_some_and(|instance| {
                        instance.source_track_id == stamp.track_node_id
                            && instance.petal_id == marker.petal_id
                    }) && crate::verse_manager::parse_stamp_marker_id(&marker.node_id)
                        == Some((stamp.track_node_id.as_str(), stamp.stamp_index))
                        && pickable(&marker.node_id, &nav, &visibility)
                } else {
                    marker.node_id == *node_id
                }
        });
        if let Some((entity, _, _)) = matched {
            if !targets.iter().any(|target: &SelectionTarget| {
                target.node_id == *node_id || target.entity == entity
            }) {
                targets.push(SelectionTarget {
                    entity,
                    node_id: node_id.clone(),
                });
            }
        } else {
            unavailable += 1;
        }
    }
    if targets.is_empty() {
        if ids.len() > 1 && hidden + unavailable > 0 {
            ui_mgr.show_advisory(format!("No objects were selected: {hidden} hidden and {unavailable} unavailable. The previous selection was kept."), time.elapsed_secs_f64());
            return;
        }

        if hidden > 0 {
            ui_mgr.show_toast(HIDDEN_SELECT_TOAST, time.elapsed_secs_f64());
            return;
        }
        if let Some(mut pending) = navigation {
            pending.remaining_frames = pending.remaining_frames.saturating_sub(1);
            if pending.remaining_frames > 0 {
                manager.pending_navigation_select = Some(pending);
            } else {
                ui_mgr.show_toast("The object did not become available in this petal. Select it again after loading.", time.elapsed_secs_f64());
            }
        } else if ids.len() == 1 {
            manager.deselect();
        }
    } else if let Err(reason) = manager.reduce_requested_selection(&request, targets) {
        ui_mgr.show_advisory(reason, time.elapsed_secs_f64());
    }
    if ids.len() > 1 && hidden + unavailable > 0 {
        ui_mgr.show_advisory(format!("Selected {} available objects; {hidden} hidden and {unavailable} unavailable objects were skipped", manager.selection_count()), time.elapsed_secs_f64());
    }
    for (entity, restore) in manager.take_cancelled_restores() {
        if let Ok(mut transform) = transforms.get_mut(entity) {
            restore.apply(&mut transform);
        }
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
        app.init_resource::<crate::notice::NoticeClock>()
            .init_resource::<NodeManager>()
            .init_resource::<UiManager>()
            .init_resource::<VisibilityState>()
            .init_resource::<crate::actions::asset::StampInteractionState>()
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

        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("p2");
        app.world_mut()
            .resource_mut::<NodeManager>()
            .request_select_after_navigation("later", "p2");
        app.update();
        assert!(app
            .world()
            .resource::<NodeManager>()
            .pending_navigation_select
            .is_some());
        app.world_mut().spawn(SpawnedNodeMarker {
            node_id: "later".into(),
            petal_id: "p1".into(),
        });
        app.update();
        assert!(
            app.world().resource::<NodeManager>().selected.is_none(),
            "same node id in another petal cannot win"
        );
        let arrived = app
            .world_mut()
            .spawn(SpawnedNodeMarker {
                node_id: "later".into(),
                petal_id: "p2".into(),
            })
            .id();
        app.update();
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(arrived)
        );
        assert!(app
            .world()
            .resource::<NodeManager>()
            .pending_navigation_select
            .is_none());

        app.world_mut()
            .resource_mut::<NodeManager>()
            .request_select_after_navigation("missing", "p2");
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("p3");
        app.update();
        assert!(
            app.world()
                .resource::<NodeManager>()
                .pending_navigation_select
                .is_none(),
            "leaving destination cancels delayed selection"
        );

        app.world_mut()
            .resource_mut::<NodeManager>()
            .request_select_after_navigation("missing", "p3");
        app.world_mut()
            .resource_mut::<NodeManager>()
            .select(arrived, "explicit");
        app.update();
        assert_eq!(
            app.world()
                .resource::<NodeManager>()
                .selected
                .as_ref()
                .unwrap()
                .node_id,
            "explicit"
        );
        assert!(app
            .world()
            .resource::<NodeManager>()
            .pending_navigation_select
            .is_none());

        app.world_mut()
            .resource_mut::<NodeManager>()
            .request_select_after_navigation("missing", "p3");
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("p1");
        assert_eq!(sidebar_click(&mut app, "n1").as_deref(), Some("n1"));
        assert!(
            app.world()
                .resource::<NodeManager>()
                .pending_navigation_select
                .is_none(),
            "an explicit sidebar choice wins"
        );

        app.world_mut()
            .resource_mut::<NodeManager>()
            .request_select_after_navigation("missing", "p1");
        for _ in 0..120 {
            app.update();
        }
        assert!(app
            .world()
            .resource::<NodeManager>()
            .pending_navigation_select
            .is_none());
        assert!(app
            .world()
            .resource::<UiManager>()
            .active_toast(0.0)
            .unwrap()
            .0
            .contains("did not become available"));

        let properties = serde_json::json!({
            "node_kind":"stamp", "path_id":"path", "owning_path_id":"path", "instance_index":2
        });
        crate::actions::asset::hydrate_promoted_stamp(
            "path#inst-2",
            &properties,
            &mut app
                .world_mut()
                .resource_mut::<crate::actions::asset::StampInteractionState>(),
        );
        let stamp = app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: "path::stamp::2".into(),
                    petal_id: "p1".into(),
                },
                crate::verse_manager::PathAssetInstance {
                    source_track_id: "path".into(),
                    petal_id: "p1".into(),
                },
            ))
            .id();
        assert_eq!(
            sidebar_click(&mut app, "path#inst-2").as_deref(),
            Some("path#inst-2")
        );
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(stamp)
        );
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("p2");
        assert!(
            sidebar_click(&mut app, "path#inst-2").is_none(),
            "a persisted address cannot select its other-Petal projection"
        );
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

    /// Notices are stamped on `NoticeClock` (real time, what egui renders
    /// against), not the virtual `Time` that lags after a hitch — a lag of
    /// more than an Error's 7 s would otherwise hide it on arrival.
    #[test]
    fn notice_is_stamped_on_the_render_clock_not_virtual_time() {
        let (mut app, _) = app_with_selection();
        app.init_resource::<Time>(); // virtual time, left at 0 (lagging)
        {
            let mut clock = app.world_mut().resource_mut::<crate::notice::NoticeClock>();
            let start = clock.startup();
            clock.update_with_instant(start);
            clock.update_with_instant(start + std::time::Duration::from_secs(10));
        }
        app.world_mut()
            .resource_mut::<VisibilityState>()
            .set_node_override("n1", OverrideState::Hide);
        sidebar_click(&mut app, "n1");
        let ui_mgr = app.world().resource::<UiManager>();
        let (notice, _) = ui_mgr.active_notice(10.0).expect("toast live at t=10");
        assert_eq!(notice.spawned_at, 10.0);
    }
}
