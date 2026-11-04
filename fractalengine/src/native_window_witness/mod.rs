//! Actual HWND/context read-only witness; see AGENTS.md.

mod book;
#[cfg(test)]
pub(crate) mod journey;
mod windows;

use crate::native_accessibility::{OwnedRoot, Ownership};
use bevy::{
    ecs::system::NonSendMarker,
    prelude::*,
    window::{AppLifecycle, PrimaryWindow, RawHandleWrapper},
};
use bevy_egui::{
    EguiContext, EguiMultipassSchedule, EguiOutput, EguiPostUpdateSet, PrimaryEguiContext,
};
use fe_ui::native_accessibility::{self as ui, surface::SurfaceSnapshot};
use std::sync::OnceLock;

const ROOT_ID: &str = "fe.native.window.v1";
static PROCESS: OnceLock<Option<windows::Process>> = OnceLock::new();

struct NativeWindow {
    _wrapper: RawHandleWrapper,
    hwnd: isize,
}
struct Binding {
    native: NativeWindow,
    root: OwnedRoot,
    revision: u64,
    window_epoch: u64,
    context_epoch: u64,
    epoch: u64,
    last_pass: u64,
}
#[derive(Clone, Copy)]
struct Prepared {
    surface: SurfaceSnapshot,
    guard: ui::Guard,
    tick: u64,
}
#[derive(Resource, Default)]
struct Lifecycle {
    running: bool,
    cursor: bevy::ecs::message::MessageCursor<AppLifecycle>,
}
#[derive(Resource, Default)]
struct Witness {
    binding: Option<Binding>,
    prepared: Option<Prepared>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Witness>()
        .init_resource::<Lifecycle>()
        .add_systems(First, (observe_lifecycle, clear_frame).chain())
        .add_systems(
            PostUpdate,
            annotate
                .after(EguiPostUpdateSet::ProcessOutput)
                .before(bevy_egui::update_accessibility_system)
                .in_set(EguiPostUpdateSet::PostProcessOutput),
        )
        .add_systems(
            PostUpdate,
            (observe_lifecycle, observe_native_commands)
                .chain()
                .after(EguiPostUpdateSet::ProcessOutput)
                .before(annotate),
        )
        .add_systems(Last, (observe_lifecycle, publish).chain());
    observe_lifetimes(app);
}

fn observe_lifecycle(
    events: Res<Messages<AppLifecycle>>,
    mut lifecycle: ResMut<Lifecycle>,
    mut witness: ResMut<Witness>,
    _main_thread: NonSendMarker,
) {
    let Lifecycle { running, cursor } = &mut *lifecycle;
    for state in cursor.read(&events) {
        let next = *state == AppLifecycle::Running;
        if !next || !*running {
            reject(&mut witness);
            ui::surface::transition();
        }
        *running = next;
    }
}

fn observe_native_commands(mut commands: MessageReader<fe_webview::ipc::BrowserCommand>) {
    let mut opened = false;
    for command in commands.read() {
        opened |= !matches!(
            command,
            fe_webview::ipc::BrowserCommand::Close | fe_webview::ipc::BrowserCommand::GetUrl
        );
    }
    if opened {
        ui::surface::native_overlay_unproved();
        book::invalidate();
    }
}

fn observe_lifetimes(app: &mut App) {
    macro_rules! invalidate_lifetime {
        ($component:ty) => {
            app.add_observer(|_: On<Insert, $component>| book::invalidate());
            app.add_observer(|_: On<Replace, $component>| book::invalidate());
            app.add_observer(|_: On<Remove, $component>| book::invalidate());
        };
    }
    invalidate_lifetime!(RawHandleWrapper);
    invalidate_lifetime!(EguiContext);
    invalidate_lifetime!(Window);
    invalidate_lifetime!(PrimaryWindow);
    invalidate_lifetime!(PrimaryEguiContext);
    invalidate_lifetime!(EguiMultipassSchedule);
    invalidate_lifetime!(bevy::camera::RenderTarget);
}

pub(super) fn retire(world: &mut World) {
    book::invalidate();
    if let Some(mut witness) = world.get_resource_mut::<Witness>() {
        witness.prepared = None;
        witness.binding = None;
    }
}

fn clear_frame(
    mut witness: ResMut<Witness>,
    lifecycle: Res<Lifecycle>,
    terminal: Option<Res<fe_runtime::maintenance::TerminalMode>>,
    _main_thread: NonSendMarker,
) {
    let revision = book::begin_frame();
    witness.prepared = None;
    if !lifecycle.running
        || terminal.is_some_and(|mode| mode.is_terminal())
        || witness
            .binding
            .as_ref()
            .is_some_and(|binding| Some(binding.revision) != revision)
    {
        witness.binding = None;
    }
}

fn reject(witness: &mut Witness) {
    book::invalidate();
    witness.prepared = None;
    witness.binding = None;
}

