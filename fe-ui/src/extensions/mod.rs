//! Extension UI: the slot host, the Extensions panel, consent and the built-in connectors
//! (see `fe-ui/src/extensions/AGENTS.md` and `docs/extensions/extensions-ui.md`).

pub mod bindings;
pub mod bound_slots;
pub mod browse;
pub mod builtin;
pub mod consent;
pub mod details;
pub mod install_offer;
pub mod panel;
pub mod permissions;
pub mod slots;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use fe_sdk::{UiContribution, UiExtensionRegistry};

use crate::accessibility::ResponseExt;
use crate::portal::binding_offer::{OfferResolution, PortalBindingPoint};
use crate::theme;

pub use bound_slots::{BoundPanel, BoundSlotsState, SnippetCallArgs, SnippetRequest, SnippetTarget};
pub use builtin::{SnippetInputs, SnippetRow};
pub use consent::ConsentState;
pub use panel::{ExtensionOp, ExtensionsPanelPlugin, ExtensionsPanelState};
pub use slots::{
    ExtensionCommandRequest, ExtensionSlotState, SlotContext, details_only, draw_command_slot,
    draw_context_menu_items, draw_inspector_rows, draw_stream_slot,
};

/// Everything the slot surfaces read or write for one pass; lent from the resources (see `extensions/AGENTS.md` section host-handle).
#[derive(Default)]
struct HostInner {
    lent: bool,
    panel_open: bool,
    active_petal: Option<String>,
    registry: UiExtensionRegistry,
    slots: ExtensionSlotState,
    bound: BoundSlotsState,
    /// Packages the installed list reports not running here, or removed since; none of their contributions draw.
    stopped: HashSet<String>,
    /// `ExtensionsPanelState::rows_generation` that `stopped` was copied from.
    rows_generation: u64,
}

impl HostInner {
    /// Toolbar, sidebar, overlay and dashboard: the package runs here and the contribution is not `ui.panel` content.
    fn in_plain_slot(&self, contribution: &UiContribution) -> bool {
        !self.stopped.contains(&contribution.plugin_id) && !self.bound.is_panel_content(contribution)
    }

    /// Inspector and context menu need a node, so `ui.panel` content draws in place, only while its binding resolves here.
    fn in_node_slot(&self, contribution: &UiContribution) -> bool {
        !self.stopped.contains(&contribution.plugin_id)
            && (!self.bound.is_panel_content(contribution)
                || self.bound.panel_bound(&contribution.plugin_id, self.active_petal.as_deref()))
    }
}

/// Shared handle to the lent slot state; surfaces reach it through the egui context, so no draw signature grows.
#[derive(Resource, Clone, Default)]
pub struct ExtensionSlotHost(Arc<Mutex<HostInner>>);

fn host_id() -> egui::Id {
    egui::Id::new("fractalengine_extension_slot_host")
}

impl ExtensionSlotHost {
    fn lock(&self) -> MutexGuard<'_, HostInner> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn publish(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(host_id(), self.clone()));
    }

    fn from_ctx(ctx: &egui::Context) -> Option<Self> {
        ctx.data(|data| data.get_temp::<Self>(host_id()))
    }

    /// Open or close the Extensions window.
    pub fn set_panel_open(&self, open: bool) {
        self.lock().panel_open = open;
    }
}

/// Initialises the slot host state; the registry itself is `plugin::UiExtensions`.
pub struct ExtensionSlotsPlugin;

impl Plugin for ExtensionSlotsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExtensionSlotState>()
            .init_resource::<BoundSlotsState>()
            .init_resource::<ExtensionSlotHost>()
            .add_systems(Update, feed_portal_offer_point);
    }
}

/// Pure: the winning `portal.open` row of the bindings snapshot as the inspector offer's input.
pub fn portal_point_from_snapshot(snapshot: &bindings::BindingsSnapshot) -> Option<PortalBindingPoint> {
    use fe_runtime::extension_binding::BindingResolution;
    let row = snapshot.points.iter().find(|row| row.hook == "portal.open")?;
    let resolution = match row.resolution.as_ref()? {
        BindingResolution::Resolved { .. } => OfferResolution::Resolved,
        BindingResolution::Builtin { .. } => OfferResolution::Builtin,
        BindingResolution::Elsewhere { .. } => OfferResolution::Elsewhere,
        BindingResolution::Unresolved { reason, .. } => OfferResolution::Unresolved {
            reason: serde_json::to_value(reason)
                .ok()
                .and_then(|value| value.as_str().map(|text| text.replace('-', " ")))
                .unwrap_or_else(|| "not runnable here".to_owned()),
        },
    };
    Some(PortalBindingPoint {
        scope: row.scope_label.clone(),
        package_id: row.package_id.clone(),
        resolution,
    })
}

