//! Hierarchy tree; see panels/AGENTS.md sidebar section.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::tree_appearance::EntryKind;
use super::tree_entry::{self, RowSelection, Target};
use crate::actions::{UiAction, UiManager};
use crate::atlas::DashboardState;
use crate::dialogs::{ActiveDialog, CreateKind};
use crate::navigation_manager::NavigationManager;
use crate::plugin::{CameraFocusTarget, SidebarState};
use crate::theme;
use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseManager};
use crate::visibility::{self, NodeAncestry, OverrideState, VisibilityState};
use fe_runtime::messages::{DbCommand, ScopeLevel};

pub(crate) fn left_sidebar(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    width: &mut crate::ui_shell::layout_policy::PanelWidthState,
    nav: &mut NavigationManager,
    _dashboard: &DashboardState,
    hierarchy: &mut VerseManager,
    camera_focus: &mut CameraFocusTarget,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
    settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
) {
    use crate::ui_shell::layout_policy::{self, PanelSide};
    let id = egui::Id::new("fe-hierarchy-panel-divider");
    let bounds = [120.0, layout_policy::left_width(ctx)];
    let effective = width.prepare(ctx, id, PanelSide::Left, bounds);
    let metrics = layout_policy::divider_metrics(ctx);
    let panel = egui::SidePanel::left("sidebar")
        .resizable(false)
        .exact_width(effective)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(metrics.map_or(egui::Margin::ZERO, |m| m.side_margin(PanelSide::Left))),
        )
        .show_animated(ctx, sidebar.open, |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
            sidebar_verse_header(ui, nav, db_tx, ui_mgr);
            ui.separator();

            let tree_height = ui.available_height().max(40.0);
            crate::ui_shell::layout_policy::scroll(
                ui,
                "Hierarchy",
                [true, true],
                egui::ScrollArea::both()
                    .max_height(tree_height)
                    .auto_shrink([false; 2]),
                |ui| {
                    ui.set_min_width(metrics.map_or(0.0, |m| m.hierarchy_content_width()));
                    configure_tree_spacing(ui);
                    ui.add_space(4.0);
                    render_verse_tree(
                        ui,
                        hierarchy,
                        nav,
                        camera_focus,
                        node_mgr,
                        ui_mgr,
                        vis_state,
                        settings,
                        local_role,
                    );
                    ui.add_space(8.0);
                    sidebar_section_groups(ui, nav, ui_mgr, vis_state);

                    ui.add_space(4.0);
                },
            );
        });
    if let Some(panel) = panel {
        width.register(ctx, id, PanelSide::Left, bounds, panel.response.rect);
    } else {
        width.suspend();
    }
}

fn configure_tree_spacing(ui: &mut egui::Ui) {
    let minimum = 24.0_f32.max(20.0 / ui.ctx().pixels_per_point());
    ui.spacing_mut().indent = minimum;
    ui.spacing_mut().icon_width = minimum;
    ui.spacing_mut().interact_size.y = minimum;
}

fn sidebar_verse_header(
    ui: &mut egui::Ui,
    nav: &NavigationManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    ui_mgr: &mut UiManager,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Verse:").small().color(theme::TEXT_DIM));
        let name = if nav.active_verse_id.is_some() { nav.active_verse_name.as_str() } else { "No Verse" };
        ui.add_sized(
            egui::vec2((ui.available_width() - 8.0).max(0.0), ui.spacing().interact_size.y),
            egui::Label::new(egui::RichText::new(name).strong().color(theme::TEXT_STRONG)).truncate(),
        ).on_hover_text(name);
    });
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.add(egui::Button::new("Join").fill(theme::BG_BUTTON)
            .min_size(egui::vec2(0.0, 24.0)))
            .named(ui, "Join verse by invite")
            .on_hover_text("Join a verse by invite").clicked() {
            ui_mgr.open_dialog(ActiveDialog::JoinDialog { invite_buf: String::new() });
        }
        if ui.add(egui::Button::new("+").fill(theme::BG_BUTTON_ALT)
            .min_size(egui::vec2(24.0, 24.0)))
            .named(ui, "Create new verse")
            .on_hover_text("Create new Verse").clicked() {
            ui_mgr.open_dialog(ActiveDialog::CreateEntity {
                kind: CreateKind::Verse, parent_id: String::new(), name_buf: String::new(),
            });
        }
    });
    ui.add_space(6.0);
}

/// Right-aligned eye-toggle button (spec §4.1 gutter idiom). Returns `true`
/// if clicked this frame. `enabled=false` disables it (RATIFICATION #4: the
/// active petal's eye is disabled + tooltipped, never clickable).
fn eye_button(
    ui: &mut egui::Ui,
    glyph: &str,
    enabled: bool,
    tooltip: &str,
    name: impl std::fmt::Display,
) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(glyph)
            .small()
            .color(theme::TREE_NODE_ICON),
    )
    .frame(false)
    .min_size(egui::Vec2::splat(
        24.0_f32.max(20.0 / ui.ctx().pixels_per_point()),
    ));
    ui.add_enabled(enabled, btn)
        .named(ui, name)
        .on_hover_text(tooltip)
        .clicked()
}

