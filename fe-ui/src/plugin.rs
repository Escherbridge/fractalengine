use crate::{atlas::DashboardState, panels, panels::Tool, role_chip};
use bevy::prelude::*;
use bevy_egui::EguiContexts;

/// Persistent UI state: sidebar visibility, selection, active tool, inspector field buffers.
#[derive(Resource)]
pub struct UiState {
    pub sidebar_open: bool,
    pub selected_entity: Option<Entity>,
    pub active_tool: Tool,
    pub is_admin: bool,

    // Inspector field edit buffers
    pub inspector_external_url: String,
    pub inspector_config_url: String,

    // Sidebar search buffer
    pub tag_filter_buf: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            sidebar_open: true,
            selected_entity: None,
            active_tool: Tool::Select,
            is_admin: false,
            inspector_external_url: String::new(),
            inspector_config_url: String::new(),
            tag_filter_buf: String::new(),
        }
    }
}

pub struct GardenerConsolePlugin;

impl Plugin for GardenerConsolePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_egui::EguiPlugin::default());
        app.init_resource::<UiState>();
        app.init_resource::<DashboardState>();
        app.add_systems(Update, gardener_ui_system);
    }
}

fn gardener_ui_system(
    mut ctx: EguiContexts,
    mut ui_state: ResMut<UiState>,
    dashboard: Res<DashboardState>,
    mut warmup: Local<u8>,
) {
    // Egui needs a few frames to initialize after window creation.
    if *warmup < 3 {
        *warmup += 1;
        return;
    }
    let Ok(ectx) = ctx.ctx_mut() else { return };

    panels::gardener_console(ectx, &mut ui_state, &dashboard);
    role_chip::role_chip_hud(ectx);
}
