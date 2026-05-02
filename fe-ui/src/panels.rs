use bevy_egui::egui;

use crate::atlas::DashboardState;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{
    ActiveDialog, CameraFocusTarget, CreateKind, API_TOKEN_PAGE_SIZE,
    InspectorFormState, InspectorTab, LocalUserRole, SidebarState, ToolState, UiAction, UiManager,
    ViewportCursorWorld,
};
use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseManager};
use crate::theme;
use fe_runtime::messages::DbCommand;

use crate::dialogs::{
    render_context_menu, render_create_dialog, render_entity_settings_dialog,
    render_gltf_import_dialog,
    render_join_dialog, render_node_options_dialog, render_peer_debug_panel,
};
use crate::viewport::viewport_overlay;

// ---------------------------------------------------------------------------
// Top-level layout entry point
// ---------------------------------------------------------------------------

/// Renders the full UI shell: toolbar -> status bar -> sidebar -> inspector -> viewport.
/// Returns the screen-space rect of the 3-D viewport (CentralPanel) so the
/// caller can store it for viewport-click gating in the gimbal system.
pub fn gardener_console(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    tool: &mut ToolState,
    inspector: &mut InspectorFormState,
    nav: &mut NavigationManager,
    dashboard: &DashboardState,
    hierarchy: &mut VerseManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    camera_focus: &mut CameraFocusTarget,
    cursor_world: &ViewportCursorWorld,
    sync_status: Option<&fe_sync::SyncStatus>,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
) -> egui::Rect {
    top_toolbar(ctx, sidebar, tool, node_mgr);
    status_bar(ctx, dashboard, sync_status, nav, ui_mgr);
    left_sidebar(
        ctx,
        sidebar,
        nav,
        dashboard,
        hierarchy,
        camera_focus,
        db_tx,
        node_mgr,
        ui_mgr,
    );

    // Auto-collapse left sidebar when right panel is open, restore when closed.
    let right_panel_open = ui_mgr.portal_is_open() || node_mgr.selected_entity().is_some();
    sidebar.open = !right_panel_open;

    // When the portal webview is open, show the portal toolbar instead of inspector.
    if ui_mgr.portal_is_open() {
        right_portal_toolbar(ctx, ui_mgr);
    } else {
        right_inspector(ctx, inspector, node_mgr, ui_mgr, local_role, db_tx, nav);
    }

    let viewport_response = egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            viewport_overlay(
                ui,
                nav,
                node_mgr,
                hierarchy,
                db_tx,
                dashboard,
                cursor_world,
                ui_mgr,
                local_role,
            );
        });

    // Floating dialogs / menus (rendered after panels so they layer on top)
    render_context_menu(ctx, ui_mgr);
    render_create_dialog(ctx, ui_mgr, hierarchy, nav, db_tx);
    render_gltf_import_dialog(ctx, ui_mgr, nav, db_tx);
    render_join_dialog(ctx, ui_mgr, db_tx);
    render_peer_debug_panel(ctx, ui_mgr, sync_status);
    render_node_options_dialog(ctx, ui_mgr, hierarchy, db_tx);
    render_entity_settings_dialog(ctx, ui_mgr, db_tx);

    // Toast overlay (bottom-left, semi-transparent)
    render_toast(ctx, ui_mgr);

    viewport_response.response.rect
}

fn render_toast(ctx: &egui::Context, ui_mgr: &UiManager) {
    let now = ctx.input(|i| i.time);
    let Some((msg, alpha)) = ui_mgr.active_toast(now) else { return };

    let bg = egui::Color32::from_rgba_unmultiplied(30, 30, 40, (alpha * 220.0) as u8);
    let text_color = egui::Color32::from_rgba_unmultiplied(220, 220, 220, (alpha * 255.0) as u8);

    egui::Area::new(egui::Id::new("toast_overlay"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -40.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(bg)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).color(text_color).small());
                });
        });

    // Request repaint while toast is visible so the fade animates
    ctx.request_repaint();
}

// ---------------------------------------------------------------------------
// Top toolbar
// ---------------------------------------------------------------------------

