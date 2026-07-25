//! Left-sidebar area manager (FR-5): owns the sidebar-visibility POLICY. The
//! pure `left_visibility` replaces the old per-frame `sidebar.open = !right_open`
//! stomp; its DEFAULT (`AutoCollapse`) reproduces that formula bit-for-bit so the
//! prior manual-toggle no-op behavior is preserved exactly. See
//! `fe-ui/src/ui_shell/AGENTS.md` §left.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::UiManager;
use crate::atlas::DashboardState;
use crate::navigation_manager::NavigationManager;
use crate::panels::sidebar;
use crate::plugin::{CameraFocusTarget, SidebarState};
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

/// Left-sidebar visibility policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeftSidebarPolicy {
    /// Auto-collapse when the right region is open (portal OR selection).
    /// Reproduces the pre-refactor `sidebar.open = !right_open` behavior exactly —
    /// `user_intent` is ignored, matching the old manual-toggle no-op.
    #[default]
    AutoCollapse,
    /// Honor explicit user intent, ignoring the right region (NOT the default;
    /// the seam for a future manual sidebar toggle).
    Manual,
}

/// Left-sidebar manager state: the visibility policy + the user's explicit
/// open/close intent (honored only under `Manual`).
#[derive(Resource, Debug, Clone)]
pub struct LeftSidebarState {
    pub policy: LeftSidebarPolicy,
    pub user_intent: bool,
}

impl Default for LeftSidebarState {
    fn default() -> Self {
        // `SidebarState` defaulted to open=true; keep intent=true so a later flip
        // to `Manual` starts open.
        Self {
            policy: LeftSidebarPolicy::default(),
            user_intent: true,
        }
    }
}

/// Pure visibility decision. DEFAULT policy = today's `!right_open`, where
/// `right_open == portal_is_open() || selected_entity().is_some()`.
pub fn left_visibility(policy: LeftSidebarPolicy, right_open: bool, user_intent: bool) -> bool {
    match policy {
        LeftSidebarPolicy::AutoCollapse => !right_open,
        LeftSidebarPolicy::Manual => user_intent,
    }
}

/// Applies the visibility policy, then renders the left sidebar. Replaces both
/// the old `sidebar::left_sidebar(...)` call AND the post-render
/// `sidebar.open = !right_open` stomp (the manager now owns that decision).
pub fn render_left_sidebar(
    ctx: &egui::Context,
    state: &LeftSidebarState,
    sidebar_state: &mut SidebarState,
    right_open: bool,
    nav: &mut NavigationManager,
    dashboard: &DashboardState,
    hierarchy: &mut VerseManager,
    camera_focus: &mut CameraFocusTarget,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
) {
    sidebar_state.open = left_visibility(state.policy, right_open, state.user_intent);
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
    fn autocollapse_default_reproduces_legacy_formula() {
        // Legacy: sidebar.open = !(portal_is_open() || selected.is_some()) = !right_open,
        // with `user_intent` a no-op. Reproduce that truth table bit-for-bit.
        let p = LeftSidebarPolicy::default();
        assert_eq!(p, LeftSidebarPolicy::AutoCollapse);
        assert!(left_visibility(p, false, true)); // right closed -> visible
        assert!(left_visibility(p, false, false)); // ... independent of intent
        assert!(!left_visibility(p, true, true)); // right open -> collapsed
        assert!(!left_visibility(p, true, false)); // ... independent of intent
    }

    #[test]
    fn default_state_is_autocollapse_open() {
        let s = LeftSidebarState::default();
        assert_eq!(s.policy, LeftSidebarPolicy::AutoCollapse);
        assert!(s.user_intent);
    }

    #[test]
    fn manual_policy_honors_user_intent() {
        let p = LeftSidebarPolicy::Manual;
        assert!(left_visibility(p, true, true)); // stays open despite right open
        assert!(!left_visibility(p, false, false)); // stays closed despite right closed
    }
}
