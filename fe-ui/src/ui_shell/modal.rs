//! Panel quarantine and dialog decisions; see ui_shell/AGENTS.md.

use std::collections::HashSet;

use bevy::prelude::Resource;
use bevy_egui::egui;

/// Session-scoped modal-manager state. DEFINED here; `init_resource` registration
/// in `plugin.rs` is deferred to S3b (see report).
#[derive(Resource, Default, Debug, Clone)]
pub struct ModalManagerState {
    /// Panels quarantined after a panic — skipped for the rest of the session.
    pub disabled_panels: HashSet<String>,
    /// Last user-facing guard error, surfaced by S3b as a status-bar segment (Q-5).
    pub last_error: Option<String>,
}

impl ModalManagerState {
    /// Retry one quarantined panel without clearing unrelated failures.
    pub fn clear_quarantine(&mut self, panel: &str) {
        self.disabled_panels.remove(panel);
        self.last_error = self.disabled_panels.iter().min().map(|name| {
            format!("{} disabled after error", humanized_panel_name(name))
        });
    }
}

/// Readable labels for stable quarantine keys.
pub fn humanized_panel_name(panel: &str) -> &str {
    match panel {
        "right.inspector" | "inspector" => "Inspector",
        "right.options" => "Tool options",
        "right.terrain_tools" => "Terrain tools",
        "right.proposal_report" => "Terrain report",
        "right.settings" => "Settings",
        "right.maps" => "Maps",
        "right.groups" => "Groups",
        "portal_toolbar" => "Web page controls",
        "left_sidebar" => "Hierarchy",
        "topbar" => "Toolbar",
        "viewport" => "Scene",
        "gis_panel" => "Data",
        "context_menu" => "Object menu",
        "toast" => "Notifications",
        name if name.starts_with("dialog_") => "Dialog",
        _ => "Panel",
    }
}

/// `&str`/`String` cover every `unwrap`/`expect`/`panic!` payload; anything else
/// yields a placeholder rather than silently dropping the reason. Mirrors
/// `fractalengine/src/panic_log.rs::panic_payload_string`.
///
/// Only reachable from [`guarded_catching`] (release `guarded` arm) or tests —
/// `guarded`'s debug arm re-propagates instead, so a debug/non-test build
/// never calls this; cfg-gated to match, avoiding dead-code in that profile.
#[cfg(any(test, not(debug_assertions)))]
fn panic_payload_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// The release-shaped guard: ALWAYS catches. Skips an already-disabled panel
/// (returns `None` without running `f`); otherwise runs `f` under
/// `catch_unwind`, and on panic disables the panel + records `last_error`. Tests
/// target this directly so the catch path is exercisable without flipping `cfg`.
///
/// `AssertUnwindSafe` is justified: `f` is a generic `FnOnce() -> R` closure, so
/// no raw `&mut` egui borrow escapes its type; the guarded per-panel state is
/// exactly what this function then quarantines.
///
/// Same reachability note as [`panic_payload_string`]: only called from
/// [`guarded`]'s release arm, plus directly by tests.
#[cfg(any(test, not(debug_assertions)))]
fn guarded_catching<R>(
    state: &mut ModalManagerState,
    name: &str,
    f: impl FnOnce() -> R,
) -> Option<R> {
    if state.disabled_panels.contains(name) {
        // Poisoned panel: never re-run.
        return None;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(payload) => {
            let reason = panic_payload_string(&*payload);
            bevy::log::error!("panel '{name}' disabled after panic: {reason}");
            state.disabled_panels.insert(name.to_string());
            state.last_error = Some(format!("{} disabled after error", humanized_panel_name(name)));
            None
        }
    }
}

