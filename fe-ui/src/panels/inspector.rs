//! Right inspector panel: entity/transform/URL/properties/schema/API-access
//! tabs for the selected node. This is a browser-integration seam — the
//! "Portal URLs" section here is where `UiAction::SaveUrl` and
//! `UiAction::OpenPortal` originate; see `fe-ui/src/AGENTS.md` §portal for
//! the full save/open chain and its known fragilities.

use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::navigation_manager::NavigationManager;
use crate::plugin::{InspectorFormState, InspectorTab, LocalUserRole, API_TOKEN_PAGE_SIZE};
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

pub(crate) fn right_inspector(
    ctx: &egui::Context,
    inspector: &mut InspectorFormState,
    node_mgr: &mut crate::node_manager::NodeManager,
    hierarchy: &VerseManager,
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
                    (InspectorTab::Query, "Query"),
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
                            crate::panels::asset_card::asset_card_section(ui, node_mgr, hierarchy, ui_mgr);
                            ui.add_space(2.0);
                            inspector_transform_section(ui, inspector);
                            ui.add_space(2.0);
                            inspector_url_meta_section(ui, inspector, ui_mgr, local_role);
                            ui.add_space(2.0);
                            inspector_properties_section(ui, inspector, node_mgr, ui_mgr);
                            ui.add_space(2.0);
                            inspector_schema_section(ui, inspector, node_mgr, local_role, db_tx, nav);
                        }
                        InspectorTab::ApiAccess => {
                            ui.add_space(4.0);
                            inspector_api_access_section(ui, inspector, node_mgr, db_tx, local_role, ui_mgr);
                        }
                        InspectorTab::Query => {
                            ui.add_space(4.0);
                            crate::panels::query_tab::inspector_query_section(ui, inspector, nav, ui_mgr);
                        }
                    }
                });
        });
}