fn top_toolbar(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    tool: &mut ToolState,
    node_mgr: &mut crate::node_manager::NodeManager,
) {
    egui::TopBottomPanel::top("toolbar")
        .exact_height(40.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_TOOLBAR)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                let sidebar_icon = if sidebar.open {
                    "\u{25C0} Hide"
                } else {
                    "\u{25B6} Show"
                };
                if ui
                    .add(egui::Button::new(sidebar_icon).fill(theme::BG_BUTTON))
                    .clicked()
                {
                    sidebar.open = !sidebar.open;
                }

                ui.separator();

                for (t, label, tooltip) in [
                    (Tool::Select, "\u{2B1A} Select", "Select objects (S)"),
                    (Tool::Move, "\u{271B} Move", "Move selected object (G)"),
                    (
                        Tool::Rotate,
                        "\u{21BB} Rotate",
                        "Rotate selected object (R)",
                    ),
                    (Tool::Scale, "\u{2921} Scale", "Scale selected object (X)"),
                ] {
                    let active = tool.active_tool == t;
                    let btn = egui::Button::new(label).fill(if active {
                        theme::BG_BUTTON_ACTIVE
                    } else {
                        theme::BG_BUTTON
                    });
                    if ui.add(btn).on_hover_text(tooltip).clicked() {
                        tool.active_tool = t;
                    }
                }

                ui.separator();

                if node_mgr.selected_entity().is_some()
                    && ui
                        .add(egui::Button::new("\u{2715} Deselect").fill(theme::BG_DANGER))
                        .clicked()
                {
                    node_mgr.deselect();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("FractalEngine")
                            .color(theme::TEXT_DIM)
                            .small(),
                    );
                });
            });
        });
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn status_bar(
    ctx: &egui::Context,
    dashboard: &DashboardState,
    sync_status: Option<&fe_sync::SyncStatus>,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
) {
    egui::TopBottomPanel::bottom("statusbar")
        .exact_height(22.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_STATUSBAR)
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Phase F: prefer SyncStatus over DashboardState for online/peer info
                let is_online = sync_status.map(|s| s.online).unwrap_or(dashboard.is_online);
                let peer_count = sync_status
                    .map(|s| s.peer_count as u64)
                    .unwrap_or(dashboard.peer_count);

                if is_online {
                    ui.colored_label(theme::STATUS_ONLINE_DOT, "\u{25CF}");
                    ui.label(
                        egui::RichText::new("Online")
                            .small()
                            .color(theme::STATUS_ONLINE),
                    );
                } else {
                    ui.colored_label(theme::STATUS_OFFLINE_DOT, "\u{25CF}");
                    ui.label(
                        egui::RichText::new("Offline")
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }

                ui.separator();
                // Clickable peer count opens debug panel
                let peer_label = egui::RichText::new(format!("{} peers", peer_count))
                    .small()
                    .color(theme::TEXT_DIM);
                if ui
                    .add(egui::Label::new(peer_label).sense(egui::Sense::click()))
                    .on_hover_text("Click for peer debug panel")
                    .clicked()
                {
                    if matches!(ui_mgr.active_dialog, ActiveDialog::PeerDebug) {
                        ui_mgr.close_dialog();
                    } else {
                        ui_mgr.open_dialog(ActiveDialog::PeerDebug);
                    }
                }

                // Phase F: show active verse name
                if let Some(ref _vid) = nav.active_verse_id {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("Verse: {}", nav.active_verse_name))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                } else if !is_online {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Local only")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }

                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{} petals  {} rooms  {} models",
                        dashboard.petal_count, dashboard.room_count, dashboard.model_count
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });
}

// ---------------------------------------------------------------------------
// Left sidebar — verse hierarchy tree
// ---------------------------------------------------------------------------

