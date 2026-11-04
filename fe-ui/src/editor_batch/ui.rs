//! Shared selection and group controls; see editor_batch/AGENTS.md.

use super::affine_edits;
use crate::{
    accessibility::ResponseExt,
    actions::{UiAction, UiManager},
    navigation_manager::NavigationManager,
    node_manager::NodeManager,
    plugin::LocalUserRole,
};
use bevy::prelude::*;
use bevy_egui::egui;
use fe_runtime::editor_batch::*;
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) struct SelectionDraft {
    translation: [String; 3],
    rotation: [String; 3],
    scale: [String; 3],
    group_name: String,
    property_key: String,
    property_value: String,
}
impl Default for SelectionDraft {
    fn default() -> Self {
        Self {
            translation: std::array::from_fn(|_| "0".into()),
            rotation: std::array::from_fn(|_| "0".into()),
            scale: std::array::from_fn(|_| "1".into()),
            group_name: String::new(),
            property_key: String::new(),
            property_value: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PropertyPresence {
    Common(Value),
    Mixed,
    Missing { present: usize, total: usize },
}

pub(crate) fn property_presence(capture: &EditorBatchCapture, key: &str) -> PropertyPresence {
    let present = capture
        .members
        .iter()
        .filter(|member| member.state.properties.contains_key(key))
        .count();
    if present != capture.members.len() {
        return PropertyPresence::Missing {
            present,
            total: capture.members.len(),
        };
    }
    let first = capture
        .members
        .first()
        .and_then(|member| member.state.properties.get(key));
    if capture
        .members
        .iter()
        .all(|member| member.state.properties.get(key) == first)
    {
        first
            .cloned()
            .map(PropertyPresence::Common)
            .unwrap_or(PropertyPresence::Missing {
                present: 0,
                total: 0,
            })
    } else {
        PropertyPresence::Mixed
    }
}

fn axes(ui: &mut egui::Ui, label: &str, values: &mut [String; 3], enabled: bool) {
    ui.label(label);
    let target = crate::object_data::target_size(ui);
    ui.add_enabled_ui(enabled, |ui| {
        ui.columns(3, |columns| {
            for (index, (axis, value)) in ["X", "Y", "Z"].into_iter().zip(values).enumerate() {
                let column = &mut columns[index];
                column
                    .add_sized(
                        [column.available_width().max(target), target],
                        crate::ui_shell::layout_policy::text_input(column, value)
                            .desired_width(0.0)
                            .char_limit(32),
                    )
                    .named(column, format_args!("Selection {label} {axis}"));
            }
        })
    });
}

fn parse(values: &[String; 3]) -> Result<Vec3, String> {
    let mut result = [0.0; 3];
    for (output, input) in result.iter_mut().zip(values) {
        *output = input
            .trim()
            .parse::<f32>()
            .map_err(|_| "Each transform component must be a number")?;
        if !output.is_finite() {
            return Err("Each transform component must be finite".into());
        }
    }
    Ok(Vec3::from_array(result))
}

pub(crate) fn render(
    ui: &mut egui::Ui,
    nodes: &NodeManager,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    role: &LocalUserRole,
) {
    render_form(ui, nodes, nav, ui_mgr, role, true);
}

pub(crate) fn render_transform(
    ui: &mut egui::Ui,
    nodes: &NodeManager,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    role: &LocalUserRole,
) {
    render_form(ui, nodes, nav, ui_mgr, role, false);
}

fn render_form(
    ui: &mut egui::Ui,
    nodes: &NodeManager,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    role: &LocalUserRole,
    include_properties: bool,
) {
    let target = crate::object_data::target_size(ui);
    ui.spacing_mut().interact_size = egui::Vec2::splat(target);
    let mut state = std::mem::take(&mut ui_mgr.editor_batch);
    let mut action = None;
    ui.heading(format!(
        "{} selected object{}",
        nodes.selection_count(),
        if nodes.selection_count() == 1 {
            ""
        } else {
            "s"
        }
    ));
    if let Some(primary) = nodes.primary() {
        let name = state
            .capture(nav, nodes)
            .and_then(|capture| {
                capture
                    .members
                    .iter()
                    .find(|member| member.node_id == primary.node_id)
            })
            .and_then(|member| member.state.display_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&primary.node_id);
        ui.add(egui::Label::new(format!("Primary: {name}")).truncate())
            .on_hover_text(&primary.node_id);
    }
    ui.label(if include_properties {
        "Transform and property changes below apply to every selected object and save together."
    } else {
        "This transform applies to the whole selected object and saves as one edit."
    });
    if let Some(error) = &state.error {
        ui.colored_label(crate::theme::STATUS_OFFLINE, error);
    }
    if state.is_pending() {
        ui.label(if state.has_submitted_write() {
            "Saving or reconciling the complete edit."
        } else {
            "Reading stored targets; no write has been submitted."
        });
        if state.has_submitted_write() && ui.button("Check saved selection").observed(ui).clicked()
        {
            action = Some(UiAction::CheckSelectionSave);
        }
    }
    if ui
        .add_enabled(
            !state.is_pending(),
            egui::Button::new("Refresh selected objects"),
        )
        .observed(ui)
        .clicked()
    {
        state.refresh();
    }
    let captured = state.capture_shared(nav, nodes);
    let interaction = state.interaction_shared(nav, nodes);
    if let (Some(capture), Some(interaction)) = (captured, interaction) {
        ui.separator();
        ui.strong("Current values");
        let transforms = interaction.members.as_ref();
        match transforms {
            Ok(transforms) => {
                for (label, values) in [
                    (
                        "Position",
                        transforms
                            .iter()
                            .map(|value| value.transform.translation)
                            .collect::<Vec<_>>(),
                    ),
                    (
                        "Rotation (degrees)",
                        transforms
                            .iter()
                            .map(|value| {
                                let (x, y, z) = value.transform.rotation.to_euler(EulerRot::XYZ);
                                Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI)
                            })
                            .collect::<Vec<_>>(),
                    ),
                    (
                        "Scale",
                        transforms
                            .iter()
                            .map(|value| value.transform.scale)
                            .collect::<Vec<_>>(),
                    ),
                ] {
                    let components: Vec<_> = ["X", "Y", "Z"]
                        .into_iter()
                        .enumerate()
                        .map(|(axis, label)| {
                            let first = values[0][axis];
                            if values
                                .iter()
                                .all(|value| (value[axis] - first).abs() <= 0.0001)
                            {
                                format!("{label}: common {first:.3}")
                            } else {
                                format!("{label}: Mixed")
                            }
                        })
                        .collect();
                    ui.label(format!("{label}: {}", components.join("; ")));
                }
            }
            Err(error) => {
                ui.label(error);
            }
        }
        for (label, tool) in [
            ("Move", crate::panels::toolbar::Tool::Move),
            ("Rotate", crate::panels::toolbar::Tool::Rotate),
            ("Axis Scale", crate::panels::toolbar::Tool::Scale),
        ] {
            if let Err(reason) = interaction.capability(tool, false) {
                ui.label(format!("{label}: {reason}"));
            }
        }
        ui.label("Pivot: center of the selected objects. Translation is a world-unit delta; rotation is in degrees; scale values are factors.");
        if let Some(reason) = interaction.source_limitation {
            ui.label(reason);
        }
        let editable =
            role.can_edit() && !state.is_pending() && interaction.source_limitation.is_none();
        axes(ui, "Move by", &mut state.draft.translation, editable);
        axes(ui, "Rotate by", &mut state.draft.rotation, editable);
        axes(ui, "Scale by", &mut state.draft.scale, editable);
        if ui
            .add_enabled(
                editable,
                egui::Button::new("Apply selection transform").wrap(),
            )
            .observed(ui)
            .clicked()
        {
            let edit = (|| {
                let translation = parse(&state.draft.translation)?;
                let rotation = parse(&state.draft.rotation)? * (std::f32::consts::PI / 180.0);
                let scale = parse(&state.draft.scale)?;
                let affine = EditorAffine {
                    pivot: interaction
                        .center
                        .as_ref()
                        .map_err(Clone::clone)?
                        .to_array()
                        .map(f64::from),
                    translation: translation.to_array().map(f64::from),
                    rotation: Quat::from_euler(EulerRot::XYZ, rotation.x, rotation.y, rotation.z)
                        .to_array(),
                    scale: scale.to_array().map(f64::from),
                };
                Ok::<_, String>(UiAction::SelectionBatch {
                    origin: state.origin(nodes)?,
                    edit: affine_edits(&capture, &affine)?,
                })
            })();
            match edit {
                Ok(edit) => action = Some(edit),
                Err(error) => state.error = Some(error),
            }
        }
        if include_properties {
            ui.separator();
            ui.strong("Properties across the selection");
            let keys = capture
                .members
                .iter()
                .flat_map(|member| member.state.properties.keys().cloned())
                .collect::<BTreeSet<_>>();
            for key in keys.iter().take(100) {
                let value = match property_presence(&capture, key) {
                    PropertyPresence::Common(value) => format!("Common: {value}"),
                    PropertyPresence::Mixed => "Mixed values".into(),
                    PropertyPresence::Missing { present, total } => format!(
                        "Present on {present}/{total}; missing on {}",
                        total - present
                    ),
                };
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(state.draft.property_key == *key, key)
                        .observed(ui)
                        .clicked()
                    {
                        state.draft.property_key = key.clone();
                    }
                    ui.add(egui::Label::new(&value).truncate())
                        .on_hover_text(&value);
                });
            }
            if keys.len() > 100 {
                ui.label(format!(
                    "{} more keys; enter a key below to edit it",
                    keys.len() - 100
                ));
            }
            ui.add_enabled(
                editable,
                crate::ui_shell::layout_policy::text_input(ui, &mut state.draft.property_key)
                    .hint_text("Property key")
                    .char_limit(256),
            )
            .named(ui, "Selection property key");
            ui.add_enabled(
                editable,
                egui::TextEdit::multiline(&mut state.draft.property_value)
                    .hint_text("JSON value, including objects, arrays or null")
                    .desired_rows(4)
                    .char_limit(64 * 1024),
            )
            .named(ui, "Selection property JSON value");
            let value = serde_json::from_str::<Value>(&state.draft.property_value);
            ui.horizontal_wrapped(|ui| {
                for (label, set) in [
                    ("Set on all selected objects", true),
                    ("Delete from all selected objects", false),
                ] {
                    if ui
                        .add_enabled(
                            editable
                                && !state.draft.property_key.trim().is_empty()
                                && (!set || value.is_ok()),
                            egui::Button::new(label).wrap(),
                        )
                        .observed(ui)
                        .clicked()
                    {
                        let property = if set {
                            EditorPropertyEdit::Set {
                                key: state.draft.property_key.trim().into(),
                                value: value.as_ref().unwrap().clone(),
                            }
                        } else {
                            EditorPropertyEdit::Delete {
                                key: state.draft.property_key.trim().into(),
                            }
                        };
                        match state.origin(nodes) {
                            Ok(origin) => {
                                action = Some(UiAction::SelectionBatch {
                                    origin,
                                    edit: EditorBatchEdit::Targets(
                                        capture
                                            .members
                                            .iter()
                                            .map(|member| EditorTargetEdit {
                                                node_id: member.node_id.clone(),
                                                mutation: EditorTargetMutation::Properties(vec![
                                                    property.clone(),
                                                ]),
                                            })
                                            .collect(),
                                    ),
                                })
                            }
                            Err(error) => state.error = Some(error),
                        }
                    }
                }
            });
            ui.label("Missing and JSON null are distinct. Every selected object's schema is checked before the batch can save.");
        }
    } else if state.error.is_none() {
        ui.label("Loading the complete selected set…");
    }
    ui_mgr.editor_batch = state;
    if let Some(action) = action {
        ui_mgr.push_action(action);
    }
}

