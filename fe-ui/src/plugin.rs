use crate::{atlas::DashboardState, panels, panels::Tool, role_chip};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};

/// Sidebar visibility and search state.
#[derive(Resource)]
pub struct SidebarState {
    pub open: bool,
    pub tag_filter_buf: String,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            open: true,
            tag_filter_buf: String::new(),
        }
    }
}

/// Currently active editor tool.
#[derive(Resource, Default)]
pub struct ToolState {
    pub active_tool: Tool,
}

/// Inspector panel state: selection, transform buffers, URL fields.
#[derive(Resource)]
pub struct InspectorState {
    pub selected_entity: Option<Entity>,
    pub is_admin: bool,
    pub external_url: String,
    pub config_url: String,
    pub pos: [String; 3],
    pub rot: [String; 3],
    pub scale: [String; 3],
}

impl Default for InspectorState {
    fn default() -> Self {
        Self {
            selected_entity: None,
            is_admin: false,
            external_url: String::new(),
            config_url: String::new(),
            pos: ["0.00".into(), "0.00".into(), "0.00".into()],
            rot: ["0.00".into(), "0.00".into(), "0.00".into()],
            scale: ["1.00".into(), "1.00".into(), "1.00".into()],
        }
    }
}

/// Verse and fractal navigation state.
#[derive(Resource, Default)]
pub struct NavigationState {
    pub active_verse_id: Option<String>,
    pub active_verse_name: String,
    pub active_fractal_id: Option<String>,
    pub active_fractal_name: String,
    pub active_petal_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_state_default_has_no_active_verse() {
        let state = NavigationState::default();
        assert!(state.active_verse_id.is_none());
        assert!(state.active_fractal_id.is_none());
        assert!(state.active_petal_id.is_none());
    }
}

pub struct GardenerConsolePlugin;

impl Plugin for GardenerConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SidebarState>();
        app.init_resource::<ToolState>();
        app.init_resource::<InspectorState>();
        app.init_resource::<NavigationState>();
        app.init_resource::<DashboardState>();
        app.add_systems(EguiPrimaryContextPass, gardener_ui_system);
    }
}

fn gardener_ui_system(
    mut ctx: EguiContexts,
    mut sidebar: ResMut<SidebarState>,
    mut tool: ResMut<ToolState>,
    mut inspector: ResMut<InspectorState>,
    nav: Res<NavigationState>,
    dashboard: Res<DashboardState>,
) {
    let Ok(ectx) = ctx.ctx_mut() else { return };

    panels::gardener_console(ectx, &mut sidebar, &mut tool, &mut inspector, &nav, &dashboard);
    let role_label = if inspector.is_admin { "admin" } else { "member" };
    role_chip::role_chip_hud(ectx, role_label);
}