fn left_sidebar(
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
                    egui::RichText::new("Reset Database").small().color(theme::TEXT_DIM),
                )
                .fill(theme::BG_BUTTON_ALT);
                if ui.add(reset_btn).on_hover_text("Wipe all data and re-seed defaults").clicked() {
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
                add_button_inline(
                    ui,
                    "Add Fractal",
                    CreateKind::Fractal,
                    &verse_id,
                    ui_mgr,
                );
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
                add_button_inline(
                    ui,
                    "Add Petal",
                    CreateKind::Petal,
                    &fractal_id,
                    ui_mgr,
                );
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
                render_nodes(ui, &mut petals[pi].nodes, camera_focus, node_mgr, ui_mgr, is_active);
                // [+] Add Node inside the petal collapse
                add_button_inline(ui, "Add Node", CreateKind::Node, &petal_id, ui_mgr);
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

    let node_count = nodes.len();
    for i in 0..node_count {
        let node_id = nodes[i].id.clone();
        let node_name = nodes[i].name.clone();
        let has_asset = nodes[i].has_asset;
        let position = nodes[i].position;
        let webpage_url = nodes[i].webpage_url.clone().unwrap_or_default();
        let is_selected = node_mgr.selected.as_ref().map(|s| s.node_id.as_str()) == Some(node_id.as_str());
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
                    ui.label(egui::RichText::new(icon).small().color(theme::TREE_NODE_ICON));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&node_name).small().color(
                                if !is_active_petal {
                                    theme::TEXT_MUTED
                                } else if is_selected {
                                    theme::TEXT_BRIGHT
                                } else {
                                    theme::TEXT_SECTION
                                },
                            ),
                        )
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

        if dragging_idx.is_some() && drag_resp.hovered() {
            if ui.input(|inp| inp.pointer.primary_released()) {
                drop_target_idx = Some(i);
            }
        }

        // Show drop indicator when dragging over this item
        if dragging_idx.is_some() && drag_resp.hovered() {
            ui.painter()
                .hline(row.response.rect.x_range(), row.response.rect.top(), egui::Stroke::new(2.0, theme::BG_BUTTON_ACTIVE));
        }
    }

    // Apply selection — route through NodeManager's pending mechanism.
    if let Some((nid, pos)) = node_click {
        node_mgr.pending_sidebar_select = Some(nid);
        camera_focus.target = Some(pos);
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

// ---------------------------------------------------------------------------
// Right inspector panel
// ---------------------------------------------------------------------------

fn right_inspector(
    ctx: &egui::Context,
    inspector: &mut InspectorFormState,
    node_mgr: &mut crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
) {
    let open = node_mgr.selected_entity().is_some();
    // Allow up to 80% of screen width.
    let max_w = ctx.viewport_rect().width() * 0.8;
    egui::SidePanel::right("inspector")
        .resizable(true)
        .default_width(260.0)
        .width_range(200.0..=max_w)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0))
                .stroke(egui::Stroke::new(2.0, theme::BG_BUTTON)),
        )
        .show_animated(ctx, open, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Inspector")
                        .strong()
                        .color(theme::TEXT_HEADING),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Button::new("\u{2715}")
                                .fill(egui::Color32::TRANSPARENT)
                                .small(),
                        )
                        .clicked()
                    {
                        node_mgr.deselect();
                    }
                });
            });

            // Tab bar
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                for (tab, label) in [
                    (InspectorTab::Properties, "Properties"),
                    (InspectorTab::ApiAccess, "API Access"),
                ] {
                    let active = inspector.active_tab == tab;
                    let btn = egui::Button::new(
                        egui::RichText::new(label).small().color(
                            if active { theme::TEXT_BRIGHT } else { theme::TEXT_DIM }
                        ),
                    )
                    .fill(if active { theme::BG_BUTTON_ACTIVE } else { theme::BG_BUTTON });
                    if ui.add(btn).clicked() {
                        let prev_tab = inspector.active_tab;
                        inspector.active_tab = tab;

                        // Clear sensitive state when leaving the API tab
                        if prev_tab == InspectorTab::ApiAccess && tab != InspectorTab::ApiAccess {
                            inspector.generated_api_token = None;
                        }

                        // Auto-populate scope + fetch tokens when entering API tab
                        if tab == InspectorTab::ApiAccess {
                            inspector.api_tokens_page = 0;
                            if inspector.api_token_scope_buf.is_empty() {
                                inspector.api_token_scope_buf = build_nav_scope(nav);
                            }
                            let scope = inspector.api_token_scope_buf.clone();
                            if !scope.is_empty() {
                                db_tx.send(DbCommand::ListApiTokensByScope {
                                    scope_prefix: scope,
                                    offset: 0,
                                    limit: API_TOKEN_PAGE_SIZE,
                                }).ok();
                            }
                        }
                    }
                }
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    match inspector.active_tab {
                        InspectorTab::Properties => {
                            ui.add_space(4.0);
                            inspector_entity_section(ui, node_mgr);
                            ui.add_space(2.0);
                            inspector_transform_section(ui, inspector);
                            ui.add_space(2.0);
                            inspector_url_meta_section(ui, inspector, ui_mgr, local_role);
                        }
                        InspectorTab::ApiAccess => {
                            ui.add_space(4.0);
                            inspector_api_access_section(ui, inspector, node_mgr, db_tx, local_role, ui_mgr);
                        }
                    }
                });
        });
}

