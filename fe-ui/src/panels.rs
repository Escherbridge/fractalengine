use bevy_egui::egui;

use crate::atlas::DashboardState;
use crate::plugin::{
    CameraFocusTarget, ContextMenuState, CreateDialogState, CreateKind, GltfImportState,
    InspectorState, InviteDialogState, JoinDialogState, NavigationState, PeerDebugPanelState,
    SidebarState, ToolState, VerseHierarchy, ViewportCursorWorld,
};
use crate::theme;
use fe_runtime::messages::DbCommand;

// ---------------------------------------------------------------------------
// Top-level layout entry point
// ---------------------------------------------------------------------------

/// Renders the full UI shell: toolbar -> status bar -> sidebar -> inspector -> viewport.
pub fn gardener_console(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    tool: &mut ToolState,
    inspector: &mut InspectorState,
    nav: &mut NavigationState,
    dashboard: &DashboardState,
    hierarchy: &mut VerseHierarchy,
    create_dialog: &mut CreateDialogState,
    context_menu: &mut ContextMenuState,
    gltf_import: &mut GltfImportState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    camera_focus: &mut CameraFocusTarget,
    cursor_world: &ViewportCursorWorld,
    invite_dialog: &mut InviteDialogState,
    join_dialog: &mut JoinDialogState,
    sync_status: Option<&fe_sync::SyncStatus>,
    peer_debug: &mut PeerDebugPanelState,
) {
    top_toolbar(ctx, sidebar, tool, inspector);
    status_bar(ctx, dashboard, sync_status, nav, peer_debug);
    left_sidebar(
        ctx,
        sidebar,
        nav,
        dashboard,
        hierarchy,
        create_dialog,
        camera_focus,
        invite_dialog,
        join_dialog,
        db_tx,
    );
    right_inspector(ctx, inspector);

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            viewport_overlay(
                ui,
                nav,
                inspector,
                context_menu,
                hierarchy,
                create_dialog,
                db_tx,
                dashboard,
                cursor_world,
            );
        });

    // Floating dialogs / menus (rendered after panels so they layer on top)
    render_context_menu(ctx, context_menu, gltf_import);
    render_create_dialog(ctx, create_dialog, hierarchy, nav, db_tx);
    render_gltf_import_dialog(ctx, gltf_import, nav, db_tx);
    render_invite_dialog(ctx, invite_dialog);
    render_join_dialog(ctx, join_dialog, db_tx);
    render_peer_debug_panel(ctx, peer_debug, sync_status);
}

// ---------------------------------------------------------------------------
// Top toolbar
// ---------------------------------------------------------------------------

fn top_toolbar(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    tool: &mut ToolState,
    inspector: &mut InspectorState,
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

                if inspector.selected_entity.is_some()
                    && ui
                        .add(egui::Button::new("\u{2715} Deselect").fill(theme::BG_DANGER))
                        .clicked()
                {
                    inspector.selected_entity = None;
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
    nav: &NavigationState,
    peer_debug: &mut PeerDebugPanelState,
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
                    peer_debug.open = !peer_debug.open;
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
    nav: &mut NavigationState,
    dashboard: &DashboardState,
    hierarchy: &mut VerseHierarchy,
    create_dialog: &mut CreateDialogState,
    camera_focus: &mut CameraFocusTarget,
    invite_dialog: &mut InviteDialogState,
    join_dialog: &mut JoinDialogState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
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
            sidebar_verse_header(ui, nav, create_dialog, invite_dialog, join_dialog, db_tx);
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    render_verse_tree(ui, hierarchy, nav, create_dialog, camera_focus);
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
    nav: &NavigationState,
    create_dialog: &mut CreateDialogState,
    invite_dialog: &mut InviteDialogState,
    join_dialog: &mut JoinDialogState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
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
                create_dialog.open = true;
                create_dialog.kind = CreateKind::Verse;
                create_dialog.parent_id.clear();
                create_dialog.name_buf.clear();
            }
            // Phase F: Join Verse button
            if ui
                .add(egui::Button::new("Join").fill(theme::BG_BUTTON).small())
                .on_hover_text("Join a verse by invite")
                .clicked()
            {
                join_dialog.open = true;
                join_dialog.invite_buf.clear();
            }
        });
    });
    // Phase F: Generate Invite button (only when a verse is active)
    if nav.active_verse_id.is_some() {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if ui
                .add(
                    egui::Button::new("Generate Invite")
                        .fill(theme::BG_BUTTON)
                        .small(),
                )
                .on_hover_text("Create an invite link for this verse")
                .clicked()
            {
                if let Some(ref vid) = nav.active_verse_id {
                    let expiry = if invite_dialog.expiry_hours == 0 {
                        24
                    } else {
                        invite_dialog.expiry_hours
                    };
                    db_tx
                        .send(DbCommand::GenerateVerseInvite {
                            verse_id: vid.clone(),
                            include_write_cap: invite_dialog.include_write_cap,
                            expiry_hours: expiry,
                        })
                        .ok();
                }
            }
        });
    }
    ui.add_space(6.0);
}

