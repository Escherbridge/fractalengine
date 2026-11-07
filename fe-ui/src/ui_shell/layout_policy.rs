//! Finite shell geometry and same-frame layout observations; see AGENTS.md §layout-policy.

use bevy_egui::egui;

const BASE_DIVIDER_RADIUS: f32 = 5.0;
const MIN_DIVIDER_HIT_PX: f32 = 20.0;
pub(crate) const HIERARCHY_DEFAULT_WIDTH: f32 = 220.0;
const SCROLL_WIDTH_RESERVE: f32 = 12.0;
const NORMAL_CONTROL_POINTS: f32 = 24.0;
const MIN_CONTROL_HIT_PX: f32 = 20.0;

/// Physical target and interior clearance; see AGENTS.md §divider-accessibility.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DividerMetrics {
    pub radius: f32,
    pub gutter: i8,
    clearance: f32,
}

impl DividerMetrics {
    fn from_scale(scale: f32) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 { return None; }
        let quantum = egui::emath::GUI_ROUNDING;
        let radius = (BASE_DIVIDER_RADIUS.max((MIN_DIVIDER_HIT_PX * 0.5 + 1.0) / scale) / quantum).ceil() * quantum;
        let clearance = radius + 2.0 / scale;
        let gutter = (clearance + scale.recip()).ceil();
        (radius.is_finite() && clearance.is_finite() && gutter.is_finite()
            && (clearance - radius) * scale >= 1.0 && (gutter - clearance) * scale >= 1.0
            && gutter >= 0.0 && gutter <= i8::MAX as f32)
            .then_some(Self { radius, gutter: gutter as i8, clearance })
    }

    fn horizontal_hit(self, edge: f32, scale: f32) -> Option<std::ops::RangeInclusive<f32>> {
        let scale64 = f64::from(scale);
        let low = ((f64::from(edge) - f64::from(self.radius)) * scale64).floor() / scale64;
        let high = ((f64::from(edge) + f64::from(self.radius)) * scale64).ceil() / scale64;
        let (low, high) = (low as f32, high as f32);
        (low.is_finite() && high.is_finite() && (high - low) * scale >= MIN_DIVIDER_HIT_PX
            && low >= edge - self.clearance && high <= edge + self.clearance)
            .then_some(low..=high)
    }

    pub(crate) fn side_margin(self, side: PanelSide) -> egui::Margin {
        match side {
            PanelSide::Left => egui::Margin { right: self.gutter, ..egui::Margin::ZERO },
            PanelSide::Right => egui::Margin { left: self.gutter, ..egui::Margin::ZERO },
        }
    }

    pub(crate) fn hierarchy_content_width(self) -> f32 {
        (HIERARCHY_DEFAULT_WIDTH - f32::from(self.gutter) - SCROLL_WIDTH_RESERVE).max(0.0)
    }

    /// Scope the SDK metric to the native Results owner, including unwinding.
    pub(crate) fn native_results<R>(self, ctx: &egui::Context, body: impl FnOnce() -> R) -> R {
        struct RestoreRadius(egui::Context, f32);
        impl Drop for RestoreRadius {
            fn drop(&mut self) {
                if self.0.style().interaction.resize_grab_radius_side != self.1 {
                    self.0.style_mut(|style| style.interaction.resize_grab_radius_side = self.1);
                }
            }
        }
        let restore = RestoreRadius(ctx.clone(), ctx.style().interaction.resize_grab_radius_side);
        if restore.1 != self.radius {
            ctx.style_mut(|style| style.interaction.resize_grab_radius_side = self.radius);
        }
        let result = body();
        drop(restore);
        result
    }
}

pub(crate) fn divider_metrics(ctx: &egui::Context) -> Option<DividerMetrics> {
    let scale = ctx.pixels_per_point();
    ctx.data(|data| data.get_temp::<(u64, f32, Option<DividerMetrics>)>(divider_policy_key()))
        .filter(|(pass, stored_scale, _)| *pass == ctx.cumulative_pass_nr() && *stored_scale == scale)
        .map_or_else(|| DividerMetrics::from_scale(scale), |(_, _, metrics)| metrics)
}

fn divider_policy_key() -> egui::Id { egui::Id::new("fe-divider-metrics") }

fn results_edge_key() -> egui::Id { egui::Id::new("fe-results-divider-edge") }

pub(crate) fn record_results_divider(ctx: &egui::Context, rect: egui::Rect) {
    if rect.is_finite() && rect.is_positive() {
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|data| data.insert_temp(results_edge_key(), (pass, rect)));
    }
}

fn has_results_divider(ctx: &egui::Context, visual: egui::Rect) -> bool {
    ctx.data(|data| data.get_temp::<(u64, egui::Rect)>(results_edge_key()))
        .is_some_and(|(pass, rect)| pass == ctx.cumulative_pass_nr() && rect.top() == visual.bottom())
}

/// Inset only interactive overlay contents, never the visual/pick viewport.
pub(crate) fn overlay_content_rect(ctx: &egui::Context, visual: egui::Rect) -> Option<egui::Rect> {
    let metrics = divider_metrics(ctx)?;
    let screen = ctx.viewport_rect();
    if !visual.is_finite() || !screen.is_finite() { return None; }
    let mut content = visual;
    let gutter = f32::from(metrics.gutter);
    if visual.left() > screen.left() { content.min.x += gutter; }
    if visual.right() < screen.right() { content.max.x -= gutter; }
    if has_results_divider(ctx, visual) { content.max.y -= gutter; }
    content.is_positive().then_some(content)
}

