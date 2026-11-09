//! Bound `ui.panel` and `egress.snippet` surfaces (bindings.md section 6, BND-10c). The binary feeds the resolution
//! cache and answers snippet calls; this module only reads that cache and queues requests (no host calls here).
//! Compiled-in Copy-for-BI targets always render first and no binding hides them (R33).

use std::collections::{HashMap, HashSet};

use bevy::prelude::Resource;
use bevy_egui::egui;
use fe_sdk::{UiContribution, UiExtensionRegistry, UiSlot};

use super::builtin::{snippet_rows, BoundSnippet, SnippetInputs, SnippetRow};
use super::slots::{draw_command_slot, draw_stream_slot, ExtensionSlotState};
use crate::accessibility::ResponseExt;
use crate::gis::egress_strings::{self as es, ExportFormat};
use crate::theme;

/// A cached `egress.snippet` answer is reused for this long before the next call is requested.
pub const SNIPPET_TTL_SECS: f64 = 1.0;

/// Longest snippet text kept and shown (bindings.md section 6).
pub const MAX_SNIPPET_BYTES: usize = 16 * 1024;

/// A `ui.panel` binding that resolves at the active petal on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundPanel {
    /// Panel name (the binding key).
    pub panel: String,
    /// Package whose `uses` contributions render in the panel.
    pub package_id: String,
}

/// An `egress.snippet` binding that resolves at the active petal on this host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnippetTarget {
    pub package_id: String,
    /// Target name (the binding key).
    pub key: String,
}

/// The call argument `{sql, query_url, export_urls}` handed to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetCallArgs {
    pub sql: String,
    pub query_url: String,
    pub export_parquet: String,
    pub export_csv: String,
}

impl SnippetCallArgs {
    /// Same strings the compiled-in targets render, so a provider sees exactly what the card shows.
    pub fn from_inputs(inputs: &SnippetInputs) -> Self {
        Self {
            sql: inputs.sql.to_owned(),
            query_url: es::query_post_url(inputs.base),
            export_parquet: es::export_url(inputs.base, inputs.petal_id, inputs.sql, ExportFormat::Parquet),
            export_csv: es::export_url(inputs.base, inputs.petal_id, inputs.sql, ExportFormat::Csv),
        }
    }
}

/// One call the binary should make (cached provider answer is stale or missing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetRequest {
    pub target: SnippetTarget,
    pub args: SnippetCallArgs,
}

#[derive(Debug, Clone)]
struct CachedSnippet {
    label: String,
    text: String,
    /// Seconds (UI clock) of the last stored answer; `None` before the first.
    stored_at: Option<f64>,
    /// Seconds of the last queued request, so a slow call is not re-requested every frame.
    requested_at: Option<f64>,
}

/// Resolution cache for the active petal plus the snippet answer cache; fed and drained by the binary.
#[derive(Resource, Debug, Default)]
pub struct BoundSlotsState {
    petal_id: Option<String>,
    panels: Vec<BoundPanel>,
    /// Snippet targets in binding order (stable row order).
    targets: Vec<SnippetTarget>,
    snippets: HashMap<SnippetTarget, CachedSnippet>,
    requests: Vec<SnippetRequest>,
}

impl BoundSlotsState {
    /// Replace what resolves at `petal_id`; answers for targets that no longer resolve are dropped.
    pub fn set_resolution(&mut self, petal_id: &str, panels: Vec<BoundPanel>, targets: Vec<SnippetTarget>) {
        let keep: HashSet<&SnippetTarget> = targets.iter().collect();
        self.snippets.retain(|target, _| keep.contains(target));
        self.requests.retain(|request| keep.contains(&request.target));
        self.petal_id = Some(petal_id.to_owned());
        self.panels = panels;
        self.targets = targets;
    }

    /// Nothing resolves (no active petal, or the binary lost the host): the built-ins serve alone.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Drop everything held for a package (disabled, removed or revoked).
    pub fn forget_package(&mut self, package_id: &str) {
        self.panels.retain(|panel| panel.package_id != package_id);
        self.targets.retain(|target| target.package_id != package_id);
        self.snippets.retain(|target, _| target.package_id != package_id);
        self.requests.retain(|request| request.target.package_id != package_id);
    }

    /// Panels that resolve at `active_petal`; empty when the cache belongs to another petal.
    pub fn panels_for(&self, active_petal: Option<&str>) -> &[BoundPanel] {
        match (active_petal, self.petal_id.as_deref()) {
            (Some(active), Some(cached)) if active == cached => &self.panels,
            _ => &[],
        }
    }

    /// Queue a call for every target whose cached answer is older than the TTL (at most one in flight per TTL).
    pub fn request_stale(&mut self, now_secs: f64, args: &SnippetCallArgs) {
        for target in &self.targets {
            let entry = self.snippets.entry(target.clone()).or_insert(CachedSnippet {
                label: String::new(),
                text: String::new(),
                stored_at: None,
                requested_at: None,
            });
            let last = entry.stored_at.into_iter().chain(entry.requested_at).fold(f64::NEG_INFINITY, f64::max);
            if now_secs - last >= SNIPPET_TTL_SECS {
                entry.requested_at = Some(now_secs);
                self.requests.push(SnippetRequest { target: target.clone(), args: args.clone() });
            }
        }
    }

