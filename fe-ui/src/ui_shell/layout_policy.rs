//! Finite shell geometry and same-frame layout observations; see AGENTS.md §layout-policy.

use bevy_egui::egui;

#[derive(Clone, Default)]
pub struct LayoutObservations {
    pub enabled: bool,
    pub pass: u64,
    pub scrolls: Vec<ScrollObservation>,
    pub controls: Vec<ControlObservation>,
    pub texts: Vec<TextObservation>,
}

#[derive(Clone)]
pub struct ScrollObservation {
    pub content_id: u64,
    pub axes: [bool; 2],
    pub label: String,
    pub clip: egui::Rect,
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
    let width = ui.available_width();
    let layer = ui.layer_id();
    let output = area.show(ui, |ui| {
        content_id = ui.unique_id().value();
        ui.set_max_width((width - 12.0).max(40.0));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.spacing_mut().text_edit_width = ui.available_width().min(200.0);
        body(ui)
    });
    if enabled {
        ui.ctx().data_mut(|data| {
            let observations = data.get_temp_mut_or_default::<LayoutObservations>(key());
            observations.scrolls.push(ScrollObservation {
                content_id,
                axes,
                label: label.to_owned(),
                clip: output.inner_rect.intersect(ui.clip_rect()),
                offset: output.state.offset,
                content_size: output.content_size,
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

pub fn bounded_window<'a>(
    ctx: &egui::Context,
    title: impl Into<egui::WidgetText>,
) -> egui::Window<'a> {
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
    let id = egui::Id::new("layout_policy_style");
    if ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    ctx.style_mut(|style| {
        style.spacing.button_padding.y = style.spacing.button_padding.y.max(5.0);
        style.spacing.interact_size.y = style.spacing.interact_size.y.max(24.0);
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
    let minimum = (20.0 / ui.ctx().pixels_per_point()).max(24.0);
    let response = ui.add(button.min_size(egui::Vec2::splat(minimum)));
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_label(name));
    record_response(ui, &response);
    response
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