pub(crate) fn scene_interaction_rect(ctx: &egui::Context, visual: egui::Rect, maximized: bool) -> egui::Rect {
    if !visual.is_finite() || !visual.is_positive() || !ctx.viewport_rect().is_finite() {
        return egui::Rect::NOTHING;
    }
    divider_metrics(ctx).map_or(egui::Rect::NOTHING, |metrics| {
        super::topbar::viewport_interaction_rect(visual, ctx.viewport_rect(), maximized,
            metrics.clearance, has_results_divider(ctx, visual))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelSide {
    Left,
    Right,
}

#[derive(Clone, Debug)]
struct PanelGeometry {
    context: egui::Context,
    viewport: egui::Rect,
    available: egui::Rect,
    scale: f32,
    bounds: [f32; 2],
    dividers: DividerMetrics,
}

impl PartialEq for PanelGeometry {
    fn eq(&self, other: &Self) -> bool {
        self.context == other.context
            && self.viewport == other.viewport
            && self.available == other.available
            && self.scale == other.scale
            && self.bounds == other.bounds
            && self.dividers == other.dividers
    }
}

#[derive(Clone, Debug)]
struct PanelLayout {
    geometry: PanelGeometry,
    rect: egui::Rect,
    pass: u64,
}

#[derive(Clone, Debug)]
struct PanelResize {
    geometry: PanelGeometry,
    origin: egui::Pos2,
    displayed: f32,
    preview: f32,
}

/// Caller-owned gates for the existing divider; see AGENTS.md.
#[derive(Clone, Copy, Debug)]
pub struct DividerGates {
    pub app_enabled: bool,
    pub dialog_allowed: bool,
    pub close_pending: bool,
    pub quarantined: bool,
}

impl DividerGates {
    pub(crate) const ALLOWED: Self = Self {
        app_enabled: true, dialog_allowed: true, close_pending: false, quarantined: false,
    };

    fn blocker(self) -> Option<&'static str> {
        if !self.app_enabled { Some("app_disabled") }
        else if !self.dialog_allowed { Some("dialog_blocked") }
        else if self.close_pending { Some("close_pending") }
        else if self.quarantined { Some("section_quarantined") }
        else { None }
    }
}

/// Exact decision inputs at a divider boundary; see AGENTS.md.
#[derive(Clone, Copy, Debug)]
pub struct DividerEligibility {
    pub gates: DividerGates,
    pub input_focused: bool,
    pub escape_pressed: bool,
    pub popup_open: bool,
    pub modal_open: bool,
    pub primary_down: bool,
    pub wait_for_release: bool,
}

impl DividerEligibility {
    fn interaction_blocker(self) -> Option<&'static str> {
        self.gates.blocker().or({
            if !self.input_focused { Some("input_unfocused") }
            else if self.escape_pressed { Some("escape_pressed") }
            else if self.popup_open { Some("popup_open") }
            else if self.modal_open { Some("modal_open") }
            else { None }
        })
    }

    pub fn blocker(self) -> Option<&'static str> {
        self.interaction_blocker().or_else(|| self.wait_for_release.then_some("release_wait"))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DividerObservation {
    pub id: u64,
    pub side: &'static str,
    pub pass: u64,
    pub eligibility: DividerEligibility,
    pub response_enabled: bool,
}

/// Session width preference and its one owned divider gesture; see AGENTS.md.
#[derive(Clone, Debug)]
pub(crate) struct PanelWidthState {
    preferred: f32,
    drag: Option<PanelResize>,
    layout: Option<PanelLayout>,
    prepared: Option<PanelGeometry>,
    gates: DividerGates,
    wait_for_release: bool,
}

impl Default for PanelWidthState {
    fn default() -> Self {
        Self::new(320.0)
    }
}

impl PanelWidthState {
    pub(crate) const fn new(preferred: f32) -> Self {
        Self {
            preferred,
            drag: None,
            layout: None,
            prepared: None,
            gates: DividerGates::ALLOWED,
            wait_for_release: false,
        }
    }

    pub(crate) fn set_interactive(&mut self, enabled: bool) {
        self.set_gates(DividerGates { app_enabled: enabled, ..DividerGates::ALLOWED });
    }

    pub(crate) fn set_gates(&mut self, gates: DividerGates) {
        self.gates = gates;
        if gates.blocker().is_some() {
            self.suspend();
        }
    }

    pub(crate) fn quarantine(&mut self) {
        self.set_gates(DividerGates { quarantined: true, ..self.gates });
    }

    pub(crate) fn suspend(&mut self) {
        self.wait_for_release |= self.drag.is_some();
        self.drag = None;
        self.layout = None;
    }

    fn geometry(ctx: &egui::Context, bounds: [f32; 2]) -> Option<PanelGeometry> {
        let dividers = divider_metrics(ctx)?;
        let scale = ctx.pixels_per_point();
        let viewport = ctx.viewport_rect();
        let available = ctx.available_rect();
        (bounds[0].is_finite() && bounds[1].is_finite()
            && bounds[0] >= 0.0 && bounds[1] >= bounds[0]
            && viewport.is_finite() && available.is_finite()
            && scale.is_finite() && scale > 0.0)
            .then(|| PanelGeometry {
                context: ctx.clone(),
                viewport,
                available,
                scale,
                bounds,
                dividers,
            })
    }

    fn effective(&self, bounds: [f32; 2]) -> f32 {
        let width = self.drag.as_ref().map_or(self.preferred, |drag| drag.preview);
        if width.is_finite() { width.clamp(bounds[0], bounds[1]) } else { bounds[0] }
    }

    fn eligibility(&self, ctx: &egui::Context) -> DividerEligibility {
        let (input_focused, escape_pressed, primary_down) = ctx.input(|input| (
            input.focused, input.key_pressed(egui::Key::Escape), input.pointer.primary_down(),
        ));
        DividerEligibility {
            gates: self.gates, input_focused, escape_pressed, primary_down,
            popup_open: egui::Popup::is_any_open(ctx),
            modal_open: ctx.memory(|memory| memory.top_modal_layer().is_some()),
            wait_for_release: self.wait_for_release,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        side: PanelSide,
        bounds: [f32; 2],
    ) -> f32 {
        let Some(geometry) = Self::geometry(ctx, bounds) else {
            self.suspend();
            self.prepared = None;
            return 0.0;
        };
        self.prepared = Some(geometry.clone());
        let down = ctx.input(|input| input.pointer.primary_down());
        if !down { self.wait_for_release = false; }
        let fresh = self.layout.as_ref().is_some_and(|layout| {
            layout.geometry == geometry
                && layout.pass.checked_add(1) == Some(ctx.cumulative_pass_nr())
        });
        let allowed = self.eligibility(ctx).interaction_blocker().is_none();
        if !allowed || !fresh
            || self.drag.as_ref().is_some_and(|drag| drag.geometry != geometry) {
            self.suspend();
            self.wait_for_release |= down;
            return self.effective(bounds);
        }
        if self.wait_for_release { return self.effective(bounds); }
        if let Some(response) = ctx.read_response(id) {
            if self.drag.is_none() && response.drag_started_by(egui::PointerButton::Primary) {
                if let Some(origin) = ctx.input(|input| input.pointer.press_origin())
                    .filter(|origin| origin.is_finite()) {
                    let displayed = self.layout.as_ref().map_or(0.0, |layout| layout.rect.width());
                    self.drag = Some(PanelResize {
                        geometry: geometry.clone(), origin, displayed, preview: displayed,
                    });
                }
            }
            if let Some(drag) = self.drag.as_mut() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let direction = if side == PanelSide::Left { 1.0 } else { -1.0 };
                    let delta = direction * (pointer.x - drag.origin.x);
                    let requested = drag.displayed + delta;
                    if requested.is_finite() {
                        drag.preview = if delta.abs() > 0.5 / geometry.scale {
                            requested.clamp(bounds[0], bounds[1])
                        } else { drag.displayed };
                    }
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    if (drag.preview - drag.displayed).abs() > 0.5 / geometry.scale {
                        self.preferred = drag.preview;
                    }
                    self.drag = None;
                } else if !response.dragged_by(egui::PointerButton::Primary) {
                    self.suspend();
                    self.wait_for_release |= down;
                }
            }
        } else if self.drag.is_some() {
            self.suspend();
            self.wait_for_release |= down;
        }
        self.effective(bounds)
    }

    pub(crate) fn register(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        side: PanelSide,
        bounds: [f32; 2],
        rect: egui::Rect,
    ) {
        use crate::accessibility::ResponseExt;
        let Some(geometry) = self.prepared.take().filter(|prepared| {
            prepared.context == *ctx && prepared.bounds == bounds
        }) else {
            self.suspend();
            return;
        };
        let radius = geometry.dividers.radius;
        if !rect.is_finite() || !rect.is_positive() || !radius.is_finite() || radius < 0.0 {
            self.suspend();
            return;
        }
        let edge = if side == PanelSide::Left { rect.max.x } else { rect.min.x };
        let Some(horizontal) = geometry.dividers.horizontal_hit(edge, geometry.scale) else {
            self.suspend();
            return;
        };
        let strip = egui::Rect::from_x_y_ranges(horizontal, rect.y_range()).intersect(ctx.viewport_rect());
        if !strip.is_positive() || strip.width() * geometry.scale < MIN_DIVIDER_HIT_PX {
            self.suspend();
            return;
        }
        let mut builder = egui::UiBuilder::new()
            .layer_id(egui::LayerId::background()).max_rect(ctx.viewport_rect());
        let eligibility = self.eligibility(ctx);
        if eligibility.blocker().is_some() { builder = builder.disabled(); }
        let ui = egui::Ui::new(ctx.clone(), id.with("surface"), builder);
        let name = if side == PanelSide::Left { "Resize hierarchy panel" } else { "Resize right panel" };
        let response = ui.interact(strip, id, egui::Sense::drag()).named(&ui, name);
        ctx.accesskit_node_builder(id, |node| {
            node.set_role(egui::accesskit::Role::Splitter);
            node.set_orientation(egui::accesskit::Orientation::Vertical);
        });
        if response.hovered() || response.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let pass = ctx.cumulative_pass_nr();
        let observe = ctx.data(|data| data.get_temp::<LayoutObservations>(key())
            .is_some_and(|observations| observations.enabled));
        if observe {
            ctx.data_mut(|data| {
                let observations = data.get_temp_mut_or_default::<LayoutObservations>(key());
                if observations.dividers.len() < 2 {
                    observations.dividers.push(DividerObservation {
                        id: id.value(), side: if side == PanelSide::Left { "left" } else { "right" },
                        pass, eligibility, response_enabled: response.enabled(),
                    });
                }
            });
        }
        self.layout = Some(PanelLayout { geometry, rect, pass });
    }
}

#[cfg(test)]
pub(super) fn extend_panel_width_journey() {
    extend_physical_targets_journey();
    struct Panels {
        ctx: egui::Context,
        left: PanelWidthState,
        right: PanelWidthState,
        physical: f32,
        scale: f32,
        time: f64,
        visible: bool,
        side_visible: [bool; 2],
        blocked: bool,
        quarantined: bool,
        focused: bool,
        results: bool,
        topbar: super::topbar::TopbarState,
        ui: crate::actions::UiManager,
        rects: [egui::Rect; 2],
    }
    impl Panels {
        fn frame(&mut self, events: Vec<egui::Event>) {
            let ctx = self.ctx.clone();
            ctx.set_pixels_per_point(self.scale);
            self.time += 0.25;
            let _ = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                    egui::vec2(self.physical / self.scale, 720.0 / self.scale))),
                time: Some(self.time), focused: self.focused, events, ..Default::default()
            }, |ctx| {
                apply_style(ctx);
                begin_frame(ctx, true);
                let metrics = divider_metrics(ctx).unwrap();
                for side in [PanelSide::Left, PanelSide::Right] {
                    let (state, bounds, id) = if side == PanelSide::Left {
                        (&mut self.left, [120.0, left_width(ctx)], egui::Id::new("width-left"))
                    } else {
                        let max = right_width(ctx);
                        (&mut self.right, [max.min(260.0), max], egui::Id::new("width-right"))
                    };
                    let available = if side == PanelSide::Left {
                        super::left_sidebar::resize_interaction_allowed(&self.ui, self.topbar.close_pending)
                    } else { !self.topbar.close_pending };
                    if side == PanelSide::Right {
                        state.set_gates(DividerGates {
                            app_enabled: !self.blocked,
                            dialog_allowed: matches!(self.ui.active_dialog,
                                crate::dialogs::ActiveDialog::None | crate::dialogs::ActiveDialog::HexonManager { .. }),
                            close_pending: self.topbar.close_pending,
                            quarantined: false,
                        });
                        if self.quarantined { state.quarantine(); }
                    } else { state.set_interactive(!self.blocked && available); }
                    let width = state.prepare(ctx, id, side, bounds);
                    let panel = if side == PanelSide::Left {
                        egui::SidePanel::left("width-test-left")
                    } else { egui::SidePanel::right("width-test-right") };
                    let mut content = egui::Rect::NOTHING;
                    let panel = panel.resizable(false).exact_width(width)
                        .frame(egui::Frame::NONE.inner_margin(metrics.side_margin(side)))
                        .show_animated(ctx, self.visible && self.side_visible[if side == PanelSide::Left { 0 } else { 1 }],
                            |ui| { content = ui.max_rect(); });
                    if let Some(panel) = panel {
                        self.rects[if side == PanelSide::Left { 0 } else { 1 }] = panel.response.rect;
                        assert!(panel.response.rect.width() <= bounds[1] + 1.0 / self.scale);
                        state.register(ctx, id, side, bounds, panel.response.rect);
                        let observations = observations(ctx);
                        assert!(observations.dividers.len() <= 2);
                        let observed = observations.dividers.iter().find(|divider| divider.id == id.value()).unwrap();
                        assert_eq!(observed.pass, ctx.cumulative_pass_nr());
                        assert_eq!(observed.response_enabled, ctx.read_response(id).unwrap().enabled());
                        assert_eq!(observed.response_enabled, observed.eligibility.blocker().is_none());
                        let strip = ctx.read_response(id).unwrap().interact_rect;
                        assert!(strip.width() * self.scale >= MIN_DIVIDER_HIT_PX);
                        let gap = if side == PanelSide::Left {
                            strip.left() - content.right()
                        } else { content.left() - strip.right() };
                        assert!(gap * self.scale >= 1.0 - 0.001,
                            "panel contents and scrollbars remain beyond the inclusive divider strip");
                    } else { state.suspend(); }
                }
                let original_radius = ctx.style().interaction.resize_grab_radius_side;
                if self.results {
                    let mut results = super::results_dock::ResultsDockState::default();
                    let rows = (0..64).map(|row| serde_json::Value::Object(
                        (0..24).map(|column| (format!("column{column:02}"),
                            serde_json::json!(format!("row {row} column {column}")))).collect()
                    )).collect::<Vec<_>>();
                    results.complete(&rows);
                    results.open = true;
                    super::results_dock::render(ctx, &mut results,
                        &crate::navigation_manager::NavigationManager::default(),
                        &crate::verse_manager::VerseManager::default(),
                        &mut crate::node_manager::NodeManager::default(),
                        &mut crate::plugin::CameraFocusTarget::default(), &mut self.ui);
                }
                assert_eq!(ctx.style().interaction.resize_grab_radius_side, original_radius,
                    "native Results restores the SDK radius for portal/floating-window owners");
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                    let visual = ui.max_rect();
                    let content = overlay_content_rect(ctx, visual).unwrap();
                    let scene = scene_interaction_rect(ctx, visual, true);
                    if visual.left() > ctx.viewport_rect().left() {
                        assert!(content.left() > scene.left());
                    } else { assert_eq!(content.left(), visual.left()); }
                    if visual.right() < ctx.viewport_rect().right() {
                        assert!(content.right() < scene.right());
                    } else { assert_eq!(content.right(), visual.right()); }
                    assert_eq!(has_results_divider(ctx, visual), self.results);
                    if self.results {
                        assert!(content.bottom() < scene.bottom());
                        let (_, result) = ctx.data(|data|
                            data.get_temp::<(u64, egui::Rect)>(results_edge_key()).unwrap());
                        let (table, content_size, _) = super::results_dock::table_geometry(ctx);
                        assert!(content_size.x > table.width() && content_size.y > table.height(),
                            "populated Results must expose real horizontal and vertical overflow");
                        for control in observations(ctx).controls {
                            let hit = control.hit.intersect(control.clip);
                            if hit.is_positive() && control.rect.top() >= result.top() {
                                self.result_gaps(hit);
                            }
                        }
                    }
                    let region = fe_renderer::camera::CameraPointerRegion { rect: scene, pixels_per_point: self.scale };
                    for (index, id) in [egui::Id::new("width-left"), egui::Id::new("width-right")].into_iter().enumerate() {
                        if let Some(response) = ctx.read_response(id).filter(|_| self.visible && self.side_visible[index]) {
                            let hit = response.interact_rect;
                            for point in [hit.left_center(), hit.center(), hit.right_center()] {
                                assert!(!scene.contains(point), "actual divider endpoints cannot enter camera geometry");
                            }
                        }
                    }
                    {
                        use fe_renderer::scene_pointer::{ScenePointerChrome, ScenePointerStamp};
                        let screen = ctx.viewport_rect();
                        ScenePointerChrome::observe(ctx, ScenePointerStamp::new(bevy::prelude::Entity::from_bits(1),
                            bevy::prelude::UVec2::new((screen.width() * self.scale).round() as u32,
                                (screen.height() * self.scale).round() as u32), 1.0, 0));
                        ScenePointerChrome::begin_pass(ctx, 0);
                        let header = ui.horizontal(|ui| { ui.label("Scene header"); }).response;
                        ScenePointerChrome::exclude(ctx, header.rect.intersect(ui.clip_rect()));
                        ScenePointerChrome::finish_pass(ctx, scene, 0);
                    }
                    for (point, inside) in [
                        (egui::pos2(scene.left(), scene.center().y), false),
                        (egui::pos2(scene.left() + 1.0 / self.scale, scene.center().y), true),
                        (egui::pos2(scene.right(), scene.center().y), false),
                        (egui::pos2(scene.right() - 1.0 / self.scale, scene.center().y), true),
                        (egui::pos2(scene.center().x, scene.bottom()), false),
                        (egui::pos2(scene.center().x, scene.bottom() - 1.0 / self.scale), true),
                    ] {
                        let physical = bevy::prelude::Vec2::new(point.x, point.y) * self.scale;
                        assert_eq!(region.accepts_editor_pointer(physical, ctx), inside);
                    }
                });
            });
        }
        fn divider(&self, side: PanelSide) -> DividerObservation {
            let id = egui::Id::new(if side == PanelSide::Left { "width-left" } else { "width-right" });
            observations(&self.ctx).dividers.into_iter().find(|divider| divider.id == id.value()).unwrap()
        }
        fn settle(&mut self) { for _ in 0..3 { self.frame(Vec::new()); } }
        fn result_gaps(&self, hit: egui::Rect) {
            let (_, result) = self.ctx.data(|data|
                data.get_temp::<(u64, egui::Rect)>(results_edge_key()).unwrap());
            let metrics = divider_metrics(&self.ctx).unwrap();
            let clearance = metrics.radius + self.scale.recip();
            let tolerance = 0.001 / self.scale;
            assert!(hit.top() + tolerance >= result.top() + clearance,
                "actual Results hits remain below the inclusive top divider");
            if result.left() > self.ctx.viewport_rect().left() {
                assert!(hit.left() + tolerance >= result.left() + clearance,
                    "actual Results hits remain beyond the inclusive left divider");
            }
            if result.right() < self.ctx.viewport_rect().right() {
                assert!(hit.right() - tolerance <= result.right() - clearance,
                    "actual Results hits remain before the inclusive right divider");
            }
        }
        fn scrollbar(&mut self, vertical: bool) {
            let (table, _, before) = super::results_dock::table_geometry(&self.ctx);
            let point = if vertical {
                egui::pos2(table.right() - self.scale.recip(), table.center().y)
            } else { egui::pos2(table.center().x, table.bottom() - self.scale.recip()) };
            self.frame(vec![egui::Event::PointerMoved(point)]);
            self.settle();
            let hovered = self.ctx.interaction_snapshot(|snapshot|
                snapshot.hovered.iter().copied().collect::<Vec<_>>());
            let response = hovered.into_iter().filter_map(|id| self.ctx.read_response(id))
                .find(|response| {
                    let rect = response.interact_rect;
                    response.sense.senses_click() && response.sense.senses_drag()
                        && if vertical { rect.height() > rect.width() * 2.0 }
                           else { rect.width() > rect.height() * 2.0 }
                }).expect("actual hovered native scrollbar response, without SDK ID reconstruction");
            self.result_gaps(response.interact_rect);
            self.pointer(point, true);
            let end = point + if vertical { egui::vec2(0.0, 16.0) } else { egui::vec2(16.0, 0.0) };
            self.frame(vec![egui::Event::PointerMoved(end)]);
            self.pointer(end, false);
            self.settle();
            let (_, _, after) = super::results_dock::table_geometry(&self.ctx);
            assert!(if vertical { after.y != before.y } else { after.x != before.x },
                "the measured native scrollbar must actually scroll its table");
        }
        fn pointer(&mut self, point: egui::Pos2, pressed: bool) {
            self.frame(vec![egui::Event::PointerMoved(point), egui::Event::PointerButton {
                pos: point, button: egui::PointerButton::Primary, pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        fn edge(&self, side: PanelSide) -> egui::Pos2 {
            if side == PanelSide::Left { egui::pos2(self.rects[0].max.x, 120.0) }
            else { egui::pos2(self.rects[1].min.x, 120.0) }
        }
        fn drag(&mut self, side: PanelSide, delta: f32) {
            let start = self.edge(side);
            self.pointer(start, true);
            let end = start + egui::vec2(delta, 0.0);
            self.frame(vec![egui::Event::PointerMoved(end)]);
            self.pointer(end, false);
            self.settle();
        }
    }
    let mut panels = Panels {
        ctx: egui::Context::default(), left: PanelWidthState::new(220.0),
        right: PanelWidthState::default(), physical: 1280.0, scale: 1.0,
        time: 0.0, visible: true, side_visible: [true; 2], blocked: false, quarantined: false, focused: true,
        results: false, rects: [egui::Rect::NOTHING; 2],
        topbar: super::topbar::TopbarState::default(), ui: crate::actions::UiManager::default(),
    };
    panels.settle();
    panels.ui.active_dialog = crate::dialogs::ActiveDialog::PeerDebug;
    panels.frame(Vec::new());
    let decision = panels.divider(PanelSide::Right);
    assert!(!decision.eligibility.gates.dialog_allowed && !decision.response_enabled);
    assert_eq!(decision.eligibility.blocker(), Some("dialog_blocked"));
    panels.ui.active_dialog = crate::dialogs::ActiveDialog::None;
    panels.settle();
    panels.quarantined = true; panels.frame(Vec::new());
    let decision = panels.divider(PanelSide::Right);
    assert!(decision.eligibility.gates.dialog_allowed && decision.eligibility.gates.quarantined);
    assert_eq!(decision.eligibility.blocker(), Some("section_quarantined"));
    assert!(!decision.response_enabled);
    panels.quarantined = false; panels.settle();
    assert_eq!(panels.right.preferred, 320.0, "diagnostics cannot write width preference");
    for _ in 0..4 {
        assert!((panels.rects[0].width() - 220.0).abs() <= 1.0);
        assert!((panels.rects[1].width() - 320.0).abs() <= 1.0,
            "the outer right frame must not acquire a four-point stroke feedback");
        panels.frame(Vec::new());
    }
    panels.physical = 1024.0; panels.scale = 2.0; panels.settle();
    assert!(panels.rects[0].width() < 220.0 && panels.rects[1].width() < 320.0);
    panels.physical = 1280.0; panels.scale = 1.0; panels.settle();
    assert!((panels.rects[0].width() - 220.0).abs() <= 1.0);
    assert!((panels.rects[1].width() - 320.0).abs() <= 1.0);
    panels.drag(PanelSide::Left, 30.0); panels.drag(PanelSide::Right, -40.0);
    let custom = [panels.left.preferred, panels.right.preferred];
    assert!((custom[0] - 250.0).abs() <= 1.0 && (custom[1] - 360.0).abs() <= 1.0,
        "actual public divider drags update only their owned preferences");
    panels.physical = 1024.0; panels.scale = 2.0; panels.settle();
    panels.drag(PanelSide::Left, 0.0);
    assert_eq!(panels.left.preferred, custom[0], "stationary press keeps the unclamped preference");
    panels.drag(PanelSide::Left, 10.0);
    assert_eq!(panels.left.preferred, custom[0], "movement beyond the cap without a changed divider is not a resize");
    let displayed = panels.rects[0].width();
    panels.drag(PanelSide::Left, -10.0);
    assert!((panels.left.preferred - (displayed - 10.0)).abs() <= 1.0,
        "constrained dragging starts at actual displayed width, not the old preference");
    let saved = panels.left.preferred;
    let origin = panels.edge(PanelSide::Left); panels.pointer(origin, true);
    panels.frame(vec![egui::Event::PointerMoved(origin + egui::vec2(-8.0, 0.0))]);
    panels.blocked = true; panels.frame(Vec::new());
    panels.pointer(origin, false); panels.blocked = false; panels.settle();
    assert_eq!(panels.left.preferred, saved, "modal loss cancels preview without saving it");
    let origin = panels.edge(PanelSide::Left); panels.pointer(origin, true);
    let end = origin + egui::vec2(-8.0, 0.0);
    panels.frame(vec![egui::Event::PointerMoved(end)]);
    panels.topbar.close_pending = true;
    assert!(panels.ctx.memory(|memory| memory.top_modal_layer().is_none()),
        "the pending-close gate is required before the first modal paint");
    panels.pointer(end, false);
    let decision = panels.divider(PanelSide::Right);
    assert!(decision.eligibility.gates.close_pending);
    assert_eq!(decision.eligibility.blocker(), Some("close_pending"));
    assert!(!decision.response_enabled);
    assert_eq!(panels.left.preferred, saved,
        "new pending close on the held drag release cannot commit its preference");
    assert!(panels.left.drag.is_none());
    panels.topbar.close_pending = false; panels.settle();
    let origin = panels.edge(PanelSide::Left); panels.pointer(origin, true);
    panels.frame(vec![egui::Event::PointerMoved(origin + egui::vec2(-8.0, 0.0))]);
    panels.topbar.close_pending = true; panels.frame(Vec::new());
    assert!(panels.left.wait_for_release && panels.left.drag.is_none());
    panels.topbar.close_pending = false;
    panels.frame(vec![egui::Event::PointerMoved(origin + egui::vec2(-16.0, 0.0))]);
    assert!(panels.left.wait_for_release,
        "removing pending close cannot revive the canceled held gesture");
    panels.pointer(origin, false); panels.settle();
    assert!(!panels.left.wait_for_release);
    assert_eq!(panels.left.preferred, saved);
    let origin = panels.edge(PanelSide::Left); panels.pointer(origin, true);
    panels.frame(vec![egui::Event::PointerMoved(origin + egui::vec2(-8.0, 0.0))]);
    panels.frame(vec![egui::Event::Key {
        key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    let decision = panels.divider(PanelSide::Left);
    assert!(decision.eligibility.escape_pressed && !decision.response_enabled);
    assert_eq!(decision.eligibility.blocker(), Some("escape_pressed"));
    panels.pointer(origin, false); panels.settle();
    assert_eq!(panels.left.preferred, saved, "Escape cancels this owner without a save");
    let origin = panels.edge(PanelSide::Left); panels.pointer(origin, true);
    panels.frame(vec![egui::Event::PointerMoved(origin + egui::vec2(-8.0, 0.0))]);
    panels.focused = false; panels.frame(Vec::new());
    let decision = panels.divider(PanelSide::Left);
    assert!(!decision.eligibility.input_focused && !decision.response_enabled);
    assert_eq!(decision.eligibility.blocker(), Some("input_unfocused"));
    panels.focused = true; panels.frame(Vec::new());
    let decision = panels.divider(PanelSide::Left);
    assert!(decision.eligibility.input_focused && decision.eligibility.wait_for_release);
    assert_eq!(decision.eligibility.blocker(), Some("release_wait"));
    assert!(!decision.response_enabled, "restored focus cannot revive a held canceled gesture");
    panels.pointer(origin, false); panels.settle();
    assert!(panels.divider(PanelSide::Left).response_enabled);
    assert_eq!(panels.left.preferred, saved, "focus loss cannot commit a preview");
    let origin = panels.edge(PanelSide::Left); panels.pointer(origin, true);
    panels.frame(vec![egui::Event::PointerMoved(origin + egui::vec2(-8.0, 0.0))]);
    panels.scale = 1.0; panels.physical = 1280.0; panels.frame(Vec::new());
    panels.frame(vec![egui::Event::PointerMoved(origin + egui::vec2(50.0, 0.0))]);
    panels.pointer(origin, false); panels.settle();
    assert_eq!(panels.left.preferred, saved, "scale/window change cancels and suppresses the held gesture");
    assert!((panels.rects[1].width() - custom[1]).abs() <= 1.0);
    panels.visible = false; panels.settle(); panels.visible = true; panels.settle();
    assert_eq!(panels.left.preferred, saved);
    assert_eq!(panels.right.preferred, custom[1]);
    let screen = panels.ctx.viewport_rect();
    let visual = egui::Rect::from_min_max(
        egui::pos2(panels.rects[0].max.x, 0.0), egui::pos2(panels.rects[1].min.x, screen.max.y));
    let radius = divider_metrics(&panels.ctx).unwrap().radius;
    let input = super::topbar::viewport_interaction_rect(visual, screen, true, radius, false);
    assert_eq!(input.min.x, visual.min.x + radius);
    assert_eq!(input.max.x, visual.max.x - radius);
    for scale in [0.5, 0.75, 1.0, 1.25, 2.0, 4.0] {
        let metrics = DividerMetrics::from_scale(scale).unwrap();
        assert!(metrics.radius * 2.0 * scale >= MIN_DIVIDER_HIT_PX);
        assert!((f32::from(metrics.gutter) - metrics.radius) * scale >= 1.0);
    }
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY, 0.01] {
        assert!(DividerMetrics::from_scale(scale).is_none());
    }
    panels.results = true;
    for scale in [1.0, 1.25, 2.0, 1.0] {
        panels.scale = scale;
        panels.settle();
        assert_eq!(divider_metrics(&panels.ctx), DividerMetrics::from_scale(scale),
            "one context refreshes metrics across scale restoration without a radius ratchet");
    }
    for scale in [0.75, 1.0, 1.25, 2.0, 1.0] {
        for sides in [[true, true], [true, false], [false, true], [false, false]] {
            panels.ctx = egui::Context::default();
            panels.scale = scale;
            panels.side_visible = sides;
            panels.settle();
            let metrics = divider_metrics(&panels.ctx).unwrap();
            assert_eq!(Some(metrics), DividerMetrics::from_scale(scale),
                "the populated profile uses the current physical target policy");
            let (table, _, _) = super::results_dock::table_geometry(&panels.ctx);
            let (_, result) = panels.ctx.data(|data|
                data.get_temp::<(u64, egui::Rect)>(results_edge_key()).unwrap());
            if !sides[0] {
                assert!((table.left() - result.left() - 8.0).abs() < 0.01,
                    "without an adjacent left divider the native eight-point margin remains");
            }
            if !sides[1] {
                assert!((result.right() - table.right() - 8.0).abs() < 0.01,
                    "without an adjacent right divider the native eight-point margin remains");
            }
            let controls = observations(&panels.ctx).controls;
            let handles = controls.iter().filter(|control| control.draggable
                && (control.rect.top() - table.top()).abs() < 0.01
                && control.clip.contains_rect(control.rect)).collect::<Vec<_>>();
            assert!(!handles.is_empty(), "actual populated Results exposes a complete column handle");
            for handle in handles {
                assert!(handle.hit.width() * scale >= 20.0 && handle.hit.height() * scale >= 20.0,
                    "allocated column target survives native rounding and clip");
                assert!(controls.iter().filter(|control| control.clickable)
                    .all(|control| !control.hit.intersects(handle.hit)
                        || control.hit.intersect(handle.hit).width() <= 0.001
                        || control.hit.intersect(handle.hit).height() <= 0.001),
                    "column slots cannot steal neighboring sort/cell targets");
            }
            assert!(controls.iter().any(|control| control.clickable
                && (control.rect.left() - table.left()).abs() < 0.01
                && (control.rect.top() - table.top()).abs() < 0.01),
                "the actual first-column sort header is rendered");
            assert!(controls.iter().any(|control| control.clickable
                && (control.rect.left() - table.left()).abs() < 0.01
                && control.rect.top() > table.top() + 20.0
                && control.hit.intersect(control.clip).is_positive()),
                "an actual first-column data cell is visible");
            panels.scrollbar(true);
            panels.scrollbar(false);
        }
    }
    panels.results = false;
    panels.settle();
}

#[cfg(test)]
fn extend_physical_targets_journey() {
    let ctx = egui::Context::default();
    let mut text = String::new();
    let mut checked = false;
    for scale in [0.75, 1.0, 1.25, 2.0, 0.75, 1.0] {
        ctx.set_pixels_per_point(scale);
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 600.0))),
            ..Default::default()
        }, |ctx| {
            apply_style(ctx);
            let minimum = target_size(ctx);
            assert_eq!(ctx.style().spacing.interact_size.y, minimum);
            assert!(minimum * scale >= 20.0);
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut hits = vec![
                    ui.button("Ordinary action").interact_rect,
                    small_button(ui, "Compact action").interact_rect,
                    action(ui, egui::Button::new("+"), "Add fixture").interact_rect,
                    ui.add_sized(control_size(ctx, [26.0, 26.0]), egui::Button::new("X")).interact_rect,
                    ui.add(text_input(ui, &mut text).desired_width(100.0)).interact_rect,
                    ui.checkbox(&mut checked, "Flag").interact_rect,
                    ui.radio(checked, "Mode").interact_rect,
                ];
                let header = ui.allocate_ui(egui::vec2(80.0, minimum), |ui| {
                    let title = "A long native section title that must remain within its parent";
                    let expected_id = ui.make_persistent_id(egui::Id::new(title));
                    let response = section_header(ui, title).show(ui, |_| {});
                    assert_eq!(response.header_response.id, expected_id,
                        "truncation preserves native full-title collapse identity");
                    assert!(response.header_response.rect.width() <= 80.01);
                    response.header_response
                });
                hits.push(header.inner.interact_rect);
                for hit in hits {
                    assert!(hit.width() * scale >= 20.0 && hit.height() * scale >= 20.0,
                        "actual allocated native targets must survive logical grid rounding: {hit:?} at {scale}");
                }
            });
        });
    }
    assert_eq!(target_size(&ctx), 24.0, "restoring normal scale restores normal density");
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(quantized_target(scale).is_none());
    }
}