fn render_verse_tree(
    ui: &mut egui::Ui,
    hierarchy: &mut VerseHierarchy,
    nav: &mut NavigationState,
    create_dialog: &mut CreateDialogState,
    camera_focus: &mut CameraFocusTarget,
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
                    create_dialog,
                    &verse_id,
                    camera_focus,
                );
                // [+] Add Fractal inside the verse collapse
                add_button_inline(
                    ui,
                    "Add Fractal",
                    CreateKind::Fractal,
                    &verse_id,
                    create_dialog,
                );
            });

        if resp.header_response.clicked() {
            nav.active_verse_id = Some(verse_id.clone());
            nav.active_verse_name = verse_name.clone();
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
    fractals: &mut [crate::plugin::FractalEntry],
    nav: &mut NavigationState,
    create_dialog: &mut CreateDialogState,
    verse_id: &str,
    camera_focus: &mut CameraFocusTarget,
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
                    create_dialog,
                    &fractal_id,
                    camera_focus,
                );
                // [+] Add Petal inside the fractal collapse
                add_button_inline(
                    ui,
                    "Add Petal",
                    CreateKind::Petal,
                    &fractal_id,
                    create_dialog,
                );
            });

        if resp.header_response.clicked() {
            nav.active_fractal_id = Some(fractal_id.clone());
            nav.active_fractal_name = fractal_name.clone();
        }
    }
}

#[allow(clippy::needless_range_loop)]
fn render_petals(
    ui: &mut egui::Ui,
    petals: &mut [crate::plugin::PetalEntry],
    nav: &mut NavigationState,
    create_dialog: &mut CreateDialogState,
    fractal_id: &str,
    camera_focus: &mut CameraFocusTarget,
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
                render_nodes(ui, &petals[pi].nodes, nav, camera_focus);
                // [+] Add Node inside the petal collapse
                add_button_inline(ui, "Add Node", CreateKind::Node, &petal_id, create_dialog);
            });

        if resp.header_response.clicked() {
            nav.active_petal_id = Some(petal_id.clone());
        }
    }
}

fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &[crate::plugin::NodeEntry],
    _nav: &mut NavigationState,
    camera_focus: &mut CameraFocusTarget,
) {
    for node in nodes {
        let resp = ui.horizontal(|ui| {
            let icon = if node.has_asset {
                "\u{25C6}" // filled diamond
            } else {
                "\u{25CF}" // filled circle
            };
            ui.label(
                egui::RichText::new(icon)
                    .small()
                    .color(theme::TREE_NODE_ICON),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&node.name)
                        .small()
                        .color(theme::TEXT_SECTION),
                )
                .sense(egui::Sense::click()),
            )
        });
        if resp.inner.clicked() {
            // Navigate camera to node position
            camera_focus.target = Some([node.position[0], node.position[1], node.position[2]]);
        }
    }
}

fn add_button_inline(
    ui: &mut egui::Ui,
    tooltip: &str,
    kind: CreateKind,
    parent_id: &str,
    create_dialog: &mut CreateDialogState,
) {
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new("+").fill(theme::BG_BUTTON_ALT).small())
            .on_hover_text(tooltip)
            .clicked()
        {
            create_dialog.open = true;
            create_dialog.kind = kind;
            create_dialog.parent_id = parent_id.to_string();
            create_dialog.name_buf.clear();
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

fn right_inspector(ctx: &egui::Context, inspector: &mut InspectorState) {
    let open = inspector.selected_entity.is_some();
    egui::SidePanel::right("inspector")
        .resizable(true)
        .default_width(260.0)
        .width_range(200.0..=400.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0)),
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
                                .fill(egui::Color32::TRANSPARENT) // intentionally transparent, not themed
                                .small(),
                        )
                        .clicked()
                    {
                        inspector.selected_entity = None;
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    inspector_entity_section(ui, inspector);
                    ui.add_space(2.0);
                    inspector_transform_section(ui, inspector);
                    ui.add_space(2.0);
                    inspector_url_meta_section(ui, inspector);
                });
        });
}