/// Copies the active petal's `portal.open` point from the bindings snapshot to the UI manager (the inspector offer reads it).
fn feed_portal_offer_point(
    panel: Option<Res<ExtensionsPanelState>>,
    ui_manager: Option<ResMut<crate::actions::UiManager>>,
) {
    let (Some(panel), Some(mut ui_manager)) = (panel, ui_manager) else { return };
    if !panel.is_changed() {
        return;
    }
    let point = panel.bindings.snapshot.as_ref().and_then(portal_point_from_snapshot);
    if ui_manager.portal_binding_point != point {
        ui_manager.portal_binding_point = point;
    }
}

/// Lends the registry, slot state and bound cache to this pass's surfaces; pairs with [`reclaim_slots_from_surfaces`].
pub(crate) fn lend_slots_to_surfaces(
    mut egui_ctx: EguiContexts,
    host: Res<ExtensionSlotHost>,
    mut registry: ResMut<crate::plugin::UiExtensions>,
    mut slots: ResMut<ExtensionSlotState>,
    mut bound: ResMut<BoundSlotsState>,
    mut panel: Option<ResMut<ExtensionsPanelState>>,
    nav: Res<crate::navigation_manager::NavigationManager>,
) {
    let mut inner = host.lock();
    if !inner.lent {
        std::mem::swap(&mut registry.bypass_change_detection().0, &mut inner.registry);
        std::mem::swap(slots.bypass_change_detection(), &mut inner.slots);
        std::mem::swap(bound.bypass_change_detection(), &mut inner.bound);
        inner.lent = true;
    }
    if inner.active_petal != nav.active_petal_id {
        inner.active_petal = nav.active_petal_id.clone();
        // Installed rows and bindings describe the petal they were read in; the next draw asks again.
        if let Some(panel) = panel.as_deref_mut() {
            panel.on_scope_changed();
        }
    }
    if let Some(panel) = panel.as_deref() {
        if panel.rows_generation() != inner.rows_generation {
            inner.rows_generation = panel.rows_generation();
            inner.stopped = panel.stopped_packages().clone();
        }
    }
    drop(inner);
    if let Ok(ctx) = egui_ctx.ctx_mut() {
        host.publish(ctx);
    }
}

/// Returns the lent state to its resources so the binary's Update systems see pressed commands next frame.
pub(crate) fn reclaim_slots_from_surfaces(
    host: Res<ExtensionSlotHost>,
    mut registry: ResMut<crate::plugin::UiExtensions>,
    mut slots: ResMut<ExtensionSlotState>,
    mut bound: ResMut<BoundSlotsState>,
) {
    let mut inner = host.lock();
    if inner.lent {
        std::mem::swap(&mut registry.bypass_change_detection().0, &mut inner.registry);
        std::mem::swap(slots.bypass_change_detection(), &mut inner.slots);
        std::mem::swap(bound.bypass_change_detection(), &mut inner.bound);
        inner.lent = false;
    }
}

/// Topbar (or the compact Tools menu): toolbar actions of running packages, `ui.panel` content excluded.
pub fn draw_topbar_controls(ui: &mut egui::Ui) {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return };
    let mut guard = host.lock();
    let inner = &mut *guard;
    let items: Vec<&UiContribution> =
        inner.registry.toolbar_actions.iter().filter(|c| inner.in_plain_slot(c)).collect();
    draw_command_slot(ui, &items, &mut inner.slots);
}

/// Topbar toggle for the Extensions window.
pub fn topbar_toggle(ui: &mut egui::Ui) {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return };
    let open = host.lock().panel_open;
    let fill = if open { theme::BG_BUTTON_ACTIVE } else { theme::BG_BUTTON };
    if ui
        .add(egui::Button::new("Extensions").fill(fill))
        .named(ui, "Toggle Extensions panel")
        .on_hover_text("Installed extensions, browse, bindings")
        .clicked()
    {
        host.set_panel_open(!open);
    }
}

