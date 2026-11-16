//! Models section body (Browse, Installed, Credits); see `fe-ui/src/model_library/AGENTS.md`.

use std::collections::{HashMap, HashSet};

use bevy_egui::egui;

use super::{
    InstallState, InstalledPackage, LicenceKind, Load, ModelDetail, ModelLibraryState, ModelRow,
    ModelView, PendingModelOps, ThumbnailCache, is_installed, is_placing,
};
use crate::accessibility::ResponseExt;
use crate::panels::widgets::{clipped_value, confirm_button, role_denial_reason, role_gated};
use crate::plugin::LocalUserRole;
use crate::theme;

/// Error text colour, as in the Maps section.
const ERROR_TEXT: egui::Color32 = egui::Color32::from_rgb(200, 80, 80);
/// Thumbnail side in a list row (points).
const ROW_THUMBNAIL: f32 = 40.0;
/// Thumbnail side in the detail view (points).
const DETAIL_THUMBNAIL: f32 = 96.0;

/// What a Browse click asks for, applied after the row borrows end.
enum BrowseIntent {
    Install(String),
    Details(String),
}

/// What an Installed click asks for.
enum InstalledIntent {
    Place { uri: String, entry_id: String },
    Remove(String),
}

/// Render the Models section body; the caller provides the section chrome.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut ModelLibraryState,
    pending: &mut PendingModelOps,
    local_role: &LocalUserRole,
    active_petal_id: Option<&str>,
) {
    view_tabs(ui, state);
    state.prepare(active_petal_id, pending);
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    let view = state.view;
    let detail_open = state.selected.is_some();
    match view {
        ModelView::Browse if detail_open => detail_view(ui, state, pending),
        ModelView::Browse => browse_view(ui, state, pending),
        ModelView::Installed => installed_view(ui, state, pending, local_role, active_petal_id),
        ModelView::Credits => credits_view(ui, state, pending, active_petal_id),
    }
}

fn view_tabs(ui: &mut egui::Ui, state: &mut ModelLibraryState) {
    ui.horizontal_wrapped(|ui| {
        for (view, label, name) in [
            (ModelView::Browse, "Browse", "Browse registry models"),
            (ModelView::Installed, "Installed", "Installed models"),
            (ModelView::Credits, "Credits", "Petal credits"),
        ] {
            let fill = if state.view == view { theme::BG_TAB_ACTIVE } else { theme::BG_TAB_INACTIVE };
            if ui
                .add(egui::Button::new(egui::RichText::new(label).color(theme::TEXT_BRIGHT)).fill(fill))
                .named(ui, name)
                .clicked()
            {
                state.view = view;
            }
        }
    });
}

/// Spinner while loading, error with Retry on failure, the value when ready.
fn load_state<'a, T>(
    ui: &mut egui::Ui,
    load: &'a Load<T>,
    loading_text: &str,
    retry_name: &str,
    retry: &mut bool,
) -> Option<&'a T> {
    match load {
        Load::NotAsked | Load::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(loading_text);
            });
            None
        }
        Load::Failed(error) => {
            ui.add(egui::Label::new(egui::RichText::new(error.as_str()).color(ERROR_TEXT)).wrap());
            if ui
                .add(egui::Button::new("Retry").fill(theme::BG_BUTTON))
                .named(ui, retry_name)
                .clicked()
            {
                *retry = true;
            }
            None
        }
        Load::Ready(value) => Some(value),
    }
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(theme::TEXT_MUTED)).wrap());
}

fn error_line(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).small().color(ERROR_TEXT)).wrap());
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().strong().color(theme::TEXT_SECTION));
}

/// One full-width card, as the Maps section's installed cards.
fn card<R>(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(theme::BG_BUTTON)
        .inner_margin(egui::Margin::same(8))
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        })
        .inner
}