/// Runs panel closure `f` under the panic guard, returning `Some(r)` on success
/// and `None` if the panel is (or becomes) disabled. See the module doc for the
/// full contract.
///
/// Release: delegates to [`guarded_catching`] (catch + disable + log). Debug
/// (`debug_assertions`): still skips already-disabled panels, but a FRESH panic
/// is re-propagated via `resume_unwind` so developers crash loudly. Both arms are
/// compiled and warning-clean in each profile.
pub fn guarded<R>(state: &mut ModalManagerState, name: &str, f: impl FnOnce() -> R) -> Option<R> {
    #[cfg(not(debug_assertions))]
    {
        guarded_catching(state, name, f)
    }
    #[cfg(debug_assertions)]
    {
        if state.disabled_panels.contains(name) {
            return None;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(r) => Some(r),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// A transient overlay layer, in the shell's fixed back-to-front paint order.
/// See [`transient_order`]. Consumed by S3b's `render_transient_layer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransientLayer {
    /// The mutually-exclusive `ActiveDialog` (at most one modal dialog painted).
    Dialog,
    /// The right-click context menu (a distinguished dialog-family overlay).
    ContextMenu,
    /// The toast/notification overlay — painted on top, INDEPENDENT of dialogs.
    Toast,
}

/// The transient layer's fixed paint order, back (index 0) to front (last): the
/// dialog paints first, then the context menu, then the toast overlay on top so
/// notifications stay visible over any modal content. Pure/testable now; S3b
/// consumes it to sequence `render_transient_layer`.
#[must_use]
pub const fn transient_order() -> [TransientLayer; 3] {
    [
        TransientLayer::Dialog,
        TransientLayer::ContextMenu,
        TransientLayer::Toast,
    ]
}

/// Whether `layer` belongs to the dialog family (a modal dialog OR the context
/// menu) — the family that is mutually exclusive. The toast is not a member.
#[must_use]
pub const fn is_dialog_family(layer: TransientLayer) -> bool {
    matches!(layer, TransientLayer::Dialog | TransientLayer::ContextMenu)
}

/// What S3b wants visible in the transient layer this frame. `dialog` is already
/// mutually-exclusive by construction (`ActiveDialog` is a single value);
/// `context_menu` is the distinguished dialog-family overlay; `toast` is
/// INDEPENDENT of dialog state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransientVisibility {
    pub dialog: bool,
    pub context_menu: bool,
    pub toast: bool,
}

impl TransientVisibility {
    /// Enforces at-most-one dialog-family overlay: if both a modal dialog and the
    /// context menu are requested, the explicit dialog wins and the context menu
    /// is suppressed. `toast` passes through unchanged — it overlays
    /// independently of dialog state (Q-5's status-bar error path does not go
    /// through here). The result always satisfies [`Self::dialog_family_exclusive`].
    #[must_use]
    pub const fn resolve_exclusive(self) -> Self {
        Self {
            dialog: self.dialog,
            context_menu: self.context_menu && !self.dialog,
            toast: self.toast,
        }
    }

    /// True iff at most one dialog-family overlay (dialog OR context menu) is
    /// visible — the transient-layer exclusivity invariant.
    #[must_use]
    pub const fn dialog_family_exclusive(self) -> bool {
        !(self.dialog && self.context_menu)
    }
}

#[derive(Clone)]
struct ConfirmationSession {
    target: String,
    generation: u64,
    seen_frame: u64,
}

/// A confirmation scope bound to one target and one continuous dialog lifetime.
pub(crate) fn confirmation_scope(ctx: &egui::Context, dialog: &str, target: &str) -> egui::Id {
    let key = egui::Id::new(("dialog_confirmation_session", dialog));
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| {
        let previous = data.get_temp::<ConfirmationSession>(key);
        let new_lifetime = previous.as_ref().is_none_or(|state| {
            state.target != target || frame.saturating_sub(state.seen_frame) > 1
        });
        let mut state = if new_lifetime {
            let generation_key = key.with("generation");
            let generation = data
                .get_temp::<u64>(generation_key)
                .unwrap_or(0)
                .wrapping_add(1);
            data.insert_temp(generation_key, generation);
            ConfirmationSession {
                target: target.to_owned(),
                generation,
                seen_frame: frame,
            }
        } else {
            previous.unwrap()
        };
        state.seen_frame = frame;
        let scope = key.with((&state.target, state.generation));
        data.insert_temp(key, state);
        scope
    })
}

