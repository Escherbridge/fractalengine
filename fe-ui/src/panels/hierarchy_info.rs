//! Hierarchy info card for the Inspector; see panels/AGENTS.md section "hierarchy info".

use bevy::prelude::Resource;
use bevy_egui::egui;
use fe_runtime::PeerRegistry;

use crate::actions::UiManager;
use crate::dialogs::EntitySettingsType;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::plugin::LocalUserRole;
use crate::theme;
use crate::verse_manager::VerseManager;

use super::widgets;

/// A hierarchy level the user can pin the Inspector to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HierarchyLevel {
    Verse,
    Fractal,
    Petal,
}

/// What the Inspector is currently describing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectedEntity {
    Node { id: String },
    Petal { id: String },
    Fractal { id: String },
    Verse { id: String },
}

impl InspectedEntity {
    /// Kind tag used to detect "the inspected kind changed" (tab-state reset).
    pub fn level(&self) -> Option<HierarchyLevel> {
        match self {
            Self::Node { .. } => None,
            Self::Petal { .. } => Some(HierarchyLevel::Petal),
            Self::Fractal { .. } => Some(HierarchyLevel::Fractal),
            Self::Verse { .. } => Some(HierarchyLevel::Verse),
        }
    }
}

/// Selected node wins; else the override level; else the deepest active level.
pub fn inspected_entity(
    nav: &NavigationManager,
    node_mgr: &NodeManager,
    override_level: Option<HierarchyLevel>,
) -> Option<InspectedEntity> {
    if let Some(selection) = node_mgr.primary() {
        return Some(InspectedEntity::Node { id: selection.node_id.clone() });
    }
    let verse = || nav.active_verse_id.clone().map(|id| InspectedEntity::Verse { id });
    let fractal = || nav.active_fractal_id.clone().map(|id| InspectedEntity::Fractal { id });
    let petal = || nav.active_petal_id.clone().map(|id| InspectedEntity::Petal { id });
    match override_level {
        Some(HierarchyLevel::Verse) => verse(),
        Some(HierarchyLevel::Fractal) => fractal(),
        Some(HierarchyLevel::Petal) => petal(),
        None => None,
    }
    .or_else(petal)
    .or_else(fractal)
    .or_else(verse)
}

/// Sidebar re-click override; cleared on node selection or navigation change.
#[derive(Resource, Default, Debug)]
pub struct InspectedOverride {
    pub level: Option<HierarchyLevel>,
    seen_navigation: Option<[Option<String>; 3]>,
    seen_level: Option<HierarchyLevel>,
}

fn request_id() -> egui::Id {
    egui::Id::new("fe-hierarchy-inspect-request")
}

/// Sidebar side: ask for the info card of an already-active level.
pub(crate) fn request_inspect(ctx: &egui::Context, level: HierarchyLevel) {
    ctx.data_mut(|data| data.insert_temp(request_id(), level));
}

impl InspectedOverride {
    /// Once per frame before `inspected_entity`: clear on change, then apply a sidebar request.
    pub fn sync(&mut self, ctx: &egui::Context, nav: &NavigationManager, node_mgr: &NodeManager) {
        let unchanged = self.seen_navigation.as_ref().is_some_and(|seen| {
            seen[0] == nav.active_verse_id
                && seen[1] == nav.active_fractal_id
                && seen[2] == nav.active_petal_id
        });
        if !unchanged {
            self.seen_navigation = Some([
                nav.active_verse_id.clone(),
                nav.active_fractal_id.clone(),
                nav.active_petal_id.clone(),
            ]);
            self.level = None;
        }
        if node_mgr.primary().is_some() {
            self.level = None;
        }
        let requested = ctx.data_mut(|data| data.remove_temp::<HierarchyLevel>(request_id()));
        if requested.is_some() {
            self.level = requested;
        }
    }

    /// True when the inspected hierarchy kind differs from last frame (reset per-tab state then).
    pub fn kind_changed(&mut self, current: Option<&InspectedEntity>) -> bool {
        let level = current.and_then(InspectedEntity::level);
        let changed = level != self.seen_level;
        self.seen_level = level;
        changed
    }
}

