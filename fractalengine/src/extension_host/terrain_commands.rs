//! Dispatches the terrain package's slot commands to editor actions; see src/AGENTS.md §extension-host.

use bevy::prelude::*;
use fe_terrain::extension::TERRAIN_EXTENSION_ID;
use fe_ui::actions::{UiAction, UiManager};
use fe_ui::extensions::slots::{ExtensionCommandRequest, ExtensionSlotState};
use fe_ui::file_dialogs::FileDialogRequest;
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

/// Commands this dispatcher understands (the terrain manifest's command ids).
const CMD_CONFIG: &str = "terrain-config";
const CMD_GPX_IMPORT: &str = "gpx-import";
const CMD_TOGGLE: &str = "terrain-toggle";

/// Count of terrain commands with an unrecognised id, ignored.
#[derive(Resource, Debug, Default)]
pub struct TerrainCommandStats {
    pub ignored_unknown: u64,
}

/// Registers the terrain command dispatcher.
pub struct TerrainCommandsPlugin;

impl Plugin for TerrainCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainCommandStats>()
            .add_systems(Update, dispatch_terrain_commands);
    }
}

/// Drains slot commands, acts on the terrain package's, and re-queues every other package's untouched.
pub fn dispatch_terrain_commands(
    slots: Option<ResMut<ExtensionSlotState>>,
    sidebar: Option<ResMut<RightSidebarState>>,
    ui: Option<ResMut<UiManager>>,
    nav: Option<Res<NavigationManager>>,
    mut stats: ResMut<TerrainCommandStats>,
) {
    let (Some(mut slots), Some(mut sidebar), Some(mut ui)) = (slots, sidebar, ui) else {
        return;
    };
    if slots.pending_commands.is_empty() {
        return;
    }
    let mut foreign: Vec<ExtensionCommandRequest> = Vec::new();
    for request in slots.take_commands() {
        if request.plugin_id != TERRAIN_EXTENSION_ID {
            foreign.push(request);
            continue;
        }
        match request.command.as_str() {
            CMD_CONFIG => sidebar.reveal(RightSidebarSection::TerrainTools),
            CMD_GPX_IMPORT => import_gpx(&mut ui, nav.as_deref()),
            CMD_TOGGLE => ui.push_action(UiAction::ToggleTerrainVisibility),
            other => {
                stats.ignored_unknown += 1;
                debug!(command = other, "ignored unknown terrain command");
            }
        }
    }
    // Other packages' commands stay queued for their own dispatcher.
    slots.pending_commands.extend(foreign);
}

/// Opens the GPX file dialog for the active petal and queues the import.
fn import_gpx(ui: &mut UiManager, nav: Option<&NavigationManager>) {
    let Some(petal_id) = nav.and_then(|n| n.active_petal_id.clone()) else {
        warn!("gpx-import ignored: no active petal");
        return;
    };
    let verse_id = nav.and_then(|n| n.active_verse_id.clone());
    let chosen = ui.file_dialogs.choose(FileDialogRequest {
        kind: fe_runtime::qa_debug::DialogKind::Open,
        verse_id: verse_id.as_deref(),
        filters: &[("GPX Tracks", &["gpx"]), ("All Files", &["*"])],
        suggested_name: None,
    });
    match chosen {
        Ok(Some(path)) => ui.push_action(UiAction::GpxImportFile { petal_id, path }),
        Ok(None) => {}
        Err(error) => ui.show_error(error.to_string(), 0.0),
    }
}
