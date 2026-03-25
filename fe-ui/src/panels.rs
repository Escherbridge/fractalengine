use bevy_egui::egui;

use crate::atlas::DashboardState;
use crate::plugin::UiState;

// ---------------------------------------------------------------------------
// Top-level layout entry point
// ---------------------------------------------------------------------------

/// Renders the full UI shell: toolbar → status bar → sidebar → inspector → viewport.
/// The Bevy 3D scene renders into the CentralPanel area automatically.
pub fn gardener_console(ctx: &egui::Context, state: &mut UiState, dashboard: &DashboardState) {
    top_toolbar(ctx, state);
    status_bar(ctx, dashboard);
    left_sidebar(ctx, state, dashboard);
    right_inspector(ctx, state);

    // Central panel = Bevy 3D viewport (keep transparent / no frame)
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            viewport_hints(ui, state);
        });
}

// ---------------------------------------------------------------------------
// Top toolbar
// ---------------------------------------------------------------------------

fn top_toolbar(ctx: &egui::Context, state: &mut UiState) {
    egui::TopBottomPanel::top("toolbar")
        .exact_height(40.0)
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(22, 22, 28))
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // Sidebar toggle
                let sidebar_icon = if state.sidebar_open { "◀ Hide" } else { "▶ Show" };
                if ui
                    .add(
                        egui::Button::new(sidebar_icon)
                            .fill(egui::Color32::from_rgb(40, 40, 52)),
                    )
                    .clicked()
                {
                    state.sidebar_open = !state.sidebar_open;
                }

                ui.separator();

                // Tool buttons
                for (tool, label, tooltip) in [
                    (Tool::Select, "⬚ Select", "Select objects (S)"),
                    (Tool::Move, "✛ Move", "Move selected object (G)"),
                    (Tool::Rotate, "↻ Rotate", "Rotate selected object (R)"),
                    (Tool::Scale, "⤡ Scale", "Scale selected object (X)"),
                ] {
                    let active = state.active_tool == tool;
                    let btn = egui::Button::new(label).fill(if active {
                        egui::Color32::from_rgb(60, 100, 160)
                    } else {
                        egui::Color32::from_rgb(40, 40, 52)
                    });
                    if ui.add(btn).on_hover_text(tooltip).clicked() {
                        state.active_tool = tool;
                    }
                }

                ui.separator();

                // Deselect
                if state.selected_entity.is_some()
                    && ui
                        .add(
                            egui::Button::new("✕ Deselect")
                                .fill(egui::Color32::from_rgb(80, 40, 40)),
                        )
                        .clicked()
                {
                    state.selected_entity = None;
                }

                // Right-align: space name
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("FractalEngine")
                            .color(egui::Color32::from_rgb(120, 120, 140))
                            .small(),
                    );
                });
            });
        });
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn status_bar(ctx: &egui::Context, dashboard: &DashboardState) {
    egui::TopBottomPanel::bottom("statusbar")
        .exact_height(22.0)
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(18, 18, 24))
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if dashboard.is_online {
                    ui.colored_label(egui::Color32::GREEN, "●");
                    ui.label(
                        egui::RichText::new("Online")
                            .small()
                            .color(egui::Color32::from_rgb(100, 180, 100)),
                    );
                } else {
                    ui.colored_label(egui::Color32::from_rgb(180, 60, 60), "●");
                    ui.label(
                        egui::RichText::new("Offline")
                            .small()
                            .color(egui::Color32::from_rgb(180, 100, 100)),
                    );
                }

                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{} peers", dashboard.peer_count))
                        .small()
                        .color(egui::Color32::from_rgb(120, 120, 140)),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{} petals  {} rooms  {} models",
                        dashboard.petal_count, dashboard.room_count, dashboard.model_count
                    ))
                    .small()
                    .color(egui::Color32::from_rgb(100, 100, 120)),
                );
            });
        });
}

// ---------------------------------------------------------------------------
// Left sidebar
// ---------------------------------------------------------------------------

