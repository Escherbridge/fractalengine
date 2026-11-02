//! Bottom status bar: online/peer indicators, active verse, space counts, and
//! the persistent panel-guard error segment (Q-5, FR-7) — see
//! `ui_shell/AGENTS.md` §modal.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use crate::actions::UiManager;
use crate::atlas::DashboardState;
use crate::dialogs::ActiveDialog;
use crate::navigation_manager::NavigationManager;
use crate::theme;
use crate::ui_shell::modal::ModalManagerState;
use crate::verse_manager::VerseManager;
use crate::visibility::{self, VisibilityState};

struct ContentCounts<'a> {
    level: &'static str,
    name: &'a str,
    petals: usize,
    objects: usize,
}

impl<'a> ContentCounts<'a> {
    fn for_petals(
        level: &'static str,
        name: &'a str,
        petals: impl Iterator<Item = &'a crate::verse_manager::PetalEntry>,
    ) -> Self {
        let (petals, objects) = petals.fold((0, 0), |(count, objects), petal| {
            (count + 1, objects + petal.nodes.len())
        });
        Self {
            level,
            name,
            petals,
            objects,
        }
    }
}

fn content_counts<'a>(
    hierarchy: &'a VerseManager,
    nav: &NavigationManager,
) -> Option<ContentCounts<'a>> {
    let Some(verse_id) = nav.active_verse_id.as_deref() else {
        if nav.active_fractal_id.is_some() || nav.active_petal_id.is_some() {
            return None;
        }
        return Some(ContentCounts::for_petals(
            "Library",
            "All loaded content",
            hierarchy
                .verses
                .iter()
                .flat_map(|verse| &verse.fractals)
                .flat_map(|fractal| &fractal.petals),
        ));
    };
    let verse = hierarchy.verses.iter().find(|verse| verse.id == verse_id)?;
    let Some(fractal_id) = nav.active_fractal_id.as_deref() else {
        if nav.active_petal_id.is_some() {
            return None;
        }
        return Some(ContentCounts::for_petals(
            "Verse",
            &verse.name,
            verse.fractals.iter().flat_map(|fractal| &fractal.petals),
        ));
    };
    let fractal = verse
        .fractals
        .iter()
        .find(|fractal| fractal.id == fractal_id)?;
    let Some(petal_id) = nav.active_petal_id.as_deref() else {
        return Some(ContentCounts::for_petals(
            "Fractal",
            &fractal.name,
            fractal.petals.iter(),
        ));
    };
    let petal = fractal.petals.iter().find(|petal| petal.id == petal_id)?;
    Some(ContentCounts::for_petals(
        "Petal",
        &petal.name,
        std::iter::once(petal),
    ))
}

enum FooterAction {
    TogglePeers,
    DismissIdentity,
    DismissError(u64),
    CopyError(String),
}

struct FooterDetails<'a> {
    manager: &'a UiManager,
    modal: &'a ModalManagerState,
    hidden: Option<&'a str>,
    identity: Option<&'a str>,
    latest: Option<(u64, &'a str)>,
    counts: Option<&'a ContentCounts<'a>>,
    peer_count: u64,
}

fn footer_button(
    ui: &mut egui::Ui,
    caption: &str,
    name: &str,
    width: f32,
    minimum: f32,
) -> egui::Response {
    ui.add_sized(
        [width.max(minimum), minimum],
        egui::Button::new(caption).truncate(),
    )
    .button_named(ui, name)
}

fn counts_label(counts: &ContentCounts<'_>) -> String {
    format!(
        "{}: {} petal{} \u{00b7} {} object{}",
        counts.level,
        counts.petals,
        if counts.petals == 1 { "" } else { "s" },
        counts.objects,
        if counts.objects == 1 { "" } else { "s" }
    )
}

fn bounded_details(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    ui.set_max_width(520.0_f32.min((ui.ctx().viewport_rect().width() - 32.0).max(120.0)));
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    egui::ScrollArea::vertical()
        .max_height((ui.ctx().viewport_rect().height() - 100.0).max(80.0))
        .show(ui, body);
}

