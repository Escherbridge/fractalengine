//! Ordinary per-window native accessibility input; see AGENTS.md.

#[cfg(test)]
pub(crate) mod journey;
mod state;

use bevy::{
    a11y::{AccessibilityRequested, AccessibilitySystems},
    camera::RenderTarget,
    ecs::{schedule::ScheduleLabel, system::SystemParam},
    prelude::*,
    window::PrimaryWindow,
    winit::accessibility::{WinitActionRequestHandler, WinitActionRequestHandlers},
};
use bevy_egui::{
    EguiContext, EguiContextSettings, EguiInput, EguiInputSet, EguiMultipassSchedule, EguiOutput,
    EguiPostUpdateSet, EguiPreUpdateSet, EguiPrimaryContextPass, PrimaryEguiContext,
    input::WindowToEguiContextMap,
};
use fe_ui::native_accessibility as ui;
use state::Adapter;
use std::sync::{Arc, Mutex};

type Handler = Arc<Mutex<WinitActionRequestHandler>>;

#[derive(Resource, Default)]
struct DetachedHandler(Option<(Entity, Handler)>);

fn detach(
    window: Entity,
    handlers: &mut WinitActionRequestHandlers,
    held: &mut DetachedHandler,
) -> Option<Handler> {
    if held.0.is_some() {
        return None;
    }
    let handler = handlers.remove(&window)?;
    held.0 = Some((window, handler.clone()));
    Some(handler)
}

fn restore_entry(
    window: Entity,
    handler: Handler,
    handlers: &mut WinitActionRequestHandlers,
    live: bool,
) -> bool {
    if !live {
        return false;
    }
    if let Some(current) = handlers.get(&window) {
        return Arc::ptr_eq(current, &handler);
    }
    handlers.insert(window, handler);
    true
}

#[derive(SystemParam)]
struct Product<'w> {
    ui: Res<'w, fe_ui::actions::UiManager>,
    nodes: Res<'w, fe_ui::node_manager::NodeManager>,
    nav: Res<'w, fe_ui::navigation_manager::NavigationManager>,
    tree: Res<'w, fe_ui::verse_manager::VerseManager>,
    role: Res<'w, fe_ui::plugin::LocalUserRole>,
    topbar: Res<'w, fe_ui::ui_shell::topbar::TopbarState>,
    modal: Res<'w, fe_ui::ui_shell::modal::ModalManagerState>,
    path: Res<'w, fe_ui::gis::PathEditorState>,
}