/// Display model of one hierarchy entity.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HierarchyInfo {
    pub kind: EntitySettingsType,
    pub header: String,
    pub name: String,
    pub id: String,
    pub counts: Vec<String>,
    pub verse_id: String,
    pub fractal_id: Option<String>,
}

fn plural(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
}

/// Resolve an entity against the in-memory tree; `None` for a Node or a stale id.
pub(crate) fn describe(
    hierarchy: &VerseManager,
    peer_count: usize,
    entity: &InspectedEntity,
) -> Option<HierarchyInfo> {
    match entity {
        InspectedEntity::Node { .. } => None,
        InspectedEntity::Verse { id } => {
            let verse = hierarchy.verses.iter().find(|verse| &verse.id == id)?;
            Some(HierarchyInfo {
                kind: EntitySettingsType::Verse,
                header: format!("Verse: {}", verse.name),
                name: verse.name.clone(),
                id: verse.id.clone(),
                counts: vec![plural(verse.fractals.len(), "fractal"), plural(peer_count, "known peer")],
                verse_id: verse.id.clone(),
                fractal_id: None,
            })
        }
        InspectedEntity::Fractal { id } => hierarchy.verses.iter().find_map(|verse| {
            let fractal = verse.fractals.iter().find(|fractal| &fractal.id == id)?;
            Some(HierarchyInfo {
                kind: EntitySettingsType::Fractal,
                header: format!("Fractal: {}", fractal.name),
                name: fractal.name.clone(),
                id: fractal.id.clone(),
                counts: vec![plural(fractal.petals.len(), "petal")],
                verse_id: verse.id.clone(),
                fractal_id: None,
            })
        }),
        InspectedEntity::Petal { id } => hierarchy.verses.iter().find_map(|verse| {
            verse.fractals.iter().find_map(|fractal| {
                let petal = fractal.petals.iter().find(|petal| &petal.id == id)?;
                Some(HierarchyInfo {
                    kind: EntitySettingsType::Petal,
                    header: format!("Petal: {}", petal.name),
                    name: petal.name.clone(),
                    id: petal.id.clone(),
                    counts: vec![plural(petal.nodes.len(), "node")],
                    verse_id: verse.id.clone(),
                    fractal_id: Some(fractal.id.clone()),
                })
            })
        }),
    }
}

/// Denial copy when the local role cannot open Settings; `None` when allowed.
pub(crate) fn settings_denial(role: &LocalUserRole) -> Option<String> {
    (!role.can_edit()).then(|| widgets::role_denial_reason(role, "Editor", None))
}

