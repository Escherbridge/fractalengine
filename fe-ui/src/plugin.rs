use crate::{panels, role_chip};
use bevy::prelude::{App, Plugin, Update};

pub struct GardenerConsolePlugin;

impl Plugin for GardenerConsolePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_egui::EguiPlugin::default());
        app.add_systems(
            Update,
            (
                panels::node_status_panel,
                panels::petal_list_panel,
                panels::peer_manager_panel,
                panels::role_editor_panel,
                panels::asset_browser_panel,
                panels::session_monitor_panel,
                role_chip::role_chip_hud,
            ),
        );
    }
}