fn left_sidebar(ctx: &egui::Context, state: &mut UiState, dashboard: &DashboardState) {
    egui::SidePanel::left("sidebar")
        .resizable(true)
        .default_width(240.0)
        .width_range(180.0..=400.0)
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(24, 24, 30))
                .inner_margin(egui::Margin::same(0)),
        )
        .show_animated(ctx, state.sidebar_open, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    sidebar_section_space_overview(ui, dashboard, state);
                    ui.add_space(2.0);
                    sidebar_section_petals(ui);
                    ui.add_space(2.0);
                    sidebar_section_peers(ui, dashboard);
                    ui.add_space(2.0);
                    sidebar_section_roles(ui);
                    ui.add_space(2.0);
                    sidebar_section_assets(ui);
                    ui.add_space(4.0);
                });
        });
}


fn sidebar_section_space_overview(ui: &mut egui::Ui, dashboard: &DashboardState, state: &mut UiState) {
    let header = egui::CollapsingHeader::new(
        egui::RichText::new("Space Overview")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        egui::Grid::new("space_stats")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Petals").color(egui::Color32::from_rgb(120, 120, 140)).small());
                ui.label(egui::RichText::new(dashboard.petal_count.to_string()).strong());
                ui.end_row();
                ui.label(egui::RichText::new("Rooms").color(egui::Color32::from_rgb(120, 120, 140)).small());
                ui.label(egui::RichText::new(dashboard.room_count.to_string()).strong());
                ui.end_row();
                ui.label(egui::RichText::new("Models").color(egui::Color32::from_rgb(120, 120, 140)).small());
                ui.label(egui::RichText::new(dashboard.model_count.to_string()).strong());
                ui.end_row();
                ui.label(egui::RichText::new("Peers").color(egui::Color32::from_rgb(120, 120, 140)).small());
                ui.label(egui::RichText::new(dashboard.peer_count.to_string()).strong());
                ui.end_row();
            });

        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Tag Filter")
                .small()
                .color(egui::Color32::from_rgb(120, 120, 140)),
        );
        ui.add(
            egui::TextEdit::singleline(&mut state.tag_filter_buf)
                .hint_text("filter by tag…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
    });
    let _ = header;
}

fn sidebar_section_petals(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Petals")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("No petals loaded")
                .italics()
                .color(egui::Color32::from_rgb(100, 100, 120)),
        );
        ui.add_space(4.0);
        if ui
            .add(
                egui::Button::new("+ Load Petal")
                    .fill(egui::Color32::from_rgb(40, 70, 40))
                    .min_size(egui::vec2(ui.available_width(), 24.0)),
            )
            .clicked()
        {
            // TODO: open petal load dialog
        }
        ui.add_space(4.0);
    });
}

fn sidebar_section_peers(ui: &mut egui::Ui, dashboard: &DashboardState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Peers")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);
        if dashboard.peer_count == 0 {
            ui.label(
                egui::RichText::new("No peers connected")
                    .italics()
                    .color(egui::Color32::from_rgb(100, 100, 120)),
            );
        } else {
            ui.label(format!("{} peers connected", dashboard.peer_count));
        }
        ui.add_space(4.0);
    });
}

fn sidebar_section_roles(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Roles")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Custom Roles")
                .small()
                .color(egui::Color32::from_rgb(100, 100, 120)),
        );
        if ui
            .add(
                egui::Button::new("+ New Role")
                    .fill(egui::Color32::from_rgb(40, 40, 60))
                    .min_size(egui::vec2(ui.available_width(), 24.0)),
            )
            .clicked()
        {
            // TODO: open role editor
        }
        ui.add_space(4.0);
    });
}

fn sidebar_section_assets(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Assets")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("No assets (BLAKE3-addressed)")
                .italics()
                .small()
                .color(egui::Color32::from_rgb(100, 100, 120)),
        );
        ui.add_space(4.0);
    });
}

// ---------------------------------------------------------------------------
// Right inspector panel (slides in when an entity is selected)
// ---------------------------------------------------------------------------

fn right_inspector(ctx: &egui::Context, state: &mut UiState) {
    let open = state.selected_entity.is_some();
    egui::SidePanel::right("inspector")
        .resizable(true)
        .default_width(260.0)
        .width_range(200.0..=400.0)
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(24, 24, 30))
                .inner_margin(egui::Margin::same(0)),
        )
        .show_animated(ctx, open, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Inspector")
                        .strong()
                        .color(egui::Color32::from_rgb(180, 180, 210)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Button::new("✕")
                                .fill(egui::Color32::TRANSPARENT)
                                .small(),
                        )
                        .clicked()
                    {
                        state.selected_entity = None;
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    inspector_entity_section(ui, state);
                    ui.add_space(2.0);
                    inspector_transform_section(ui, state);
                    ui.add_space(2.0);
                    inspector_url_meta_section(ui, state);
                });
        });
}