#[allow(clippy::too_many_arguments)]
fn render_verse_tree(
    ui: &mut egui::Ui,
    hierarchy: &mut VerseManager,
    nav: &mut NavigationManager,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
    settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
) {
    let verse_count = hierarchy.verses.len();
    for vi in 0..verse_count {
        let verse_id = hierarchy.verses[vi].id.clone();
        let verse_name = hierarchy.verses[vi].name.clone();
        let is_active_verse = nav.active_verse_id.as_deref() == Some(&verse_id);

        let own_visible = visibility::verse_effective_visible(&verse_id, vis_state);
        let rollup_state = visibility::rollup(
            hierarchy.verses[vi]
                .fractals
                .iter()
                .map(|f| visibility::fractal_effective_visible(&f.id, &verse_id, vis_state)),
            own_visible,
        );
        let glyph = visibility::glyph_for_rollup(rollup_state);
        let eye_tooltip = if own_visible {
            "Hide verse"
        } else {
            "Show verse"
        };

        let id = ui.make_persistent_id(egui::Id::new(format!("verse_{}", verse_id)));
        let mut name_clicked = false;
        let mut eye_clicked = false;
        let header =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    let (response, eye) = tree_entry::row(
                        ui,
                        Target {
                            kind: EntryKind::Verse,
                            id: &verse_id,
                            name: &verse_name,
                            verse_id: &verse_id,
                            fractal_id: None,
                            petal_id: None,
                            url: None,
                        },
                        RowSelection::from_primary(
                            is_active_verse
                                && nav.active_fractal_id.is_none()
                                && node_mgr.primary().is_none(),
                        ),
                        is_active_verse,
                        own_visible,
                        nav,
                        local_role,
                        settings,
                        ui_mgr,
                        |ui| {
                            eye_button(
                                ui,
                                glyph,
                                true,
                                eye_tooltip,
                                format_args!("{eye_tooltip}: verse {verse_name}"),
                            )
                        },
                        |_| {},
                    );
                    name_clicked = response.clicked();
                    eye_clicked = eye;
                });
        let (toggle_response, _, _) = header.body(|ui| {
            render_fractals(
                ui,
                &mut hierarchy.verses[vi].fractals,
                nav,
                &verse_id,
                &verse_name,
                camera_focus,
                node_mgr,
                ui_mgr,
                vis_state,
                settings,
                local_role,
            );
            // [+] Add Fractal inside the verse collapse
            add_button_inline(
                ui,
                "+ Fractal",
                CreateKind::Fractal,
                &verse_id,
                ui_mgr,
                local_role.can_manage()
                    && is_active_verse
                    && nav.active_fractal_id.is_none()
                    && nav.active_petal_id.is_none(),
            );
        });
        toggle_response.named(ui, format_args!("Expand or collapse verse {verse_name}"));

        if name_clicked && !is_active_verse {
            node_mgr.deselect();
            node_mgr.pending_sidebar_select = None;
            nav.navigate_to_verse(verse_id.clone(), verse_name.clone());
        }
        if eye_clicked {
            push_scope_eye(ui_mgr, vis_state, ScopeLevel::Verse, &verse_id);
        }
    }

    if hierarchy.verses.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("No verses. Click + above to create one.")
                .italics()
                .color(theme::TEXT_MUTED),
        );
    }
}

#[allow(clippy::needless_range_loop, clippy::too_many_arguments)]
fn render_fractals(
    ui: &mut egui::Ui,
    fractals: &mut [FractalEntry],
    nav: &mut NavigationManager,
    verse_id: &str,
    verse_name: &str,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
    settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
) {
    let fractal_count = fractals.len();
    for fi in 0..fractal_count {
        let fractal_id = fractals[fi].id.clone();
        let fractal_name = fractals[fi].name.clone();
        let is_active = nav.active_fractal_id.as_deref() == Some(&fractal_id);

        let own_visible = visibility::fractal_effective_visible(&fractal_id, verse_id, vis_state);
        let rollup_state = visibility::rollup(
            fractals[fi].petals.iter().map(|p| {
                visibility::petal_effective_visible(&p.id, &fractal_id, verse_id, vis_state)
            }),
            own_visible,
        );
        let glyph = visibility::glyph_for_rollup(rollup_state);
        let eye_tooltip = if own_visible {
            "Hide fractal"
        } else {
            "Show fractal"
        };

        let id = ui.make_persistent_id(egui::Id::new(format!(
            "fractal_{}_{}",
            verse_id, fractal_id
        )));
        let mut name_clicked = false;
        let mut eye_clicked = false;
        let header =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    let (response, eye) = tree_entry::row(
                        ui,
                        Target {
                            kind: EntryKind::Fractal,
                            id: &fractal_id,
                            name: &fractal_name,
                            verse_id: verse_id,
                            fractal_id: None,
                            petal_id: None,
                            url: None,
                        },
                        RowSelection::from_primary(
                            is_active && nav.active_petal_id.is_none() && node_mgr.primary().is_none(),
                        ),
                        is_active,
                        own_visible,
                        nav,
                        local_role,
                        settings,
                        ui_mgr,
                        |ui| {
                            eye_button(
                                ui,
                                glyph,
                                true,
                                eye_tooltip,
                                format_args!("{eye_tooltip}: fractal {fractal_name}"),
                            )
                        },
                        |_| {},
                    );
                    name_clicked = response.clicked();
                    eye_clicked = eye;
                });
        let (toggle_response, _, _) = header.body(|ui| {
            render_petals(
                ui,
                &mut fractals[fi].petals,
                nav,
                &fractal_id,
                &fractal_name,
                verse_id,
                verse_name,
                camera_focus,
                node_mgr,
                ui_mgr,
                vis_state,
                settings,
                local_role,
            );
            // [+] Add Petal inside the fractal collapse
            add_button_inline(
                ui,
                "+ Petal",
                CreateKind::Petal,
                &fractal_id,
                ui_mgr,
                local_role.can_manage()
                    && nav.active_verse_id.as_deref() == Some(verse_id)
                    && nav.active_fractal_id.as_deref() == Some(&fractal_id)
                    && nav.active_petal_id.is_none(),
            );
        });
        toggle_response.named(
            ui,
            format_args!("Expand or collapse fractal {fractal_name}"),
        );

        if name_clicked && !is_active {
            node_mgr.deselect();
            node_mgr.pending_sidebar_select = None;
            nav.navigate_to_fractal_in_verse(verse_id, verse_name, &fractal_id, &fractal_name);
        }
        if eye_clicked {
            push_scope_eye(ui_mgr, vis_state, ScopeLevel::Fractal, &fractal_id);
        }
    }
}

