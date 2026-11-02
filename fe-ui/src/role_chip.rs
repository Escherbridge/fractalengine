use bevy_egui::egui;
use fe_database::RoleLevel;

use crate::accessibility::ResponseExt;
use crate::{navigation_manager::NavigationManager, plugin::LocalUserRole, theme};

/// Inline scoped role summary; unresolved roles remain explicit.
pub(crate) fn role_status(
    ui: &mut egui::Ui,
    local_role: &LocalUserRole,
    nav: &NavigationManager,
    identity: Option<&fe_identity::NodeIdentity>,
) {
    let scoped = nav.active_verse_id.is_some();
    let role = scoped.then_some(local_role.role).flatten();
    let label = match role {
        Some(RoleLevel::Owner) => "Owner",
        Some(RoleLevel::Manager) => "Manager",
        Some(RoleLevel::Editor) => "Editor",
        Some(RoleLevel::Viewer) => "Viewer",
        Some(RoleLevel::None) => "No access",
        None => "\u{2014}",
    };
    let summary = if scoped {
        format!("Role: {label} \u{00b7} {}", nav.active_verse_name)
    } else {
        format!("Role: {label}")
    };
    ui.add_sized(
        egui::vec2(
            ui.available_width().clamp(80.0, 220.0),
            ui.spacing().interact_size.y.min(18.0),
        ),
        egui::Label::new(egui::RichText::new(summary).small().color(theme::TEXT_DIM))
            .truncate()
            .selectable(false)
            .sense(egui::Sense::hover()),
    )
    .named(ui, format_args!("Current role: {label}"))
    .on_hover_ui(|ui| {
        if scoped {
            ui.label(format!("Verse: {}", nav.active_verse_name));
        }
        let capabilities = match role {
            Some(RoleLevel::Owner) => "View and edit content; manage access and ownership.",
            Some(RoleLevel::Manager) => "View and edit content; manage hierarchy and access.",
            Some(RoleLevel::Editor) => {
                "View and edit content. Hierarchy and access management require Manager."
            }
            Some(RoleLevel::Viewer) => {
                "View content. Editing requires Editor; access management requires Manager."
            }
            Some(RoleLevel::None) => "No access at the current scope.",
            None if scoped => {
                "Role unresolved. Editing and management remain unavailable until resolved."
            }
            None => "Navigate to a verse to resolve your role.",
        };
        ui.label(capabilities);
        ui.separator();
        if let Some(identity) = identity {
            ui.label("Local identity");
            ui.label(egui::RichText::new(&identity.did).monospace());
        } else {
            ui.label("Local identity unavailable");
        }
    });
}