/// Info card for a verse, fractal or petal; a Node renders nothing (the node form owns it).
pub fn render_hierarchy_info(
    ui: &mut egui::Ui,
    entity: &InspectedEntity,
    hierarchy: &VerseManager,
    local_role: &LocalUserRole,
    peers: &PeerRegistry,
    ui_mgr: &mut UiManager,
) {
    let Some(info) = describe(hierarchy, peers.len(), entity) else {
        return;
    };
    ui.label(egui::RichText::new(&info.header).strong());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Name").color(theme::TEXT_MUTED));
        widgets::clipped_value(ui, &info.name, theme::TEXT_BRIGHT);
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("ID").color(theme::TEXT_MUTED));
        widgets::clipped_value_copyable(ui, ui_mgr, "ID", &info.id, theme::TEXT_BRIGHT, "ID copied");
    });
    for count in &info.counts {
        ui.label(count);
    }
    ui.label(format!("Your role: {}", crate::notice::role_label(local_role.role)));
    ui.add_space(4.0);
    let denial = settings_denial(local_role).unwrap_or_default();
    let opened = widgets::role_gated(
        ui,
        denial.is_empty(),
        &denial,
        egui::Button::new("Settings...").min_size(egui::Vec2::splat(
            crate::ui_shell::layout_policy::target_size(ui.ctx()),
        )),
    );
    if opened.clicked() {
        ui_mgr.open_dialog(crate::dialogs::entity_details::dialog(
            info.kind,
            &info.id,
            &info.name,
            &info.verse_id,
            info.fractal_id.as_deref(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verse_manager::{FractalEntry, PetalEntry, VerseEntry};
    use fe_database::RoleLevel;

    fn fixture() -> VerseManager {
        let petal = |id: &str, name: &str| PetalEntry { id: id.into(), name: name.into(), ..Default::default() };
        VerseManager::from_verses(vec![VerseEntry {
            id: "v1".into(),
            name: "Atlas".into(),
            namespace_id: None,
            expanded: true,
            fractals: vec![FractalEntry {
                id: "f1".into(),
                name: "Harbour".into(),
                expanded: true,
                petals: vec![petal("p1", "Pier"), petal("p2", "Quay")],
            }],
        }])
    }

    fn navigation() -> NavigationManager {
        let mut nav = NavigationManager::default();
        nav.navigate_to_petal_in_fractal("v1", "Atlas", "f1", "Harbour", "p1");
        nav
    }

    #[test]
    fn hierarchy_info_follows_navigation_and_row_clicks() {
        let hierarchy = fixture();
        let mut nav = navigation();
        let mut node_mgr = NodeManager::default();
        let ctx = egui::Context::default();
        let mut override_state = InspectedOverride::default();

        // Nothing selected: the deepest active level, the petal.
        override_state.sync(&ctx, &nav, &node_mgr);
        let routed = inspected_entity(&nav, &node_mgr, override_state.level);
        assert_eq!(routed, Some(InspectedEntity::Petal { id: "p1".into() }));
        assert!(override_state.kind_changed(routed.as_ref()));
        assert!(!override_state.kind_changed(routed.as_ref()));

        // Re-click of the active verse row: override wins and survives a quiet frame.
        request_inspect(&ctx, HierarchyLevel::Verse);
        override_state.sync(&ctx, &nav, &node_mgr);
        let routed = inspected_entity(&nav, &node_mgr, override_state.level);
        assert_eq!(routed, Some(InspectedEntity::Verse { id: "v1".into() }));
        override_state.sync(&ctx, &nav, &node_mgr);
        assert_eq!(override_state.level, Some(HierarchyLevel::Verse));
        assert!(override_state.kind_changed(routed.as_ref()));

        // Navigating clears it.
        nav.navigate_to_petal_in_fractal("v1", "Atlas", "f1", "Harbour", "p2");
        override_state.sync(&ctx, &nav, &node_mgr);
        assert_eq!(override_state.level, None);
        assert_eq!(
            inspected_entity(&nav, &node_mgr, override_state.level),
            Some(InspectedEntity::Petal { id: "p2".into() })
        );

        // Header and count strings per level.
        let info = |entity: InspectedEntity, peers| describe(&hierarchy, peers, &entity).unwrap();
        let petal = info(InspectedEntity::Petal { id: "p1".into() }, 0);
        assert_eq!((petal.header.as_str(), petal.counts.as_slice()), ("Petal: Pier", ["0 nodes".to_string()].as_slice()));
        let fractal = info(InspectedEntity::Fractal { id: "f1".into() }, 0);
        assert_eq!((fractal.header.as_str(), fractal.counts.as_slice()), ("Fractal: Harbour", ["2 petals".to_string()].as_slice()));
        let verse = info(InspectedEntity::Verse { id: "v1".into() }, 1);
        assert_eq!(verse.header, "Verse: Atlas");
        assert_eq!(verse.counts, ["1 fractal".to_string(), "1 known peer".to_string()]);
        assert!(describe(&hierarchy, 0, &InspectedEntity::Node { id: "n1".into() }).is_none());

        // Settings is denied to a Viewer with the role_denial_reason text, allowed to an Editor.
        let viewer = LocalUserRole { role: Some(RoleLevel::Viewer) };
        assert_eq!(settings_denial(&viewer).as_deref(), Some("Editor role needed \u{2014} you are Viewer"));
        assert_eq!(settings_denial(&LocalUserRole { role: Some(RoleLevel::Editor) }), None);

        // The card renders for a verse without opening a dialog on its own.
        let mut ui_mgr = UiManager::default();
        let peers = PeerRegistry::default();
        let entity = InspectedEntity::Verse { id: "v1".into() };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_hierarchy_info(ui, &entity, &hierarchy, &viewer, &peers, &mut ui_mgr);
            });
        });
        assert!(!ui_mgr.any_dialog_open());
    }
}