fn footer_actions(
    ui: &mut egui::Ui,
    details: &FooterDetails<'_>,
    minimum: f32,
    compact: bool,
) -> Option<FooterAction> {
    let mut action = None;
    let width = |normal| if compact { minimum } else { normal };
    let condition_count = details.manager.conditions().count()
        + usize::from(details.hidden.is_some())
        + usize::from(details.modal.last_error.is_some());
    let status = footer_button(
        ui,
        &if compact {
            "!".into()
        } else if condition_count > 0 {
            format!("Status {condition_count}")
        } else {
            "Status".into()
        },
        "Open status details",
        width(80.0),
        minimum,
    )
    .on_hover_text(format!(
        "{condition_count} active conditions. Open complete status details."
    ));
    egui::Popup::from_toggle_button_response(&status)
        .id(egui::Id::new("footer_status_details"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            bounded_details(ui, |ui| {
                ui.heading("Status details");
                if let Some(counts) = details.counts {
                    ui.label(format!("{}: {}", counts.level, counts.name));
                    ui.label(counts_label(counts));
                } else {
                    ui.label("Content counts unavailable");
                }
                if let Some(hidden) = details.hidden {
                    ui.label(hidden);
                }
                for (_, condition) in details.manager.conditions() {
                    ui.add(egui::Label::new(condition).wrap());
                }
                if let Some(error) = &details.modal.last_error {
                    ui.add(egui::Label::new(error).wrap());
                }
                if condition_count == 0 {
                    ui.label("No active conditions.");
                }
            })
        });

    let identity_id = egui::Id::new("footer_identity_warning");
    if let Some(message) = details.identity {
        let response = footer_button(
            ui,
            if compact { "ID" } else { "Identity !" },
            "Open identity warning",
            width(80.0),
            minimum,
        )
        .on_hover_text(message);
        egui::Popup::from_toggle_button_response(&response)
            .id(identity_id)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .layout(egui::Layout::top_down(egui::Align::Min))
            .show(|ui| {
                bounded_details(ui, |ui| {
                    ui.heading("Identity warning");
                    ui.add(egui::Label::new(message).wrap());
                    if ui
                        .add(
                            egui::Button::new("Dismiss warning")
                                .min_size(egui::Vec2::splat(minimum)),
                        )
                        .button_named(ui, "Dismiss identity warning")
                        .clicked()
                    {
                        action = Some(FooterAction::DismissIdentity);
                        ui.close();
                    }
                })
            });
    } else {
        egui::Popup::close_id(ui.ctx(), identity_id);
    }

    let response = footer_button(
        ui,
        if compact {
            "Err"
        } else if details.latest.is_some() {
            "Error"
        } else {
            "Errors"
        },
        "Open error history",
        width(72.0),
        minimum,
    )
    .on_hover_text(
        details
            .latest
            .map(|(_, text)| text)
            .unwrap_or("Open error history"),
    );
    egui::Popup::from_toggle_button_response(&response)
        .id(egui::Id::new("footer_error_history"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            bounded_details(ui, |ui| {
                ui.heading("Error history");
                let mut empty = true;
                for notice in details.manager.error_history() {
                    empty = false;
                    egui::containers::Sides::new()
                        .shrink_left()
                        .wrap()
                        .height(minimum)
                        .show(
                            ui,
                            |ui| {
                                ui.vertical(|ui| {
                                    ui.add(egui::Label::new(&notice.text).wrap());
                                    if let Some(detail) = &notice.detail {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(detail).monospace(),
                                            )
                                            .wrap(),
                                        );
                                    }
                                });
                            },
                            |ui| {
                                if footer_button(
                                    ui,
                                    "Copy",
                                    &format!("Copy error notice {}", notice.id),
                                    48.0,
                                    minimum,
                                )
                                .clicked()
                                {
                                    action = Some(FooterAction::CopyError(
                                        notice.detail.as_ref().unwrap_or(&notice.text).clone(),
                                    ));
                                }
                                if details.latest.is_some_and(|(id, _)| id == notice.id)
                                    && footer_button(
                                        ui,
                                        "Dismiss",
                                        &format!("Dismiss error notice {}", notice.id),
                                        72.0,
                                        minimum,
                                    )
                                    .clicked()
                                {
                                    action = Some(FooterAction::DismissError(notice.id));
                                }
                            },
                        );
                    ui.separator();
                }
                if empty {
                    ui.label("No recorded errors.");
                }
            })
        });

    let caption = if compact {
        details.peer_count.to_string()
    } else {
        format!(
            "{} {}",
            details.peer_count,
            if details.peer_count == 1 {
                "peer"
            } else {
                "peers"
            }
        )
    };
    if footer_button(ui, &caption, "Open peer debug panel", width(80.0), minimum)
        .on_hover_text(format!(
            "{} peers. Open connection and peer details",
            details.peer_count
        ))
        .clicked()
    {
        action = Some(FooterAction::TogglePeers);
    }
    action
}

