//! Hierarchy tree; see panels/AGENTS.md sidebar section.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::tree_appearance::EntryKind;
use super::tree_entry::{self, Target};
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
    egui::SidePanel::left("sidebar")
        .resizable(true)
        .default_width(220.0)
        .width_range(120.0..=crate::ui_shell::layout_policy::left_width(ctx))
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0)),
        )
        .show_animated(ctx, sidebar.open, |ui| {
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
                    ui.set_min_width(220.0);
                    ui.spacing_mut().indent = 12.0;
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
        if nav.active_verse_id.is_some() {
            ui.label(
                egui::RichText::new(&nav.active_verse_name)
                    .strong()
                    .color(theme::TEXT_STRONG),
            );
        } else {
            ui.label(
                egui::RichText::new("No Verse")
                    .italics()
                    .color(theme::TEXT_MUTED),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui
                .add(egui::Button::new("+").fill(theme::BG_BUTTON_ALT).small())
                .named(ui, "Create new verse")
                .on_hover_text("Create new Verse")
                .clicked()
            {
                ui_mgr.open_dialog(ActiveDialog::CreateEntity {
                    kind: CreateKind::Verse,
                    parent_id: String::new(),
                    name_buf: String::new(),
                });
            }
            // Phase F: Join Verse button
            if ui
                .add(egui::Button::new("Join").fill(theme::BG_BUTTON).small())
                .observed(ui)
                .on_hover_text("Join a verse by invite")
                .clicked()
            {
                ui_mgr.open_dialog(ActiveDialog::JoinDialog {
                    invite_buf: String::new(),
                });
            }
        });
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
    .min_size(egui::vec2(24.0, 24.0));
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
                            url: None,
                        },
                        is_active_verse
                            && nav.active_fractal_id.is_none()
                            && node_mgr.selected.is_none(),
                        is_active_verse,
                        own_visible,
                        nav.active_verse_id.as_deref(),
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
                local_role.can_manage() && is_active_verse,
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
                            url: None,
                        },
                        is_active && nav.active_petal_id.is_none() && node_mgr.selected.is_none(),
                        is_active,
                        own_visible,
                        nav.active_verse_id.as_deref(),
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
                local_role.can_manage() && nav.active_verse_id.as_deref() == Some(verse_id),
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
                            url: None,
                        },
                        is_active && node_mgr.selected.is_none(),
                        is_active,
                        own_visible,
                        nav.active_verse_id.as_deref(),
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
                verse_name,
                fractal_name,
                camera_focus,
                node_mgr,
                ui_mgr,
                is_active,
                ancestry,
                vis_state,
                settings,
                local_role,
            );
            ui.horizontal(|ui| {
                // [+] Add Node inside the petal collapse
                add_button_inline(
                    ui,
                    "+ Node",
                    CreateKind::Node,
                    &petal_id,
                    ui_mgr,
                    local_role.can_edit() && nav.active_verse_id.as_deref() == Some(verse_id),
                );
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("Petal requirements").small())
                            .fill(theme::BG_BUTTON)
                            .small(),
                    )
                    .observed(ui)
                    .on_hover_text("Edit this petal's map and hexon requirements")
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

#[allow(clippy::too_many_arguments)] // one tree level: rows, selection, menu, eyes
fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &[NodeEntry],
    nav: &mut NavigationManager,
    verse_name: &str,
    fractal_name: &str,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    is_active_petal: bool,
    ancestry: NodeAncestry<'_>,
    vis_state: &mut VisibilityState,
    settings: &crate::settings::AppSettings,
    local_role: &crate::plugin::LocalUserRole,
) {
    let mut node_click: Option<(String, [f32; 3])> = None;
    let mut node_alt_click: Option<(String, String, String)> = None;
    let mut override_write: Option<(String, OverrideState)> = None;
    let mut row_verb: Option<(String, NodeRowVerb)> = None;

    for node in nodes.iter() {
        let node_id = node.id.clone();
        let node_name = node.name.clone();
        let position = node.position;
        let webpage_url = node.webpage_url.clone().unwrap_or_default();
        let is_selected =
            node_mgr.selected.as_ref().map(|s| s.node_id.as_str()) == Some(node_id.as_str());

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
                url: Some(&webpage_url),
            },
            is_selected,
            false,
            is_active_petal && effectively_visible,
            nav.active_verse_id.as_deref(),
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
            label_resp.on_hover_text("Enter this petal and select the object. Hidden objects must be shown before selection.")
        };
        if label_resp.clicked() {
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
            node_mgr.request_select_and_frame(&nid, pos, camera_focus);
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
            node_mgr.pending_sidebar_select = Some(nid.clone());
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
                        true,
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
    }
}