/// Build a scope string from the current navigation state.
pub(crate) fn build_nav_scope(nav: &NavigationManager) -> String {
    match (&nav.active_verse_id, &nav.active_fractal_id, &nav.active_petal_id) {
        (Some(v), Some(f), Some(p)) => fe_database::build_scope(v, Some(f), Some(p)),
        (Some(v), Some(f), None) => fe_database::build_scope(v, Some(f), None),
        (Some(v), None, _) => fe_database::build_scope(v, None, None),
        _ => String::new(),
    }
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

/// The "Portal URLs" section: `Save` pushes `UiAction::SaveUrl` (commits
/// `external_url`/`config_url` for the selected node); `Open Portal` pushes
/// `UiAction::OpenPortal` with the current `external_url` text. See
/// `fe-ui/src/AGENTS.md` §portal for the full chain from here to `DbCommand`.
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

                    let mut revoke_jti: Option<(String, String)> = None;
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
                                                revoke_jti = Some((tok.jti.clone(), tok.sub.clone()));
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
                    if let Some((jti, sub)) = revoke_jti.take() {
                        db_tx.send(DbCommand::RevokeApiToken { jti, sub }).ok();
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

// ---------------------------------------------------------------------------
// Property type options for ComboBox dropdowns
// ---------------------------------------------------------------------------

const PROPERTY_TYPES: &[&str] = &[
    "string", "number", "bool", "datetime",
    "geometry_point", "geometry_polygon",
    "blob_ref", "hexon_ref", "address", "array", "object",
];

// ---------------------------------------------------------------------------
// Custom Properties section (key-value editor)
// ---------------------------------------------------------------------------

fn inspector_properties_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Custom Properties")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);

        let node_id = match node_mgr.selected.as_ref() {
            Some(sel) => sel.node_id.clone(),
            None => {
                ui.label(
                    egui::RichText::new("No node selected")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                return;
            }
        };

        if inspector.node_properties_loading {
            ui.label(
                egui::RichText::new("Loading...")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            return;
        }

        // Display existing properties
        let mut delete_key: Option<String> = None;
        if let Some(obj) = inspector.node_properties.as_object() {
            if obj.is_empty() {
                ui.label(
                    egui::RichText::new("No custom properties")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            } else {
                egui::Grid::new("node_props_grid")
                    .num_columns(3)
                    .spacing([6.0, 3.0])
                    .show(ui, |ui| {
                        for (key, value) in obj {
                            ui.label(
                                egui::RichText::new(key)
                                    .small()
                                    .strong()
                                    .color(theme::TEXT_DIM),
                            );
                            let val_str = match value {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            ui.label(
                                egui::RichText::new(&val_str)
                                    .small()
                                    .monospace()
                                    .color(theme::TEXT_BRIGHT),
                            );
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("\u{2715}").small(),
                                    )
                                    .fill(theme::BG_DANGER)
                                    .small(),
                                )
                                .on_hover_text("Delete property")
                                .clicked()
                            {
                                delete_key = Some(key.clone());
                            }
                            ui.end_row();
                        }
                    });
            }
        }

        if let Some(key) = delete_key {
            ui_mgr.push_action(UiAction::DeleteNodeProperty {
                node_id: node_id.clone(),
                key,
            });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        // Add property row
        ui.label(
            egui::RichText::new("Add Property")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut inspector.prop_add_key_buf)
                    .hint_text("key")
                    .desired_width(60.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut inspector.prop_add_value_buf)
                    .hint_text("value")
                    .desired_width(60.0),
            );
            egui::ComboBox::from_id_salt("prop_type_combo")
                .selected_text(&inspector.prop_add_type_buf)
                .width(70.0)
                .show_ui(ui, |ui| {
                    for &t in PROPERTY_TYPES {
                        ui.selectable_value(&mut inspector.prop_add_type_buf, t.to_string(), t);
                    }
                });
        });

        ui.add_space(4.0);

        let can_add = !inspector.prop_add_key_buf.trim().is_empty()
            && !inspector.prop_add_value_buf.trim().is_empty();

        let btn = egui::Button::new("Add")
            .fill(if can_add { theme::BG_SAVE } else { theme::BG_BUTTON })
            .min_size(egui::vec2(ui.available_width(), 24.0));

        if ui.add_enabled(can_add, btn).clicked() {
            let key = inspector.prop_add_key_buf.trim().to_string();
            let raw = inspector.prop_add_value_buf.trim().to_string();
            let value = match inspector.prop_add_type_buf.as_str() {
                "number" => raw
                    .parse::<f64>()
                    .map(|n| serde_json::Value::from(n))
                    .unwrap_or_else(|_| serde_json::Value::String(raw.clone())),
                "bool" => match raw.to_lowercase().as_str() {
                    "true" | "1" | "yes" => serde_json::Value::Bool(true),
                    "false" | "0" | "no" => serde_json::Value::Bool(false),
                    _ => serde_json::Value::String(raw.clone()),
                },
                _ => serde_json::Value::String(raw.clone()),
            };
            ui_mgr.push_action(UiAction::SetNodeProperty {
                node_id: node_id.clone(),
                key,
                value,
            });
            inspector.prop_add_key_buf.clear();
            inspector.prop_add_value_buf.clear();
        }

        ui.add_space(4.0);
    });
}

// ---------------------------------------------------------------------------
// Property Schema section (field definition management)
// ---------------------------------------------------------------------------

