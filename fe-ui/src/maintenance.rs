//! Terminal editor invalidation; see src/AGENTS.md §terminal-maintenance.

use bevy::prelude::*;

fn reset<T: Resource + Default>(world: &mut World) {
    if world.contains_resource::<T>() {
        world.insert_resource(T::default());
    }
}

pub fn clear_read_models(world: &mut World) {
    if let Some(mut assets) = world.get_resource_mut::<crate::scene_assets::SceneAssets>() {
        assets.retire_for_terminal_reset();
    }
    if let Some(mut nav) = world.get_resource_mut::<crate::navigation_manager::NavigationManager>()
    {
        nav.back_from_verse();
    }
    if let Some(mut nodes) = world.get_resource_mut::<crate::node_manager::NodeManager>() {
        nodes.clear_for_terminal_reset();
    }
    if let Some(mut tree) = world.get_resource_mut::<crate::verse_manager::VerseManager>() {
        tree.clear_for_terminal_reset();
    }
    if let Some(mut ui) = world.get_resource_mut::<crate::actions::UiManager>() {
        ui.clear_for_terminal_reset();
    }
    reset::<crate::invitations::ReplicaHandoff>(world);
    reset::<crate::plugin::InspectorFormState>(world);
    reset::<crate::plugin::SidebarState>(world);
    reset::<crate::plugin::ToolState>(world);
    reset::<crate::plugin::LocalUserRole>(world);
    reset::<crate::plugin::CameraFocusTarget>(world);
    reset::<crate::plugin::ViewportCursorWorld>(world);
    reset::<crate::undo::UndoState>(world);
    reset::<crate::gis::GisPanelState>(world);
    reset::<crate::gis::PathEditorState>(world);
    reset::<crate::actions::asset::StampInteractionState>(world);
    reset::<crate::node_manager::StampSelectionCatalog>(world);
    reset::<crate::node_manager::StampSelectionRequests>(world);
    reset::<crate::verse_manager::PrimitiveDescriptorCache>(world);
    reset::<crate::verse_manager::PathAssetCache>(world);
    reset::<crate::verse_manager::StampRenderIndex>(world);
    reset::<crate::verse_manager::ResidencyLedger>(world);
    reset::<crate::terrain_map::PetalMapState>(world);
    reset::<crate::terrain_map::TilesetDownloads>(world);
    reset::<crate::terrain_map::PendingHexonOps>(world);
    reset::<crate::asset_ops::PendingAssetOps>(world);
    reset::<crate::gpx_ops::PendingGpxOps>(world);
    reset::<crate::path_ops::PendingPathOps>(world);
    reset::<crate::actions::terrain_proposal::SculptToolState>(world);
    reset::<crate::actions::terrain_proposal::EarthworkNodeMap>(world);
    reset::<crate::terrain_proposal_state::ProposalEditState>(world);
    reset::<crate::terrain_preview::PreviewReadout>(world);
    reset::<crate::panels::volume_report_panel::VolumeReportState>(world);
    reset::<crate::panels::tool_panel::ToolPanelState>(world);
    reset::<crate::scene_analysis::SceneMaterialCache>(world);
    reset::<crate::settings::PetalRenderOverride>(world);
}

pub fn clear_scene_asset_outcomes_after_database_join(world: &mut World) {
    reset::<crate::scene_assets::SceneAssets>(world);
    if let Some(mut ui) = world.get_resource_mut::<crate::actions::UiManager>() {
        ui.scene_asset_view = Default::default();
    }
}
