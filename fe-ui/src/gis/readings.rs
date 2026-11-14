//! Readings tab state: polled petal-wide latest readings (see `fe-ui/src/AGENTS.md` §gis-query-ui).

use bevy_egui::egui;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_runtime::node_history::NodeAccessGrant;
use fe_runtime::petal_readings::{PetalReadingsSummary, WINDOW_MS};

use crate::accessibility::ResponseExt;
use crate::node_manager::NodeManager;
use crate::plugin::CameraFocusTarget;
use crate::theme;
use crate::verse_manager::VerseManager;

/// Seconds between polls while the tab is visible.
pub(crate) const POLL_INTERVAL_S: f64 = 2.0;
/// Seconds before an unanswered request is abandoned and re-polled.
const REQUEST_TIMEOUT_S: f64 = 10.0;
const CORRELATION_PREFIX: &str = "gis-readings-";

/// Live readings read model for the active petal.
#[derive(Default)]
pub(crate) struct ReadingsState {
    /// Correlation id of the single in-flight request.
    pub(crate) request: Option<String>,
    pub(crate) summary: Option<PetalReadingsSummary>,
    pub(crate) error: Option<String>,
    /// Time of the last poll; `None` until the first request is sent.
    pub(crate) last_poll_s: Option<f64>,
    pub(crate) paused: bool,
}

impl ReadingsState {
    /// `true` when the tab should issue another request now.
    pub(crate) fn due(&self, now_s: f64) -> bool {
        if self.paused {
            return false;
        }
        let Some(last_poll_s) = self.last_poll_s else {
            return true;
        };
        let wait = if self.request.is_some() {
            REQUEST_TIMEOUT_S
        } else {
            POLL_INTERVAL_S
        };
        now_s - last_poll_s >= wait
    }

    /// Sends one `ReadPetalReadings` (one in flight max) when due.
    pub(crate) fn poll(
        &mut self,
        now_s: f64,
        now_ms: i64,
        petal_id: Option<&str>,
        sender: &crossbeam::channel::Sender<DbCommand>,
    ) {
        let Some(petal_id) = petal_id else { return };
        if !self.due(now_s) {
            return;
        }
        let correlation_id = format!("{CORRELATION_PREFIX}{}", ulid::Ulid::new());
        self.last_poll_s = Some(now_s);
        match sender.try_send(DbCommand::ReadPetalReadings {
            petal_id: petal_id.to_owned(),
            since_ms: now_ms - WINDOW_MS,
            grant: NodeAccessGrant::Local,
            correlation_id: correlation_id.clone(),
        }) {
            Ok(()) => self.request = Some(correlation_id),
            Err(crossbeam::channel::TrySendError::Full(_)) => {
                self.request = None;
                self.error = Some("Database is busy; retrying.".into());
            }
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                bevy::log::warn!("DB channel closed while polling petal readings");
                self.request = None;
                self.error = Some("Database connection closed.".into());
            }
        }
    }

    /// Cheap prefilter: `true` for results this state could own.
    pub(crate) fn claims(&self, result: &DbResult) -> bool {
        self.request.is_some()
            && matches!(
                result,
                DbResult::PetalReadingsLoaded { .. } | DbResult::RequestFailed { .. }
            )
    }

    /// Accepts only the reply carrying the in-flight correlation id.
    pub(crate) fn route(&mut self, result: &DbResult) -> bool {
        let id = match result {
            DbResult::PetalReadingsLoaded { correlation_id, .. }
            | DbResult::RequestFailed { correlation_id, .. } => correlation_id,
            _ => return false,
        };
        if !id.starts_with(CORRELATION_PREFIX) || self.request.as_deref() != Some(id.as_str()) {
            return false;
        }
        self.request = None;
        match result {
            DbResult::PetalReadingsLoaded { summary, .. } => {
                let mut summary = summary.clone();
                summary
                    .rows
                    .sort_by(|a, b| b.recorded_at_ms.cmp(&a.recorded_at_ms));
                self.summary = Some(summary);
                self.error = None;
            }
            DbResult::RequestFailed { error, .. } => self.error = Some(error.clone()),
            _ => {}
        }
        true
    }

    /// Draws the readings grid; row click selects and frames the node.
    pub(crate) fn render(
        &mut self,
        ui: &mut egui::Ui,
        verse_mgr: &VerseManager,
        petal_id: &str,
        node_mgr: &mut NodeManager,
        camera_focus: &mut CameraFocusTarget,
    ) {
        ui.horizontal(|ui| {
            ui.strong("Readings");
            ui.checkbox(&mut self.paused, "Pause updates")
                .named(ui, "Pause updates");
        });
        if let Some(error) = &self.error {
            ui.colored_label(theme::STATUS_OFFLINE, error);
        }
        let Some(summary) = &self.summary else {
            return;
        };
        if summary.rows.is_empty() {
            ui.label(
                egui::RichText::new("No readings in the last minute.")
                    .small()
                    .color(theme::TEXT_MUTED)
                    .italics(),
            );
            return;
        }
        let petal = verse_mgr.find_petal(petal_id);
        let mut clicked: Option<(String, [f32; 3])> = None;
        egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
            egui::Grid::new("gis_readings_grid")
                .striped(true)
                .num_columns(6)
                .show(ui, |ui| {
                    for heading in ["Node", "Metric", "Value", "Units", "Age", "Per min"] {
                        ui.label(
                            egui::RichText::new(heading)
                                .small()
                                .color(theme::TEXT_SECTION),
                        );
                    }
                    ui.end_row();
                    for row in &summary.rows {
                        let node =
                            petal.and_then(|petal| petal.nodes.iter().find(|n| n.id == row.node_id));
                        let name = node.map_or(row.node_id.as_str(), |n| n.name.as_str());
                        let label = egui::RichText::new(name).color(theme::TEXT_BRIGHT);
                        if ui
                            .add(egui::Label::new(label).sense(egui::Sense::click()))
                            .button_named(ui, format_args!("Select and frame {} readings node", name))
                            .clicked()
                        {
                            if let Some(node) = node {
                                clicked = Some((node.id.clone(), node.position));
                            }
                        }
                        ui.label(&row.metric);
                        ui.label(format!("{:.4}", row.value));
                        ui.label(&row.units);
                        ui.label(age_label(summary.as_of_ms - row.recorded_at_ms));
                        ui.label(per_minute_label(row.count_window, WINDOW_MS));
                        ui.end_row();
                    }
                });
        });
        if summary.truncated {
            ui.label(
                egui::RichText::new(format!("Showing {} of more series", summary.rows.len()))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        }
        if let Some((node_id, position)) = clicked {
            node_mgr.request_select_and_frame(&node_id, position, camera_focus);
        }
    }
}

