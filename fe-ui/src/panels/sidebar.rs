//! Left sidebar: verse/fractal/petal/node hierarchy tree + space overview.
//!
//! hierarchy_visibility_groups_20260808 Phase 1 adds a right-aligned eye
//! toggle to every tree row (verse/fractal/petal/node), reusing the gutter
//! idiom the verse header already established (`Layout::right_to_left`). All
//! visibility state lives in `crate::visibility::VisibilityState`; this file
//! only renders it and writes user clicks back — the resolver
//! (`crate::visibility::resolve`) owns the actual precedence logic. See
//! `panels/AGENTS.md` §sidebar and `conductor/tracks/
//! hierarchy_visibility_groups_20260808/spec.md` §4.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::atlas::DashboardState;
use crate::dialogs::{ActiveDialog, CreateKind};
use crate::navigation_manager::NavigationManager;
use crate::plugin::{CameraFocusTarget, SidebarState};
use crate::theme;
use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseManager};
use crate::visibility::{self, NodeAncestry, OverrideState, VisibilityState};
use fe_runtime::messages::DbCommand;

pub(crate) fn left_sidebar(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    nav: &mut NavigationManager,
    dashboard: &DashboardState,
    hierarchy: &mut VerseManager,
    camera_focus: &mut CameraFocusTarget,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
) {
    egui::SidePanel::left("sidebar")
        .resizable(true)
        .default_width(220.0)
        .width_range(180.0..=400.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0)),
        )
        .show_animated(ctx, sidebar.open, |ui| {
            sidebar_verse_header(ui, nav, db_tx, ui_mgr);
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    render_verse_tree(
                        ui,
                        hierarchy,
                        nav,
                        camera_focus,
                        node_mgr,
                        ui_mgr,
                        vis_state,
                    );
                    ui.add_space(8.0);
                    sidebar_section_space_overview(ui, dashboard);
                    ui.add_space(4.0);
                });

            // Bottom-pinned reset button (two-step destructive confirm)
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let pending_id = egui::Id::new("sidebar_reset_db_pending");
                let pending: bool = ui.ctx().data(|d| d.get_temp(pending_id)).unwrap_or(false);
                if !pending {
                    let reset_btn = egui::Button::new(
                        egui::RichText::new("Reset Database")
                            .small()
                            .color(theme::TEXT_DIM),
                    )
                    .fill(theme::BG_BUTTON_ALT);
                    if ui
                        .add(reset_btn)
                        .on_hover_text("Wipe all data and re-seed defaults")
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| d.insert_temp(pending_id, true));
                    }
                } else {
                    // bottom_up layout: buttons first so the warning sits above.
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Confirm Reset")
                                        .color(egui::Color32::WHITE),
                                )
                                .fill(theme::BG_DANGER)
                                .small(),
                            )
                            .clicked()
                        {
                            db_tx.send(DbCommand::ResetDatabase).ok();
                            ui.ctx().data_mut(|d| d.remove::<bool>(pending_id));
                        }
                        if ui
                            .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON).small())
                            .clicked()
                        {
                            ui.ctx().data_mut(|d| d.remove::<bool>(pending_id));
                        }
                    });
                    ui.label(
                        egui::RichText::new("Are you sure? This cannot be undone.")
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }
                ui.add_space(4.0);
            });
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
fn eye_button(ui: &mut egui::Ui, glyph: &str, enabled: bool, tooltip: &str) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(glyph)
            .small()
            .color(theme::TREE_NODE_ICON),
    )
    .frame(false)
    .small();
    ui.add_enabled(enabled, btn)
        .on_hover_text(tooltip)
        .clicked()
}

