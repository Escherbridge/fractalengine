//! Positive empty-Inspector capture classification; see AGENTS.md.

use super::{Guard, shared};
use bevy_egui::egui;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

static EPOCH: AtomicU64 = AtomicU64::new(1);
static POISONED: AtomicBool = AtomicBool::new(false);
static NATIVE_DIALOGS: AtomicUsize = AtomicUsize::new(0);
static PORTAL_UNPROVED: AtomicBool = AtomicBool::new(false);

pub fn transition() {
    if EPOCH
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            value
                .checked_add(1)
                .filter(|next| *next <= isize::MAX as u64)
        })
        .is_err()
    {
        POISONED.store(true, Ordering::SeqCst);
    }
}

pub fn native_overlay_unproved() {
    PORTAL_UNPROVED.store(true, Ordering::SeqCst);
    transition();
}

pub(crate) struct NativeDialog;
impl NativeDialog {
    pub(crate) fn enter() -> Self {
        if NATIVE_DIALOGS
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_add(1)
            })
            .is_err()
        {
            POISONED.store(true, Ordering::SeqCst);
        }
        transition();
        Self
    }
}
impl Drop for NativeDialog {
    fn drop(&mut self) {
        if NATIVE_DIALOGS
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_err()
        {
            POISONED.store(true, Ordering::SeqCst);
        }
        transition();
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Producer {
    Topbar,
    Status,
    Tree,
    Inspector,
    Results,
    Viewport,
    Transient,
    Data,
}
const ALL: u16 = (1 << 8) - 1;

#[derive(Clone)]
struct Pass {
    number: u64,
    guard: Guard,
    epoch: u64,
    seen: u16,
    supported: bool,
    finished: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SurfaceSnapshot {
    pub epoch: u64,
    pub pass: u64,
    guard: Guard,
}

fn key() -> egui::Id {
    egui::Id::new("fe-positive-native-surface-v1")
}
fn native_clear() -> bool {
    !POISONED.load(Ordering::SeqCst)
        && !PORTAL_UNPROVED.load(Ordering::SeqCst)
        && NATIVE_DIALOGS.load(Ordering::SeqCst) == 0
}

pub(crate) fn begin(ctx: &egui::Context, guard: Guard) {
    if shared(ctx).is_none() {
        return;
    }
    let previous = ctx.data(|data| data.get_temp::<Pass>(key()));
    if previous.as_ref().is_some_and(|pass| pass.guard != guard) {
        transition();
    }
    let number = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        data.insert_temp(
            key(),
            Pass {
                number,
                guard,
                epoch: EPOCH.load(Ordering::SeqCst),
                seen: 0,
                supported: guard.ready
                    && guard.capture_view
                    && guard.capture_labels_clear
                    && native_clear(),
                finished: false,
            },
        )
    });
}

pub(crate) fn enter(ctx: &egui::Context, producer: Producer, supported: bool) {
    if shared(ctx).is_none() {
        return;
    }
    let number = ctx.cumulative_pass_nr();
    if !supported {
        ctx.accesskit_node_builder(
            egui::Id::new("fe-unsupported-surface").with(producer as u16),
            |node| {
                node.set_role(egui::accesskit::Role::GenericContainer);
                node.set_automation_id("fe.surface.other.v1");
            },
        );
    }
    ctx.data_mut(|data| {
        if let Some(mut pass) = data.get_temp::<Pass>(key()) {
            let bit = 1 << producer as u16;
            if pass.number != number || pass.seen & bit != 0 || !supported {
                if pass.supported {
                    transition();
                }
                pass.supported = false;
            }
            pass.seen |= bit;
            data.insert_temp(key(), pass);
        }
    });
}

pub(crate) fn credential(ctx: &egui::Context) {
    popup(ctx);
    ctx.accesskit_node_builder(egui::Id::new("fe-sensitive-surface-v1"), |node| {
        node.set_role(egui::accesskit::Role::GenericContainer);
        node.set_automation_id("fe.surface.credential.v1");
    });
}

pub(crate) fn popup(ctx: &egui::Context) {
    transition();
    ctx.data_mut(|data| {
        if let Some(mut pass) = data.get_temp::<Pass>(key()) {
            pass.supported = false;
            data.insert_temp(key(), pass);
        }
    });
}

pub(crate) fn observe(ctx: &egui::Context, response: &egui::Response) {
    if response.clicked() || response.changed() {
        transition();
    }
    if egui::Popup::is_any_open(ctx) || ctx.memory(|memory| memory.top_modal_layer().is_some()) {
        ctx.data_mut(|data| {
            if let Some(mut pass) = data.get_temp::<Pass>(key()) {
                pass.supported = false;
                data.insert_temp(key(), pass);
            }
        });
    }
}

// Direct painters have a separate source-audited contract; see AGENTS.md.
fn areas_supported(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| {
        let visible = memory.areas().visible_layer_ids();
        visible.len() <= 32
            && visible.iter().all(|layer| {
                layer.order == egui::Order::Background
                    || (*layer
                        == egui::LayerId::new(
                            egui::Order::Middle,
                            egui::Id::new("viewport_tool_hint"),
                        ))
            })
    })
}