#[allow(clippy::needless_range_loop, clippy::too_many_arguments)]
fn render_petals(
    ui: &mut egui::Ui,
    petals: &mut [PetalEntry],
    nav: &mut NavigationManager,
    fractal_id: &str,
    fractal_name: &str,
    verse_id: &str,
    verse_name: &str,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
    settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
) {
    let petal_count = petals.len();
    for pi in 0..petal_count {
        let petal_id = petals[pi].id.clone();
        let petal_name = petals[pi].name.clone();
        let is_active = nav.active_petal_id.as_deref() == Some(&petal_id);

        let own_visible =
            visibility::petal_effective_visible(&petal_id, fractal_id, verse_id, vis_state);
        let glyph = visibility::glyph_for_bool(own_visible);
        let (eye_enabled, eye_tooltip) = petal_eye(
            is_active,
            own_visible,
            vis_state.is_scope_hidden(ScopeLevel::Petal, &petal_id),
        );

        let id = ui.make_persistent_id(egui::Id::new(format!("petal_{}_{}", fractal_id, petal_id)));
        let mut name_clicked = false;
        let mut eye_clicked = false;
        let header =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    let (response, eye) = tree_entry::row(
                        ui,
                        Target {
                            kind: EntryKind::Petal,
                            id: &petal_id,
                            name: &petal_name,
                            verse_id: verse_id,
                            fractal_id: Some(fractal_id),
                            petal_id: Some(&petal_id),
                            url: None,
                        },
                        RowSelection::from_primary(is_active && node_mgr.primary().is_none()),
                        is_active,
                        own_visible,
                        nav,
                        local_role,
                        settings,
                        ui_mgr,
                        |ui| {
                            eye_button(
                                ui,
                                glyph,
                                eye_enabled,
                                eye_tooltip,
                                format_args!("{eye_tooltip}: petal {petal_name}"),
                            )
                        },
                        |_| {},
                    );
                    name_clicked = response.clicked();
                    eye_clicked = eye;
                });
        let ancestry = NodeAncestry {
            verse_id,
            fractal_id,
            petal_id: &petal_id,
        };
        let (toggle_response, _, _) = header.body(|ui| {
            render_nodes(
                ui,
                &petals[pi].nodes,
                nav,
                NodeTreeScope {
                    verse_name,
                    fractal_name,
                    petal_name: &petal_name,
                    is_active_petal: is_active,
                    ancestry,
                },
                camera_focus,
                node_mgr,
                ui_mgr,
                vis_state,
                settings,
                local_role,
            );
            ui.horizontal_wrapped(|ui| {
                // [+] Add Node inside the petal collapse
                add_button_inline(
                    ui,
                    "+ Node",
                    CreateKind::Node,
                    &petal_id,
                    ui_mgr,
                    local_role.can_edit()
                        && nav.active_verse_id.as_deref() == Some(verse_id)
                        && nav.active_petal_id.as_deref() == Some(&petal_id),
                );
                if ui
                    .add_enabled(
                        local_role.can_manage()
                            && nav.active_verse_id.as_deref() == Some(verse_id)
                            && nav.active_petal_id.as_deref() == Some(&petal_id),
                        egui::Button::new(egui::RichText::new("Petal requirements").small())
                            .fill(theme::BG_BUTTON)
                            .min_size(egui::vec2(0.0, 24.0)),
                    )
                    .named(ui, format_args!("Petal requirements for {petal_name}"))
                    .on_hover_text("Edit this petal's map and hexon requirements")
                    .on_disabled_hover_text(
                        "Manager role in this verse is required to edit petal requirements.",
                    )
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::PetalManifestOpen {
                        petal_id: petal_id.clone(),
                        petal_name: petal_name.clone(),
                    });
                }
            });
        });
        toggle_response.named(ui, format_args!("Expand or collapse petal {petal_name}"));

        if name_clicked && !is_active {
            node_mgr.deselect();
            node_mgr.pending_sidebar_select = None;
            nav.navigate_to_petal_in_fractal(
                verse_id,
                verse_name,
                fractal_id,
                fractal_name,
                &petal_id,
            );
        }
        if eye_clicked {
            push_scope_eye(ui_mgr, vis_state, ScopeLevel::Petal, &petal_id);
        }
    }
}

/// A petal eye's `(enabled, tooltip)`. RATIFICATION #4: the ACTIVE petal can't
/// be hidden — but a petal persisted hidden and then entered can still be SHOWN
/// (otherwise its contents stay hidden with a dead eye). Pure.
fn petal_eye(is_active: bool, own_visible: bool, own_flag_hidden: bool) -> (bool, &'static str) {
    match (is_active, own_flag_hidden) {
        (true, true) => (true, "Show petal"),
        (true, false) => (false, "Can't hide the petal you're in"),
        (false, _) if own_visible => (true, "Hide petal"),
        (false, _) => (true, "Show petal"),
    }
}

/// Flip one verse/fractal/petal eye through the persisted producer
/// (optimistic + authed `SetScopeHidden`, D-1).
fn push_scope_eye(
    ui_mgr: &mut UiManager,
    vis_state: &VisibilityState,
    level: ScopeLevel,
    id: &str,
) {
    ui_mgr.push_action(UiAction::SetScopeHidden {
        level,
        id: id.to_string(),
        hidden: !vis_state.is_scope_hidden(level, id),
    });
}

/// A node row's right-click verb (D-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeRowVerb {
    Isolate,
    HideOthers,
    ShowAll,
    /// Selects the row and reveals the right-sidebar Groups editor (D-10).
    AddToGroup,
}

/// Apply a node-row menu verb: Isolate is session-only solo; Hide others and
/// Show all persist through `UiAction::SetNodeView`.
fn apply_node_row_verb(
    verb: NodeRowVerb,
    node_id: &str,
    petal_node_ids: &[&str],
    ancestry: NodeAncestry<'_>,
    vis_state: &mut VisibilityState,
    ui_mgr: &mut UiManager,
) {
    match verb {
        NodeRowVerb::Isolate => {
            vis_state.set_solo(std::collections::HashSet::from([node_id.to_string()]))
        }
        NodeRowVerb::HideOthers => visibility::hide_others_and_persist(
            vis_state,
            petal_node_ids.iter().copied(),
            node_id,
            ui_mgr,
        ),
        NodeRowVerb::ShowAll => visibility::show_all_and_persist(
            vis_state,
            ancestry,
            petal_node_ids.iter().copied(),
            ui_mgr,
        ),
        NodeRowVerb::AddToGroup => ui_mgr.push_action(UiAction::RevealSection {
            slug: "groups".to_string(),
        }),
    }
}

#[derive(Clone, Copy)]
struct NodeTreeScope<'a> {
    verse_name: &'a str,
    fractal_name: &'a str,
    petal_name: &'a str,
    is_active_petal: bool,
    ancestry: NodeAncestry<'a>,
}

#[derive(Clone, Copy)]
enum NodeRowFilter<'a> {
    All,
    Group(&'a str),
    Ungrouped,
}

