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

pub(crate) fn status_bar(
    ctx: &egui::Context,
    dashboard: &DashboardState,
    sync_status: Option<&fe_sync::SyncStatus>,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    modal: &ModalManagerState,
    // hierarchy_visibility_groups_20260808 RATIFICATION #16: persistent
    // "N hidden" chip — absent when nothing is hidden in the active petal.
    // `&mut` because F19's memoization cache lives on `VisibilityState`
    // itself (this fn is a plain call, not a Bevy system — no `Local<T>`
    // available to cache in here directly).
    hierarchy: &VerseManager,
    vis_state: &mut VisibilityState,
    // F6: startup identity-degradation warning — session-sticky until the
    // user clicks it away (fractalengine/src/AGENTS.md §identity-tiers).
    identity_banner: &mut crate::plugin::IdentityBanner,
    local_role: &crate::plugin::LocalUserRole,
    node_identity: Option<&fe_identity::NodeIdentity>,
) {
    let minimum = 24.0_f32.max(20.0 / ctx.pixels_per_point());
    egui::TopBottomPanel::bottom("statusbar")
        .exact_height(minimum + 4.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_STATUSBAR)
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Phase F: prefer SyncStatus over DashboardState for online/peer info
                let is_online = sync_status.map(|s| s.online).unwrap_or(dashboard.is_online);
                let peer_count = sync_status
                    .map(|s| s.peer_count as u64)
                    .unwrap_or(dashboard.peer_count);

                if is_online {
                    ui.colored_label(theme::STATUS_ONLINE_DOT, "\u{25CF}");
                    ui.label(
                        egui::RichText::new("Online")
                            .small()
                            .color(theme::STATUS_ONLINE),
                    );
                } else {
                    ui.colored_label(theme::STATUS_OFFLINE_DOT, "\u{25CF}");
                    ui.label(
                        egui::RichText::new("Offline")
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }

                ui.separator();
                // Clickable peer count opens debug panel
                let peer_label = egui::RichText::new(format!(
                    "{} {}",
                    peer_count,
                    if peer_count == 1 { "peer" } else { "peers" }
                ))
                .small()
                .color(theme::STATUS_NORMAL);
                if ui
                    .add(egui::Button::new(peer_label).min_size(egui::Vec2::splat(minimum)))
                    .button_named(ui, "Open peer debug panel")
                    .on_hover_text("Open connection and peer details")
                    .clicked()
                {
                    if matches!(ui_mgr.active_dialog, ActiveDialog::PeerDebug) {
                        ui_mgr.close_dialog();
                    } else {
                        ui_mgr.open_dialog(ActiveDialog::PeerDebug);
                    }
                }

                ui.separator();
                crate::role_chip::role_status(ui, local_role, nav, node_identity);

                ui.separator();
                if let Some(counts) = content_counts(hierarchy, nav) {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "{}: {} petal{} \u{00b7} {} object{}",
                                counts.level,
                                counts.petals,
                                if counts.petals == 1 { "" } else { "s" },
                                counts.objects,
                                if counts.objects == 1 { "" } else { "s" },
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        )
                        .selectable(false),
                    )
                    .on_hover_text(format!(
                        "{}: {}. Counts include hidden objects in the loaded hierarchy.",
                        counts.level, counts.name
                    ));
                } else {
                    ui.label(
                        egui::RichText::new("Content counts unavailable")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }

                // hierarchy_visibility_groups_20260808 RATIFICATION #16: dim
                // persistent "N hidden" chip — an abnormal-condition
                // indicator per ui_ux.md §6, not a toast. Absent when zero.
                // Memoized (F19): counts HIERARCHY ROWS, not spawned
                // entities — see `visibility::hidden_count_in_active_petal`
                // docs for why that's a deliberately different number than
                // `sync_node_visibility`'s per-frame apply count.
                let hidden_count = vis_state.hidden_count_in_active_petal_cached(hierarchy, nav);
                if let Some(label) = visibility::hidden_chip_label(hidden_count) {
                    ui.separator();
                    ui.label(egui::RichText::new(label).small().color(theme::TEXT_MUTED));
                }

                // X-05: every persistent condition (e.g. "mesh budget
                // exceeded", written by W3-S2's watchdog) as a dim chip —
                // same abnormal-condition tier as the hidden-count chip above.
                for (_key, text) in ui_mgr.conditions() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("\u{26A0} {text}"))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }

                // Persistent guard-error segment (Q-5, FR-7): a disabled
                // panel's error stays visible for the session — no
                // auto-clear on petal switch, unlike the general §6
                // clears-on-resolution tier (ratified exception for this
                // guard, since the panel itself stays disabled).
                if let Some(err) = &modal.last_error {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("\u{26A0} {err}"))
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }

                // F6: identity-degradation banner — session-sticky (no
                // auto-fade like a toast, since the user may not be looking
                // when it first appears), clears only on click.
                if let Some(msg) = identity_banner.message.clone() {
                    ui.separator();
                    let resp = ui
                        .add_sized(
                            [ui.available_width().min(240.0), minimum],
                            egui::Label::new(
                                egui::RichText::new(format!("\u{26A0} {msg} (click to dismiss)"))
                                    .small()
                                    .color(theme::STATUS_OFFLINE),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        )
                        .button_named(ui, "Dismiss identity warning");
                    if resp.clicked() {
                        identity_banner.dismiss();
                    }
                }

                // X-01: the newest undismissed error under a minute old,
                // right-aligned. Drawn LAST because its right-to-left layout
                // takes the rest of the row. Click opens the history ring
                // (cap 20) with the raw detail and a Copy per entry.
                let now = ui.ctx().input(|i| i.time);
                let latest = ui_mgr
                    .latest_unacknowledged_error()
                    .filter(|notice| now - notice.spawned_at < ERROR_SEGMENT_SECONDS)
                    .map(|notice| (notice.id, format!("\u{26A0} {}", notice.text)));
                if let Some((latest_id, latest_label)) = latest {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let resp = ui
                            .add_sized(
                                [ui.available_width().min(240.0), minimum],
                                egui::Label::new(
                                    egui::RichText::new(latest_label)
                                        .small()
                                        .color(theme::STATUS_OFFLINE),
                                )
                                .truncate()
                                .sense(egui::Sense::click()),
                            )
                            .button_named(ui, "Open error history");
                        let mut dismiss_latest = false;
                        let mut copy_notice = None;
                        egui::Popup::from_toggle_button_response(&resp)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| {
                                ui.set_min_width(280.0);
                                ui.label(egui::RichText::new("Error history").strong());
                                ui.separator();
                                for notice in ui_mgr.error_history() {
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(egui::RichText::new(&notice.text).small());
                                            if let Some(detail) = &notice.detail {
                                                ui.label(
                                                    egui::RichText::new(detail)
                                                        .small()
                                                        .monospace()
                                                        .color(theme::TEXT_MUTED),
                                                );
                                            }
                                        });
                                        if ui
                                            .add(
                                                egui::Button::new("Copy")
                                                    .min_size(egui::Vec2::splat(minimum)),
                                            )
                                            .named(
                                                ui,
                                                format_args!("Copy error notice {}", notice.id),
                                            )
                                            .clicked()
                                        {
                                            let raw =
                                                notice.detail.as_ref().unwrap_or(&notice.text);
                                            copy_notice = Some(raw.clone());
                                        }
                                        if notice.id == latest_id
                                            && ui
                                                .add(
                                                    egui::Button::new("Dismiss")
                                                        .min_size(egui::Vec2::splat(minimum)),
                                                )
                                                .named(
                                                    ui,
                                                    format_args!(
                                                        "Dismiss error notice {}",
                                                        notice.id
                                                    ),
                                                )
                                                .clicked()
                                        {
                                            dismiss_latest = true;
                                        }
                                    });
                                }
                            });
                        if let Some(raw) = copy_notice {
                            super::widgets::copy_with_feedback(
                                ui,
                                ui_mgr,
                                &raw,
                                "Copied error details",
                            );
                        }
                        if dismiss_latest {
                            ui_mgr.dismiss_notice(latest_id);
                        }
                    });
                }
            });
        });
}

/// How long the newest error stays in the status bar (plan W1-S2 step 5).
const ERROR_SEGMENT_SECONDS: f64 = 60.0;
