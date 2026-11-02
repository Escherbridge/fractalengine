//! Topbar area manager (FR-4): renders the top toolbar — transform-tool
//! switcher, deselect, and the Data/Tools/Settings/Maps cluster. The tool data
//! (`TOOL_DEFS`), the `InputContext` temp-data stash, and `mode_button_fill`
//! stay single-source in `panels::toolbar`; this manager calls them. Tool
//! activation routes through `ToolState::activate` — the single writer. See
//! `fe-ui/src/ui_shell/AGENTS.md` §topbar. The bar is also the frameless
//! window's title bar (brand, drag-to-move, min/max/close, edge resize grips) —
//! §window-chrome in the same file.

use bevy::ecs::system::NonSendMarker;
use bevy::math::CompassOctant;
use bevy::prelude::{
    Commands, Entity, MessageReader, Messages, Query, Res, ResMut, Resource, SystemSet, With,
};
use bevy::window::{PrimaryWindow, Window, WindowCloseRequested};
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::panels::toolbar::{
    mode_button_fill, stash_input_context, tool_tooltip_text, InputContext, TOOL_DEFS,
};
use crate::plugin::{LocalUserRole, ToolState};
use crate::theme;
use crate::ui_shell::left_sidebar::LeftSidebarState;
use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

/// Topbar height in logical points (also the title-bar drag strip).
pub const TOPBAR_HEIGHT: f32 = 40.0;
/// Width of the edge band that resizes the frameless window.
pub const RESIZE_BAND: f32 = 5.0;
/// Caption-button size (minimize / maximize / close), platform-sized.
const CAPTION_BUTTON_SIZE: egui::Vec2 = egui::vec2(46.0, 28.0);
const BRAND_ICON_SIZE: f32 = 22.0;

/// A window action the title bar asks for; `apply_window_chrome` performs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowChromeRequest {
    Minimize,
    ToggleMaximize,
    Maximize,
    Restore,
    SetSize {
        width: u32,
        height: u32,
    },
    /// Routed through `WindowCloseRequested` — the OS close path, never a hard exit.
    Close,
    StartMove,
    StartResize(CompassOctant),
}

#[derive(Debug, Clone, Copy)]
pub struct WindowChromeBusy;

/// The host binary's app-icon pixels (straight RGBA8), shown at the bar's left.
#[derive(Debug, Clone)]
pub struct BrandIcon {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Topbar-owned state: the pending window-chrome request, the OS maximized
/// mirror, and the brand mark (pixels + lazily uploaded egui texture).
#[derive(Resource, Default)]
pub struct TopbarState {
    pub close_pending: bool,
    close_confirmed: bool,
    /// Set by the bar this frame, consumed by `apply_window_chrome`.
    chrome_request: Option<WindowChromeRequest>,
    /// Mirror of the OS window's maximized state (restore glyph, grips off).
    pub maximized: bool,
    pub minimized: bool,
    /// Supplied by the binary (`TopbarState::with_brand_icon`); `None` = name only.
    pub brand_icon: Option<BrandIcon>,
    brand_texture: Option<egui::TextureHandle>,
}

impl TopbarState {
    /// Accept one window request without replacing an operation already queued.
    pub fn try_request_chrome(
        &mut self,
        request: WindowChromeRequest,
    ) -> Result<(), WindowChromeBusy> {
        if self.chrome_request.is_some() {
            return Err(WindowChromeBusy);
        }
        self.chrome_request = Some(request);
        Ok(())
    }

    fn request_from_ui(&mut self, request: WindowChromeRequest) {
        if self.try_request_chrome(request).is_err() {
            bevy::log::warn!("window request already pending; UI request refused");
        }
    }
    /// State carrying the app icon the bar shows beside the app name.
    pub fn with_brand_icon(icon: BrandIcon) -> Self {
        Self {
            brand_icon: Some(icon),
            ..Self::default()
        }
    }

    /// The brand texture, uploaded to egui once on first use.
    fn brand_texture_id(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.brand_texture.is_none() {
            let icon = self.brand_icon.as_ref()?;
            let image =
                egui::ColorImage::from_rgba_unmultiplied([icon.width, icon.height], &icon.rgba);
            self.brand_texture =
                Some(ctx.load_texture("fe-brand-icon", image, egui::TextureOptions::LINEAR));
        }
        self.brand_texture.as_ref().map(egui::TextureHandle::id)
    }
}

/// Where the bar's widget clusters sit this frame — everything else on the
/// bar is free title-bar space that drags the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitleBarLayout {
    pub bar: egui::Rect,
    /// Left cluster (brand excluded — it drags like a native caption) spans
    /// `[left_widgets_start_x, left_widgets_end_x)`.
    pub left_widgets_start_x: f32,
    pub left_widgets_end_x: f32,
    /// The right cluster (sections + caption buttons) starts here.
    pub right_widgets_start_x: f32,
}

impl TitleBarLayout {
    /// True when `pos` is on the bar but over none of its widgets.
    pub fn is_free_space(&self, pos: egui::Pos2) -> bool {
        let over_left = pos.x >= self.left_widgets_start_x && pos.x < self.left_widgets_end_x;
        let over_right = pos.x >= self.right_widgets_start_x;
        self.bar.contains(pos) && !over_left && !over_right
    }
}

/// The title-bar gesture for this frame: a double-click on free space toggles
/// maximize, a primary drag that STARTED on free space (outside the resize
/// band) moves the window. A press that began on any widget never moves it.
pub fn title_bar_gesture(
    layout: &TitleBarLayout,
    gesture_origin: Option<egui::Pos2>,
    in_resize_band: bool,
    double_clicked: bool,
    drag_started: bool,
) -> Option<WindowChromeRequest> {
    let origin = gesture_origin?;
    if !layout.is_free_space(origin) || in_resize_band {
        return None;
    }
    if double_clicked {
        Some(WindowChromeRequest::ToggleMaximize)
    } else if drag_started {
        Some(WindowChromeRequest::StartMove)
    } else {
        None
    }
}