/// Thumbnail slot; fetches only when the slot is on screen. Placeholder until decoded pixels arrive.
fn thumbnail(
    ui: &mut egui::Ui,
    row: &ModelRow,
    thumbnails: &mut ThumbnailCache,
    pending: &mut PendingModelOps,
    side: f32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    if let Some(op) = thumbnails.request(&row.uri, row.preview.as_deref(), row.preview_bytes) {
        pending.0.push(op);
    }
    match thumbnails.texture(ui.ctx(), &row.uri) {
        Some((texture, size)) if size.x > 0.0 && size.y > 0.0 => {
            let scale = (side / size.x).min(side / size.y);
            let fitted = egui::Rect::from_center_size(rect.center(), size * scale);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(texture, fitted, uv, egui::Color32::WHITE);
        }
        _ => {
            ui.painter().rect_filled(rect, 3.0, theme::BG_PANEL);
        }
    }
}

fn browse_view(ui: &mut egui::Ui, state: &mut ModelLibraryState, pending: &mut PendingModelOps) {
    let search = ui
        .add(
            crate::ui_shell::layout_policy::text_input(ui, &mut state.search_text)
                .hint_text("Search models")
                .desired_width(ui.available_width()),
        )
        .named(ui, "Search registry models");
    let submitted = search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));

    let mut toggled_tag: Option<String> = None;
    if !state.available_tags.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for tag in &state.available_tags {
                let on = state.tag_filter.contains(tag);
                let fill = if on { theme::BG_TAB_ACTIVE } else { theme::BG_TAB_INACTIVE };
                if ui
                    .add(egui::Button::new(egui::RichText::new(tag.as_str()).small()).fill(fill))
                    .named(ui, format_args!("Filter by tag {tag}"))
                    .clicked()
                {
                    toggled_tag = Some(tag.clone());
                }
            }
        });
    }
    ui.add_space(6.0);

    let mut retry = false;
    let mut intent = None;
    {
        let ModelLibraryState { browse, thumbnails, installs, installed, search_text, tag_filter, .. } =
            &mut *state;
        match load_state(ui, browse, "Loading registry...", "Retry registry listing", &mut retry) {
            None => {}
            Some(rows) if rows.is_empty() => {
                let filtered = !search_text.trim().is_empty() || !tag_filter.is_empty();
                muted(ui, if filtered { "No models match" } else { "No model packages published yet" });
            }
            Some(rows) => {
                for row in rows {
                    if let Some(clicked) = registry_row(ui, row, thumbnails, installs, installed, pending) {
                        intent = Some(clicked);
                    }
                    ui.add_space(6.0);
                }
            }
        }
    }

    if submitted || retry {
        state.request_listing(pending);
    } else if let Some(tag) = toggled_tag {
        state.toggle_tag(&tag, pending);
    }
    match intent {
        Some(BrowseIntent::Install(uri)) => state.install(&uri, pending),
        Some(BrowseIntent::Details(uri)) => state.open_detail(&uri, pending),
        None => {}
    }
}

fn registry_row(
    ui: &mut egui::Ui,
    row: &ModelRow,
    thumbnails: &mut ThumbnailCache,
    installs: &HashMap<String, InstallState>,
    installed: &Load<Vec<InstalledPackage>>,
    pending: &mut PendingModelOps,
) -> Option<BrowseIntent> {
    let mut intent = None;
    card(ui, |ui| {
        ui.horizontal(|ui| {
            thumbnail(ui, row, thumbnails, pending, ROW_THUMBNAIL);
            ui.vertical(|ui| {
                clipped_value(ui, &row.title, theme::TEXT_BRIGHT);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(row.licence.chip.as_str()).small().color(theme::TEXT_BRIGHT));
                    ui.label(egui::RichText::new(row.size_label.as_str()).small().color(theme::TEXT_MUTED));
                });
                clipped_value(ui, &row.publisher, theme::TEXT_DIM);
                if let Some(attribution) = &row.licence.attribution {
                    ui.add(
                        egui::Label::new(egui::RichText::new(attribution.as_str()).small().color(theme::TEXT_MUTED))
                            .truncate(),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    install_action(ui, row, installs, installed, &mut intent);
                    if ui
                        .add(egui::Button::new("Details").fill(theme::BG_BUTTON))
                        .named(ui, format_args!("Details for {}", row.title))
                        .clicked()
                    {
                        intent = Some(BrowseIntent::Details(row.uri.clone()));
                    }
                });
            });
        });
    });
    intent
}