fn render_verse_tree(
    ui: &mut egui::Ui,
    hierarchy: &mut VerseManager,
    nav: &mut NavigationManager,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
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

        let header_text = egui::RichText::new(&verse_name)
            .strong()
            .color(if !own_visible {
                theme::TEXT_MUTED
            } else if is_active_verse {
                theme::TEXT_BRIGHT
            } else {
                theme::TEXT_SECTION
            });

        // egui::Id::new(...) wrapper matches the old `CollapsingHeader::id_salt`
        // double-hash exactly, so previously-persisted open/closed state isn't
        // reset by this refactor from `CollapsingHeader::show` to the lower-level
        // `CollapsingState::show_header`/`.body()` (needed for the eye gutter).
        let id = ui.make_persistent_id(egui::Id::new(format!("verse_{}", verse_id)));
        let mut name_clicked = false;
        let mut eye_clicked = false;
        let mut header =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label_resp =
                            ui.add(egui::Label::new(header_text).sense(egui::Sense::click()));
                        name_clicked = label_resp.clicked();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            eye_clicked = eye_button(ui, glyph, true, eye_tooltip);
                        });
                    });
                });
        if name_clicked {
            header.toggle();
        }
        let _ = header.body(|ui| {
            render_fractals(
                ui,
                &mut hierarchy.verses[vi].fractals,
                nav,
                &verse_id,
                camera_focus,
                node_mgr,
                ui_mgr,
                vis_state,
            );
            // [+] Add Fractal inside the verse collapse
            add_button_inline(ui, "Add Fractal", CreateKind::Fractal, &verse_id, ui_mgr);
        });

        if name_clicked {
            nav.navigate_to_verse(verse_id.clone(), verse_name.clone());
        }
        if eye_clicked {
            vis_state.toggle_verse_hidden(&verse_id);
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

#[allow(clippy::needless_range_loop)]
fn render_fractals(
    ui: &mut egui::Ui,
    fractals: &mut [FractalEntry],
    nav: &mut NavigationManager,
    verse_id: &str,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
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

        let header_text = egui::RichText::new(&fractal_name).color(if !own_visible {
            theme::TEXT_MUTED
        } else if is_active {
            theme::TEXT_BRIGHT
        } else {
            theme::TEXT_SECTION
        });

        let id = ui.make_persistent_id(egui::Id::new(format!(
            "fractal_{}_{}",
            verse_id, fractal_id
        )));
        let mut name_clicked = false;
        let mut eye_clicked = false;
        let mut header =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label_resp =
                            ui.add(egui::Label::new(header_text).sense(egui::Sense::click()));
                        name_clicked = label_resp.clicked();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            eye_clicked = eye_button(ui, glyph, true, eye_tooltip);
                        });
                    });
                });
        if name_clicked {
            header.toggle();
        }
        let _ = header.body(|ui| {
            render_petals(
                ui,
                &mut fractals[fi].petals,
                nav,
                &fractal_id,
                verse_id,
                camera_focus,
                node_mgr,
                ui_mgr,
                vis_state,
            );
            // [+] Add Petal inside the fractal collapse
            add_button_inline(ui, "Add Petal", CreateKind::Petal, &fractal_id, ui_mgr);
        });

        if name_clicked {
            nav.navigate_to_fractal(fractal_id.clone(), fractal_name.clone());
        }
        if eye_clicked {
            vis_state.toggle_fractal_hidden(&fractal_id);
        }
    }
}

#[allow(clippy::needless_range_loop)]
fn render_petals(
    ui: &mut egui::Ui,
    petals: &mut [PetalEntry],
    nav: &mut NavigationManager,
    fractal_id: &str,
    verse_id: &str,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    vis_state: &mut VisibilityState,
) {
    let petal_count = petals.len();
    for pi in 0..petal_count {
        let petal_id = petals[pi].id.clone();
        let petal_name = petals[pi].name.clone();
        let is_active = nav.active_petal_id.as_deref() == Some(&petal_id);

        let own_visible =
            visibility::petal_effective_visible(&petal_id, fractal_id, verse_id, vis_state);
        let glyph = visibility::glyph_for_bool(own_visible);
        // RATIFICATION #4: the ACTIVE petal's eye is disabled — you can't hide
        // the petal you're standing in.
        let eye_enabled = !is_active;
        let eye_tooltip = if is_active {
            "Can't hide the petal you're in"
        } else if own_visible {
            "Hide petal"
        } else {
            "Show petal"
        };

        let header_text = egui::RichText::new(&petal_name).color(if !own_visible {
            theme::TEXT_MUTED
        } else if is_active {
            theme::TEXT_BRIGHT
        } else {
            theme::TEXT_SECTION
        });

        let id = ui.make_persistent_id(egui::Id::new(format!("petal_{}_{}", fractal_id, petal_id)));
        let mut name_clicked = false;
        let mut eye_clicked = false;
        let mut header =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label_resp =
                            ui.add(egui::Label::new(header_text).sense(egui::Sense::click()));
                        name_clicked = label_resp.clicked();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            eye_clicked = eye_button(ui, glyph, eye_enabled, eye_tooltip);
                        });
                    });
                });
        if name_clicked {
            header.toggle();
        }
        let ancestry = NodeAncestry {
            verse_id,
            fractal_id,
            petal_id: &petal_id,
        };
        let _ = header.body(|ui| {
            render_nodes(
                ui,
                &petals[pi].nodes,
                camera_focus,
                node_mgr,
                ui_mgr,
                is_active,
                ancestry,
                vis_state,
            );
            ui.horizontal(|ui| {
                // [+] Add Node inside the petal collapse
                add_button_inline(ui, "Add Node", CreateKind::Node, &petal_id, ui_mgr);
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("Manifest").small())
                            .fill(theme::BG_BUTTON)
                            .small(),
                    )
                    .on_hover_text("Edit petal hexon manifest")
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::PetalManifestOpen {
                        petal_id: petal_id.clone(),
                        petal_name: petal_name.clone(),
                    });
                }
            });
        });

        if name_clicked {
            nav.navigate_to_petal(petal_id.clone());
        }
        if eye_clicked {
            vis_state.toggle_petal_hidden(&petal_id);
        }
    }
}

fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &[NodeEntry],
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    is_active_petal: bool,
    ancestry: NodeAncestry<'_>,
    vis_state: &mut VisibilityState,
) {
    let mut node_click: Option<(String, [f32; 3])> = None;
    let mut node_alt_click: Option<(String, String, String)> = None;
    let mut override_write: Option<(String, OverrideState)> = None;

    for node in nodes.iter() {
        let node_id = node.id.clone();
        let node_name = node.name.clone();
        let has_asset = node.has_asset;
        let position = node.position;
        let webpage_url = node.webpage_url.clone().unwrap_or_default();
        let is_selected =
            node_mgr.selected.as_ref().map(|s| s.node_id.as_str()) == Some(node_id.as_str());

        let reason = visibility::node_hidden_reason(&node_id, ancestry, vis_state);
        let effectively_visible = reason == visibility::HiddenReason::NotHidden;
        let glyph = visibility::glyph_for_bool(effectively_visible);
        let eye_tooltip = visibility::node_eye_tooltip(&reason);

        let bg = if is_selected {
            theme::TREE_SELECTED_BG
        } else {
            egui::Color32::TRANSPARENT
        };

        let mut eye_clicked = false;
        let row = egui::Frame::NONE
            .fill(bg)
            .inner_margin(egui::Margin::symmetric(4, 1))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let icon = if has_asset { "\u{25C6}" } else { "\u{25CF}" };
                    ui.label(
                        egui::RichText::new(icon)
                            .small()
                            .color(theme::TREE_NODE_ICON),
                    );
                    let text_color = if !is_active_petal || !effectively_visible {
                        theme::TEXT_MUTED
                    } else if is_selected {
                        theme::TEXT_BRIGHT
                    } else {
                        theme::TEXT_SECTION
                    };
                    let label_resp = ui.add(
                        egui::Label::new(egui::RichText::new(&node_name).small().color(text_color))
                            .sense(if is_active_petal {
                                egui::Sense::click()
                            } else {
                                egui::Sense::hover()
                            }),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        eye_clicked = eye_button(ui, glyph, true, &eye_tooltip);
                    });
                    label_resp
                })
                .inner
            });

        let label_resp: egui::Response = row.inner;

        let is_alt = ui.input(|inp| inp.modifiers.alt);

        if label_resp.clicked() && is_active_petal {
            if is_alt {
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
        camera_focus.target = Some((nid.clone(), pos));
        node_mgr.pending_sidebar_select = Some(nid);
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

    // Apply the eye click's tri-state write (RATIFICATION #6 — Auto is
    // represented by absence, keeping the map small; `set_node_override`
    // handles that + bumps `VisibilityState::epoch` — see `visibility/mod.rs`
    // module docs).
    if let Some((nid, next)) = override_write {
        vis_state.set_node_override(&nid, next);
    }
}

fn add_button_inline(
    ui: &mut egui::Ui,
    tooltip: &str,
    kind: CreateKind,
    parent_id: &str,
    ui_mgr: &mut UiManager,
) {
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new("+").fill(theme::BG_BUTTON_ALT).small())
            .on_hover_text(tooltip)
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

fn sidebar_section_space_overview(ui: &mut egui::Ui, dashboard: &DashboardState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Space")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);
        egui::Grid::new("space_stats")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Petals").color(theme::TEXT_DIM).small());
                ui.label(egui::RichText::new(dashboard.petal_count.to_string()).strong());
                ui.end_row();
                ui.label(egui::RichText::new("Models").color(theme::TEXT_DIM).small());
                ui.label(egui::RichText::new(dashboard.model_count.to_string()).strong());
                ui.end_row();
                ui.label(egui::RichText::new("Peers").color(theme::TEXT_DIM).small());
                ui.label(egui::RichText::new(dashboard.peer_count.to_string()).strong());
                ui.end_row();
            });
        ui.add_space(4.0);
    });
}
