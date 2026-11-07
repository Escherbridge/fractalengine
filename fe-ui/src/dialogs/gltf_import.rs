//! Native glTF import dialog; see dialogs/AGENTS.md.

use crate::ui_shell::layout_policy::WindowTargets;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use super::ActiveDialog;
use crate::actions::{UiManager, send_or_toast};
use crate::navigation_manager::NavigationManager;
use crate::panels::widgets;
use crate::theme;
use fe_runtime::messages::DbCommand;

/// D-06: Import may fire only with a non-blank path AND an active petal —
/// importing into nothing silently did nothing before this.
pub(crate) fn import_enabled(file_path: &str, petal_id: &str) -> bool {
    !file_path.trim().is_empty() && !petal_id.trim().is_empty()
}

/// Bound before the request is marked as having an unknown outcome.
const IMPORT_LOCK_TIMEOUT_SECONDS: f64 = 30.0;

/// Begin ownership only for a command accepted by the database channel.
pub(crate) fn submit_import(
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    cmd: DbCommand,
    ui_mgr: &mut UiManager,
    now: f64,
) -> bool {
    let DbCommand::ImportGltf {
        request: Some(owner),
        ..
    } = &cmd
    else {
        return false;
    };
    let owner = owner.clone();
    if send_or_toast(db_tx, cmd, ui_mgr, now, "import the model") {
        ui_mgr.gltf_import_request.start(owner, now);
        true
    } else {
        false
    }
}

