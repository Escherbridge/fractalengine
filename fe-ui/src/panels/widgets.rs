//! Shared, reusable inspector/panel egui widgets + world↔meters unit helpers.
//! See `fe-ui/src/panels/AGENTS.md` §widgets. Keep these egui-only (no
//! queries).

use bevy_egui::egui;

use crate::actions::UiManager;
use crate::plugin::LocalUserRole;
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

/// Wrap `widget` with role-based access control (X-02, OD-3): a disallowed
/// control renders disabled (`add_enabled`) rather than hidden, with a hover
/// tooltip naming why, so the control stays discoverable. Authorization never
/// lives only here — the DB thread's `fe-policy` gate is the real guard; this
/// is advisory UI only. Owner of the three widgets below: `panels/widgets.rs`.
pub(crate) fn role_gated(
    ui: &mut egui::Ui,
    allowed: bool,
    reason: &str,
    widget: impl egui::Widget,
) -> egui::Response {
    let resp = ui.add_enabled(allowed, widget);
    if allowed {
        resp
    } else {
        resp.on_disabled_hover_text(reason)
    }
}

/// Plain-language denial copy for [`role_gated`]'s hover text (OD-3):
/// "`<needed>` role needed — you are `<role>`[ in `<verse>`]". The role word
/// comes from [`crate::notice::role_label`], the same one the denial notices
/// use, so an unresolved role (X-03) reads "—" rather than a guess.
pub(crate) fn role_denial_reason(
    role: &LocalUserRole,
    needed: &str,
    verse_name: Option<&str>,
) -> String {
    let role_label = crate::notice::role_label(role.role);
    match verse_name {
        Some(name) if !name.is_empty() => {
            format!("{needed} role needed — you are {role_label} in {name}")
        }
        _ => format!("{needed} role needed — you are {role_label}"),
    }
}