/// The resize direction for a pointer at `pos` on a window `screen`, or `None`
/// outside the `band`-wide edge strips (corners win over edges).
pub fn resize_direction_at(
    pos: egui::Pos2,
    screen: egui::Rect,
    band: f32,
) -> Option<CompassOctant> {
    if !screen.contains(pos) {
        return None;
    }
    let west = pos.x < screen.min.x + band;
    let east = pos.x >= screen.max.x - band;
    let north = pos.y < screen.min.y + band;
    let south = pos.y >= screen.max.y - band;
    match (north, south, west, east) {
        (true, _, true, _) => Some(CompassOctant::NorthWest),
        (true, _, _, true) => Some(CompassOctant::NorthEast),
        (_, true, true, _) => Some(CompassOctant::SouthWest),
        (_, true, _, true) => Some(CompassOctant::SouthEast),
        (true, _, _, _) => Some(CompassOctant::North),
        (_, true, _, _) => Some(CompassOctant::South),
        (_, _, true, _) => Some(CompassOctant::West),
        (_, _, _, true) => Some(CompassOctant::East),
        _ => None,
    }
}

/// The viewport's pointer-routing rect: `viewport` minus the edge resize bands
/// (whole `viewport` while maximized — no grips), so the Bevy-side click
/// router never treats a resize press as a pick / pen anchor / gimbal grab.
pub fn viewport_outside_resize_bands(
    viewport: egui::Rect,
    screen: egui::Rect,
    maximized: bool,
) -> egui::Rect {
    if maximized {
        viewport
    } else {
        viewport.intersect(screen.shrink(RESIZE_BAND))
    }
}

/// Interaction geometry excludes panel grab bands without shrinking visual viewport metrics.
pub fn viewport_interaction_rect(
    viewport: egui::Rect,
    screen: egui::Rect,
    maximized: bool,
    grab_radius: f32,
    results_open: bool,
) -> egui::Rect {
    let mut input = viewport_outside_resize_bands(viewport, screen, maximized);
    if viewport.min.x > screen.min.x {
        input.min.x = input.min.x.max(viewport.min.x + grab_radius);
    }
    if viewport.max.x < screen.max.x {
        input.max.x = input.max.x.min(viewport.max.x - grab_radius);
    }
    if results_open {
        input.max.y = input.max.y.min(viewport.max.y - grab_radius);
    }
    input
}

/// The OS resize cursor for a drag direction.
fn resize_cursor(direction: CompassOctant) -> egui::CursorIcon {
    match direction {
        CompassOctant::North => egui::CursorIcon::ResizeNorth,
        CompassOctant::South => egui::CursorIcon::ResizeSouth,
        CompassOctant::East => egui::CursorIcon::ResizeEast,
        CompassOctant::West => egui::CursorIcon::ResizeWest,
        CompassOctant::NorthEast => egui::CursorIcon::ResizeNorthEast,
        CompassOctant::NorthWest => egui::CursorIcon::ResizeNorthWest,
        CompassOctant::SouthEast => egui::CursorIcon::ResizeSouthEast,
        CompassOctant::SouthWest => egui::CursorIcon::ResizeSouthWest,
    }
}

/// Edge/corner resize grips for the frameless window: a resize cursor in the
/// edge band and a `StartResize` request on a primary press there. Runs after
/// every panel so its cursor wins; inert while maximized.
pub fn render_window_resize_grips(ctx: &egui::Context, topbar: &mut TopbarState) {
    if topbar.maximized {
        return;
    }
    let screen = ctx.viewport_rect();
    let (hover, pressed, origin) = ctx.input(|i| {
        (
            i.pointer.hover_pos(),
            i.pointer.primary_pressed(),
            i.pointer.press_origin(),
        )
    });
    if let Some(direction) = hover.and_then(|pos| resize_direction_at(pos, screen, RESIZE_BAND)) {
        ctx.set_cursor_icon(resize_cursor(direction));
    }
    if pressed {
        if let Some(direction) =
            origin.and_then(|pos| resize_direction_at(pos, screen, RESIZE_BAND))
        {
            topbar.request_from_ui(WindowChromeRequest::StartResize(direction));
        }
    }
}

/// Which caption button to draw.
#[derive(Clone, Copy)]
enum CaptionGlyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// One painted caption button (vector glyph — independent of font coverage).
fn caption_button(ui: &mut egui::Ui, glyph: CaptionGlyph, tooltip: &str) -> egui::Response {
    let size = if ui.ctx().viewport_rect().width() < 760.0 {
        egui::vec2(24.0, 28.0)
    } else {
        CAPTION_BUTTON_SIZE
    };
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.button_named(ui, tooltip).on_hover_text(tooltip);
    let painter = ui.painter_at(rect);
    let hovered = response.hovered();
    let close = matches!(glyph, CaptionGlyph::Close);
    if hovered {
        let fill = if close {
            theme::BG_WINDOW_CLOSE_HOVER
        } else {
            theme::BG_BUTTON
        };
        painter.rect_filled(rect, 2.0, fill);
    }
    let color = if hovered && close {
        egui::Color32::WHITE
    } else {
        theme::TEXT_BRIGHT
    };
    let stroke = egui::Stroke::new(1.2_f32, color);
    let center = rect.center();
    let half = 5.0_f32;
    match glyph {
        CaptionGlyph::Minimize => {
            painter.line_segment(
                [
                    center + egui::vec2(-half, 0.0),
                    center + egui::vec2(half, 0.0),
                ],
                stroke,
            );
        }
        CaptionGlyph::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(2.0 * half, 2.0 * half)),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        CaptionGlyph::Restore => {
            let size = egui::vec2(8.0, 8.0);
            let back = egui::Rect::from_min_size(center + egui::vec2(-half + 2.0, -half), size);
            let front = egui::Rect::from_min_size(center + egui::vec2(-half, -half + 2.0), size);
            painter.rect_stroke(back, 0.0, stroke, egui::StrokeKind::Middle);
            painter.rect_filled(front, 0.0, theme::BG_TOOLBAR);
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Middle);
        }
        CaptionGlyph::Close => {
            painter.line_segment(
                [
                    center + egui::vec2(-half, -half),
                    center + egui::vec2(half, half),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-half, half),
                    center + egui::vec2(half, -half),
                ],
                stroke,
            );
        }
    }
    response
}

