//! Extends the existing input/clipboard journey without native OS operations.

use super::{Handler, state::Adapter, unique_context};
use bevy::prelude::*;
use bevy_egui::{
    egui::{self, accesskit as ak},
    input::WindowToEguiContextMap,
};
use fe_ui::{accessibility::ResponseExt, native_accessibility as ui};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Surface {
    ctx: egui::Context,
    ui: fe_ui::actions::UiManager,
    nodes: fe_ui::node_manager::NodeManager,
    nav: fe_ui::navigation_manager::NavigationManager,
    tree: fe_ui::verse_manager::VerseManager,
    role: fe_ui::plugin::LocalUserRole,
    topbar: fe_ui::ui_shell::topbar::TopbarState,
    modal: fe_ui::ui_shell::modal::ModalManagerState,
    path: fe_ui::gis::PathEditorState,
    count: usize,
    disabled: bool,
}

impl Surface {
    fn guard(&self) -> ui::Guard {
        ui::product_guard(
            &self.ctx,
            &self.ui,
            &self.nodes,
            &self.nav,
            &self.tree,
            &self.role,
            &self.topbar,
            &self.modal,
            &self.path,
        )
    }

    fn frame(&mut self, request: Option<ak::ActionRequest>) -> (ui::Snapshot, ak::ActionRequest) {
        let ctx = self.ctx.clone();
        let guard = self.guard();
        let mut id = egui::Id::NULL;
        let events = request
            .into_iter()
            .map(egui::Event::AccessKitActionRequest)
            .collect();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                ui::begin_pass(ctx, guard);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let button = ui
                        .add_enabled(!self.disabled, egui::Button::new("Counter"))
                        .named(ui, "Ordinary native counter");
                    id = button.id;
                    if button.clicked() {
                        self.count += 1;
                    }
                });
                ui::finish_pass(ctx, guard);
            },
        );
        let snapshot = ui::complete(
            &ctx,
            output.platform_output.accesskit_update.as_ref().unwrap(),
        )
        .expect("complete ordinary context output");
        (
            snapshot,
            ak::ActionRequest {
                target: ak::NodeId(id.value()),
                action: ak::Action::Click,
                data: None,
            },
        )
    }
}