pub(crate) fn finish(ctx: &egui::Context, guard: Guard, supported: bool) {
    if shared(ctx).is_none() {
        return;
    }
    let neutral = ctx
        .input(|input| super::neutral(input.modifiers, &input.events) && !input.pointer.any_down())
        && !ctx.wants_keyboard_input()
        && areas_supported(ctx);
    ctx.data_mut(|data| {
        if let Some(mut pass) = data.get_temp::<Pass>(key()) {
            let valid = eligible(
                &pass,
                guard,
                supported && neutral && native_clear(),
                EPOCH.load(Ordering::SeqCst),
            );
            if pass.supported && !valid {
                transition();
            }
            pass.supported &= valid;
            pass.finished = true;
            data.insert_temp(key(), pass);
        }
    });
}

fn eligible(pass: &Pass, guard: Guard, environment: bool, epoch: u64) -> bool {
    pass.supported
        && environment
        && pass.guard == guard
        && guard.ready
        && guard.capture_view
        && guard.capture_labels_clear
        && pass.epoch == epoch
        && pass.seen == ALL
}

pub fn snapshot(ctx: &egui::Context, guard: Guard) -> Option<SurfaceSnapshot> {
    if shared(ctx).is_none()
        || !native_clear()
        || ctx.wants_keyboard_input()
        || egui::Popup::is_any_open(ctx)
        || ctx.memory(|memory| memory.top_modal_layer().is_some())
        || !areas_supported(ctx)
    {
        return None;
    }
    let pass = ctx.data(|data| data.get_temp::<Pass>(key()))?;
    (pass.supported
        && pass.finished
        && pass.guard == guard
        && guard.capture_view
        && guard.capture_labels_clear
        && pass.epoch == EPOCH.load(Ordering::SeqCst)
        && pass.number.checked_add(1) == Some(ctx.cumulative_pass_nr()))
    .then_some(SurfaceSnapshot {
        epoch: pass.epoch,
        pass: ctx.cumulative_pass_nr(),
        guard: pass.guard,
    })
}

pub(super) fn retire(ctx: &egui::Context) {
    transition();
    ctx.data_mut(|data| data.remove::<Pass>(key()));
}

#[cfg(test)]
pub(super) fn extend_existing_journey(mut guard: Guard) {
    guard.capture_view = true;
    guard.capture_labels_clear = true;
    let mut pass = Pass {
        number: 1,
        guard,
        epoch: 7,
        seen: ALL,
        supported: true,
        finished: false,
    };
    assert!(
        eligible(&pass, guard, true, 7),
        "complete ordinary empty-scene classification has a positive pure path"
    );
    for producer in 0..8 {
        pass.seen = ALL & !(1 << producer);
        assert!(
            !eligible(&pass, guard, true, 7),
            "every actual producer is required"
        );
    }
    pass.seen = ALL;
    let mut blocked = guard;
    blocked.capture_view = false;
    pass.guard = blocked;
    assert!(!eligible(&pass, blocked, true, 7),
        "blocked current scene access refuses even with an older Viewer role");
    pass.guard = guard;
    assert!(
        !eligible(&pass, guard, false, 7),
        "unknown/native/exclusive environment refuses"
    );
    assert!(
        !eligible(&pass, guard, true, 8),
        "a covered transient cannot be hidden by a return to Scene"
    );
    pass.supported = false;
    assert!(
        !eligible(&pass, guard, true, 7),
        "a blocked producer is sticky through the pass"
    );
    let before = EPOCH.load(Ordering::SeqCst);
    let mut inspector = crate::plugin::InspectorFormState::default();
    inspector.set_active_tab(crate::plugin::InspectorTab::ApiAccess);
    inspector.set_active_tab(crate::plugin::InspectorTab::Properties);
    assert!(
        EPOCH.load(Ordering::SeqCst) > before,
        "real API tab enter AND exit cannot restore old surface identity"
    );
    let before = EPOCH.load(Ordering::SeqCst);
    let mut manager = crate::actions::UiManager::default();
    manager.open_dialog(crate::dialogs::ActiveDialog::JoinDialog {
        invite_buf: String::new(),
    });
    manager.close_dialog();
    assert!(
        EPOCH.load(Ordering::SeqCst) > before,
        "real dialog open/close advances process lifetime state"
    );
    let before = EPOCH.load(Ordering::SeqCst);
    {
        let _dialog = NativeDialog::enter();
    }
    assert!(
        EPOCH.load(Ordering::SeqCst) > before,
        "native dialog lifetime cannot restore a former surface epoch"
    );
}
