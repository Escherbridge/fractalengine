//! Portal webview lifecycle state + screen-rect sync math. See
//! `fe-ui/src/AGENTS.md` §portal.

use bevy::prelude::Entity;

/// Portal webview lifecycle state (owned by [`crate::actions::UiManager`]).
#[derive(Debug, Clone, Default)]
pub enum PortalState {
    #[default]
    Closed,
    Open {
        current_url: String,
        cached_hostname: String,
        opened_for_entity: Entity,
    },
}

/// Screen-space rect (in egui points) for the webview portal overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortalRectInsets {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Height reserved for the portal toolbar (back/url/close row).
const TOOLBAR_HEADER: f32 = 36.0;
/// Height reserved for the bottom status bar.
const STATUS_BAR: f32 = 22.0;
/// Horizontal gap left for the inspector panel's resize handle.
const LEFT_PAD: f32 = 6.0;

/// Pure: compute the webview portal rect from the full screen rect and the
/// viewport rect `gardener_console` returns (the CentralPanel bounds, which
/// end where the inspector/portal side panel begins).
pub fn compute_portal_rect(
    screen: bevy_egui::egui::Rect,
    viewport_rect: bevy_egui::egui::Rect,
) -> PortalRectInsets {
    PortalRectInsets {
        x: viewport_rect.right() + LEFT_PAD,
        y: viewport_rect.top() + TOOLBAR_HEADER,
        width: (screen.right() - viewport_rect.right() - LEFT_PAD).max(1.0),
        height: (viewport_rect.height() - TOOLBAR_HEADER - STATUS_BAR).max(1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_egui::egui::{pos2, Rect};

    #[test]
    fn compute_portal_rect_insets_for_toolbar_and_status_bar() {
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(1200.0, 800.0));
        // Inspector/viewport rect ends at x=900 (right panel starts there).
        let viewport_rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(900.0, 800.0));

        let insets = compute_portal_rect(screen, viewport_rect);

        assert_eq!(insets.x, 906.0); // 900 + left_pad(6)
        assert_eq!(insets.y, 36.0); // top(0) + toolbar_header(36)
        assert_eq!(insets.width, 294.0); // 1200 - 900 - 6
        assert_eq!(insets.height, 742.0); // 800 - 36 - 22
    }

    #[test]
    fn compute_portal_rect_clamps_width_and_height_to_min_one() {
        // Degenerate case: viewport fills the screen and is shorter than the
        // toolbar+status insets (58), so both dimensions must clamp.
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(500.0, 50.0));
        let viewport_rect = screen;

        let insets = compute_portal_rect(screen, viewport_rect);

        assert_eq!(insets.width, 1.0);
        assert_eq!(insets.height, 1.0);
    }
}