#[derive(Clone, Default)]
pub struct LayoutObservations {
    pub enabled: bool,
    pub pass: u64,
    pub scrolls: Vec<ScrollObservation>,
    pub controls: Vec<ControlObservation>,
    pub texts: Vec<TextObservation>,
    pub dividers: Vec<DividerObservation>,
}

#[derive(Clone)]
pub struct ScrollObservation {
    pub content_id: u64,
    pub parent_id: u64,
    pub axes: [bool; 2],
    pub label: String,
    pub clip: egui::Rect,
    pub viewport: egui::Rect,
    pub content_rect: egui::Rect,
    pub offset: egui::Vec2,
    pub content_size: egui::Vec2,
    pub layer: String,
}

fn key() -> egui::Id {
    egui::Id::new("qa_layout_observations")
}

pub fn begin_frame(ctx: &egui::Context, enabled: bool) {
    if !enabled {
        return;
    }
    ctx.data_mut(|data| {
        data.insert_temp(
            key(),
            LayoutObservations {
                enabled,
                ..Default::default()
            },
        )
    });
}

pub fn observations(ctx: &egui::Context) -> LayoutObservations {
    ctx.data(|data| data.get_temp(key()).unwrap_or_default())
}

pub fn scroll<R>(
    ui: &mut egui::Ui,
    label: &str,
    axes: [bool; 2],
    area: egui::ScrollArea,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    let enabled = ui.ctx().data(|data| {
        data.get_temp::<LayoutObservations>(key())
            .is_some_and(|o| o.enabled)
    });
    let mut content_id = 0;
    let parent_id = ui.unique_id().value();
    let mut clip = egui::Rect::NOTHING;
    let mut content_rect = egui::Rect::NOTHING;
    let width = ui.available_width();
    let layer = ui.layer_id();
    let output = area.show(ui, |ui| {
        content_id = ui.unique_id().value();
        clip = ui.clip_rect();
        ui.set_max_width((width - SCROLL_WIDTH_RESERVE).max(40.0));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.spacing_mut().text_edit_width = ui.available_width().min(200.0);
        let inner = body(ui);
        content_rect = ui.min_rect();
        inner
    });
    if enabled {
        let global = ui
            .ctx()
            .layer_transform_to_global(layer)
            .unwrap_or_default();
        ui.ctx().data_mut(|data| {
            let observations = data.get_temp_mut_or_default::<LayoutObservations>(key());
            observations.scrolls.push(ScrollObservation {
                content_id,
                parent_id,
                axes,
                label: label.to_owned(),
                clip: global * clip,
                viewport: global * output.inner_rect,
                content_rect: global * content_rect,
                offset: output.state.offset * global.scaling,
                content_size: output.content_size * global.scaling,
                layer: format!("{layer:?}"),
            });
        });
    }
    output
}

