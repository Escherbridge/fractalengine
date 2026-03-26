use bevy_egui::egui;

use crate::atlas::DashboardState;
use crate::plugin::{InspectorState, NavigationState, SidebarState, ToolState};
use crate::theme;

// ---------------------------------------------------------------------------
// Top-level layout entry point
// ---------------------------------------------------------------------------

/// Renders the full UI shell: toolbar -> status bar -> sidebar -> inspector -> viewport.
pub fn gardener_console(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    tool: &mut ToolState,
    inspector: &mut InspectorState,
    nav: &NavigationState,
    dashboard: &DashboardState,
) {
    top_toolbar(ctx, sidebar, tool, inspector);
    status_bar(ctx, dashboard);
    left_sidebar(ctx, sidebar, nav, dashboard);
    right_inspector(ctx, inspector);

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            viewport_hints(ui, inspector);
        });
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
                let sidebar_icon = if sidebar.open { "\u{25C0} Hide" } else { "\u{25B6} Show" };
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
                    (Tool::Rotate, "\u{21BB} Rotate", "Rotate selected object (R)"),
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

fn status_bar(ctx: &egui::Context, dashboard: &DashboardState) {
    egui::TopBottomPanel::bottom("statusbar")
        .exact_height(22.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_STATUSBAR)
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if dashboard.is_online {
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
                ui.label(
                    egui::RichText::new(format!("{} peers", dashboard.peer_count))
                        .small()
                        .color(theme::TEXT_DIM),
                );
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
// Left sidebar
// ---------------------------------------------------------------------------

fn left_sidebar(
    ctx: &egui::Context,
    sidebar: &mut SidebarState,
    nav: &NavigationState,
    dashboard: &DashboardState,
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
            sidebar_verse_header(ui, nav);
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    sidebar_section_fractal(ui, nav);
                    ui.add_space(2.0);
                    sidebar_section_petals(ui, sidebar, dashboard);
                    ui.add_space(2.0);
                    sidebar_section_nodes(ui, dashboard);
                    ui.add_space(2.0);
                    sidebar_section_space_overview(ui, dashboard);
                    ui.add_space(4.0);
                });
        });
}

fn sidebar_verse_header(ui: &mut egui::Ui, nav: &NavigationState) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Verse:")
                .small()
                .color(theme::TEXT_DIM),
        );
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
                .add(
                    egui::Button::new("Switch")
                        .fill(theme::BG_BUTTON_ALT)
                        .small(),
                )
                .clicked()
            {
                // TODO: open verse switcher dialog
            }
        });
    });
    ui.add_space(6.0);
}

fn sidebar_section_fractal(ui: &mut egui::Ui, nav: &NavigationState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Fractal")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("My Fractal")
                    .small()
                    .color(theme::TEXT_DIM),
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let fractal_label = if nav.active_fractal_id.is_some() {
                nav.active_fractal_name.as_str()
            } else {
                "\u{2014}"
            };
            ui.label(
                egui::RichText::new(fractal_label)
                    .strong()
                    .color(theme::TEXT_BRIGHT),
            );
        });
        ui.add_space(4.0);
    });
}

fn sidebar_section_petals(ui: &mut egui::Ui, sidebar: &mut SidebarState, dashboard: &DashboardState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Petals")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut sidebar.tag_filter_buf)
                .hint_text("search petals\u{2026}")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        if dashboard.petal_count == 0 {
            ui.label(
                egui::RichText::new("No petals")
                    .italics()
                    .color(theme::TEXT_MUTED),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("{} petals in this fractal", dashboard.petal_count))
                    .color(theme::TEXT_SECTION),
            );
        }
        ui.add_space(4.0);
    });
}

fn sidebar_section_nodes(ui: &mut egui::Ui, dashboard: &DashboardState) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Nodes")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);
        if dashboard.node_count == 0 {
            ui.label(
                egui::RichText::new("No nodes")
                    .italics()
                    .color(theme::TEXT_MUTED),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("{} nodes", dashboard.node_count))
                    .color(theme::TEXT_SECTION),
            );
        }
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("Select a petal to see its nodes")
                .small()
                .italics()
                .color(theme::TEXT_MUTED),
        );
        ui.add_space(4.0);
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
                    ui.label(egui::RichText::new(format!("{:?}", entity)).monospace().small());
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
            ("Scale",    &mut inspector.scale),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [60.0, 16.0],
                    egui::Label::new(
                        egui::RichText::new(label)
                            .small()
                            .color(theme::TEXT_DIM),
                    ),
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

        ui.label(egui::RichText::new("External URL").small().color(theme::TEXT_DIM));
        ui.add(
            egui::TextEdit::singleline(&mut inspector.external_url)
                .hint_text("https://\u{2026}")
                .desired_width(f32::INFINITY),
        );

        if inspector.is_admin {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Config URL (admin)").small().color(theme::TEXT_DIM));
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
// Viewport overlay
// ---------------------------------------------------------------------------

fn viewport_hints(ui: &mut egui::Ui, inspector: &InspectorState) {
    if inspector.selected_entity.is_none() {
        let rect = ui.available_rect_before_wrap();
        let center = rect.center();

        ui.painter().text(
            egui::pos2(center.x, rect.max.y - 40.0),
            egui::Align2::CENTER_CENTER,
            "Click an object to select it  \u{2022}  S = Select  G = Move  R = Rotate",
            egui::FontId::proportional(12.0),
            theme::TEXT_VIEWPORT_HINT,
        );
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