fn inspector_entity_section(ui: &mut egui::Ui, inspector: &InspectorState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Entity")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        if let Some(entity) = inspector.selected_entity {
            egui::Grid::new("entity_info")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("ID").small().color(theme::TEXT_DIM));
                    ui.label(
                        egui::RichText::new(format!("{:?}", entity))
                            .monospace()
                            .small(),
                    );
                    ui.end_row();
                });
        }
        ui.add_space(4.0);
    });
}

fn inspector_transform_section(ui: &mut egui::Ui, inspector: &mut InspectorState) {
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
                ui.add_sized(
                    [60.0, 16.0],
                    egui::Label::new(egui::RichText::new(label).small().color(theme::TEXT_DIM)),
                );
                for (axis, buf) in ["X", "Y", "Z"].iter().zip(bufs.iter_mut()) {
                    ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
                    ui.add(
                        egui::TextEdit::singleline(buf)
                            .desired_width(52.0)
                            .font(egui::TextStyle::Monospace),
                    );
                }
            });
        }
        ui.add_space(4.0);
    });
}

fn inspector_url_meta_section(ui: &mut egui::Ui, inspector: &mut InspectorState) {
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

        if inspector.is_admin {
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
            // TODO: emit UrlEditorSaved event via Bevy
        }
        ui.add_space(4.0);
    });
}

// ---------------------------------------------------------------------------
// Viewport overlay — navigation-state-aware
// ---------------------------------------------------------------------------

/// Renders the viewport overlay based on navigation depth:
/// - No verse → peer browser / verse discovery
/// - Verse selected → fractal list
/// - Fractal selected → petal list for navigation
/// - Petal selected → 3D space with object hints + right-click
fn viewport_overlay(
    ui: &mut egui::Ui,
    nav: &mut NavigationState,
    inspector: &InspectorState,
    context_menu: &mut ContextMenuState,
    hierarchy: &VerseHierarchy,
    create_dialog: &mut CreateDialogState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    dashboard: &DashboardState,
    cursor_world: &ViewportCursorWorld,
) {
    let rect = ui.available_rect_before_wrap();
    let center = rect.center();

    if nav.active_petal_id.is_some() {
        // === PETAL SELECTED: Full 3D space ===
        viewport_petal_space(
            ui,
            nav,
            inspector,
            context_menu,
            hierarchy,
            rect,
            center,
            cursor_world,
        );
    } else if nav.active_fractal_id.is_some() {
        // === FRACTAL SELECTED: Show petal cards ===
        viewport_petal_browser(ui, nav, hierarchy, create_dialog, db_tx, rect, center);
    } else if nav.active_verse_id.is_some() {
        // === VERSE SELECTED: Show fractal cards ===
        viewport_fractal_browser(ui, nav, hierarchy, create_dialog, db_tx, rect, center);
    } else {
        // === NO VERSE: Peer discovery / verse browser ===
        viewport_verse_browser(
            ui,
            nav,
            hierarchy,
            create_dialog,
            db_tx,
            dashboard,
            rect,
            center,
        );
    }
}

/// Petal selected — full 3D space with tool hints, breadcrumb, right-click.
fn viewport_petal_space(
    ui: &mut egui::Ui,
    nav: &mut NavigationState,
    inspector: &InspectorState,
    context_menu: &mut ContextMenuState,
    hierarchy: &VerseHierarchy,
    rect: egui::Rect,
    center: egui::Pos2,
    cursor_world: &ViewportCursorWorld,
) {
    // Breadcrumb at top of viewport
    let petal_name = find_petal_name(hierarchy, nav.active_petal_id.as_deref().unwrap_or(""));
    let breadcrumb = format!(
        "{} / {} / {}",
        nav.active_verse_name, nav.active_fractal_name, petal_name
    );
    ui.painter().text(
        egui::pos2(center.x, rect.min.y + 16.0),
        egui::Align2::CENTER_CENTER,
        breadcrumb,
        egui::FontId::proportional(11.0),
        theme::TEXT_DIM,
    );

    // Tool hints at bottom
    if inspector.selected_entity.is_none() {
        ui.painter().text(
            egui::pos2(center.x, rect.max.y - 40.0),
            egui::Align2::CENTER_CENTER,
            "Click an object to select it  \u{2022}  S = Select  G = Move  R = Rotate",
            egui::FontId::proportional(12.0),
            theme::TEXT_VIEWPORT_HINT,
        );
    }

    // Use a child ui for the back button so it has proper layout
    let mut back_clicked = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 8.0, rect.min.y + 4.0),
            egui::vec2(80.0, 24.0),
        )),
        |ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("\u{25C0} Back").small())
                        .fill(theme::BG_BUTTON),
                )
                .clicked()
            {
                back_clicked = true;
            }
        },
    );
    if back_clicked {
        nav.active_petal_id = None;
    }

    // Right-click for context menu — use input directly to avoid stealing clicks
    if ui.input(|i| i.pointer.secondary_clicked()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if rect.contains(pos) {
                context_menu.open = true;
                context_menu.screen_pos = [pos.x, pos.y];
                context_menu.world_pos = cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]);
            }
        }
    }
}