/// Install, or the reason there is none (paid, too large), or install state.
fn install_action(
    ui: &mut egui::Ui,
    row: &ModelRow,
    installs: &HashMap<String, InstallState>,
    installed: &Load<Vec<InstalledPackage>>,
    intent: &mut Option<BrowseIntent>,
) {
    if row.licence.kind == LicenceKind::Paid {
        ui.label(egui::RichText::new("Not supported in this build").small().color(theme::TEXT_MUTED));
        return;
    }
    if row.too_large() {
        ui.label(egui::RichText::new("Too large to install (64 MiB limit)").small().color(theme::TEXT_MUTED));
        return;
    }
    if is_installed(installed, &row.package.hexon_id) {
        ui.label(egui::RichText::new("Installed").small().italics().color(theme::TEXT_MUTED));
        return;
    }
    match installs.get(&row.uri) {
        Some(InstallState::Running { received_bytes, total_bytes }) => {
            match total_bytes.filter(|total| *total > 0) {
                Some(total) => {
                    let fraction = (*received_bytes as f32 / total as f32).clamp(0.0, 1.0);
                    ui.add(egui::ProgressBar::new(fraction).desired_width(140.0).text("Installing\u{2026}"));
                }
                None => {
                    ui.spinner();
                    ui.label(egui::RichText::new("Installing\u{2026}").small().color(theme::TEXT_MUTED));
                }
            }
        }
        Some(InstallState::Failed(error)) => {
            error_line(ui, error);
            if ui
                .add(egui::Button::new("Retry install").fill(theme::BG_SAVE))
                .named(ui, format_args!("Retry install {}", row.title))
                .clicked()
            {
                *intent = Some(BrowseIntent::Install(row.uri.clone()));
            }
        }
        None => {
            if ui
                .add(egui::Button::new("Install").fill(theme::BG_SAVE))
                .named(ui, format_args!("Install {}", row.title))
                .clicked()
            {
                *intent = Some(BrowseIntent::Install(row.uri.clone()));
            }
        }
    }
}

fn detail_view(ui: &mut egui::Ui, state: &mut ModelLibraryState, pending: &mut PendingModelOps) {
    let mut back = false;
    let mut retry = false;
    let mut intent = None;
    {
        let ModelLibraryState { selected, detail, browse, thumbnails, installs, installed, .. } = &mut *state;
        let Some(uri) = selected.as_deref() else { return };
        if ui
            .add(egui::Button::new("Back").fill(theme::BG_BUTTON))
            .named(ui, "Back to model list")
            .clicked()
        {
            back = true;
        }
        ui.add_space(6.0);
        let row = match &*browse {
            Load::Ready(rows) => rows.iter().find(|row| row.uri == uri),
            _ => None,
        };
        if let Some(row) = row {
            ui.horizontal(|ui| {
                thumbnail(ui, row, thumbnails, pending, DETAIL_THUMBNAIL);
                ui.vertical(|ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(row.title.as_str()).strong().color(theme::TEXT_BRIGHT))
                            .wrap(),
                    );
                    ui.label(egui::RichText::new(row.size_label.as_str()).small().color(theme::TEXT_MUTED));
                    ui.horizontal_wrapped(|ui| install_action(ui, row, installs, installed, &mut intent));
                });
            });
            ui.add_space(6.0);
        }
        if let Some(detail) = load_state(ui, detail, "Loading details...", "Retry package details", &mut retry) {
            detail_body(ui, detail, row.map(|row| row.publisher.as_str()));
        }
    }

    if back {
        state.selected = None;
    } else if retry {
        if let Some(uri) = state.selected.clone() {
            state.open_detail(&uri, pending);
        }
    } else if let Some(BrowseIntent::Install(uri)) = intent {
        state.install(&uri, pending);
    }
}

