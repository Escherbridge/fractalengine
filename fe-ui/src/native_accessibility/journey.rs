//! Extends the existing confirmation journey with the ordinary adapter seam.

use super::{Snapshot, begin_pass, complete, finish_pass, product_guard, stage};
use crate::accessibility::ResponseExt;
use bevy_egui::egui::{self, accesskit as ak};

#[derive(Default)]
struct Harness {
    ctx: egui::Context,
    ui: crate::actions::UiManager,
    nodes: crate::node_manager::NodeManager,
    nav: crate::navigation_manager::NavigationManager,
    tree: crate::verse_manager::VerseManager,
    role: crate::plugin::LocalUserRole,
    topbar: crate::ui_shell::topbar::TopbarState,
    modal: crate::ui_shell::modal::ModalManagerState,
    path: crate::gis::PathEditorState,
    time: f64,
    fired: usize,
    other: usize,
    copies: usize,
    snapshot: Option<Snapshot>,
    update: Option<ak::TreeUpdate>,
    hide_other: bool,
    disable_other: bool,
    duplicate: bool,
}

impl Harness {
    fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        let ctx = self.ctx.clone();
        self.time += 0.01;
        let guard = product_guard(
            &ctx,
            &self.ui,
            &self.nodes,
            &self.nav,
            &self.tree,
            &self.role,
            &self.topbar,
            &self.modal,
            &self.path,
        );
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 400.0),
                )),
                time: Some(self.time),
                events,
                modifiers,
                ..Default::default()
            },
            |ctx| {
                begin_pass(ctx, guard);
                egui::CentralPanel::default().show(ctx, |ui| {
                    if crate::panels::widgets::confirm_button(
                        ui,
                        "native-row",
                        "Delete",
                        "Confirm delete",
                        "Delete native row",
                    ) {
                        self.fired += 1;
                    }
                    if !self.hide_other {
                        let other = ui
                            .add_enabled(!self.disable_other, egui::Button::new("Other"))
                            .named(ui, "Native other control");
                        if self.duplicate {
                            super::record(ui, &other);
                        }
                        if other.clicked() && super::selection_allowed(ctx, &other) {
                            self.other += 1;
                            ctx.copy_text("ordinary native copy".into());
                        }
                    }
                });
                finish_pass(ctx, guard);
            },
        );
        self.copies += output.platform_output.commands.iter().filter(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "ordinary native copy")).count();
        self.update = output.platform_output.accesskit_update;
        self.snapshot = self
            .update
            .as_ref()
            .and_then(|update| complete(&ctx, update));
    }

    fn request(&self, label: &str) -> ak::ActionRequest {
        let target = self
            .update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find_map(|(id, node)| (node.label() == Some(label)).then_some(*id))
            .unwrap();
        ak::ActionRequest {
            action: ak::Action::Click,
            target,
            data: None,
        }
    }

    fn click(&mut self, label: &str) {
        let request = self.request(label);
        assert!(stage(
            &self.ctx,
            self.snapshot.as_ref().expect("fresh completed pass"),
            &request
        ));
        self.frame(
            vec![egui::Event::AccessKitActionRequest(request)],
            egui::Modifiers::NONE,
        );
    }

    fn layout(&mut self) {
        self.frame(Vec::new(), egui::Modifiers::NONE);
    }
}

