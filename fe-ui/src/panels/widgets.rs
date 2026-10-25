//! Shared, reusable inspector/panel egui widgets + world↔meters unit helpers.
//! See `fe-ui/src/panels/AGENTS.md` §widgets. Keep these egui-only (no
//! queries).

use bevy_egui::egui;

use crate::actions::UiManager;
use crate::theme;

/// Read-only, width-capped, copyable value box.
///
/// The frame never exceeds `ui.available_width()` and the value wraps in
/// monospace, so an arbitrarily large value can't push the enclosing panel
/// wider (FR-1 of `inspector_units_width_20260716`). `display` is what's shown
/// (already elided if long); `copy_value` is the full text the copy button
/// places on the clipboard. `left`/`right` draw caller content on the header
/// row (label on the left; extra right-aligned controls, e.g. a delete button,
/// beside the copy button on the right).
///
/// NOTE: must be rendered OUTSIDE an `egui::Grid` cell — a Grid cell ignores
/// `set_max_width`, which would let the value force horizontal growth again.
pub(crate) fn copy_value_box(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    display: &str,
    copy_value: &str,
    toast: &str,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::NONE
        .fill(theme::BG_BUTTON)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .corner_radius(3.0)
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.horizontal(|ui| {
                left(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new("\u{1F4CB}")
                                .fill(egui::Color32::TRANSPARENT)
                                .small(),
                        )
                        .on_hover_text("Copy to clipboard")
                        .clicked()
                    {
                        ui.ctx().copy_text(copy_value.to_string());
                        let now = ui.ctx().input(|i| i.time);
                        ui_mgr.show_toast(toast.to_string(), now);
                    }
                    right(ui);
                });
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(display)
                        .small()
                        .monospace()
                        .color(theme::TEXT_BRIGHT),
                )
                .wrap(),
            );
        });
    ui.add_space(3.0);
}

/// Labeled monospace value with a one-click copy button (egui builtin clipboard
/// via `ctx.copy_text` — no new deps). Thin convenience over [`copy_value_box`]
/// where display == copy == value; used by the "Copy for BI" egress card.
pub(crate) fn copy_row(ui: &mut egui::Ui, ui_mgr: &mut UiManager, label: &str, value: &str) {
    copy_value_box(
        ui,
        ui_mgr,
        value,
        value,
        &format!("Copied {label}"),
        |ui| {
            ui.label(
                egui::RichText::new(label)
                    .small()
                    .strong()
                    .color(theme::TEXT_SECTION),
            );
        },
        |_ui| {},
    );
}

/// world_scale sanitized: ≤0 / non-finite ⇒ 1.0 (1 world-unit : 1 m).
fn sane_scale(world_scale: f64) -> f64 {
    if world_scale.is_finite() && world_scale > 0.0 {
        world_scale
    } else {
        1.0
    }
}

/// World units → meters (`real_m = world / world_scale`).
pub(crate) fn world_to_meters(world: f32, world_scale: f64) -> f32 {
    (world as f64 / sane_scale(world_scale)) as f32
}

/// Meters → world units (`world = m * world_scale`).
pub(crate) fn meters_to_world(meters: f32, world_scale: f64) -> f32 {
    (meters as f64 * sane_scale(world_scale)) as f32
}

/// Single-line value, ellipsis-clipped to the available width instead of
/// wrapping or extending it (egui's default `Extend` wrap mode for a plain
/// `horizontal`/Grid cell sets `wrap.max_width = INFINITY`, which is exactly
/// what let a long Scope/URL/DID string blow the inspector out — 2026-10-25
/// in-app finding F1). The full value is always on the hover tooltip. Unlike
/// [`copy_value_box`] (a boxed, multi-row, always-copyable value for
/// arbitrary-length blobs), this is for a compact field embedded in an
/// existing row that must not grow past one line. See
/// `fe-ui/src/AGENTS.md` §inspector.
pub(crate) fn clipped_value(
    ui: &mut egui::Ui,
    value: &str,
    color: egui::Color32,
) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(value).small().monospace().color(color)).truncate())
        .on_hover_text(value)
}

/// [`clipped_value`] plus click-to-copy with a toast, for fields where a
/// separate copy button would be redundant (F1 "where useful a
/// click-to-copy").
pub(crate) fn clipped_value_copyable(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    value: &str,
    color: egui::Color32,
    toast: &str,
) -> egui::Response {
    let resp = ui
        .add(
            egui::Label::new(egui::RichText::new(value).small().monospace().color(color))
                .truncate()
                .sense(egui::Sense::click()),
        )
        .on_hover_text(format!("{value}\n\n(click to copy)"));
    if resp.clicked() {
        ui.ctx().copy_text(value.to_string());
        let now = ui.ctx().input(|i| i.time);
        ui_mgr.show_toast(toast.to_string(), now);
    }
    resp
}

/// Truncate `s` to at most `max_chars` characters, appending an ellipsis when
/// clipped. Char-boundary safe (multibyte-aware). Keeps a huge property value
/// (e.g. a `gpx_points` JSON blob) from walling the inspector vertically; the
/// full value is still copyable via the copy button.
pub(crate) fn elide(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (count, ch) in s.chars().enumerate() {
        if count >= max_chars {
            out.push('\u{2026}');
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_conversion_round_trips_and_sanitizes_scale() {
        assert_eq!(world_to_meters(10.0, 2.0), 5.0);
        assert_eq!(meters_to_world(5.0, 2.0), 10.0);
        assert_eq!(world_to_meters(meters_to_world(3.5, 0.25), 0.25), 3.5);
        // Degenerate scales fall back to 1:1.
        assert_eq!(world_to_meters(7.0, 0.0), 7.0);
        assert_eq!(meters_to_world(7.0, f64::NAN), 7.0);
    }

    #[test]
    fn elide_shorter_than_cap_is_unchanged() {
        assert_eq!(elide("hello", 10), "hello");
        assert_eq!(elide("", 5), "");
    }

    #[test]
    fn elide_at_cap_is_unchanged() {
        assert_eq!(elide("abcde", 5), "abcde");
    }

    #[test]
    fn elide_longer_than_cap_appends_ellipsis() {
        assert_eq!(elide("abcdef", 5), "abcde\u{2026}");
    }

    #[test]
    fn elide_is_char_boundary_safe() {
        // 6 multibyte chars, cap 3 → first 3 chars + ellipsis, no panic.
        assert_eq!(
            elide("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 3),
            "\u{e9}\u{e9}\u{e9}\u{2026}"
        );
    }
}