/// Signed licence, attribution, publisher and the models a package contains.
fn detail_body(ui: &mut egui::Ui, detail: &ModelDetail, listed_publisher: Option<&str>) {
    if detail.listing_out_of_date {
        muted(ui, "Listing out of date");
    }
    if !detail.description.is_empty() {
        ui.add(egui::Label::new(egui::RichText::new(detail.description.as_str()).color(theme::TEXT_BRIGHT)).wrap());
        ui.add_space(6.0);
    }
    if let Some(publisher) = detail.publisher_name.as_deref().or(listed_publisher) {
        heading(ui, "Publisher");
        clipped_value(ui, publisher, theme::TEXT_DIM);
    }
    heading(ui, "Licence");
    ui.label(egui::RichText::new(detail.licence.chip.as_str()).small().color(theme::TEXT_BRIGHT));
    if let Some(attribution) = &detail.licence.attribution {
        heading(ui, "Attribution");
        ui.add(egui::Label::new(egui::RichText::new(attribution.as_str()).small().color(theme::TEXT_BRIGHT)).wrap());
    }
    ui.add_space(6.0);
    heading(ui, "Models");
    for model in &detail.models {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(model.label.as_str()).color(theme::TEXT_BRIGHT));
            for tag in &model.tags {
                ui.label(egui::RichText::new(tag.as_str()).small().color(theme::TEXT_MUTED));
            }
        });
    }
}

/// Why Place is unavailable, or `None` when it is allowed.
fn place_denial(local_role: &LocalUserRole, active_petal_id: Option<&str>) -> Option<String> {
    if active_petal_id.is_none() {
        Some("Navigate to a petal first".to_owned())
    } else if !local_role.can_edit() {
        Some(role_denial_reason(local_role, "Editor", None))
    } else {
        None
    }
}

fn installed_view(
    ui: &mut egui::Ui,
    state: &mut ModelLibraryState,
    pending: &mut PendingModelOps,
    local_role: &LocalUserRole,
    active_petal_id: Option<&str>,
) {
    let denial = place_denial(local_role, active_petal_id);
    let mut retry = false;
    let mut intent = None;
    {
        let ModelLibraryState { installed, placing, removing, remove_error, place_error, .. } = &*state;
        match load_state(ui, installed, "Loading installed models...", "Retry installed models", &mut retry) {
            None => {}
            Some(packages) if packages.is_empty() => muted(ui, "No model packages installed"),
            Some(packages) => {
                if let Some(reason) = &denial {
                    muted(ui, reason);
                }
                if let Some(error) = place_error {
                    error_line(ui, error);
                }
                if let Some(error) = remove_error {
                    error_line(ui, error);
                }
                for package in packages {
                    if let Some(clicked) =
                        installed_card(ui, package, placing, removing, denial.as_deref())
                    {
                        intent = Some(clicked);
                    }
                    ui.add_space(6.0);
                }
            }
        }
    }

    if retry {
        state.request_installed(pending);
    }
    match intent {
        Some(InstalledIntent::Place { uri, entry_id }) => {
            if let Some(petal_id) = active_petal_id {
                state.place(&uri, &entry_id, petal_id, pending);
            }
        }
        Some(InstalledIntent::Remove(uri)) => state.remove(&uri, pending),
        None => {}
    }
}