/// No verse selected — verse browser + peer discovery.
fn viewport_verse_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationState,
    hierarchy: &VerseHierarchy,
    create_dialog: &mut CreateDialogState,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    dashboard: &DashboardState,
    rect: egui::Rect,
    center: egui::Pos2,
) {
    // Title
    ui.painter().text(
        egui::pos2(center.x, rect.min.y + 30.0),
        egui::Align2::CENTER_CENTER,
        "FractalEngine",
        egui::FontId::proportional(24.0),
        theme::TEXT_STRONG,
    );
    ui.painter().text(
        egui::pos2(center.x, rect.min.y + 55.0),
        egui::Align2::CENTER_CENTER,
        "Select a verse to enter, or discover peers to join",
        egui::FontId::proportional(12.0),
        theme::TEXT_MUTED,
    );

    // --- Your Verses section ---
    ui.painter().text(
        egui::pos2(rect.min.x + 20.0, rect.min.y + 85.0),
        egui::Align2::LEFT_CENTER,
        "Your Verses",
        egui::FontId::proportional(14.0),
        theme::TEXT_SECTION,
    );

    let card_w = 200.0_f32;
    let card_h = 70.0_f32;
    let gap = 12.0_f32;
    let start_y = rect.min.y + 105.0;
    let cards_per_row = ((rect.width() - 40.0) / (card_w + gap)).floor().max(1.0) as usize;

    for (i, verse) in hierarchy.verses.iter().enumerate() {
        let col = i % cards_per_row;
        let row = i / cards_per_row;
        let x = rect.min.x + 20.0 + col as f32 * (card_w + gap);
        let y = start_y + row as f32 * (card_h + gap);
        let card_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(card_w, card_h));

        ui.scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
            let resp = ui.add_sized(
                [card_w, card_h],
                egui::Button::new(
                    egui::RichText::new(&verse.name)
                        .strong()
                        .size(13.0)
                        .color(theme::TEXT_BRIGHT),
                )
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM))
                .corner_radius(6.0),
            );
            if resp.clicked() {
                nav.active_verse_id = Some(verse.id.clone());
                nav.active_verse_name = verse.name.clone();
            }
        });

        ui.painter().text(
            egui::pos2(x + card_w / 2.0, y + card_h - 12.0),
            egui::Align2::CENTER_CENTER,
            format!("{} fractals", verse.fractals.len()),
            egui::FontId::proportional(10.0),
            theme::TEXT_MUTED,
        );
    }

    // [+ New Verse] button
    let new_verse_row = hierarchy.verses.len() / cards_per_row;
    let new_verse_col = hierarchy.verses.len() % cards_per_row;
    let new_x = rect.min.x + 20.0 + new_verse_col as f32 * (card_w + gap);
    let new_y = start_y + new_verse_row as f32 * (card_h + gap);
    let new_rect = egui::Rect::from_min_size(egui::pos2(new_x, new_y), egui::vec2(card_w, card_h));

    ui.scope_builder(egui::UiBuilder::new().max_rect(new_rect), |ui| {
        let resp = ui.add_sized(
            [card_w, card_h],
            egui::Button::new(
                egui::RichText::new("+ New Verse")
                    .size(13.0)
                    .color(theme::TEXT_DIM),
            )
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, theme::BG_BUTTON_ALT))
            .corner_radius(6.0),
        );
        if resp.clicked() {
            create_dialog.open = true;
            create_dialog.kind = CreateKind::Verse;
            create_dialog.parent_id.clear();
            create_dialog.name_buf.clear();
        }
    });

    // --- Peer Discovery section ---
    let peer_section_y =
        start_y + ((hierarchy.verses.len() / cards_per_row + 1) as f32) * (card_h + gap) + 20.0;

    ui.painter().text(
        egui::pos2(rect.min.x + 20.0, peer_section_y),
        egui::Align2::LEFT_CENTER,
        "Peer Discovery",
        egui::FontId::proportional(14.0),
        theme::TEXT_SECTION,
    );

    let peer_box_rect = egui::Rect::from_min_size(
        egui::pos2(rect.min.x + 20.0, peer_section_y + 20.0),
        egui::vec2(rect.width() - 40.0, 80.0),
    );

    ui.painter().rect(
        peer_box_rect,
        6.0,
        theme::BG_PANEL,
        egui::Stroke::new(1.0, theme::BG_BUTTON_ALT),
        egui::StrokeKind::Outside,
    );

    if dashboard.peer_count > 0 {
        ui.painter().text(
            peer_box_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!(
                "{} peers online — browse their open verses",
                dashboard.peer_count
            ),
            egui::FontId::proportional(12.0),
            theme::TEXT_SECTION,
        );
    } else {
        ui.painter().text(
            egui::pos2(peer_box_rect.center().x, peer_box_rect.center().y - 8.0),
            egui::Align2::CENTER_CENTER,
            "No peers connected",
            egui::FontId::proportional(13.0),
            theme::TEXT_MUTED,
        );
        ui.painter().text(
            egui::pos2(peer_box_rect.center().x, peer_box_rect.center().y + 12.0),
            egui::Align2::CENTER_CENTER,
            "Connect to the network to discover and join other verses",
            egui::FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    }
}