/// Physical-pixel viewport floor, expressed in the current egui point scale.
pub fn protected_width(window_width: f32, pixels_per_point: f32) -> f32 {
    (window_width * 0.45)
        .max(360.0 / pixels_per_point.max(0.01))
        .min(window_width)
}

pub fn can_show_left(ctx: &egui::Context) -> bool {
    ctx.available_rect().width()
        >= protected_width(ctx.viewport_rect().width(), ctx.pixels_per_point()) + 240.0
}

pub fn can_show_right(ctx: &egui::Context) -> bool {
    ctx.available_rect().width()
        >= protected_width(ctx.viewport_rect().width(), ctx.pixels_per_point()) + 120.0
}

pub fn right_width(ctx: &egui::Context) -> f32 {
    (ctx.available_rect().width()
        - protected_width(ctx.viewport_rect().width(), ctx.pixels_per_point()))
    .max(0.0)
}

pub fn left_width(ctx: &egui::Context) -> f32 {
    (ctx.available_rect().width()
        - protected_width(ctx.viewport_rect().width(), ctx.pixels_per_point())
        - 120.0)
        .clamp(120.0, 400.0)
}

pub fn editor_height(ctx: &egui::Context) -> f32 {
    (ctx.available_rect().height() * 0.38).clamp(64.0, 240.0)
}

