// Bevy UI systems and panel functions commonly exceed clippy's 7-argument
// limit because each piece of UI state is a separate Bevy resource.
#![allow(clippy::too_many_arguments)]

pub mod atlas;
pub mod gimbal;
pub mod navigation_manager;
pub mod node_manager;
pub mod panels;
pub mod plugin;
pub mod role_chip;
pub mod theme;
pub mod verse_manager;