/// Minimize / maximize-restore / close, laid right-to-left (close outermost).
fn caption_buttons(ui: &mut egui::Ui, topbar: &mut TopbarState) {
    if caption_button(ui, CaptionGlyph::Close, "Close FractalEngine").clicked() {
        topbar.request_from_ui(WindowChromeRequest::Close);
    }
    let (glyph, tooltip) = if topbar.maximized {
        (CaptionGlyph::Restore, "Restore down")
    } else {
        (CaptionGlyph::Maximize, "Maximize")
    };
    if caption_button(ui, glyph, tooltip).clicked() {
        topbar.request_from_ui(WindowChromeRequest::ToggleMaximize);
    }
    if caption_button(ui, CaptionGlyph::Minimize, "Minimize").clicked() {
        topbar.request_from_ui(WindowChromeRequest::Minimize);
    }
}

/// App icon + name at the bar's left edge (non-interactive: it drags the window).
fn brand(ui: &mut egui::Ui, topbar: &mut TopbarState) {
    if let Some(texture) = topbar.brand_texture_id(ui.ctx()) {
        ui.add(egui::Image::new((
            texture,
            egui::vec2(BRAND_ICON_SIZE, BRAND_ICON_SIZE),
        )));
    }
    if ui.ctx().viewport_rect().width() >= 760.0 {
        ui.add(
            egui::Label::new(
                egui::RichText::new("FractalEngine")
                    .color(theme::TEXT_STRONG)
                    .strong(),
            )
            .selectable(false),
        );
    }
}