pub fn trailing_row<R>(
    ui: &mut egui::Ui,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), body)
            .inner
    })
}

pub fn dialog_width(ctx: &egui::Context, preferred: f32) -> f32 {
    preferred
        .min((ctx.viewport_rect().width() * 0.55).max(120.0))
        .min((ctx.viewport_rect().width() - 16.0).max(0.0))
}

pub(crate) fn target_size(ctx: &egui::Context) -> f32 {
    quantized_target(ctx.pixels_per_point()).unwrap_or(NORMAL_CONTROL_POINTS)
}

fn quantized_target(scale: f32) -> Option<f32> {
    if !scale.is_finite() || scale <= 0.0 { return None; }
    let quantum = egui::emath::GUI_ROUNDING;
    let target = (NORMAL_CONTROL_POINTS.max(MIN_CONTROL_HIT_PX / scale) / quantum).ceil() * quantum;
    target.is_finite().then_some(target)
}

/// Preserve requested control space with the shared physical minimum.
pub(crate) fn control_size(ctx: &egui::Context, desired: impl Into<egui::Vec2>) -> egui::Vec2 {
    desired.into().max(egui::Vec2::splat(target_size(ctx)))
}

pub(crate) fn small_button(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    let minimum = target_size(ui.ctx());
    ui.add(egui::Button::new(text).small().min_size(egui::Vec2::splat(minimum)))
}