/// Two-step inline confirm for a destructive action (X-06; extracted from the
/// inspector token-revoke pattern at `inspector.rs:924-970`, read-only during
/// extraction). The first click arms (shows `label`); a second click within
/// 4 s on the now-shown `confirm_label` button fires (`true`). The armed
/// state disarms — reverting to the single `label` button — after 4 s, or
/// immediately on any click elsewhere this frame (clicking Cancel counts as
/// "elsewhere" and disarms too, without firing). State lives in egui temp
/// memory keyed by `id_salt`, so callers need no extra struct field (the
/// motivating case: single-click destruction with no undo, X-06/X-07).
pub(crate) fn confirm_button(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    label: &str,
    confirm_label: &str,
) -> bool {
    const ARM_SECONDS: f64 = 4.0;
    let id = egui::Id::new("confirm_button_armed_at").with(id_salt);
    let now = ui.ctx().input(|i| i.time);
    let armed_at: Option<f64> = ui.ctx().data(|d| d.get_temp(id));
    let is_armed = armed_at.is_some_and(|t| now - t < ARM_SECONDS);

    if is_armed {
        let mut confirmed = false;
        let mut own_click = false;
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new(confirm_label).small())
                        .fill(theme::BG_DANGER),
                )
                .clicked()
            {
                confirmed = true;
                own_click = true;
            }
            if ui
                .add(egui::Button::new(egui::RichText::new("Cancel").small()).fill(theme::BG_BUTTON))
                .clicked()
            {
                own_click = true;
            }
        });
        // A click this frame that landed on neither Confirm nor Cancel is
        // "elsewhere" — disarm without firing (same convention as dismissing
        // an open popover by clicking outside it).
        let clicked_elsewhere = !own_click && ui.ctx().input(|i| i.pointer.any_click());
        if confirmed || own_click || clicked_elsewhere {
            ui.ctx().data_mut(|d| d.remove::<f64>(id));
        }
        confirmed
    } else {
        if ui
            .add(egui::Button::new(egui::RichText::new(label).small()).fill(theme::BG_DANGER))
            .clicked()
        {
            ui.ctx().data_mut(|d| d.insert_temp(id, now));
        }
        false
    }
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

    // --- role_gated / confirm_button (X-02, X-06) -------------------------
    //
    // Headless `egui::Context::run` harness, same pattern as
    // `ui_shell::topbar::tests::TitleBarHarness` (synthetic `RawInput` events
    // driving real widget interaction — no mocked response objects).

    fn run_frame(
        ctx: &egui::Context,
        time: &mut f64,
        events: Vec<egui::Event>,
        mut body: impl FnMut(&mut egui::Ui),
    ) {
        *time += 0.05;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 200.0),
            )),
            time: Some(*time),
            events,
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // W1-GATE fix: `CentralPanel`'s own `ui` always fills the
                // whole panel, so `ui.min_rect()` on it is the full screen
                // rect regardless of what `body` draws — the geometry tests
                // below need a child `ui` whose `min_rect()` tightly wraps
                // just the widgets `body` adds.
                ui.scope(|ui| body(ui));
            });
        });
    }

    fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Hover, press, release at `at` — three separate frames, each one real
    /// input event, mirroring `TitleBarHarness::click`.
    fn click_sequence(
        ctx: &egui::Context,
        time: &mut f64,
        at: egui::Pos2,
        mut body: impl FnMut(&mut egui::Ui),
    ) {
        run_frame(ctx, time, vec![egui::Event::PointerMoved(at)], &mut body);
        run_frame(ctx, time, vec![pointer_button(at, true)], &mut body);
        run_frame(ctx, time, vec![pointer_button(at, false)], &mut body);
    }

    #[test]
    fn role_gated_disables_and_explains() {
        let ctx = egui::Context::default();
        let mut time = 0.0;

        // Disallowed: renders disabled, and a click on it never registers.
        let mut rect = egui::Rect::NOTHING;
        run_frame(&ctx, &mut time, Vec::new(), |ui| {
            let resp = role_gated(ui, false, "Editor role needed", egui::Button::new("Edit"));
            assert!(!resp.enabled(), "a disallowed control must render disabled");
            rect = resp.rect;
        });
        let mut viewer_clicked = 0u32;
        click_sequence(&ctx, &mut time, rect.center(), |ui| {
            let resp = role_gated(ui, false, "Editor role needed", egui::Button::new("Edit"));
            if resp.clicked() {
                viewer_clicked += 1;
            }
        });
        assert_eq!(
            viewer_clicked, 0,
            "a disabled widget must never register a click"
        );

        // Allowed: renders enabled, and the same click now registers.
        let mut rect2 = egui::Rect::NOTHING;
        run_frame(&ctx, &mut time, Vec::new(), |ui| {
            let resp = role_gated(ui, true, "Editor role needed", egui::Button::new("Edit"));
            assert!(resp.enabled(), "an allowed control must render enabled");
            rect2 = resp.rect;
        });
        let mut editor_clicked = 0u32;
        click_sequence(&ctx, &mut time, rect2.center(), |ui| {
            let resp = role_gated(ui, true, "Editor role needed", egui::Button::new("Edit"));
            if resp.clicked() {
                editor_clicked += 1;
            }
        });
        assert_eq!(
            editor_clicked, 1,
            "an allowed widget must register its click"
        );

        // `role_denial_reason` is the text role_gated attaches as the hover
        // explanation — verify it names both the needed and the held role,
        // and falls back to "—" for an unresolved role (X-03).
        let mut role = LocalUserRole {
            role: Some(fe_database::RoleLevel::Viewer),
        };
        assert_eq!(
            role_denial_reason(&role, "Editor", Some("Acme Verse")),
            "Editor role needed — you are Viewer in Acme Verse"
        );
        role.role = None;
        assert_eq!(
            role_denial_reason(&role, "Editor", None),
            "Editor role needed — you are \u{2014}"
        );
    }

    #[test]
    fn confirm_button_requires_second_click_and_disarms() {
        let ctx = egui::Context::default();
        let mut time = 0.0;

        // Lay out the unarmed "Delete" button and capture its rect.
        let mut unarmed_rect = egui::Rect::NOTHING;
        run_frame(&ctx, &mut time, Vec::new(), |ui| {
            let fired = confirm_button(ui, "row-1", "Delete", "Confirm delete");
            assert!(!fired, "the very first render must not fire");
            unarmed_rect = ui.min_rect();
        });

        // First click arms it — must not fire yet.
        let mut fired_on_arm = false;
        click_sequence(&ctx, &mut time, unarmed_rect.center(), |ui| {
            fired_on_arm = confirm_button(ui, "row-1", "Delete", "Confirm delete");
        });
        assert!(!fired_on_arm, "arming must not itself confirm");

        // Now armed: Confirm is the left-most of the Confirm/Cancel pair.
        let mut armed_rect = egui::Rect::NOTHING;
        run_frame(&ctx, &mut time, Vec::new(), |ui| {
            let fired = confirm_button(ui, "row-1", "Delete", "Confirm delete");
            assert!(!fired, "a bare re-render while armed must not fire");
            armed_rect = ui.min_rect();
        });
        let confirm_point = egui::pos2(armed_rect.left() + 3.0, armed_rect.center().y);

        // Second click, on Confirm, fires.
        let mut fired_on_confirm = false;
        click_sequence(&ctx, &mut time, confirm_point, |ui| {
            fired_on_confirm = confirm_button(ui, "row-1", "Delete", "Confirm delete");
        });
        assert!(fired_on_confirm, "the second click on Confirm must fire");

        // It disarmed after firing: the very next render is the single
        // button again, at the original unarmed position.
        run_frame(&ctx, &mut time, Vec::new(), |ui| {
            let fired = confirm_button(ui, "row-1", "Delete", "Confirm delete");
            assert!(!fired, "a render right after firing must not re-fire");
            assert_eq!(
                ui.min_rect(),
                unarmed_rect,
                "a fired confirm must disarm back to the single-button layout"
            );
        });

        // Arm again, then let the 4s window elapse with no interaction: the
        // very next render (no click at all) must time out back to the
        // single button — proving the disarm is time-driven, not just a
        // missed click.
        let mut fired_on_rearm = false;
        click_sequence(&ctx, &mut time, unarmed_rect.center(), |ui| {
            fired_on_rearm = confirm_button(ui, "row-1", "Delete", "Confirm delete");
        });
        assert!(!fired_on_rearm);
        time += 5.0; // > the 4s arm window
        run_frame(&ctx, &mut time, Vec::new(), |ui| {
            let fired = confirm_button(ui, "row-1", "Delete", "Confirm delete");
            assert!(!fired);
            assert_eq!(
                ui.min_rect(),
                unarmed_rect,
                "after 4s with no click, the arm must time out back to the single button"
            );
        });
    }
}
