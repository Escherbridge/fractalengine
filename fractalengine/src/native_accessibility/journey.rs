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
    modal: fe_ui::ui_shell::ModalManagerState,
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
    extend_scheduled_lifecycle_journey();
}

// The pinned SDK poll is private; this fixture uses its exact public-map drain boundary.
fn flattened_poll(
    handlers: Res<bevy::winit::accessibility::WinitActionRequestHandlers>,
    mut observed: ResMut<Flattened>,
) {
    for (_, handler) in handlers.iter() {
        while let Some(request) = handler.lock().unwrap().pop_front() {
            observed.0.push(request.target);
        }
    }
}

#[derive(Resource, Default)]
struct Flattened(Vec<ak::NodeId>);

#[derive(Resource)]
struct LateIngress(Handler, ak::ActionRequest);

fn enqueue_after_intake(late: Option<Res<LateIngress>>) {
    if let Some(late) = late {
        late.0.lock().unwrap().push_back(late.1.clone());
    }
}

fn scheduled_output(world: &mut World, entity: Entity) -> ak::ActionRequest {
    let ctx = world
        .get_mut::<bevy_egui::EguiContext>(entity)
        .unwrap()
        .get_mut()
        .clone();
    let guard = ui::product_guard(
        &ctx,
        world.resource::<fe_ui::actions::UiManager>(),
        world.resource::<fe_ui::node_manager::NodeManager>(),
        world.resource::<fe_ui::navigation_manager::NavigationManager>(),
        world.resource::<fe_ui::verse_manager::VerseManager>(),
        world.resource::<fe_ui::plugin::LocalUserRole>(),
        world.resource::<fe_ui::ui_shell::topbar::TopbarState>(),
        world.resource::<fe_ui::ui_shell::ModalManagerState>(),
        world.resource::<fe_ui::gis::PathEditorState>(),
    );
    let mut id = egui::Id::NULL;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        ui::begin_pass(ctx, guard);
        egui::CentralPanel::default().show(ctx, |ui| {
            id = ui
                .button("Scheduled counter")
                .named(ui, "Scheduled counter")
                .id;
        });
        ui::finish_pass(ctx, guard);
    });
    world
        .get_mut::<bevy_egui::EguiOutput>(entity)
        .unwrap()
        .platform_output = output.platform_output;
    ak::ActionRequest {
        target: ak::NodeId(id.value()),
        action: ak::Action::Click,
        data: None,
    }
}

fn begin_scheduled_input(
    mut contexts: Query<(&mut bevy_egui::EguiContext, &mut bevy_egui::EguiInput)>,
) {
    for (mut context, mut input) in &mut contexts {
        context.get_mut().begin_pass(input.0.take());
    }
}

fn freeze_after_begin(world: &mut World) {
    // A second buffered request has not yet begun; both reservoirs must be retired.
    let request = world.resource::<LateIngress>().1.clone();
    let mut inputs = world.query::<&mut bevy_egui::EguiInput>();
    for mut input in inputs.iter_mut(world) {
        input
            .events
            .push(egui::Event::AccessKitActionRequest(request.clone()));
        input.events.push(egui::Event::PointerGone);
    }
    let mut contexts = world.query::<&mut bevy_egui::EguiContext>();
    for mut context in contexts.iter_mut(world) {
        assert!(
            context.get_mut().input(|input| input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::AccessKitActionRequest(_)))),
            "BeginPass has consumed the previously admitted event before retirement"
        );
    }
    let mut terminal = fe_runtime::maintenance::TerminalMode::default();
    terminal.enter();
    world.insert_resource(terminal);
    super::retire(world);
}

fn terminal_replacement(
    mut contexts: Query<(&mut bevy_egui::EguiContext, &bevy_egui::EguiInput)>,
    previous: Res<LateIngress>,
) {
    for (mut context, input) in &mut contexts {
        assert!(
            !input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::AccessKitActionRequest(_)))
        );
        assert!(
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::PointerGone))
        );
        let ctx = context.get_mut();
        assert!(!ctx.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::AccessKitActionRequest(_)))
        }));
        assert!(ctx.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::PointerMoved(_)))
        }));
        egui::CentralPanel::default().show(ctx, |ui| {
            let target = ui.button("Scheduled counter");
            assert_eq!(
                target.id.value(),
                previous.1.target.0,
                "the replacement really recreates the formerly matching target"
            );
            assert!(
                !target.clicked(),
                "terminal traversal cannot inherit the matching begun native click"
            );
        });
        let _ = ctx.end_pass();
    }
}

