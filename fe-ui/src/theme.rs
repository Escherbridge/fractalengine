use bevy_egui::egui::Color32;

// Background colors
pub const BG_TOOLBAR: Color32 = Color32::from_rgb(22, 22, 28);
pub const BG_STATUSBAR: Color32 = Color32::from_rgb(18, 18, 24);
pub const BG_PANEL: Color32 = Color32::from_rgb(24, 24, 30);
pub const BG_BUTTON: Color32 = Color32::from_rgb(40, 40, 52);
pub const BG_BUTTON_ALT: Color32 = Color32::from_rgb(40, 40, 60);
pub const BG_BUTTON_ACTIVE: Color32 = Color32::from_rgb(60, 100, 160);
/// Active tool-MODE emphasis (ui_ux.md §1): a brighter NEUTRAL fill, so the
/// active mode reads via luminance rather than the saturated-blue hue shift of
/// `BG_BUTTON_ACTIVE`. Same hue family as `BG_BUTTON`, higher luminance.
pub const BG_MODE_ACTIVE: Color32 = Color32::from_rgb(88, 88, 104);
pub const BG_DANGER: Color32 = Color32::from_rgb(80, 40, 40);
/// Window close-X hover (ui_ux.md §1 destructive chrome; the platform's red close cue).
pub const BG_WINDOW_CLOSE_HOVER: Color32 = Color32::from_rgb(196, 43, 28);
pub const BG_SAVE: Color32 = Color32::from_rgb(40, 80, 40);
pub const BG_ROLE_CHIP: Color32 = Color32::from_rgb(30, 30, 50);

// Text colors
pub const TEXT_DIM: Color32 = Color32::from_rgb(155, 155, 175);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(145, 145, 165);
pub const TEXT_SECTION: Color32 = Color32::from_rgb(160, 160, 190);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(180, 180, 220);
pub const TEXT_BRIGHT: Color32 = Color32::from_rgb(200, 200, 230);
pub const TEXT_HEADING: Color32 = Color32::from_rgb(180, 180, 210);
pub const TEXT_AXIS: Color32 = Color32::from_rgb(160, 100, 100);

// Status colors
pub const STATUS_NORMAL: Color32 = Color32::from_rgb(155, 165, 175);
pub const STATUS_ONLINE_DOT: Color32 = STATUS_NORMAL;
pub const STATUS_ONLINE: Color32 = STATUS_NORMAL;
pub const STATUS_OFFLINE: Color32 = Color32::from_rgb(180, 100, 100);
pub const STATUS_OFFLINE_DOT: Color32 = Color32::from_rgb(180, 60, 60);

// Tree hierarchy
pub const TREE_NODE_ICON: Color32 = Color32::from_rgb(140, 180, 220);
pub const TREE_SELECTED_BG: Color32 = Color32::from_rgb(40, 55, 80);
pub const BG_CONTEXT_MENU: Color32 = Color32::from_rgb(30, 30, 40);
pub const BG_DIALOG: Color32 = Color32::from_rgb(28, 28, 38);

// Misc
pub const TEXT_VIEWPORT_HINT: Color32 = Color32::from_rgba_unmultiplied_const(180, 180, 200, 80);

// Settings dialog
pub const BG_TAB_ACTIVE: Color32 = Color32::from_rgb(64, 64, 78);
pub const BG_TAB_INACTIVE: Color32 = Color32::from_rgb(30, 30, 42);
pub const BG_PEER_ROW_EVEN: Color32 = Color32::from_rgb(26, 26, 34);
pub const BG_PEER_ROW_ODD: Color32 = Color32::from_rgb(30, 30, 40);
pub const TEXT_ROLE_VIEWER: Color32 = Color32::from_rgb(120, 160, 200);
pub const TEXT_ROLE_EDITOR: Color32 = Color32::from_rgb(140, 200, 140);
pub const TEXT_ROLE_MANAGER: Color32 = Color32::from_rgb(200, 180, 100);
pub const TEXT_ROLE_OWNER: Color32 = Color32::from_rgb(200, 140, 100);
pub const BG_INVITE_LINK: Color32 = Color32::from_rgb(35, 35, 50);
pub const ICON_GEAR: Color32 = Color32::from_rgb(140, 140, 170);
pub const ICON_ONLINE: Color32 = Color32::from_rgb(80, 180, 80);
pub const ICON_OFFLINE: Color32 = Color32::from_rgb(145, 145, 165);

// Data icons (data_icons_20260713): recolorable geometric glyphs prepended to
// Paths-tab rows + used as the billboard/overlay type marker. See
// `fe-ui/src/AGENTS.md` §data-icons.
pub const ICON_TRACK: Color32 = Color32::from_rgb(120, 190, 240);
pub const ICON_POINT: Color32 = Color32::from_rgb(240, 210, 90);
pub const ICON_WAYPOINT: Color32 = Color32::from_rgb(200, 150, 240);

// Shared feedback and neutral action tokens; see AGENTS.md ?feedback-status.
pub const BG_PRIMARY: Color32 = Color32::from_rgb(60, 60, 74);
pub const NOTICE_INFO_BG: Color32 = Color32::from_rgb(30, 42, 52);
pub const NOTICE_ADVISORY_BG: Color32 = Color32::from_rgb(58, 47, 24);
pub const NOTICE_ERROR_BG: Color32 = Color32::from_rgb(62, 29, 33);
pub const NOTICE_TEXT: Color32 = Color32::from_rgb(230, 230, 240);

/// Shared selected-tab fill.
pub fn tab_fill(active: bool) -> Color32 {
    if active {
        BG_TAB_ACTIVE
    } else {
        BG_TAB_INACTIVE
    }
}

/// Severity palette for the shared notice renderer.
pub fn notice_fill(severity: crate::notice::Severity) -> Color32 {
    match severity {
        crate::notice::Severity::Info => NOTICE_INFO_BG,
        crate::notice::Severity::Advisory => NOTICE_ADVISORY_BG,
        crate::notice::Severity::Error => NOTICE_ERROR_BG,
    }
}