/// Verse selected — browse fractals within it.
fn viewport_fractal_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationState,
    hierarchy: &VerseHierarchy,
    create_dialog: &mut CreateDialogState,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    rect: egui::Rect,
    center: egui::Pos2,
) {
    // Back button
    let mut back_clicked = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 8.0, rect.min.y + 4.0),
            egui::vec2(80.0, 24.0),
        )),
        |ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("\u{25C0} Verses").small())
                        .fill(theme::BG_BUTTON),
                )
                .clicked()
            {
                back_clicked = true;
            }
        },
    );
    if back_clicked {
        nav.active_verse_id = None;
        nav.active_verse_name.clear();
        nav.active_fractal_id = None;
        nav.active_fractal_name.clear();
    }

    ui.painter().text(
        egui::pos2(center.x, rect.min.y + 35.0),
        egui::Align2::CENTER_CENTER,
        format!("{} — Select a Fractal", nav.active_verse_name),
        egui::FontId::proportional(18.0),
        theme::TEXT_STRONG,
    );

    let verse = hierarchy
        .verses
        .iter()
        .find(|v| nav.active_verse_id.as_deref() == Some(&v.id));

    if let Some(verse) = verse {
        let card_w = 200.0_f32;
        let card_h = 70.0_f32;
        let gap = 12.0_f32;
        let start_y = rect.min.y + 65.0;
        let cards_per_row = ((rect.width() - 40.0) / (card_w + gap)).floor().max(1.0) as usize;

        for (i, fractal) in verse.fractals.iter().enumerate() {
            let col = i % cards_per_row;
            let row = i / cards_per_row;
            let x = rect.min.x + 20.0 + col as f32 * (card_w + gap);
            let y = start_y + row as f32 * (card_h + gap);
            let card_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(card_w, card_h));

            ui.scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
                let resp = ui.add_sized(
                    [card_w, card_h],
                    egui::Button::new(
                        egui::RichText::new(&fractal.name)
                            .strong()
                            .size(13.0)
                            .color(theme::TEXT_BRIGHT),
                    )
                    .fill(theme::BG_PANEL)
                    .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM))
                    .corner_radius(6.0),
                );
                if resp.clicked() {
                    nav.active_fractal_id = Some(fractal.id.clone());
                    nav.active_fractal_name = fractal.name.clone();
                }
            });

            ui.painter().text(
                egui::pos2(x + card_w / 2.0, y + card_h - 12.0),
                egui::Align2::CENTER_CENTER,
                format!("{} petals", fractal.petals.len()),
                egui::FontId::proportional(10.0),
                theme::TEXT_MUTED,
            );
        }

        // [+ New Fractal] button
        let new_row = verse.fractals.len() / cards_per_row;
        let new_col = verse.fractals.len() % cards_per_row;
        let new_x = rect.min.x + 20.0 + new_col as f32 * (card_w + gap);
        let new_y = start_y + new_row as f32 * (card_h + gap);
        let new_rect =
            egui::Rect::from_min_size(egui::pos2(new_x, new_y), egui::vec2(card_w, card_h));

        ui.scope_builder(egui::UiBuilder::new().max_rect(new_rect), |ui| {
            let resp = ui.add_sized(
                [card_w, card_h],
                egui::Button::new(
                    egui::RichText::new("+ New Fractal")
                        .size(13.0)
                        .color(theme::TEXT_DIM),
                )
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(1.0, theme::BG_BUTTON_ALT))
                .corner_radius(6.0),
            );
            if resp.clicked() {
                create_dialog.open = true;
                create_dialog.kind = CreateKind::Fractal;
                create_dialog.parent_id = verse.id.clone();
                create_dialog.name_buf.clear();
            }
        });

        if verse.fractals.is_empty() {
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                "No fractals yet — click + New Fractal to create one",
                egui::FontId::proportional(14.0),
                theme::TEXT_MUTED,
            );
        }
    }
}