/// Bound native scroll minima to the actual remaining visible parent budget.
pub(crate) fn visible_scroll_height(ui: &egui::Ui) -> f32 {
    let remaining = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    remaining.height().max(0.0)
}

/// Keep navigation compact when ordinary chrome would consume the body budget.
pub(crate) fn compact_navigation(ui: &egui::Ui) -> bool {
    let target = target_size(ui.ctx());
    let rail_width = 7.0 * target + 6.0 * ui.spacing().item_spacing.x + 16.0;
    ui.available_width() < rail_width
        || visible_scroll_height(ui) < navigation_body_budget(ui) + 5.0 * target + 48.0
}

pub(crate) fn navigation_body_budget(ui: &egui::Ui) -> f32 {
    (3.0 * target_size(ui.ctx())).max(96.0)
}

/// Prepare a native section title without expanding a constrained parent.
pub(crate) fn section_header(ui: &egui::Ui, text: impl Into<egui::WidgetText>) -> SectionHeader {
    let text = text.into();
    let title = text.text().to_owned();
    let width = (ui.available_width() - ui.spacing().indent - ui.spacing().button_padding.x).max(0.0);
    let galley = text.into_galley(ui, Some(egui::TextWrapMode::Truncate), width, egui::TextStyle::Button);
    SectionHeader { native: egui::CollapsingHeader::new(galley), title }
}

