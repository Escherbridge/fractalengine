//! Bound `ui.panel` and `egress.snippet` surfaces (bindings.md section 6, BND-10c). The binary feeds the resolution
//! cache and answers snippet calls; this module only reads that cache and queues requests (no host calls here).
//! Compiled-in Copy-for-BI targets are drawn by the card before these rows and no binding hides them (R33).
//! See `fe-ui/src/extensions/AGENTS.md` §bound-slots.

use std::collections::{HashMap, HashSet};

use bevy::prelude::Resource;
use bevy_egui::egui;
use fe_sdk::{UiContribution, UiExtensionRegistry, UiSlot};

use super::builtin::{SnippetInputs, SnippetRow};
use super::slots::{draw_command_slot, draw_stream_slot, ExtensionSlotState};
use crate::accessibility::ResponseExt;
use crate::gis::egress_strings::{self as es, ExportFormat};
use crate::theme;

/// A cached `egress.snippet` answer is reused for this long before the next call is requested.
pub const SNIPPET_TTL_SECS: f64 = 1.0;

/// Longest snippet text kept and shown (bindings.md section 6).
pub const MAX_SNIPPET_BYTES: usize = 16 * 1024;

/// Longest provider label kept; the row also carries the package id, so a label cannot pass for a built-in target.
pub const MAX_SNIPPET_LABEL_CHARS: usize = 48;

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

/// One call the binary should make (the cached answer is stale, missing or for other arguments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetRequest {
    pub target: SnippetTarget,
    pub args: SnippetCallArgs,
}

#[derive(Debug, Clone, Default)]
struct CachedSnippet {
    label: String,
    text: String,
    /// Arguments the stored answer was computed for; a row shows only while they are the card's current ones.
    answered: Option<SnippetCallArgs>,
    /// Arguments of the last queued request; an answer is attributed to them.
    requested: Option<SnippetCallArgs>,
    /// UI-clock seconds of the last queued request: one call per arguments per TTL, answered or not.
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
    /// Per package, the `ui` ids its `ui.panel` provides entry `uses` (install-time manifest data, kept across petals).
    panel_uses: HashMap<String, HashSet<String>>,
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

    /// Nothing resolves (no active petal, or the binary lost the host): the built-ins serve alone; `panel_uses` stays.
    pub fn clear(&mut self) {
        let panel_uses = std::mem::take(&mut self.panel_uses);
        *self = Self { panel_uses, ..Self::default() };
    }

    /// Record the `uses` ids of a package's `ui.panel` entry (at install or update); an empty list forgets them.
    pub fn set_panel_uses(&mut self, package_id: &str, uses: impl IntoIterator<Item = String>) {
        let uses: HashSet<String> = uses.into_iter().collect();
        if uses.is_empty() {
            self.panel_uses.remove(package_id);
        } else {
            self.panel_uses.insert(package_id.to_owned(), uses);
        }
    }

    /// Drop everything held for a package (disabled, removed or revoked).
    pub fn forget_package(&mut self, package_id: &str) {
        self.panels.retain(|panel| panel.package_id != package_id);
        self.targets.retain(|target| target.package_id != package_id);
        self.snippets.retain(|target, _| target.package_id != package_id);
        self.requests.retain(|request| request.target.package_id != package_id);
        self.panel_uses.remove(package_id);
    }

    /// Panels that resolve at `active_petal`; empty when the cache belongs to another petal.
    pub fn panels_for(&self, active_petal: Option<&str>) -> &[BoundPanel] {
        match (active_petal, self.petal_id.as_deref()) {
            (Some(active), Some(cached)) if active == cached => &self.panels,
            _ => &[],
        }
    }

    /// True when the contribution is `ui.panel` content: it renders only while its package's panel binding resolves.
    pub fn is_panel_content(&self, contribution: &UiContribution) -> bool {
        self.panel_uses.get(&contribution.plugin_id).is_some_and(|uses| uses.contains(&contribution.id))
    }

    /// True when a `ui.panel` binding of `package_id` resolves at `active_petal`.
    pub fn panel_bound(&self, package_id: &str, active_petal: Option<&str>) -> bool {
        self.panels_for(active_petal).iter().any(|panel| panel.package_id == package_id)
    }

    /// True when any `egress.snippet` binding resolves here (the card skips all snippet work otherwise).
    pub fn has_snippet_targets(&self) -> bool {
        !self.targets.is_empty()
    }