/// Bevy system: perform the bar's pending window request on the primary
/// window and mirror its maximized state. Main-thread only (`NonSendMarker`):
/// winit window queries must not run off the event-loop thread.
pub fn apply_window_chrome(
    mut topbar: ResMut<TopbarState>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    close_requests: Option<ResMut<Messages<WindowCloseRequested>>>,
    _main_thread: NonSendMarker,
    mut commands: Commands,
) {
    let Ok((entity, mut window)) = windows.single_mut() else {
        return;
    };
    if topbar.close_confirmed {
        topbar.close_confirmed = false;
        commands.entity(entity).despawn();
        return;
    }
    let os_state = bevy::winit::WINIT_WINDOWS.with_borrow(|winit| {
        winit
            .get_window(entity)
            .map(|w| (w.is_maximized(), w.is_minimized().unwrap_or(false)))
    });
    if let Some((maximized, minimized)) = os_state {
        topbar.minimized = minimized;
        if topbar.maximized != maximized {
            topbar.maximized = maximized;
        }
    }
    // Read through `Deref` first so an idle frame never flags `TopbarState` changed.
    let Some(request) = topbar.chrome_request else {
        return;
    };
    topbar.chrome_request = None;
    match request {
        WindowChromeRequest::Minimize => window.set_minimized(true),
        WindowChromeRequest::Maximize => {
            window.set_minimized(false);
            window.set_maximized(true);
            topbar.maximized = true;
        }
        WindowChromeRequest::Restore => {
            window.set_minimized(false);
            window.set_maximized(false);
            topbar.maximized = false;
        }
        WindowChromeRequest::SetSize { width, height } => {
            window.set_minimized(false);
            window.set_maximized(false);
            topbar.maximized = false;
            window.resolution.set_physical_resolution(width, height);
        }
        WindowChromeRequest::ToggleMaximize => {
            let maximize = !topbar.maximized;
            window.set_maximized(maximize);
            topbar.maximized = maximize;
        }
        WindowChromeRequest::StartMove => window.start_drag_move(),
        WindowChromeRequest::StartResize(direction) => window.start_drag_resize(direction),
        // The OS close button's own path: `close_when_requested` → window
        // despawn → `exit_on_all_closed` → AppExit (§window-chrome).
        WindowChromeRequest::Close => match close_requests {
            Some(mut messages) => {
                messages.write(WindowCloseRequested { window: entity });
            }
            None => bevy::log::warn!("close requested but WindowCloseRequested is not registered"),
        },
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowChromeApply;

/// Route OS, caption, and QA close requests through one confirmation.
pub fn receive_close_requests(
    mut requests: MessageReader<WindowCloseRequested>,
    mut topbar: ResMut<TopbarState>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    let Ok(primary) = windows.single() else {
        return;
    };
    if requests.read().any(|request| request.window == primary) {
        topbar.close_pending = true;
    }
}

pub fn render_close_confirmation(
    mut contexts: bevy_egui::EguiContexts,
    mut topbar: ResMut<TopbarState>,
    ui_mgr: Res<crate::actions::UiManager>,
    settings: Res<crate::settings::AppSettings>,
    session: Res<crate::settings::SettingsSession>,
) {
    if !topbar.close_pending {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Modal::new(egui::Id::new("close-fractalengine-confirmation")).show(ctx, |ui| {
        ui.heading("Close FractalEngine?");
        ui.label("Close this window and end the current session?");
        if matches!(
            &ui_mgr.active_dialog,
            crate::dialogs::ActiveDialog::PetalManifest { dirty: true, .. }
        ) {
            ui.label("Unsaved Petal requirements will be discarded. Cancel to keep editing.");
        }
        if session.persistence_available && !session.is_saved(&settings) {
            ui.label("Local settings have not finished saving. Cancel to wait or retry saving.");
        }
        ui.horizontal(|ui| {
            if ui.button("Cancel close").observed(ui).clicked() {
                topbar.close_pending = false;
            }
            if ui.button("Close FractalEngine").observed(ui).clicked() {
                topbar.close_pending = false;
                topbar.close_confirmed = true;
            }
        });
    });
}

fn tool_buttons(
    ui: &mut egui::Ui,
    tool: &mut ToolState,
    right: &mut RightSidebarState,
    role: &LocalUserRole,
) {
    for def in &TOOL_DEFS {
        let active = tool.active_tool == def.tool;
        // tool_inspector_ux_20260719 (FR-1): active MODE reads via
        // luminance, not a saturated-blue hue (ui_ux.md §1).
        let caption = format!("{} {}", def.glyph, def.name);
        let btn = egui::Button::new(caption).fill(mode_button_fill(active));
        let allowed = !def.tool.requires_edit() || role.can_edit();
        if ui.add_enabled_ui(allowed, |ui| super::layout_policy::action(ui, btn, def.name)).inner
            .on_hover_text(tool_tooltip_text(def))
            .on_disabled_hover_text("Editor access is required to edit objects or terrain in the current scope. Select and Measure remain available.")
            .clicked()
        {
            // D1/D9: one activation writer — re-press toggles to
            // Select, and the options reveal is its job, not ours.
            tool.activate(def.tool, right);
        }
    }
}

fn section_buttons(
    ui: &mut egui::Ui,
    gis_panel: &mut crate::gis::GisPanelState,
    right: &mut RightSidebarState,
) {
    if ui
        .add(egui::Button::new("\u{1F5FA} Data").fill(if gis_panel.open {
            theme::BG_BUTTON_ACTIVE
        } else {
            theme::BG_BUTTON
        }))
        .named(ui, "Toggle Data panel")
        .on_hover_text("Query nodes and layers, export for BI")
        .clicked()
    {
        gis_panel.open = !gis_panel.open;
    }

    // D7: "Tools" was the PathTools reveal; that section is
    // retired, so the button is the Options surface's toggle —
    // one label for "the active tool's settings", whatever the
    // active tool is.
    if ui
        .add(egui::Button::new("\u{1F527} Options").fill(
            if right.is_active(RightSidebarSection::Options) {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            },
        ))
        .named(ui, "Toggle tool Options section")
        .on_hover_text("Settings for the active tool (pen curves, brush, stamping)")
        .clicked()
    {
        right.toggle(RightSidebarSection::Options);
    }

    // FR-1 (D-A10): Settings is a one-at-a-time right-sidebar
    // section now, not a floating modal — toggle it like the
    // other tool surfaces.
    if ui
        .add(egui::Button::new("\u{2699} Settings").fill(
            if right.is_active(RightSidebarSection::Settings) {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            },
        ))
        .named(ui, "Toggle Settings section")
        .on_hover_text("Application settings (render distance, mesh budget, ...)")
        .clicked()
    {
        right.toggle(RightSidebarSection::Settings);
    }

    // FR-2 (D-A10): Maps (map manager) is a right-sidebar section
    // too; the section self-seeds its data + refresh on open.
    if ui
        .add(egui::Button::new("\u{1F4E6} Maps").fill(
            if right.is_active(RightSidebarSection::Maps) {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            },
        ))
        .named(ui, "Toggle Maps section")
        .on_hover_text("Manage petal maps")
        .clicked()
    {
        right.toggle(RightSidebarSection::Maps);
    }
}

fn help_button(ui: &mut egui::Ui, keyboard_toggle: bool) {
    let response =
        super::layout_policy::action(ui, egui::Button::new("?"), "Open keyboard and pointer help")
            .on_hover_text("Keyboard and pointer controls (?)");
    let id = egui::Id::new("fractalengine_shortcut_help");
    if keyboard_toggle {
        if egui::Popup::is_id_open(ui.ctx(), id) {
            egui::Popup::close_id(ui.ctx(), id);
        } else {
            egui::Popup::open_id(ui.ctx(), id);
        }
    }
    egui::Popup::from_toggle_button_response(&response)
        .id(id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_max_width(420.0_f32.min((ui.ctx().viewport_rect().width() - 32.0).max(160.0)));
            egui::ScrollArea::vertical().max_height((ui.ctx().viewport_rect().height() - 100.0).max(120.0)).show(ui, |ui| {
                ui.heading("Keyboard and pointer controls");
                for def in &TOOL_DEFS {
                    ui.label(format!("{} - {}", def.key, def.name));
                }
                ui.separator();
                for text in [
                    "Middle drag: orbit; Shift + middle drag: pan; wheel: zoom",
                    "F: frame selection; Delete: request object deletion",
                    "Ctrl/Cmd+B: hierarchy; Ctrl/Cmd+Shift+B: inspector sidebar",
                    "Ctrl/Cmd+Z: Undo transforms / one point; Ctrl/Cmd+Shift+Z: Redo",
                    "H: hide selected object; Shift+H: show branch; Alt+H: isolate selection",
                    "Escape: cancel the current step; right-click: cancel gesture or open menu",
                    "Shortcuts pause while typing or using a popup. Editing requires Editor access.",
                ] {
                    ui.add(egui::Label::new(text).wrap());
                }
                if super::layout_policy::action(ui, egui::Button::new("Close help"), "Close keyboard and pointer help").clicked() {
                    ui.close();
                }
            });
        });
}

/// Renders the top toolbar. Migrated verbatim from `panels::toolbar::top_toolbar`
/// (FR-4). The former "Tools" button is the **Options** toggle since D7 — the
/// `PathTools` section it used to reveal is retired into the Options
/// dispatcher's Pen arm. Section buttons here write `RightSidebarState::toggle`
/// directly rather than `UiAction::RevealSection`: they are TOGGLES (re-press
/// closes), and the addressable reveal action is deliberately idempotent.
#[allow(clippy::too_many_arguments)]
pub fn render_topbar(
    ctx: &egui::Context,
    topbar: &mut TopbarState,
    tool: &mut ToolState,
    node_mgr: &mut crate::node_manager::NodeManager,
    gis_panel: &mut crate::gis::GisPanelState,
    left: &mut LeftSidebarState,
    right: &mut RightSidebarState,
    role: &LocalUserRole,
) {
    let keyboard_owned = ctx.wants_keyboard_input() || egui::Popup::is_any_open(ctx);
    let help_id = egui::Id::new("fractalengine_shortcut_help");
    let help_keyboard_toggle = !ctx.wants_keyboard_input()
        && (!egui::Popup::is_any_open(ctx) || egui::Popup::is_id_open(ctx, help_id))
        && ctx.input(|input| {
            !input.modifiers.command
                && !input.modifiers.alt
                && input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Text(text) if text == "?"))
        });
    // FR-3 (D-A11): keyboard shortcut for the user-sticky left-sidebar toggle
    // (Ctrl/Cmd+B). Flips session-scoped intent; the manager honors it verbatim.
    if !keyboard_owned
        && ctx.input(|i| {
            i.key_pressed(egui::Key::B)
                && i.modifiers.command
                && !i.modifiers.shift
                && !i.modifiers.alt
        })
    {
        left.user_intent = !left.user_intent;
    }

    egui::TopBottomPanel::top("toolbar")
        .exact_height(TOPBAR_HEIGHT)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_TOOLBAR)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ctx, |ui| {
            // Title-bar surface: registered BEFORE the widgets so each widget
            // wins egui's click hit test; `title_bar_gesture` then admits only
            // presses that began on free space (§window-chrome).
            let bar_rect = ui.max_rect().expand2(egui::vec2(8.0, 6.0));
            let bar = ui.interact(
                bar_rect,
                ui.id().with("title_bar"),
                egui::Sense::click_and_drag(),
            );
            ui.ctx().accesskit_node_builder(bar.id, |node| {
                node.set_role(egui::accesskit::Role::TitleBar)
            });
            let bar = bar.named(ui, "FractalEngine title bar");
            let mut layout = TitleBarLayout {
                bar: bar_rect,
                left_widgets_start_x: bar_rect.max.x,
                left_widgets_end_x: bar_rect.max.x,
                right_widgets_start_x: bar_rect.max.x,
            };
            ui.horizontal_centered(|ui| {
                brand(ui, topbar);
                ui.separator();
                layout.left_widgets_start_x = ui.cursor().min.x;

                // FR-3: explicit left-sidebar toggle (the only reveal/hide path
                // now that auto-collapse is gone). Active fill = currently open.
                if super::layout_policy::action(
                    ui,
                    egui::Button::new("\u{2630}").fill(mode_button_fill(left.user_intent)),
                    "Toggle hierarchy sidebar",
                )
                .on_hover_text("Toggle left sidebar (Ctrl+B)")
                .clicked()
                {
                    left.user_intent = !left.user_intent;
                }
                ui.separator();

                let compact = ctx.viewport_rect().width() < 1160.0;
                if compact {
                    ui.menu_button("Tools", |ui| {
                        tool_buttons(ui, tool, right, role);
                        if node_mgr.selected_entity().is_some()
                            && ui.button("Deselect").observed(ui).clicked()
                        {
                            node_mgr.deselect();
                        }
                    })
                    .response
                    .observed(ui);
                } else {
                    tool_buttons(ui, tool, right, role);
                }
                // Publish the frame's input context for the egui-side surfaces
                // that cannot read Bevy resources (viewport hint + right-click).
                stash_input_context(
                    ui.ctx(),
                    InputContext {
                        active_tool: tool.active_tool,
                        gesture_active: node_mgr.gesture_active,
                    },
                );

                ui.separator();

                if !compact
                    && node_mgr.selected_entity().is_some()
                    && ui
                        .add(egui::Button::new("\u{2715} Deselect").fill(theme::BG_DANGER))
                        .named(ui, "Deselect object")
                        .clicked()
                {
                    node_mgr.deselect();
                }
                layout.left_widgets_end_x = ui.cursor().min.x;

                let right_cluster =
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        caption_buttons(ui, topbar);
                        ui.separator();
                        help_button(ui, help_keyboard_toggle);

                        if compact {
                            ui.menu_button("Panels", |ui| section_buttons(ui, gis_panel, right))
                                .response
                                .observed(ui);
                        } else {
                            section_buttons(ui, gis_panel, right);
                        }
                        ui.min_rect().min.x
                    });
                layout.right_widgets_start_x = right_cluster.inner;
            });

            // egui clears `press_origin` on release, the frame a (double-)click
            // reports — a click never moved, so the latest pointer pos is its origin.
            let gesture_origin =
                ui.input(|i| i.pointer.press_origin().or(i.pointer.interact_pos()));
            let screen = ui.ctx().viewport_rect();
            let in_resize_band = !topbar.maximized
                && gesture_origin
                    .and_then(|pos| resize_direction_at(pos, screen, RESIZE_BAND))
                    .is_some();
            if let Some(request) = title_bar_gesture(
                &layout,
                gesture_origin,
                in_resize_band,
                bar.double_clicked(),
                bar.drag_started_by(egui::PointerButton::Primary),
            ) {
                topbar.request_from_ui(request);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{App, Update};

    use crate::panels::toolbar::{input_context, Tool};

    #[test]
    fn options_button_toggles_the_options_section_both_ways() {
        // Mirrors the (ex-"Tools") Options button arm: a toggle, not a reveal.
        let mut right = RightSidebarState::default();
        right.toggle(RightSidebarSection::Options);
        assert!(right.is_active(RightSidebarSection::Options));
        right.toggle(RightSidebarSection::Options);
        assert_eq!(
            right.requested, None,
            "re-press falls back to the selection default"
        );
    }

    #[test]
    fn tool_button_press_routes_through_the_single_activation_writer() {
        // Mirrors the button arm: no Brush special case survives here.
        let (mut tool, mut right) = (ToolState::default(), RightSidebarState::default());
        right.toggle_collapsed();
        tool.activate(Tool::Brush, &mut right);
        assert!(!right.collapsed, "explicit activation reveals Options");
        assert_eq!(tool.active_tool, Tool::Brush);
        assert!(right.is_active(RightSidebarSection::Options));
        tool.activate(Tool::Brush, &mut right);
        assert_eq!(tool.active_tool, Tool::Select, "re-press rests on Select");
        assert_eq!(right.requested, None);

        let mut bar = TitleBarHarness::new();
        bar.role.role = Some(fe_database::RoleLevel::Viewer);
        bar.frame(Vec::new());
        for def in &TOOL_DEFS {
            if def.tool.requires_edit() {
                let at = bar.text_center(&format!("{} {}", def.glyph, def.name));
                bar.click(at);
                assert_eq!(
                    bar.tool.active_tool,
                    Tool::Select,
                    "Viewer cannot activate {}",
                    def.name
                );
            }
        }
        let measure = TOOL_DEFS
            .iter()
            .find(|def| def.tool == Tool::Measure)
            .unwrap();
        bar.click(bar.text_center(&format!("{} {}", measure.glyph, measure.name)));
        assert_eq!(bar.tool.active_tool, Tool::Measure);
        bar.role.role = Some(fe_database::RoleLevel::Owner);
        bar.frame(Vec::new());
        let move_def = TOOL_DEFS.iter().find(|def| def.tool == Tool::Move).unwrap();
        bar.click(bar.text_center(&format!("{} {}", move_def.glyph, move_def.name)));
        assert_eq!(bar.tool.active_tool, Tool::Move);
        assert!(bar.right.is_active(RightSidebarSection::Options));

        let help = egui::Id::new("fractalengine_shortcut_help");
        bar.click(bar.text_center("?"));
        assert!(egui::Popup::is_id_open(&bar.ctx, help));
        bar.frame(Vec::new());
        assert!(bar
            .painted_texts
            .iter()
            .any(|(text, _)| text == "Keyboard and pointer controls"));
        for def in &TOOL_DEFS {
            assert!(bar
                .painted_texts
                .iter()
                .any(|(text, _)| text == &format!("{} - {}", def.key, def.name)));
        }
        bar.click(bar.text_center("Close help"));
        assert!(!egui::Popup::is_id_open(&bar.ctx, help));
        bar.frame(vec![egui::Event::Text("?".into())]);
        assert!(
            egui::Popup::is_id_open(&bar.ctx, help),
            "keyboard opens the same help"
        );
        let left_before = bar.left.user_intent;
        bar.frame(vec![egui::Event::Key {
            key: egui::Key::B,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
        }]);
        assert_eq!(
            bar.left.user_intent, left_before,
            "popup owns keyboard shortcuts"
        );
        bar.frame(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(!egui::Popup::is_id_open(&bar.ctx, help));

        bar.node_mgr.select(Entity::PLACEHOLDER, "selected-node");
        for width in [1024.0, 1280.0, 1920.0] {
            bar.screen = egui::vec2(width, 720.0);
            bar.frame(Vec::new());
            bar.frame(Vec::new());
            let observations = super::super::layout_policy::observations(&bar.ctx);
            let actions = observations
                .controls
                .iter()
                .filter(|control| control.clickable && control.rect.width() < width - 1.0)
                .collect::<Vec<_>>();
            for (index, left) in actions.iter().enumerate() {
                for right in actions.iter().skip(index + 1) {
                    if left.id != right.id {
                        let overlap = left.rect.intersect(right.rect);
                        assert!(
                            overlap.width() <= 0.1 || overlap.height() <= 0.1,
                            "{width}: actions overlap {:?} {:?}",
                            left.rect,
                            right.rect
                        );
                    }
                }
            }
            for control in &observations.controls {
                if control.clickable && control.rect.width() < width - 1.0 {
                    assert!(
                        control.hit.width() >= 24.0 && control.hit.height() >= 24.0,
                        "{width}: {:?}",
                        control.hit
                    );
                    assert!(
                        control.clip.contains_rect(control.rect),
                        "{width}: clipped action {:?}",
                        control.rect
                    );
                }
            }
        }
    }

    #[test]
    fn topbar_publishes_both_halves_of_the_input_context() {
        // Mirrors the stash call after the tool-button loop: the tool half from
        // `ToolState`, the gesture half from the `NodeManager` per-frame mirror.
        let ctx = egui::Context::default();
        let tool = ToolState {
            active_tool: Tool::Pen,
        };
        let node_mgr = crate::node_manager::NodeManager {
            gesture_active: true,
            ..Default::default()
        };
        stash_input_context(
            &ctx,
            InputContext {
                active_tool: tool.active_tool,
                gesture_active: node_mgr.gesture_active,
            },
        );
        let read = input_context(&ctx).expect("topbar stashed it");
        assert_eq!(read.active_tool, Tool::Pen);
        assert!(read.gesture_active);
    }

    // -- §window-chrome: the bar driven through real egui input ---------------

    const SCREEN: egui::Vec2 = egui::vec2(2400.0, 900.0);
    /// Mid-bar, far from both widget clusters on a 2400-wide window.
    const FREE_SPACE: egui::Pos2 = egui::pos2(1200.0, 20.0);
    /// Inside the close-X: the right cluster's outermost 46-px caption button.
    const CLOSE_BUTTON: egui::Pos2 = egui::pos2(2369.0, 20.0);

    /// The topbar rendered frame by frame from synthetic pointer events.
    struct TitleBarHarness {
        ctx: egui::Context,
        time: f64,
        topbar: TopbarState,
        tool: ToolState,
        node_mgr: crate::node_manager::NodeManager,
        gis_panel: crate::gis::GisPanelState,
        left: LeftSidebarState,
        right: RightSidebarState,
        role: LocalUserRole,
        screen: egui::Vec2,
        painted_texts: Vec<(String, egui::Rect)>,
    }

    impl TitleBarHarness {
        fn new() -> Self {
            let mut harness = Self {
                ctx: egui::Context::default(),
                time: 0.0,
                topbar: TopbarState::default(),
                tool: ToolState::default(),
                node_mgr: crate::node_manager::NodeManager::default(),
                gis_panel: crate::gis::GisPanelState::default(),
                left: LeftSidebarState::default(),
                right: RightSidebarState::default(),
                role: LocalUserRole {
                    role: Some(fe_database::RoleLevel::Owner),
                },
                screen: SCREEN,
                painted_texts: Vec::new(),
            };
            harness.frame(Vec::new()); // lay out once: hit tests use last frame's rects
            harness
        }

        /// Run one frame; returns the chrome request the bar raised in it.
        fn frame(&mut self, events: Vec<egui::Event>) -> Option<WindowChromeRequest> {
            self.time += 0.05;
            super::super::layout_policy::begin_frame(&self.ctx, true);
            super::super::layout_policy::apply_style(&self.ctx);
            let modifiers = events
                .iter()
                .rev()
                .find_map(|event| match event {
                    egui::Event::Key { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or(egui::Modifiers::NONE);
            let raw = egui::RawInput {
                modifiers,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let Self {
                ctx,
                topbar,
                tool,
                node_mgr,
                gis_panel,
                left,
                right,
                role,
                ..
            } = self;
            let output = ctx.run(raw, |ctx| {
                render_topbar(ctx, topbar, tool, node_mgr, gis_panel, left, right, role);
                render_window_resize_grips(ctx, topbar);
            });
            fn collect(
                shape: &egui::epaint::Shape,
                clip: egui::Rect,
                texts: &mut Vec<(String, egui::Rect)>,
            ) {
                match shape {
                    egui::epaint::Shape::Text(text) => {
                        let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                        if clip.contains_rect(bounds) {
                            texts.push((text.galley.text().to_owned(), bounds));
                        }
                    }
                    egui::epaint::Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect(shape, clip, texts);
                        }
                    }
                    _ => {}
                }
            }
            self.painted_texts.clear();
            for shape in &output.shapes {
                collect(&shape.shape, shape.clip_rect, &mut self.painted_texts);
            }
            self.topbar.chrome_request.take()
        }

        fn text_center(&self, caption: &str) -> egui::Pos2 {
            self.painted_texts
                .iter()
                .find(|(text, _)| text == caption)
                .unwrap_or_else(|| {
                    panic!("missing visible text {caption}: {:?}", self.painted_texts)
                })
                .1
                .center()
        }

        fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }
        }

        /// Hover, press at `from`, drag 60 px right, release; every request raised.
        fn press_drag_release(&mut self, from: egui::Pos2) -> Vec<WindowChromeRequest> {
            let to = from + egui::vec2(60.0, 0.0);
            [
                vec![egui::Event::PointerMoved(from)],
                vec![Self::button(from, true)],
                vec![egui::Event::PointerMoved(from + egui::vec2(20.0, 0.0))],
                vec![egui::Event::PointerMoved(to)],
                vec![Self::button(to, false)],
            ]
            .into_iter()
            .filter_map(|events| self.frame(events))
            .collect()
        }

        /// Hover then click (press + release, no movement) at `at`.
        fn click(&mut self, at: egui::Pos2) -> Vec<WindowChromeRequest> {
            [
                vec![egui::Event::PointerMoved(at)],
                vec![Self::button(at, true)],
                vec![Self::button(at, false)],
            ]
            .into_iter()
            .filter_map(|events| self.frame(events))
            .collect()
        }
    }

    #[test]
    fn dragging_free_title_bar_space_moves_the_window() {
        let mut bar = TitleBarHarness::new();
        assert_eq!(
            bar.press_drag_release(FREE_SPACE),
            vec![WindowChromeRequest::StartMove]
        );
    }

    #[test]
    fn a_drag_that_starts_on_a_widget_never_moves_the_window() {
        let mut bar = TitleBarHarness::new();
        let requests = bar.press_drag_release(CLOSE_BUTTON);
        assert!(
            !requests.contains(&WindowChromeRequest::StartMove),
            "drag decision must be false over a widget: {requests:?}"
        );
        assert!(
            !requests.contains(&WindowChromeRequest::Close),
            "a drag off the close-X is not a click: {requests:?}"
        );
    }

    #[test]
    fn close_x_click_requests_the_graceful_close_path() {
        let mut bar = TitleBarHarness::new();
        assert_eq!(bar.click(CLOSE_BUTTON), vec![WindowChromeRequest::Close]);
        let mut pending = TopbarState::default();
        pending
            .try_request_chrome(WindowChromeRequest::Minimize)
            .unwrap();
        assert!(pending
            .try_request_chrome(WindowChromeRequest::Maximize)
            .is_err());
        assert_eq!(pending.chrome_request, Some(WindowChromeRequest::Minimize));
        let mut app = App::new();
        app.add_message::<WindowCloseRequested>()
            .init_resource::<TopbarState>()
            .add_systems(Update, receive_close_requests);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.world_mut()
            .write_message(WindowCloseRequested { window });
        app.update();
        assert!(app.world().resource::<TopbarState>().close_pending);
        assert!(
            app.world().get_entity(window).is_ok(),
            "a close request waits for confirmation"
        );
        app.world_mut().resource_mut::<TopbarState>().close_pending = false;
        app.update();
        assert!(
            !app.world().resource::<TopbarState>().close_pending,
            "cancel remains cancelled without another request"
        );
        app.world_mut()
            .write_message(WindowCloseRequested { window });
        app.update();
        assert!(app.world().resource::<TopbarState>().close_pending);
    }

    #[test]
    fn double_clicking_free_title_bar_space_toggles_maximize() {
        let mut bar = TitleBarHarness::new();
        let mut requests = bar.click(FREE_SPACE);
        requests.extend(bar.click(FREE_SPACE));
        assert_eq!(requests, vec![WindowChromeRequest::ToggleMaximize]);
    }

    #[test]
    fn a_press_in_the_top_edge_band_resizes_instead_of_moving() {
        let mut bar = TitleBarHarness::new();
        let requests = bar.press_drag_release(egui::pos2(FREE_SPACE.x, 2.0));
        assert_eq!(
            requests,
            vec![WindowChromeRequest::StartResize(CompassOctant::North)]
        );
        bar.topbar.maximized = true;
        assert!(
            bar.press_drag_release(egui::pos2(FREE_SPACE.x, 2.0))
                .iter()
                .all(|r| !matches!(r, WindowChromeRequest::StartResize(_))),
            "a maximized window has no resize grips"
        );
    }

    /// A press in a resize band starts a window resize, so the viewport's click
    /// router must not see it too (it would pick, deselect or drop a pen anchor);
    /// maximized windows have no grips and keep the whole viewport.
    #[test]
    fn resize_band_presses_never_reach_the_viewport_router() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        // Viewport reaching the right, bottom and left window edges (sidebars closed).
        let viewport = egui::Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(800.0, 600.0));
        let routed = viewport_outside_resize_bands(viewport, screen, false);
        let cases = [
            (egui::pos2(798.0, 300.0), false), // east band
            (egui::pos2(2.0, 300.0), false),   // west band
            (egui::pos2(400.0, 598.0), false), // south band
            (egui::pos2(798.5, 598.5), false), // south-east corner
            (egui::pos2(400.0, 300.0), true),  // interior
            (egui::pos2(6.0, 300.0), true),    // just inside the west band
        ];
        for (pos, reaches_viewport) in cases {
            assert_eq!(routed.contains(pos), reaches_viewport, "{pos:?}");
            assert_eq!(
                resize_direction_at(pos, screen, RESIZE_BAND).is_some(),
                !reaches_viewport,
                "{pos:?}: exactly one of resize / viewport owns the press"
            );
        }
        let visual = egui::Rect::from_min_max(egui::pos2(220.0, 40.0), egui::pos2(700.0, 500.0));
        let input = viewport_interaction_rect(visual, screen, false, 5.0, true);
        let context = egui::Context::default();
        let region = fe_renderer::camera::CameraPointerRegion {
            rect: input,
            pixels_per_point: 2.0,
        };
        for (point, accepted) in [
            (egui::vec2(220.0, 200.0), false),
            (egui::vec2(700.0, 200.0), false),
            (egui::vec2(400.0, 500.0), false),
            (egui::vec2(400.0, 200.0), true),
        ] {
            assert_eq!(region.accepts_pointer(point * 2.0, &context), accepted);
        }
        for point in [
            egui::vec2(input.min.x, 200.0),
            egui::vec2(input.max.x, 200.0),
            egui::vec2(400.0, input.min.y),
            egui::vec2(400.0, input.max.y),
        ] {
            assert!(
                !region.accepts_pointer(point * 2.0, &context),
                "exact grab-band endpoints stay outside scene input"
            );
        }
        assert_eq!(
            visual.width(),
            480.0,
            "input insets do not alter visual viewport geometry"
        );
        assert_eq!(
            viewport_outside_resize_bands(viewport, screen, true),
            viewport,
            "maximized: no grips, the whole viewport routes"
        );
    }

    #[test]
    fn resize_direction_covers_every_edge_and_corner() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let cases = [
            (egui::pos2(1.0, 1.0), Some(CompassOctant::NorthWest)),
            (egui::pos2(798.0, 1.0), Some(CompassOctant::NorthEast)),
            (egui::pos2(1.0, 598.0), Some(CompassOctant::SouthWest)),
            (egui::pos2(798.0, 598.0), Some(CompassOctant::SouthEast)),
            (egui::pos2(400.0, 1.0), Some(CompassOctant::North)),
            (egui::pos2(400.0, 598.0), Some(CompassOctant::South)),
            (egui::pos2(1.0, 300.0), Some(CompassOctant::West)),
            (egui::pos2(798.0, 300.0), Some(CompassOctant::East)),
            (egui::pos2(400.0, 300.0), None),
            (egui::pos2(400.0, 5.0), None),
            (egui::pos2(-1.0, 300.0), None),
        ];
        for (pos, expected) in cases {
            assert_eq!(
                resize_direction_at(pos, screen, RESIZE_BAND),
                expected,
                "{pos:?}"
            );
        }
    }
}
