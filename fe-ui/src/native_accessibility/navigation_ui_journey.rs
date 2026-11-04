use super::*;
use crate::{accessibility::ResponseExt, verse_manager::{FractalEntry, PetalEntry, VerseEntry}};

struct Harness {
    ctx: egui::Context,
    reads: NavigationReads,
    ui: crate::actions::UiManager,
    nodes: crate::node_manager::NodeManager,
    nav: crate::navigation_manager::NavigationManager,
    tree: crate::verse_manager::VerseManager,
    role: crate::plugin::LocalUserRole,
    topbar: crate::ui_shell::topbar::TopbarState,
    modal: crate::ui_shell::modal::ModalManagerState,
    path: crate::gis::PathEditorState,
    sender: crossbeam::channel::Sender<DbCommand>,
    receiver: crossbeam::channel::Receiver<DbCommand>,
    target: fe_runtime::native_navigation::PetalTarget,
    snapshot: Option<super::super::Snapshot>,
    original: Option<ak::TreeUpdate>,
    mapped: Option<ak::TreeUpdate>,
    physical: Option<ak::NodeId>,
    physical_rect: Option<egui::Rect>,
    physical_clicks: usize,
    ordinary_clicks: usize,
    callbacks_created: usize,
    callbacks_used: usize,
    rename_before_use: Option<u8>,
    hold_callback: bool,
    restore_callback: bool,
    held_callback: Option<Callback>,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let mut harness = Self::unresolved();
        harness.resolve_role("viewer");
        harness
    }

    fn unresolved() -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        super::super::activate(&ctx);
        let reads = NavigationReads::default();
        attach(&ctx, &reads);
        let mut nav = crate::navigation_manager::NavigationManager::default();
        nav.navigate_to_fractal_in_verse("nav-ui-verse", "Verse", "nav-ui-fractal", "Fractal");
        let tree = crate::verse_manager::VerseManager::from_verses(vec![VerseEntry {
            id: "nav-ui-verse".into(), name: "Verse".into(), expanded: true,
            fractals: vec![FractalEntry {
                id: "nav-ui-fractal".into(), name: "Fractal".into(), expanded: true,
                petals: vec![PetalEntry {
                    id: "nav-ui-petal".into(), name: "Petal".into(), ..Default::default()
                }],
            }], ..Default::default()
        }]);
        let (sender, receiver) = crossbeam::channel::bounded(4);
        let role = crate::plugin::LocalUserRole::default();
        Self {
            ctx, reads, ui: Default::default(), nodes: Default::default(), nav, tree, role,
            topbar: Default::default(), modal: Default::default(), path: Default::default(),
            sender, receiver,
            target: fe_runtime::native_navigation::PetalTarget {
                verse_id: "nav-ui-verse".into(), fractal_id: "nav-ui-fractal".into(),
                petal_id: "nav-ui-petal".into(), name: "Petal".into(),
            },
            snapshot: None, original: None, mapped: None, physical: None, physical_rect: None,
            physical_clicks: 0, ordinary_clicks: 0,
            callbacks_created: 0, callbacks_used: 0, rename_before_use: None, time: 0.0,
            hold_callback: false, restore_callback: false, held_callback: None,
        }
    }

    fn guard(&self) -> Guard {
        super::super::product_guard(&self.ctx, &self.ui, &self.nodes, &self.nav,
            &self.tree, &self.role, &self.topbar, &self.modal, &self.path)
    }

    fn resolve_role(&mut self, role: &str) {
        self.apply_role_result(DbResult::LocalRoleResolved {
            scope: self.nav.active_scope().unwrap(), role: role.into(),
        });
    }

    fn apply_role_result(&mut self, result: DbResult) {
        crate::verse_manager::db_results::apply_local_role_result(&result, &mut self.tree,
            &self.nav, &mut self.role, &mut self.path);
    }

    fn frame(&mut self, events: Vec<egui::Event>, retire_before_use: bool) {
        let ctx = self.ctx.clone();
        self.time += 0.01;
        let output = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 300.0))),
            time: Some(self.time), events, ..Default::default()
        }, |ctx| {
            let guard = self.guard();
            super::super::begin_pass(ctx, guard);
            super::super::surface::begin(ctx, guard);
            let _ = self.guard();
            use super::super::surface::{self, Producer};
            for producer in [Producer::Topbar, Producer::Status, Producer::Tree, Producer::Inspector,
                Producer::Results, Producer::Viewport, Producer::Transient, Producer::Data] {
                surface::enter(ctx, producer, true);
            }
            begin(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.button("Petal").named(ui, "Open petal Petal");
                self.physical = Some(ak::NodeId(response.id.value()));
                self.physical_rect = Some(response.interact_rect);
                record(ui, &response, self.target.clone());
                if response.clicked()
                    && !defer_if_native(ctx, &response, self.target.clone(), "Verse", "Fractal") {
                    self.physical_clicks += 1;
                    self.nav.navigate_to_petal_in_fractal(&self.target.verse_id, "Verse",
                        &self.target.fractal_id, "Fractal", &self.target.petal_id);
                }
                let ordinary = ui.button("Other").named(ui, "Native unrelated control");
                if ordinary.clicked() && super::super::selection_allowed(ctx, &ordinary) {
                    self.ordinary_clicks += 1;
                }
            });
            if self.reads.session.try_lock().unwrap().callback.is_some() {
                self.callbacks_created += 1;
            }
            if self.hold_callback {
                self.held_callback = self.reads.session.try_lock().unwrap().callback.take();
                self.hold_callback = false;
            }
            if self.restore_callback {
                self.reads.session.try_lock().unwrap().callback = self.held_callback.take();
                self.restore_callback = false;
            }
            if retire_before_use {
                self.reads.session.try_lock().unwrap().owner.retire();
            }
            match self.rename_before_use {
                Some(0) => self.tree.verses[0].name.push_str(" changed"),
                Some(1) => self.tree.verses[0].fractals[0].name.push_str(" changed"),
                Some(2) => self.tree.verses[0].fractals[0].petals[0].name.push_str(" changed"),
                _ => {}
            }
            let _ = self.guard();
            if let Some(callback) = take_callback(ctx, &self.tree) {
                self.callbacks_used += 1;
                self.nodes.deselect();
                self.nodes.pending_sidebar_select = None;
                self.nav.navigate_to_petal_in_fractal(&callback.target.verse_id, &callback.verse_name,
                    &callback.target.fractal_id, &callback.fractal_name, &callback.target.petal_id);
            }
            let guard = self.guard();
            finish(ctx, &self.sender);
            surface::finish(ctx, guard, true);
            super::super::finish_pass(ctx, guard);
        });
        self.original = output.platform_output.accesskit_update;
        self.mapped = self.original.as_ref().and_then(|update| remap(&ctx, update));
        self.snapshot = self.mapped.as_ref().and_then(|update| super::super::complete(&ctx, update));
    }

    fn reply(&mut self) {
        let command = self.receiver.try_recv().expect("exact bounded read was submitted");
        let DbCommand::ReadNativePetalView { request_id, target } = command else {
            panic!("native navigation emitted another command family");
        };
        assert!(target == self.target);
        let result = DbResult::NativePetalViewRead {
            request_id, target: target.clone(),
            outcome: Ok(fe_runtime::native_navigation::PetalView {
                target, source_fence: "a".repeat(64),
            }),
        };
        let mut state = self.reads.session.try_lock().unwrap();
        let source = state.source.clone().unwrap();
        assert_eq!(state.owner.receive(&result, &source, Instant::now()),
            super::super::navigation::Reception::Admitted);
        publish_epoch(&mut state);
    }

    fn prepared() -> Self {
        let mut h = Self::new();
        h.frame(Vec::new(), false);
        assert!(eligible(&h.ctx));
        h.reply();
        h.frame(Vec::new(), false);
        assert!(h.snapshot.is_some());
        h
    }

    fn request(&self) -> ak::ActionRequest {
        let target = self.mapped.as_ref().unwrap().nodes.iter()
            .find_map(|(id, node)| (node.label() == Some("Open petal Petal")).then_some(*id)).unwrap();
        ak::ActionRequest { action: ak::Action::Click, target, data: None }
    }

    fn renew(&mut self) {
        let mut state = self.reads.session.try_lock().unwrap();
        let source = state.source.clone().unwrap();
        let targets = state.rendered.clone();
        state.owner.retire();
        assert!(state.owner.observe(&source, &targets, Instant::now()));
        assert_eq!(state.owner.submit(&source, Instant::now(), &self.sender),
            super::super::navigation::Submission::Queued);
        drop(state);
        self.reply();
    }

    fn stage(&self, request: &ak::ActionRequest) -> (Binding, ak::ActionRequest) {
        let snapshot = self.snapshot.as_ref().unwrap();
        let binding = snapshot.navigation(request.target).unwrap();
        assert!(super::super::stage_bound(&self.ctx, snapshot, request, Some(binding.clone())));
        let pass = super::super::shared(&self.ctx).unwrap();
        let pass = pass.try_lock().unwrap();
        let target = pass.delivery.as_ref().unwrap().target;
        assert_eq!(target, binding.physical());
        assert_ne!(target, request.target);
        let translated = binding.translate(&self.ctx, ak::ActionRequest {
            action: request.action, target: request.target, data: None }).unwrap();
        assert_eq!(translated.target, target, "production host translator matches the staged real response");
        (binding, translated)
    }
}

