//! Session-only hierarchy visibility + groups — Phases 0-1 of
//! `hierarchy_visibility_groups_20260808`. See `resolve.rs` for the pure
//! lattice and `conductor/tracks/hierarchy_visibility_groups_20260808/spec.md`
//! for the full design + ratification.
//!
//! NO PERSISTENCE yet. `VisibilityState` resets on app restart — node
//! overrides / group membership move to node properties (`view.state` /
//! `view.groups`) and the group registry to `petal.visibility_groups` in
//! Phases 3-4 (RATIFICATION #2/#3). Fractal/verse hidden sets ARE ratified to
//! persist eventually (RATIFICATION #1, a Phase 4 schema migration) but are
//! session-held here too, same as everything else in this resource.
//!
//! Lattice fields are `pub(crate)` (crate-internal only — never `pub`, so
//! `fe-ui` consumers outside the crate can't see them). PRODUCTION mutation
//! (toggle fns, node overrides, solo set/clear, group edits) routes
//! exclusively through the `VisibilityState` methods below, which bump
//! `epoch` (review finding F3a; see `sync_node_visibility` docs for why that
//! counter exists instead of Bevy's `ResMut::is_changed()`) — `panels/
//! sidebar.rs` is the one production call site and is fully migrated to
//! them. Fields stay `pub(crate)` rather than fully module-private only to
//! keep `node_manager/viewport_pick.rs`'s existing test fixtures (crate-
//! internal test code, out of this module's edit boundary this wave)
//! compiling; nothing outside `panels/sidebar.rs` mutates the lattice in
//! production. `resolve.rs` (a descendant module) reads fields directly, as
//! does any other crate-internal reader.
//!
//! Phase 2 wires the resolver into every picker + camera-focus path
//! (spawn-then-hide, RATIFICATION #7) — NOT built here (FX2's separate work
//! this wave). Force-deselect-on-hide (RATIFICATION D-13) IS built here:
//! `sync_node_visibility` clears `NodeManager.selected` the frame its node
//! becomes effectively hidden. Until Phase 2 lands the picker/camera-focus
//! filters, hidden nodes stay pickable/focusable in the viewport — documented,
//! not a bug in this phase.

pub mod resolve;

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

pub use resolve::{
    effective_visibility, fractal_effective_visible, glyph_for_bool, glyph_for_rollup,
    hidden_chip_label, node_eye_tooltip, node_hidden_reason, petal_effective_visible, rollup,
    toggle_node_override, toggle_scope_hidden, verse_effective_visible, HiddenReason, NodeAncestry,
    RollupState,
};

use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::plugin::SpawnedNodeMarker;
use crate::verse_manager::VerseManager;

/// Per-node tri-state visibility override (RATIFICATION #6). `Auto` is the
/// default — the node defers to the ancestor chain / group state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverrideState {
    #[default]
    Auto,
    Show,
    Hide,
}

/// Transient isolate/solo lens (RATIFICATION: NOT part of the persisted
/// lattice — session-only, cleared on petal switch and on Escape; the Escape
/// ladder rung is Phase 2 / `node_manager/shortcuts.rs`, out of scope here).
/// A set of node ids, not scopes — see spec §3.1.
#[derive(Debug, Clone, Default)]
pub struct SoloSet {
    pub members: HashSet<String>,
}

/// A visibility Group row. Session placeholder for the petal-scoped
/// `petal.visibility_groups` registry Phases 3-4 will persist (RATIFICATION
/// #2/#3). The user-facing word is "Group" (RATIFICATION #18) — use it in all
/// copy, never "Layer" (collides with terrain layers) or "View set".
#[derive(Debug, Clone)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub visible: bool,
}