/// Fractal selected — browse petals, select one to enter the 3D room.
fn viewport_petal_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationState,
    hierarchy: &VerseHierarchy,
    create_dialog: &mut CreateDialogState,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    rect: egui::Rect,
    center: egui::Pos2,
) {
    // Back button
    let mut back_clicked = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 8.0, rect.min.y + 4.0),
            egui::vec2(100.0, 24.0),
        )),
        |ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("\u{25C0} Fractals").small())
                        .fill(theme::BG_BUTTON),
                )
                .clicked()
            {
                back_clicked = true;
            }
        },
    );
    if back_clicked {
        nav.active_fractal_id = None;
        nav.active_fractal_name.clear();
    }

    ui.painter().text(
        egui::pos2(center.x, rect.min.y + 35.0),
        egui::Align2::CENTER_CENTER,
        format!(
            "{} / {} — Select a Petal to enter",
            nav.active_verse_name, nav.active_fractal_name
        ),
        egui::FontId::proportional(18.0),
        theme::TEXT_STRONG,
    );
    ui.painter().text(
        egui::pos2(center.x, rect.min.y + 55.0),
        egui::Align2::CENTER_CENTER,
        "Each petal is a room where 3D objects live",
        egui::FontId::proportional(12.0),
        theme::TEXT_MUTED,
    );

    let fractal_data = hierarchy
        .verses
        .iter()
        .find(|v| nav.active_verse_id.as_deref() == Some(&v.id))
        .and_then(|v| {
            v.fractals
                .iter()
                .find(|f| nav.active_fractal_id.as_deref() == Some(&f.id))
        });

    if let Some(fractal) = fractal_data {
        let card_w = 180.0_f32;
        let card_h = 80.0_f32;
        let gap = 12.0_f32;
        let start_y = rect.min.y + 80.0;
        let cards_per_row = ((rect.width() - 40.0) / (card_w + gap)).floor().max(1.0) as usize;

        for (i, petal) in fractal.petals.iter().enumerate() {
            let col = i % cards_per_row;
            let row = i / cards_per_row;
            let x = rect.min.x + 20.0 + col as f32 * (card_w + gap);
            let y = start_y + row as f32 * (card_h + gap);
            let card_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(card_w, card_h));

            ui.scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
                let resp = ui.add_sized(
                    [card_w, card_h],
                    egui::Button::new(
                        egui::RichText::new(&petal.name)
                            .strong()
                            .size(13.0)
                            .color(theme::TEXT_BRIGHT),
                    )
                    .fill(theme::BG_PANEL)
                    .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM))
                    .corner_radius(6.0),
                );
                if resp.clicked() {
                    nav.active_petal_id = Some(petal.id.clone());
                }
            });

            ui.painter().text(
                egui::pos2(x + card_w / 2.0, y + card_h - 12.0),
                egui::Align2::CENTER_CENTER,
                format!("{} nodes", petal.nodes.len()),
                egui::FontId::proportional(10.0),
                theme::TEXT_MUTED,
            );
        }

        // [+ New Petal] button
        let fractal_id = fractal.id.clone();
        let new_row = fractal.petals.len() / cards_per_row;
        let new_col = fractal.petals.len() % cards_per_row;
        let new_x = rect.min.x + 20.0 + new_col as f32 * (card_w + gap);
        let new_y = start_y + new_row as f32 * (card_h + gap);
        let new_rect =
            egui::Rect::from_min_size(egui::pos2(new_x, new_y), egui::vec2(card_w, card_h));

        ui.scope_builder(egui::UiBuilder::new().max_rect(new_rect), |ui| {
            let resp = ui.add_sized(
                [card_w, card_h],
                egui::Button::new(
                    egui::RichText::new("+ New Petal")
                        .size(13.0)
                        .color(theme::TEXT_DIM),
                )
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(1.0, theme::BG_BUTTON_ALT))
                .corner_radius(6.0),
            );
            if resp.clicked() {
                create_dialog.open = true;
                create_dialog.kind = CreateKind::Petal;
                create_dialog.parent_id = fractal_id;
                create_dialog.name_buf.clear();
            }
        });

        if fractal.petals.is_empty() {
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                "No petals yet — click + New Petal to create a room",
                egui::FontId::proportional(14.0),
                theme::TEXT_MUTED,
            );
        }
    }
}