/// Current wall-clock time in unix milliseconds.
pub(crate) fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Compact age such as `3 s`, `2 min`, `1 h`.
pub(crate) fn age_label(age_ms: i64) -> String {
    let seconds = age_ms.max(0) / 1_000;
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3_599 => format!("{} min", seconds / 60),
        _ => format!("{} h", seconds / 3_600),
    }
}

/// Readings per minute over the count window.
pub(crate) fn per_minute_label(count: u32, window_ms: i64) -> String {
    if window_ms <= 0 {
        return "0".into();
    }
    let rate = f64::from(count) * 60_000.0 / window_ms as f64;
    if (rate - rate.round()).abs() < 0.05 {
        format!("{rate:.0}")
    } else {
        format!("{rate:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::petal_readings::LatestReading;

    fn reading(node_id: &str, recorded_at_ms: i64) -> LatestReading {
        LatestReading {
            node_id: node_id.into(),
            metric: "m".into(),
            units: "u".into(),
            value: 1.0,
            recorded_at_ms,
            count_window: 1,
        }
    }

    fn loaded(id: &str) -> DbResult {
        DbResult::PetalReadingsLoaded {
            summary: PetalReadingsSummary {
                petal_id: "p".into(),
                as_of_ms: 10_000,
                rows: vec![reading("a", 1_000), reading("b", 9_000)],
                truncated: false,
            },
            correlation_id: id.into(),
        }
    }

    #[test]
    fn route_accepts_only_own_correlation_id() {
        let failed = |id: &str| DbResult::RequestFailed {
            correlation_id: id.into(),
            error: "denied".into(),
        };
        // (reply, accepted, summary stored, error)
        let cases: Vec<(DbResult, bool, bool, Option<&str>)> = vec![
            (loaded("gis-readings-own"), true, true, None),
            (loaded("gis-readings-other"), false, false, None),
            (loaded("object-measurements-1"), false, false, None),
            (failed("gis-readings-own"), true, false, Some("denied")),
            (failed("gis-readings-other"), false, false, None),
        ];
        for (reply, accepted, stored, error) in cases {
            let mut state = ReadingsState {
                request: Some("gis-readings-own".into()),
                ..Default::default()
            };
            assert_eq!(state.route(&reply), accepted);
            assert_eq!(state.summary.is_some(), stored);
            assert_eq!(state.error.as_deref(), error);
            assert_eq!(state.request.is_none(), accepted);
            if let Some(summary) = &state.summary {
                assert_eq!(summary.rows[0].node_id, "b", "newest first");
            }
        }
    }

    #[test]
    fn age_and_rate_formatting() {
        for (age_ms, expected) in [(-5, "0 s"), (3_000, "3 s"), (120_000, "2 min"), (7_300_000, "2 h")] {
            assert_eq!(age_label(age_ms), expected);
        }
        for (count, window, expected) in [(3, 60_000, "3"), (1, 30_000, "2"), (1, 40_000, "1.5"), (5, 0, "0")] {
            assert_eq!(per_minute_label(count, window), expected);
        }
    }
}