/// Session-only visibility state: per-node tri-state overrides, session
/// -hidden petal/fractal/verse id sets, the Group registry + node membership
/// map, and the transient solo lens. See module docs — nothing here survives
/// app restart in this phase.
///
/// Lattice fields are `pub(crate)` (see module docs for why not fully
/// private). Mutate exclusively through the methods below (they keep `epoch`
/// honest — F3a); read either through a method, or through the free resolver
/// functions in `resolve.rs` which this module re-exports.
#[derive(Resource, Debug, Clone, Default)]
pub struct VisibilityState {
    /// node_id -> tri-state override. Absent = `Auto`.
    pub(crate) node_overrides: HashMap<String, OverrideState>,
    pub(crate) hidden_petals: HashSet<String>,
    pub(crate) hidden_fractals: HashSet<String>,
    pub(crate) hidden_verses: HashSet<String>,
    /// Session placeholder for the future `petal.visibility_groups` registry.
    pub(crate) groups: Vec<Group>,
    /// node_id -> group ids it belongs to. Session placeholder for the future
    /// `view.groups` node property.
    pub(crate) group_membership: HashMap<String, Vec<String>>,
    /// Transient isolate/solo overlay — never persisted (spec §3.1).
    pub(crate) solo: Option<SoloSet>,
    /// Bumped by exactly 1 on every real mutation below. `sync_node_visibility`
    /// gates its apply pass on this instead of `ResMut::is_changed()` (F3a:
    /// `gardener_ui_system` derefs `&mut VisibilityState` unconditionally every
    /// frame via the `MiscUiParams` bundle, so `is_changed()` is permanently
    /// true and defeats change detection entirely).
    epoch: u64,
    /// F19 memoization cache for `hidden_count_in_active_petal_cached`:
    /// (epoch, active_petal_id, count) as of the last computation. A pure
    /// cache, not part of the mutation-tracked lattice — writing it does NOT
    /// bump `epoch`.
    hidden_count_cache: Option<(u64, Option<String>, usize)>,
}

impl VisibilityState {
    /// Monotonic mutation counter (F3a). See the field doc for why it exists.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    fn bump_epoch(&mut self) {
        self.epoch += 1;
    }

    /// Toggles whether `verse_id` is session-hidden (sidebar verse-row eye).
    /// Bumps `epoch`.
    pub fn toggle_verse_hidden(&mut self, verse_id: &str) {
        toggle_scope_hidden(&mut self.hidden_verses, verse_id);
        self.bump_epoch();
    }

    /// Toggles whether `fractal_id` is session-hidden (sidebar fractal-row
    /// eye). Bumps `epoch`.
    pub fn toggle_fractal_hidden(&mut self, fractal_id: &str) {
        toggle_scope_hidden(&mut self.hidden_fractals, fractal_id);
        self.bump_epoch();
    }

    /// Toggles whether `petal_id` is session-hidden (sidebar petal-row eye).
    /// Bumps `epoch`.
    pub fn toggle_petal_hidden(&mut self, petal_id: &str) {
        toggle_scope_hidden(&mut self.hidden_petals, petal_id);
        self.bump_epoch();
    }

    /// Current tri-state override for `node_id` (`Auto` when absent — the map
    /// is kept sparse, matching the old field's doc contract).
    pub fn node_override(&self, node_id: &str) -> OverrideState {
        self.node_overrides
            .get(node_id)
            .copied()
            .unwrap_or_default()
    }

    /// Sets `node_id`'s tri-state override (sidebar node-row eye). `Auto`
    /// removes the entry rather than storing it explicitly. Bumps `epoch`.
    pub fn set_node_override(&mut self, node_id: &str, next: OverrideState) {
        if next == OverrideState::Auto {
            self.node_overrides.remove(node_id);
        } else {
            self.node_overrides.insert(node_id.to_string(), next);
        }
        self.bump_epoch();
    }

    /// The active isolate/solo lens, if any.
    pub fn solo(&self) -> Option<&SoloSet> {
        self.solo.as_ref()
    }

    /// Activates the isolate/solo lens over exactly `members` (Phase 2 UI —
    /// not wired to any control yet). Bumps `epoch`.
    pub fn set_solo(&mut self, members: HashSet<String>) {
        self.solo = Some(SoloSet { members });
        self.bump_epoch();
    }

    /// Clears the isolate/solo lens. A no-op (and does NOT bump `epoch`) when
    /// solo was already inactive, so petal-switch churn — which calls this
    /// unconditionally — doesn't manufacture spurious epoch bumps.
    pub fn clear_solo(&mut self) {
        if self.solo.take().is_some() {
            self.bump_epoch();
        }
    }

    /// Registers a new visibility Group (Phase 3-4 persistence target,
    /// session-only for now — see module docs). Bumps `epoch`.
    pub fn add_group(&mut self, group: Group) {
        self.groups.push(group);
        self.bump_epoch();
    }

    /// Removes a Group by id and drops it from every node's membership list.
    /// Bumps `epoch`.
    pub fn remove_group(&mut self, id: &str) {
        self.groups.retain(|g| g.id != id);
        for members in self.group_membership.values_mut() {
            members.retain(|gid| gid != id);
        }
        self.bump_epoch();
    }

    /// Sets whether `id`'s Group is visible. Bumps `epoch` unconditionally
    /// (matches the other setters — cheap, and a no-op find still means "no
    /// observable state changed" callers should avoid calling redundantly).
    pub fn set_group_visible(&mut self, id: &str, visible: bool) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == id) {
            group.visible = visible;
        }
        self.bump_epoch();
    }

    /// Replaces `node_id`'s group membership list wholesale. Bumps `epoch`.
    pub fn set_node_group_membership(&mut self, node_id: &str, group_ids: Vec<String>) {
        if group_ids.is_empty() {
            self.group_membership.remove(node_id);
        } else {
            self.group_membership.insert(node_id.to_string(), group_ids);
        }
        self.bump_epoch();
    }

    /// Memoized wrapper around [`hidden_count_in_active_petal`] for the
    /// status-bar chip (F19): recomputes only when `epoch` or the active
    /// petal id differs from the cached pass, O(1) otherwise. See that
    /// function's docs for the denominator note (hierarchy rows, not spawned
    /// entities).
    pub fn hidden_count_in_active_petal_cached(
        &mut self,
        hierarchy: &VerseManager,
        nav: &NavigationManager,
    ) -> usize {
        let cache_epoch = self.epoch;
        let cache_petal = nav.active_petal_id.clone();
        if let Some((cached_epoch, cached_petal, cached_count)) = &self.hidden_count_cache {
            if *cached_epoch == cache_epoch && *cached_petal == cache_petal {
                return *cached_count;
            }
        }
        let count = hidden_count_in_active_petal(hierarchy, nav, self);
        self.hidden_count_cache = Some((cache_epoch, cache_petal, count));
        count
    }
}