fn record(
    process: windows::Process,
    binding: &Binding,
    surface: SurfaceSnapshot,
    tick: u64,
) -> Option<String> {
    let status = format!(
        "FEW1;p={};birth={:016x};w={};c={};b={};s={};pass={};tick={};class=scene;profile=scene_empty_v1;complete=1",
        process.pid,
        process.birth,
        binding.window_epoch,
        binding.context_epoch,
        binding.epoch,
        surface.epoch,
        surface.pass,
        tick
    );
    (status.len() <= 512 && status.is_ascii()).then_some(status)
}

fn annotate(
    mut reads: ParamSet<(Ownership, Query<&mut EguiOutput>)>,
    wrappers: Query<Ref<RawHandleWrapper>>,
    mut witness: ResMut<Witness>,
    lifecycle: Res<Lifecycle>,
    terminal: Option<Res<fe_runtime::maintenance::TerminalMode>>,
    capture: Option<Res<fe_renderer::camera::CameraCaptureLock>>,
    _main_thread: NonSendMarker,
) {
    if !lifecycle.running || capture.is_some() || terminal.is_some_and(|mode| mode.is_terminal()) {
        reject(&mut witness);
        return;
    }
    let Some(root) = reads.p0().current() else {
        reject(&mut witness);
        return;
    };
    let Some(surface) = ui::surface::snapshot(&root.egui, root.guard) else {
        reject(&mut witness);
        return;
    };
    let Ok(wrapper) = wrappers.get(root.window) else {
        reject(&mut witness);
        return;
    };
    let Some(hwnd) = windows::handle(&wrapper) else {
        reject(&mut witness);
        return;
    };
    let Some(process) = *PROCESS.get_or_init(windows::process) else {
        reject(&mut witness);
        return;
    };
    let Some(revision) = book::revision() else {
        reject(&mut witness);
        return;
    };
    let new_binding = witness.binding.as_ref().is_none_or(|binding| {
        binding.revision != revision || !binding.root.same_lifetime(&root) || wrapper.is_changed()
    });
    if new_binding {
        reject(&mut witness);
        let Some((revision, window_epoch, context_epoch, epoch)) = book::revision()
            .zip(book::fresh())
            .zip(book::fresh())
            .zip(book::fresh())
            .map(|(((revision, window), context), epoch)| (revision, window, context, epoch))
        else {
            return;
        };
        witness.binding = Some(Binding {
            native: NativeWindow {
                _wrapper: (*wrapper).clone(),
                hwnd,
            },
            root,
            revision,
            window_epoch,
            context_epoch,
            epoch,
            last_pass: 0,
        });
    } else if let Some(binding) = witness.binding.as_mut() {
        binding.root = root;
    }
    let binding = witness.binding.as_ref().expect("bound exact native window");
    if surface.pass <= binding.last_pass {
        reject(&mut witness);
        return;
    }
    let tick = windows::tick();
    let Some(status) = record(process, binding, surface, tick) else {
        reject(&mut witness);
        return;
    };
    let mut outputs = reads.p1();
    let Ok(mut output) = outputs.get_mut(binding.root.context) else {
        reject(&mut witness);
        return;
    };
    let Some(update) = output.platform_output.accesskit_update.as_mut() else {
        reject(&mut witness);
        return;
    };
    let Some(completed) = ui::complete(&binding.root.egui, update).filter(|completed| {
        completed.guard() == binding.root.guard && completed.pass() == surface.pass
    }) else {
        reject(&mut witness);
        return;
    };
    let Some(root_id) = update.tree.as_ref().map(|tree| tree.root) else {
        reject(&mut witness);
        return;
    };
    let Some((_, node)) = update.nodes.iter_mut().find(|(id, _)| *id == root_id) else {
        reject(&mut witness);
        return;
    };
    node.set_automation_id(ROOT_ID);
    node.set_description(status);
    witness.prepared = Some(Prepared { surface, guard: completed.guard(), tick });
}

fn publish(
    mut ownership: Ownership,
    mut witness: ResMut<Witness>,
    lifecycle: Res<Lifecycle>,
    terminal: Option<Res<fe_runtime::maintenance::TerminalMode>>,
    capture: Option<Res<fe_renderer::camera::CameraCaptureLock>>,
    _main_thread: NonSendMarker,
) {
    let Some(prepared) = witness.prepared.take() else {
        return;
    };
    let Some(binding) = witness.binding.as_mut() else {
        return;
    };
    let current = ownership.current();
    let valid = lifecycle.running
        && capture.is_none()
        && !terminal.is_some_and(|mode| mode.is_terminal())
        && current.as_ref().is_some_and(|root| {
            binding.root.same_lifetime(root)
                && prepared.guard == root.guard
                && binding.root.guard == prepared.guard
        })
        && ui::surface::snapshot(&binding.root.egui, prepared.guard) == Some(prepared.surface)
        && windows::tick()
            .checked_sub(prepared.tick)
            .is_some_and(|age| age <= 250)
        && book::publish(binding.revision, binding.native.hwnd, binding.epoch);
    if valid {
        binding.last_pass = prepared.surface.pass;
    } else {
        reject(&mut witness);
    }
}