pub(crate) fn status_bar(
    ctx: &egui::Context,
    dashboard: &DashboardState,
    sync_status: Option<&fe_sync::SyncStatus>,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    modal: &ModalManagerState,
    hierarchy: &VerseManager,
    vis_state: &mut VisibilityState,
    identity_banner: &mut crate::plugin::IdentityBanner,
    local_role: &crate::plugin::LocalUserRole,
    node_identity: Option<&fe_identity::NodeIdentity>,
) {
    let minimum = 24.0_f32.max(20.0 / ctx.pixels_per_point());
    let counts = content_counts(hierarchy, nav);
    let hidden_count = vis_state.hidden_count_in_active_petal_cached(hierarchy, nav);
    let hidden = visibility::hidden_chip_label(hidden_count);
    let now = ctx.input(|input| input.time);
    let latest = ui_mgr
        .latest_unacknowledged_error()
        .filter(|notice| now - notice.spawned_at < ERROR_SEGMENT_SECONDS)
        .map(|notice| (notice.id, notice.text.as_str()));
    let details = FooterDetails {
        manager: ui_mgr,
        modal,
        hidden: hidden.as_deref(),
        identity: identity_banner.message.as_deref(),
        latest,
        counts: counts.as_ref(),
        peer_count: sync_status
            .map(|status| status.peer_count as u64)
            .unwrap_or(dashboard.peer_count),
    };
    let online = sync_status
        .map(|status| status.online)
        .unwrap_or(dashboard.is_online);
    let mut action = None;
    egui::TopBottomPanel::bottom("statusbar")
        .exact_height(minimum + 4.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_STATUSBAR)
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ctx, |ui| {
            let compact = ui.available_width() < 600.0;
            action = egui::containers::Sides::new()
                .shrink_left()
                .truncate()
                .height(minimum)
                .show(
                    ui,
                    |ui| {
                        ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                        ui.label(
                            egui::RichText::new(if online { "Online" } else { "Offline" })
                                .small()
                                .color(theme::STATUS_NORMAL),
                        );
                        ui.separator();
                        crate::role_chip::role_status(ui, local_role, nav, node_identity);
                        ui.separator();
                        if let Some(counts) = &counts {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(counts_label(counts))
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                )
                                .selectable(false)
                                .truncate(),
                            )
                            .on_hover_text(format!(
                                "{}: {}. Counts include hidden objects in the loaded hierarchy.",
                                counts.level, counts.name
                            ));
                        } else {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new("Content counts unavailable")
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                )
                                .selectable(false)
                                .truncate(),
                            );
                        }
                    },
                    |ui| footer_actions(ui, &details, minimum, compact),
                )
                .1;
        });
    match action {
        Some(FooterAction::TogglePeers) => {
            if matches!(ui_mgr.active_dialog, ActiveDialog::PeerDebug) {
                ui_mgr.close_dialog();
            } else {
                ui_mgr.open_dialog(ActiveDialog::PeerDebug);
            }
        }
        Some(FooterAction::DismissIdentity) => identity_banner.dismiss(),
        Some(FooterAction::DismissError(id)) => ui_mgr.dismiss_notice(id),
        Some(FooterAction::CopyError(raw)) => {
            super::widgets::copy_in_context(ctx, ui_mgr, &raw, "Copied error details");
        }
        None => {}
    }
}

const ERROR_SEGMENT_SECONDS: f64 = 60.0;
