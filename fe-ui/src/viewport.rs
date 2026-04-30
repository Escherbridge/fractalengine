use bevy_egui::egui;

use crate::atlas::DashboardState;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{
    ActiveDialog, CreateKind, EntitySettingsType, LocalUserRole, SettingsTab, UiManager,
    ViewportCursorWorld,
};
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

// ---------------------------------------------------------------------------
// Viewport overlay — navigation-state-aware
// ---------------------------------------------------------------------------

/// Renders the viewport overlay based on navigation depth:
/// - No verse → peer browser / verse discovery
/// - Verse selected → fractal list
/// - Fractal selected → petal list for navigation
/// - Petal selected → 3D space with object hints + right-click
pub fn viewport_overlay(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    node_mgr: &crate::node_manager::NodeManager,
    hierarchy: &VerseManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    dashboard: &DashboardState,
    cursor_world: &ViewportCursorWorld,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
) {
    let rect = ui.available_rect_before_wrap();
    let center = rect.center();

    if nav.active_petal_id.is_some() {
        // === PETAL SELECTED: Full 3D space ===
        viewport_petal_space(
            ui,
            nav,
            node_mgr,
            hierarchy,
            rect,
            center,
            cursor_world,
            ui_mgr,
            local_role,
        );
    } else if nav.active_fractal_id.is_some() {
        // === FRACTAL SELECTED: Show petal cards ===
        viewport_petal_browser(ui, nav, hierarchy, db_tx, rect, center, ui_mgr, local_role);
    } else if nav.active_verse_id.is_some() {
        // === VERSE SELECTED: Show fractal cards ===
        viewport_fractal_browser(ui, nav, hierarchy, db_tx, rect, center, ui_mgr, local_role);
    } else {
        // === NO VERSE: Peer discovery / verse browser ===
        viewport_verse_browser(
            ui,
            nav,
            hierarchy,
            db_tx,
            dashboard,
            rect,
            center,
            ui_mgr,
            local_role,
        );
    }
}

/// Petal selected — full 3D space with tool hints, breadcrumb, right-click.
pub fn viewport_petal_space(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    node_mgr: &crate::node_manager::NodeManager,
    hierarchy: &VerseManager,
    rect: egui::Rect,
    center: egui::Pos2,
    cursor_world: &ViewportCursorWorld,
    ui_mgr: &mut UiManager,
    _local_role: &LocalUserRole,
) {
    // Breadcrumb at top of viewport (names)
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

    // Breadcrumb: verse / fractal / petal with IDs (top-left)
    if let (Some(ref vid), Some(ref fid), Some(ref pid)) =
        (&nav.active_verse_id, &nav.active_fractal_id, &nav.active_petal_id)
    {
        let crumb = format!(
            "v:{} / f:{} / p:{}",
            truncate_id(vid), truncate_id(fid), truncate_id(pid),
        );
        ui.painter().text(
            egui::pos2(rect.min.x + 10.0, rect.min.y + 10.0),
            egui::Align2::LEFT_CENTER,
            crumb,
            egui::FontId::proportional(9.0),
            theme::TEXT_DIM,
        );
    }

    // Tool hints at bottom
    if node_mgr.selected_entity().is_none() {
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
        nav.back_from_petal();
    }

    // Right-click for context menu — use input directly to avoid stealing clicks
    if ui.input(|i| i.pointer.secondary_clicked()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if rect.contains(pos) {
                ui_mgr.open_dialog(ActiveDialog::ContextMenu {
                    screen_pos: [pos.x, pos.y],
                    world_pos: cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]),
                });
            }
        }
    }
}