pub(super) fn extend_existing_journey() {
    extend_role_bootstrap_journey();
    let mut h = Harness::prepared();
    let request = h.request();
    let physical = h.physical.unwrap();
    assert_ne!(request.target, physical);
    assert_eq!(h.original.as_ref().unwrap().nodes.iter()
        .find_map(|(id, node)| (node.label() == Some("Open petal Petal")).then_some(*id)), Some(physical));
    let (_, input) = h.stage(&request);
    h.frame(vec![egui::Event::AccessKitActionRequest(input)], false);
    assert_eq!(h.physical, Some(physical), "real widget identity survives alias delivery");
    assert_eq!(h.callbacks_created, 1);
    assert_eq!(h.callbacks_used, 1);
    assert_eq!(h.nav.active_petal_id.as_deref(), Some("nav-ui-petal"));

    let mut h = Harness::prepared();
    let mut direct = h.request();
    direct.target = h.physical.unwrap();
    assert!(!h.snapshot.as_ref().unwrap().allows(&direct));
    assert!(!super::super::stage_bound(&h.ctx, h.snapshot.as_ref().unwrap(), &direct, None));
    h.frame(vec![egui::Event::AccessKitActionRequest(direct)], false);
    assert_eq!(h.callbacks_created, 0);
    assert_eq!(h.callbacks_used, 0);
    assert!(h.nav.active_petal_id.is_none(), "direct physical native ID cannot bypass the typed path");

    let mut h = Harness::prepared();
    let old_request = h.request();
    let old_binding = h.snapshot.as_ref().unwrap().navigation(old_request.target).unwrap();
    h.renew();
    h.frame(Vec::new(), false);
    let new_request = h.request();
    assert_ne!(old_request.target, new_request.target);
    assert!(!is_current(&h.ctx, &old_binding));
    assert!(!h.snapshot.as_ref().unwrap().allows(&old_request));
    assert!(!super::super::stage_bound(&h.ctx, h.snapshot.as_ref().unwrap(), &new_request,
        Some(old_binding)));
    assert!(super::super::provider_identity::was_issued(old_request.target));
    let (_, input) = h.stage(&new_request);
    h.frame(vec![egui::Event::AccessKitActionRequest(input)], false);
    assert_eq!(h.callbacks_used, 1, "replacement proof works only with its own provider ID");

    let mut h = Harness::prepared();
    let request = h.request();
    let (old_binding, input) = h.stage(&request);
    h.renew();
    assert!(!is_current(&h.ctx, &old_binding));
    h.frame(vec![egui::Event::AccessKitActionRequest(input)], false);
    assert_eq!(h.callbacks_created, 0, "begin_pass rechecks the retained original binding");
    assert_eq!(h.callbacks_used, 0);
    assert!(h.nav.active_petal_id.is_none());

    let mut h = Harness::prepared();
    let request = h.request();
    let (_, input) = h.stage(&request);
    h.frame(vec![egui::Event::AccessKitActionRequest(input)], true);
    assert_eq!(h.callbacks_created, 1, "callback was deferred before retirement");
    assert_eq!(h.callbacks_used, 0, "final callback refuses the retired original proof");
    assert!(h.nav.active_petal_id.is_none());

    for ancestor in 0..3 {
        let mut h = Harness::prepared();
        let request = h.request();
        let (_, input) = h.stage(&request);
        h.rename_before_use = Some(ancestor);
        h.frame(vec![egui::Event::AccessKitActionRequest(input)], false);
        assert_eq!(h.callbacks_created, 1);
        assert_eq!(h.callbacks_used, 0, "callback requires current Verse/Fractal/Petal names");
        assert!(h.nav.active_petal_id.is_none());
    }

    let mut h = Harness::prepared();
    let physical = h.physical.unwrap();
    let old_binding = h.snapshot.as_ref().unwrap().navigation(h.request().target).unwrap();
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false,
        modifiers: egui::Modifiers::NONE,
    }], false);
    assert_eq!(h.ctx.memory(|memory| memory.focused().map(|id| id.value())), Some(physical.0));
    let center = h.physical_rect.unwrap().center();
    h.frame(vec![egui::Event::PointerMoved(center), egui::Event::PointerButton {
        pos: center, button: egui::PointerButton::Primary, pressed: true,
        modifiers: egui::Modifiers::NONE,
    }], false);
    assert!(h.ctx.input(|input| input.pointer.primary_down()));
    assert_eq!(h.physical_clicks, 0);
    assert_eq!(h.physical, Some(physical));
    let _ = h.guard();
    h.renew();
    assert!(epoch(&h.ctx).is_some(), "the exact background target received a genuinely new proof");
    assert!(!is_current(&h.ctx, &old_binding));
    assert_eq!(h.ctx.memory(|memory| memory.focused().map(|id| id.value())), Some(physical.0));
    h.frame(Vec::new(), false);
    assert!(h.ctx.input(|input| input.pointer.primary_down()));
    assert_eq!(h.physical, Some(physical));
    assert_eq!(h.physical_clicks, 0);
    assert_eq!(h.ctx.memory(|memory| memory.focused().map(|id| id.value())), Some(physical.0));
    h.frame(vec![egui::Event::PointerButton {
        pos: center, button: egui::PointerButton::Primary, pressed: false,
        modifiers: egui::Modifiers::NONE,
    }], false);
    assert!(!h.ctx.input(|input| input.pointer.primary_down()));
    assert_eq!(h.physical, Some(physical));
    assert_eq!(h.physical_clicks, 1, "proof renewal cannot lose a held ordinary pointer release");
    assert_eq!(h.callbacks_used, 0, "ordinary physical navigation needs no native proof callback");
    assert_eq!(h.nav.active_petal_id.as_deref(), Some("nav-ui-petal"));
    assert_eq!(h.ctx.memory(|memory| memory.focused().map(|id| id.value())), Some(physical.0));

    let mut h = Harness::prepared();
    let ordinary = ak::ActionRequest {
        action: ak::Action::Click, data: None,
        target: h.mapped.as_ref().unwrap().nodes.iter().find_map(|(id, node)| {
            (node.label() == Some("Native unrelated control")).then_some(*id)
        }).unwrap(),
    };
    let before = h.snapshot.as_ref().unwrap().guard();
    assert!(before.navigation_proof.is_some());
    assert!(h.snapshot.as_ref().unwrap().navigation(ordinary.target).is_none());
    assert!(super::super::stage_bound(&h.ctx, h.snapshot.as_ref().unwrap(), &ordinary, None));
    {
        let mut state = h.reads.session.try_lock().unwrap();
        state.owner.retire();
        publish_epoch(&mut state);
    }
    let after = h.guard();
    assert!(after.navigation_proof.is_none());
    assert!(before != after && before.same_base(after));
    h.frame(vec![egui::Event::AccessKitActionRequest(ordinary)], false);
    assert_eq!(h.ordinary_clicks, 1, "background navigation retirement preserves an unrelated A1 action");
    assert_eq!(h.callbacks_used, 0);
    assert!(h.nav.active_petal_id.is_none());
    extend_provider_recovery_journey();
}