/// Build a scope string from the current navigation state.
fn build_nav_scope(nav: &NavigationManager) -> String {
    match (&nav.active_verse_id, &nav.active_fractal_id, &nav.active_petal_id) {
        (Some(v), Some(f), Some(p)) => fe_database::build_scope(v, Some(f), Some(p)),
        (Some(v), Some(f), None) => fe_database::build_scope(v, Some(f), None),
        (Some(v), None, _) => fe_database::build_scope(v, None, None),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Portal webview toolbar (replaces inspector when portal is open)
// ---------------------------------------------------------------------------

fn right_portal_toolbar(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let max_w = ctx.viewport_rect().width() * 0.8;
    egui::SidePanel::right("portal_toolbar")
        .resizable(true)
        .default_width(400.0)
        .width_range(260.0..=max_w)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0))
                .stroke(egui::Stroke::new(2.0, theme::BG_BUTTON)),
        )
        .show(ctx, |ui| {
            // Toolbar row: back, URL, close
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);

                // Back button
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("\u{2190}").size(16.0), // ←
                        )
                        .fill(theme::BG_BUTTON)
                        .min_size(egui::vec2(28.0, 24.0)),
                    )
                    .on_hover_text("Go back")
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::PortalGoBack);
                }

                ui.add_space(4.0);

                // URL label (truncated hostname — pre-cached, no per-frame parse)
                let display_url = ui_mgr.portal_hostname().to_string();
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("\u{1F310} {display_url}"))
                            .color(theme::TEXT_DIM)
                            .size(12.0),
                    )
                    .truncate(),
                );

                // Close button (right-aligned)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("\u{2715}").size(14.0), // ✕
                            )
                            .fill(egui::Color32::TRANSPARENT)
                            .small(),
                        )
                        .on_hover_text("Close portal")
                        .clicked()
                    {
                        ui_mgr.push_action(UiAction::ClosePortal);
                    }
                });
            });

            ui.separator();

            // The rest of the panel is empty — the native webview renders over it.
            ui.allocate_space(ui.available_size());
        });
}

fn inspector_entity_section(ui: &mut egui::Ui, node_mgr: &crate::node_manager::NodeManager) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Entity")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        if let Some(sel) = &node_mgr.selected {
            egui::Grid::new("entity_info")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Node ID").small().color(theme::TEXT_DIM));
                    let id_label = ui.add(
                        egui::Label::new(
                            egui::RichText::new(&sel.node_id)
                                .monospace()
                                .small(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if id_label.clicked() {
                        ui.ctx().copy_text(sel.node_id.clone());
                    }
                    id_label.on_hover_text("Click to copy");
                    ui.end_row();
                });
        }
        ui.add_space(4.0);
    });
}

fn inspector_transform_section(ui: &mut egui::Ui, inspector: &mut InspectorFormState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Transform")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        for (label, bufs) in [
            ("Position", &mut inspector.pos),
            ("Rotation", &mut inspector.rot),
            ("Scale", &mut inspector.scale),
        ] {
            ui.horizontal(|ui| {
                // Compute a dynamic input width so the three fields always fit
                // within the inspector panel regardless of its current width.
                const LABEL_W: f32 = 54.0;
                const AXIS_W: f32 = 10.0; // "X" / "Y" / "Z"
                let spacing = ui.spacing().item_spacing.x;
                // total = LABEL_W + gap + 3*(AXIS_W + gap + input_w + gap)
                let input_w = ((ui.available_width()
                    - LABEL_W
                    - spacing * 7.0
                    - AXIS_W * 3.0)
                    / 3.0)
                    .max(32.0);

                ui.add_sized(
                    [LABEL_W, 16.0],
                    egui::Label::new(egui::RichText::new(label).small().color(theme::TEXT_DIM)),
                );
                for (axis, buf) in ["X", "Y", "Z"].iter().zip(bufs.iter_mut()) {
                    ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
                    ui.add(
                        egui::TextEdit::singleline(buf)
                            .desired_width(input_w)
                            .font(egui::TextStyle::Monospace),
                    );
                }
            });
        }
        ui.add_space(4.0);
    });
}