/// No verse selected — verse browser + peer discovery.
pub fn viewport_verse_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    hierarchy: &VerseManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    dashboard: &DashboardState,
    rect: egui::Rect,
    center: egui::Pos2,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
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

        let mut card_clicked = false;
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
                card_clicked = true;
            }
        });

        // Gear icon overlay on top-right of card (managers only)
        let mut gear_clicked = false;
        if local_role.can_manage() {
            let gear_rect = egui::Rect::from_min_size(
                egui::pos2(card_rect.right() - 26.0, card_rect.top() + 4.0),
                egui::vec2(22.0, 22.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(gear_rect), |ui| {
                let gear_resp = ui.add(
                    egui::Button::new(
                        egui::RichText::new("\u{2699}")
                            .size(14.0)
                            .color(theme::ICON_GEAR),
                    )
                    .fill(theme::BG_BUTTON)
                    .corner_radius(4.0),
                );
                if gear_resp.clicked() {
                    gear_clicked = true;
                }
            });
        }

        if gear_clicked {
            ui_mgr.open_dialog(ActiveDialog::EntitySettings {
                entity_type: EntitySettingsType::Verse,
                entity_id: verse.id.clone(),
                entity_name: verse.name.clone(),
                parent_verse_id: verse.id.clone(),
                parent_fractal_id: None,
                active_tab: SettingsTab::General,
                name_buf: verse.name.clone(),
                default_access_buf: Some("viewer".to_string()),
                description_buf: None,
                peer_roles: vec![],
                roles_loading: false,
                invite_role_buf: "viewer".to_string(),
                invite_expiry_buf: 24,
                generated_invite_link: None,
                pending_delete: false,
                api_tokens: vec![],
                api_tokens_loading: false,
                api_token_scope_buf: String::new(),
                api_token_role_buf: "viewer".to_string(),
                api_token_expiry_buf: 24,
                generated_api_token: None,
                scoped_api_tokens: vec![],
                scoped_tokens_loading: false,
            });
        } else if card_clicked {
            nav.navigate_to_verse(verse.id.clone(), verse.name.clone());
        }

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
            ui_mgr.open_dialog(ActiveDialog::CreateEntity {
                kind: CreateKind::Verse,
                parent_id: String::new(),
                name_buf: String::new(),
            });
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
pub fn viewport_fractal_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    hierarchy: &VerseManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    rect: egui::Rect,
    center: egui::Pos2,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
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
        nav.back_from_verse();
    }

    ui.painter().text(
        egui::pos2(center.x, rect.min.y + 35.0),
        egui::Align2::CENTER_CENTER,
        format!("{} — Select a Fractal", nav.active_verse_name),
        egui::FontId::proportional(18.0),
        theme::TEXT_STRONG,
    );

    // Breadcrumb: verse ID + name
    if let Some(ref vid) = nav.active_verse_id {
        let crumb = format!("verse: {} ({})", nav.active_verse_name, truncate_id(vid));
        ui.painter().text(
            egui::pos2(rect.min.x + 10.0, rect.min.y + 32.0),
            egui::Align2::LEFT_CENTER,
            crumb,
            egui::FontId::proportional(9.0),
            theme::TEXT_DIM,
        );
    }

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

            let mut card_clicked = false;
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
                    card_clicked = true;
                }
            });

            // Gear icon overlay on top-right of card (managers only)
            let mut gear_clicked = false;
            if local_role.can_manage() {
                let gear_rect = egui::Rect::from_min_size(
                    egui::pos2(card_rect.right() - 26.0, card_rect.top() + 4.0),
                    egui::vec2(22.0, 22.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(gear_rect), |ui| {
                    let gear_resp = ui.add(
                        egui::Button::new(
                            egui::RichText::new("\u{2699}")
                                .size(14.0)
                                .color(theme::ICON_GEAR),
                        )
                        .fill(theme::BG_BUTTON)
                        .corner_radius(4.0),
                    );
                    if gear_resp.clicked() {
                        gear_clicked = true;
                    }
                });
            }

            if gear_clicked {
                ui_mgr.open_dialog(ActiveDialog::EntitySettings {
                    entity_type: EntitySettingsType::Fractal,
                    entity_id: fractal.id.clone(),
                    entity_name: fractal.name.clone(),
                    parent_verse_id: verse.id.clone(),
                    parent_fractal_id: None,
                    active_tab: SettingsTab::General,
                    name_buf: fractal.name.clone(),
                    default_access_buf: Some("viewer".to_string()),
                    description_buf: None,
                    peer_roles: vec![],
                    roles_loading: false,
                    invite_role_buf: "viewer".to_string(),
                    invite_expiry_buf: 24,
                    generated_invite_link: None,
                    pending_delete: false,
                    api_tokens: vec![],
                    api_tokens_loading: false,
                    api_token_scope_buf: String::new(),
                    api_token_role_buf: "viewer".to_string(),
                    api_token_expiry_buf: 24,
                    generated_api_token: None,
                    scoped_api_tokens: vec![],
                    scoped_tokens_loading: false,
                });
            } else if card_clicked {
                nav.navigate_to_fractal(fractal.id.clone(), fractal.name.clone());
            }

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
                ui_mgr.open_dialog(ActiveDialog::CreateEntity {
                    kind: CreateKind::Fractal,
                    parent_id: verse.id.clone(),
                    name_buf: String::new(),
                });
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
pub fn viewport_petal_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    hierarchy: &VerseManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    rect: egui::Rect,
    center: egui::Pos2,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
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
        nav.back_from_fractal();
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

    // Breadcrumb: verse / fractal with IDs
    if let (Some(ref vid), Some(ref fid)) = (&nav.active_verse_id, &nav.active_fractal_id) {
        let crumb = format!(
            "verse: {} ({}) / fractal: {} ({})",
            nav.active_verse_name, truncate_id(vid),
            nav.active_fractal_name, truncate_id(fid),
        );
        ui.painter().text(
            egui::pos2(rect.min.x + 10.0, rect.min.y + 32.0),
            egui::Align2::LEFT_CENTER,
            crumb,
            egui::FontId::proportional(9.0),
            theme::TEXT_DIM,
        );
    }

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

            let mut card_clicked = false;
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
                    card_clicked = true;
                }
            });

            // Gear icon overlay on top-right of card (managers only)
            let mut gear_clicked = false;
            if local_role.can_manage() {
                let gear_rect = egui::Rect::from_min_size(
                    egui::pos2(card_rect.right() - 26.0, card_rect.top() + 4.0),
                    egui::vec2(22.0, 22.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(gear_rect), |ui| {
                    let gear_resp = ui.add(
                        egui::Button::new(
                            egui::RichText::new("\u{2699}")
                                .size(14.0)
                                .color(theme::ICON_GEAR),
                        )
                        .fill(theme::BG_BUTTON)
                        .corner_radius(4.0),
                    );
                    if gear_resp.clicked() {
                        gear_clicked = true;
                    }
                });
            }

            if gear_clicked {
                ui_mgr.open_dialog(ActiveDialog::EntitySettings {
                    entity_type: EntitySettingsType::Petal,
                    entity_id: petal.id.clone(),
                    entity_name: petal.name.clone(),
                    parent_verse_id: nav.active_verse_id.clone().unwrap_or_default(),
                    parent_fractal_id: nav.active_fractal_id.clone(),
                    active_tab: SettingsTab::General,
                    name_buf: petal.name.clone(),
                    default_access_buf: Some("viewer".to_string()),
                    description_buf: None,
                    peer_roles: vec![],
                    roles_loading: false,
                    invite_role_buf: "viewer".to_string(),
                    invite_expiry_buf: 24,
                    generated_invite_link: None,
                    pending_delete: false,
                    api_tokens: vec![],
                    api_tokens_loading: false,
                    api_token_scope_buf: String::new(),
                    api_token_role_buf: "viewer".to_string(),
                    api_token_expiry_buf: 24,
                    generated_api_token: None,
                    scoped_api_tokens: vec![],
                    scoped_tokens_loading: false,
                });
            } else if card_clicked {
                nav.navigate_to_petal(petal.id.clone());
            }

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
                ui_mgr.open_dialog(ActiveDialog::CreateEntity {
                    kind: CreateKind::Petal,
                    parent_id: fractal_id,
                    name_buf: String::new(),
                });
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
/// Truncate a ULID to its first 8 characters for compact display in breadcrumbs.
fn truncate_id(id: &str) -> &str {
    &id[..8.min(id.len())]
}

pub fn find_petal_name(hierarchy: &VerseManager, petal_id: &str) -> String {
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
