//! Left-sidebar area manager (FR-3 shell_ux_sidebar_20260725): owns the
//! sidebar-visibility POLICY. Per D-A11 the auto-collapse-on-selection default
//! is REMOVED entirely (Q-3 ratified) — the sidebar is now user-sticky: it
//! stays exactly where the user last set it (`user_intent`), unaffected by
//! selection, right-section open, or petal switch (session-scoped, Q-2). The
//! explicit topbar toggle + shortcut (`ui_shell/topbar.rs`) flips `user_intent`.
//! See `fe-ui/src/ui_shell/AGENTS.md` §left.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::UiManager;
use crate::atlas::DashboardState;
use crate::navigation_manager::NavigationManager;
use crate::panels::sidebar;
use crate::plugin::{CameraFocusTarget, SidebarState};
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

/// Left-sidebar visibility policy. Auto-collapse was removed (D-A11 / Q-3): the
/// only policy is user-sticky `Manual`. The enum is retained as the decision
/// seam so `left_visibility` stays a pure, testable function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeftSidebarPolicy {
    /// User-sticky: honor the user's explicit open/close intent, ignoring
    /// selection / right-region state (D-A11). This is the only policy.
    #[default]
    Manual,
}

/// Left-sidebar manager state: the visibility policy + the user's explicit
/// session-scoped open/close intent.
#[derive(Resource, Debug, Clone)]
pub struct LeftSidebarState {
    pub policy: LeftSidebarPolicy,
    pub user_intent: bool,
}

impl Default for LeftSidebarState {
    fn default() -> Self {
        // Start open (matches `SidebarState` default) and sticky.
        Self {
            policy: LeftSidebarPolicy::default(),
            user_intent: true,
        }
    }
}

/// Pure visibility decision. User-sticky: visibility is exactly `user_intent`,
/// independent of selection / portal / right-section open (D-A11).
pub fn left_visibility(policy: LeftSidebarPolicy, user_intent: bool) -> bool {
    match policy {
        LeftSidebarPolicy::Manual => user_intent,
    }
}