/// Count of effectively-hidden nodes in the active petal — feeds the
/// status-bar "N hidden" chip (RATIFICATION #16). No active petal (nothing
/// navigated to yet, or the petal isn't in the loaded hierarchy) counts as
/// zero.
///
/// DENOMINATOR (F19): this counts HIERARCHY ROWS — `VerseManager`'s
/// `PetalEntry::nodes` for the active petal. `sync_node_visibility` below
/// counts something different: SPAWNED ENTITIES (`SpawnedNodeMarker`
/// instances), which can diverge from the node-row count (e.g. a node not
/// yet spawned, or a GPX ribbon/stamp materializing several entities for one
/// node). The two are answering different questions — don't "fix" one to
/// match the other without re-deriving both from spec.
///
/// Prefer [`VisibilityState::hidden_count_in_active_petal_cached`] at call
/// sites that run every frame (e.g. the status bar); this free function
/// recomputes unconditionally and is meant for tests / one-off callers.
pub fn hidden_count_in_active_petal(
    hierarchy: &VerseManager,
    nav: &NavigationManager,
    state: &VisibilityState,
) -> usize {
    let (Some(verse_id), Some(fractal_id), Some(petal_id)) = (
        nav.active_verse_id.as_deref(),
        nav.active_fractal_id.as_deref(),
        nav.active_petal_id.as_deref(),
    ) else {
        return 0;
    };
    let Some(petal) = hierarchy.find_petal(petal_id) else {
        return 0;
    };
    let ancestry = NodeAncestry {
        verse_id,
        fractal_id,
        petal_id,
    };
    petal
        .nodes
        .iter()
        .filter(|n| !effective_visibility(&n.id, ancestry, state))
        .count()
}