pub(crate) struct SectionHeader { native: egui::CollapsingHeader, title: String }

impl SectionHeader {
    pub(crate) fn default_open(mut self, open: bool) -> Self {
        self.native = self.native.default_open(open);
        self
    }

    pub(crate) fn show<R>(self, ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R)
        -> egui::collapsing_header::CollapsingResponse<R> {
        let mut response = self.native.show(ui, body);
        response.header_response = response.header_response.on_hover_text(self.title);
        response
    }
}

/// A single-line form input with the shared native interaction minimum.
pub(crate) fn text_input<'a>(ui: &egui::Ui, text: &'a mut dyn egui::TextBuffer) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text).min_size(egui::Vec2::splat(target_size(ui.ctx())))
}

/// Render a native window header with the shared interaction minimum.
pub(crate) trait WindowTargets {
    fn show_with_targets<R>(
        self,
        ctx: &egui::Context,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<egui::InnerResponse<Option<R>>>;
}

impl WindowTargets for egui::Window<'_> {
    fn show_with_targets<R>(
        self,
        ctx: &egui::Context,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<egui::InnerResponse<Option<R>>> {
        let mut header = HeaderStyleScope::new(ctx);
        self.show(ctx, |ui| {
            ui.set_style(header.restore());
            body(ui)
        })
    }
}

struct HeaderStyleScope {
    ctx: egui::Context,
    theme: egui::Theme,
    original: Option<std::sync::Arc<egui::Style>>,
}

impl HeaderStyleScope {
    fn new(ctx: &egui::Context) -> Self {
        let theme = ctx.theme();
        let original = ctx.style_of(theme);
        let mut header = (*original).clone();
        let minimum = target_size(ctx);
        header.spacing.icon_width = header.spacing.icon_width.max(minimum);
        header.spacing.interact_size.y = header.spacing.interact_size.y.max(minimum);
        ctx.set_style_of(theme, header);
        Self { ctx: ctx.clone(), theme, original: Some(original) }
    }