/// Applies the visibility policy, then renders the left sidebar. The manager
/// owns the open/close decision; there is NO per-frame `!right_open` stomp
/// anymore (D-A11) — `sidebar.open` tracks the user's sticky intent only.
pub fn render_left_sidebar(
    ctx: &egui::Context,
    state: &LeftSidebarState,
    sidebar_state: &mut SidebarState,
    nav: &mut NavigationManager,
    dashboard: &DashboardState,
    hierarchy: &mut VerseManager,
    camera_focus: &mut CameraFocusTarget,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    // hierarchy_visibility_groups_20260808 Phase 1: sidebar eye toggles.
    vis_state: &mut crate::visibility::VisibilityState,
    app_settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
) {
    sidebar_state.open = left_visibility(state.policy, state.user_intent)
        && super::layout_policy::can_show_left(ctx);
    sidebar::left_sidebar(
        ctx,
        sidebar_state,
        nav,
        dashboard,
        hierarchy,
        camera_focus,
        db_tx,
        node_mgr,
        ui_mgr,
        vis_state,
        app_settings,
        local_role,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_manual_and_open() {
        let s = LeftSidebarState::default();
        assert_eq!(s.policy, LeftSidebarPolicy::Manual);
        assert!(s.user_intent);
    }

    #[test]
    fn open_intent_survives_selection_right_open_and_petal_switch() {
        // Visibility depends ONLY on user_intent — selection / right-section
        // open / petal switch are not inputs, so an open sidebar stays open
        // through all of them (D-A11 sticky).
        let p = LeftSidebarPolicy::Manual;
        assert!(left_visibility(p, true));
        for (physical, scale) in [(640.0, 2.0), (1024.0, 1.0), (1280.0, 1.0), (1920.0, 1.0)] {
            let points = physical / scale;
            let protected = super::super::layout_policy::protected_width(points, scale);
            let left = if points >= protected + 240.0 {
                (points - protected - 120.0).clamp(120.0, 400.0)
            } else {
                0.0
            };
            let right = (points - protected - left).max(0.0);
            assert!(points - left - right >= points * 0.45 - 0.01);
            assert!(
                left_visibility(p, true),
                "effective collapse must not change user intent"
            );
        }
        use super::super::layout_policy;
        for parent_height in [600.0, 130.0] {
            let ctx = egui::Context::default();
            layout_policy::begin_frame(&ctx, true);
            let mut lower_row = None;
            let mut cancel = None;
            let mut header = None;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    layout_policy::begin_ui_pass(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        layout_policy::scroll(
                            ui,
                            "parent",
                            [false, true],
                            egui::ScrollArea::vertical()
                                .max_height(parent_height)
                                .auto_shrink([false, false]),
                            |ui| {
                                header = Some(ui.button("Parent header"));
                                layout_policy::scroll(
                                    ui,
                                    "horizontal tall rows",
                                    [true, false],
                                    egui::ScrollArea::horizontal()
                                        .max_height(80.0)
                                        .auto_shrink([false, false]),
                                    |ui| {
                                        for row in 0..12 {
                                            let response = ui.add_sized(
                                                [340.0, 24.0],
                                                egui::Button::new(format!("Delete {row}")),
                                            );
                                            layout_policy::record_response(ui, &response);
                                            if row == 5 {
                                                lower_row = Some(response);
                                            }
                                        }
                                    },
                                );
                                if parent_height > 500.0 {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(70.0, 30.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let clip = egui::Rect::from_min_size(
                                                ui.min_rect().min,
                                                egui::vec2(70.0, 30.0),
                                            )
                                            .intersect(ui.clip_rect());
                                            ui.set_clip_rect(clip);
                                            ui.add_sized(
                                                [60.0, 24.0],
                                                egui::Button::new("Confirm"),
                                            );
                                            let response = ui.add_sized(
                                                [60.0, 24.0],
                                                egui::Button::new("Cancel"),
                                            );
                                            layout_policy::record_response(ui, &response);
                                            cancel = Some(response);
                                        },
                                    );
                                }
                            },
                        );
                    });
                },
            );
            let observations = layout_policy::observations(&ctx);
            let horizontal = observations
                .scrolls
                .iter()
                .find(|s| s.label == "horizontal tall rows")
                .unwrap();
            let parent = observations
                .scrolls
                .iter()
                .find(|s| s.label == "parent")
                .unwrap();
            let response = lower_row.unwrap();
            let control = observations
                .controls
                .iter()
                .find(|c| c.id == response.id.value())
                .unwrap();
            assert!(
                response.rect.min.y > horizontal.viewport.max.y,
                "the old inner_rect would report false clipping"
            );
            assert!(horizontal.content_rect.contains_rect(response.rect));
            assert!(horizontal.clip.max.y <= parent.clip.max.y);
            assert_eq!(horizontal.parent_id, parent.content_id);
            assert_ne!(horizontal.content_id, horizontal.parent_id);
            assert!(
                header.unwrap().rect.max.y < horizontal.content_rect.min.y,
                "parent headers are not scroll content"
            );
            if parent_height > 500.0 {
                assert!(horizontal.clip.contains_rect(response.rect));
                assert!(control.clip.contains_rect(control.rect));
                let cancel = cancel.unwrap();
                let cancel = observations
                    .controls
                    .iter()
                    .find(|c| c.id == cancel.id.value())
                    .unwrap();
                assert!(
                    !cancel.clip.contains_rect(cancel.rect),
                    "the real armed Cancel scissor must stay visible in telemetry"
                );
                assert!(cancel.clip.max.x < parent.clip.max.x);
            } else {
                assert!(!horizontal.clip.contains_rect(response.rect));
                assert!(
                    !control.clip.intersects(control.rect),
                    "parent-hidden rows remain unreachable at the current offset"
                );
            }
        }
    }

    #[test]
    fn closed_intent_stays_closed() {
        let p = LeftSidebarPolicy::Manual;
        assert!(!left_visibility(p, false));
    }
}