/// Left sidebar: sidebar tools of running packages, then the bound panels of the active petal.
pub fn draw_sidebar_slot(ui: &mut egui::Ui) {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return };
    let mut guard = host.lock();
    let inner = &mut *guard;
    let tools: Vec<&UiContribution> = inner
        .registry
        .sidebar_tools
        .iter()
        .filter(|c| c.command.is_some() && inner.in_plain_slot(c))
        .collect();
    let has_panels = !inner.bound.panels_for(inner.active_petal.as_deref()).is_empty();
    if tools.is_empty() && !has_panels {
        return;
    }
    ui.add_space(8.0);
    if !tools.is_empty() {
        egui::CollapsingHeader::new(egui::RichText::new("Extensions").strong().color(theme::TEXT_SECTION))
            .default_open(false)
            .show(ui, |ui| draw_command_slot(ui, &tools, &mut inner.slots));
    }
    bound_slots::draw_bound_panels(
        ui,
        &inner.registry,
        &inner.bound,
        &mut inner.slots,
        inner.active_petal.as_deref(),
        &inner.stopped,
    );
}

/// Inspector: read-only rows for the selected node's declared property keys.
pub fn draw_inspector_slot(ui: &mut egui::Ui, node_properties: &serde_json::Value) {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return };
    let guard = host.lock();
    let items: Vec<&UiContribution> =
        guard.registry.inspector_panels.iter().filter(|c| guard.in_node_slot(c)).collect();
    if items.is_empty() {
        return;
    }
    let node = |key: &str| node_properties.get(key).cloned();
    draw_inspector_rows(ui, &items, &node);
}

/// Context menu: entries whose `when` admits the clicked node (`id` and `name` are the readable keys); true when one was pressed.
pub fn draw_context_slot(ui: &mut egui::Ui, node_id: &str, node_name: &str) -> bool {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return false };
    let mut guard = host.lock();
    let inner = &mut *guard;
    let node = |key: &str| match key {
        "id" => Some(serde_json::Value::String(node_id.to_owned())),
        "name" => Some(serde_json::Value::String(node_name.to_owned())),
        _ => None,
    };
    let items: Vec<&UiContribution> = inner
        .registry
        .context_items
        .iter()
        .filter(|c| c.command.is_some() && inner.in_node_slot(c) && slots::when_matches(c.when.as_ref(), &node))
        .collect();
    if items.is_empty() {
        return false;
    }
    ui.separator();
    let before = inner.slots.pending_commands.len();
    draw_context_menu_items(ui, &items, &node, &mut inner.slots);
    inner.slots.pending_commands.len() > before
}

/// Bound `egress.snippet` rows as `(state name, row)`, drawn after the card's built-in rows; queues calls for stale answers.
pub fn bound_snippet_rows(ctx: &egui::Context, inputs: &SnippetInputs) -> Vec<(String, SnippetRow)> {
    let Some(host) = ExtensionSlotHost::from_ctx(ctx) else { return Vec::new() };
    let now = ctx.input(|input| input.time);
    let mut guard = host.lock();
    let inner = &mut *guard;
    if !inner.bound.has_snippet_targets() {
        return Vec::new();
    }
    let args = SnippetCallArgs::from_inputs(inputs);
    inner.bound.request_stale(now, &args, &inner.stopped);
    inner.bound.bound_rows(&args, &inner.stopped)
}

/// Viewport overlay and dashboard widgets of running packages: a read-only stack in the viewport's top-left corner.
pub(crate) fn extension_overlay_system(mut egui_ctx: EguiContexts, host: Res<ExtensionSlotHost>) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let guard = host.lock();
    let items: Vec<&UiContribution> = guard
        .registry
        .overlays
        .iter()
        .chain(guard.registry.dashboard_widgets.iter())
        .filter(|c| c.stream.is_some() && guard.in_plain_slot(c))
        .collect();
    if items.is_empty() {
        return;
    }
    let corner = ctx.available_rect().left_top() + egui::vec2(8.0, 8.0);
    egui::Area::new(egui::Id::new("extension_overlay"))
        .fixed_pos(corner)
        .order(egui::Order::Middle)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::BG_PANEL.gamma_multiply(0.85))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| draw_stream_slot(ui, &items, &guard.slots));
        });
}

/// The Extensions window (panel body) and the consent modal; the modal draws every pass while a request is pending.
pub(crate) fn extensions_window_system(
    mut egui_ctx: EguiContexts,
    host: Res<ExtensionSlotHost>,
    mut panel_state: ResMut<ExtensionsPanelState>,
    mut consent_state: ResMut<ConsentState>,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    consent::render(ctx, &mut consent_state);
    let mut open = host.lock().panel_open;
    if !open {
        return;
    }
    crate::ui_shell::layout_policy::bounded_window(ctx, "Extensions")
        .id(egui::Id::new("extensions_window"))
        .open(&mut open)
        .default_width(480.0)
        .show(ctx, |ui| panel::draw(ui, &mut panel_state));
    host.lock().panel_open = open;
}