/// Builds a `petal_id -> (fractal_id, verse_id)` index from the full
/// hierarchy — lets `sync_node_visibility` resolve each `SpawnedNodeMarker`'s
/// OWN ancestry via `marker.petal_id` (F12) instead of applying the active
/// petal's ancestry to every spawned entity regardless of which petal it
/// actually belongs to (GPX ribbons and other cross-petal spawns make that
/// substitution wrong).
fn build_petal_ancestry_index(hierarchy: &VerseManager) -> HashMap<String, (String, String)> {
    let mut index = HashMap::new();
    for verse in &hierarchy.verses {
        for fractal in &verse.fractals {
            for petal in &fractal.petals {
                index.insert(petal.id.clone(), (fractal.id.clone(), verse.id.clone()));
            }
        }
    }
    index
}

/// F3a gate: whether `sync_node_visibility` should re-apply this pass. Pure
/// so it's directly unit-testable without a Bevy `World`. A petal switch
/// always forces a re-apply (every entity's ancestry potentially changed,
/// independent of `VisibilityState` mutation); otherwise only an `epoch`
/// change does.
fn should_apply(petal_switched: bool, last_epoch: u64, current_epoch: u64) -> bool {
    petal_switched || last_epoch != current_epoch
}

/// Pure decision core for `sync_node_visibility` (F3b minimal-authority
/// semantics + F12 per-entity ancestry + D-13 force-deselect) — no Bevy
/// dependency, so it's fully unit-tested without a `World`/`Commands`. Takes
/// each spawned entity's `(entity, node_id, petal_id)`, the petal ancestry
/// index, the set of entities THIS module previously hid, and the currently
/// selected node id; returns what the caller should do.
struct SyncDecision {
    /// Entities to `insert(Visibility::Hidden)` on and add to the
    /// caller's hidden-by-us set.
    to_hide: Vec<Entity>,
    /// Entities to `insert(Visibility::Inherited)` on and remove from the
    /// caller's hidden-by-us set. ONLY ever contains entities that were
    /// already in `hidden_by_us` — this module never writes `Inherited` onto
    /// an entity it didn't hide itself (F3b: don't resurrect ribbons another
    /// writer, e.g. `gis.track.visible`/`LayerStack`, hid on purpose).
    to_restore: Vec<Entity>,
    /// True if the selected node became effectively hidden this pass and
    /// should be force-deselected (RATIFICATION D-13).
    deselect: bool,
}

fn compute_sync_decision(
    entities: &[(Entity, &str, &str)],
    petal_ancestry: &HashMap<String, (String, String)>,
    hidden_by_us: &HashSet<Entity>,
    state: &VisibilityState,
    selected_node_id: Option<&str>,
) -> SyncDecision {
    let mut to_hide = Vec::new();
    let mut to_restore = Vec::new();
    let mut deselect = false;

    for &(entity, node_id, petal_id) in entities {
        let (fractal_id, verse_id) = petal_ancestry
            .get(petal_id)
            .map(|(f, v)| (f.as_str(), v.as_str()))
            .unwrap_or(("", ""));
        let ancestry = NodeAncestry {
            verse_id,
            fractal_id,
            petal_id,
        };
        let visible = effective_visibility(node_id, ancestry, state);
        if visible {
            if hidden_by_us.contains(&entity) {
                to_restore.push(entity);
            }
        } else {
            to_hide.push(entity);
            if selected_node_id == Some(node_id) {
                deselect = true;
            }
        }
    }

    SyncDecision {
        to_hide,
        to_restore,
        deselect,
    }
}