fn installed_card(
    ui: &mut egui::Ui,
    package: &InstalledPackage,
    placing: &HashMap<String, (String, String)>,
    removing: &HashSet<String>,
    denial: Option<&str>,
) -> Option<InstalledIntent> {
    let mut intent = None;
    card(ui, |ui| {
        clipped_value(ui, &package.title, theme::TEXT_BRIGHT);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(package.licence.chip.as_str()).small().color(theme::TEXT_BRIGHT));
            ui.label(egui::RichText::new(package.size_label.as_str()).small().color(theme::TEXT_MUTED));
        });
        clipped_value(ui, &package.publisher, theme::TEXT_DIM);
        ui.add_space(4.0);
        for model in &package.models {
            ui.horizontal(|ui| {
                if model.placeable {
                    if is_placing(placing, &package.uri, &model.entry_id) {
                        ui.add_enabled(false, egui::Button::new("Placing\u{2026}").fill(theme::BG_BUTTON))
                            .named(ui, format_args!("Placing {}", model.label));
                    } else if role_gated(
                        ui,
                        denial.is_none(),
                        denial.unwrap_or_default(),
                        egui::Button::new("Place").fill(theme::BG_SAVE),
                    )
                    .named(ui, format_args!("Place {} from {}", model.label, package.title))
                    .clicked()
                    {
                        intent = Some(InstalledIntent::Place {
                            uri: package.uri.clone(),
                            entry_id: model.entry_id.clone(),
                        });
                    }
                }
                ui.add(egui::Label::new(egui::RichText::new(model.label.as_str()).color(theme::TEXT_BRIGHT)).truncate());
            });
        }
        ui.add_space(4.0);
        if removing.contains(&package.uri) {
            ui.add_enabled(false, egui::Button::new("Removing\u{2026}").fill(theme::BG_BUTTON))
                .named(ui, format_args!("Removing {}", package.title));
        } else if confirm_button(
            ui,
            ("model-remove", package.uri.as_str()),
            "Remove",
            "Confirm remove",
            format_args!("Remove model package {}", package.title),
        ) {
            intent = Some(InstalledIntent::Remove(package.uri.clone()));
        }
    });
    intent
}