impl NodeRowFilter<'_> {
    fn row(self, petal_id: &str, node_id: &str) -> crate::node_manager::SelectionRow {
        use crate::node_manager::{SelectionRow, SelectionSection};
        SelectionRow {
            petal_id: petal_id.into(),
            node_id: node_id.into(),
            section: match self {
                Self::All => SelectionSection::All,
                Self::Group(id) => SelectionSection::Group(id.into()),
                Self::Ungrouped => SelectionSection::Ungrouped,
            },
        }
    }

    fn matches(self, id: &str, visibility: &VisibilityState) -> bool {
        match self {
            Self::All => true,
            Self::Group(group) => visibility
                .node_groups(id)
                .iter()
                .any(|member| member == group),
            Self::Ungrouped => !visibility
                .node_groups(id)
                .iter()
                .any(|id| visibility.groups().iter().any(|group| group.id == *id)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &[NodeEntry],
    nav: &mut NavigationManager,
    scope: NodeTreeScope<'_>,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
    settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
) {
    if !scope.is_active_petal || vis_state.groups().is_empty() {
        let order = nodes
            .iter()
            .filter(|node| visibility::pickable(&node.id, nav, vis_state))
            .map(|node| NodeRowFilter::All.row(scope.ancestry.petal_id, &node.id))
            .collect::<Vec<_>>();
        render_node_rows(
            ui,
            nodes,
            nav,
            scope,
            camera_focus,
            node_mgr,
            ui_mgr,
            vis_state,
            settings,
            local_role,
            NodeRowFilter::All,
            &order,
        );
        return;
    }
    let groups = vis_state.groups().to_vec();
    let sections = groups
        .iter()
        .map(|group| {
            (
                group.id.as_str(),
                group.name.as_str(),
                NodeRowFilter::Group(&group.id),
            )
        })
        .chain(std::iter::once((
            "ungrouped",
            "Ungrouped",
            NodeRowFilter::Ungrouped,
        )))
        .collect::<Vec<_>>();
    let mut visible_order = Vec::new();
    for (id, _, filter) in &sections {
        let collapse = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id((
                "petal-node-group",
                scope.ancestry.petal_id,
                matches!(filter, NodeRowFilter::Ungrouped),
                id,
            )),
            true,
        );
        if collapse.is_open() {
            for node in nodes.iter().filter(|node| {
                filter.matches(&node.id, vis_state)
                    && visibility::pickable(&node.id, nav, vis_state)
            }) {
                visible_order.push(filter.row(scope.ancestry.petal_id, &node.id));
            }
        }
    }
    for (id, name, filter) in sections {
        let members = nodes
            .iter()
            .filter(|node| filter.matches(&node.id, vis_state))
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        let selected = members
            .iter()
            .filter(|id| node_mgr.contains_node(id))
            .count();
        let selection = if selected == 0 {
            RowSelection::None
        } else if selected < members.len() {
            RowSelection::Partial
        } else if node_mgr
            .primary()
            .is_some_and(|primary| members.contains(&primary.node_id))
        {
            RowSelection::Primary
        } else {
            RowSelection::Selected
        };
        let collapse = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id((
                "petal-node-group",
                scope.ancestry.petal_id,
                matches!(filter, NodeRowFilter::Ungrouped),
                id,
            )),
            true,
        );
        let header = collapse.show_header(ui, |ui| {
            let frame = egui::Frame::NONE.fill(selection.fill(false)).inner_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                let target = 24.0_f32.max(20.0 / ui.ctx().pixels_per_point());
                let response = ui.add_enabled(!members.is_empty(), egui::Button::new(format!("{name}  {selected}/{}", members.len())).truncate()
                    .frame(false).selected(selected > 0 && selected == members.len()).min_size(egui::vec2(ui.available_width(), target)))
                    .named(ui, format_args!("Select group {name}"))
                    .on_hover_text("Select the available members. Ctrl/Cmd toggles the group; the arrow expands it. Hidden members are reported and skipped.");
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_selected(selected > 0 && selected == members.len());
                    node.set_description(if selected > 0 && selected < members.len() { "Partially selected group" } else { "Petal node group" });
                });
                if response.clicked() && crate::native_accessibility::selection_allowed(ui.ctx(), &response) {
                    let command = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                    node_mgr.request_selection(scope.ancestry.petal_id, members.clone(), if command { crate::node_manager::SelectionMode::Toggle } else { crate::node_manager::SelectionMode::Replace }, visible_order.clone());
                }
            });
            selection.paint(ui.painter(), frame.response.rect);
        });
        let (toggle, _, _) = header.body(|ui| {
            render_node_rows(
                ui,
                nodes,
                nav,
                scope,
                camera_focus,
                node_mgr,
                ui_mgr,
                vis_state,
                settings,
                local_role,
                filter,
                &visible_order,
            );
        });
        toggle.named(ui, format_args!("Expand or collapse node group {name}"));
    }
}

#[allow(clippy::too_many_arguments)] // one tree level: rows, selection, menu, eyes
fn render_node_rows(
    ui: &mut egui::Ui,
    nodes: &[NodeEntry],
    nav: &mut NavigationManager,
    scope: NodeTreeScope<'_>,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
    settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
    filter: NodeRowFilter<'_>,
    visible_order: &[crate::node_manager::SelectionRow],
) {
    let NodeTreeScope {
        verse_name,
        fractal_name,
        petal_name,
        is_active_petal,
        ancestry,
    } = scope;
    let mut node_click: Option<(String, [f32; 3])> = None;
    let mut node_alt_click: Option<(String, String, String)> = None;
    let mut override_write: Option<(String, OverrideState)> = None;
    let mut row_verb: Option<(String, NodeRowVerb)> = None;

    for node in nodes
        .iter()
        .filter(|node| filter.matches(&node.id, vis_state))
    {
        let node_id = node.id.clone();
        let node_name = node.name.clone();
        let position = node.position;
        let webpage_url = node.webpage_url.clone().unwrap_or_default();
        let selection = if node_mgr
            .primary()
            .is_some_and(|primary| primary.node_id == node_id)
        {
            RowSelection::Primary
        } else if node_mgr.contains_node(&node_id) {
            RowSelection::Selected
        } else {
            RowSelection::None
        };

        let reason = visibility::node_hidden_reason(&node_id, ancestry, vis_state);
        let effectively_visible = reason == visibility::HiddenReason::NotHidden;
        let glyph = visibility::glyph_for_bool(effectively_visible);
        let eye_tooltip = visibility::node_eye_tooltip(&reason);

        let (label_resp, eye_clicked) = tree_entry::row(
            ui,
            Target {
                kind: EntryKind::Node,
                id: &node_id,
                name: &node_name,
                verse_id: ancestry.verse_id,
                fractal_id: Some(ancestry.fractal_id),
                petal_id: Some(ancestry.petal_id),
                url: Some(&webpage_url),
            },
            selection,
            false,
            is_active_petal && effectively_visible,
            nav,
            local_role,
            settings,
            ui_mgr,
            |ui| {
                eye_button(
                    ui,
                    glyph,
                    true,
                    &eye_tooltip,
                    format_args!("{eye_tooltip}: node {node_name}"),
                )
            },
            |ui| {
                if is_active_petal {
                    ui.separator();
                    for (label, verb) in [
                        ("Isolate", NodeRowVerb::Isolate),
                        ("Hide others", NodeRowVerb::HideOthers),
                        ("Show all", NodeRowVerb::ShowAll),
                        ("Add selection to group...", NodeRowVerb::AddToGroup),
                    ] {
                        if ui
                            .button(label)
                            .named(ui, format_args!("{label}: node {node_name}"))
                            .clicked()
                        {
                            row_verb = Some((node_id.clone(), verb));
                            ui.close();
                        }
                    }
                }
            },
        );

        let is_alt = ui.input(|inp| inp.modifiers.alt);

        let label_resp = if is_active_petal {
            label_resp
        } else {
            label_resp.on_hover_text(format!("Enter {petal_name} to select this object. Hidden objects must be shown before selection."))
        };
        if label_resp.clicked() && crate::native_accessibility::selection_allowed(ui.ctx(), &label_resp) {
            if is_alt && is_active_petal && local_role.can_edit() {
                node_alt_click = Some((node_id.clone(), node_name.clone(), webpage_url));
            } else {
                node_click = Some((node_id.clone(), position));
            }
        }

        if eye_clicked {
            let current = vis_state.node_override(&node_id);
            let next = visibility::toggle_node_override(&reason, current);
            override_write = Some((node_id.clone(), next));
        }
    }

    // Apply selection — route through NodeManager's pending mechanism.
    if let Some((nid, pos)) = node_click {
        // camera_focus_clip_20260716 FR-2: carry node_id so apply_camera_focus
        // can prefer the live spawned transform over this cached fallback.
        if !is_active_petal {
            nav.navigate_to_petal_in_fractal(
                ancestry.verse_id,
                verse_name,
                ancestry.fractal_id,
                fractal_name,
                ancestry.petal_id,
            );
            node_mgr.request_select_after_navigation(&nid, ancestry.petal_id);
            camera_focus.target = Some((nid, pos));
        } else {
            let modifiers = ui.input(|input| input.modifiers);
            node_mgr.request_row_selection(
                filter.row(ancestry.petal_id, &nid),
                crate::node_manager::SelectionMode::from_modifiers(
                    modifiers.command || modifiers.ctrl,
                    modifiers.shift,
                ),
                visible_order.to_vec(),
            );
            camera_focus.target = Some((nid, pos));
        }
    }

    // Apply alt-click options dialog
    if let Some((nid, nname, url)) = node_alt_click {
        ui_mgr.open_dialog(ActiveDialog::NodeOptions {
            node_id: nid,
            node_name_buf: nname,
            webpage_url_buf: url,
            pending_delete: false,
            descendant_count: None,
        });
    }

    // Apply the eye click's tri-state write (RATIFICATION #6) through the
    // persisted producer: optimistic session write + authed SetNodeView.
    if let Some((nid, next)) = override_write {
        if next != vis_state.node_override(&nid) {
            ui_mgr.push_action(UiAction::SetNodeView {
                node_id: nid,
                state: next,
            });
        }
    }

    if let Some((nid, verb)) = row_verb {
        if verb == NodeRowVerb::AddToGroup {
            if !node_mgr.contains_node(&nid) {
                node_mgr.request_selection(
                    ancestry.petal_id,
                    vec![nid.clone()],
                    crate::node_manager::SelectionMode::Replace,
                    Vec::new(),
                );
            }
        }
        let petal_node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        apply_node_row_verb(verb, &nid, &petal_node_ids, ancestry, vis_state, ui_mgr);
    }
}

