use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::actions::UiManager;
use crate::atlas::DashboardState;
use crate::dialogs::{ActiveDialog, CreateKind, EntitySettingsType};
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar;
use crate::plugin::{LocalUserRole, ViewportCursorWorld};
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
            ui, nav, hierarchy, db_tx, dashboard, rect, center, ui_mgr, local_role,
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
    let mut navigated = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            rect.min + egui::vec2(8.0, 4.0),
            egui::vec2((rect.width() - 16.0).max(0.0), 70.0),
        )),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_sized([52.0, 26.0], egui::Button::new("Back"))
                    .named(ui, "Back to petal list")
                    .clicked()
                {
                    nav.back_from_petal();
                    navigated = true;
                }
                if !navigated {
                    navigated = breadcrumbs(ui, nav, 3, hierarchy);
                }
            });
        },
    );
    if navigated {
        return;
    }
    let input = toolbar::input_context(ui.ctx());
    let active_tool = input.map(|input| input.active_tool);
    let gesture_active = input.is_some_and(|input| input.gesture_active);
    let hint=match active_tool {
        Some(toolbar::Tool::Pen)=>Some("Pen: click terrain to add points; drag a marker to move; Ctrl+drag raises or lowers height; Shift/Alt+click annotates.".to_owned()),
        Some(toolbar::Tool::Brush)=>Some("Brush: press-drag to sample earthwork; release commits; Escape or right-click cancels the stroke.".to_owned()),
        Some(toolbar::Tool::Measure)=>Some("Measure: click ground to place points; read and copy the result in Options; Escape returns to Select.".to_owned()),
        _ if node_mgr.selected_entity().is_none()=>Some(format!("Click an object to select it. {}",toolbar::shortcut_hint_line())),
        _=>None,
    };
    if let Some(hint) = hint {
        viewport_hint(ui.ctx(), rect, &hint);
    }

    // Right-click for context menu — use input directly to avoid stealing clicks.
    // `target: None` = classification pending; `node_manager::context_pick`
    // fills it from the same pick machinery left-click uses (T4 FR-1).
    // D5, one rule for every tool: right-click cancels an in-flight gesture
    // (the gesture systems consume the press themselves), otherwise it opens
    // the object-aware menu. No tool-specific gate — Brush is not a mode.
    if !gesture_active && ui.input(|i| i.pointer.secondary_clicked()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if rect.contains(pos) {
                ui_mgr.open_dialog(ActiveDialog::ContextMenu {
                    screen_pos: [pos.x, pos.y],
                    world_pos: cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]),
                    target: None,
                    pending_delete: false,
                    descendant_count: None,
                });
            }
        }
    }
}

#[derive(Clone, Copy)]
struct BrowserCard<'a> {
    id: &'a str,
    name: &'a str,
    kind: EntitySettingsType,
    verse: &'a str,
    fractal: Option<&'a str>,
    count: usize,
    noun: &'static str,
    plural: &'static str,
}

enum BrowserAction {
    Open(usize),
    Options(usize),
    Create,
    Join,
}

