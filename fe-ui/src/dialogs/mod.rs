//! Floating dialogs and the `ActiveDialog` mutual-exclusion enum. See
//! `fe-ui/src/AGENTS.md` §dialogs.

mod context_menu;
mod create_entity;
pub(crate) mod entity_details;
pub(crate) mod entity_access;
mod entity_settings;
mod gltf_import;
mod hexon_manager;
mod join;
mod node_options;
mod peer_debug;
mod petal_manifest;
pub(crate) mod requirements_state;

pub(crate) use context_menu::node_options_prefill;
pub use context_menu::render_context_menu;
pub use create_entity::render_create_dialog;
pub use entity_settings::render_entity_settings_dialog;
pub use gltf_import::render_gltf_import_dialog;
pub use hexon_manager::render_hexon_manager;
pub use join::render_join_dialog;
pub use node_options::{node_options_save_url, render_node_options_dialog};
pub use peer_debug::render_peer_debug_panel;
pub use petal_manifest::render_petal_manifest;

use crate::terrain_map::dto::{
    AvailableTilesetDto, HexonManagerTab, InstalledTilesetDto, StorageInfoDto,
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
    /// The token's `client` claim; `None` for a legacy (node-level) token.
    pub client: Option<String>,
}

/// The classified object a right-click context menu acts on (T4 FR-1): the
/// viewport `HitTarget` (same taxonomy as left-click) plus the payloads its
/// verbs need. Filled by `node_manager::context_pick::classify_context_menu`.
/// See `dialogs/AGENTS.md` §context-menu.
#[derive(Debug, Clone)]
pub struct ContextTarget {
    /// Petal owning this classified hit for the lifetime of the menu.
    pub petal_id: Option<String>,
    /// Classification driving `context_menu::menu_for`.
    pub hit: crate::node_manager::HitTarget,
    /// The object's node id when the hit is node-backed (node / track).
    pub node_id: Option<String>,
    /// `(track_node_id, stamp_index)` when the hit is a stamp (pre-promotion id).
    pub stamp: Option<(String, usize)>,
}

/// Which floating dialog is currently open. At most one at a time.
/// Replaces 7 separate dialog-state resources.
///
/// `large_enum_variant` is allowed rather than satisfied: exactly one
/// `ActiveDialog` exists per app (held in a Bevy resource), it changes only
/// when a dialog opens/closes/switches tabs, and every render/update site
/// matches it by reference (`&mut ActiveDialog` in `ResMut`). Boxing
/// `EntitySettings`'s ~20 fields would add an allocation and an indirection
/// to every field access across its 15 call sites to save ~220 bytes on a
/// value that is never copied in a hot path — the same trade the lint exists
/// to prompt against, per the precedent at
/// `fe-api/src/canonical_ws/handler.rs::PinnedSessionStatus`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Default)]
pub enum ActiveDialog {
    #[default]
    None,
    TreeAppearance {
        draft: crate::panels::tree_appearance::AppearanceDraft,
    },
    CreateEntity {
        kind: CreateKind,
        parent_id: String,
        name_buf: String,
    },
    ContextMenu {
        screen_pos: [f32; 2],
        world_pos: [f32; 3],
        /// `None` until the classifier runs (≤1 frame); the menu renders a
        /// placeholder until then — never a wrong-object menu.
        target: Option<ContextTarget>,
        /// Two-step delete confirmation state (armed inside the menu).
        pending_delete: bool,
        /// Live descendant count for the cascade confirm (`NodeDescendantCount`).
        descendant_count: Option<usize>,
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
        /// Live descendant count for the cascade confirm (`NodeDescendantCount`).
        descendant_count: Option<usize>,
    },
    JoinDialog {
        invite_buf: String,
    },
    PeerDebug,
    HexonManager {
        installed_tilesets: Vec<InstalledTilesetDto>,
        available_tilesets: Vec<AvailableTilesetDto>,
        filter_text: String,
        active_tab: HexonManagerTab,
        storage_info: StorageInfoDto,
        loading: bool,
        pending_remove: Option<String>,
        /// PAS-P5 S7b: the "Install from registry" URI text buffer.
        registry_uri_buf: String,
    },
    PetalManifest {
        petal_id: String,
        petal_name: String,
        manifest: PetalManifest,
        /// Hexon IDs available locally (from the global hexon store).
        available_hexon_ids: Vec<String>,
        add_hexon_id_buf: String,
        add_hexon_type_buf: String,
        requirements_state: requirements_state::RequirementsState,
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
        details: entity_details::EntityDetailsState,
        active_tab: SettingsTab,
        // General tab state
        name_buf: String,
        default_access_buf: Option<String>,
        description_buf: Option<String>,
        // Access tab state
        access: entity_access::EntityAccessState,
        invite_role_buf: String,
        invite_expiry_buf: u32,
        // Confirmation state
        pending_delete: bool,
        /// F6: two-step confirm for "Claim ownership" (Verse only) — the
        /// DB handler (`ClaimOrphanedVerse`) enforces real eligibility; this
        /// is just the explicit-click gate. See `dialogs/AGENTS.md` §claim-ownership.
        pending_claim_ownership: bool,
        // API Access tab state
        api_token_role_buf: String,
        api_token_expiry_buf: u32,
        /// Ed25519 `did:key` to mint for; blank mints a fresh client (W6-7b).
        api_token_client_buf: String,
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
