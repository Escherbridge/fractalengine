//! Portal webview toolbar (replaces the inspector panel while a portal is open).

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::actions::{UiAction, UiManager};
use crate::portal::chrome::{chrome_model, ChromeAction, ChromeTone};
use crate::theme;

/// Host-drawn admission badge plus the Admit / Revoke actions the state allows (ux.md §3).
fn render_bundle_chrome(ui: &mut egui::Ui, ui_mgr: &mut UiManager) {
    let Some(model) = ui_mgr.portal.bundle_state().and_then(chrome_model) else {
        return;
    };
    let color = match model.tone {
        ChromeTone::Accent => theme::TREE_PRIMARY_ACCENT,
        ChromeTone::Neutral => theme::TEXT_DIM,
        ChromeTone::Error => theme::STATUS_OFFLINE,
    };
    ui.add(egui::Label::new(egui::RichText::new(&model.badge_text).color(color)).truncate())
        .named(ui, &model.badge_name)
        .on_hover_text(&model.badge_text);
    ui.horizontal_wrapped(|ui| {
        for action in [Some(model.primary), model.secondary].into_iter().flatten() {
            if action == ChromeAction::Close {
                continue; // The fixed Close button above is the one Close control.
            }
            let size = crate::ui_shell::layout_policy::control_size(ui.ctx(), [28.0, 24.0]);
            if ui
                .add(egui::Button::new(action.label()).min_size(size))
                .named(ui, action.accessible_name())
                .clicked()
            {
                ui_mgr.push_action(match action {
                    ChromeAction::Admit => UiAction::PortalAdmitBundle,
                    _ => UiAction::PortalRevokeBundle,
                });
            }
        }
    });
}

const FEEDBACK_HEIGHT: f32 = 64.0;

/// Reserves a stable native viewport while loading and error feedback changes.
fn render_feedback(ui: &mut egui::Ui, ui_mgr: &UiManager) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), FEEDBACK_HEIGHT),
        egui::Sense::hover(),
    );
    let mut feedback = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("portal-feedback")
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    feedback.set_clip_rect(rect.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical()
        .id_salt("portal-feedback-scroll")
        .max_height(FEEDBACK_HEIGHT)
        .auto_shrink([false, false])
        .show(&mut feedback, |ui| {
            if ui_mgr.portal_feedback.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading page...");
                });
            }
            if let Some(error) = &ui_mgr.portal_feedback.error {
                ui.add(
                    egui::Label::new(egui::RichText::new(error).color(theme::STATUS_OFFLINE))
                        .wrap(),
                )
                .on_hover_text(error);
            }
        });
}

pub(crate) fn right_portal_toolbar(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let max_w = ctx.viewport_rect().width() * 0.8;
    egui::SidePanel::right("portal_toolbar")
        .resizable(true)
        .default_width(400.0)
        .width_range(260.0..=max_w)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0))
                .stroke(egui::Stroke::new(2.0_f32, theme::BG_BUTTON)),
        )
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let embedded = fe_webview::backends::embedded_browsing_supported();
                for (label, name, action) in [
                    ("Back", "Go back in portal", UiAction::PortalGoBack),
                    ("Forward", "Go forward in portal", UiAction::PortalGoForward),
                    ("Reload", "Reload portal", UiAction::PortalReload),
                ] {
                    if ui
                        .add_enabled(
                            embedded,
                            egui::Button::new(label).min_size(crate::ui_shell::layout_policy::control_size(ui.ctx(), [28.0, 24.0])),
                        )
                        .named(ui, name)
                        .on_disabled_hover_text(
                            "Embedded browser navigation is unavailable in this build",
                        )
                        .on_hover_text(name)
                        .clicked()
                    {
                        ui_mgr.push_action(action);
                    }
                }
                if !ui_mgr.portal.is_bundle()
                    && ui
                    .add(egui::Button::new("Open externally").min_size(crate::ui_shell::layout_policy::control_size(ui.ctx(), [28.0, 24.0])))
                    .named(ui, "Open portal externally")
                    .on_hover_text("Request this URL in your system browser")
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::PortalOpenExternal);
                }
                if ui
                    .add(egui::Button::new("Close").min_size(crate::ui_shell::layout_policy::control_size(ui.ctx(), [28.0, 24.0])))
                    .named(ui, "Close portal")
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::ClosePortal);
                }
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(ui_mgr.portal.hostname()).color(theme::TEXT_DIM),
                )
                .truncate(),
            )
            .on_hover_text(ui_mgr.portal.url());
            render_bundle_chrome(ui, ui_mgr);
            render_feedback(ui, ui_mgr);
            ui.separator();

            // The rest of the panel is empty — the native webview renders over it.
            ui_mgr.portal_content_rect = Some(ui.available_rect_before_wrap());
            ui.allocate_space(ui.available_size());
        });
}
