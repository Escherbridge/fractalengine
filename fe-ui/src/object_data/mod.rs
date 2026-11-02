//! Native object history, measurements and schema views; see AGENTS.md.

mod history;
mod measurements;
mod schema;

use std::time::{Duration, Instant};

use bevy_egui::egui;
use fe_runtime::messages::{DbCommand, DbResult};

use crate::accessibility::ResponseExt;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::verse_manager::VerseManager;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Target {
    node_id: String,
    petal_id: String,
    verse_id: String,
    fractal_id: String,
    scope: String,
    scopes: [String; 3],
}

pub(super) struct Request {
    id: String,
    started: Instant,
}

impl Request {
    fn new(kind: &str) -> Self {
        Self {
            id: crate::actions::next_ui_query_id(kind),
            started: Instant::now(),
        }
    }

    fn matches(&self, id: &str) -> bool {
        self.id == id
    }
    fn expired(&self) -> bool {
        self.expired_at(Instant::now())
    }

    fn expired_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) > Duration::from_secs(30)
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectDataSection {
    #[default]
    History,
    Measurements,
    Schema,
}

#[derive(Default)]
pub(crate) struct ObjectDataState {
    target: Option<Target>,
    section: ObjectDataSection,
    history: history::HistoryState,
    measurements: measurements::MeasurementsState,
    pub(crate) schema: schema::SchemaState,
}

impl ObjectDataState {
    pub(crate) fn activate(&mut self, section: ObjectDataSection) {
        self.section = section;
    }
}

pub(crate) fn synchronize(
    state: &mut ObjectDataState,
    nodes: &NodeManager,
    tree: &VerseManager,
    nav: &NavigationManager,
    role: &crate::plugin::LocalUserRole,
) {
    let can_view = role.role.is_some_and(|role| role.can_view());
    if can_view
        && state.target.as_ref().is_some_and(|target| {
            nodes
                .selected
                .as_ref()
                .is_some_and(|selected| selected.node_id == target.node_id)
                && nav.active_verse_id.as_deref() == Some(target.verse_id.as_str())
                && nav.active_fractal_id.as_deref() == Some(target.fractal_id.as_str())
                && nav.active_petal_id.as_deref() == Some(target.petal_id.as_str())
                && tree.petal_id_of(&target.node_id) == Some(target.petal_id.as_str())
        })
    {
        return;
    }
    let target = nodes.selected.as_ref().and_then(|selected| {
        if !can_view {
            return None;
        }
        let petal_id = tree.petal_id_of(&selected.node_id)?;
        if nav.active_petal_id.as_deref() != Some(petal_id) {
            return None;
        }
        let verse_id = nav.active_verse_id.as_deref()?;
        let fractal_id = nav.active_fractal_id.as_deref()?;
        let scope = nav.active_scope()?;
        Some(Target {
            node_id: selected.node_id.clone(),
            petal_id: petal_id.to_owned(),
            verse_id: verse_id.to_owned(),
            fractal_id: fractal_id.to_owned(),
            scopes: [
                fe_database::build_scope(verse_id, None, None),
                fe_database::build_scope(verse_id, Some(fractal_id), None),
                scope.clone(),
            ],
            scope,
        })
    });
    if state.target != target {
        let section = state.section;
        *state = ObjectDataState {
            target,
            section,
            ..Default::default()
        };
    }
}

pub(crate) fn route(state: &mut ObjectDataState, result: &DbResult) -> bool {
    let Some(target) = &state.target else {
        return false;
    };
    state.schema.note_definitions(result);
    state.history.route(target, result)
        || state.measurements.route(target, result)
        || state.schema.route(target, result)
}

pub(crate) fn render(
    ui: &mut egui::Ui,
    state: &mut ObjectDataState,
    role: &crate::plugin::LocalUserRole,
    sender: &crossbeam::channel::Sender<DbCommand>,
    notices: &mut crate::actions::UiManager,
) {
    if state.target.is_none() {
        ui.label("Select an object in the active Petal with Viewer access to inspect its data.");
        return;
    }
    ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(target_size(ui));
    ui.horizontal_wrapped(|ui| {
        for (section, label) in [
            (ObjectDataSection::History, "History"),
            (ObjectDataSection::Measurements, "Measurements"),
            (ObjectDataSection::Schema, "Enforced schema"),
        ] {
            if ui
                .add(
                    egui::Button::new(label)
                        .selected(state.section == section)
                        .min_size(egui::Vec2::splat(target_size(ui))),
                )
                .named(ui, format_args!("Object data section: {label}"))
                .clicked()
            {
                state.activate(section);
            }
        }
    });
    ui.separator();
    let target = state.target.as_ref().expect("target checked above");
    match state.section {
        ObjectDataSection::History => state.history.render(ui, target, sender, notices),
        ObjectDataSection::Measurements => state.measurements.render(ui, target, sender, notices),
        ObjectDataSection::Schema => state.schema.render(ui, target, role, sender),
    }
}

pub(super) fn button(ui: &mut egui::Ui, text: &str, name: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(egui::Vec2::splat(target_size(ui))),
    )
    .named(ui, name)
    .clicked()
}

pub(super) fn target_size(ui: &egui::Ui) -> f32 {
    24.0_f32.max(20.0 / ui.ctx().pixels_per_point())
}

pub(super) fn error(ui: &mut egui::Ui, error: &Option<String>) {
    if let Some(error) = error {
        ui.colored_label(crate::theme::STATUS_OFFLINE, error);
    }
}

pub(super) fn send(
    sender: &crossbeam::channel::Sender<DbCommand>,
    command: DbCommand,
) -> Result<(), String> {
    match sender.try_send(command) {
        Ok(()) => Ok(()),
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            Err("Database is busy. Your draft is retained; try again when it is ready.".to_owned())
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            bevy::log::warn!("DB channel closed while requesting object data");
            Err("Database connection closed. Your draft is retained.".to_owned())
        }
    }
}

pub(super) fn request_error(result: &DbResult) -> Option<(&str, &str)> {
    match result {
        DbResult::RequestFailed {
            correlation_id,
            error,
        }
        | DbResult::QueryFailed {
            correlation_id,
            error,
        } => Some((correlation_id, error)),
        _ => None,
    }
}

pub(super) fn json_view(ui: &mut egui::Ui, id: &str, text: &str) {
    egui::ScrollArea::both()
        .id_salt(id)
        .max_height(220.0)
        .show(ui, |ui| {
            ui.monospace(text);
        });
}