fn extend_role_bootstrap_journey() {
    for (verse_only, role) in [(true, "viewer"), (false, "viewer"), (true, "editor")] {
        let mut h = Harness::unresolved();
        if verse_only { h.nav.navigate_to_verse("nav-ui-verse", "Verse"); }
        assert!(h.tree.scene_access_generation().is_none());
        assert!(h.role.role.is_none());
        assert!(h.guard().access.is_none());
        h.frame(Vec::new(), false);
        assert!(!eligible(&h.ctx));
        assert!(h.receiver.try_recv().is_err(), "unresolved default access cannot queue a target read");
        h.apply_role_result(DbResult::LocalRoleResolved {
            scope: "another-verse".into(), role: "viewer".into(),
        });
        assert!(h.tree.scene_access_generation().is_none(), "a stale-scope reply cannot initialize access");
        assert!(h.role.role.is_none());
        h.resolve_role(role);
        let generation = h.tree.scene_access_generation();
        assert!(generation.is_some(), "the production accepted-role path initializes default access");
        assert_eq!(h.guard().access, generation);
        assert_eq!(h.role.role, Some(fe_database::RoleLevel::from(role)));
        assert!(!h.tree.scene_access_blocked());
        h.resolve_role(role);
        assert_eq!(h.tree.scene_access_generation(), generation, "repeated accepted roles preserve the source epoch");
        h.frame(Vec::new(), false);
        assert!(eligible(&h.ctx));
        assert_eq!(h.receiver.len(), 1, "one bounded typed target read follows actual role acceptance");
        assert!(h.nav.active_petal_id.is_none());
        assert_eq!(h.nav.active_fractal_id.is_none(), verse_only);
        h.reply();
        h.frame(Vec::new(), false);
        let guard = h.guard();
        let surface = super::super::surface::snapshot(&h.ctx, guard).unwrap();
        assert_eq!(surface.profile(), "navigation_petal_v1");
        assert_eq!(surface.classification(), "navigation");
        assert!(h.mapped.is_some() && h.snapshot.is_some());
        assert!(h.nav.active_petal_id.is_none());
        assert_eq!(h.nav.active_fractal_id.is_none(), verse_only,
            "current-Verse bootstrap does not silently select a Fractal or Petal");
    }

    for denied in ["none", "not-a-role"] {
        let mut h = Harness::unresolved();
        h.nav.navigate_to_verse("nav-ui-verse", "Verse");
        h.resolve_role(denied);
        assert_eq!(h.role.role, Some(fe_database::RoleLevel::None));
        assert!(h.tree.scene_access_blocked());
        let guard = h.guard();
        assert!(!guard.capture_view);
        h.frame(Vec::new(), false);
        assert!(!eligible(&h.ctx));
        assert!(h.receiver.try_recv().is_err());
        assert!(h.mapped.is_none() && h.snapshot.is_none());
        assert!(super::super::surface::snapshot(&h.ctx, h.guard()).is_none());
    }
}

