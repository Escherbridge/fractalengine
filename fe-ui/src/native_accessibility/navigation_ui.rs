//! Actual rendered Petal proof and callback bindings; see AGENTS.md.

#[cfg(test)]
#[path = "navigation_ui_journey.rs"]
mod journey;
#[cfg(test)]
pub(super) fn extend_existing_journey() { journey::extend_existing_journey(); }

use super::{Guard, navigation::{Bindings, Proof, ProofOwner, Source, Target}, shared};
use bevy::{ecs::message::MessageCursor, prelude::{Messages, Resource}};
use bevy_egui::egui::{self, accesskit as ak};
use fe_runtime::messages::{DbCommand, DbResult};
use std::{sync::{Arc, Mutex}, time::Instant};

#[derive(Default)]
struct Session {
    owner: ProofOwner,
    source: Option<Source>,
    rendered: Vec<Target>,
    pass: u64,
    invalid: bool,
    published: Option<u64>,
    callback: Option<Callback>,
    forwarded: Option<(u64, Vec<Binding>)>,
}
type Shared = Arc<Mutex<Session>>;

/// Lifetime survives UI/context retirement and retains an accepted read.
#[derive(Resource, Default)]
pub struct NavigationReads {
    session: Shared,
    cursor: MessageCursor<DbResult>,
}

#[derive(Clone)]
pub struct Binding {
    provider: ak::NodeId,
    physical: ak::NodeId,
    set: Bindings,
    proof: Arc<Proof>,
}
impl Binding {
    pub fn physical(&self) -> ak::NodeId { self.physical }
    pub fn provider(&self) -> ak::NodeId { self.provider }
    pub fn same(&self, other: &Self) -> bool {
        self.provider == other.provider && self.physical == other.physical
            && self.set.epoch() == other.set.epoch() && Arc::ptr_eq(&self.proof, &other.proof)
    }
    pub fn translate(&self, ctx: &egui::Context, mut request: ak::ActionRequest) -> Option<ak::ActionRequest> {
        if request.target != self.provider || request.action != ak::Action::Click
            || request.data.is_some() || !is_current(ctx, self) { return None; }
        request.target = self.physical;
        Some(request)
    }
    pub(super) fn matches_node(&self, node: &ak::Node, rect: egui::Rect) -> bool {
        let bounds = ak::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(),
            x1: rect.max.x.into(), y1: rect.max.y.into() };
        let label = format!("Open petal {}", self.proof.target().petal.name);
        let tag = format!("fe.nav.petal.v1;s={};t={}", self.set.epoch(), self.proof.target_epoch());
        node.role() == ak::Role::Button && node.supports_action(ak::Action::Click)
            && node.label() == Some(label.as_str()) && node.author_id() == Some(tag.as_str())
            && node.bounds() == Some(bounds)
    }
}

pub(crate) struct Callback {
    binding: Binding,
    pub target: fe_runtime::native_navigation::PetalTarget,
    pub verse_name: String,
    pub fractal_name: String,
}

fn key() -> egui::Id { egui::Id::new("fe-native-navigation-read-owner") }
fn session(ctx: &egui::Context) -> Option<Shared> {
    ctx.data(|data| data.get_temp::<Shared>(key()))
}

pub fn attach(ctx: &egui::Context, reads: &NavigationReads) {
    ctx.data_mut(|data| data.insert_temp(key(), reads.session.clone()));
}

pub fn refresh_source(ctx: &egui::Context, guard: Guard,
    nav: &crate::navigation_manager::NavigationManager,
    manager: &crate::verse_manager::VerseManager,
    nodes: &crate::node_manager::NodeManager) {
    let Some(session) = session(ctx) else { return; };
    let lifetime = shared(ctx).and_then(|pass| pass.try_lock().ok().map(|pass| pass.lifetime));
    let source = Source::capture(lifetime.unwrap_or(0), super::surface::observed_generation(),
        guard, nav, manager, nodes);
    if let Ok(mut state) = session.try_lock() { state.source = Some(source); };
}