pub(crate) fn extend_confirmation_journey() {
    let mut h = Harness::default();
    h.ctx.enable_accesskit();
    super::activate(&h.ctx);
    h.layout();
    h.click("Delete native row");
    assert_eq!(h.fired, 0);
    h.layout();
    let confirm = h.request("Confirm Delete native row");
    h.click("Native other control");
    assert_eq!(h.other, 1);
    assert_eq!(
        h.copies, 1,
        "normal native activation keeps ordinary egui clipboard semantics"
    );
    assert_eq!(
        h.fired, 0,
        "native outside activation disarms before confirmation evaluation"
    );
    if h.snapshot.as_ref().unwrap().allows(&confirm) {
        assert!(stage(&h.ctx, h.snapshot.as_ref().unwrap(), &confirm));
        h.frame(
            vec![egui::Event::AccessKitActionRequest(confirm)],
            egui::Modifiers::NONE,
        );
    } else {
        h.frame(
            vec![egui::Event::AccessKitActionRequest(confirm)],
            egui::Modifiers::NONE,
        );
    }
    assert_eq!(
        h.fired, 0,
        "a stale Confirm never commits after another native control"
    );
    h.layout();
    if h.update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .any(|(_, node)| node.label() == Some("Cancel Delete native row"))
    {
        h.click("Cancel Delete native row");
        h.layout();
    }
    h.click("Delete native row");
    h.layout();
    h.click("Confirm Delete native row");
    assert_eq!(
        h.fired, 1,
        "the ordinary current two-step path still confirms once"
    );
    h.layout();

    let request = h.request("Native other control");
    assert!(stage(&h.ctx, h.snapshot.as_ref().unwrap(), &request));
    h.ui.active_dialog = crate::dialogs::ActiveDialog::JoinDialog {
        invite_buf: String::new(),
    };
    h.frame(
        vec![egui::Event::AccessKitActionRequest(request)],
        egui::Modifiers::NONE,
    );
    assert_eq!(
        h.other, 1,
        "a modal opened after staging still blocks background native activation at actual UI use"
    );
    assert!(h.snapshot.is_none());
    h.ui.active_dialog = crate::dialogs::ActiveDialog::None;
    h.layout();
    let request = h.request("Native other control");
    assert!(stage(&h.ctx, h.snapshot.as_ref().unwrap(), &request));
    h.nodes.gesture_active = true;
    h.frame(
        vec![egui::Event::AccessKitActionRequest(request)],
        egui::Modifiers::NONE,
    );
    assert_eq!(
        h.other, 1,
        "a tool/selection gesture started after staging still refuses native activation"
    );
    h.nodes.gesture_active = false;
    h.layout();

    for modifiers in [
        egui::Modifiers {
            ctrl: true,
            ..egui::Modifiers::NONE
        },
        egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        },
        egui::Modifiers {
            alt: true,
            ..egui::Modifiers::NONE
        },
        egui::Modifiers {
            command: true,
            ..egui::Modifiers::NONE
        },
        egui::Modifiers {
            mac_cmd: true,
            ..egui::Modifiers::NONE
        },
    ] {
        let request = h.request("Native other control");
        assert!(stage(&h.ctx, h.snapshot.as_ref().unwrap(), &request));
        h.frame(
            vec![egui::Event::AccessKitActionRequest(request)],
            modifiers,
        );
        assert_eq!(
            h.other, 1,
            "held modifiers cannot convert native single selection into Toggle/Range/options"
        );
        assert_eq!(
            h.ctx.input(|input| input.modifiers),
            modifiers,
            "physical modifiers are never rewritten"
        );
        h.layout();
    }
    let request = h.request("Native other control");
    let mut focus = request.clone();
    focus.action = ak::Action::Focus;
    assert!(
        !h.snapshot.as_ref().unwrap().allows(&focus),
        "Focus runs too early in egui and is explicitly unsupported"
    );
    let mut value = request.clone();
    value.data = Some(ak::ActionData::Value("not logged or forwarded".into()));
    assert!(!h.snapshot.as_ref().unwrap().allows(&value));
    let old = h.snapshot.take().unwrap();
    h.hide_other = true;
    h.layout();
    assert!(
        !h.snapshot.as_ref().unwrap().allows(&request),
        "full snapshots replace removed IDs rather than unioning them"
    );
    assert!(
        !stage(&h.ctx, &old, &request),
        "an old pass cannot stage even within the same context lifetime"
    );
    h.hide_other = false;
    h.disable_other = true;
    h.layout();
    assert!(
        !h.snapshot
            .as_ref()
            .unwrap()
            .allows(&h.request("Native other control"))
    );
    h.disable_other = false;
    h.duplicate = true;
    h.layout();
    assert!(
        h.snapshot.is_none(),
        "ambiguous response registration invalidates the whole pass"
    );
    h.duplicate = false;
    h.layout();
    let mut oversized = h.update.clone().unwrap();
    oversized.nodes.resize_with(super::MAX_NODES + 1, || {
        (ak::NodeId(9999), ak::Node::new(ak::Role::Button))
    });
    assert!(complete(&h.ctx, &oversized).is_none());
    let snapshot = h.snapshot.take().unwrap();
    super::retire(&h.ctx);
    super::activate(&h.ctx);
    assert!(
        !stage(&h.ctx, &snapshot, &request),
        "context read-model retirement cannot reuse former admissions"
    );
}