pub fn render_gltf_import_dialog(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    world_scale: f64,
) {
    let petal_id = nav.active_petal_id.clone().unwrap_or_default();

    // Read before the `active_dialog` sub-borrow below, same reasoning as
    // `join.rs::render_join_dialog` (the borrow of `ui_mgr.active_dialog`
    // through `file_path_buf`/`name_buf`/`position` must end before `ui_mgr`
    // as a whole can be passed to `send_or_toast`/`close_dialog` again).
    let now = ctx.input(|i| i.time);
    ui_mgr
        .gltf_import_request
        .expire(now, IMPORT_LOCK_TIMEOUT_SECONDS);
    let pending = ui_mgr.gltf_import_request.pending;
    let importing = pending && !ui_mgr.gltf_import_request.outcome_unknown;
    let target = nav
        .active_verse_id
        .as_deref()
        .zip(nav.active_fractal_id.as_deref())
        .zip(nav.active_petal_id.as_deref());
    let error = ui_mgr.gltf_import_request.error.clone();
    let file_dialogs = ui_mgr.file_dialogs.clone();
    let mut dialog_failure = None;
    let mut chosen_lease = None;

    let ActiveDialog::GltfImport {
        ref mut file_path_buf,
        ref mut name_buf,
        ref mut position,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let can_import = import_enabled(file_path_buf, &petal_id) && target.is_some();
    let mut close = false;
    let mut import_request: Option<DbCommand> = None;

    let mut still_open = true;
    let mut window = crate::ui_shell::layout_policy::bounded_window(ctx, "Import GLTF Model")
        .collapsible(false)
        .resizable(false)
        .default_width(340.0)
        .max_width(crate::ui_shell::layout_policy::dialog_width(ctx, 400.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM)),
        );
    if !importing {
        window = window.open(&mut still_open);
    }
    window.show_with_targets(ctx, |ui| {
        ui.add_enabled_ui(!importing, |ui| {
            let path_label = ui.label(
                egui::RichText::new("File Path:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                ui.add(
                    crate::ui_shell::layout_policy::text_input(ui, file_path_buf)
                        .hint_text("path/to/model.glb")
                        .desired_width(ui.available_width() - 70.0),
                )
                .labelled(ui, &path_label);
                if ui
                    .add(egui::Button::new("Browse...").fill(theme::BG_BUTTON))
                    .observed(ui)
                    .clicked()
                {
                    match file_dialogs.choose(crate::file_dialogs::FileDialogRequest {
                        kind: fe_runtime::qa_debug::DialogKind::Open,
                        verse_id: nav.active_verse_id.as_deref(),
                        filters: &[("GLTF Models", &["glb", "gltf"]), ("All Files", &["*"])],
                        suggested_name: None,
                    }) {
                        Ok(Some(path)) => {
                            *file_path_buf = path.display().to_string();
                            if name_buf.is_empty() {
                                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                    *name_buf = stem.to_string();
                                }
                            }
                            chosen_lease = Some(path);
                        }
                        Ok(None) => {}
                        Err(error) => dialog_failure = Some(error.to_string()),
                    }
                }
            });

            ui.add_space(4.0);
            let name_label = ui.label(
                egui::RichText::new("Display Name:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                crate::ui_shell::layout_policy::text_input(ui, name_buf)
                    .hint_text("My Model")
                    .desired_width(f32::INFINITY),
            )
            .labelled(ui, &name_label);

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Position:")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                for (axis, val) in ["X", "Y", "Z"].iter().zip(position.iter()) {
                    ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
                    ui.label(
                        egui::RichText::new(format!(
                            "{:.2} m",
                            widgets::world_to_meters(*val, world_scale)
                        ))
                        .monospace()
                        .small(),
                    );
                }
            });
        });

        if importing {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label("Importing\u{2026}");
            });
        } else if let Some(err) = error.as_deref() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(err).color(theme::STATUS_OFFLINE));
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !pending && can_import,
                    egui::Button::new("Import").fill(theme::BG_SAVE),
                )
                .observed(ui)
                .clicked()
            {
                match file_dialogs.lease_path(
                    std::path::Path::new(file_path_buf.trim()),
                    fe_runtime::qa_debug::DialogKind::Open,
                    nav.active_verse_id.as_deref(),
                ) {
                    Ok(path) => {
                        let import_owner = target.map(|((verse, fractal), petal)| {
                            fe_runtime::messages::NativeImportRequest {
                                request_id: ulid::Ulid::new().to_string(),
                                verse_id: verse.into(),
                                fractal_id: fractal.into(),
                                petal_id: petal.into(),
                            }
                        });
                        import_request = Some(DbCommand::ImportGltf {
                            request: import_owner,
                            file_lease: path.read_lease(),
                            petal_id: petal_id.clone(),
                            name: if name_buf.trim().is_empty() {
                                "Imported Model".to_string()
                            } else {
                                name_buf.trim().to_string()
                            },
                            file_path: file_path_buf.trim().to_string(),
                            position: *position,
                        });
                        chosen_lease = Some(path);
                    }
                    Err(error) => dialog_failure = Some(error.to_string()),
                }
            }
            if ui
                .add_enabled(
                    !importing,
                    egui::Button::new("Cancel").fill(theme::BG_BUTTON),
                )
                .observed(ui)
                .clicked()
            {
                close = true;
            }
        });
    });

    // The sub-borrow into `ui_mgr.active_dialog` ended above, so `ui_mgr` as
    // a whole can be mutated again from here down.
    if let Some(path) = chosen_lease {
        ui_mgr.gltf_file_lease = Some(path);
    }
    if let Some(error) = dialog_failure {
        ui_mgr.show_error(error, now);
    }
    if let Some(cmd) = import_request {
        submit_import(db_tx, cmd, ui_mgr, now);
    }

    if !still_open || close {
        ui_mgr.gltf_import_request.reset();
        ui_mgr.close_dialog();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gltf_import_disabled_without_petal_or_path() {
        let cases = [
            ("", "", false, "neither set"),
            ("model.glb", "", false, "no active petal"),
            ("", "petal-1", false, "no path"),
            ("model.glb", "petal-1", true, "both set"),
            (
                "   ",
                "petal-1",
                false,
                "whitespace-only path is still blank",
            ),
            (
                "model.glb",
                "   ",
                false,
                "whitespace-only petal id is still blank",
            ),
        ];
        for (file_path, petal_id, expected, why) in cases {
            assert_eq!(
                import_enabled(file_path, petal_id),
                expected,
                "{why}: import_enabled({file_path:?}, {petal_id:?})"
            );
        }

        let (db_tx, db_rx) = crossbeam::channel::bounded(1);
        let mut ui = UiManager::default();
        let owner = fe_runtime::messages::NativeImportRequest {
            request_id: "accepted-only".into(),
            verse_id: "v".into(),
            fractal_id: "f".into(),
            petal_id: "p".into(),
        };
        let command = || DbCommand::ImportGltf {
            request: Some(owner.clone()),
            file_lease: None,
            petal_id: "p".into(),
            name: "private name".into(),
            file_path: "private path".into(),
            position: [0.0; 3],
        };
        db_tx.try_send(DbCommand::LoadHierarchy).unwrap();
        assert!(!submit_import(&db_tx, command(), &mut ui, 1.0));
        assert!(!ui.gltf_import_request.pending);
        let error = ui.latest_unacknowledged_error().unwrap();
        assert!(error.text.contains("busy") && error.text.contains("not sent"));
        assert!(
            !error.text.contains("private"),
            "admission errors cannot expose the command payload"
        );
        assert!(matches!(db_rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
        assert!(
            db_rx.try_recv().is_err(),
            "a full queue never schedules automatic creation retry"
        );
        assert!(submit_import(&db_tx, command(), &mut ui, 2.0));
        assert!(ui.gltf_import_request.pending);
        assert!(
            matches!(db_rx.try_recv(),Ok(DbCommand::ImportGltf {request:Some(request),..}) if request==owner)
        );
        assert!(db_rx.try_recv().is_err());
        ui.gltf_import_request.reset();
        drop(db_rx);
        assert!(!submit_import(&db_tx, command(), &mut ui, 3.0));
        assert!(!ui.gltf_import_request.pending);
        assert!(
            ui.latest_unacknowledged_error()
                .unwrap()
                .text
                .contains("not connected")
        );

        for (physical_width, scale) in [(1024.0, 1.0), (640.0, 2.0)] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            ctx.enable_accesskit();
            ctx.style_mut(|style| {
                style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(10.0));
            });
            let mut manager = UiManager::default();
            manager.open_dialog(ActiveDialog::GltfImport {
                file_path_buf: String::new(), name_buf: String::new(), position: [0.0; 3],
            });
            let mut nav = NavigationManager::default();
            nav.navigate_to_petal_in_fractal("v", "Verse", "f", "Fractal", "p");
            let (tx, rx) = crossbeam::channel::unbounded();
            let ordinary_icon_width = ctx.style().spacing.icon_width;
            let mut close_bounds = None;
            for frame in 0..3 {
                crate::ui_shell::layout_policy::begin_frame(&ctx, true);
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                        egui::vec2(physical_width / scale, 720.0 / scale))),
                    time: Some(frame as f64), ..Default::default()
                }, |ctx| {
                    crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                    render_gltf_import_dialog(ctx, &mut manager, &nav, &tx, 1.0);
                });
                close_bounds = output.platform_output.accesskit_update.unwrap().nodes.iter()
                    .find_map(|(_, node)| (node.label() == Some("Close window")).then(|| node.bounds()).flatten());
                assert_eq!(ctx.style().spacing.icon_width, ordinary_icon_width);
            }
            let observations = crate::ui_shell::layout_policy::observations(&ctx);
            let fields: Vec<_> = observations.controls.iter().filter(|control| control.draggable).collect();
            assert_eq!(fields.len(), 2, "the actual import renders its two editable fields");
            for field in fields {
                let visible = field.hit.intersect(field.clip);
                assert!(visible.height() >= 24.0 && visible.height() * scale >= 20.0,
                    "the actual native field retains its target at {physical_width}px/{scale}x");
            }
            assert!(matches!(manager.active_dialog, ActiveDialog::GltfImport { .. }));
            assert!(rx.try_recv().is_err(), "drawing the form cannot import a blank path");
            let close = close_bounds.expect("the nonpending native import window has its real close control");
            assert!(close.width() >= 24.0 && close.height() >= 24.0);
            let center = egui::pos2((close.x0 + close.x1) as f32 / 2.0, (close.y0 + close.y1) as f32 / 2.0);
            for (frame, pressed) in [(3, true), (4, false)] {
                let _ = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                        egui::vec2(physical_width / scale, 720.0 / scale))),
                    time: Some(frame as f64),
                    events: vec![egui::Event::PointerMoved(center), egui::Event::PointerButton {
                        pos: center, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                    }], ..Default::default()
                }, |ctx| render_gltf_import_dialog(ctx, &mut manager, &nav, &tx, 1.0));
            }
            assert!(matches!(manager.active_dialog, ActiveDialog::None), "native Close retains the normal cancel path");
            assert!(rx.try_recv().is_err());
            manager.open_dialog(ActiveDialog::GltfImport {
                file_path_buf: "model.glb".into(), name_buf: "Model".into(), position: [0.0; 3],
            });
            manager.gltf_import_request.start(owner.clone(), 0.0);
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                    egui::vec2(physical_width / scale, 720.0 / scale))),
                time: Some(5.5), ..Default::default()
            }, |ctx| render_gltf_import_dialog(ctx, &mut manager, &nav, &tx, 1.0));
            assert!(!output.platform_output.accesskit_update.unwrap().nodes.iter()
                .any(|(_, node)| node.label() == Some("Close window")), "pending import still withholds native Close");
            let request = &manager.gltf_import_request;
            assert!(request.pending && !request.outcome_unknown && 5.5 - request.sent_at < IMPORT_LOCK_TIMEOUT_SECONDS);
            assert_eq!(ctx.style().spacing.icon_width, ordinary_icon_width);
            assert!(rx.try_recv().is_err());
        }

        use crate::file_dialogs::{FileDialogProvider, FileDialogRequest, QaFileDialogs};
        use fe_runtime::qa_debug::{DialogAnswer, DialogAnswerBatch, DialogKind};
        let root = std::env::temp_dir().join(format!(
            "fe-dialog-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("model.glb"), b"fixture").unwrap();
        let provider = QaFileDialogs::new(root.clone()).unwrap();
        let request = |kind| FileDialogRequest {
            kind,
            verse_id: Some("v"),
            filters: &[],
            suggested_name: None,
        };
        let answer = |kind, path: Option<&str>, cancel| DialogAnswerBatch {
            answers: vec![DialogAnswer {
                kind,
                path: path.map(str::to_owned),
                cancel,
            }],
        };
        assert_eq!(
            provider
                .enqueue("v", &answer(DialogKind::Open, Some("model.glb"), false))
                .unwrap_err()
                .code,
            "qa_controls_disabled"
        );
        provider.set_listener("127.0.0.1:8765".parse().unwrap());
        assert_eq!(
            provider.choose(request(DialogKind::Open)).unwrap_err().code,
            "dialog_answer_missing"
        );
        for path in [
            "../escape.glb",
            "\\\\server\\share\\file.glb",
            "model.glb:stream",
        ] {
            assert!(
                provider
                    .enqueue("v", &answer(DialogKind::Open, Some(path), false))
                    .is_err(),
                "unsafe path must be refused: {path}"
            );
        }
        #[cfg(windows)]
        for path in [
            "NUL",
            "CON.txt",
            "PRN ",
            "AUX.",
            "COM1.gpx",
            "LPT9.hexon",
            "COM¹",
            "LPT²",
            "COM³",
            "CONIN$",
            "CONOUT$",
            "C:relative.glb",
        ] {
            assert!(
                provider
                    .enqueue("v", &answer(DialogKind::Save, Some(path), false))
                    .is_err(),
                "Windows device/relative alias must be refused: {path}"
            );
        }
        provider
            .enqueue("v", &answer(DialogKind::Open, Some("model.glb"), false))
            .unwrap();
        let selected = provider.choose(request(DialogKind::Open)).unwrap().unwrap();
        assert_eq!(selected.file_name().unwrap(), "model.glb");
        drop(selected);
        provider
            .enqueue("v", &answer(DialogKind::Save, Some("model.glb"), false))
            .unwrap();
        let overwrite = provider.choose(request(DialogKind::Save)).unwrap().unwrap();
        overwrite
            .write_bytes(b"operation-owned lease released")
            .unwrap();
        drop(overwrite);
        provider
            .enqueue("v", &answer(DialogKind::Open, None, true))
            .unwrap();
        assert!(
            provider
                .enqueue(
                    "other",
                    &answer(DialogKind::Open, Some("../invalid"), false)
                )
                .is_err()
        );
        assert!(
            provider
                .choose(request(DialogKind::Open))
                .unwrap()
                .is_none()
        );
        provider
            .enqueue("v", &answer(DialogKind::Open, None, true))
            .unwrap();
        assert_eq!(
            provider.choose(request(DialogKind::Save)).unwrap_err().code,
            "dialog_answer_kind_mismatch"
        );
        provider
            .enqueue("v", &answer(DialogKind::Open, None, true))
            .unwrap();
        assert_eq!(
            provider
                .choose(FileDialogRequest {
                    verse_id: Some("other"),
                    ..request(DialogKind::Open)
                })
                .unwrap_err()
                .code,
            "dialog_answer_scope_changed"
        );
        provider
            .enqueue("v", &answer(DialogKind::Save, Some("export.glb"), false))
            .unwrap();
        let selected = provider.choose(request(DialogKind::Save)).unwrap().unwrap();
        selected.write_bytes(b"saved").unwrap();
        drop(selected);
        assert_eq!(std::fs::read(root.join("export.glb")).unwrap(), b"saved");
        let outside = root.with_extension("outside");
        std::fs::write(&outside, b"outside unchanged").unwrap();
        std::fs::hard_link(&outside, root.join("linked.glb")).unwrap();
        provider
            .enqueue("v", &answer(DialogKind::Save, Some("linked.glb"), false))
            .unwrap();
        let selected = provider.choose(request(DialogKind::Save)).unwrap().unwrap();
        selected.write_bytes(b"inside replacement").unwrap();
        drop(selected);
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"outside unchanged",
            "QA saves replace the selected entry instead of writing through a hard link"
        );
        std::fs::remove_file(outside).unwrap();
        provider
            .enqueue("v", &answer(DialogKind::Folder, Some("."), false))
            .unwrap();
        assert!(
            provider
                .choose(request(DialogKind::Folder))
                .unwrap()
                .unwrap()
                .is_dir()
        );
        drop(provider);
        std::fs::remove_dir_all(root).unwrap();
    }
}
