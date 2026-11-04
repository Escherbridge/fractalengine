//! Bounded commit browsing and read-only exact-version replay.

use std::time::{Duration, Instant};

use bevy_egui::egui;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_runtime::node_history::{
    HistoryAvailability, NodeAccessGrant, NodeHistoryPage, NodeStateAtVersion,
};

use super::{Request, Target, button, error, json_view, request_error, send};
use crate::accessibility::ResponseExt;

#[derive(Default)]
pub(super) struct HistoryState {
    page: Option<NodeHistoryPage>,
    page_request: Option<(Request, i64)>,
    preview: Option<NodeStateAtVersion>,
    preview_json: String,
    preview_request: Option<(Request, i64)>,
    previous: Vec<i64>,
    current_after: i64,
    version: String,
    error: Option<String>,
    playing: bool,
    next_frame: Option<Instant>,
}

impl HistoryState {
    fn completed_version(&self, version: i64) -> i64 {
        self.page
            .as_ref()
            .and_then(|page| {
                page.entries.iter().find_map(|entry| {
                    entry
                        .atomic_save
                        .as_ref()
                        .filter(|save| {
                            (save.first_version..=save.committed_version).contains(&version)
                        })
                        .map(|save| save.committed_version)
                })
            })
            .unwrap_or(version)
    }