/// Find petal name by ID in the hierarchy.
fn find_petal_name(hierarchy: &VerseHierarchy, petal_id: &str) -> String {
    for verse in &hierarchy.verses {
        for fractal in &verse.fractals {
            for petal in &fractal.petals {
                if petal.id == petal_id {
                    return petal.name.clone();
                }
            }
        }
    }
    "Unknown".to_string()
}

// ---------------------------------------------------------------------------
// Context menu (viewport right-click)
// ---------------------------------------------------------------------------

fn render_context_menu(
    ctx: &egui::Context,
    context_menu: &mut ContextMenuState,
    gltf_import: &mut GltfImportState,
) {
    if !context_menu.open {
        return;
    }

    let pos = egui::pos2(context_menu.screen_pos[0], context_menu.screen_pos[1]);
    let world = context_menu.world_pos;

    egui::Area::new(egui::Id::new("viewport_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::BG_CONTEXT_MENU)
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM))
                .show(ui, |ui| {
                    ui.set_min_width(160.0);

                    if ui
                        .add(egui::Button::new("Add GLTF Model").fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        gltf_import.open = true;
                        gltf_import.position = world;
                        gltf_import.file_path_buf.clear();
                        gltf_import.name_buf.clear();
                        context_menu.open = false;
                    }

                    if ui
                        .add(egui::Button::new("Add Empty Node").fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        // TODO: create empty node at world position
                        context_menu.open = false;
                    }
                });
        });

    // Close on click elsewhere
    if ctx.input(|i| i.pointer.any_pressed()) {
        let ptr = ctx.input(|i| i.pointer.interact_pos());
        if let Some(ptr_pos) = ptr {
            let menu_rect = egui::Rect::from_min_size(pos, egui::vec2(160.0, 80.0));
            if !menu_rect.contains(ptr_pos) {
                context_menu.open = false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Create dialog (Verse / Fractal / Petal / Node)
// ---------------------------------------------------------------------------

fn render_create_dialog(
    ctx: &egui::Context,
    create_dialog: &mut CreateDialogState,
    hierarchy: &mut VerseHierarchy,
    nav: &mut NavigationState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    if !create_dialog.open {
        return;
    }

    let title = match create_dialog.kind {
        CreateKind::Verse => "Create Verse",
        CreateKind::Fractal => "Create Fractal",
        CreateKind::Petal => "Create Petal",
        CreateKind::Node => "Create Node",
    };

    let mut still_open = true;
    egui::Window::new(title)
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(280.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Name:").small().color(theme::TEXT_DIM));
            ui.add(
                egui::TextEdit::singleline(&mut create_dialog.name_buf)
                    .hint_text("Enter name\u{2026}")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Create").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let name = create_dialog.name_buf.trim().to_string();
                    if !name.is_empty() {
                        apply_create(
                            hierarchy,
                            nav,
                            create_dialog.kind,
                            &create_dialog.parent_id,
                            &name,
                            db_tx,
                        );
                        create_dialog.open = false;
                        create_dialog.name_buf.clear();
                    }
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    create_dialog.open = false;
                    create_dialog.name_buf.clear();
                }
            });
        });

    if !still_open {
        create_dialog.open = false;
    }
}