pub(crate) fn extend_owned_input_journey() {
    let window = Entity::from_raw_u32(101).unwrap();
    let context = Entity::from_raw_u32(102).unwrap();
    let other = Entity::from_raw_u32(103).unwrap();
    let mut map = WindowToEguiContextMap::default();
    assert!(unique_context(window, &map, [(context, Some(window), true)].into_iter()).is_none());
    map.window_to_contexts
        .entry(window)
        .or_default()
        .insert(context);
    map.context_to_window.insert(context, window);
    assert_eq!(
        unique_context(window, &map, [(context, Some(window), true)].into_iter()),
        Some(context)
    );
    assert!(
        unique_context(
            window,
            &map,
            [(context, Some(window), true), (other, Some(window), true)].into_iter()
        )
        .is_none(),
        "actual extra contexts refuse even when the mapping is stale"
    );
    assert!(
        unique_context(window, &map, [(context, Some(window), false)].into_iter()).is_none(),
        "manual/unknown/nonroot contexts refuse"
    );
    map.context_to_window.insert(other, window);
    assert!(
        unique_context(window, &map, [(context, Some(window), true)].into_iter()).is_none(),
        "an extra output owner cannot be hidden by an inconsistent forward map"
    );
    map.context_to_window.remove(&other);
    map.window_to_contexts
        .get_mut(&window)
        .unwrap()
        .insert(other);
    assert!(unique_context(window, &map, [(context, Some(window), true)].into_iter()).is_none());

    let mut surface = Surface::default();
    surface.ctx.enable_accesskit();
    ui::activate(&surface.ctx);
    let handler: Handler = Arc::new(Mutex::new(Default::default()));
    let unrelated: Handler = Arc::new(Mutex::new(Default::default()));
    let mut handlers = bevy::winit::accessibility::WinitActionRequestHandlers::default();
    handlers.insert(window, handler.clone());
    handlers.insert(other, unrelated.clone());
    let mut held = super::DetachedHandler::default();
    let detached = super::detach(window, &mut handlers, &mut held).unwrap();
    assert!(Arc::ptr_eq(&detached, &handler));
    assert!(!handlers.contains_key(&window));
    assert!(handlers.contains_key(&other));
    detached.lock().unwrap().push_back(ak::ActionRequest {
        target: ak::NodeId(77),
        action: ak::Action::Click,
        data: None,
    });
    assert!(
        !handlers.contains_key(&window),
        "an enqueue after intake is hidden from the flattened poll"
    );
    assert!(super::restore_entry(
        window,
        held.0.take().unwrap().1,
        &mut handlers,
        true
    ));
    assert_eq!(
        handler.lock().unwrap().len(),
        1,
        "the late request stays on the same Arc for next owned intake"
    );
    handler.lock().unwrap().clear();
    let _ = super::detach(window, &mut handlers, &mut held).unwrap();
    let new_handler: Handler = Arc::new(Mutex::new(Default::default()));
    handlers.insert(window, new_handler.clone());
    assert!(
        !super::restore_entry(window, held.0.take().unwrap().1, &mut handlers, true),
        "a replacement installed during the bracket is never overwritten"
    );
    assert!(Arc::ptr_eq(handlers.get(&window).unwrap(), &new_handler));
    let now = Instant::now();
    let mut adapter = Adapter::default();
    adapter.bind(window, context, &surface.ctx, &handler);
    let (initial, uncertain) = surface.frame(None);
    handler.lock().unwrap().push_back(uncertain);
    adapter.admit(Some(initial), &handler, now);
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "first complete output discards ingress from before its ownership barrier"
    );
    let (snapshot, click) = surface.frame(None);
    unrelated.lock().unwrap().push_back(click.clone());
    handler
        .lock()
        .unwrap()
        .extend([click.clone(), click.clone()]);
    adapter.admit(Some(snapshot), &handler, now);
    assert!(handler.lock().unwrap().is_empty());
    assert_eq!(
        unrelated.lock().unwrap().len(),
        1,
        "the owned adapter never drains another window queue"
    );
    let first = adapter.next(&surface.ctx, surface.guard(), now).unwrap();
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "one delivery per pass retains the ordered remainder"
    );
    let (snapshot, _) = surface.frame(Some(first));
    assert_eq!(surface.count, 1);
    adapter.admit(Some(snapshot), &handler, now);
    let second = adapter.next(&surface.ctx, surface.guard(), now).unwrap();
    let (snapshot, _) = surface.frame(Some(second));
    assert_eq!(
        surface.count, 2,
        "repeated clicks are delivered across distinct completed passes"
    );
    adapter.admit(Some(snapshot), &handler, now);
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "a delivered request is never replayed"
    );

    let (snapshot, click) = surface.frame(None);
    handler.lock().unwrap().push_back(click.clone());
    adapter.admit(Some(snapshot), &handler, now);
    assert!(
        adapter
            .next(
                &surface.ctx,
                surface.guard(),
                now + Duration::from_millis(251)
            )
            .is_none(),
        "age is checked at actual delivery"
    );
    let (snapshot, click) = surface.frame(None);
    let mut focus = click.clone();
    focus.action = ak::Action::Focus;
    let mut value = click.clone();
    value.data = Some(ak::ActionData::Value("not forwarded or logged".into()));
    let unknown = ak::ActionRequest {
        target: ak::NodeId(u64::MAX),
        ..click.clone()
    };
    handler.lock().unwrap().extend([focus, value, unknown]);
    adapter.admit(Some(snapshot), &handler, now);
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "Focus, payloads and unknown IDs are explicitly unsupported"
    );
    let (snapshot, click) = surface.frame(None);
    for _ in 0..33 {
        handler.lock().unwrap().push_back(click.clone());
    }
    adapter.admit(Some(snapshot), &handler, now);
    assert!(handler.lock().unwrap().is_empty());
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "overflow refuses the batch without a truncated success"
    );

    let (fresh, _) = surface.frame(None);
    adapter.admit(Some(fresh), &handler, now);

    let (snapshot, click) = surface.frame(None);
    handler.lock().unwrap().push_back(click);
    adapter.admit(Some(snapshot), &handler, now);
    let replacement: Handler = Arc::new(Mutex::new(Default::default()));
    adapter.bind(window, context, &surface.ctx, &replacement);
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "a replacement handler cannot inherit old intent"
    );
    let (fresh, _) = surface.frame(None);
    adapter.admit(Some(fresh), &replacement, now);
    let (snapshot, click) = surface.frame(None);
    replacement.lock().unwrap().push_back(click);
    adapter.admit(Some(snapshot), &replacement, now);
    surface.nav.navigate_to_verse("new-intent", "New intent");
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "a changed product/navigation context retires queued clicks"
    );
    let (fresh, _) = surface.frame(None);
    adapter.admit(Some(fresh), &replacement, now);
    let (snapshot, click) = surface.frame(None);
    replacement.lock().unwrap().push_back(click);
    adapter.admit(Some(snapshot), &replacement, now);
    let held = replacement.lock().unwrap();
    adapter.admit(None, &replacement, now);
    drop(held);
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "contention never blocks or retains an eligible old snapshot"
    );

    surface.disabled = true;
    let (snapshot, click) = surface.frame(None);
    assert!(!snapshot.allows(&click));
    replacement.lock().unwrap().push_back(click);
    adapter.admit(Some(snapshot), &replacement, now);
    assert!(adapter.next(&surface.ctx, surface.guard(), now).is_none());
    surface.disabled = false;
    let (snapshot, click) = surface.frame(None);
    replacement.lock().unwrap().push_back(click);
    adapter.admit(Some(snapshot), &replacement, now);
    adapter.admit(None, &replacement, now);
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "missing complete output clears prior validity"
    );
    let (fresh, _) = surface.frame(None);
    adapter.admit(Some(fresh), &replacement, now);
    let (snapshot, click) = surface.frame(None);
    replacement.lock().unwrap().push_back(click);
    adapter.admit(Some(snapshot), &replacement, now);
    surface.ctx = egui::Context::default();
    surface.ctx.enable_accesskit();
    adapter.bind(window, context, &surface.ctx, &replacement);
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "a recreated egui Context on the same Entity cannot inherit input"
    );
    let (fresh, _) = surface.frame(None);
    adapter.admit(Some(fresh), &replacement, now);
    let (snapshot, click) = surface.frame(None);
    replacement.lock().unwrap().push_back(click);
    adapter.admit(Some(snapshot), &replacement, now);
    adapter.bind(other, context, &surface.ctx, &replacement);
    assert!(!adapter.owns_window(window) && adapter.owns_window(other));
    assert!(
        adapter.next(&surface.ctx, surface.guard(), now).is_none(),
        "window generation/owner changes retire intent"
    );
    adapter.clear();
    ui::activate(&surface.ctx);
    let before = surface.count;
    let _ = surface.frame(None);
    assert_eq!(surface.count, before);
    assert!(ui::neutral_raw(&egui::RawInput::default(), &surface.ctx));
    let modifiers = egui::RawInput {
        modifiers: egui::Modifiers {
            ctrl: true,
            ..egui::Modifiers::NONE
        },
        ..Default::default()
    };
    assert!(!ui::neutral_raw(&modifiers, &surface.ctx));
    assert!(
        modifiers.modifiers.ctrl,
        "the adapter does not rewrite physical modifier state"
    );
}