    /// Queue a call per target not asked about `args` within the TTL; a changed query is asked at once, not after it.
    pub fn request_stale(&mut self, now_secs: f64, args: &SnippetCallArgs, skip: &HashSet<String>) {
        for target in self.targets.iter().filter(|target| !skip.contains(&target.package_id)) {
            let entry = self.snippets.entry(target.clone()).or_default();
            let recent = entry.requested_at.is_some_and(|at| now_secs - at < SNIPPET_TTL_SECS);
            if recent && entry.requested.as_ref() == Some(args) {
                continue;
            }
            entry.requested = Some(args.clone());
            entry.requested_at = Some(now_secs);
            self.requests.retain(|request| request.target != *target);
            self.requests.push(SnippetRequest { target: target.clone(), args: args.clone() });
        }
    }

    /// Calls the binary should make now (at most one per target).
    pub fn take_requests(&mut self) -> Vec<SnippetRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Store a provider answer for `target`, attributed to its latest request; refused when unrequested, empty, oversized
    /// or no longer bound. The feed's clock is not used: freshness counts from the request on the UI clock.
    pub fn store_snippet(&mut self, target: &SnippetTarget, label: &str, text: &str, _feed_secs: f64) -> bool {
        let Some(entry) = self.snippets.get_mut(target) else {
            return false;
        };
        let Some(args) = entry.requested.clone() else {
            return false;
        };
        entry.answered = Some(args);
        if text.is_empty() || text.len() > MAX_SNIPPET_BYTES {
            entry.text.clear();
            return false;
        }
        entry.label = snippet_label(label, &target.key);
        entry.text = text.to_owned();
        true
    }

    /// `(state name, row)` per answer computed for `args`, in binding order; the card draws its built-in rows first.
    pub fn bound_rows(&self, args: &SnippetCallArgs, skip: &HashSet<String>) -> Vec<(String, SnippetRow)> {
        self.targets
            .iter()
            .filter(|target| !skip.contains(&target.package_id))
            .filter_map(|target| {
                let entry = self.snippets.get(target)?;
                (entry.answered.as_ref() == Some(args) && !entry.text.is_empty()).then(|| {
                    let row = SnippetRow {
                        label: format!("{} \u{00B7} {}", entry.label, target.package_id),
                        text: entry.text.clone(),
                        builtin: false,
                    };
                    (format!("binding:egress.snippet:{}", target.key), row)
                })
            })
            .collect()
    }
}

/// First line of the provider's label, control characters removed and capped; the binding key when it is empty.
fn snippet_label(label: &str, key: &str) -> String {
    let line = label.lines().next().unwrap_or_default().trim();
    let line = if line.is_empty() { key } else { line };
    line.chars().filter(|c| !c.is_control()).take(MAX_SNIPPET_LABEL_CHARS).collect()
}

/// A package's panel content in `slots` (commands and streams need no node context).
fn panel_contributions<'a>(
    registry: &'a UiExtensionRegistry,
    bound: &BoundSlotsState,
    package_id: &str,
    slots: &[UiSlot],
) -> Vec<&'a UiContribution> {
    slots
        .iter()
        .flat_map(|slot| registry.get_by_slot(*slot))
        .filter(|contribution| contribution.plugin_id == package_id && bound.is_panel_content(contribution))
        .collect()
}

