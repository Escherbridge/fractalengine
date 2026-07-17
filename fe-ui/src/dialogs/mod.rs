//! Floating dialogs and the `ActiveDialog` mutual-exclusion enum. See
//! `fe-ui/src/AGENTS.md` §dialogs.

mod context_menu;
mod create_entity;
mod entity_settings;
mod gltf_import;
mod hexon_manager;
mod join;
mod node_options;
mod peer_debug;
mod petal_manifest;

pub use context_menu::render_context_menu;
pub use create_entity::{apply_create, render_create_dialog};
pub use entity_settings::render_entity_settings_dialog;
pub use gltf_import::render_gltf_import_dialog;
pub use hexon_manager::render_hexon_manager;
pub use join::render_join_dialog;
pub use node_options::{node_options_save_url, render_node_options_dialog};
pub use peer_debug::render_peer_debug_panel;
pub use petal_manifest::render_petal_manifest;

use std::collections::HashMap;

use crate::terrain_map::dto::{
    AvailableTilesetDto, DownloadProgress, HexonManagerTab, InstalledTilesetDto, StorageInfoDto,
};
use crate::terrain_map::manifest::PetalManifest;

/// Which "create entity" kind the Create dialog is targeting.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum CreateKind {
    #[default]
    Verse,
    Fractal,
    Petal,
    Node,
}

/// Which entity type the Entity Settings dialog is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitySettingsType {
    Verse,
    Fractal,
    Petal,
}

/// Active tab in the Entity Settings dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Access,
    ApiAccess,
}

/// A peer's resolved role at a specific scope, for display in the Access tab.
#[derive(Debug, Clone)]
pub struct PeerRoleEntry {
    pub peer_did: String,
    pub display_name: String,
    pub role: String,
    pub is_online: bool,
}

/// An API token record for display in the API Access tab.
#[derive(Debug, Clone)]
pub struct ApiTokenEntry {
    pub jti: String,
    pub scope: String,
    pub max_role: String,
    pub label: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub revoked: bool,
    /// DID of the node that minted this token.
    pub sub: String,
}

/// Which floating dialog is currently open. At most one at a time.
/// Replaces 7 separate dialog-state resources.
#[derive(Debug, Clone, Default)]
pub enum ActiveDialog {
    #[default]
    None,
    CreateEntity {
        kind: CreateKind,
        parent_id: String,
        name_buf: String,
    },
    ContextMenu {
        screen_pos: [f32; 2],
        world_pos: [f32; 3],
    },
    GltfImport {
        file_path_buf: String,
        name_buf: String,
        position: [f32; 3],
    },
    NodeOptions {
        node_id: String,
        node_name_buf: String,
        webpage_url_buf: String,
        /// Two-step delete confirmation state.
        pending_delete: bool,
    },
    InviteDialog {
        invite_string: String,
        include_write_cap: bool,
        expiry_hours: u32,
    },
    JoinDialog {
        invite_buf: String,
    },
    PeerDebug,
    HexonManager {
        installed_tilesets: Vec<InstalledTilesetDto>,
        available_tilesets: Vec<AvailableTilesetDto>,
        download_progress: HashMap<String, DownloadProgress>,
        filter_text: String,
        active_tab: HexonManagerTab,
        storage_info: StorageInfoDto,
        loading: bool,
        pending_remove: Option<String>,
    },
    PetalManifest {
        petal_id: String,
        petal_name: String,
        manifest: PetalManifest,
        /// Hexon IDs available locally (from the global hexon store).
        available_hexon_ids: Vec<String>,
        add_hexon_id_buf: String,
        add_hexon_type_buf: String,
        render_distance_buf: String,
        dirty: bool,
    },
    EntitySettings {
        entity_type: EntitySettingsType,
        entity_id: String,
        entity_name: String,
        /// Parent verse ID (always set; needed for correct scope strings).
        parent_verse_id: String,
        /// Parent fractal ID (set when entity is a Petal).
        parent_fractal_id: Option<String>,
        active_tab: SettingsTab,
        // General tab state
        name_buf: String,
        default_access_buf: Option<String>,
        description_buf: Option<String>,
        // Access tab state
        peer_roles: Vec<PeerRoleEntry>,
        roles_loading: bool,
        invite_role_buf: String,
        invite_expiry_buf: u32,
        generated_invite_link: Option<String>,
        // Confirmation state
        pending_delete: bool,
        // API Access tab state
        api_tokens: Vec<ApiTokenEntry>,
        api_tokens_loading: bool,
        api_token_scope_buf: String,
        api_token_role_buf: String,
        api_token_expiry_buf: u32,
        generated_api_token: Option<String>,
        /// Tokens scoped to this entity's scope tree (admin view).
        scoped_api_tokens: Vec<ApiTokenEntry>,
        scoped_tokens_loading: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_dialog_default_is_none() {
        assert!(matches!(ActiveDialog::default(), ActiveDialog::None));
    }
}