    fn load_page(
        &mut self,
        target: &Target,
        after: i64,
        sender: &crossbeam::channel::Sender<DbCommand>,
    ) {
        let request = Request::new("object-history");
        match send(
            sender,
            DbCommand::ReadNodeHistory {
                node_id: target.node_id.clone(),
                after_version: after,
                limit: 50,
                at_version: None,
                grant: NodeAccessGrant::Local,
                correlation_id: Some(request.id.clone()),
            },
        ) {
            Ok(()) => {
                self.page_request = Some((request, after));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn load_version(
        &mut self,
        target: &Target,
        version: i64,
        sender: &crossbeam::channel::Sender<DbCommand>,
    ) {
        let request = Request::new("object-replay");
        match send(
            sender,
            DbCommand::ReadNodeHistory {
                node_id: target.node_id.clone(),
                after_version: 0,
                limit: 1,
                at_version: Some(version),
                grant: NodeAccessGrant::Local,
                correlation_id: Some(request.id.clone()),
            },
        ) {
            Ok(()) => {
                self.preview_request = Some((request, version));
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                self.playing = false;
            }
        }
    }

    pub(super) fn route(&mut self, target: &Target, result: &DbResult) -> bool {
        match result {
            DbResult::NodeHistoryLoaded {
                page,
                correlation_id: Some(id),
            } if self
                .page_request
                .as_ref()
                .is_some_and(|(request, _)| request.matches(id)) =>
            {
                let (_, after) = self.page_request.take().expect("matched request");
                if page.coverage.node_id != target.node_id
                    || page
                        .entries
                        .iter()
                        .any(|entry| entry.node_id != target.node_id || entry.row_version <= after)
                {
                    self.error = Some(
                        "History response did not match the selected object and cursor.".into(),
                    );
                } else {
                    self.current_after = after;
                    self.page = Some(page.clone());
                }
                true
            }
            DbResult::NodeStateAtVersionLoaded {
                state,
                correlation_id: Some(id),
            } if self
                .preview_request
                .as_ref()
                .is_some_and(|(request, _)| request.matches(id)) =>
            {
                let (_, requested) = self.preview_request.take().expect("matched request");
                if state.coverage.node_id != target.node_id
                    || state.at_version != requested
                    || state
                        .state
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.node_id != target.node_id)
                {
                    self.error =
                        Some("Replay response did not match the requested object version.".into());
                    self.playing = false;
                } else {
                    self.version = state.at_version.to_string();
                    if !matches!(
                        state.availability,
                        HistoryAvailability::Available | HistoryAvailability::InsideAtomicBatch
                    ) {
                        self.playing = false;
                    }
                    self.preview_json = state
                        .state
                        .as_ref()
                        .map(|snapshot| {
                            serde_json::to_string_pretty(&snapshot.properties).unwrap_or_default()
                        })
                        .unwrap_or_default();
                    self.preview = Some(state.clone());
                    self.next_frame = Some(Instant::now() + Duration::from_millis(700));
                }
                true
            }
            _ => {
                let Some((id, message)) = request_error(result) else {
                    return false;
                };
                let page = self
                    .page_request
                    .as_ref()
                    .is_some_and(|(request, _)| request.matches(id));
                let preview = self
                    .preview_request
                    .as_ref()
                    .is_some_and(|(request, _)| request.matches(id));
                if page {
                    self.page_request = None;
                }
                if preview {
                    self.preview_request = None;
                }
                if page || preview {
                    self.error = Some(message.to_owned());
                    self.playing = false;
                }
                page || preview
            }
        }
    }

    pub(super) fn render(
        &mut self,
        ui: &mut egui::Ui,
        target: &Target,
        sender: &crossbeam::channel::Sender<DbCommand>,
        notices: &mut crate::actions::UiManager,
    ) {
        if self
            .page_request
            .as_ref()
            .is_some_and(|(request, _)| request.expired())
        {
            self.page_request = None;
            self.error = Some("History request timed out. Refresh to try again.".into());
        }
        if self
            .preview_request
            .as_ref()
            .is_some_and(|(request, _)| request.expired())
        {
            self.preview_request = None;
            self.playing = false;
            self.error =
                Some("Replay request timed out. Your version selection is retained.".into());
        }
        if self.page.is_none() && self.page_request.is_none() && self.error.is_none() {
            self.load_page(target, 0, sender);
        }
        ui.label(
            "Commit versions order changes to this object. Measurement timestamps are independent.",
        );
        if button(
            ui,
            "Refresh history",
            "Refresh object history",
            self.page_request.is_none(),
        ) {
            self.previous.clear();
            self.load_page(target, 0, sender);
        }
        if self.page_request.is_some() {
            ui.label("Loading committed history...");
        }
        error(ui, &self.error);
        let mut selected_version = None;
        let mut exact_version = false;
        let mut next_cursor = None;
        let mut previous_cursor = None;
        if let Some(page) = &self.page {
            ui.label(format!(
                "Current committed version: {}",
                page.coverage.current_version
            ));
            match page.coverage.coverage_start_version {
                Some(version) => {
                    ui.label(format!("Replay coverage begins at version {version}."));
                }
                None => {
                    ui.label("No complete replay baseline is available. Retained facts are audit history only.");
                }
            }
            if !page.coverage.current_state_verified {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Current state has changes outside complete replay coverage.",
                );
            }
            ui.small(
                "History below is read-only. Replaying a version leaves the live scene unchanged.",
            );
            if page.entries.is_empty() {
                ui.label("No retained changes on this page.");
            }
            egui::ScrollArea::vertical()
                .id_salt("object-history-rows")
                .max_height(230.0)
                .show(ui, |ui| {
                    for entry in &page.entries {
                        ui.push_id(entry.row_version, |ui| {
                            let label = match &entry.atomic_save {
                                Some(save) => format!(
                                    "v{}  {} · saved together v{}–{}",
                                    entry.row_version,
                                    entry.op,
                                    save.first_version,
                                    save.committed_version
                                ),
                                None => format!("v{}  {}", entry.row_version, entry.op),
                            };
                            if ui
                                .add_enabled(
                                    !matches!(
                                        entry.replay_availability,
                                        Some(HistoryAvailability::IncompleteAtomicBatch)
                                    ),
                                    egui::Button::new(&label)
                                        .min_size(egui::Vec2::splat(super::target_size(ui))),
                                )
                                .named(
                                    ui,
                                    format_args!(
                                        "Replay object version {}",
                                        entry
                                            .atomic_save
                                            .as_ref()
                                            .map_or(entry.row_version, |save| save
                                                .committed_version)
                                    ),
                                )
                                .on_hover_text(format!(
                                    "Recorded: {}\nActor: {}",
                                    entry.created_at, entry.source_did
                                ))
                                .clicked()
                            {
                                selected_version = Some(
                                    entry
                                        .atomic_save
                                        .as_ref()
                                        .map_or(entry.row_version, |save| save.committed_version),
                                );
                            }
                        });
                    }
                });
            ui.horizontal_wrapped(|ui| {
                if button(
                    ui,
                    "Previous page",
                    "Previous object history page",
                    !self.previous.is_empty() && self.page_request.is_none(),
                ) {
                    previous_cursor = self.previous.last().copied();
                }
                if button(
                    ui,
                    "Next page",
                    "Next object history page",
                    page.next_after_version.is_some()
                        && self.page_request.is_none()
                        && self.previous.len() < 64,
                ) {
                    next_cursor = page.next_after_version;
                }
            });
            if self.previous.len() >= 64 {
                ui.small("Page navigation limit reached. Refresh starts again at the first page.");
            }
        }
        if let Some(cursor) = previous_cursor {
            self.previous.pop();
            self.load_page(target, cursor, sender);
        }
        if let Some(cursor) = next_cursor {
            self.previous.push(self.current_after);
            self.load_page(target, cursor, sender);
        }
        ui.separator();
        ui.label("Exact committed version");
        ui.small("Rows saved together replay at their completed Save version. Individual audit facts remain listed above.");
        ui.add(
            crate::ui_shell::layout_policy::text_input(ui, &mut self.version)
                .desired_width(ui.available_width())
                .char_limit(20),
        )
        .named(ui, "Object replay version");
        ui.horizontal_wrapped(|ui| {
            if button(
                ui,
                "Read version",
                "Read exact object version",
                self.preview_request.is_none(),
            ) {
                match self.version.trim().parse::<i64>() {
                    Ok(version) if version > 0 => {
                        selected_version = Some(version);
                        exact_version = true;
                        self.playing = false;
                    }
                    _ => self.error = Some("Enter a positive committed version number.".into()),
                }
            }
            let current = self.preview.as_ref().map(|preview| preview.at_version);
            let start = self
                .page
                .as_ref()
                .and_then(|page| page.coverage.coverage_start_version);
            let end = self.page.as_ref().map(|page| page.coverage.current_version);
            if button(
                ui,
                "Earlier",
                "Replay earlier object version",
                self.preview_request.is_none()
                    && current
                        .zip(start)
                        .is_some_and(|(value, start)| value > start),
            ) {
                selected_version = self.preview.as_ref().and_then(|preview| {
                    preview
                        .atomic_save
                        .as_ref()
                        .map_or(preview.at_version, |save| save.first_version)
                        .checked_sub(1)
                });
            }
            if button(
                ui,
                "Later",
                "Replay later object version",
                self.preview_request.is_none()
                    && current.zip(end).is_some_and(|(value, end)| value < end),
            ) {
                selected_version = self.preview.as_ref().and_then(|preview| {
                    if matches!(preview.availability, HistoryAvailability::InsideAtomicBatch) {
                        preview
                            .atomic_save
                            .as_ref()
                            .map(|save| save.committed_version)
                    } else {
                        preview.at_version.checked_add(1)
                    }
                });
            }
            if button(
                ui,
                if self.playing {
                    "Pause replay"
                } else {
                    "Play replay"
                },
                "Toggle read-only object replay",
                self.playing || start.is_some(),
            ) {
                self.playing = !self.playing;
                if self.playing {
                    selected_version = start;
                }
            }
        });
        if self.playing
            && self.preview_request.is_none()
            && self
                .next_frame
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            let next = self.preview.as_ref().and_then(|preview| {
                if matches!(preview.availability, HistoryAvailability::InsideAtomicBatch) {
                    preview
                        .atomic_save
                        .as_ref()
                        .map(|save| save.committed_version)
                } else {
                    preview.at_version.checked_add(1)
                }
            });
            let end = self.page.as_ref().map(|page| page.coverage.current_version);
            if next.zip(end).is_some_and(|(next, end)| next <= end) {
                selected_version = next;
            } else {
                self.playing = false;
            }
        }
        if let Some(version) = selected_version {
            let version = if exact_version {
                version
            } else {
                self.completed_version(version)
            };
            self.load_version(target, version, sender);
        }
        if self.playing || self.preview_request.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if self.preview_request.is_some() {
            ui.label("Reading historical state...");
        }
        if let Some(preview) = &self.preview {
            ui.strong(format!(
                "Historical preview: version {}",
                preview.at_version
            ));
            if let Some(snapshot) = &preview.state {
                ui.label(format!("Position: {:?}", snapshot.position));
                ui.label(format!("Stored rotation: {:?}", snapshot.rotation));
                ui.label(format!("Scale: {:?}", snapshot.scale));
                if button(
                    ui,
                    "Copy historical properties",
                    "Copy historical object properties",
                    true,
                ) {
                    ui.ctx().copy_text(self.preview_json.clone());
                    notices
                        .show_toast("Copied historical properties", ui.input(|input| input.time));
                }
                json_view(ui, "historical-object-properties", &self.preview_json);
            } else {
                ui.label(availability_label(&preview.availability));
                if let Some(save) = &preview.atomic_save {
                    ui.label(format!(
                        "Versions {}–{} were saved together. The completed Save is version {}.",
                        save.first_version, save.committed_version, save.committed_version
                    ));
                    if button(
                        ui,
                        &format!("Read saved version {}", save.committed_version),
                        "Read completed atomic Save",
                        self.preview_request.is_none(),
                    ) {
                        let version = save.committed_version;
                        self.load_version(target, version, sender);
                    }
                }
            }
        }
    }
}

fn availability_label(availability: &HistoryAvailability) -> &'static str {
    match availability {
        HistoryAvailability::Available => "Historical state is available.",
        HistoryAvailability::InsideAtomicBatch => {
            "This audit version is inside one atomic Save. Its intermediate fields were never a separately committed object state."
        }
        HistoryAvailability::IncompleteAtomicBatch => {
            "The atomic Save receipt or its contiguous facts cannot be verified. No partial object state is shown."
        }
        HistoryAvailability::BeforeCoverage => {
            "This version precedes the complete replay baseline."
        }
        HistoryAvailability::IncompleteInterval => {
            "A history gap prevents exact reconstruction of this version."
        }
        HistoryAvailability::UnsupportedOperation => {
            "This interval includes an operation without a replay rule."
        }
        HistoryAvailability::VersionUnavailable => {
            "The requested committed version is unavailable."
        }
        HistoryAvailability::UnavailableScope => {
            "This historical interval is not available with your current access."
        }
    }
}