/// Draw the bound panels of the active petal: each package's `uses` contributions under the panel name and package id.
/// A package bound under several names draws once, under the first; a stopped package draws nothing.
pub fn draw_bound_panels(
    ui: &mut egui::Ui,
    registry: &UiExtensionRegistry,
    bound: &BoundSlotsState,
    slot_state: &mut ExtensionSlotState,
    active_petal: Option<&str>,
    stopped: &HashSet<String>,
) {
    let mut drawn: HashSet<&str> = HashSet::new();
    for panel in bound.panels_for(active_petal) {
        if stopped.contains(&panel.package_id) || !drawn.insert(panel.package_id.as_str()) {
            continue;
        }
        let commands = panel_contributions(registry, bound, &panel.package_id, &[UiSlot::SidebarTool, UiSlot::ToolbarAction]);
        let streams = panel_contributions(registry, bound, &panel.package_id, &[UiSlot::DashboardWidget, UiSlot::ViewportOverlay]);
        if commands.is_empty() && streams.is_empty() {
            continue;
        }
        let section = egui::CollapsingHeader::new(&panel.panel)
            .id_salt(("bound-panel", &panel.package_id, &panel.panel))
            .default_open(true)
            .show(ui, |ui| {
                ui.label(egui::RichText::new(&panel.package_id).small().color(theme::TEXT_MUTED));
                draw_command_slot(ui, &commands, slot_state);
                draw_stream_slot(ui, &streams, slot_state);
            });
        section.header_response.named(ui, format!("binding:ui.panel:{}", panel.panel));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::builtin::snippet_rows;

    /// Journey: resolve, request, answer, cache hit, stale re-request, query change, oversized answer, panel content, navigate away.
    #[test]
    fn bound_snippet_journey_builtins_first_and_cached_for_one_second() {
        let target = SnippetTarget { package_id: "org.example.bi".into(), key: "looker".into() };
        let inputs = SnippetInputs { base: "http://127.0.0.1:8080", petal_id: "p1", sql: "SELECT 1" };
        let args = SnippetCallArgs::from_inputs(&inputs);
        let none = HashSet::new();
        let mut state = BoundSlotsState::default();
        state.set_resolution("p1", Vec::new(), vec![target.clone()]);
        assert!(!state.store_snippet(&target, "Looker", "unasked", 0.0), "an unrequested answer is refused");

        state.request_stale(0.0, &args, &none);
        assert_eq!(state.take_requests().len(), 1, "the first call is requested");
        state.request_stale(0.5, &args, &none);
        assert!(state.take_requests().is_empty(), "one call per TTL");

        // The feed's clock is ignored (it may be Bevy virtual time); freshness counts from the request on the UI clock.
        assert!(state.store_snippet(&target, "Looker\nsecond line", "explore: x", 9_000.0));
        state.request_stale(0.9, &args, &none);
        assert!(state.take_requests().is_empty(), "the answer is reused within the TTL");
        state.request_stale(1.0, &args, &none);
        assert_eq!(state.take_requests().len(), 1, "asked again after the TTL");

        // The row names its binding and package, so it cannot pass for a compiled-in target; those come first (R33).
        let rows = state.bound_rows(&args, &none);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "binding:egress.snippet:looker");
        assert_eq!(rows[0].1.label, "Looker \u{00B7} org.example.bi");
        assert!(!rows[0].1.builtin && rows[0].1.text == "explore: x");
        assert!(snippet_rows(&inputs, &[]).iter().all(|row| row.builtin));
        assert!(state.bound_rows(&args, &HashSet::from(["org.example.bi".to_owned()])).is_empty(), "a stopped package shows nothing");

        // A changed query hides the old answer at once and asks again without waiting for the TTL.
        let other = SnippetCallArgs::from_inputs(&SnippetInputs { sql: "SELECT 2", ..inputs });
        assert!(state.bound_rows(&other, &none).is_empty(), "an answer for another query is never shown");
        state.request_stale(1.1, &other, &none);
        assert_eq!(state.take_requests().len(), 1, "a changed query is asked at once");
        assert!(!state.store_snippet(&target, "Looker", &"x".repeat(MAX_SNIPPET_BYTES + 1), 1.2));
        assert!(state.bound_rows(&other, &none).is_empty(), "an oversized answer drops the package row only");

        // ui.panel: only the `uses` ids are panel content, and only a resolving binding at the active petal shows them.
        state.set_panel_uses("org.example.bi", ["speed".to_owned()]);
        let speed = UiContribution { slot: UiSlot::DashboardWidget, plugin_id: "org.example.bi".into(), id: "speed".into(), ..Default::default() };
        let start = UiContribution { slot: UiSlot::ToolbarAction, plugin_id: "org.example.bi".into(), id: "start".into(), ..Default::default() };
        assert!(state.is_panel_content(&speed) && !state.is_panel_content(&start));
        state.set_resolution("p1", vec![BoundPanel { panel: "fleet".into(), package_id: "org.example.bi".into() }], vec![target]);
        assert!(state.panel_bound("org.example.bi", Some("p1")) && !state.panel_bound("org.example.bi", Some("p2")));

        state.set_resolution("p2", Vec::new(), Vec::new());
        assert!(state.panels_for(Some("p1")).is_empty() && state.bound_rows(&other, &none).is_empty());
        state.clear();
        assert!(state.is_panel_content(&speed), "install-time panel content survives a cleared resolution");
        state.forget_package("org.example.bi");
        assert!(!state.is_panel_content(&speed));
    }
}