impl Product<'_> {
    fn guard(&self, ctx: &bevy_egui::egui::Context) -> ui::Guard {
        ui::product_guard(
            ctx,
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
}

#[derive(SystemParam)]
struct Contexts<'w, 's> {
    map: Res<'w, WindowToEguiContextMap>,
    contexts: Query<
        'w,
        's,
        (
            Entity,
            &'static RenderTarget,
            &'static mut EguiContext,
            &'static EguiContextSettings,
            &'static mut EguiInput,
            &'static EguiOutput,
            Option<&'static EguiMultipassSchedule>,
            Option<&'static PrimaryEguiContext>,
        ),
    >,
}

impl Contexts<'_, '_> {
    fn unique(&mut self, window: Entity) -> Option<Entity> {
        unique_context(
            window,
            &self.map,
            self.contexts.iter_mut().map(
                |(entity, target, mut context, settings, input, _, schedule, primary)| {
                    let target = if let RenderTarget::Window(reference) = target {
                        reference
                            .normalize(Some(window))
                            .map(|reference| reference.entity())
                    } else {
                        None
                    };
                    let supported = !settings.run_manually
                        && primary.is_some()
                        && schedule
                            .is_none_or(|schedule| schedule.0 == EguiPrimaryContextPass.intern())
                        && context.get_mut().viewport_id() == bevy_egui::egui::ViewportId::ROOT
                        && input.viewport_id == bevy_egui::egui::ViewportId::ROOT;
                    (entity, target, supported)
                },
            ),
        )
    }
}

fn unique_context(
    window: Entity,
    map: &WindowToEguiContextMap,
    candidates: impl Iterator<Item = (Entity, Option<Entity>, bool)>,
) -> Option<Entity> {
    let mapped = map.window_to_contexts.get(&window)?;
    if mapped.len() != 1 {
        return None;
    }
    let entity = *mapped.iter().next()?;
    if map.context_to_window.get(&entity) != Some(&window) {
        return None;
    }
    if map
        .context_to_window
        .values()
        .filter(|target| **target == window)
        .count()
        != 1
        || map
            .window_to_contexts
            .values()
            .filter(|contexts| contexts.contains(&entity))
            .count()
            != 1
    {
        return None;
    }
    let mut actual = None;
    for (candidate, target, supported) in candidates {
        if target != Some(window) {
            continue;
        }
        if actual.is_some() || candidate != entity || !supported {
            return None;
        }
        actual = Some(candidate);
    }
    actual
}

pub fn install(app: &mut App) {
    app.init_resource::<Adapter>()
        .init_resource::<DetachedHandler>()
        .add_systems(
            PreUpdate,
            deliver
                .after(EguiInputSet::WriteEguiEvents)
                .before(EguiPreUpdateSet::BeginPass)
                .run_if(fe_runtime::maintenance::editor_running),
        )
        .add_systems(
            PostUpdate,
            intake
                .after(EguiPostUpdateSet::PostProcessOutput)
                .after(fe_ui::ui_shell::topbar::WindowChromeApply)
                .before(AccessibilitySystems::Update),
        )
        .add_systems(PostUpdate, restore.after(AccessibilitySystems::Update));
}

pub fn retire(world: &mut World) {
    if let Some(mut adapter) = world.get_resource_mut::<Adapter>() {
        adapter.quarantine();
    }
    let mut contexts = world.query::<(&mut EguiContext, &mut EguiInput)>();
    for (mut context, mut input) in contexts.iter_mut(world) {
        ui::retire_input(context.get_mut(), &mut input.0);
    }
}

// Preserve the exact former queue across the windowless SDK poll, even while retired.
fn quarantine_owned(
    adapter: &mut Adapter,
    handlers: &mut WinitActionRequestHandlers,
    detached: &mut DetachedHandler,
) {
    let Some((window, handler)) = adapter.owned_handler() else {
        return;
    };
    adapter.quarantine();
    if handlers
        .get(&window)
        .is_some_and(|current| Arc::ptr_eq(current, &handler))
    {
        if detach(window, handlers, detached).is_none() {
            return;
        }
    } else {
        // A different handler belongs to a different owner; never remove or drain it.
        adapter.clear();
    }
    if let Ok(mut queue) = handler.try_lock() {
        queue.clear();
    }
}

fn deliver(
    mut adapter: ResMut<Adapter>,
    requested: Res<AccessibilityRequested>,
    windows: Query<Entity, (With<PrimaryWindow>, With<Window>)>,
    handlers: Res<WinitActionRequestHandlers>,
    mut contexts: Contexts,
    product: Product,
    mut removed_contexts: RemovedComponents<EguiContext>,
    mut removed_windows: RemovedComponents<Window>,
) {
    if removed_contexts
        .read()
        .any(|entity| adapter.owns_context(entity))
        || removed_windows
            .read()
            .any(|entity| adapter.owns_window(entity))
    {
        adapter.quarantine();
    }
    let Ok(window) = windows.single() else {
        adapter.quarantine();
        return;
    };
    if adapter
        .owned_handler()
        .is_some_and(|(owned, _)| owned != window)
    {
        adapter.quarantine();
        return;
    }
    let Some(handler) = handlers.get(&window) else {
        adapter.quarantine();
        return;
    };
    let Some(entity) = contexts.unique(window) else {
        adapter.quarantine();
        return;
    };
    let Ok((_, _, mut context, _, mut input, _, _, _)) = contexts.contexts.get_mut(entity) else {
        adapter.quarantine();
        return;
    };
    let ctx = context.get_mut();
    if !requested.get() {
        ui::retire(ctx);
        adapter.quarantine();
        return;
    }
    ui::activate(ctx);
    adapter.bind(window, entity, ctx, handler);
    if !ui::neutral_raw(&input.0, ctx)
        || input
            .events
            .iter()
            .any(|event| matches!(event, bevy_egui::egui::Event::AccessKitActionRequest(_)))
    {
        input
            .events
            .retain(|event| !matches!(event, bevy_egui::egui::Event::AccessKitActionRequest(_)));
        adapter.discard_pending();
        return;
    }
    if let Some(request) = adapter.next(ctx, product.guard(ctx), std::time::Instant::now()) {
        input
            .events
            .push(bevy_egui::egui::Event::AccessKitActionRequest(request));
    }
}

fn intake(
    mut adapter: ResMut<Adapter>,
    requested: Res<AccessibilityRequested>,
    windows: Query<Entity, (With<PrimaryWindow>, With<Window>)>,
    mut handlers: ResMut<WinitActionRequestHandlers>,
    mut detached: ResMut<DetachedHandler>,
    mut contexts: Contexts,
    product: Product,
    terminal: Option<Res<fe_runtime::maintenance::TerminalMode>>,
    mut removed_contexts: RemovedComponents<EguiContext>,
    mut removed_windows: RemovedComponents<Window>,
) {
    let removed = removed_contexts
        .read()
        .any(|entity| adapter.owns_context(entity))
        || removed_windows
            .read()
            .any(|entity| adapter.owns_window(entity));
    let window = windows.single().ok();
    if removed
        || terminal.is_some_and(|mode| mode.is_terminal())
        || window.is_none()
        || adapter
            .owned_handler()
            .is_some_and(|(owned, _)| Some(owned) != window)
    {
        quarantine_owned(&mut adapter, &mut handlers, &mut detached);
        return;
    }
    let window = window.expect("unique actual primary window");
    let Some(handler) = detach(window, &mut handlers, &mut detached) else {
        adapter.quarantine();
        return;
    };
    let Some(entity) = contexts.unique(window) else {
        adapter.quarantine();
        if let Ok(mut handler) = handler.try_lock() {
            handler.clear();
        }
        return;
    };
    let Ok((_, _, mut context, _, _, output, _, _)) = contexts.contexts.get_mut(entity) else {
        adapter.quarantine();
        if let Ok(mut handler) = handler.try_lock() {
            handler.clear();
        }
        return;
    };
    let ctx = context.get_mut();
    if !requested.get() {
        ui::retire(ctx);
        adapter.quarantine();
        if let Ok(mut handler) = handler.try_lock() {
            handler.clear();
        }
        return;
    }
    ui::activate(ctx);
    adapter.bind(window, entity, ctx, &handler);
    let snapshot = output
        .platform_output
        .accesskit_update
        .as_ref()
        .and_then(|update| ui::complete(ctx, update))
        .filter(|snapshot| snapshot.guard() == product.guard(ctx));
    adapter.admit(snapshot, &handler, std::time::Instant::now());
}

fn restore(
    mut detached: ResMut<DetachedHandler>,
    mut handlers: ResMut<WinitActionRequestHandlers>,
    mut adapter: ResMut<Adapter>,
    windows: Query<Entity, With<Window>>,
    mut closed: MessageReader<bevy::window::WindowClosed>,
) {
    let window = detached.0.as_ref().map(|(window, _)| *window);
    let closed = closed.read().any(|event| Some(event.window) == window);
    if let Some((window, handler)) = detached.0.take() {
        if !restore_entry(
            window,
            handler,
            &mut handlers,
            !closed && windows.contains(window),
        ) {
            adapter.clear();
        }
    }
}