/// Send a DB command to create the entity. The hierarchy will update when
/// the DbResult comes back — no local push to avoid duplicates.
fn apply_create(
    _hierarchy: &mut VerseHierarchy,
    _nav: &mut NavigationState,
    kind: CreateKind,
    parent_id: &str,
    name: &str,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    match kind {
        CreateKind::Verse => {
            db_tx
                .send(DbCommand::CreateVerse {
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Fractal => {
            db_tx
                .send(DbCommand::CreateFractal {
                    verse_id: parent_id.to_string(),
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Petal => {
            db_tx
                .send(DbCommand::CreatePetal {
                    fractal_id: parent_id.to_string(),
                    name: name.to_string(),
                })
                .ok();
        }
        CreateKind::Node => {
            db_tx
                .send(DbCommand::CreateNode {
                    petal_id: parent_id.to_string(),
                    name: name.to_string(),
                    position: [0.0, 0.0, 0.0],
                })
                .ok();
        }
    }
}

// ---------------------------------------------------------------------------
// GLTF import dialog
// ---------------------------------------------------------------------------

fn render_gltf_import_dialog(
    ctx: &egui::Context,
    gltf_import: &mut GltfImportState,
    nav: &NavigationState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    if !gltf_import.open {
        return;
    }

    let mut still_open = true;
    egui::Window::new("Import GLTF Model")
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .default_width(340.0)
        .max_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("File Path:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut gltf_import.file_path_buf)
                        .hint_text("path/to/model.glb")
                        .desired_width(ui.available_width() - 70.0),
                );
                if ui
                    .add(egui::Button::new("Browse...").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("GLTF Models", &["glb", "gltf"])
                        .add_filter("All Files", &["*"])
                        .pick_file()
                    {
                        gltf_import.file_path_buf = path.display().to_string();
                        if gltf_import.name_buf.is_empty() {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                gltf_import.name_buf = stem.to_string();
                            }
                        }
                    }
                }
            });

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Display Name:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut gltf_import.name_buf)
                    .hint_text("My Model")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Position:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                for (axis, val) in ["X", "Y", "Z"].iter().zip(gltf_import.position.iter()) {
                    ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
                    ui.label(
                        egui::RichText::new(format!("{:.2}", val))
                            .monospace()
                            .small(),
                    );
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Import").fill(theme::BG_SAVE))
                    .clicked()
                {
                    let file_path = gltf_import.file_path_buf.trim().to_string();
                    let name = gltf_import.name_buf.trim().to_string();
                    let petal_id = nav.active_petal_id.clone().unwrap_or_default();
                    if !file_path.is_empty() && !petal_id.is_empty() {
                        db_tx
                            .send(DbCommand::ImportGltf {
                                petal_id,
                                name: if name.is_empty() {
                                    "Imported Model".to_string()
                                } else {
                                    name
                                },
                                file_path,
                                position: gltf_import.position,
                            })
                            .ok();
                    }
                    gltf_import.open = false;
                }
                if ui
                    .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
                    .clicked()
                {
                    gltf_import.open = false;
                }
            });
        });

    if !still_open {
        gltf_import.open = false;
    }
}

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

// ---------------------------------------------------------------------------
// Phase F: Invite dialog
// ---------------------------------------------------------------------------

fn render_invite_dialog(ctx: &egui::Context, invite_dialog: &mut InviteDialogState) {
    if !invite_dialog.open {
        return;
    }

    egui::Window::new("Verse Invite")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Share this invite string with others to let them join your verse:");
            ui.add_space(4.0);

            // Copyable text field with the invite string
            let mut invite_str = invite_dialog.invite_string.clone();
            ui.add(
                egui::TextEdit::multiline(&mut invite_str)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Copy to Clipboard").clicked() {
                    ui.ctx().copy_text(invite_dialog.invite_string.clone());
                }
                if ui.button("Close").clicked() {
                    invite_dialog.open = false;
                    invite_dialog.invite_string.clear();
                }
            });
        });
}

// ---------------------------------------------------------------------------
// Phase F: Join dialog
// ---------------------------------------------------------------------------

fn render_join_dialog(
    ctx: &egui::Context,
    join_dialog: &mut JoinDialogState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    if !join_dialog.open {
        return;
    }

    egui::Window::new("Join Verse")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Paste an invite string to join a verse:");
            ui.add_space(4.0);

            ui.add(
                egui::TextEdit::multiline(&mut join_dialog.invite_buf)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste invite string here...")
                    .font(egui::TextStyle::Monospace),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let can_join = !join_dialog.invite_buf.trim().is_empty();
                if ui
                    .add_enabled(can_join, egui::Button::new("Join"))
                    .clicked()
                {
                    db_tx
                        .send(DbCommand::JoinVerseByInvite {
                            invite_string: join_dialog.invite_buf.trim().to_string(),
                        })
                        .ok();
                    join_dialog.open = false;
                    join_dialog.invite_buf.clear();
                }
                if ui.button("Cancel").clicked() {
                    join_dialog.open = false;
                    join_dialog.invite_buf.clear();
                }
            });
        });
}

// ---------------------------------------------------------------------------
// Phase F: Peer debug panel
// ---------------------------------------------------------------------------

fn render_peer_debug_panel(
    ctx: &egui::Context,
    peer_debug: &mut PeerDebugPanelState,
    sync_status: Option<&fe_sync::SyncStatus>,
) {
    if !peer_debug.open {
        return;
    }

    egui::Window::new("Peer Debug")
        .collapsible(true)
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            if let Some(status) = sync_status {
                ui.label(format!(
                    "Online: {}",
                    if status.online { "Yes" } else { "No" }
                ));
                ui.label(format!("Peer count: {}", status.peer_count));
                if let Some(ref addr) = status.node_addr {
                    ui.add_space(4.0);
                    ui.label("Node address:");
                    ui.monospace(addr);
                }
            } else {
                ui.label("Sync status not available");
            }

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Peer list will be populated when gossip discovery is active.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );

            ui.add_space(4.0);
            if ui.button("Close").clicked() {
                peer_debug.open = false;
            }
        });
}