fn credits_view(
    ui: &mut egui::Ui,
    state: &mut ModelLibraryState,
    pending: &mut PendingModelOps,
    active_petal_id: Option<&str>,
) {
    let Some(petal_id) = active_petal_id else {
        muted(ui, "Navigate to a petal to see its credits");
        return;
    };
    let mut retry = false;
    if let Some(credits) = load_state(ui, &state.credits, "Loading credits...", "Retry petal credits", &mut retry) {
        if credits.packages.is_empty() && credits.sources_not_installed == 0 {
            muted(ui, "No library models in this petal");
        }
        for package in &credits.packages {
            card(ui, |ui| {
                clipped_value(ui, &package.title, theme::TEXT_BRIGHT);
                clipped_value(ui, &package.publisher, theme::TEXT_DIM);
                ui.label(egui::RichText::new(package.licence.chip.as_str()).small().color(theme::TEXT_BRIGHT));
                if let Some(attribution) = &package.licence.attribution {
                    ui.add(
                        egui::Label::new(egui::RichText::new(attribution.as_str()).small().color(theme::TEXT_BRIGHT))
                            .wrap(),
                    );
                }
            });
            ui.add_space(6.0);
        }
        match credits.sources_not_installed {
            0 => {}
            1 => muted(ui, "Source not installed here: 1 asset"),
            count => muted(ui, &format!("Source not installed here: {count} assets")),
        }
    }
    if retry {
        state.request_credits(petal_id, pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_library::{
        Credits, Licence, ModelEntryRow, ModelOp, ModelResult, ModelRow, InstalledPackage,
    };
    use crate::terrain_map::dto::RegistryPackageRow;

    const MIB: u64 = 1024 * 1024;

    fn collect_text(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                out.push((text.galley.text().to_owned(), shape.visual_bounding_rect()))
            }
            egui::epaint::Shape::Vec(inner) => inner.iter().for_each(|shape| collect_text(shape, out)),
            _ => {}
        }
    }

    /// Headless egui frames over the real panel, as the widgets and peer-debug tests drive theirs.
    struct Harness {
        ctx: egui::Context,
        time: f64,
        state: ModelLibraryState,
        ops: PendingModelOps,
        role: LocalUserRole,
        petal: Option<String>,
    }

    impl Harness {
        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
            crate::ui_shell::layout_policy::begin_frame(&self.ctx, true);
            self.time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 1400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let Self { ctx, state, ops, role, petal, .. } = self;
            let output = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, state, ops, role, petal.as_deref()));
            });
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
            texts
        }

        fn text(&mut self) -> String {
            let texts = self.frame(Vec::new());
            texts.into_iter().map(|(text, _)| text).collect::<Vec<_>>().join("\n")
        }

        fn count(&mut self, label: &str) -> usize {
            self.frame(Vec::new()).iter().filter(|(text, _)| text == label).count()
        }

        /// Hover, press, release on the first painted text equal to `label`.
        fn click(&mut self, label: &str) {
            let texts = self.frame(Vec::new());
            let at = texts
                .iter()
                .find(|(text, _)| text == label)
                .unwrap_or_else(|| panic!("no {label:?} painted"))
                .1
                .center();
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
        }

        fn take_ops(&mut self) -> Vec<ModelOp> {
            std::mem::take(&mut self.ops.0)
        }

        fn listing_generation(&mut self) -> u64 {
            self.take_ops()
                .into_iter()
                .find_map(|op| match op {
                    ModelOp::List { generation, .. } => Some(generation),
                    _ => None,
                })
                .expect("a registry listing was requested")
        }
    }

    fn listed_row(id: &str, name: &str, size_bytes: u64, licence: Licence) -> ModelRow {
        ModelRow::new(
            RegistryPackageRow {
                hexon_id: id.into(),
                version: "1.0.0".into(),
                name: name.into(),
                size_bytes,
                publisher_did: "did:key:z6MkPublisher".into(),
                attribution: None,
            },
            None,
            &["furniture".to_owned()],
            licence,
            Some("preview/model.png".into()),
            Some(10_000),
        )
    }

    fn listing() -> Vec<ModelRow> {
        vec![
            listed_row("oak-chair", "Oak chair", 2 * MIB, Licence::new(Some("free"), Some("CC0-1.0"), None)),
            listed_row(
                "street-lamp",
                "Street lamp\u{202E}fdp.exe",
                3 * MIB,
                Licence::new(Some("attribution"), Some("CC-BY-4.0"), Some("Lamp by A. Maker\nCC BY 4.0")),
            ),
            listed_row("harbour-crane", "Harbour crane", 65 * MIB, Licence::new(Some("free"), None, None)),
            listed_row("statue", "Paid statue", MIB, Licence::new(Some("paid"), None, None)),
            listed_row("bench", "Bench", MIB, Licence::new(None, None, None)),
        ]
    }

    fn installed_chair() -> InstalledPackage {
        InstalledPackage::new(
            "oak-chair@1.0.0".into(),
            "oak-chair".into(),
            "Oak chair",
            "did:key:z6MkPublisher",
            Some("A. Maker"),
            None,
            Licence::new(Some("free"), Some("CC0-1.0"), None),
            vec![
                ModelEntryRow::new("chair", "Chair", &[], true),
                ModelEntryRow::new("collider", "Collider", &[], false),
            ],
            2 * MIB,
        )
    }

    #[test]
    fn model_library_journey() {
        let mut harness = Harness {
            ctx: egui::Context::default(),
            time: 0.0,
            state: ModelLibraryState::default(),
            ops: PendingModelOps::default(),
            role: LocalUserRole { role: Some(fe_database::RoleLevel::Viewer) },
            petal: Some("petal-1".into()),
        };

        // Loading: opening Browse asks for the registry listing and the installed list.
        assert!(harness.text().contains("Loading registry"));
        let ops = harness.take_ops();
        assert!(ops.contains(&ModelOp::ListInstalled), "{ops:?}");
        let first = ops
            .iter()
            .find_map(|op| match op {
                ModelOp::List { generation, .. } => Some(*generation),
                _ => None,
            })
            .expect("listing requested on open");
        harness.state.apply(ModelResult::InstalledListed(Ok(Vec::new())));

        // Error with Retry (U3): Retry asks again; a late reply to the old request is dropped.
        harness.state.apply(ModelResult::Listed {
            generation: first,
            result: Err("registry unreachable: connection refused".into()),
        });
        assert!(harness.text().contains("registry unreachable: connection refused"));
        harness.click("Retry");
        let retried = harness.listing_generation();
        assert_ne!(retried, first);
        harness.state.apply(ModelResult::Listed { generation: first, result: Ok(listing()) });
        assert!(harness.text().contains("Loading registry"), "stale listing must not land");

        // Empty.
        harness.state.apply(ModelResult::Listed { generation: retried, result: Ok(Vec::new()) });
        assert!(harness.text().contains("No model packages published yet"));

        // Listing: licence chips, sanitised publisher text, U1 too large, U6 paid.
        harness.state.request_listing(&mut harness.ops);
        let generation = harness.listing_generation();
        harness.state.apply(ModelResult::Listed { generation, result: Ok(listing()) });
        let text = harness.text();
        for expected in [
            "Free \u{00b7} CC0-1.0",
            "Attribution \u{00b7} CC-BY-4.0",
            "Not stated",
            "Street lampfdp.exe",
            "Lamp by A. Maker CC BY 4.0",
            "Too large to install (64 MiB limit)",
            "Not supported in this build",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(!text.contains('\u{202E}'), "bidi override reached the screen");
        assert_eq!(harness.count("Install"), 3, "too-large and paid rows offer no Install");
        let thumbnail_fetches = harness
            .take_ops()
            .into_iter()
            .filter(|op| matches!(op, ModelOp::Thumbnail { .. }))
            .count();
        assert_eq!(thumbnail_fetches, crate::model_library::MAX_THUMBNAIL_FETCHES, "five visible rows, four fetches in flight");

        // Install: progress shows on the row, then the package lands under Installed (F5).
        harness.click("Install");
        assert!(harness.take_ops().contains(&ModelOp::Install { uri: "oak-chair@1.0.0".into() }));
        assert!(harness.text().contains("Installing"));
        harness.state.apply(ModelResult::Installed { uri: "oak-chair@1.0.0".into(), result: Ok(installed_chair()) });
        assert_eq!(harness.count("Installed"), 2, "the tab plus the installed row's status");
        assert_eq!(harness.count("Install"), 2);

        // U7: a Viewer sees Place disabled with the role reason; a click places nothing.
        harness.click("Installed");
        let text = harness.text();
        assert!(text.contains("Editor role needed \u{2014} you are Viewer"), "{text}");
        assert_eq!(harness.count("Place"), 1, "U4: the non-placeable entry has no Place");
        harness.click("Place");
        assert!(!harness.take_ops().iter().any(|op| matches!(op, ModelOp::Place { .. })));

        // An Editor places into the active petal; the button waits for the import result.
        harness.role = LocalUserRole { role: Some(fe_database::RoleLevel::Editor) };
        harness.click("Place");
        let request_id = harness
            .take_ops()
            .into_iter()
            .find_map(|op| match op {
                ModelOp::Place { request_id, uri, entry_id, petal_id } => {
                    assert_eq!((uri.as_str(), entry_id.as_str(), petal_id.as_str()), ("oak-chair@1.0.0", "chair", "petal-1"));
                    Some(request_id)
                }
                _ => None,
            })
            .expect("Editor Place queues an op");
        assert!(crate::model_library::is_library_request(&request_id));
        assert!(harness.text().contains("Placing\u{2026}"));
        harness.state.apply(ModelResult::Placed { request_id });
        assert_eq!(harness.count("Place"), 1);

        // No active petal: Place is disabled with its own reason.
        harness.petal = None;
        assert!(harness.text().contains("Navigate to a petal first"));

        // Credits for the active petal, with a used asset whose package is not installed here.
        harness.petal = Some("petal-1".into());
        harness.click("Credits");
        assert!(harness.take_ops().contains(&ModelOp::LoadCredits { petal_id: "petal-1".into() }));
        harness.state.apply(ModelResult::Credits {
            petal_id: "petal-1".into(),
            result: Ok(Credits { packages: vec![installed_chair()], sources_not_installed: 1 }),
        });
        let text = harness.text();
        assert!(text.contains("Oak chair") && text.contains("Free \u{00b7} CC0-1.0"), "{text}");
        assert!(text.contains("Source not installed here: 1 asset"), "{text}");
    }
}
