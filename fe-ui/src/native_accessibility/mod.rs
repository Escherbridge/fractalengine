//! Ordinary native action validity and UI-use guards; see AGENTS.md.

mod confirmation;
#[cfg(test)]
pub(crate) mod journey;
mod snapshot;
pub mod surface;

use bevy_egui::egui::{self, accesskit as ak};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(crate) use confirmation::{Confirmation, confirmation_state};
pub use snapshot::{Snapshot, complete};

pub const MAX_NODES: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guard {
    navigation: u64,
    scene: u128,
    hierarchy: u64,
    primary_source: Option<crate::verse_manager::SceneAdmission>,
    access: Option<u128>,
    selection: u64,
    role: Option<fe_database::RoleLevel>,
    ready: bool,
    capture_view: bool,
    capture_labels_clear: bool,
}

pub fn product_guard(
    ctx: &egui::Context,
    ui: &crate::actions::UiManager,
    nodes: &crate::node_manager::NodeManager,
    nav: &crate::navigation_manager::NavigationManager,
    manager: &crate::verse_manager::VerseManager,
    role: &crate::plugin::LocalUserRole,
    topbar: &crate::ui_shell::topbar::TopbarState,
    modal: &crate::ui_shell::modal::ModalManagerState,
    path: &crate::gis::PathEditorState,
) -> Guard {
    Guard {
        navigation: nav.intent_revision,
        scene: manager.scene_incarnation(),
        hierarchy: manager.scene_hierarchy_revision(),
        primary_source: nodes.primary().and_then(|node| {
            manager
                .petal_id_of(&node.node_id)
                .and_then(|petal| manager.scene_admission(&node.node_id, petal))
        }),
        access: manager.scene_access_generation(),
        selection: nodes.generation(),
        role: role.role,
        capture_view: role.role.is_some_and(|role| role.can_view())
            && !manager.scene_access_blocked(),
        capture_labels_clear: path.editing_track_id.is_none() || path.points.is_empty(),
        ready: matches!(ui.active_dialog, crate::dialogs::ActiveDialog::None)
            && !topbar.close_pending
            && modal.disabled_panels.is_empty()
            && !nodes.gesture_active
            && !nodes.is_dragging()
            && path.pending_pen_create.is_none()
            && !egui::Popup::is_any_open(ctx)
            && ctx.memory(|memory| memory.top_modal_layer().is_none()),
    }
}

#[derive(Clone, Copy)]
struct Widget {
    id: egui::Id,
    layer: egui::LayerId,
    enabled: bool,
    confirmation: Option<confirmation::Owner>,
}

#[derive(Clone, Copy)]
struct Delivery {
    pass: u64,
    guard: Guard,
    target: ak::NodeId,
    action: ak::Action,
    confirmation: Option<confirmation::Owner>,
}

struct Pass {
    lifetime: u128,
    number: u64,
    guard: Option<Guard>,
    widgets: HashMap<u64, Widget>,
    invalid: bool,
    delivery: Option<Delivery>,
    semantic_click: Option<(ak::NodeId, Option<confirmation::Owner>)>,
}

impl Default for Pass {
    fn default() -> Self {
        Self {
            lifetime: ulid::Ulid::new().into(),
            number: 0,
            guard: None,
            widgets: HashMap::new(),
            invalid: true,
            delivery: None,
            semantic_click: None,
        }
    }
}

type Shared = Arc<Mutex<Pass>>;

fn key() -> egui::Id {
    egui::Id::new("fe-ordinary-native-accessibility")
}

fn shared(ctx: &egui::Context) -> Option<Shared> {
    ctx.data(|data| data.get_temp::<Shared>(key()))
}

pub fn activate(ctx: &egui::Context) {
    if shared(ctx).is_none() {
        ctx.data_mut(|data| data.insert_temp(key(), Shared::default()));
    }
}

pub fn retire(ctx: &egui::Context) {
    surface::retire(ctx);
    strip_requests(ctx);
    ctx.data_mut(|data| data.remove::<Shared>(key()));
}

pub fn retire_input(ctx: &egui::Context, input: &mut egui::RawInput) {
    retire(ctx);
    input
        .events
        .retain(|event| !matches!(event, egui::Event::AccessKitActionRequest(_)));
}

pub fn neutral_raw(input: &egui::RawInput, ctx: &egui::Context) -> bool {
    neutral(input.modifiers, &input.events) && !ctx.input(|input| input.pointer.any_down())
}

fn neutral(modifiers: egui::Modifiers, events: &[egui::Event]) -> bool {
    !modifiers.alt
        && !modifiers.ctrl
        && !modifiers.shift
        && !modifiers.command
        && !modifiers.mac_cmd
        && events.iter().all(|event| {
            matches!(
                event,
                egui::Event::PointerMoved(_)
                    | egui::Event::PointerGone
                    | egui::Event::WindowFocused(_)
                    | egui::Event::AccessKitActionRequest(_)
            )
        })
}

