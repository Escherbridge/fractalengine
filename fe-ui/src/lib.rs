// Bevy UI systems and panel functions commonly exceed clippy's 7-argument
// limit because each piece of UI state is a separate Bevy resource.
#![allow(clippy::too_many_arguments)]

pub mod actions;
pub mod asset_ops;
pub mod atlas;
pub mod dialogs;
pub mod gimbal;
pub mod gis;
pub mod gpx_ops;
pub mod navigation_manager;
pub mod node_manager;
pub mod panels;
pub mod path_ops;
pub mod plugin;
pub mod portal;
pub mod role_chip;
pub mod terrain_map;
pub mod theme;
pub mod verse_manager;
pub mod viewport;
pub mod viewport_labels;