fn bounded_browser(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    browser_key: &str,
    title: &str,
    cards: &[BrowserCard<'_>],
    new_label: &str,
    can_create: bool,
    show_join: bool,
) -> Option<BrowserAction> {
    let mut action = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)),|ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(rect));
        ui.add(egui::Label::new(egui::RichText::new(title).heading()).truncate()).on_hover_text(title);ui.add_space(6.0);
        if show_join && ui.add_sized([120.0,26.0],egui::Button::new("Join a verse...")).named(ui,"Join a verse from browser").clicked(){action=Some(BrowserAction::Join);}
        let area=egui::ScrollArea::vertical().id_salt(("hierarchy_browser",browser_key)).max_height(ui.available_height().max(1.0));
        crate::ui_shell::layout_policy::scroll(ui,title,[false,true],area,|ui| {
            let gap=12.0;let width=ui.available_width().max(1.0);
            let columns=((width+gap)/212.0).floor().max(1.0) as usize;
            let card_width=((width-gap*(columns.saturating_sub(1)) as f32)/columns as f32).clamp(80.0,260.0);
            egui::Grid::new(("hierarchy_cards",browser_key)).spacing(egui::vec2(gap,gap)).show(ui,|ui| {
                for (index,card) in cards.iter().enumerate() {
                    ui.push_id(card.id,|ui| {
                    egui::Frame::NONE.fill(theme::BG_PANEL).stroke(egui::Stroke::new(1.0,theme::TEXT_DIM)).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui,|ui| {
                        ui.set_width((card_width-16.0).max(48.0));
                        ui.horizontal(|ui| {
                            let name_width=(ui.available_width()-34.0).max(24.0);
                            if ui.add_sized([name_width,28.0],egui::Button::new(card.name).truncate()).named(ui,format_args!("Open {} {}",card.kind_name(),card.name)).on_hover_text(format!("{}\n{}",card.name,card.id)).clicked(){action=Some(BrowserAction::Open(index));}
                            if ui.add_sized([26.0,28.0],egui::Button::new("...")).named(ui,format_args!("Options for {} {}",card.kind_name(),card.name)).on_hover_text("View stored settings, access and ownership options").clicked(){action=Some(BrowserAction::Options(index));}
                        });
                        ui.label(format!("{} {}",card.count,if card.count==1 {card.noun}else{card.plural}));
                    });
                    });
                    if (index+1)%columns==0 {ui.end_row();}
                }
                let response=ui.add_enabled(can_create,egui::Button::new(new_label).min_size(egui::vec2(card_width,72.0))).named(ui,new_label).on_disabled_hover_text("Manager role at the current entry is required to create a child.");
                if response.clicked(){action=Some(BrowserAction::Create);}
            });
            if cards.is_empty(){ui.label("This entry has no children yet.");}
        });
    });
    action
}

impl BrowserCard<'_> {
    fn kind_name(&self) -> &'static str {
        match self.kind {
            EntitySettingsType::Verse => "verse",
            EntitySettingsType::Fractal => "fractal",
            EntitySettingsType::Petal => "petal",
        }
    }
    fn open_options(&self, ui_mgr: &mut UiManager) {
        ui_mgr.open_dialog(crate::dialogs::entity_details::dialog(
            self.kind,
            self.id,
            self.name,
            self.verse,
            self.fractal,
        ));
    }
}

fn breadcrumb_button(ui: &mut egui::Ui, name: &str, id: &str, label: &str) -> bool {
    ui.add_sized(
        [ui.available_width().clamp(32.0, 140.0), 26.0],
        egui::Button::new(name).truncate(),
    )
    .named(ui, label)
    .on_hover_text(format!("{name}\n{id}"))
    .clicked()
}

fn breadcrumbs(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    depth: u8,
    hierarchy: &VerseManager,
) -> bool {
    let mut route = 0;
    let verse = nav.active_verse_id.clone();
    let verse_name = nav.active_verse_name.clone();
    let fractal = nav.active_fractal_id.clone();
    let fractal_name = nav.active_fractal_name.clone();
    ui.horizontal_wrapped(|ui| {
        if breadcrumb_button(ui, "Verses", "all", "Browse all verses") {
            route = 1;
        }
        if let Some(id) = verse.as_deref() {
            ui.label("/");
            if depth > 1 {
                if breadcrumb_button(ui, &verse_name, id, "Browse current verse fractals") {
                    route = 2;
                }
            } else {
                ui.add(egui::Label::new(&verse_name).truncate())
                    .on_hover_text(format!("{}\n{id}", nav.active_verse_name));
            }
        }
        if let Some(id) = fractal.as_deref() {
            ui.label("/");
            if depth > 2 {
                if breadcrumb_button(ui, &fractal_name, id, "Browse current fractal petals") {
                    route = 3;
                }
            } else {
                ui.add(egui::Label::new(&fractal_name).truncate())
                    .on_hover_text(format!("{fractal_name}\n{id}"));
            }
        }
        if depth == 3 {
            if let Some(id) = nav.active_petal_id.as_deref() {
                ui.label("/");
                ui.add(egui::Label::new(find_petal_name(hierarchy, id)).truncate())
                    .on_hover_text(format!("{}\n{id}", find_petal_name(hierarchy, id)));
            }
        }
    });
    match route {
        1 => nav.back_from_verse(),
        2 => nav.navigate_to_verse(verse.unwrap(), verse_name),
        3 => nav.navigate_to_fractal(fractal.unwrap(), fractal_name),
        _ => return false,
    }
    true
}