pub fn pump(reads: &mut NavigationReads, results: &Messages<DbResult>) {
    let Ok(mut state) = reads.session.try_lock() else { return; };
    let Some(source) = state.source.clone() else { return; };
    for result in reads.cursor.read(results) {
        state.owner.receive(result, &source, Instant::now());
    }
    publish_epoch(&mut state);
}

fn current(state: &Session, binding: &Binding, now: Instant) -> bool {
    super::surface::observed_available() && state.source.as_ref().is_some_and(|source| {
        source.observed_at(super::surface::observed_generation())
            && state.owner.bindings_are_current(&binding.set, source, now)
            && state.owner.proof_is_current(&binding.proof, source, now)
            && binding.proof.target().widget == binding.physical.0
    })
}
pub fn is_current(ctx: &egui::Context, binding: &Binding) -> bool {
    session(ctx).is_some_and(|state| state.try_lock().is_ok_and(|state| {
        current(&state, binding, Instant::now())
    }))
}

fn bindings(state: &Session) -> Option<Bindings> {
    super::surface::observed_available().then_some(())?;
    let source = state.source.as_ref()?;
    source.observed_at(super::surface::observed_generation()).then_some(())?;
    state.owner.bindings(source, Instant::now())
}
fn publish_epoch(state: &mut Session) {
    let epoch = bindings(state).map(|set| set.epoch());
    if state.published != epoch {
        state.published = epoch;
        super::surface::publication_transition();
    }
}
pub fn epoch(ctx: &egui::Context) -> Option<u64> {
    let state = session(ctx)?;
    let state = state.try_lock().ok()?;
    Some(bindings(&state)?.epoch())
}
pub fn eligible(ctx: &egui::Context) -> bool {
    session(ctx).is_some_and(|state| state.try_lock().is_ok_and(|state| {
        state.source.as_ref().is_some_and(Source::eligible)
    }))
}

pub(crate) fn begin(ctx: &egui::Context) {
    if let Some(state) = session(ctx) {
        if let Ok(mut state) = state.try_lock() {
            state.pass = ctx.cumulative_pass_nr();
            state.rendered.clear();
            state.callback = None;
            state.invalid = false;
            state.forwarded = None;
        }
    }
}

pub(crate) fn record(ui: &egui::Ui, response: &egui::Response,
    target: fe_runtime::native_navigation::PetalTarget) {
    if let Some(pass) = shared(ui.ctx()) {
        if let Ok(mut pass) = pass.try_lock() {
            if let Some(widget) = pass.widgets.get_mut(&response.id.value()) { widget.petal = true; }
        }
    }
    let Some(state) = session(ui.ctx()) else { return; };
    let Ok(mut state) = state.try_lock() else { return; };
    if state.invalid || state.pass != ui.ctx().cumulative_pass_nr() || !eligible_source(&state) {
        return;
    }
    if !response.enabled() || !response.interact_rect.is_positive()
        || !ui.clip_rect().contains_rect(response.interact_rect)
        || !response.rect.is_finite() || !ui.clip_rect().contains_rect(response.rect)
        || !ui.ctx().memory(|memory| memory.allows_interaction(response.layer_id)) { return; }
    if !target.is_bounded() || state.rendered.len() >= super::navigation::MAX_TARGETS
        || state.rendered.iter().any(|old| old.widget == response.id.value()
            || old.petal.petal_id == target.petal_id) {
        state.invalid = true;
        state.rendered.clear();
        return;
    }
    let actual = Target { widget: response.id.value(), petal: target };
    if let Some(set) = bindings(&state) {
        if let Some(proof) = set.proofs().iter().find(|proof| proof.target() == &actual) {
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_author_id(format!("fe.nav.petal.v1;s={};t={}", set.epoch(), proof.target_epoch()));
            });
        }
    }
    state.rendered.push(actual);
}
fn eligible_source(state: &Session) -> bool {
    state.source.as_ref().is_some_and(Source::eligible)
}