/// Applies the resolver to every spawned node entity, using each entity's OWN
/// petal ancestry (F12 — via `marker.petal_id`, not the active petal blanket-
/// applied to everything). fe-ui's spawn helpers never insert a `Visibility`
/// component (`verse_manager/spawn.rs` — confirmed zero hits for `Visibility`
/// there), so on first-hide this system inserts it fresh via `Commands`.
///
/// Gated on `VisibilityState::epoch` (F3a) + petal switch — NOT
/// `ResMut::is_changed()`, which `gardener_ui_system` permanently defeats by
/// dereffing `&mut VisibilityState` every frame regardless of mutation. Also
/// clears the transient `solo` lens on petal switch (spec §3.1).
///
/// Minimal-authority ownership (F3b): tracks which entities THIS system hid
/// in `hidden_by_us` and only ever writes `Visibility::Inherited` back onto
/// entities in that set — never onto an entity another writer (e.g. GPX
/// ribbons carrying both `SpawnedNodeMarker` and terrain's own visibility
/// writer) owns. Entries are pruned once their entity despawns.
///
/// D-13: force-deselects `NodeManager.selected` the same frame its node
/// becomes effectively hidden.
///
/// NOTE (documented, matches RATIFICATION #7's accepted consequence):
/// pickers / camera-focus / gimbal do not yet consult the resolver — hidden
/// nodes stay pickable until Phase 2 wires the filters (separate work this
/// wave).
pub(crate) fn sync_node_visibility(
    mut state: ResMut<VisibilityState>,
    nav: Res<NavigationManager>,
    hierarchy: Res<VerseManager>,
    spawned: Query<(Entity, &SpawnedNodeMarker)>,
    mut commands: Commands,
    mut node_mgr: ResMut<NodeManager>,
    mut last_petal: Local<Option<String>>,
    mut last_epoch: Local<u64>,
    mut hidden_by_us: Local<HashSet<Entity>>,
) {
    let petal_switched = *last_petal != nav.active_petal_id;
    if petal_switched {
        *last_petal = nav.active_petal_id.clone();
        state.clear_solo();
    }

    if !should_apply(petal_switched, *last_epoch, state.epoch()) {
        return;
    }
    *last_epoch = state.epoch();

    // F3b: drop tracked entities that despawned since the last pass (e.g. a
    // petal-switch respawn) so the set can't grow unbounded or shadow a
    // recycled `Entity` id.
    hidden_by_us.retain(|e| spawned.contains(*e));

    let petal_ancestry = build_petal_ancestry_index(&hierarchy);
    let entities: Vec<(Entity, &str, &str)> = spawned
        .iter()
        .map(|(entity, marker)| (entity, marker.node_id.as_str(), marker.petal_id.as_str()))
        .collect();
    let selected_node_id = node_mgr.selected.as_ref().map(|s| s.node_id.as_str());

    let decision = compute_sync_decision(
        &entities,
        &petal_ancestry,
        &hidden_by_us,
        &state,
        selected_node_id,
    );

    for entity in decision.to_hide {
        commands.entity(entity).insert(Visibility::Hidden);
        hidden_by_us.insert(entity);
    }
    for entity in decision.to_restore {
        commands.entity(entity).insert(Visibility::Inherited);
        hidden_by_us.remove(&entity);
    }
    if decision.deselect {
        node_mgr.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};

    fn two_petal_hierarchy() -> VerseManager {
        VerseManager::from_verses(vec![VerseEntry {
            id: "v1".into(),
            name: "Verse".into(),
            fractals: vec![
                FractalEntry {
                    id: "f1".into(),
                    name: "Fractal 1".into(),
                    petals: vec![PetalEntry {
                        id: "p1".into(),
                        name: "Petal 1".into(),
                        nodes: vec![
                            NodeEntry {
                                id: "n1".into(),
                                name: "N1".into(),
                                ..Default::default()
                            },
                            NodeEntry {
                                id: "n2".into(),
                                name: "N2".into(),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                FractalEntry {
                    id: "f2".into(),
                    name: "Fractal 2".into(),
                    petals: vec![PetalEntry {
                        id: "p2".into(),
                        name: "Petal 2".into(),
                        nodes: vec![NodeEntry {
                            id: "n3".into(),
                            name: "N3".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }])
    }

    // --- hidden_count_in_active_petal (pre-existing) ---

    #[test]
    fn hidden_count_zero_when_no_active_petal() {
        let hierarchy = VerseManager::default();
        let nav = NavigationManager::default();
        let state = VisibilityState::default();
        assert_eq!(hidden_count_in_active_petal(&hierarchy, &nav, &state), 0);
    }

    #[test]
    fn hidden_count_zero_when_active_petal_not_found() {
        let hierarchy = VerseManager::default();
        let nav = NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f1".into()),
            active_petal_id: Some("missing-petal".into()),
            ..Default::default()
        };
        let state = VisibilityState::default();
        assert_eq!(hidden_count_in_active_petal(&hierarchy, &nav, &state), 0);
    }

    #[test]
    fn hidden_count_matches_effectively_hidden_nodes_in_active_petal() {
        let petal = PetalEntry {
            id: "p1".into(),
            name: "Petal".into(),
            nodes: vec![
                NodeEntry {
                    id: "n1".into(),
                    name: "N1".into(),
                    ..Default::default()
                },
                NodeEntry {
                    id: "n2".into(),
                    name: "N2".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let hierarchy = VerseManager::from_verses(vec![VerseEntry {
            id: "v1".into(),
            name: "Verse".into(),
            fractals: vec![FractalEntry {
                id: "f1".into(),
                name: "Fractal".into(),
                petals: vec![petal],
                ..Default::default()
            }],
            ..Default::default()
        }]);
        let nav = NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f1".into()),
            active_petal_id: Some("p1".into()),
            ..Default::default()
        };
        let mut state = VisibilityState::default();
        assert_eq!(hidden_count_in_active_petal(&hierarchy, &nav, &state), 0);

        state.set_node_override("n1", OverrideState::Hide);
        assert_eq!(hidden_count_in_active_petal(&hierarchy, &nav, &state), 1);

        state.toggle_petal_hidden("p1");
        // n1 stays hidden (override), n2 now hidden via ancestor — both count.
        assert_eq!(hidden_count_in_active_petal(&hierarchy, &nav, &state), 2);
    }

    // --- F3a: epoch bumps on every mutator ---

    #[test]
    fn toggle_verse_hidden_bumps_epoch() {
        let mut state = VisibilityState::default();
        assert_eq!(state.epoch(), 0);
        state.toggle_verse_hidden("v1");
        assert_eq!(state.epoch(), 1);
        state.toggle_verse_hidden("v1");
        assert_eq!(state.epoch(), 2);
    }

    #[test]
    fn toggle_fractal_hidden_bumps_epoch() {
        let mut state = VisibilityState::default();
        state.toggle_fractal_hidden("f1");
        assert_eq!(state.epoch(), 1);
    }

    #[test]
    fn toggle_petal_hidden_bumps_epoch() {
        let mut state = VisibilityState::default();
        state.toggle_petal_hidden("p1");
        assert_eq!(state.epoch(), 1);
    }

    #[test]
    fn set_node_override_bumps_epoch_and_round_trips() {
        let mut state = VisibilityState::default();
        assert_eq!(state.node_override("n1"), OverrideState::Auto);
        state.set_node_override("n1", OverrideState::Hide);
        assert_eq!(state.epoch(), 1);
        assert_eq!(state.node_override("n1"), OverrideState::Hide);
        state.set_node_override("n1", OverrideState::Auto);
        assert_eq!(state.epoch(), 2);
        assert_eq!(state.node_override("n1"), OverrideState::Auto);
    }

    #[test]
    fn set_solo_bumps_epoch() {
        let mut state = VisibilityState::default();
        state.set_solo(HashSet::from(["n1".to_string()]));
        assert_eq!(state.epoch(), 1);
        assert!(state.solo().is_some());
    }

    #[test]
    fn clear_solo_is_a_noop_when_already_clear() {
        let mut state = VisibilityState::default();
        assert_eq!(state.epoch(), 0);
        state.clear_solo();
        // No epoch bump — petal-switch churn calls this unconditionally and
        // must not manufacture spurious "something changed" signals.
        assert_eq!(state.epoch(), 0);
    }

    #[test]
    fn clear_solo_bumps_epoch_when_it_was_active() {
        let mut state = VisibilityState::default();
        state.set_solo(HashSet::from(["n1".to_string()]));
        assert_eq!(state.epoch(), 1);
        state.clear_solo();
        assert_eq!(state.epoch(), 2);
        assert!(state.solo().is_none());
    }

    #[test]
    fn group_mutators_bump_epoch() {
        let mut state = VisibilityState::default();
        state.add_group(Group {
            id: "g1".into(),
            name: "Utilities".into(),
            visible: true,
        });
        assert_eq!(state.epoch(), 1);
        state.set_group_visible("g1", false);
        assert_eq!(state.epoch(), 2);
        state.set_node_group_membership("n1", vec!["g1".into()]);
        assert_eq!(state.epoch(), 3);
        state.remove_group("g1");
        assert_eq!(state.epoch(), 4);
    }

    // --- F3a: no-write-without-epoch-change (the sync gate itself) ---

    #[test]
    fn should_apply_false_when_nothing_changed() {
        assert!(!should_apply(false, 5, 5));
    }

    #[test]
    fn should_apply_true_on_epoch_change() {
        assert!(should_apply(false, 5, 6));
    }

    #[test]
    fn should_apply_true_on_petal_switch_even_without_epoch_change() {
        assert!(should_apply(true, 5, 5));
    }

    // --- F12: per-entity ancestry ---

    #[test]
    fn build_petal_ancestry_index_maps_every_petal_to_its_fractal_and_verse() {
        let hierarchy = two_petal_hierarchy();
        let index = build_petal_ancestry_index(&hierarchy);
        assert_eq!(index.get("p1"), Some(&("f1".to_string(), "v1".to_string())));
        assert_eq!(index.get("p2"), Some(&("f2".to_string(), "v1".to_string())));
    }

    #[test]
    fn compute_sync_decision_uses_each_entitys_own_petal_ancestry() {
        // F12 regression: previously every entity was evaluated against the
        // ACTIVE petal's ancestry regardless of which petal it belonged to.
        // Here we hide fractal f1 (which owns p1, not p2) and prove only the
        // p1 entity gets hidden — with no "active petal" concept involved at
        // all in this pure core.
        let hierarchy = two_petal_hierarchy();
        let petal_ancestry = build_petal_ancestry_index(&hierarchy);
        let mut state = VisibilityState::default();
        state.toggle_fractal_hidden("f1");

        let entities = [
            (Entity::from_bits(1), "n1", "p1"),
            (Entity::from_bits(2), "n3", "p2"),
        ];
        let decision =
            compute_sync_decision(&entities, &petal_ancestry, &HashSet::new(), &state, None);

        assert_eq!(decision.to_hide, vec![Entity::from_bits(1)]);
        assert!(decision.to_restore.is_empty());
    }

    #[test]
    fn compute_sync_decision_unknown_petal_falls_back_to_no_ancestor_hide() {
        // A marker whose petal_id isn't in the index (stale/orphaned) should
        // still resolve via node override / solo, just without ancestor-chain
        // participation — not panic, not silently vanish.
        let state = VisibilityState::default();
        let entities = [(Entity::from_bits(1), "n1", "missing-petal")];
        let decision =
            compute_sync_decision(&entities, &HashMap::new(), &HashSet::new(), &state, None);
        assert!(decision.to_hide.is_empty());
    }

    // --- F3b: minimal-authority hidden-set ownership ---

    #[test]
    fn compute_sync_decision_hides_and_tracks_newly_hidden_entities() {
        let mut state = VisibilityState::default();
        state.set_node_override("n1", OverrideState::Hide);
        let entities = [(Entity::from_bits(1), "n1", "p1")];
        let decision =
            compute_sync_decision(&entities, &HashMap::new(), &HashSet::new(), &state, None);
        assert_eq!(decision.to_hide, vec![Entity::from_bits(1)]);
    }

    #[test]
    fn compute_sync_decision_restores_only_entities_it_previously_hid() {
        // e1 was hidden by US last pass; e2 is resolver-visible too but was
        // NEVER in our hidden-by-us set (simulating another writer, e.g. a
        // GPX ribbon's terrain visibility writer, owning its Hidden state).
        // Both are resolver-visible now — only e1 should come back.
        let state = VisibilityState::default();
        let hidden_by_us = HashSet::from([Entity::from_bits(1)]);
        let entities = [
            (Entity::from_bits(1), "n1", "p1"),
            (Entity::from_bits(2), "n2", "p1"),
        ];
        let decision =
            compute_sync_decision(&entities, &HashMap::new(), &hidden_by_us, &state, None);

        assert_eq!(decision.to_restore, vec![Entity::from_bits(1)]);
        assert!(decision.to_hide.is_empty());
    }

    #[test]
    fn compute_sync_decision_never_restores_an_entity_it_did_not_hide() {
        let state = VisibilityState::default();
        let entities = [(Entity::from_bits(9), "n9", "p1")];
        let decision = compute_sync_decision(
            &entities,
            &HashMap::new(),
            &HashSet::new(), // empty — we never hid entity 9
            &state,
            None,
        );
        assert!(decision.to_restore.is_empty());
    }

    // --- D-13: force-deselect on hide ---

    #[test]
    fn compute_sync_decision_force_deselects_when_selected_node_becomes_hidden() {
        let mut state = VisibilityState::default();
        state.set_node_override("n1", OverrideState::Hide);
        let entities = [(Entity::from_bits(1), "n1", "p1")];
        let decision = compute_sync_decision(
            &entities,
            &HashMap::new(),
            &HashSet::new(),
            &state,
            Some("n1"),
        );
        assert!(decision.deselect);
    }

    #[test]
    fn compute_sync_decision_no_deselect_when_selected_node_stays_visible() {
        let state = VisibilityState::default();
        let entities = [(Entity::from_bits(1), "n1", "p1")];
        let decision = compute_sync_decision(
            &entities,
            &HashMap::new(),
            &HashSet::new(),
            &state,
            Some("n1"),
        );
        assert!(!decision.deselect);
    }

    #[test]
    fn compute_sync_decision_no_deselect_when_nothing_selected() {
        let mut state = VisibilityState::default();
        state.set_node_override("n1", OverrideState::Hide);
        let entities = [(Entity::from_bits(1), "n1", "p1")];
        let decision =
            compute_sync_decision(&entities, &HashMap::new(), &HashSet::new(), &state, None);
        assert!(!decision.deselect);
    }

    // --- F19: memoized hidden count ---

    #[test]
    fn hidden_count_cached_matches_uncached_and_invalidates_on_epoch_change() {
        let hierarchy = two_petal_hierarchy();
        let nav = NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f1".into()),
            active_petal_id: Some("p1".into()),
            ..Default::default()
        };
        let mut state = VisibilityState::default();

        assert_eq!(
            state.hidden_count_in_active_petal_cached(&hierarchy, &nav),
            0
        );
        assert_eq!(
            state.hidden_count_cache,
            Some((0, Some("p1".to_string()), 0))
        );

        state.set_node_override("n1", OverrideState::Hide);
        // Cache must invalidate on epoch change, not keep serving the stale 0.
        assert_eq!(
            state.hidden_count_in_active_petal_cached(&hierarchy, &nav),
            1
        );
        assert_eq!(
            state.hidden_count_cache,
            Some((1, Some("p1".to_string()), 1))
        );

        // Same epoch, same petal — cache hit, still correct.
        assert_eq!(
            state.hidden_count_in_active_petal_cached(&hierarchy, &nav),
            1
        );
    }

    #[test]
    fn hidden_count_cached_invalidates_on_active_petal_change() {
        let hierarchy = two_petal_hierarchy();
        let mut state = VisibilityState::default();
        state.toggle_fractal_hidden("f1"); // hides p1's only fractal, not p2's

        let nav_p1 = NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f1".into()),
            active_petal_id: Some("p1".into()),
            ..Default::default()
        };
        let nav_p2 = NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f2".into()),
            active_petal_id: Some("p2".into()),
            ..Default::default()
        };

        assert_eq!(
            state.hidden_count_in_active_petal_cached(&hierarchy, &nav_p1),
            2
        );
        // Same epoch, different active petal — must not reuse p1's cached 2.
        assert_eq!(
            state.hidden_count_in_active_petal_cached(&hierarchy, &nav_p2),
            0
        );
    }
}
