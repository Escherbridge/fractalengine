//! Live transform constraint controls; see panels/AGENTS.md §transform-snapping.

use bevy_egui::egui;

use crate::accessibility::ResponseExt;
use crate::node_manager::{ToolSnapSettings, TransformAxisLock};
use crate::panels::toolbar::Tool;

pub(crate) fn render(ui: &mut egui::Ui, tool: Tool, settings: &mut ToolSnapSettings, world_scale: f64) {
    ui.label("Drag the highlighted gimbal axis. Constraints apply to object and whole-track transforms.");
    ui.horizontal_wrapped(|ui| {
        ui.label("Axis");
        for (axis, label) in [(TransformAxisLock::Any, "Any"), (TransformAxisLock::X, "X"), (TransformAxisLock::Y, "Y"), (TransformAxisLock::Z, "Z")] {
            ui.selectable_value(&mut settings.axis, axis, label).named(ui, format_args!("Transform axis lock {label}"));
        }
    });
    match tool {
        Tool::Move => {
            ui.checkbox(&mut settings.grid_enabled, "Grid snap").named(ui, "Enable transform grid snap");
            let unit = if world_scale.is_finite() && world_scale > 0.0 { "m" } else { "world units" };
            ui.add(egui::DragValue::new(&mut settings.grid_m).range(0.001..=1_000_000.0).speed(0.1).suffix(format!(" {unit}")))
                .named(ui, "Transform grid increment");
        }
        Tool::Rotate => {
            ui.checkbox(&mut settings.angle_enabled, "Angle snap").named(ui, "Enable transform angle snap");
            ui.add(egui::DragValue::new(&mut settings.angle_degrees).range(0.1..=180.0).speed(1.0).suffix("°"))
                .named(ui, "Transform angle increment");
        }
        Tool::Scale => {
            ui.checkbox(&mut settings.uniform_scale, "Uniform scale").named(ui, "Use uniform transform scaling");
            ui.checkbox(&mut settings.scale_enabled, "Scale snap").named(ui, "Enable transform scale snap");
            ui.add(egui::DragValue::new(&mut settings.scale_step).range(0.001..=1000.0).speed(0.01))
                .named(ui, "Transform scale increment");
        }
        _ => {}
    }
}