fn viewport_hint(ctx: &egui::Context, rect: egui::Rect, text: &str) {
    egui::Area::new(egui::Id::new("viewport_tool_hint"))
        .order(egui::Order::Middle)
        .interactable(false)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(rect.center().x, rect.bottom() - 8.0))
        .show(ctx, |ui| {
            ui.set_max_width((rect.width() - 24.0).clamp(60.0, 620.0));
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(8))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).color(theme::TEXT_BRIGHT))
                            .wrap(),
                    );
                });
        });
}

/// Browse verses with target-authorized Options available on every card.
pub fn viewport_verse_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    hierarchy: &VerseManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    dashboard: &DashboardState,
    rect: egui::Rect,
    _center: egui::Pos2,
    ui_mgr: &mut UiManager,
    _local_role: &LocalUserRole,
) {
    let cards: Vec<_> = hierarchy
        .verses
        .iter()
        .map(|verse| BrowserCard {
            id: &verse.id,
            name: &verse.name,
            kind: EntitySettingsType::Verse,
            verse: &verse.id,
            fractal: None,
            count: verse.fractals.len(),
            noun: "fractal",
            plural: "fractals",
        })
        .collect();
    let title = format!(
        "{} {} - {} reported peers",
        cards.len(),
        if cards.len() == 1 { "verse" } else { "verses" },
        dashboard.peer_count
    );
    match bounded_browser(
        ui,
        rect,
        "all-verses",
        &title,
        &cards,
        "+ New Verse",
        true,
        true,
    ) {
        Some(BrowserAction::Open(index)) => {
            nav.navigate_to_verse(cards[index].id, cards[index].name)
        }
        Some(BrowserAction::Options(index)) => cards[index].open_options(ui_mgr),
        Some(BrowserAction::Create) => ui_mgr.open_dialog(ActiveDialog::CreateEntity {
            kind: CreateKind::Verse,
            parent_id: String::new(),
            name_buf: String::new(),
        }),
        Some(BrowserAction::Join) => ui_mgr.open_dialog(ActiveDialog::JoinDialog {
            invite_buf: String::new(),
        }),
        None => {}
    }
}

/// Browse an active verse's fractals through a bounded card flow.
pub fn viewport_fractal_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    hierarchy: &VerseManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    rect: egui::Rect,
    _center: egui::Pos2,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
) {
    let mut changed = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width(), 60.0),
        )),
        |ui| {
            changed = breadcrumbs(ui, nav, 1, hierarchy);
        },
    );
    if changed {
        return;
    }
    let Some(verse) = hierarchy
        .verses
        .iter()
        .find(|verse| nav.active_verse_id.as_deref() == Some(&verse.id))
    else {
        return;
    };
    let cards: Vec<_> = verse
        .fractals
        .iter()
        .map(|fractal| BrowserCard {
            id: &fractal.id,
            name: &fractal.name,
            kind: EntitySettingsType::Fractal,
            verse: &verse.id,
            fractal: None,
            count: fractal.petals.len(),
            noun: "petal",
            plural: "petals",
        })
        .collect();
    let title = format!(
        "{} {} in {}",
        cards.len(),
        if cards.len() == 1 {
            "fractal"
        } else {
            "fractals"
        },
        verse.name
    );
    let body = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 62.0), rect.max);
    match bounded_browser(
        ui,
        body,
        &verse.id,
        &title,
        &cards,
        "+ New Fractal",
        local_role.can_manage(),
        false,
    ) {
        Some(BrowserAction::Open(index)) => {
            nav.navigate_to_fractal(cards[index].id, cards[index].name)
        }
        Some(BrowserAction::Options(index)) => cards[index].open_options(ui_mgr),
        Some(BrowserAction::Create) => ui_mgr.open_dialog(ActiveDialog::CreateEntity {
            kind: CreateKind::Fractal,
            parent_id: verse.id.clone(),
            name_buf: String::new(),
        }),
        _ => {}
    }
}

