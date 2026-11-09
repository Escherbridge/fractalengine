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
            .init_resource::<ExtensionSlotHost>();
    }
}

/// Packages whose bound panel draws at the active petal; their contributions render in that panel only.
fn bound_packages(bound: &BoundSlotsState, active_petal: Option<&str>) -> HashSet<String> {
    bound.panels_for(active_petal).iter().map(|panel| panel.package_id.clone()).collect()
}

/// Lends the registry, slot state and bound cache to this pass's surfaces; pairs with [`reclaim_slots_from_surfaces`].
pub(crate) fn lend_slots_to_surfaces(
    mut egui_ctx: EguiContexts,
    host: Res<ExtensionSlotHost>,
    mut registry: ResMut<crate::plugin::UiExtensions>,
    mut slots: ResMut<ExtensionSlotState>,
    mut bound: ResMut<BoundSlotsState>,
    nav: Res<crate::navigation_manager::NavigationManager>,
) {
    let mut inner = host.lock();
    if !inner.lent {
        std::mem::swap(&mut registry.bypass_change_detection().0, &mut inner.registry);
        std::mem::swap(slots.bypass_change_detection(), &mut inner.slots);
        std::mem::swap(bound.bypass_change_detection(), &mut inner.bound);
        inner.lent = true;
    }
    inner.active_petal = nav.active_petal_id.clone();
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

/// Topbar: toolbar actions of unbound packages.
pub fn draw_topbar_controls(ui: &mut egui::Ui) {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return };
    let mut guard = host.lock();
    let HostInner { registry, slots, bound, active_petal, .. } = &mut *guard;
    let skip = bound_packages(bound, active_petal.as_deref());
    let items: Vec<&UiContribution> =
        registry.toolbar_actions.iter().filter(|c| !skip.contains(&c.plugin_id)).collect();
    draw_command_slot(ui, &items, slots);
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

/// Left sidebar: sidebar tools of unbound packages, then the bound panels of the active petal.
pub fn draw_sidebar_slot(ui: &mut egui::Ui) {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return };
    let mut guard = host.lock();
    let HostInner { registry, slots, bound, active_petal, .. } = &mut *guard;
    let skip = bound_packages(bound, active_petal.as_deref());
    let tools: Vec<&UiContribution> = registry
        .sidebar_tools
        .iter()
        .filter(|c| c.command.is_some() && !skip.contains(&c.plugin_id))
        .collect();
    if tools.is_empty() && skip.is_empty() {
        return;
    }
    ui.add_space(8.0);
    if !tools.is_empty() {
        egui::CollapsingHeader::new(egui::RichText::new("Extensions").strong().color(theme::TEXT_SECTION))
            .default_open(false)
            .show(ui, |ui| draw_command_slot(ui, &tools, slots));
    }
    bound_slots::draw_bound_panels(ui, registry, bound, slots, active_petal.as_deref());
}

/// Inspector: read-only rows for the selected node's declared property keys.
pub fn draw_inspector_slot(ui: &mut egui::Ui, node_properties: &serde_json::Value) {
    let Some(host) = ExtensionSlotHost::from_ctx(ui.ctx()) else { return };
    let guard = host.lock();
    let items: Vec<&UiContribution> = guard.registry.inspector_panels.iter().collect();
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
    let HostInner { registry, slots, .. } = &mut *guard;
    let items: Vec<&UiContribution> = registry.context_items.iter().collect();
    if items.iter().all(|c| c.command.is_none()) {
        return false;
    }
    let node = |key: &str| match key {
        "id" => Some(serde_json::Value::String(node_id.to_owned())),
        "name" => Some(serde_json::Value::String(node_name.to_owned())),
        _ => None,
    };
    ui.separator();
    let before = slots.pending_commands.len();
    draw_context_menu_items(ui, &items, &node, slots);
    slots.pending_commands.len() > before
}

/// Copy-for-BI rows: built-ins first, then bound package answers; requests stale answers for the binary to fetch.
pub fn bound_snippet_rows(ctx: &egui::Context, inputs: &SnippetInputs) -> Vec<SnippetRow> {
    let Some(host) = ExtensionSlotHost::from_ctx(ctx) else {
        return builtin::snippet_rows(inputs, &[]);
    };
    let now = ctx.input(|input| input.time);
    let mut guard = host.lock();
    guard.bound.request_stale(now, &SnippetCallArgs::from_inputs(inputs));
    guard.bound.rows(inputs)
}

/// Viewport overlay and dashboard widgets of unbound packages: a read-only stack in the viewport's top-left corner.
pub(crate) fn extension_overlay_system(mut egui_ctx: EguiContexts, host: Res<ExtensionSlotHost>) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let mut guard = host.lock();
    let HostInner { registry, slots, bound, active_petal, .. } = &mut *guard;
    let skip = bound_packages(bound, active_petal.as_deref());
    let items: Vec<&UiContribution> = registry
        .overlays
        .iter()
        .chain(registry.dashboard_widgets.iter())
        .filter(|c| c.stream.is_some() && !skip.contains(&c.plugin_id))
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
                .show(ui, |ui| draw_stream_slot(ui, &items, slots));
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
