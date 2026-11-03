// Bevy UI systems and panel functions commonly exceed clippy's 7-argument
// limit because each piece of UI state is a separate Bevy resource.
#![allow(clippy::too_many_arguments)]

pub mod accessibility;
pub mod actions;
pub mod asset_ops;
pub mod atlas;
pub mod dialogs;
pub mod editor_batch;
pub mod file_dialogs;
pub mod geometry;
pub mod gimbal;
pub mod gis;
pub mod maintenance;
pub(crate) mod invitations;
pub mod gpx_ops;
pub mod navigation_manager;
pub mod node_manager;
pub mod notice;
pub(crate) mod object_data;
pub mod panels;
pub mod path_ops;
pub mod plugin;
pub mod portal;
pub mod role_chip;
pub(crate) mod scene_analysis;
pub mod scene_assets;
pub mod sculpt_cursor;
pub mod settings;
pub mod terrain_map;
pub mod terrain_preview;
pub mod terrain_proposal_state;
pub mod theme;
pub mod timeseries_settings;
pub mod ui_shell;
pub mod undo;
pub mod verse_manager;
pub mod viewport;
pub mod viewport_labels;
pub mod visibility;