fn extend_scheduled_lifecycle_journey() {
    use bevy::a11y::{AccessibilityRequested, AccessibilitySystems};
    use bevy::window::{PrimaryWindow, WindowClosed, WindowRef};
    use bevy::winit::accessibility::WinitActionRequestHandlers;
    use bevy_egui::{EguiContext, PrimaryEguiContext};
    let mut app = App::new();
    app.init_resource::<Adapter>()
        .init_resource::<super::DetachedHandler>()
        .init_resource::<WinitActionRequestHandlers>()
        .init_resource::<AccessibilityRequested>()
        .init_resource::<WindowToEguiContextMap>()
        .init_resource::<fe_ui::actions::UiManager>()
        .init_resource::<fe_ui::node_manager::NodeManager>()
        .init_resource::<fe_ui::navigation_manager::NavigationManager>()
        .init_resource::<fe_ui::verse_manager::VerseManager>()
        .init_resource::<fe_ui::plugin::LocalUserRole>()
        .init_resource::<fe_ui::ui_shell::topbar::TopbarState>()
        .init_resource::<fe_ui::ui_shell::ModalManagerState>()
        .init_resource::<fe_ui::gis::PathEditorState>()
        .init_resource::<Flattened>()
        .add_message::<WindowClosed>()
        .add_systems(
            PostUpdate,
            super::intake.before(AccessibilitySystems::Update),
        )
        .add_systems(
            PostUpdate,
            enqueue_after_intake
                .after(super::intake)
                .before(AccessibilitySystems::Update),
        )
        .add_systems(
            PostUpdate,
            flattened_poll.in_set(AccessibilitySystems::Update),
        )
        .add_systems(
            PostUpdate,
            super::restore.after(AccessibilitySystems::Update),
        );
    app.world().resource::<AccessibilityRequested>().set(true);
    let window = app
        .world_mut()
        .spawn((Window::default(), PrimaryWindow))
        .id();
    let unrelated_window = app.world_mut().spawn(Window::default()).id();
    let mut egui = EguiContext::default();
    egui.get_mut().enable_accesskit();
    ui::activate(egui.get_mut());
    let ctx = egui.get_mut().clone();
    let entity = app
        .world_mut()
        .spawn((
            egui,
            PrimaryEguiContext,
            bevy::camera::RenderTarget::Window(WindowRef::Entity(window)),
        ))
        .id();
    {
        let mut map = app.world_mut().resource_mut::<WindowToEguiContextMap>();
        map.window_to_contexts
            .entry(window)
            .or_default()
            .insert(entity);
        map.context_to_window.insert(entity, window);
    }
    let handler: Handler = Arc::new(Mutex::new(Default::default()));
    let unrelated: Handler = Arc::new(Mutex::new(Default::default()));
    {
        let mut handlers = app.world_mut().resource_mut::<WinitActionRequestHandlers>();
        handlers.insert(window, handler.clone());
        handlers.insert(unrelated_window, unrelated.clone());
    }
    let _ = scheduled_output(app.world_mut(), entity);
    app.update();
    assert!(app.world().resource::<Adapter>().owns_window(window));
    for ambiguous in [false, true] {
        if ambiguous {
            app.world_mut()
                .entity_mut(unrelated_window)
                .insert(PrimaryWindow);
        } else {
            app.world_mut().entity_mut(window).remove::<PrimaryWindow>();
        }
        let request = scheduled_output(app.world_mut(), entity);
        handler.lock().unwrap().push_back(request.clone());
        unrelated.lock().unwrap().push_back(ak::ActionRequest {
            target: ak::NodeId(888),
            ..request.clone()
        });
        app.world_mut()
            .insert_resource(LateIngress(handler.clone(), request));
        let before = app.world().resource::<Flattened>().0.len();
        app.update();
        assert_eq!(
            &app.world().resource::<Flattened>().0[before..],
            &[ak::NodeId(888)],
            "zero/two primary windows quarantine the exact former queue, including late ingress, without consuming unrelated input"
        );
        assert!(app.world().resource::<Adapter>().owns_window(window));
        assert!(Arc::ptr_eq(
            app.world()
                .resource::<WinitActionRequestHandlers>()
                .get(&window)
                .unwrap(),
            &handler
        ));
        app.world_mut().remove_resource::<LateIngress>();
        app.world_mut().entity_mut(window).insert(PrimaryWindow);
        app.world_mut()
            .entity_mut(unrelated_window)
            .remove::<PrimaryWindow>();
        let _ = scheduled_output(app.world_mut(), entity);
        app.update();
        assert!(
            handler.lock().unwrap().is_empty(),
            "fresh output discards late uncertain backlog"
        );
        let _ = scheduled_output(app.world_mut(), entity);
        app.update();
        let request = scheduled_output(app.world_mut(), entity);
        handler.lock().unwrap().push_back(request.clone());
        app.update();
        let guard = ui::product_guard(
            &ctx,
            app.world().resource::<fe_ui::actions::UiManager>(),
            app.world().resource::<fe_ui::node_manager::NodeManager>(),
            app.world()
                .resource::<fe_ui::navigation_manager::NavigationManager>(),
            app.world().resource::<fe_ui::verse_manager::VerseManager>(),
            app.world().resource::<fe_ui::plugin::LocalUserRole>(),
            app.world()
                .resource::<fe_ui::ui_shell::topbar::TopbarState>(),
            app.world()
                .resource::<fe_ui::ui_shell::ModalManagerState>(),
            app.world().resource::<fe_ui::gis::PathEditorState>(),
        );
        assert!(
            app.world_mut()
                .resource_mut::<Adapter>()
                .next(&ctx, guard, Instant::now())
                .is_some(),
            "later fresh unique output restores ordinary admission"
        );
        // Consume this staged delivery before entering another independent lifetime case.
        ui::retire(&ctx);
        app.world_mut().resource_mut::<Adapter>().quarantine();
        ui::activate(&ctx);
    }
    let request = scheduled_output(app.world_mut(), entity);
    let snapshot = ui::complete(
        &ctx,
        app.world()
            .get::<bevy_egui::EguiOutput>(entity)
            .unwrap()
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert!(ui::stage(&ctx, &snapshot, &request));
    app.world_mut()
        .insert_resource(LateIngress(handler, request.clone()));
    app.world_mut()
        .get_mut::<bevy_egui::EguiInput>(entity)
        .unwrap()
        .events
        .extend([
            egui::Event::AccessKitActionRequest(request),
            egui::Event::PointerMoved(egui::pos2(200.0, 200.0)),
        ]);
    app.add_systems(PreUpdate, begin_scheduled_input)
        .add_systems(Update, (freeze_after_begin, terminal_replacement).chain());
    app.update();
}