fn inspector_url_meta_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Portal URLs")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);

        ui.label(
            egui::RichText::new("External URL")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(
            egui::TextEdit::singleline(&mut inspector.external_url)
                .hint_text("https://\u{2026}")
                .desired_width(f32::INFINITY),
        );

        if local_role.can_manage() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Config URL (admin)")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut inspector.config_url)
                    .hint_text("https://admin.\u{2026}")
                    .desired_width(f32::INFINITY),
            );
        }

        ui.add_space(6.0);
        if ui
            .add(
                egui::Button::new("\u{1F4BE} Save")
                    .fill(theme::BG_SAVE)
                    .min_size(egui::vec2(ui.available_width(), 28.0)),
            )
            .clicked()
        {
            ui_mgr.push_action(UiAction::SaveUrl);
        }

        ui.add_space(4.0);

        let has_url = !inspector.external_url.trim().is_empty();
        let btn = egui::Button::new("\u{1F310} Open Portal")
            .fill(if has_url { theme::BG_BUTTON_ACTIVE } else { theme::BG_BUTTON })
            .min_size(egui::vec2(ui.available_width(), 28.0));
        let resp = ui.add_enabled(has_url, btn);
        if resp.clicked() {
            bevy::log::info!("Portal: 'Open Portal' clicked — URL: {}", inspector.external_url);
            ui_mgr.push_action(UiAction::OpenPortal { url: inspector.external_url.clone() });
        }
        if !has_url {
            resp.on_hover_text("Set a Portal URL above to open the webview");
        }

        ui.add_space(4.0);
    });
}