fn add_button_inline(
    ui: &mut egui::Ui,
    tooltip: &str,
    kind: CreateKind,
    parent_id: &str,
    ui_mgr: &mut UiManager,
    allowed: bool,
) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                allowed,
                egui::Button::new(tooltip)
                    .fill(theme::BG_BUTTON_ALT)
                    .min_size(egui::vec2(0.0, 24.0)),
            )
            .named(ui, tooltip)
            .on_hover_text(tooltip)
            .on_disabled_hover_text(
                "Open this verse with the required edit role before creating content.",
            )
            .clicked()
        {
            ui_mgr.open_dialog(ActiveDialog::CreateEntity {
                kind,
                parent_id: parent_id.to_string(),
                name_buf: String::new(),
            });
        }
    });
}

/// Collapsed "Groups" section (D-10): the active petal's Groups with their eye
/// + member count — read + toggle only; editing lives in the right sidebar.
fn sidebar_section_groups(
    ui: &mut egui::Ui,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    vis_state: &VisibilityState,
) {
    let Some(petal_id) = nav.active_petal_id.as_deref() else {
        return;
    };
    egui::CollapsingHeader::new(
        egui::RichText::new("Groups")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(false)
    .show(ui, |ui| {
        let groups = vis_state.groups();
        if groups.is_empty() {
            ui.label(
                egui::RichText::new("No Groups in this petal.")
                    .small()
                    .italics()
                    .color(theme::TEXT_MUTED),
            );
        }
        for group in groups {
            ui.horizontal(|ui| {
                let members = vis_state.group_member_count(&group.id);
                let name_color = if group.visible {
                    theme::TEXT_STRONG
                } else {
                    theme::TEXT_MUTED
                };
                ui.label(egui::RichText::new(&group.name).color(name_color));
                ui.label(
                    egui::RichText::new(format!("({members})"))
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let tooltip = if group.visible {
                        "Hide this Group"
                    } else {
                        "Show this Group"
                    };
                    if eye_button(
                        ui,
                        visibility::glyph_for_bool(group.visible),
                        vis_state.registry_ready(petal_id) && !ui_mgr.editor_batch.is_pending(),
                        tooltip,
                        format_args!("{tooltip}: {}", group.name),
                    ) {
                        ui_mgr.push_action(UiAction::SetGroupRegistry {
                            petal_id: petal_id.to_string(),
                            groups: visibility::groups::with_group_visible(
                                groups,
                                &group.id,
                                !group.visible,
                            ),
                        });
                    }
                });
            });
        }
        if ui
            .add(egui::Button::new("Manage Groups…").small())
            .observed(ui)
            .clicked()
        {
            ui_mgr.push_action(UiAction::RevealSection {
                slug: "groups".to_string(),
            });
        }
    })
    .header_response
    .observed(ui);
}

#[cfg(test)]
mod tests {
    use super::petal_eye;