fn fail_and_recover(h: &mut Harness) {
    let mut output = bevy_egui::EguiOutput::default();
    let mut unsafe_update = h.original.as_ref().unwrap().clone();
    unsafe_update.tree = None;
    output.platform_output.accesskit_update = Some(unsafe_update);
    assert!(!provider_handoff(&h.ctx, &mut output.platform_output.accesskit_update,
        |_| panic!("unsafe original cannot reach positive mapping")));
    assert!(output.platform_output.accesskit_update.is_none());
    for _ in 0..2 {
        assert!(!provider_handoff(&h.ctx, &mut output.platform_output.accesskit_update, |_| None));
        assert!(epoch(&h.ctx).is_none());
    }
    output.platform_output.accesskit_update = h.original.clone();
    assert!(provider_handoff(&h.ctx, &mut output.platform_output.accesskit_update, |_| None));
    let recovered = output.platform_output.accesskit_update.take().unwrap();
    assert!(recovered.nodes.iter().all(|(_, node)| {
        node.author_id().is_none_or(|id| !id.starts_with("fe.nav.petal.v1;"))
    }));
    assert!(recovered.nodes.iter().any(|(id, _)| Some(*id) == h.physical));
    assert!(epoch(&h.ctx).is_none(), "safe unmarked output cannot restore retired proof eligibility");
}

