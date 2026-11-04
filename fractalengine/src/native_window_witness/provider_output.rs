//! Validate the real provider handoff independently of positive annotation.

use super::{Lifecycle, Ownership, annotate};
use bevy::{a11y::{AccessibilitySystems, ManageAccessibilityUpdates}, prelude::*};
use bevy_egui::{EguiContext, EguiOutput, EguiPostUpdateSet, PrimaryEguiContext, input::WindowToEguiContextMap};
use fe_ui::native_accessibility as ui;
use std::sync::atomic::{AtomicBool, Ordering};

static HOLD: AtomicBool = AtomicBool::new(true);
pub(super) fn quarantined() -> bool { HOLD.load(Ordering::SeqCst) }

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, prepare
        .in_set(super::ProviderHandoff)
        .after(EguiPostUpdateSet::ProcessOutput)
        .before(annotate)
        .before(bevy_egui::update_accessibility_system)
        .in_set(EguiPostUpdateSet::PostProcessOutput))
        .add_systems(PostUpdate, hold_alternate_publisher
            .after(bevy_egui::update_accessibility_system)
            .before(AccessibilitySystems::Update));
}

fn hold_alternate_publisher(mut manage: ResMut<ManageAccessibilityUpdates>) {
    // The validated external tree remains the sole primary publisher after recovery.
    **manage = false;
}

fn prepare(
    mut reads: ParamSet<(Ownership, Query<(Entity, &mut EguiOutput, Option<&PrimaryEguiContext>, Option<&mut EguiContext>)>)>,
    map: Res<WindowToEguiContextMap>,
    windows: Query<Entity, With<bevy::window::PrimaryWindow>>,
    lifecycle: Res<Lifecycle>,
    terminal: Option<Res<fe_runtime::maintenance::TerminalMode>>,
    capture: Option<Res<fe_renderer::camera::CameraCaptureLock>>,
) {
    HOLD.store(true, Ordering::SeqCst);
    let root = reads.p0().current();
    if let Some(root) = &root { ui::navigation_ui::suppress(&root.egui); }
    let positive = lifecycle.running && capture.is_none()
        && !terminal.is_some_and(|mode| mode.is_terminal());
    let mut suppressed = false;
    let mut forwarded = false;
    for (entity, mut output, primary, context) in &mut reads.p1() {
        let candidate = root.as_ref().filter(|root| root.context == entity);
        let owned_surface = candidate.is_some() || primary.is_some()
            || map.context_to_window.get(&entity).is_some_and(|window| windows.contains(*window));
        if !owned_surface { continue; }
        let Some(root) = candidate else {
            output.platform_output.accesskit_update = None;
            if let Some(mut context) = context { ui::navigation_ui::retire_provider(context.get_mut()); }
            suppressed = true;
            continue;
        };
        let safe = handoff(&root.egui, &mut output, |original| {
            positive.then(|| ui::surface::snapshot(&root.egui, root.guard)).flatten()
                .filter(|surface| surface.profile() == "navigation_petal_v1")
                .and_then(|_| ui::navigation_ui::remap(&root.egui, original))
        });
        let navigation_unmapped = ui::surface::snapshot(&root.egui, root.guard)
            .is_some_and(|surface| surface.profile() == "navigation_petal_v1")
            && !ui::navigation_ui::forwarded_is_current(&root.egui);
        if navigation_unmapped { ui::navigation_ui::retire_provider(&root.egui); }
        forwarded |= safe;
        suppressed |= !safe;
    }
    let held = suppressed || !forwarded;
    HOLD.store(held, Ordering::SeqCst);
    if held {
        if let Some(root) = &root {
            ui::navigation_ui::retire_provider(&root.egui);
        }
    }
}

fn handoff(ctx: &bevy_egui::egui::Context, output: &mut EguiOutput,
    mapping: impl FnOnce(&bevy_egui::egui::accesskit::TreeUpdate)
        -> Option<bevy_egui::egui::accesskit::TreeUpdate>) -> bool {
    ui::navigation_ui::provider_handoff(ctx, &mut output.platform_output.accesskit_update, mapping)
}

#[cfg(test)]
pub(super) fn extend_existing_journey() {
    use bevy_egui::egui::accesskit as ak;
    let ctx = bevy_egui::egui::Context::default();
    let root = ak::NodeId(1);
    let physical = ak::NodeId(2);
    let mut parent = ak::Node::new(ak::Role::Window);
    parent.set_children(vec![physical]);
    let mut button = ak::Node::new(ak::Role::Button);
    button.add_action(ak::Action::Click);
    button.set_author_id("fe.nav.petal.v1;s=900001;t=900002");
    let full = ak::TreeUpdate { nodes: vec![(root, parent), (physical, button)],
        tree: Some(ak::Tree::new(root)), focus: physical };
    let identities = [ui::provider_identity::TargetIdentity { physical, epoch: 900002 }];
    let mut output = EguiOutput::default();
    output.platform_output.accesskit_update = Some(full.clone());
    assert!(handoff(&ctx, &mut output, |tree| {
        ui::provider_identity::remap(tree, 900001, &identities).map(|(tree, _)| tree)
    }));
    let mapped = output.platform_output.accesskit_update.take().unwrap();
    let alias = mapped.focus;
    assert_ne!(alias, physical);
    assert_eq!(mapped.nodes[0].1.children(), &[alias]);
    let mut forwarded = vec![mapped];

    for failure in 0..4 {
        let mut unsafe_update = full.clone();
        match failure {
            0 => { unsafe_update.tree = None; unsafe_update.nodes.remove(0); }
            1 => { unsafe_update.nodes.remove(1); }
            2 => unsafe_update.nodes[0].1.set_controls(vec![ak::NodeId(99)]),
            _ => unsafe_update.focus = ak::NodeId(99),
        }
        output.platform_output.accesskit_update = Some(unsafe_update);
        HOLD.store(!handoff(&ctx, &mut output, |_| panic!("unsafe original must never reach positive checks")), Ordering::SeqCst);
        let mut app = App::new();
        app.init_resource::<ManageAccessibilityUpdates>();
        app.world_mut().resource_mut::<ManageAccessibilityUpdates>().set(true);
        app.add_systems(Update, hold_alternate_publisher);
        app.update();
        assert!(!app.world().resource::<ManageAccessibilityUpdates>().get(),
            "egui None branch cannot re-enable the alternate publisher during quarantine");
        assert!(output.platform_output.accesskit_update.is_none());
        assert_eq!(forwarded.len(), 1, "neither stand-in forwards an unsafe delta");
        assert!(ui::provider_identity::was_issued(alias));
    }
    assert!(!handoff(&ctx, &mut output, |_| None), "missing output cannot recover");
    assert!(HOLD.load(Ordering::SeqCst));
    output.platform_output.accesskit_update = Some(full);
    let recovered = handoff(&ctx, &mut output, |_| None);
    HOLD.store(!recovered, Ordering::SeqCst);
    assert!(recovered, "positive failure permits only independently validated full fallback");
    forwarded.push(output.platform_output.accesskit_update.take().unwrap());
    assert_eq!(forwarded[1].nodes[0].1.children(), &[physical]);
    assert_eq!(forwarded[1].focus, physical);
    assert!(forwarded[1].nodes.iter().all(|(id, node)| *id != alias && node.author_id().is_none()));
    assert!(ui::provider_identity::was_issued(alias), "safe recovery retains the tombstone");
    assert!(!HOLD.load(Ordering::SeqCst));
}