    /// Calls the binary should make now.
    pub fn take_requests(&mut self) -> Vec<SnippetRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Store a provider answer; an empty, oversized or unrequested answer is refused and any old text is dropped.
    pub fn store_snippet(&mut self, target: &SnippetTarget, label: &str, text: &str, now_secs: f64) -> bool {
        let Some(entry) = self.snippets.get_mut(target) else {
            return false;
        };
        entry.stored_at = Some(now_secs);
        if text.is_empty() || text.len() > MAX_SNIPPET_BYTES {
            entry.text.clear();
            return false;
        }
        entry.label = if label.is_empty() { target.key.clone() } else { label.to_owned() };
        entry.text = text.to_owned();
        true
    }

    /// Answers to show after the built-ins, in binding order.
    pub fn bound_snippets(&self) -> Vec<BoundSnippet> {
        self.targets
            .iter()
            .filter_map(|target| self.snippets.get(target))
            .filter(|entry| !entry.text.is_empty())
            .map(|entry| BoundSnippet { label: entry.label.clone(), text: entry.text.clone() })
            .collect()
    }

    /// Built-in rows first, then cached package rows: the card's whole list.
    pub fn rows(&self, inputs: &SnippetInputs) -> Vec<SnippetRow> {
        snippet_rows(inputs, &self.bound_snippets())
    }
}

/// Contributions of one package that a panel can draw without a node context (commands and streams).
fn panel_contributions<'a>(registry: &'a UiExtensionRegistry, package_id: &str, slots: &[UiSlot]) -> Vec<&'a UiContribution> {
    slots
        .iter()
        .flat_map(|slot| registry.get_by_slot(*slot))
        .filter(|contribution| contribution.plugin_id == package_id)
        .collect()
}

/// Draw the bound panels of the active petal; a package bound under several names draws once, under the first.
pub fn draw_bound_panels(
    ui: &mut egui::Ui,
    registry: &UiExtensionRegistry,
    bound: &BoundSlotsState,
    slot_state: &mut ExtensionSlotState,
    active_petal: Option<&str>,
) {
    let mut drawn: HashSet<&str> = HashSet::new();
    for panel in bound.panels_for(active_petal) {
        if !drawn.insert(panel.package_id.as_str()) {
            continue;
        }
        let commands = panel_contributions(
            registry,
            &panel.package_id,
            &[UiSlot::SidebarTool, UiSlot::ToolbarAction, UiSlot::DashboardWidget],
        );
        let streams = panel_contributions(registry, &panel.package_id, &[UiSlot::DashboardWidget, UiSlot::ViewportOverlay]);
        if commands.is_empty() && streams.is_empty() {
            continue;
        }
        let section = egui::CollapsingHeader::new(&panel.panel)
            .id_salt(("bound-panel", &panel.package_id, &panel.panel))
            .default_open(true)
            .show(ui, |ui| {
                draw_command_slot(ui, &commands, slot_state);
                draw_stream_slot(ui, &streams, slot_state);
            });
        section.header_response.named(ui, format!("binding:ui.panel:{}", panel.panel));
    }
}

/// Read-only copy field per row, built-ins first; the text is shown for copying and never run.
pub fn draw_snippet_rows(ui: &mut egui::Ui, rows: &[SnippetRow]) {
    for row in rows {
        ui.label(egui::RichText::new(&row.label).small().strong().color(theme::TEXT_SECTION));
        egui::ScrollArea::vertical()
            .id_salt(("bound-snippet", &row.label))
            .max_height(120.0)
            .show(ui, |ui| {
                let mut shown: &str = row.text.as_str();
                ui.add(egui::TextEdit::multiline(&mut shown).desired_rows(1).desired_width(f32::INFINITY))
                    .named(ui, format!("Copy for BI: {}", row.label));
            });
        if ui.button("Copy").named(ui, format!("Copy {}", row.label)).clicked() {
            ui.ctx().copy_text(row.text.clone());
        }
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Journey: resolve, request, answer, cache hit, stale re-request, navigate away, built-ins stay first.
    #[test]
    fn bound_snippet_journey_builtins_first_and_cached_for_one_second() {
        let target = SnippetTarget { package_id: "org.example.bi".into(), key: "looker".into() };
        let inputs = SnippetInputs { base: "http://127.0.0.1:8080", petal_id: "p1", sql: "SELECT 1" };
        let args = SnippetCallArgs::from_inputs(&inputs);
        let mut state = BoundSlotsState::default();
        state.set_resolution("p1", Vec::new(), vec![target.clone()]);

        state.request_stale(0.0, &args);
        assert_eq!(state.take_requests().len(), 1);
        state.request_stale(0.5, &args);
        assert!(state.take_requests().is_empty(), "one call in flight per TTL");

        assert!(state.store_snippet(&target, "Looker", "explore: x", 0.6));
        state.request_stale(1.2, &args);
        assert!(state.take_requests().is_empty(), "fresh answer is reused");
        state.request_stale(1.7, &args);
        assert_eq!(state.take_requests().len(), 1, "stale after the TTL");

        let rows = state.rows(&inputs);
        assert!(rows.first().is_some_and(|row| row.builtin));
        assert!(rows.last().is_some_and(|row| !row.builtin && row.text == "explore: x"));

        assert!(!state.store_snippet(&target, "Looker", &"x".repeat(MAX_SNIPPET_BYTES + 1), 2.0));
        assert!(state.rows(&inputs).iter().all(|row| row.builtin), "oversized answer drops the package row only");

        state.set_resolution("p2", Vec::new(), Vec::new());
        assert!(state.panels_for(Some("p1")).is_empty());
        assert!(state.rows(&inputs).iter().all(|row| row.builtin));
    }
}
