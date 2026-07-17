//! Left sidebar: verse/fractal/petal/node hierarchy tree + space overview.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::atlas::DashboardState;
use crate::dialogs::{ActiveDialog, CreateKind};
use crate::navigation_manager::NavigationManager;
use crate::plugin::{CameraFocusTarget, SidebarState};
use crate::theme;
use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseManager};
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
                    render_verse_tree(ui, hierarchy, nav, camera_focus, node_mgr, ui_mgr);
                    ui.add_space(8.0);
                    sidebar_section_space_overview(ui, dashboard);
                    ui.add_space(4.0);
                });

            // Bottom-pinned reset button
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(6.0);
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
                    db_tx.send(DbCommand::ResetDatabase).ok();
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

fn render_verse_tree(
    ui: &mut egui::Ui,
    hierarchy: &mut VerseManager,
    nav: &mut NavigationManager,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
) {
    let verse_count = hierarchy.verses.len();
    for vi in 0..verse_count {
        let verse_id = hierarchy.verses[vi].id.clone();
        let verse_name = hierarchy.verses[vi].name.clone();
        let is_active_verse = nav.active_verse_id.as_deref() == Some(&verse_id);

        let header_text = egui::RichText::new(&verse_name)
            .strong()
            .color(if is_active_verse {
                theme::TEXT_BRIGHT
            } else {
                theme::TEXT_SECTION
            });

        let resp = egui::CollapsingHeader::new(header_text)
            .id_salt(format!("verse_{}", verse_id))
            .default_open(true)
            .show(ui, |ui| {
                render_fractals(
                    ui,
                    &mut hierarchy.verses[vi].fractals,
                    nav,
                    &verse_id,
                    camera_focus,
                    node_mgr,
                    ui_mgr,
                );
                // [+] Add Fractal inside the verse collapse
                add_button_inline(ui, "Add Fractal", CreateKind::Fractal, &verse_id, ui_mgr);
            });

        if resp.header_response.clicked() {
            nav.navigate_to_verse(verse_id.clone(), verse_name.clone());
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
) {
    let fractal_count = fractals.len();
    for fi in 0..fractal_count {
        let fractal_id = fractals[fi].id.clone();
        let fractal_name = fractals[fi].name.clone();
        let is_active = nav.active_fractal_id.as_deref() == Some(&fractal_id);

        let header_text = egui::RichText::new(&fractal_name).color(if is_active {
            theme::TEXT_BRIGHT
        } else {
            theme::TEXT_SECTION
        });

        let resp = egui::CollapsingHeader::new(header_text)
            .id_salt(format!("fractal_{}_{}", verse_id, fractal_id))
            .default_open(true)
            .show(ui, |ui| {
                render_petals(
                    ui,
                    &mut fractals[fi].petals,
                    nav,
                    &fractal_id,
                    camera_focus,
                    node_mgr,
                    ui_mgr,
                );
                // [+] Add Petal inside the fractal collapse
                add_button_inline(ui, "Add Petal", CreateKind::Petal, &fractal_id, ui_mgr);
            });

        if resp.header_response.clicked() {
            nav.navigate_to_fractal(fractal_id.clone(), fractal_name.clone());
        }
    }
}

#[allow(clippy::needless_range_loop)]
fn render_petals(
    ui: &mut egui::Ui,
    petals: &mut [PetalEntry],
    nav: &mut NavigationManager,
    fractal_id: &str,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
) {
    let petal_count = petals.len();
    for pi in 0..petal_count {
        let petal_id = petals[pi].id.clone();
        let petal_name = petals[pi].name.clone();
        let is_active = nav.active_petal_id.as_deref() == Some(&petal_id);

        let header_text = egui::RichText::new(&petal_name).color(if is_active {
            theme::TEXT_BRIGHT
        } else {
            theme::TEXT_SECTION
        });

        let resp = egui::CollapsingHeader::new(header_text)
            .id_salt(format!("petal_{}_{}", fractal_id, petal_id))
            .default_open(true)
            .show(ui, |ui| {
                render_nodes(
                    ui,
                    &mut petals[pi].nodes,
                    camera_focus,
                    node_mgr,
                    ui_mgr,
                    is_active,
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

        if resp.header_response.clicked() {
            nav.navigate_to_petal(petal_id.clone());
        }
    }
}

fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &mut Vec<NodeEntry>,
    camera_focus: &mut CameraFocusTarget,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    is_active_petal: bool,
) {
    let drag_id = egui::Id::new("sidebar_node_drag");
    let dragging_idx: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_id));

    let mut drop_target_idx: Option<usize> = None;
    let mut node_click: Option<(String, [f32; 3])> = None;
    let mut node_alt_click: Option<(String, String, String)> = None;

    for (i, node) in nodes.iter().enumerate() {
        let node_id = node.id.clone();
        let node_name = node.name.clone();
        let has_asset = node.has_asset;
        let position = node.position;
        let webpage_url = node.webpage_url.clone().unwrap_or_default();
        let is_selected =
            node_mgr.selected.as_ref().map(|s| s.node_id.as_str()) == Some(node_id.as_str());
        let is_being_dragged = dragging_idx == Some(i);

        let bg = if is_selected {
            theme::TREE_SELECTED_BG
        } else {
            egui::Color32::TRANSPARENT
        };

        let row = egui::Frame::NONE
            .fill(bg)
            .inner_margin(egui::Margin::symmetric(4, 1))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Drag handle
                    ui.label(
                        egui::RichText::new(if is_being_dragged { "✦" } else { "⠿" })
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    let icon = if has_asset { "\u{25C6}" } else { "\u{25CF}" };
                    ui.label(
                        egui::RichText::new(icon)
                            .small()
                            .color(theme::TREE_NODE_ICON),
                    );
                    ui.add(
                        egui::Label::new(egui::RichText::new(&node_name).small().color(
                            if !is_active_petal {
                                theme::TEXT_MUTED
                            } else if is_selected {
                                theme::TEXT_BRIGHT
                            } else {
                                theme::TEXT_SECTION
                            },
                        ))
                        .sense(if is_active_petal {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        }),
                    )
                })
                .inner
            });

        let label_resp: egui::Response = row.inner;
        let row_id = egui::Id::new(("node_row_drag", node_id.as_str()));
        let drag_resp = ui.interact(row.response.rect, row_id, egui::Sense::drag());

        let is_alt = ui.input(|inp| inp.modifiers.alt);

        if label_resp.clicked() && is_active_petal {
            if is_alt {
                node_alt_click = Some((node_id.clone(), node_name.clone(), webpage_url));
            } else {
                node_click = Some((node_id.clone(), position));
            }
        }

        if drag_resp.drag_started() {
            ui.ctx().data_mut(|d| d.insert_temp::<usize>(drag_id, i));
        }

        if dragging_idx.is_some()
            && drag_resp.hovered()
            && ui.input(|inp| inp.pointer.primary_released())
        {
            drop_target_idx = Some(i);
        }

        // Show drop indicator when dragging over this item
        if dragging_idx.is_some() && drag_resp.hovered() {
            ui.painter().hline(
                row.response.rect.x_range(),
                row.response.rect.top(),
                egui::Stroke::new(2.0, theme::BG_BUTTON_ACTIVE),
            );
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
        });
    }

    // Apply DnD reorder on pointer release
    if ui.input(|inp| inp.pointer.primary_released()) {
        if let Some(from_idx) = dragging_idx {
            ui.ctx().data_mut(|d| d.remove::<usize>(drag_id));
            if let Some(to_idx) = drop_target_idx {
                if from_idx != to_idx && from_idx < nodes.len() && to_idx < nodes.len() {
                    let item = nodes.remove(from_idx);
                    let insert_at = if from_idx < to_idx {
                        to_idx - 1
                    } else {
                        to_idx
                    };
                    nodes.insert(insert_at, item);
                }
            }
        }
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
                ui.label(egui::RichText::new("Rooms").color(theme::TEXT_DIM).small());
                ui.label(egui::RichText::new(dashboard.room_count.to_string()).strong());
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