fn inspector_entity_section(ui: &mut egui::Ui, state: &UiState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Entity")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        if let Some(entity) = state.selected_entity {
            egui::Grid::new("entity_info")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("ID").small().color(egui::Color32::from_rgb(120, 120, 140)));
                    ui.label(egui::RichText::new(format!("{:?}", entity)).monospace().small());
                    ui.end_row();
                });
        }
        ui.add_space(4.0);
    });
}

fn inspector_transform_section(ui: &mut egui::Ui, state: &mut UiState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Transform")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        let axis_color = egui::Color32::from_rgb(160, 100, 100);
        for (label, bufs) in [
            ("Position", &mut state.inspector_pos),
            ("Rotation", &mut state.inspector_rot),
            ("Scale",    &mut state.inspector_scale),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [60.0, 16.0],
                    egui::Label::new(
                        egui::RichText::new(label)
                            .small()
                            .color(egui::Color32::from_rgb(120, 120, 140)),
                    ),
                );
                for (axis, buf) in ["X", "Y", "Z"].iter().zip(bufs.iter_mut()) {
                    ui.label(egui::RichText::new(*axis).small().color(axis_color));
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

fn inspector_url_meta_section(ui: &mut egui::Ui, state: &mut UiState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Portal URLs")
            .strong()
            .color(egui::Color32::from_rgb(160, 160, 190)),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);

        ui.label(egui::RichText::new("External URL").small().color(egui::Color32::from_rgb(120, 120, 140)));
        ui.add(
            egui::TextEdit::singleline(&mut state.inspector_external_url)
                .hint_text("https://…")
                .desired_width(f32::INFINITY),
        );

        if state.is_admin {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Config URL (admin)").small().color(egui::Color32::from_rgb(120, 120, 140)));
            ui.add(
                egui::TextEdit::singleline(&mut state.inspector_config_url)
                    .hint_text("https://admin.…")
                    .desired_width(f32::INFINITY),
            );
        }

        ui.add_space(6.0);
        if ui
            .add(
                egui::Button::new("💾 Save")
                    .fill(egui::Color32::from_rgb(40, 80, 40))
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
// Viewport overlay — hints rendered in CentralPanel when nothing is selected
// ---------------------------------------------------------------------------

fn viewport_hints(ui: &mut egui::Ui, state: &UiState) {
    if state.selected_entity.is_none() {
        let rect = ui.available_rect_before_wrap();
        let center = rect.center();

        ui.painter().text(
            egui::pos2(center.x, rect.max.y - 40.0),
            egui::Align2::CENTER_CENTER,
            "Click an object to select it  •  S = Select  G = Move  R = Rotate",
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgba_unmultiplied(180, 180, 200, 80),
        );
    }
}

// ---------------------------------------------------------------------------
// Tool enum (kept here, re-exported through plugin)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
}

// Keep old gardener_console signature available for any direct callers.
// The real entry point is the one that takes UiState + DashboardState.
pub fn space_overview_panel(ctx: &egui::Context, state: &DashboardState, is_admin: bool) {
    // Legacy standalone window — retained so existing call sites compile.
    egui::Window::new("Space Overview").show(ctx, |ui| {
        if state.is_online {
            ui.colored_label(egui::Color32::GREEN, "● Online");
        } else {
            ui.colored_label(egui::Color32::RED, "● Offline");
        }
        ui.separator();
        ui.label(format!("Petals: {}", state.petal_count));
        ui.label(format!("Rooms:  {}", state.room_count));
        ui.label(format!("Models: {}", state.model_count));
        ui.label(format!("Peers:  {}", state.peer_count));
        ui.separator();
        ui.label("Tag Filter:");
        ui.label("(tag filter placeholder)");
        if is_admin {
            ui.separator();
            ui.label("Config URL:");
            ui.label("(config url field — admin only)");
        }
    });
}