pub(crate) fn finish(ctx: &egui::Context, sender: &crossbeam::channel::Sender<DbCommand>) {
    let Some(state) = session(ctx) else { return; };
    let Ok(mut state) = state.try_lock() else { return; };
    let Some(source) = state.source.clone() else { state.owner.retire(); return; };
    let targets = if !state.invalid && state.pass == ctx.cumulative_pass_nr()
        && super::surface::navigation_environment_current(ctx) {
        state.rendered.clone()
    } else { Vec::new() };
    state.owner.observe(&source, &targets, Instant::now());
    state.owner.submit(&source, Instant::now(), sender);
    publish_epoch(&mut state);
}

/// Remap only actual completed sidebar buttons under their original proof set.
pub fn remap(ctx: &egui::Context, update: &ak::TreeUpdate) -> Option<ak::TreeUpdate> {
    let pass = shared(ctx)?;
    let pass = pass.try_lock().ok()?;
    if pass.invalid || pass.number.checked_add(1) != Some(ctx.cumulative_pass_nr()) { return None; }
    let state = session(ctx)?;
    let mut state = state.try_lock().ok()?;
    let set = bindings(&state)?;
    if state.invalid || state.pass.checked_add(1) != Some(ctx.cumulative_pass_nr())
        || set.proofs().len() != state.rendered.len() { return None; }
    let mut identities = Vec::with_capacity(set.proofs().len());
    for proof in set.proofs() {
        let target = proof.target();
        if !state.rendered.contains(target) { return None; }
        let node = &update.nodes.iter().find(|(id, _)| id.0 == target.widget)?.1;
        let widget = pass.widgets.get(&target.widget)?;
        let expected_bounds = ak::Rect { x0: widget.rect.min.x.into(), y0: widget.rect.min.y.into(),
            x1: widget.rect.max.x.into(), y1: widget.rect.max.y.into() };
        if !widget.enabled || !widget.petal || node.bounds() != Some(expected_bounds)
            || !ctx.memory(|memory| memory.allows_interaction(widget.layer)) { return None; }
        if node.role() != ak::Role::Button || !node.supports_action(ak::Action::Click)
            || node.label() != Some(format!("Open petal {}", target.petal.name).as_str()) {
            return None;
        }
        let tag = format!("fe.nav.petal.v1;s={};t={}", set.epoch(), proof.target_epoch());
        if node.author_id() != Some(tag.as_str()) { return None; }
        identities.push(super::provider_identity::TargetIdentity {
            physical: ak::NodeId(target.widget), epoch: proof.target_epoch(),
        });
    }
    let (mapped, aliases) = super::provider_identity::remap(update, set.epoch(), &identities)?;
    let retained = aliases.into_iter().map(|alias| {
        let proof = set.proofs().iter().find(|proof| {
            proof.target().widget == alias.physical.0 && proof.target_epoch() == alias.epoch
        })?.clone();
        Some(Binding { provider: alias.provider, physical: alias.physical, set: set.clone(), proof })
    }).collect::<Option<Vec<_>>>()?;
    if !retained.iter().all(|binding| current(&state, binding, Instant::now())) { return None; }
    state.forwarded = Some((ctx.cumulative_pass_nr(), retained));
    Some(mapped)
}

pub(super) fn translated(ctx: &egui::Context, provider: ak::NodeId) -> Option<Binding> {
    let state = session(ctx)?;
    let state = state.try_lock().ok()?;
    let (pass, bindings) = state.forwarded.as_ref()?;
    if *pass != ctx.cumulative_pass_nr() { return None; }
    bindings.iter().find(|binding| binding.provider == provider
        && current(&state, binding, Instant::now())).cloned()
}
pub fn forwarded_is_current(ctx: &egui::Context) -> bool {
    session(ctx).is_some_and(|state| state.try_lock().is_ok_and(|state| {
        state.forwarded.as_ref().is_some_and(|(pass, bindings)| {
            *pass == ctx.cumulative_pass_nr() && !bindings.is_empty()
                && bindings.iter().all(|binding| current(&state, binding, Instant::now()))
        })
    }))
}
pub(crate) fn staged_click(ctx: &egui::Context, response: &egui::Response) -> bool {
    shared(ctx).is_some_and(|state| state.try_lock().is_ok_and(|state| {
        state.navigation_click.as_ref().is_some_and(|binding| binding.physical.0 == response.id.value())
    }))
}