fn inspector_schema_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &crate::node_manager::NodeManager,
    local_role: &LocalUserRole,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
) {
    if !local_role.can_manage() {
        return;
    }

    egui::CollapsingHeader::new(
        egui::RichText::new("Property Schema")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);

        if node_mgr.selected.is_none() {
            ui.label(
                egui::RichText::new("No node selected")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            return;
        }

        if inspector.field_defs_loading {
            ui.label(
                egui::RichText::new("Loading...")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            return;
        }

        // Display existing field definitions
        let mut delete_idx: Option<usize> = None;
        if inspector.field_defs.is_empty() {
            ui.label(
                egui::RichText::new("No field definitions")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        } else {
            egui::Grid::new("field_defs_grid")
                .num_columns(4)
                .spacing([6.0, 3.0])
                .show(ui, |ui| {
                    // Header
                    ui.label(egui::RichText::new("Key").small().strong().color(theme::TEXT_DIM));
                    ui.label(egui::RichText::new("Type").small().strong().color(theme::TEXT_DIM));
                    ui.label(egui::RichText::new("Req").small().strong().color(theme::TEXT_DIM));
                    ui.label(egui::RichText::new("").small());
                    ui.end_row();

                    for (idx, fd) in inspector.field_defs.iter().enumerate() {
                        ui.label(
                            egui::RichText::new(&fd.key)
                                .small()
                                .color(theme::TEXT_BRIGHT),
                        );
                        ui.label(
                            egui::RichText::new(&fd.value_type)
                                .small()
                                .monospace()
                                .color(theme::TEXT_AXIS),
                        );
                        let req_label = if fd.required { "\u{2713}" } else { "\u{2014}" };
                        ui.label(egui::RichText::new(req_label).small().color(theme::TEXT_DIM));
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("\u{2715}").small(),
                                )
                                .fill(theme::BG_DANGER)
                                .small(),
                            )
                            .on_hover_text(format!("Delete field def '{}'", fd.key))
                            .clicked()
                        {
                            delete_idx = Some(idx);
                        }
                        ui.end_row();
                    }
                });
        }

        if let Some(idx) = delete_idx {
            let fd = inspector.field_defs.remove(idx);
            db_tx.send(DbCommand::DeleteFieldDef {
                field_def_id: fd.field_def_id,
            }).ok();
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        // Add field definition row
        ui.label(
            egui::RichText::new("Add Field Definition")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut inspector.field_def_add_key_buf)
                    .hint_text("key")
                    .desired_width(60.0),
            );
            egui::ComboBox::from_id_salt("field_def_type_combo")
                .selected_text(&inspector.field_def_add_type_buf)
                .width(80.0)
                .show_ui(ui, |ui| {
                    for &t in PROPERTY_TYPES {
                        ui.selectable_value(
                            &mut inspector.field_def_add_type_buf,
                            t.to_string(),
                            t,
                        );
                    }
                });
            ui.checkbox(&mut inspector.field_def_add_required, "Req");
        });

        ui.add_space(2.0);
        ui.add(
            egui::TextEdit::singleline(&mut inspector.field_def_add_desc_buf)
                .hint_text("description (optional)")
                .desired_width(f32::INFINITY),
        );

        ui.add_space(4.0);

        let can_add = !inspector.field_def_add_key_buf.trim().is_empty();
        let btn = egui::Button::new("Add Field")
            .fill(if can_add { theme::BG_SAVE } else { theme::BG_BUTTON })
            .min_size(egui::vec2(ui.available_width(), 24.0));

        if ui.add_enabled(can_add, btn).clicked() {
            let key = inspector.field_def_add_key_buf.trim().to_string();
            let value_type = inspector.field_def_add_type_buf.clone();
            let scope = build_nav_scope(nav);

            // Send DbCommand to persist
            db_tx.send(DbCommand::CreateFieldDef {
                scope: scope.clone(),
                entity_type: "node".to_string(),
                key: key.clone(),
                value_type: value_type.clone(),
                default_val: None,
            }).ok();

            // Add locally for immediate feedback
            let entry = crate::plugin::FieldDefEntry {
                field_def_id: format!("pending-{}", inspector.field_defs.len()),
                key,
                value_type,
                description: inspector.field_def_add_desc_buf.trim().to_string(),
                required: inspector.field_def_add_required,
                default_val: None,
            };
            inspector.field_defs.push(entry);
            inspector.field_def_add_key_buf.clear();
            inspector.field_def_add_desc_buf.clear();
            inspector.field_def_add_required = false;
        }

        ui.add_space(4.0);
    });
}