    fn restore(&mut self) -> std::sync::Arc<egui::Style> {
        let original = self.original.take().expect("window body runs at most once");
        self.ctx.set_style_of(self.theme, original.clone());
        original
    }
}

impl Drop for HeaderStyleScope {
    fn drop(&mut self) {
        if self.original.is_some() {
            self.restore();
        }
    }
}

pub fn bounded_window<'a>(
    ctx: &egui::Context,
    title: impl Into<egui::WidgetText>,
) -> egui::Window<'a> {
    crate::native_accessibility::surface::popup(ctx);
    let screen = ctx.viewport_rect();
    egui::Window::new(title)
        .constrain_to(screen.shrink(8.0))
        .max_width(dialog_width(ctx, f32::INFINITY))
        .max_height((screen.height() - 90.0).max(160.0))
        .hscroll(true)
        .vscroll(true)
}

/// Unicode-safe path shortening keeps both the root and filename visible.
pub fn middle_ellipsis(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let head = keep / 3;
    let tail = keep - head;
    format!(
        "{}\u{2026}{}",
        value.chars().take(head).collect::<String>(),
        value.chars().skip(count - tail).collect::<String>()
    )
}

pub fn apply_style(ctx: &egui::Context) {
    let scale = ctx.pixels_per_point();
    let pass = ctx.cumulative_pass_nr();
    let metrics = DividerMetrics::from_scale(scale);
    ctx.data_mut(|data| {
        data.insert_temp(divider_policy_key(), (pass, scale, metrics));
        data.remove::<(u64, egui::Rect)>(results_edge_key());
    });
    let minimum = target_size(ctx);
    if ctx.style().spacing.interact_size.y != minimum {
        ctx.style_mut(|style| style.spacing.interact_size.y = minimum);
    }
    let id = egui::Id::new("layout_policy_style");
    if ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    ctx.style_mut(|style| {
        style.spacing.button_padding.y = style.spacing.button_padding.y.max(5.0);
    });
    ctx.data_mut(|d| d.insert_temp(id, true));
}

#[derive(Clone)]
pub struct ControlObservation {
    pub id: u64,
    pub rect: egui::Rect,
    pub hit: egui::Rect,
    pub clip: egui::Rect,
    pub layer: egui::LayerId,
    pub allowed: bool,
    pub clickable: bool,
    pub draggable: bool,
}

#[derive(Clone)]
pub struct TextObservation {
    pub id: u64,
    pub truncated: bool,
    pub hover: bool,
}

pub fn begin_ui_pass(ctx: &egui::Context) {
    let enabled = ctx.data(|d| {
        d.get_temp::<LayoutObservations>(key())
            .is_some_and(|o| o.enabled)
    });
    if !enabled {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        d.insert_temp(
            key(),
            LayoutObservations {
                enabled,
                pass,
                ..Default::default()
            },
        )
    });
}

pub fn record_response(ui: &egui::Ui, response: &egui::Response) {
    if !ui.ctx().data(|d| {
        d.get_temp::<LayoutObservations>(key())
            .is_some_and(|o| o.enabled)
    }) {
        return;
    }
    let global = ui
        .ctx()
        .layer_transform_to_global(response.layer_id)
        .unwrap_or_default();
    let allowed = ui
        .ctx()
        .memory(|memory| memory.allows_interaction(response.layer_id));
    let observation = ControlObservation {
        id: response.id.value(),
        rect: global * response.rect,
        hit: global * response.interact_rect,
        clip: global * ui.clip_rect(),
        layer: response.layer_id,
        allowed,
        clickable: response.sense.senses_click(),
        draggable: response.sense.senses_drag(),
    };
    ui.ctx().data_mut(|d| {
        let o = d.get_temp_mut_or_default::<LayoutObservations>(key());
        if o.controls.len() < 20000 {
            o.controls.push(observation);
        }
    });
}

pub fn action(ui: &mut egui::Ui, button: egui::Button<'_>, name: &str) -> egui::Response {
    let minimum = target_size(ui.ctx());
    use crate::accessibility::ResponseExt;
    ui.add(button.min_size(egui::Vec2::splat(minimum))).named(ui, name)
}

pub fn value_label(ui: &mut egui::Ui, value: &str, color: egui::Color32) -> egui::Response {
    let display = if value.contains('\\') || value.contains('/') {
        middle_ellipsis(
            value,
            (ui.available_width() / 7.0).floor().max(5.0) as usize,
        )
    } else {
        value.to_owned()
    };
    let shortened = display != value;
    let (position, galley, response) = egui::Label::new(
        egui::RichText::new(display)
            .small()
            .monospace()
            .color(color),
    )
    .truncate()
    .layout_in_ui(ui);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), value));
    if ui.is_rect_visible(response.rect) {
        ui.painter().galley(position, galley.clone(), color);
    }
    let truncated = shortened || galley.elided;
    if ui.ctx().data(|d| {
        d.get_temp::<LayoutObservations>(key())
            .is_some_and(|o| o.enabled)
    }) {
        ui.ctx().data_mut(|d| {
            let o = d.get_temp_mut_or_default::<LayoutObservations>(key());
            if o.texts.len() < 20000 {
                o.texts.push(TextObservation {
                    id: response.id.value(),
                    truncated,
                    hover: true,
                });
            }
        });
    }
    response.on_hover_text(value)
}