/// Native effects wait until the guarded sidebar has returned to the root.
pub(crate) fn defer_if_native(ctx: &egui::Context, response: &egui::Response,
    target: fe_runtime::native_navigation::PetalTarget, verse_name: &str, fractal_name: &str) -> bool {
    let native = shared(ctx).is_some_and(|state| state.try_lock().map_or(true, |pass| pass.native_input));
    if !native { return false; }
    let binding = shared(ctx).and_then(|state| state.try_lock().ok()
        .and_then(|state| state.navigation_click.clone()));
    if let (Some(binding), Some(state)) = (binding, session(ctx)) {
        if binding.physical.0 == response.id.value() && binding.proof.target().petal == target {
            if let Ok(mut state) = state.try_lock() {
                if state.callback.is_none() && current(&state, &binding, Instant::now()) {
                    state.callback = Some(Callback { binding, target,
                        verse_name: verse_name.into(), fractal_name: fractal_name.into() });
                }
            }
        }
    }
    true
}

pub(crate) fn take_callback(ctx: &egui::Context, hierarchy: &crate::verse_manager::VerseManager) -> Option<Callback> {
    let state = session(ctx)?;
    let mut state = state.try_lock().ok()?;
    let callback = state.callback.take()?;
    let verse = hierarchy.verses.iter().find(|verse| verse.id == callback.target.verse_id)?;
    let fractal = verse.fractals.iter().find(|fractal| fractal.id == callback.target.fractal_id)?;
    let petal = fractal.petals.iter().find(|petal| petal.id == callback.target.petal_id)?;
    if verse.name != callback.verse_name || fractal.name != callback.fractal_name
        || petal.name != callback.target.name { return None; }
    (state.pass == ctx.cumulative_pass_nr() && !state.invalid
        && state.rendered.contains(callback.binding.proof.target())
        && current(&state, &callback.binding, Instant::now()))
        .then_some(callback)
}

pub fn suppress(ctx: &egui::Context) {
    if let Some(state) = session(ctx) {
        if let Ok(mut state) = state.try_lock() {
            state.forwarded = None;
            state.callback = None;
        }
    }
}

/// Unsafe provider output permanently retires this binding incarnation.
pub fn retire_provider(ctx: &egui::Context) {
    super::surface::transition();
    if let Some(state) = session(ctx) {
        if let Ok(mut state) = state.try_lock() {
            state.owner.retire();
            state.forwarded = None;
            state.callback = None;
            publish_epoch(&mut state);
        }
    }
    super::surface::suppress_publication(ctx);
}

pub fn provider_update_is_safe(ctx: &egui::Context, update: Option<&ak::TreeUpdate>) -> bool {
    if update.is_some_and(super::provider_identity::full_tree_is_safe) { return true; }
    retire_provider(ctx);
    false
}

/// The actual output slot is empty on unsafe input, including a missing full tree.
pub fn provider_handoff(ctx: &egui::Context, output: &mut Option<ak::TreeUpdate>,
    mapping: impl FnOnce(&ak::TreeUpdate) -> Option<ak::TreeUpdate>) -> bool {
    let original = output.take();
    if !provider_update_is_safe(ctx, original.as_ref()) { return false; }
    let Some(mut original) = original else { return false; };
    *output = Some(mapping(&original).unwrap_or_else(|| {
        for (_, node) in &mut original.nodes {
            if node.author_id().is_some_and(|id| id.starts_with("fe.nav.petal.v1;")
                || id == "fe.native.window.v1") {
                node.clear_author_id();
                if node.description().is_some_and(|description| description.starts_with("FEW1;")) {
                    node.clear_description();
                }
            }
        }
        original
    }));
    true
}
pub(super) fn retire(ctx: &egui::Context) {
    if let Some(state) = session(ctx) {
        if let Ok(mut state) = state.try_lock() {
            state.owner.retire();
            state.source = None;
            state.forwarded = None;
            state.callback = None;
            state.rendered.clear();
            publish_epoch(&mut state);
        }
    }
    ctx.data_mut(|data| data.remove::<Shared>(key()));
}