pub fn stage(ctx: &egui::Context, snapshot: &Snapshot, request: &ak::ActionRequest) -> bool {
    if !snapshot.guard.ready || !snapshot.allows(request) {
        return false;
    }
    let Some(shared) = shared(ctx) else {
        return false;
    };
    let Ok(mut pass) = shared.try_lock() else {
        return false;
    };
    if pass.delivery.is_some()
        || pass.lifetime != snapshot.lifetime
        || snapshot.pass != ctx.cumulative_pass_nr()
        || pass.number.checked_add(1) != Some(snapshot.pass)
    {
        return false;
    }
    pass.delivery = Some(Delivery {
        pass: ctx.cumulative_pass_nr(),
        guard: snapshot.guard,
        target: request.target,
        action: request.action,
        confirmation: snapshot.confirmation(request.target),
    });
    true
}

pub fn begin_pass(ctx: &egui::Context, guard: Guard) {
    let Some(shared) = shared(ctx) else {
        strip_requests(ctx);
        return;
    };
    let Ok(mut pass) = shared.try_lock() else {
        strip_requests(ctx);
        return;
    };
    let number = ctx.cumulative_pass_nr();
    let neutral =
        ctx.input(|input| neutral(input.modifiers, &input.events) && !input.pointer.any_down());
    let delivery = pass.delivery.take().filter(|delivery| {
        delivery.pass == number && delivery.guard == guard && guard.ready && neutral
    });
    let valid = ctx.input(|input| {
        let mut requests = input.events.iter().filter_map(|event| {
            if let egui::Event::AccessKitActionRequest(request) = event {
                Some(request)
            } else {
                None
            }
        });
        match (requests.next(), requests.next(), delivery) {
            (Some(request), None, Some(delivery)) => {
                request.target == delivery.target
                    && request.action == delivery.action
                    && request.data.is_none()
            }
            (None, None, None) => true,
            _ => false,
        }
    });
    if !valid {
        strip_requests(ctx);
    }
    pass.number = number;
    pass.guard = guard.ready.then_some(guard);
    pass.widgets.clear();
    pass.invalid = false;
    pass.semantic_click = delivery
        .filter(|delivery| valid && delivery.action == ak::Action::Click)
        .map(|delivery| (delivery.target, delivery.confirmation));
}

fn strip_requests(ctx: &egui::Context) {
    ctx.input_mut(|input| {
        input
            .events
            .retain(|event| !matches!(event, egui::Event::AccessKitActionRequest(_)))
    });
}

pub fn finish_pass(ctx: &egui::Context, guard: Guard) {
    if let Some(shared) = shared(ctx) {
        if let Ok(mut pass) = shared.try_lock() {
            if pass.guard != Some(guard) || !guard.ready {
                pass.guard = None;
            }
        }
    }
}

pub(crate) fn record(ui: &egui::Ui, response: &egui::Response) {
    let Some(shared) = shared(ui.ctx()) else {
        return;
    };
    let Ok(mut pass) = shared.try_lock() else {
        return;
    };
    if pass.number != ui.ctx().cumulative_pass_nr() || pass.invalid {
        return;
    }
    if pass.widgets.len() >= MAX_NODES || pass.widgets.contains_key(&response.id.value()) {
        pass.invalid = true;
        pass.widgets.clear();
        return;
    }
    pass.widgets.insert(
        response.id.value(),
        Widget {
            id: response.id,
            layer: response.layer_id,
            enabled: response.enabled()
                && ui.clip_rect().contains_rect(response.interact_rect)
                && response.interact_rect.is_positive()
                && ui
                    .ctx()
                    .memory(|memory| memory.allows_interaction(response.layer_id)),
            confirmation: None,
        },
    );
}

pub(crate) fn record_confirmation(
    ctx: &egui::Context,
    response: &egui::Response,
    owner: confirmation::Owner,
) {
    if let Some(shared) = shared(ctx) {
        if let Ok(mut pass) = shared.try_lock() {
            if let Some(widget) = pass.widgets.get_mut(&response.id.value()) {
                widget.confirmation = Some(owner);
            }
        }
    }
}

pub(crate) fn selection_allowed(ctx: &egui::Context, response: &egui::Response) -> bool {
    let native = ctx.input(|input| input.events.iter().any(|event| matches!(event,
        egui::Event::AccessKitActionRequest(request) if request.target.0 == response.id.value() && request.action == ak::Action::Click)));
    !native
        || ctx.input(|input| neutral(input.modifiers, &input.events) && !input.pointer.any_down())
}
