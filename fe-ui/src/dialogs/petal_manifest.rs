//! Petal hexon manifest editor dialog.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::ActiveDialog;
use crate::actions::{UiManager, UiAction};
use crate::terrain_map::manifest::ManifestHexonEntry;
use crate::theme;
use crate::ui_shell::modal::{
    close_confirmation_scope, confirmation_scope, dirty_close, DirtyClose,
};

pub fn render_petal_manifest(ctx: &egui::Context, ui_mgr: &mut UiManager, nav: &crate::navigation_manager::NavigationManager, db: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>) {
    super::requirements_state::request_if_needed(ui_mgr, nav, db, ctx.input(|i|i.time));
    let ActiveDialog::PetalManifest {
        ref petal_id,
        ref petal_name,
        ref mut manifest,
        ref available_hexon_ids,
        ref mut add_hexon_id_buf,
        ref mut add_hexon_type_buf,
        ref mut dirty,
        ref mut requirements_state,
        ..
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let current_petal_id = petal_id.clone();
    let mut save = None;
    let mut review = None;
    let mut close = false;
    let mut still_open = true;
    let scope = confirmation_scope(ctx, "petal_manifest", &current_petal_id);
    let mut discard_requested = ctx.data(|data| {
        data.get_temp::<bool>(scope.with("discard"))
            .unwrap_or(false)
    });
    let mut discard_confirmed = false;
    let mut copied_json = false;
    let mut copy_failed = false;

    crate::ui_shell::layout_policy::bounded_window(ctx, format!("Petal Manifest — {}", petal_name))
        .open(&mut still_open)
        .collapsible(false)
        .resizable(true)
        .default_width(600.0)
        .min_width(crate::ui_shell::layout_policy::dialog_width(ctx,450.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(16))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        )
        .show_with_targets(ctx, |ui| {
            if discard_requested {
                ui.heading("Discard unsaved requirements?");
                ui.label("Your draft has not been saved. Discard it or return to editing.");
                ui.horizontal_wrapped(|ui| {
                    if ui.add(egui::Button::new("Discard changes").fill(theme::BG_DANGER)).named(ui, "Discard petal requirements changes").clicked() { discard_confirmed = true; }
                    if ui.button("Keep editing").named(ui, "Keep editing petal requirements").clicked() { discard_requested = false; }
                });
                return;
            }

            ui.label(
                egui::RichText::new("Declare content dependencies for this Petal. This list does not install content or change scene visibility.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(8.0);

            ui.label(requirements_state.status(*dirty || !add_hexon_id_buf.is_empty() || !add_hexon_type_buf.is_empty()));
            if *dirty {ui.label("Unsaved draft changes.");}
            if let Some(error) = &requirements_state.error { ui.colored_label(theme::STATUS_OFFLINE, error); }
            if requirements_state.error.is_some() && ui.button("Reload saved data (keep draft)").observed(ui).clicked() { requirements_state.refresh(); }
            if requirements_state.needs_review {
                ui.label("Saved data differs from your retained draft. Review it before saving.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Use saved declaration").observed(ui).clicked() { review=Some(true); }
                    if ui.button("Keep my draft against loaded data").observed(ui).clicked() { review=Some(false); }
                });
            }
            if !requirements_state.editable() {
                if ui.button("Copy retained draft JSON").observed(ui).clicked() {
                    match serde_json::to_string_pretty(&*manifest) {
                        Ok(json)=>{ui.ctx().copy_text(json);copied_json=true;},
                        Err(_)=>copy_failed=true,
                    }
                }
                if ui.button("Close").observed(ui).clicked() { close=true; }
                return;
            }
            ui.label(egui::RichText::new("Render distance is configured in Settings.").small().color(theme::TEXT_DIM));
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // --- Required hexons table ---
            ui.label(egui::RichText::new("Required Hexons").color(theme::TEXT_SECTION).strong());
            ui.add_space(4.0);

            if manifest.hexons.is_empty() {
                ui.label(
                    egui::RichText::new("No hexons assigned. Add one below.")
                        .color(theme::TEXT_MUTED)
                        .italics(),
                );
            } else {
                let mut remove_idx: Option<usize> = None;

                egui::Grid::new("manifest_hexons_grid")
                    .num_columns(5)
                    .striped(true)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        // Header
                        ui.label(egui::RichText::new("Hexon ID").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("Type").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("Required").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("Status").color(theme::TEXT_SECTION).strong());
                        ui.label(egui::RichText::new("").color(theme::TEXT_SECTION));
                        ui.end_row();

                        for (i, entry) in manifest.hexons.iter_mut().enumerate() {
                            ui.label(&entry.hexon_id);
                            ui.label(&entry.hexon_type);

                            if ui.checkbox(&mut entry.required, "").named(ui, format_args!("Require hexon {}", entry.hexon_id)).changed() {
                                *dirty = true;
                            }

                            // Status: installed locally?
                            let installed = available_hexon_ids.contains(&entry.hexon_id);
                            if installed {
                                ui.label(
                                    egui::RichText::new("Installed").color(egui::Color32::from_rgb(100, 200, 100)),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new(if available_hexon_ids.is_empty() { "Availability unknown" } else { "Not listed locally" }).color(egui::Color32::from_rgb(200, 100, 80)),
                                );
                            }

                            if ui
                                .add(egui::Button::new("X").fill(theme::BG_DANGER).min_size(egui::Vec2::splat(crate::ui_shell::layout_policy::target_size(ui.ctx())))).named(ui, format_args!("Remove hexon {} from manifest", entry.hexon_id))
                                .on_hover_text("Remove from manifest")
                                .clicked()
                            {
                                remove_idx = Some(i);
                            }
                            ui.end_row();
                        }
                    });

                if let Some(idx) = remove_idx {
                    manifest.hexons.remove(idx);
                    *dirty = true;
                }
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // --- Add hexon ---
            ui.label(egui::RichText::new("Add Hexon").color(theme::TEXT_SECTION).strong());
            ui.add_space(4.0);
            let mut manual = ctx.data(|data| data.get_temp::<bool>(scope.with("manual_id")).unwrap_or(available_hexon_ids.is_empty()));
            if available_hexon_ids.is_empty() {
                manual = true;
                ui.label("Installed choices are unavailable here. Enter a hexon ID manually.");
            } else {
                egui::ComboBox::from_id_salt(scope.with("installed_hexons"))
                    .selected_text(if add_hexon_id_buf.is_empty() { "Choose an installed hexon" } else { add_hexon_id_buf.as_str() })
                    .show_ui(ui, |ui| {
                        for id in available_hexon_ids.iter().filter(|id| !id.trim().is_empty()) {
                            let choice = ui.selectable_value(add_hexon_id_buf, id.clone(), id).named(ui, format_args!("Choose installed hexon {id}"));
                            if choice.changed() { *dirty = true; }
                            if choice.clicked() { manual = false; }
                        }
                    }).response.named(ui, "Installed hexon choices");
                ui.checkbox(&mut manual, "Enter another ID manually").observed(ui);
            }
            if manual
                && ui.add(crate::ui_shell::layout_policy::text_input(ui, add_hexon_id_buf).hint_text("hexon-id").desired_width(ui.available_width())).named(ui, "Hexon ID to add").changed() { *dirty = true; }
            ctx.data_mut(|data| data.insert_temp(scope.with("manual_id"), manual));
            ui.horizontal_wrapped(|ui| {
                ui.label("Type:");
                const TYPES: [&str;12] = fe_runtime::petal_requirements::HEXON_TYPES;
                egui::ComboBox::from_id_salt(scope.with("hexon_type"))
                    .selected_text(if add_hexon_type_buf.is_empty() { "Choose type" } else { add_hexon_type_buf.as_str() })
                    .show_ui(ui, |ui| {
                        for kind in TYPES {
                            if ui.selectable_value(add_hexon_type_buf, kind.to_string(), kind).observed(ui).changed() { *dirty = true; }
                        }
                    }).response.named(ui, "Hexon type to add");
                let duplicate = manifest.hexons.iter().any(|entry| entry.hexon_id == add_hexon_id_buf.trim());
                let can_add = manifest.hexons.len() < 256 && fe_runtime::safe_id::is_safe_path_id(add_hexon_id_buf.trim()) && !duplicate && TYPES.contains(&add_hexon_type_buf.as_str());
                if ui.add_enabled(can_add, egui::Button::new("Add").fill(theme::BG_SAVE)).observed(ui)
                    .on_disabled_hover_text("Choose an ID and type; an existing requirement cannot be added twice.").clicked() {
                    manifest.hexons.push(ManifestHexonEntry { hexon_id: add_hexon_id_buf.trim().to_string(), hexon_type: add_hexon_type_buf.clone(), required: true });
                    add_hexon_id_buf.clear();
                    add_hexon_type_buf.clear();
                    *dirty = true;
                }
            });

            ui.label(egui::RichText::new("Click Add to include the ID and type fields in the requirements JSON.").small().color(theme::TEXT_DIM));

            // --- Sharing hint ---
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Availability on another device depends on its installed content and reachable peers.")
                    .small()
                    .italics()
                    .color(theme::TEXT_MUTED),
            );

            // JSON preview (collapsible)
            ui.add_space(8.0);
            egui::CollapsingHeader::new(
                egui::RichText::new("Requirements JSON").small().color(theme::TEXT_DIM),
            )
            .default_open(false)
            .show(ui, |ui| {
                let json = serde_json::to_string_pretty(&*manifest).unwrap_or_default();
                ui.add(
                    egui::TextEdit::multiline(&mut json.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                )
                .named(ui, "Manifest JSON preview");
            }).header_response.observed(ui);

            // --- Save / Close ---
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let valid = manifest.validate().is_ok();
                if ui.add_enabled(*dirty && valid && add_hexon_id_buf.is_empty() && add_hexon_type_buf.is_empty() && requirements_state.can_save(), egui::Button::new("Save").fill(theme::BG_SAVE)).named(ui,"Save petal requirements").on_disabled_hover_text("Load saved data, use Manager access and apply valid Add fields before saving.").clicked() {
                    save=Some(manifest.clone());
                }
                if !valid { ui.label("Requirements contain an invalid or duplicate ID or unsupported type."); }
                if ui.button("Copy JSON").named(ui, "Copy petal requirements JSON").clicked() {
                    match serde_json::to_string_pretty(&*manifest) {
                        Ok(json) => { ui.ctx().copy_text(json); copied_json = true; }
                        Err(_) => copy_failed = true,
                    }
                }
                if ui.add(egui::Button::new("Close").fill(theme::BG_BUTTON)).observed(ui).clicked() {
                    close = true;
                }
            });
        });

    let decision = dirty_close(
        *dirty,
        !still_open || close,
        discard_requested && discard_confirmed,
    );
    if decision == DirtyClose::AskDiscard {
        discard_requested = true;
    }
    ctx.data_mut(|data| data.insert_temp(scope.with("discard"), discard_requested));
    if decision == DirtyClose::Close {
        close_confirmation_scope(ctx, "petal_manifest");
        ui_mgr.close_dialog();
    }
    if let Some(use_saved)=review {super::requirements_state::review(ui_mgr,use_saved);}
    if let Some(manifest)=save {
        ui_mgr.push_action(UiAction::PetalManifestSave {petal_id:current_petal_id,manifest});
    }
    if copied_json {
        ui_mgr.show_toast(
            "Requirements JSON copied; unapplied Add fields are excluded and the draft is not saved",
            ctx.input(|input| input.time),
        );
    }
    if copy_failed {
        ui_mgr.show_error(
            "Requirements JSON could not be generated; your draft is unchanged",
            ctx.input(|input| input.time),
        );
    }
}