pub(crate) fn group_controls(
    ui: &mut egui::Ui,
    nodes: &NodeManager,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    visibility: &crate::visibility::VisibilityState,
    role: &LocalUserRole,
) {
    let target = crate::object_data::target_size(ui);
    ui.spacing_mut().interact_size = egui::Vec2::splat(target);
    pending_controls(ui, ui_mgr);
    if nodes.selection_count() == 0 {
        if let Some(error) = &ui_mgr.editor_batch.error {
            ui.colored_label(crate::theme::STATUS_OFFLINE, error);
        }
        return;
    }
    let mut state = std::mem::take(&mut ui_mgr.editor_batch);
    let mut action = None;
    ui.label(format!(
        "{} selected objects · group changes save together",
        nodes.selection_count()
    ));
    if let Some(error) = &state.error {
        ui.label(error);
    }
    if ui
        .add_enabled(
            !state.is_pending(),
            egui::Button::new("Refresh selected objects"),
        )
        .observed(ui)
        .clicked()
    {
        state.refresh();
    }
    let limitation = state
        .interaction(nav, nodes)
        .and_then(|interaction| interaction.source_limitation);
    if let Some(reason) = limitation {
        ui.label(reason);
    }
    let ready = role.can_edit()
        && !visibility.registry_write_pending()
        && !state.is_pending()
        && state.capture(nav, nodes).is_some()
        && limitation.is_none();
    ui.add_enabled(
        role.can_edit() && !state.is_pending(),
        crate::ui_shell::layout_policy::text_input(ui, &mut state.draft.group_name)
            .hint_text("Group from selection")
            .char_limit(80),
    )
    .named(ui, "Name group from selection");
    ui.small("Large stored values may require smaller groups to keep Undo and Redo available.");
    if ui
        .add_enabled(
            ready && !state.draft.group_name.trim().is_empty(),
            egui::Button::new("Create group from selection").wrap(),
        )
        .observed(ui)
        .clicked()
    {
        match state.origin(nodes) {
            Ok(origin) => {
                action = Some(UiAction::SelectionBatch {
                    origin,
                    edit: EditorBatchEdit::CreateGroup {
                        id: ulid::Ulid::new().to_string(),
                        name: state.draft.group_name.trim().into(),
                    },
                })
            }
            Err(error) => state.error = Some(error),
        }
    }
    let captured = state.capture_shared(nav, nodes);
    let groups = captured
        .as_ref()
        .map(|capture| capture.groups.clone())
        .unwrap_or_else(|| crate::visibility::groups::dtos_from_groups(visibility.groups()));
    for group in &groups {
        let present = captured.as_ref().map_or(0, |capture| {
            capture
                .members
                .iter()
                .filter(|member| {
                    member
                        .state
                        .properties
                        .get("view.groups")
                        .and_then(Value::as_array)
                        .is_some_and(|ids| {
                            ids.iter().any(|id| id.as_str() == Some(group.id.as_str()))
                        })
                })
                .count()
        });
        ui.label(format!(
            "{}: {present}/{} selected members",
            group.name,
            nodes.selection_count()
        ));
        ui.horizontal_wrapped(|ui| {
            for (label, member) in [("Add all selected", true), ("Remove all selected", false)] {
                if ui
                    .add_enabled(ready, egui::Button::new(label))
                    .named(ui, format_args!("{label} {}", group.name))
                    .clicked()
                {
                    match state.origin(nodes) {
                        Ok(origin) => {
                            action = Some(UiAction::SelectionBatch {
                                origin,
                                edit: EditorBatchEdit::Membership {
                                    group_id: group.id.clone(),
                                    member,
                                },
                            })
                        }
                        Err(error) => state.error = Some(error),
                    }
                }
            }
        });
    }
    ui.separator();
    ui_mgr.editor_batch = state;
    if let Some(action) = action {
        ui_mgr.push_action(action);
    }
}

pub(crate) fn pending_controls(ui: &mut egui::Ui, ui_mgr: &mut UiManager) {
    if ui_mgr.editor_batch.is_pending() {
        ui.label(if ui_mgr.editor_batch.has_submitted_write() { "An edit is pending. Checking reads the original request outcome; it never repeats the write." } else { "Reading stored Group or Undo targets; no write has been submitted." });
        if ui_mgr.editor_batch.has_submitted_write()
            && ui.button("Check saved selection").observed(ui).clicked()
        {
            ui_mgr.push_action(UiAction::CheckSelectionSave);
        }
    }
}
