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
) {
    sidebar_state.open = left_visibility(state.policy, state.user_intent);
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
    }

    #[test]
    fn closed_intent_stays_closed() {
        let p = LeftSidebarPolicy::Manual;
        assert!(!left_visibility(p, false));
    }
}