fn extend_provider_recovery_journey() {
    let mut h = Harness::prepared();
    let old_request = h.request();
    let old_snapshot = h.snapshot.take().unwrap();
    let old_binding = old_snapshot.navigation(old_request.target).unwrap();
    let original_deadline = old_binding.proof.original_deadline();
    assert!(super::super::stage_bound(&h.ctx, &old_snapshot, &old_request, Some(old_binding.clone())));
    let retained_delivery = super::super::shared(&h.ctx).unwrap()
        .try_lock().unwrap().delivery.take().unwrap();
    fail_and_recover(&mut h);
    assert!(!is_current(&h.ctx, &old_binding));
    assert!(old_binding.translate(&h.ctx, ak::ActionRequest {
        target: old_request.target, action: ak::Action::Click, data: None }).is_none());
    assert!(!super::super::stage_bound(&h.ctx, &old_snapshot, &old_request, Some(old_binding.clone())));
    h.frame(Vec::new(), false);
    h.reply();
    h.frame(Vec::new(), false);
    let replacement = h.request();
    assert!(Instant::now() < original_deadline,
        "positive recovery must be demonstrated inside the retired original one-second deadline");
    assert_ne!(replacement.target, old_request.target);
    let fresh_binding = h.snapshot.as_ref().unwrap().navigation(replacement.target).unwrap();
    assert_ne!(fresh_binding.set.epoch(), old_binding.set.epoch());
    assert_ne!(fresh_binding.proof.target_epoch(), old_binding.proof.target_epoch());
    assert!(!h.snapshot.as_ref().unwrap().allows(&old_request));
    assert!(!h.snapshot.as_ref().unwrap().matches_navigation(replacement.target, Some(&old_binding)),
        "the production Pending-retention matching gate cannot substitute the replacement proof");
    assert!(!super::super::stage_bound(&h.ctx, &old_snapshot, &old_request, Some(old_binding.clone())));
    assert!(!is_current(&h.ctx, &old_binding));
    assert!(super::super::provider_identity::was_issued(old_request.target));
    super::super::shared(&h.ctx).unwrap().try_lock().unwrap().delivery = Some(retained_delivery);
    h.frame(vec![egui::Event::AccessKitActionRequest(ak::ActionRequest {
        target: old_binding.physical(), action: ak::Action::Click, data: None,
    })], false);
    assert_eq!(h.callbacks_created, 0, "old Delivery cannot become a callback after recovery");
    assert_eq!(h.callbacks_used, 0);
    assert_eq!(h.physical_clicks, 0, "stripped old native delivery cannot fall back to physical navigation");

    let mut h = Harness::prepared();
    let request = h.request();
    let (old_binding, input) = h.stage(&request);
    let original_deadline = old_binding.proof.original_deadline();
    h.hold_callback = true;
    h.frame(vec![egui::Event::AccessKitActionRequest(input)], false);
    assert!(h.held_callback.is_some());
    assert_eq!(h.callbacks_used, 0);
    fail_and_recover(&mut h);
    h.frame(Vec::new(), false);
    h.reply();
    h.frame(Vec::new(), false);
    assert!(Instant::now() < original_deadline);
    assert_ne!(h.request().target, request.target);
    assert!(!is_current(&h.ctx, &old_binding));
    h.restore_callback = true;
    h.frame(Vec::new(), false);
    assert_eq!(h.callbacks_used, 0, "retained old callback stays invalid with a fresh current response");
    assert!(h.nav.active_petal_id.is_none());
    let request = h.request();
    let (_, input) = h.stage(&request);
    h.frame(vec![egui::Event::AccessKitActionRequest(input)], false);
    assert_eq!(h.callbacks_used, 1, "only the fresh provider binding can navigate after quarantine");

    let mut h = Harness::new();
    h.frame(Vec::new(), false);
    let DbCommand::ReadNativePetalView { request_id, target } = h.receiver.try_recv().unwrap()
        else { panic!("exact accepted native read expected"); };
    fail_and_recover(&mut h);
    for _ in 0..2 {
        h.frame(Vec::new(), false);
        assert!(h.receiver.try_recv().is_err(), "retirement keeps the accepted read's ownership");
        let mut missing = None;
        assert!(!provider_handoff(&h.ctx, &mut missing, |_| None));
    }
    let _ = h.guard();
    let stale = DbResult::NativePetalViewRead { request_id: request_id.clone(), target: target.clone(),
        outcome: Ok(fe_runtime::native_navigation::PetalView { target, source_fence: "b".repeat(64) }) };
    {
        let mut state = h.reads.session.try_lock().unwrap();
        let source = state.source.clone().unwrap();
        assert_eq!(state.owner.receive(&stale, &source, Instant::now()),
            super::super::navigation::Reception::Discarded,
            "the retired exact result drains the accepted read but never restores its proof");
    }
    h.frame(Vec::new(), false);
    assert_eq!(h.receiver.len(), 1, "one fresh request is admitted only after the old read drains");
    h.reply();
    h.frame(Vec::new(), false);
    assert!(h.snapshot.is_some());
}