    #[test]
    fn the_active_petal_can_be_shown_but_never_hidden() {
        // (is_active, own_visible, own_flag_hidden) -> (enabled, tooltip)
        let cases = [
            (
                (true, true, false),
                (false, "Can't hide the petal you're in"),
            ),
            // Persisted hidden, then entered: the eye must stay usable to show it.
            ((true, false, true), (true, "Show petal")),
            // Hidden only through a hidden fractal/verse: fix it up there.
            (
                (true, false, false),
                (false, "Can't hide the petal you're in"),
            ),
            ((false, true, false), (true, "Hide petal")),
            ((false, false, true), (true, "Show petal")),
        ];
        for ((active, visible, flag), expected) in cases {
            assert_eq!(
                petal_eye(active, visible, flag),
                expected,
                "{active} {visible} {flag}"
            );
        }

        use crate::panels::tree_appearance::{format_hex, parse_hex, AppearanceDraft, EntryKind};
        use crate::settings::{AppSettings, EntryAppearance, TreeIcon};
        for rgb in [[0, 0, 0], [255, 255, 255], [0, 255, 128], [255, 0, 64]] {
            assert_eq!(parse_hex(&format_hex(rgb)), Some(rgb));
        }
        for invalid in [
            "", "#FFF", "FFFFFF", "#GG0000", "#1234567", "#é1234", " #123456",
        ] {
            assert_eq!(parse_hex(invalid), None, "{invalid}");
        }
        let mut settings = AppSettings::default();
        settings.tree_appearance.insert(
            EntryKind::Node.key("n1"),
            EntryAppearance {
                icon: TreeIcon::Star,
                color: [20, 30, 40],
            },
        );
        let draft = AppearanceDraft::new(EntryKind::Node, "n1", "Node", &settings);
        let mut manager = crate::actions::UiManager::default();
        manager.open_dialog(crate::dialogs::ActiveDialog::TreeAppearance { draft });
        assert!(manager.any_dialog_open());
        manager.close_dialog();
        assert_eq!(
            settings.tree_appearance[&EntryKind::Node.key("n1")].color,
            [20, 30, 40],
            "cancel does not mutate persisted preferences"
        );
        assert_ne!(
            EntryKind::Node.key("same-id"),
            EntryKind::Verse.key("same-id")
        );

        use bevy_egui::egui;
        for width in [220.0, 320.0, 480.0] {
            for scale in [0.75, 1.0, 2.0] {
                for selection in [
                    super::RowSelection::None,
                    super::RowSelection::Primary,
                    super::RowSelection::Selected,
                    super::RowSelection::Partial,
                ] {
                    let ctx = egui::Context::default();
                    ctx.set_pixels_per_point(scale);
                    ctx.enable_accesskit();
                    let input = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 300.0),
                        )),
                        ..Default::default()
                    };
                    let mut response = None;
                    let mut toggle_response = None;
                    crate::ui_shell::layout_policy::begin_frame(&ctx, true);
                    let output = ctx.run(input, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            super::configure_tree_spacing(ui);
                            let header =
                                egui::collapsing_header::CollapsingState::load_with_default_open(
                                    ui.ctx(),
                                    ui.make_persistent_id("test_tree_expansion"),
                                    true,
                                )
                                .show_header(ui, |ui| {
                                    let (name, _) = crate::panels::tree_entry::row(
                                        ui,
                                        crate::panels::tree_entry::Target {
                                            kind: EntryKind::Node,
                                            id: "n1",
                                            name: "Long object name with meaningful suffix",
                                            verse_id: "v1",
                                            fractal_id: Some("f1"),
                                            petal_id: Some("p1"),
                                            url: None,
                                        },
                                        selection,
                                        false,
                                        true,
                                        &crate::navigation_manager::NavigationManager {
                                            active_verse_id: Some("v1".into()),
                                            active_fractal_id: Some("f1".into()),
                                            active_petal_id: Some("p1".into()),
                                            ..Default::default()
                                        },
                                        &crate::plugin::LocalUserRole {
                                            role: Some(fe_database::RoleLevel::Owner),
                                        },
                                        &settings,
                                        &mut manager,
                                        |ui| {
                                            super::eye_button(
                                                ui,
                                                crate::visibility::glyph_for_bool(true),
                                                true,
                                                "Hide node",
                                                "Hide test node",
                                            )
                                        },
                                        |_| {},
                                    );
                                    response = Some(name);
                                });
                            let (toggle, _, _) = header.body(|ui| ui.label("Expanded content"));
                            toggle_response = Some(crate::accessibility::ResponseExt::named(
                                toggle,
                                ui,
                                "Expand or collapse test entry",
                            ));
                        });
                    });
                    let toggle = toggle_response.unwrap();
                    assert!(toggle.rect.width() >= 24.0 && toggle.rect.height() >= 24.0);
                    assert!(
                        toggle.interact_rect.width() >= 24.0 && toggle.interact_rect.height() >= 24.0
                    );
                    let response = response.unwrap();
                    assert!(response.rect.width() >= 24.0 && response.rect.height() >= 24.0);
                    let accessible = output
                        .platform_output
                        .accesskit_update
                        .as_ref()
                        .unwrap()
                        .nodes
                        .iter()
                        .find_map(|(_, node)| {
                            (node.label()
                                == Some("Select node Long object name with meaningful suffix"))
                            .then_some(node)
                        })
                        .expect("the row retains its full accessible name");
                    assert_eq!(
                        accessible.is_selected(),
                        Some(matches!(
                            selection,
                            super::RowSelection::Primary | super::RowSelection::Selected
                        ))
                    );
                    assert_eq!(
                        accessible.description(),
                        Some(match selection {
                            super::RowSelection::None => "Not selected",
                            super::RowSelection::Primary => "Primary selection",
                            super::RowSelection::Selected => "Selected",
                            super::RowSelection::Partial => "Partially selected group",
                        })
                    );
                    let glyph_segments = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::epaint::Shape::LineSegment { points, stroke }
                                if stroke.color == egui::Color32::from_rgb(20, 30, 40) =>
                            {
                                Some((points, stroke))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(
                        glyph_segments.len(),
                        10,
                        "the chosen Star remains the row icon"
                    );
                    let mut glyph_bounds = egui::Rect::NOTHING;
                    for (points, stroke) in glyph_segments {
                        assert_eq!(stroke.width, 1.25, "no chunky backing outline");
                        glyph_bounds.extend_with(points[0]);
                        glyph_bounds.extend_with(points[1]);
                    }
                    assert!(
                        glyph_bounds.width() <= 14.0 && glyph_bounds.height() <= 14.0,
                        "glyphs stay text-sized independently of the hit target: {glyph_bounds:?}"
                    );
                    let expected_fill = match selection {
                        super::RowSelection::Primary => crate::theme::TREE_PRIMARY_BG,
                        super::RowSelection::Selected => crate::theme::TREE_SELECTED_BG,
                        super::RowSelection::Partial => crate::theme::TREE_PARTIAL_BG,
                        super::RowSelection::None => egui::Color32::TRANSPARENT,
                    };
                    if selection != super::RowSelection::None {
                        assert!(
                            output.shapes.iter().any(|shape| matches!(&shape.shape,
                            egui::epaint::Shape::Rect(rect) if rect.fill == expected_fill
                                && rect.rect.contains_rect(response.rect))),
                            "selection fill covers the complete row, independently of icon color"
                        );
                    }
                    let observed = crate::ui_shell::layout_policy::observations(&ctx);
                    assert!(observed.controls.iter().any(
                        |control| control.id == response.id.value() && control.hit.width() >= 24.0
                    ));
                    for control in &observed.controls {
                        if control.clickable || control.draggable {
                            assert!(
                                control.hit.width() >= 24.0 && control.hit.height() >= 24.0,
                                "tree action: {:?}",
                                control.hit
                            );
                            assert!(
                                control.hit.width() * scale >= 20.0
                                    && control.hit.height() * scale >= 20.0
                            );
                        }
                    }
                }
            }
        }
        fn texts(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        texts(shape, out);
                    }
                }
                _ => {}
            }
        }
        let hierarchy = crate::verse_manager::VerseManager::from_verses(vec![
            crate::verse_manager::VerseEntry {
                id: "v".into(),
                name: "Counted verse".into(),
                fractals: vec![crate::verse_manager::FractalEntry {
                    id: "f".into(),
                    name: "Counted fractal".into(),
                    petals: vec![
                        crate::verse_manager::PetalEntry {
                            id: "p".into(),
                            name: "Counted petal".into(),
                            nodes: vec![
                                crate::verse_manager::NodeEntry {
                                    id: "n1".into(),
                                    ..Default::default()
                                },
                                crate::verse_manager::NodeEntry {
                                    id: "n2".into(),
                                    ..Default::default()
                                },
                            ],
                            ..Default::default()
                        },
                        crate::verse_manager::PetalEntry {
                            id: "empty".into(),
                            name: "Empty petal".into(),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            },
        ]);
        for width in [1024.0, 1280.0, 1920.0] {
            for (verse, fractal, petal, expected) in [
                (None, None, None, "Library: 2 petals \u{00b7} 2 objects"),
                (Some("v"), None, None, "Verse: 2 petals \u{00b7} 2 objects"),
                (
                    Some("v"),
                    Some("f"),
                    None,
                    "Fractal: 2 petals \u{00b7} 2 objects",
                ),
                (
                    Some("v"),
                    Some("f"),
                    Some("p"),
                    "Petal: 1 petal \u{00b7} 2 objects",
                ),
                (
                    Some("v"),
                    Some("f"),
                    Some("empty"),
                    "Petal: 1 petal \u{00b7} 0 objects",
                ),
                (
                    Some("v"),
                    Some("f"),
                    Some("missing"),
                    "Content counts unavailable",
                ),
            ] {
                let ctx = egui::Context::default();
                let nav = crate::navigation_manager::NavigationManager {
                    active_verse_id: verse.map(str::to_owned),
                    active_verse_name: "Counted verse".into(),
                    active_fractal_id: fractal.map(str::to_owned),
                    active_petal_id: petal.map(str::to_owned),
                    ..Default::default()
                };
                let mut manager = crate::actions::UiManager::default();
                let dashboard = crate::atlas::DashboardState {
                    petal_count: 999,
                    model_count: 999,
                    ..Default::default()
                };
                crate::ui_shell::layout_policy::begin_frame(&ctx, true);
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 720.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        crate::panels::status_bar::status_bar(
                            ctx,
                            &dashboard,
                            None,
                            &nav,
                            &mut manager,
                            &crate::ui_shell::modal::ModalManagerState::default(),
                            &hierarchy,
                            &mut crate::visibility::VisibilityState::default(),
                            &mut crate::plugin::IdentityBanner::default(),
                            &crate::plugin::LocalUserRole {
                                role: Some(fe_database::RoleLevel::Owner),
                            },
                            None,
                        );
                    },
                );
                let mut rendered = Vec::new();
                for shape in &output.shapes {
                    texts(&shape.shape, &mut rendered);
                }
                assert!(
                    rendered.iter().any(|text| text == expected),
                    "{width}: {rendered:?}"
                );
                let observed = crate::ui_shell::layout_policy::observations(&ctx);
                for control in &observed.controls {
                    if control.clickable || control.draggable {
                        assert!(
                            control.hit.width() >= 24.0 && control.hit.height() >= 24.0,
                            "actual footer action: {:?}",
                            control.hit
                        );
                    }
                }
                assert!(
                    observed
                        .controls
                        .iter()
                        .any(|control| control.rect.height() == 18.0
                            && !control.clickable
                            && !control.draggable),
                    "role summary is informational"
                );
            }
        }

        fn text_rect(shape: &egui::epaint::Shape, expected: &str) -> Option<egui::Rect> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == expected => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| text_rect(shape, expected))
                }
                _ => None,
            }
        }
        for width in [1024.0, 1280.0] {
            let ctx = egui::Context::default();
            let condition_a =
                "Mesh budget exceeded; inspect heavy objects before importing more. ".repeat(6);
            let condition_b =
                "Some content is waiting for its source peer; local edits remain available. "
                    .repeat(5);
            let guard_error =
                "Inspector disabled after a panel error; use its Retry action. ".repeat(6);
            let identity_message =
                "Identity storage is unavailable; this session uses a temporary identity. "
                    .repeat(6);
            let recent_error = "A fixture write failed and did not change stored data. ".repeat(6);
            let mut manager = crate::actions::UiManager::default();
            manager.set_condition("fixture_mesh", Some(condition_a.clone()));
            manager.set_condition("fixture_peer", Some(condition_b.clone()));
            manager.show_error(recent_error.clone(), 0.0);
            let modal = crate::ui_shell::modal::ModalManagerState {
                last_error: Some(guard_error.clone()),
                disabled_panels: std::collections::HashSet::from(["right.inspector".into()]),
            };
            let mut banner = crate::plugin::IdentityBanner {
                message: Some(identity_message.clone()),
            };
            let nav = crate::navigation_manager::NavigationManager {
                active_verse_id: Some("v".into()),
                active_verse_name: "Counted verse".into(),
                active_fractal_id: Some("f".into()),
                active_petal_id: Some("p".into()),
                ..Default::default()
            };
            let mut time = 0.0;
            let mut draw = |events: Vec<egui::Event>| {
                time += 0.05;
                crate::ui_shell::layout_policy::begin_frame(&ctx, true);
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 720.0),
                        )),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        crate::panels::status_bar::status_bar(
                            ctx,
                            &crate::atlas::DashboardState::default(),
                            None,
                            &nav,
                            &mut manager,
                            &modal,
                            &hierarchy,
                            &mut crate::visibility::VisibilityState::default(),
                            &mut banner,
                            &crate::plugin::LocalUserRole {
                                role: Some(fe_database::RoleLevel::Owner),
                            },
                            None,
                        );
                    },
                )
            };
            let _ = draw(Vec::new());
            let output = draw(Vec::new());
            let actions = crate::ui_shell::layout_policy::observations(&ctx)
                .controls
                .into_iter()
                .filter(|control| control.clickable || control.draggable)
                .collect::<Vec<_>>();
            assert_eq!(
                actions.len(),
                4,
                "peer/status/identity/errors are real reserved actions"
            );
            for action in &actions {
                assert!(
                    action.hit.width() >= 24.0 && action.hit.height() >= 24.0,
                    "{width}: {:?}",
                    action.hit
                );
                assert!(
                    action.clip.contains_rect(action.rect),
                    "{width}: clipped action {:?}",
                    action.rect
                );
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 720.0))
                        .contains_rect(action.rect)
                );
            }
            for (index, left) in actions.iter().enumerate() {
                for right in actions.iter().skip(index + 1) {
                    let overlap = left.rect.intersect(right.rect);
                    assert!(
                        overlap.width() <= 0.1 || overlap.height() <= 0.1,
                        "reserved footer actions overlap"
                    );
                }
            }
            for caption in ["Status 3", "Identity !", "Error", "0 peers"] {
                assert!(
                    output
                        .shapes
                        .iter()
                        .any(|shape| text_rect(&shape.shape, caption).is_some()),
                    "missing footer action {caption}"
                );
            }
            let status = output
                .shapes
                .iter()
                .find_map(|shape| text_rect(&shape.shape, "Status 3"))
                .unwrap()
                .center();
            draw(vec![
                egui::Event::PointerMoved(status),
                egui::Event::PointerButton {
                    pos: status,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            draw(vec![egui::Event::PointerButton {
                pos: status,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let output = draw(Vec::new());
            for full in [&condition_a, &condition_b, &guard_error] {
                let (rect, clip) = output
                    .shapes
                    .iter()
                    .find_map(|shape| {
                        text_rect(&shape.shape, full).map(|rect| (rect, shape.clip_rect))
                    })
                    .expect("complete condition in overflow");
                assert!(
                    clip.contains_rect(rect),
                    "full overflow text must paint inside its viewport"
                );
            }
            draw(vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let output = draw(Vec::new());
            let identity = output
                .shapes
                .iter()
                .find_map(|shape| text_rect(&shape.shape, "Identity !"))
                .unwrap()
                .center();
            draw(vec![
                egui::Event::PointerMoved(identity),
                egui::Event::PointerButton {
                    pos: identity,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            draw(vec![egui::Event::PointerButton {
                pos: identity,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let output = draw(Vec::new());
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| text_rect(&shape.shape, &identity_message).is_some()),
                "complete identity warning is reachable"
            );
            let dismiss = output
                .shapes
                .iter()
                .find_map(|shape| text_rect(&shape.shape, "Dismiss warning"))
                .unwrap()
                .center();
            draw(vec![
                egui::Event::PointerMoved(dismiss),
                egui::Event::PointerButton {
                    pos: dismiss,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            draw(vec![egui::Event::PointerButton {
                pos: dismiss,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let output = draw(Vec::new());
            assert!(!egui::Popup::is_id_open(
                &ctx,
                egui::Id::new("footer_identity_warning")
            ));
            let error = output
                .shapes
                .iter()
                .find_map(|shape| text_rect(&shape.shape, "Error"))
                .unwrap()
                .center();
            draw(vec![
                egui::Event::PointerMoved(error),
                egui::Event::PointerButton {
                    pos: error,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            draw(vec![egui::Event::PointerButton {
                pos: error,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let output = draw(Vec::new());
            let (rect, clip) = output
                .shapes
                .iter()
                .find_map(|shape| {
                    text_rect(&shape.shape, &recent_error).map(|rect| (rect, shape.clip_rect))
                })
                .expect("complete error history is reachable");
            assert!(clip.contains_rect(rect));
            let popup_actions = crate::ui_shell::layout_policy::observations(&ctx)
                .controls
                .into_iter()
                .filter(|control| control.clickable && control.rect.min.y < 680.0)
                .collect::<Vec<_>>();
            assert!(
                popup_actions.len() >= 2,
                "copy and dismiss remain reachable beside full error text"
            );
            for action in &popup_actions {
                assert!(action.hit.width() >= 24.0 && action.hit.height() >= 24.0);
                assert!(action.clip.contains_rect(action.rect));
            }
            let dismiss_error = output
                .shapes
                .iter()
                .find_map(|shape| text_rect(&shape.shape, "Dismiss"))
                .unwrap()
                .center();
            draw(vec![
                egui::Event::PointerMoved(dismiss_error),
                egui::Event::PointerButton {
                    pos: dismiss_error,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            draw(vec![egui::Event::PointerButton {
                pos: dismiss_error,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let output = draw(Vec::new());
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| text_rect(&shape.shape, "Errors").is_some()),
                "error history stays reachable after acknowledgement"
            );
            assert!(banner.message.is_none());
            assert!(manager.latest_unacknowledged_error().is_none());
            assert_eq!(
                manager.conditions().count(),
                2,
                "dismissals preserve unrelated conditions"
            );
            assert_eq!(modal.disabled_panels.len(), 1);
        }

        let ctx = egui::Context::default();
        let mut notices = crate::actions::UiManager::default();
        let mut time = 0.0;
        let nav = crate::navigation_manager::NavigationManager {
            active_verse_id: Some("v".into()),
            active_fractal_id: Some("f".into()),
            active_petal_id: Some("p".into()),
            ..Default::default()
        };
        let mut draw = |events: Vec<egui::Event>| {
            time += 0.05;
            let mut name = None;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1024.0, 768.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let (response, _) = crate::panels::tree_entry::row(
                            ui,
                            crate::panels::tree_entry::Target {
                                kind: EntryKind::Node,
                                id: "native-node-id",
                                name: "Copy target",
                                verse_id: "v",
                                fractal_id: Some("f"),
                                petal_id: Some("p"),
                                url: None,
                            },
                            super::RowSelection::Primary,
                            true,
                            true,
                            &nav,
                            &crate::plugin::LocalUserRole {
                                role: Some(fe_database::RoleLevel::Owner),
                            },
                            &settings,
                            &mut notices,
                            |ui| {
                                super::eye_button(
                                    ui,
                                    crate::visibility::glyph_for_bool(true),
                                    true,
                                    "Hide node",
                                    "Hide copy target",
                                )
                            },
                            |_| {},
                        );
                        name = Some(response.rect);
                    });
                },
            );
            (output, name.unwrap())
        };
        let (_, name) = draw(Vec::new());
        let at = name.center();
        draw(vec![egui::Event::PointerMoved(at)]);
        draw(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        draw(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let (menu, _) = draw(Vec::new());
        let copy = menu
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, "Copy ID"))
            .expect("native row menu exposes Copy ID")
            .center();
        draw(vec![
            egui::Event::PointerMoved(copy),
            egui::Event::PointerButton {
                pos: copy,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let (output, _) = draw(vec![egui::Event::PointerButton {
            pos: copy,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(value) if value == "native-node-id")));
        assert_eq!(
            notices.active_toast(time).map(|(text, _)| text),
            Some("Copied entry ID")
        );
        assert_eq!(nav.active_petal_id.as_deref(), Some("p"));
    }
}
