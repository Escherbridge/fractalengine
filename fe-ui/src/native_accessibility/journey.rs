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
    hide_confirmation: bool,
    layout_revision: u64,
    positive_surface: bool,
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
                if self.positive_surface {
                    super::surface::begin(ctx, guard);
                    use super::surface::Producer;
                    for producer in [Producer::Topbar, Producer::Status, Producer::Tree,
                        Producer::Inspector, Producer::Results, Producer::Viewport,
                        Producer::Transient, Producer::Data] {
                        super::surface::enter(ctx, producer, true);
                    }
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    if !self.hide_confirmation {
                        ui.push_id(self.layout_revision, |ui| {
                            if crate::panels::widgets::confirm_button(
                                ui,
                                "native-row",
                                "Delete",
                                "Confirm delete",
                                "Delete native row",
                            ) {
                                self.fired += 1;
                            }
                        });
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
                if self.positive_surface {
                    super::surface::finish(ctx, guard, true);
                }
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

    h.click("Delete native row");
    h.layout();
    let previous_pair = h.request("Confirm Delete native row");
    let arm_id = egui::Id::new("confirm_button_armed_at").with("native-row");
    let generation = h
        .ctx
        .data(|d| d.get_temp::<super::Confirmation>(arm_id))
        .unwrap()
        .owner();
    // A new layout changes actual response IDs without changing the product Guard.
    h.layout_revision += 1;
    assert!(stage(&h.ctx, h.snapshot.as_ref().unwrap(), &previous_pair));
    h.frame(
        vec![egui::Event::AccessKitActionRequest(previous_pair)],
        egui::Modifiers::NONE,
    );
    assert_eq!(
        h.fired, 1,
        "changed current pair cannot inherit an arm or confirm"
    );
    assert!(
        h.ctx
            .data(|d| d.get_temp::<super::Confirmation>(arm_id))
            .is_none()
    );
    h.layout();
    h.click("Delete native row");
    h.layout();
    let new_generation = h
        .ctx
        .data(|d| d.get_temp::<super::Confirmation>(arm_id))
        .unwrap()
        .owner();
    assert!(
        generation != new_generation,
        "a new arm has a distinct generation"
    );
    h.hide_confirmation = true;
    h.layout();
    h.hide_confirmation = false;
    h.layout();
    assert!(
        h.ctx
            .data(|d| d.get_temp::<super::Confirmation>(arm_id))
            .is_none(),
        "a disappeared/reappeared helper disarms before rendering its replacement"
    );
    assert_eq!(h.fired, 1);
    h.click("Delete native row");
    h.layout();
    h.click("Cancel Delete native row");
    h.layout();
    assert_eq!(h.fired, 1, "the current Cancel disarms without confirming");

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
    super::surface::extend_existing_journey(product_guard(&h.ctx, &h.ui, &h.nodes, &h.nav,
        &h.tree, &h.role, &h.topbar, &h.modal, &h.path));

    h.hide_confirmation = true;
    h.positive_surface = true;
    h.role.role = Some(fe_database::RoleLevel::Viewer);
    h.layout();
    let rendered = h.snapshot.as_ref().unwrap().guard();
    let surface = super::surface::snapshot(&h.ctx, rendered).unwrap();
    h.nav.navigate_to_petal("native-later-petal");
    let current = product_guard(&h.ctx, &h.ui, &h.nodes, &h.nav,
        &h.tree, &h.role, &h.topbar, &h.modal, &h.path);
    assert!(rendered != current);
    assert!(super::surface::snapshot(&h.ctx, current).is_none(),
        "navigation after a completed output cannot retag its old surface");
    assert!(complete(&h.ctx, h.update.as_ref().unwrap()).unwrap().guard() == rendered,
        "actual completed output keeps its original Guard");
    h.layout();
    let rendered = h.snapshot.as_ref().unwrap().guard();
    assert!(super::surface::snapshot(&h.ctx, rendered).is_some());
    h.nodes.select(bevy::prelude::Entity::from_raw_u32(701).unwrap(), "native-new-primary");
    let current = product_guard(&h.ctx, &h.ui, &h.nodes, &h.nav,
        &h.tree, &h.role, &h.topbar, &h.modal, &h.path);
    assert!(super::surface::snapshot(&h.ctx, current).is_none(),
        "a new primary after annotation cannot retain completed authority");
    assert!(surface != super::surface::snapshot(&h.ctx, rendered).unwrap());
    h.nodes.deselect();
    h.role.role = Some(fe_database::RoleLevel::None);
    h.layout();
    assert!(super::surface::snapshot(&h.ctx, h.snapshot.as_ref().unwrap().guard()).is_none(),
        "resolved No access is not a positive capture role");
    h.role.role = Some(fe_database::RoleLevel::Viewer);
    h.path.editing_track_id = Some("native-label-track".into());
    h.path.points.push(crate::gis::PathPointRow::default());
    h.layout();
    assert!(super::surface::snapshot(&h.ctx, h.snapshot.as_ref().unwrap().guard()).is_none(),
        "late Background path labels remain outside the empty capture profile");

}