/// Forget a closed dialog's confirmation lifetime.
pub(crate) fn close_confirmation_scope(ctx: &egui::Context, dialog: &str) {
    ctx.data_mut(|data| {
        data.remove::<ConfirmationSession>(egui::Id::new(("dialog_confirmation_session", dialog)))
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirtyClose {
    KeepOpen,
    AskDiscard,
    Close,
}

/// Both explicit Close and window X use the same dirty-draft decision.
pub(crate) fn dirty_close(dirty: bool, requested: bool, discard_confirmed: bool) -> DirtyClose {
    if discard_confirmed || (requested && !dirty) {
        DirtyClose::Close
    } else if requested {
        DirtyClose::AskDiscard
    } else {
        DirtyClose::KeepOpen
    }
}

/// Pending, leaf and counted cascade confirmation copy.
#[must_use]
pub fn cascade_confirm_message(descendant_count: Option<usize>) -> String {
    match descendant_count {
        None => "Counting children\u{2026}".to_string(),
        Some(0) => "Delete this node? This cannot be undone.".to_string(),
        Some(1) => "Delete this node and its 1 child node? This cannot be undone.".to_string(),
        Some(n) => format!("Delete this node and its {n} child nodes? This cannot be undone."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: the panicking tests below still run the process panic hook (which
    // prints a panic line to stderr) BEFORE `catch_unwind` returns `Err`; that
    // stderr line is cosmetic — the panic is caught, the test passes. The global
    // hook is intentionally NOT swapped, to avoid racing parallel tests.

    #[test]
    fn guarded_catching_disables_panel_on_panic() {
        let mut state = ModalManagerState::default();
        let r: Option<()> = guarded_catching(&mut state, "inspector", || panic!("boom"));
        assert!(r.is_none());
        assert!(state.disabled_panels.contains("inspector"));
        assert_eq!(
            state.last_error.as_deref(),
            Some("Inspector disabled after error")
        );
    }

    #[test]
    fn guarded_catching_does_not_rerun_poisoned_panel() {
        let mut state = ModalManagerState::default();
        // First call panics and disables the panel.
        let _: Option<()> = guarded_catching(&mut state, "p", || panic!("boom"));
        // Second call: the panel is now disabled; `f` MUST be skipped.
        let mut invoked = false;
        let r = guarded_catching(&mut state, "p", || {
            invoked = true;
            1
        });
        assert!(r.is_none());
        assert!(!invoked, "poisoned panel must not re-run f");
        state.disabled_panels.insert("right.settings".into());
        state.clear_quarantine("p");
        assert_eq!(guarded_catching(&mut state, "p", || 7), Some(7));
        assert!(state.disabled_panels.contains("right.settings"));
        assert_eq!(state.last_error.as_deref(), Some("Settings disabled after error"));
        state.clear_quarantine("right.settings");
        assert!(state.disabled_panels.is_empty());
        assert!(state.last_error.is_none());
    }

    #[test]
    fn guarded_catching_skips_already_disabled_panel_without_invoking_f() {
        let mut state = ModalManagerState::default();
        state.disabled_panels.insert("inspector".to_string());
        let mut invoked = false;
        let r = guarded_catching(&mut state, "inspector", || {
            invoked = true;
            7
        });
        assert!(r.is_none());
        assert!(!invoked, "f must not run for an already-disabled panel");
    }

    #[test]
    fn guarded_catching_happy_path_returns_value_without_disabling() {
        let mut state = ModalManagerState::default();
        let r = guarded_catching(&mut state, "inspector", || 42);
        assert_eq!(r, Some(42));
        assert!(state.disabled_panels.is_empty());
        assert!(state.last_error.is_none());
    }

    // `guarded` (the public entry) is only tested on profile-INDEPENDENT paths:
    // the happy path (both profiles return `Some`) and the disabled-skip path
    // (both profiles return `None` without running `f`). Its debug arm
    // re-propagates, so a panicking `f` through `guarded` is deliberately NOT
    // tested here (it would abort/fail depending on profile).

    #[test]
    fn guarded_happy_path_is_profile_independent() {
        let mut state = ModalManagerState::default();
        assert_eq!(guarded(&mut state, "p", || 5), Some(5));
        assert!(state.disabled_panels.is_empty());
        assert!(state.last_error.is_none());
    }

    #[test]
    fn guarded_skips_disabled_panel_in_both_profiles() {
        let mut state = ModalManagerState::default();
        state.disabled_panels.insert("p".to_string());
        let mut invoked = false;
        let r = guarded(&mut state, "p", || {
            invoked = true;
            1
        });
        assert!(r.is_none());
        assert!(!invoked);
    }

    #[test]
    fn panic_payload_string_extracts_common_payloads() {
        let s: Box<dyn std::any::Any + Send> = Box::new("boom &str");
        assert_eq!(panic_payload_string(&*s), "boom &str");
        let s: Box<dyn std::any::Any + Send> = Box::new(String::from("boom String"));
        assert_eq!(panic_payload_string(&*s), "boom String");
        let s: Box<dyn std::any::Any + Send> = Box::new(42i32);
        assert_eq!(panic_payload_string(&*s), "<non-string panic payload>");
    }

    #[test]
    fn transient_order_is_dialog_then_context_menu_then_toast() {
        assert_eq!(
            transient_order(),
            [
                TransientLayer::Dialog,
                TransientLayer::ContextMenu,
                TransientLayer::Toast,
            ]
        );
    }

    #[test]
    fn dialog_and_context_menu_are_dialog_family_toast_is_not() {
        assert!(is_dialog_family(TransientLayer::Dialog));
        assert!(is_dialog_family(TransientLayer::ContextMenu));
        assert!(!is_dialog_family(TransientLayer::Toast));
    }

    #[test]
    fn resolve_exclusive_allows_at_most_one_dialog_family() {
        // Dialog + context menu both requested -> dialog wins, menu suppressed.
        let v = TransientVisibility {
            dialog: true,
            context_menu: true,
            toast: false,
        }
        .resolve_exclusive();
        assert!(v.dialog);
        assert!(!v.context_menu);
        assert!(v.dialog_family_exclusive());
    }

    #[test]
    fn resolve_exclusive_keeps_toast_independent_of_dialog() {
        let with_dialog = TransientVisibility {
            dialog: true,
            context_menu: false,
            toast: true,
        }
        .resolve_exclusive();
        assert!(
            with_dialog.toast,
            "toast overlays independently of a dialog"
        );
        let without_dialog = TransientVisibility {
            dialog: false,
            context_menu: false,
            toast: true,
        }
        .resolve_exclusive();
        assert!(without_dialog.toast);
    }

    #[test]
    fn context_menu_alone_is_allowed() {
        let v = TransientVisibility {
            dialog: false,
            context_menu: true,
            toast: false,
        }
        .resolve_exclusive();
        assert!(v.context_menu);
        assert!(v.dialog_family_exclusive());
    }

    #[test]
    fn cascade_confirm_message_is_count_aware_and_never_silent() {
        assert_eq!(dirty_close(true, true, false), DirtyClose::AskDiscard);
        assert_eq!(dirty_close(true, false, false), DirtyClose::KeepOpen);
        assert_eq!(dirty_close(true, false, true), DirtyClose::Close);
        assert_eq!(dirty_close(false, true, false), DirtyClose::Close);
        let ctx = egui::Context::default();
        let node_a = confirmation_scope(&ctx, "node_options", "a");
        assert_eq!(node_a, confirmation_scope(&ctx, "node_options", "a"));
        assert_ne!(node_a, confirmation_scope(&ctx, "node_options", "b"));
        let reopened = confirmation_scope(&ctx, "node_options", "a");
        assert_ne!(
            node_a, reopened,
            "returning to a prior target cannot reuse an armed confirm"
        );
        close_confirmation_scope(&ctx, "node_options");
        assert_ne!(reopened, confirmation_scope(&ctx, "node_options", "a"));
        assert_ne!(node_a, confirmation_scope(&ctx, "manifest", "a"));

        assert_eq!(cascade_confirm_message(None), "Counting children\u{2026}");
        assert_eq!(
            cascade_confirm_message(Some(0)),
            "Delete this node? This cannot be undone."
        );
        assert_eq!(
            cascade_confirm_message(Some(1)),
            "Delete this node and its 1 child node? This cannot be undone."
        );
        assert_eq!(
            cascade_confirm_message(Some(3)),
            "Delete this node and its 3 child nodes? This cannot be undone."
        );
        // Every variant warns it cannot be undone / removes children.
        for n in [0usize, 1, 2, 7] {
            let m = cascade_confirm_message(Some(n));
            assert!(m.contains("cannot be undone") || m.contains("removes any children"));
        }
    }
}