/// Browse petals with human ancestor links and readable target Options.
pub fn viewport_petal_browser(
    ui: &mut egui::Ui,
    nav: &mut NavigationManager,
    hierarchy: &VerseManager,
    _db_tx: &crossbeam::channel::Sender<DbCommand>,
    rect: egui::Rect,
    _center: egui::Pos2,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
) {
    let mut changed = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width(), 60.0),
        )),
        |ui| {
            changed = breadcrumbs(ui, nav, 2, hierarchy);
        },
    );
    if changed {
        return;
    }
    let Some(verse) = hierarchy
        .verses
        .iter()
        .find(|verse| nav.active_verse_id.as_deref() == Some(&verse.id))
    else {
        return;
    };
    let Some(fractal) = verse
        .fractals
        .iter()
        .find(|fractal| nav.active_fractal_id.as_deref() == Some(&fractal.id))
    else {
        return;
    };
    let cards: Vec<_> = fractal
        .petals
        .iter()
        .map(|petal| BrowserCard {
            id: &petal.id,
            name: &petal.name,
            kind: EntitySettingsType::Petal,
            verse: &verse.id,
            fractal: Some(&fractal.id),
            count: petal.nodes.len(),
            noun: "node",
            plural: "nodes",
        })
        .collect();
    let title = format!(
        "{} {} in {}",
        cards.len(),
        if cards.len() == 1 { "petal" } else { "petals" },
        fractal.name
    );
    let body = egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 62.0), rect.max);
    match bounded_browser(
        ui,
        body,
        &fractal.id,
        &title,
        &cards,
        "+ New Petal",
        local_role.can_manage(),
        false,
    ) {
        Some(BrowserAction::Open(index)) => nav.navigate_to_petal(cards[index].id),
        Some(BrowserAction::Options(index)) => cards[index].open_options(ui_mgr),
        Some(BrowserAction::Create) => ui_mgr.open_dialog(ActiveDialog::CreateEntity {
            kind: CreateKind::Petal,
            parent_id: fractal.id.clone(),
            name_buf: String::new(),
        }),
        _ => {}
    }
}

/// Find a petal's human name in the loaded hierarchy.
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