fn inspector_api_access_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &crate::node_manager::NodeManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    local_role: &LocalUserRole,
    ui_mgr: &mut UiManager,
) {
    let node_id = match node_mgr.selected.as_ref() {
        Some(sel) => &sel.node_id,
        None => return,
    };

    // Padded container for the entire API Access tab
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());

            if !local_role.can_manage() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Manager role or higher required to manage API tokens.")
                        .small()
                        .color(theme::TEXT_MUTED)
                        .italics(),
                );
                return;
            }

            // --- Generate Token section ---
            egui::CollapsingHeader::new(
                egui::RichText::new("Generate Token")
                    .strong()
                    .color(theme::TEXT_SECTION),
            )
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(4.0);

                // Scope + Node ID — read-only, wrapped to panel width
                ui.label(egui::RichText::new("Scope").small().color(theme::TEXT_DIM));
                egui::Frame::NONE
                    .fill(theme::BG_BUTTON)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .corner_radius(3.0)
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&inspector.api_token_scope_buf)
                                    .small()
                                    .monospace()
                                    .color(theme::TEXT_SECTION),
                            )
                            .wrap(),
                        );
                    });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Node ID").small().color(theme::TEXT_DIM));
                    let id_resp = ui.add(
                        egui::Label::new(
                            egui::RichText::new(node_id)
                                .small()
                                .monospace()
                                .color(theme::TEXT_SECTION),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if id_resp.clicked() {
                        ui.ctx().copy_text(node_id.to_string());
                        let now = ui.ctx().input(|i| i.time);
                        ui_mgr.show_toast("Copied Node ID", now);
                    }
                    id_resp.on_hover_text("Click to copy");
                });

                if inspector.api_token_scope_buf.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Navigate to a verse/fractal/petal to set the token scope.")
                            .small()
                            .color(theme::TEXT_MUTED)
                            .italics(),
                    );
                    return;
                }

                ui.add_space(6.0);

                // Role + Expiry on separate rows for narrow panels
                egui::Grid::new("api_token_opts")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Role").small().color(theme::TEXT_DIM));
                        egui::ComboBox::from_id_salt("inspector_api_role")
                            .selected_text(inspector.api_token_role_buf.as_str())
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut inspector.api_token_role_buf, "viewer".to_string(), "Viewer");
                                ui.selectable_value(&mut inspector.api_token_role_buf, "editor".to_string(), "Editor");
                                ui.selectable_value(&mut inspector.api_token_role_buf, "manager".to_string(), "Manager");
                            });
                        ui.end_row();

                        ui.label(egui::RichText::new("Expires").small().color(theme::TEXT_DIM));
                        egui::ComboBox::from_id_salt("inspector_api_expiry")
                            .selected_text(match inspector.api_token_expiry_buf {
                                1 => "1 hour",
                                24 => "24 hours",
                                168 => "7 days",
                                720 => "30 days",
                                _ => "30 days",
                            })
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 1, "1 hour");
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 24, "24 hours");
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 168, "7 days");
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 720, "30 days");
                            });
                        ui.end_row();
                    });

                ui.add_space(6.0);
                if ui
                    .add(
                        egui::Button::new("Generate API Token")
                            .fill(theme::BG_SAVE)
                            .min_size(egui::vec2(ui.available_width(), 28.0)),
                    )
                    .clicked()
                {
                    db_tx
                        .send(DbCommand::MintApiToken {
                            scope: inspector.api_token_scope_buf.clone(),
                            max_role: inspector.api_token_role_buf.clone(),
                            ttl_hours: inspector.api_token_expiry_buf,
                            label: Some(format!("node:{}", node_id)),
                        })
                        .ok();
                }

                // Show generated token (selectable, with copy buttons)
                let mut dismiss_token = false;
                if let Some(ref token) = inspector.generated_api_token {
                    ui.add_space(6.0);
                    egui::Frame::NONE
                        .fill(theme::BG_BUTTON)
                        .inner_margin(egui::Margin::same(8))
                        .corner_radius(4.0)
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Generated Token")
                                        .small()
                                        .strong()
                                        .color(theme::TEXT_SECTION),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(egui::Button::new("\u{2715}").fill(egui::Color32::TRANSPARENT).small()).clicked() {
                                        dismiss_token = true;
                                    }
                                });
                            });

                            ui.add_space(2.0);
                            let mut display = token.clone();
                            ui.add(
                                egui::TextEdit::multiline(&mut display)
                                    .desired_width(ui.available_width())
                                    .desired_rows(3)
                                    .font(egui::TextStyle::Monospace),
                            );

                            ui.add_space(4.0);
                            let now = ui.ctx().input(|i| i.time);

                            // Copy Token button
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new("\u{1F4CB} Copy Token").fill(theme::BG_BUTTON_ACTIVE).small()).clicked() {
                                    ui.ctx().copy_text(token.clone());
                                    ui_mgr.show_toast("Copied token to clipboard", now);
                                }
                            });

                            // curl command — pre-built with the actual token
                            ui.add_space(4.0);
                            let curl_cmd = format!(
                                "curl.exe -s http://localhost:8765/health -H \"Authorization: Bearer {}\"",
                                token,
                            );
                            egui::Frame::NONE
                                .fill(egui::Color32::from_rgb(20, 20, 30))
                                .inner_margin(egui::Margin::symmetric(6, 4))
                                .corner_radius(3.0)
                                .show(ui, |ui| {
                                    ui.set_max_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("curl")
                                                .small()
                                                .color(theme::TEXT_MUTED),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.add(egui::Button::new("\u{1F4CB}").fill(egui::Color32::TRANSPARENT).small())
                                                .on_hover_text("Copy curl command")
                                                .clicked()
                                            {
                                                ui.ctx().copy_text(curl_cmd.clone());
                                                ui_mgr.show_toast("Copied curl command", now);
                                            }
                                        });
                                    });
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&curl_cmd)
                                                .small()
                                                .monospace()
                                                .color(theme::TEXT_DIM),
                                        )
                                        .wrap(),
                                    );
                                });
                        });
                }
                if dismiss_token {
                    inspector.generated_api_token = None;
                }

                ui.add_space(4.0);
            });

            ui.add_space(2.0);

            // --- Active Tokens section ---
            egui::CollapsingHeader::new(
                egui::RichText::new("Active Tokens")
                    .strong()
                    .color(theme::TEXT_SECTION),
            )
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(4.0);

                if inspector.api_tokens_loading {
                    ui.label(
                        egui::RichText::new("Loading tokens...")
                            .color(theme::TEXT_MUTED)
                            .italics(),
                    );
                } else if inspector.api_tokens.is_empty() {
                    ui.label(
                        egui::RichText::new("No active API tokens.")
                            .color(theme::TEXT_MUTED)
                            .italics(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "Showing {} of {}",
                            inspector.api_tokens.len(),
                            inspector.api_tokens_total,
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(2.0);

                    let mut revoke_jti: Option<String> = None;
                    for (i, tok) in inspector.api_tokens.iter().enumerate() {
                        let row_bg = if i % 2 == 0 {
                            theme::BG_PEER_ROW_EVEN
                        } else {
                            theme::BG_PEER_ROW_ODD
                        };
                        egui::Frame::NONE
                            .fill(row_bg)
                            .inner_margin(egui::Margin::symmetric(6, 4))
                            .corner_radius(2.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(&tok.max_role)
                                            .small()
                                            .strong()
                                            .color(theme::TEXT_SECTION),
                                    );
                                    if let Some(ref lbl) = tok.label {
                                        ui.label(
                                            egui::RichText::new(lbl)
                                                .small()
                                                .color(theme::TEXT_MUTED),
                                        );
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new("Revoke").small(),
                                                    )
                                                    .fill(theme::BG_DANGER),
                                                )
                                                .clicked()
                                            {
                                                revoke_jti = Some(tok.jti.clone());
                                            }
                                        },
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(format!("exp: {}", tok.expires_at))
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                            });
                        ui.add_space(1.0);
                    }
                    if let Some(jti) = revoke_jti {
                        db_tx.send(DbCommand::RevokeApiToken { jti }).ok();
                    }
                }

                // Pagination + refresh row
                ui.add_space(4.0);
                let total_pages = ((inspector.api_tokens_total as u32).max(1) - 1) / API_TOKEN_PAGE_SIZE + 1;
                let current_page = inspector.api_tokens_page;
                ui.horizontal(|ui| {
                    let can_prev = current_page > 0;
                    if ui.add_enabled(can_prev, egui::Button::new("\u{25C0}").fill(theme::BG_BUTTON).small()).clicked() {
                        inspector.api_tokens_page = current_page.saturating_sub(1);
                        send_scoped_token_list(db_tx, &inspector.api_token_scope_buf, inspector.api_tokens_page);
                    }
                    ui.label(
                        egui::RichText::new(format!("{}/{}", current_page + 1, total_pages))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    let can_next = current_page + 1 < total_pages;
                    if ui.add_enabled(can_next, egui::Button::new("\u{25B6}").fill(theme::BG_BUTTON).small()).clicked() {
                        inspector.api_tokens_page = current_page + 1;
                        send_scoped_token_list(db_tx, &inspector.api_token_scope_buf, inspector.api_tokens_page);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("\u{21BB} Refresh").fill(theme::BG_BUTTON).small()).clicked() {
                            send_scoped_token_list(db_tx, &inspector.api_token_scope_buf, inspector.api_tokens_page);
                        }
                    });
                });

                ui.add_space(4.0);
            });
        });
}

/// Send a scoped, paginated token list request.
fn send_scoped_token_list(
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    scope: &str,
    page: u32,
) {
    if scope.is_empty() {
        db_tx.send(DbCommand::ListApiTokens {
            offset: page * API_TOKEN_PAGE_SIZE,
            limit: API_TOKEN_PAGE_SIZE,
        }).ok();
    } else {
        db_tx.send(DbCommand::ListApiTokensByScope {
            scope_prefix: scope.to_string(),
            offset: page * API_TOKEN_PAGE_SIZE,
            limit: API_TOKEN_PAGE_SIZE,
        }).ok();
    }
}

// viewport_overlay and related functions are defined in crate::viewport


// render_context_menu, render_create_dialog, apply_create, render_gltf_import_dialog
// are defined in crate::dialogs


// ---------------------------------------------------------------------------
// Tool enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
}

// render_invite_dialog, render_join_dialog, render_peer_debug_panel
// are defined in crate::dialogs
