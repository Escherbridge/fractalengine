//! Event-time chart and accessible table over persisted selected-object measurements.

use bevy_egui::egui;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_runtime::node_history::NodeAccessGrant;
use fe_runtime::node_measurements::{
    MeasurementPoint, MeasurementSeries, NodeMeasurements, POINT_LIMIT, SERIES_LIMIT,
};

use super::{button, error, request_error, send, Request, Target};
use crate::accessibility::ResponseExt;

#[derive(Default)]
pub(super) struct MeasurementsState {
    request: Option<(Request, Option<MeasurementSeries>)>,
    current: Option<NodeMeasurements>,
    selected: Option<MeasurementSeries>,
    error: Option<String>,
}

impl MeasurementsState {
    fn load(&mut self, target: &Target, sender: &crossbeam::channel::Sender<DbCommand>) {
        let request = Request::new("object-measurements");
        match send(
            sender,
            DbCommand::ReadNodeMeasurements {
                node_id: target.node_id.clone(),
                petal_id: target.petal_id.clone(),
                series: self.selected.clone(),
                limit: POINT_LIMIT,
                grant: NodeAccessGrant::Local,
                correlation_id: request.id.clone(),
            },
        ) {
            Ok(()) => {
                self.request = Some((request, self.selected.clone()));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(super) fn route(&mut self, target: &Target, result: &DbResult) -> bool {
        match result {
            DbResult::NodeMeasurementsLoaded {
                measurements,
                correlation_id,
            } if self
                .request
                .as_ref()
                .is_some_and(|(request, _)| request.matches(correlation_id)) =>
            {
                let (_, requested) = self.request.take().expect("matched request");
                if measurements.node_id != target.node_id
                    || measurements.petal_id != target.petal_id
                    || requested.is_some() && measurements.series != requested
                {
                    self.error = Some(
                        "Measurement response did not match the selected object and series.".into(),
                    );
                } else if measurements
                    .points
                    .iter()
                    .any(|point| !point.value.is_finite())
                    || measurements.points.windows(2).any(|pair| {
                        (pair[0].recorded_at_ms, &pair[0].reading_id)
                            > (pair[1].recorded_at_ms, &pair[1].reading_id)
                    })
                {
                    self.error =
                        Some("Measurements contain invalid values or event-time ordering.".into());
                } else {
                    self.selected = measurements.series.clone();
                    self.current = Some(measurements.clone());
                }
                true
            }
            _ => {
                let Some((id, message)) = request_error(result) else {
                    return false;
                };
                if !self
                    .request
                    .as_ref()
                    .is_some_and(|(request, _)| request.matches(id))
                {
                    return false;
                }
                self.request = None;
                self.error = Some(message.to_owned());
                true
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
            .request
            .as_ref()
            .is_some_and(|(request, _)| request.expired())
        {
            self.request = None;
            self.error = Some("Measurement request timed out. Refresh to try again.".into());
        }
        if self.current.is_none() && self.request.is_none() && self.error.is_none() {
            self.load(target, sender);
        }
        ui.label("Measurements are appended values, ordered by their event timestamp. Object replay uses commit versions.");
        if button(
            ui,
            "Refresh measurements",
            "Refresh object measurements",
            self.request.is_none(),
        ) {
            self.load(target, sender);
        }
        if self.request.is_some() {
            ui.label("Reading persisted measurements...");
        }
        error(ui, &self.error);
        let Some(current) = &self.current else {
            return;
        };
        let before = self.selected.clone();
        egui::ComboBox::from_id_salt("object-measurement-series")
            .width(ui.available_width().max(24.0))
            .selected_text(
                self.selected
                    .as_ref()
                    .map(series_label)
                    .unwrap_or_else(|| "No series".into()),
            )
            .show_ui(ui, |ui| {
                for series in &current.available_series {
                    ui.selectable_value(
                        &mut self.selected,
                        Some(series.clone()),
                        series_label(series),
                    )
                    .named(
                        ui,
                        format_args!("Measurement series {} {}", series.metric, series.units),
                    );
                }
            })
            .response
            .named(ui, "Object measurement series");
        if before != self.selected {
            self.load(target, sender);
            return;
        }
        if current.series != self.selected {
            ui.label(if self.request.is_some() {
                "Loading the selected series..."
            } else {
                "The selected series is unavailable. Refresh to try again."
            });
            return;
        }
        if current.series_truncated {
            ui.label(format!("Showing the first {SERIES_LIMIT} measurement series. Additional series are omitted."));
        }
        if current.points.is_empty() {
            ui.label("No appended measurements for this series.");
            return;
        }
        if current.truncated {
            ui.label(format!(
                "Partial series: first {} points in event-time order. Later points are omitted.",
                current.limit
            ));
        }
        let min = current
            .points
            .iter()
            .map(|point| point.value)
            .fold(f64::INFINITY, f64::min);
        let max = current
            .points
            .iter()
            .map(|point| point.value)
            .fold(f64::NEG_INFINITY, f64::max);
        ui.label(format!(
            "{} shown · minimum {min:.6e} · maximum {max:.6e}",
            current.points.len()
        ));
        chart(ui, &current.points, min, max);
        if let (Some(first), Some(last)) = (current.points.first(), current.points.last()) {
            ui.small(format!(
                "Event time: {} → {}",
                first.recorded_at, last.recorded_at
            ));
        }
        if button(
            ui,
            "Copy measurements TSV",
            "Copy displayed object measurements",
            true,
        ) {
            ui.ctx().copy_text(tsv(current));
            notices.show_toast(
                "Copied displayed measurements",
                ui.input(|input| input.time),
            );
        }
        ui.label("Event timestamp (UTC) | Value");
        egui::ScrollArea::both()
            .id_salt("object-measurement-table")
            .max_height(220.0)
            .show_rows(ui, 24.0, current.points.len(), |ui, rows| {
                for index in rows {
                    let point = &current.points[index];
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 24.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.monospace(&point.recorded_at);
                            ui.monospace(point.value.to_string());
                        },
                    );
                }
            });
    }
}

fn series_label(series: &MeasurementSeries) -> String {
    if series.units.is_empty() {
        series.metric.clone()
    } else {
        format!("{} ({})", series.metric, series.units)
    }
}

fn chart(ui: &mut egui::Ui, points: &[MeasurementPoint], min: f64, max: f64) {
    let width = ui.available_width().max(24.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 170.0), egui::Sense::hover());
    response.named(
        ui,
        format_args!(
            "Measurement chart: {} event-time points, minimum {min:.6e}, maximum {max:.6e}",
            points.len()
        ),
    );
    let plot = rect.shrink(8.0);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, crate::theme::BG_BUTTON);
    painter.line_segment(
        [plot.left_bottom(), plot.right_bottom()],
        egui::Stroke::new(1.0, crate::theme::TEXT_DIM),
    );
    painter.line_segment(
        [plot.left_bottom(), plot.left_top()],
        egui::Stroke::new(1.0, crate::theme::TEXT_DIM),
    );
    let first = points.first().map_or(0, |point| point.recorded_at_ms);
    let last = points.last().map_or(first, |point| point.recorded_at_ms);
    let time_range = (i128::from(last) - i128::from(first)) as f64;
    let scale = min.abs().max(max.abs()).max(f64::MIN_POSITIVE);
    let low = min / scale;
    let range = max / scale - low;
    let mut previous = None;
    for point in points {
        let x = if time_range > 0.0 {
            ((i128::from(point.recorded_at_ms) - i128::from(first)) as f64 / time_range) as f32
        } else {
            0.5
        };
        let y = if range > 0.0 {
            ((point.value / scale - low) / range) as f32
        } else {
            0.5
        };
        let position = egui::pos2(
            plot.left() + x * plot.width(),
            plot.bottom() - y * plot.height(),
        );
        if let Some(previous) = previous {
            painter.line_segment(
                [previous, position],
                egui::Stroke::new(1.5, crate::theme::TREE_NODE_ICON),
            );
        }
        if points.len() <= 64 {
            painter.circle_filled(position, 2.5, crate::theme::TEXT_BRIGHT);
        }
        previous = Some(position);
    }
}

fn tsv(measurements: &NodeMeasurements) -> String {
    let mut text = String::from("event_time_utc\tevent_time_ms\tvalue\tmetric\tunits\n");
    let clean = |value: &str| value.replace(['\t', '\r', '\n'], " ");
    if let Some(series) = &measurements.series {
        for point in &measurements.points {
            use std::fmt::Write;
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}\t{}",
                clean(&point.recorded_at),
                point.recorded_at_ms,
                point.value,
                clean(&series.metric),
                clean(&series.units)
            );
        }
    }
    text
}