#[cfg(test)]
pub(crate) fn browser_access_render_journey() {
    use crate::verse_manager::{FractalEntry, PetalEntry, VerseEntry};
    let mut hierarchy = VerseManager::default();
    hierarchy.verses.push(VerseEntry {
        id: "browser-v".into(),
        name: "Long readable verse name ".repeat(8),
        fractals: vec![FractalEntry {
            id: "browser-f".into(),
            name: "Fractal name".into(),
            petals: vec![PetalEntry {
                id: "browser-p".into(),
                name: "Petal name".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    for index in 1..128 {
        hierarchy.verses.push(VerseEntry {
            id: format!("other-{index}"),
            name: format!("Other verse {index}"),
            ..Default::default()
        });
    }
    let (sender, receiver) = crossbeam::channel::unbounded();
    for size in [
        egui::vec2(1024.0, 768.0),
        egui::vec2(1280.0, 720.0),
        egui::vec2(1920.0, 1080.0),
    ] {
        for depth in 0..3 {
            let ctx = egui::Context::default();
            let mut time = 0.0;
            let mut nav = NavigationManager::default();
            if depth > 0 {
                nav.navigate_to_verse("browser-v", &hierarchy.verses[0].name);
            }
            if depth > 1 {
                nav.navigate_to_fractal("browser-f", "Fractal name");
            }
            let mut manager = UiManager::default();
            let mut frame = |events: Vec<egui::Event>,
                             nav: &mut NavigationManager,
                             manager: &mut UiManager,
                             contents: &VerseManager| {
                time += 1.0 / 60.0;
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        crate::ui_shell::layout_policy::begin_frame(ctx, true);
                        crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let rect = ui.available_rect_before_wrap();
                            let center = rect.center();
                            let unresolved = LocalUserRole::default();
                            match depth {
                                0 => viewport_verse_browser(
                                    ui,
                                    nav,
                                    contents,
                                    &sender,
                                    &DashboardState::default(),
                                    rect,
                                    center,
                                    manager,
                                    &unresolved,
                                ),
                                1 => viewport_fractal_browser(
                                    ui,
                                    nav,
                                    contents,
                                    &sender,
                                    rect,
                                    center,
                                    manager,
                                    &unresolved,
                                ),
                                _ => viewport_petal_browser(
                                    ui,
                                    nav,
                                    contents,
                                    &sender,
                                    rect,
                                    center,
                                    manager,
                                    &unresolved,
                                ),
                            }
                        });
                    },
                );
                crate::ui_shell::layout_policy::observations(&ctx)
            };
            frame(vec![], &mut nav, &mut manager, &hierarchy);
            let observed = frame(vec![], &mut nav, &mut manager, &hierarchy);
            let options = observed
                .controls
                .iter()
                .find(|control| {
                    control.clickable
                        && control.rect.width() <= 34.0
                        && control.rect.height() >= 26.0
                        && control.rect.height() <= 30.0
                        && control.clip.contains_rect(control.hit)
                })
                .expect("Options remains visible for unresolved/read-only current role")
                .hit;
            for pressed in [true, false] {
                frame(
                    vec![
                        egui::Event::PointerMoved(options.center()),
                        egui::Event::PointerButton {
                            pos: options.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    &mut nav,
                    &mut manager,
                    &hierarchy,
                );
            }
            let expected = [
                EntitySettingsType::Verse,
                EntitySettingsType::Fractal,
                EntitySettingsType::Petal,
            ][depth];
            assert!(
                matches!(&manager.active_dialog,ActiveDialog::EntitySettings {entity_type,..} if *entity_type==expected),
                "Actual Options pointer click opens target details without current Manager role"
            );
            assert!(
                receiver.try_recv().is_err(),
                "Opening Options never queues a role mutation"
            );
            assert!(observed
                .scrolls
                .iter()
                .all(|scroll| scroll.viewport.height() <= size.y
                    && scroll.viewport.width() <= size.x));
            if depth == 0 {
                assert!(
                    observed
                        .scrolls
                        .iter()
                        .any(|scroll| scroll.content_size.y > scroll.viewport.height()),
                    "A long card list has a bounded scroll viewport"
                );
                manager.close_dialog();
                let empty = VerseManager::default();
                frame(vec![], &mut nav, &mut manager, &empty);
                let empty_frame = frame(vec![], &mut nav, &mut manager, &empty);
                let join = empty_frame
                    .controls
                    .iter()
                    .find(|control| {
                        (control.rect.width() - 120.0).abs() < 0.1
                            && (control.rect.height() - 26.0).abs() < 0.1
                    })
                    .expect("Empty browser offers Join")
                    .hit;
                for pressed in [true, false] {
                    frame(
                        vec![
                            egui::Event::PointerMoved(join.center()),
                            egui::Event::PointerButton {
                                pos: join.center(),
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        &mut nav,
                        &mut manager,
                        &empty,
                    );
                }
                assert!(
                    matches!(manager.active_dialog, ActiveDialog::JoinDialog { .. }),
                    "Join opens the real existing flow"
                );
            }
        }
    }
}
