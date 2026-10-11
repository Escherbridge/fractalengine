//! Hierarchy visibility + groups (`hierarchy_visibility_groups_20260808`).
//! `resolve.rs` is the pure lattice, [`pickable`] the one picker rule,
//! `cache.rs` the `view.state`/`view.groups` persistence, `groups.rs` the
//! persisted Group registry + scope eyes. Rationale, the
//! picker list and what is still session-only: `fe-ui/src/AGENTS.md`
//! §visibility. Mutate only through the `VisibilityState` methods (they bump
//! `epoch`, finding F3a).

pub mod cache;
pub mod groups;
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
/// lattice — session-only, cleared on petal switch and by the Escape ladder's
/// `ClearIsolate` rung in `node_manager/shortcuts.rs`). A set of node ids, not
/// scopes — see spec §3.1.
#[derive(Debug, Clone, Default)]
pub struct SoloSet {
    pub members: HashSet<String>,
}

/// A visibility Group row of the active petal's persisted registry
/// (`petal.visibility_groups`, `groups.rs`). The user-facing word is "Group"
/// (RATIFICATION #18) — use it in all copy, never "Layer" (collides with
/// terrain layers) or "View set".
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub visible: bool,
}

/// Visibility state: per-node tri-state overrides, hidden petal/fractal/verse
/// id sets, the active petal's Group registry + node membership (all mirrors
/// of persisted values, hydrated by `cache.rs` / `groups.rs`), and the
/// session-only solo lens.
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
    /// The active petal's Group registry, in display order (`groups.rs`).
    pub(crate) groups: Vec<Group>,
    /// node_id -> group ids it belongs to (persisted `view.groups`, `cache.rs`).
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

    /// Toggles whether `verse_id` is hidden, session-only (tests / fixtures;
    /// the sidebar eye persists through `groups::set_scope_hidden`). Bumps `epoch`.
    pub fn toggle_verse_hidden(&mut self, verse_id: &str) {
        toggle_scope_hidden(&mut self.hidden_verses, verse_id);
        self.bump_epoch();
    }

    /// Toggles whether `fractal_id` is hidden, session-only (tests / fixtures;
    /// the sidebar eye persists through `groups::set_scope_hidden`). Bumps `epoch`.
    pub fn toggle_fractal_hidden(&mut self, fractal_id: &str) {
        toggle_scope_hidden(&mut self.hidden_fractals, fractal_id);
        self.bump_epoch();
    }

    /// Toggles whether `petal_id` is hidden, session-only (tests / fixtures;
    /// the sidebar eye persists through `groups::set_scope_hidden`). Bumps `epoch`.
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

    /// Activates the isolate/solo lens over exactly `members` (Alt+H, row-menu
    /// "Isolate"). Bumps `epoch`.
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

    /// The active petal's Group registry, in display order.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Replaces the whole Group registry (load, echo, or a local edit). A
    /// no-op (no `epoch` bump) when nothing changed. Membership is untouched —
    /// an id no longer in the registry simply stops hiding anything.
    pub fn replace_groups(&mut self, groups: Vec<Group>) {
        if self.groups != groups {
            self.groups = groups;
            self.bump_epoch();
        }
    }

    /// How many nodes list `group_id` in their membership.
    pub fn group_member_count(&self, group_id: &str) -> usize {
        self.group_membership
            .values()
            .filter(|ids| ids.iter().any(|id| id == group_id))
            .count()
    }

    /// `node_id`'s Group ids (empty when it has none).
    pub fn node_groups(&self, node_id: &str) -> &[String] {
        self.group_membership
            .get(node_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Whether a verse/fractal/petal eye is closed.
    pub fn is_scope_hidden(&self, level: fe_runtime::messages::ScopeLevel, id: &str) -> bool {
        self.hidden_set(level).contains(id)
    }

    /// Opens/closes one verse/fractal/petal eye; bumps `epoch` only on change.
    pub fn set_scope_hidden(
        &mut self,
        level: fe_runtime::messages::ScopeLevel,
        id: &str,
        hidden: bool,
    ) {
        let set = self.hidden_set_mut(level);
        let changed = if hidden {
            set.insert(id.to_string())
        } else {
            set.remove(id)
        };
        if changed {
            self.bump_epoch();
        }
    }

    /// Replaces all three hidden-scope sets (persisted eyes from
    /// `HierarchyLoaded`); bumps `epoch` only on change.
    pub fn replace_hidden_scopes(
        &mut self,
        verses: HashSet<String>,
        fractals: HashSet<String>,
        petals: HashSet<String>,
    ) {
        if self.hidden_verses != verses
            || self.hidden_fractals != fractals
            || self.hidden_petals != petals
        {
            self.hidden_verses = verses;
            self.hidden_fractals = fractals;
            self.hidden_petals = petals;
            self.bump_epoch();
        }
    }

    fn hidden_set(&self, level: fe_runtime::messages::ScopeLevel) -> &HashSet<String> {
        use fe_runtime::messages::ScopeLevel;
        match level {
            ScopeLevel::Verse => &self.hidden_verses,
            ScopeLevel::Fractal => &self.hidden_fractals,
            ScopeLevel::Petal => &self.hidden_petals,
        }
    }

    fn hidden_set_mut(&mut self, level: fe_runtime::messages::ScopeLevel) -> &mut HashSet<String> {
        use fe_runtime::messages::ScopeLevel;
        match level {
            ScopeLevel::Verse => &mut self.hidden_verses,
            ScopeLevel::Fractal => &mut self.hidden_fractals,
            ScopeLevel::Petal => &mut self.hidden_petals,
        }
    }

    /// Registers a new visibility Group locally (tests / fixtures; production
    /// edits go through `groups::set_group_registry`). Bumps `epoch`.
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

    /// "Show all" for one branch (Shift+H / row menu): drops every `Hide`
    /// override among `node_ids` and their tracks' stamp instances, un-hides
    /// the branch's petal/fractal/verse and clears solo, in ONE epoch bump.
    /// Returns the ids whose `Hide` it cleared (stamp ids last, sorted) so the
    /// caller can persist the node ones as Auto. `Show` overrides and group
    /// visibility are left alone.
    pub fn show_all_in<'a>(
        &mut self,
        ancestry: NodeAncestry,
        node_ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let node_ids: Vec<&str> = node_ids.into_iter().collect();
        let mut cleared: Vec<String> = node_ids
            .iter()
            .filter(|id| self.node_override(id) == OverrideState::Hide)
            .map(|id| id.to_string())
            .collect();
        let mut stamp_ids: Vec<String> = self
            .node_overrides
            .iter()
            .filter(|(id, state)| **state == OverrideState::Hide && stamp_track_in(id, &node_ids))
            .map(|(id, _)| id.clone())
            .collect();
        stamp_ids.sort();
        cleared.extend(stamp_ids);
        for id in &cleared {
            self.node_overrides.remove(id);
        }
        self.hidden_petals.remove(ancestry.petal_id);
        self.hidden_fractals.remove(ancestry.fractal_id);
        self.hidden_verses.remove(ancestry.verse_id);
        self.solo = None;
        self.bump_epoch();
        cleared
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

/// Whether `id` is a stamp-instance marker of one of `track_ids` (stamps are
/// session-only overrides — they have no node row to persist to).
pub(crate) fn stamp_track_in(id: &str, track_ids: &[&str]) -> bool {
    crate::verse_manager::parse_stamp_marker_id(id)
        .is_some_and(|(track, _)| track_ids.contains(&track))
}

/// The active petal's ancestry from the navigation cursor (missing levels =
/// `""`, which no hidden set contains) — the construction every picker shares.
pub(crate) fn active_ancestry(nav: &NavigationManager) -> NodeAncestry<'_> {
    NodeAncestry {
        verse_id: nav.active_verse_id.as_deref().unwrap_or(""),
        fractal_id: nav.active_fractal_id.as_deref().unwrap_or(""),
        petal_id: nav.active_petal_id.as_deref().unwrap_or(""),
    }
}

/// THE picker rule (D-7/D-12/D-13/D-14): an active-petal node may be picked,
/// right-clicked, sidebar-selected, path-edited, snapped to or camera-focused
/// only while it is effectively visible. Every surface calls this; none
/// re-derives it.
pub(crate) fn pickable(node_id: &str, nav: &NavigationManager, vis: &VisibilityState) -> bool {
    effective_visibility(node_id, active_ancestry(nav), vis)
}

/// Whether `visibility` (an entity's own component) leaves it drawn. An entity
/// hidden outside the lattice — a GPX ribbon whose map layer or style is off —
/// is no pick target either (D-12: nothing invisible is clickable).
pub(crate) fn rendered(visibility: Option<&Visibility>) -> bool {
    !matches!(visibility, Some(Visibility::Hidden))
}

/// "Hide others" targets: every node of the petal except `keep` that is not
/// already overridden to `Hide`. Pure.
fn hide_others_targets<'a>(
    petal_node_ids: impl IntoIterator<Item = &'a str>,
    keep: &str,
    vis: &VisibilityState,
) -> Vec<String> {
    petal_node_ids
        .into_iter()
        .filter(|id| *id != keep && vis.node_override(id) != OverrideState::Hide)
        .map(str::to_string)
        .collect()
}

/// "Show all" producer shared by Shift+H and the sidebar row menu: the session
/// clear ([`VisibilityState::show_all_in`]) plus one persisted Auto write per
/// Hide override it removed.
pub(crate) fn show_all_and_persist<'a>(
    vis: &mut VisibilityState,
    ancestry: NodeAncestry,
    node_ids: impl IntoIterator<Item = &'a str>,
    ui_mgr: &mut crate::actions::UiManager,
) {
    use fe_runtime::messages::ScopeLevel;
    // The branch's closed ancestor eyes are persisted (VIS-PH4), so their
    // re-open is persisted too — otherwise a restart would re-hide them.
    for (level, id) in [
        (ScopeLevel::Verse, ancestry.verse_id),
        (ScopeLevel::Fractal, ancestry.fractal_id),
        (ScopeLevel::Petal, ancestry.petal_id),
    ] {
        if vis.is_scope_hidden(level, id) {
            ui_mgr.push_action(crate::actions::UiAction::SetScopeHidden {
                level,
                id: id.to_string(),
                hidden: false,
            });
        }
    }
    let cleared = vis.show_all_in(ancestry, node_ids);
    // Stamp-instance overrides are session-only: nothing to persist.
    for node_id in cleared
        .into_iter()
        .filter(|id| crate::verse_manager::parse_stamp_marker_id(id).is_none())
    {
        ui_mgr.push_action(crate::actions::UiAction::SetNodeView {
            node_id,
            state: OverrideState::Auto,
        });
    }
}

/// "Hide others" producer (sidebar row menu): one persisted Hide write per
/// [`hide_others_targets`] node.
pub(crate) fn hide_others_and_persist<'a>(
    vis: &VisibilityState,
    petal_node_ids: impl IntoIterator<Item = &'a str>,
    keep: &str,
    ui_mgr: &mut crate::actions::UiManager,
) {
    for node_id in hide_others_targets(petal_node_ids, keep, vis) {
        ui_mgr.push_action(crate::actions::UiAction::SetNodeView {
            node_id,
            state: OverrideState::Hide,
        });
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
/// becomes effectively hidden. Pickers/camera-focus filter via [`pickable`].
///
/// Also re-applies when node entities spawn (late GLTF/ribbon/stamp spawns
/// after the petal-switch pass) and re-asserts the hide on an entity it hid
/// that another writer re-showed (Layer Manager toggle, ribbon build) — see
/// `fe-ui/src/AGENTS.md` §visibility.
#[allow(clippy::too_many_arguments)] // one apply system; each param is one authority or read
pub(crate) fn sync_node_visibility(
    mut state: ResMut<VisibilityState>,
    nav: Res<NavigationManager>,
    hierarchy: Res<VerseManager>,
    spawned: Query<(Entity, &SpawnedNodeMarker)>,
    newly_spawned: Query<(), Added<SpawnedNodeMarker>>,
    reshown: Query<(Entity, &Visibility), Changed<Visibility>>,
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

    for (entity, visibility) in &reshown {
        if *visibility != Visibility::Hidden && hidden_by_us.contains(&entity) {
            commands.entity(entity).insert(Visibility::Hidden);
        }
    }

    if !should_apply(
        petal_switched || !newly_spawned.is_empty(),
        *last_epoch,
        state.epoch(),
    ) {
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

    /// The real apply system: a node spawned AFTER the petal-switch pass (late
    /// GLTF / ribbon spawn, restart hydration first) is still hidden, and a
    /// hidden ribbon re-shown by another writer (Layer Manager toggle) is
    /// re-hidden; a visible node stays untouched by both rules.
    #[test]
    fn late_spawns_are_hidden_and_a_foreign_reshow_is_reverted() {
        let mut app = App::new();
        let mut state = VisibilityState::default();
        state.set_node_override("n1", OverrideState::Hide);
        app.insert_resource(state)
            .insert_resource(two_petal_hierarchy())
            .insert_resource(NavigationManager {
                active_verse_id: Some("v1".into()),
                active_fractal_id: Some("f1".into()),
                active_petal_id: Some("p1".into()),
                ..Default::default()
            })
            .init_resource::<NodeManager>()
            .add_systems(Update, sync_node_visibility);
        app.update(); // petal-entry pass: nothing spawned yet

        let marker = |node_id: &str| SpawnedNodeMarker {
            node_id: node_id.into(),
            petal_id: "p1".into(),
        };
        let hidden = app.world_mut().spawn(marker("n1")).id();
        let shown = app.world_mut().spawn(marker("n2")).id();
        app.update();
        let visibility = |app: &App, e: Entity| app.world().get::<Visibility>(e).copied();
        assert_eq!(
            visibility(&app, hidden),
            Some(Visibility::Hidden),
            "late spawn"
        );
        assert_eq!(visibility(&app, shown), None, "visible node untouched");

        // A layer toggle writes Inherited onto every layer-bound entity.
        app.world_mut()
            .entity_mut(hidden)
            .insert(Visibility::Inherited);
        app.world_mut().entity_mut(shown).insert(Visibility::Hidden);
        app.update();
        assert_eq!(
            visibility(&app, hidden),
            Some(Visibility::Hidden),
            "re-hidden"
        );
        assert_eq!(
            visibility(&app, shown),
            Some(Visibility::Hidden),
            "another writer's hide is never overridden"
        );
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

    // --- Phase 2: the shared picker rule + Show all / Hide others ---

    fn nav_in_p1() -> NavigationManager {
        NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f1".into()),
            active_petal_id: Some("p1".into()),
            ..Default::default()
        }
    }

    #[test]
    fn pickable_refuses_every_effectively_hidden_cause() {
        let nav = nav_in_p1();
        type Setup = fn(&mut VisibilityState);
        let cases: [(&str, Setup, bool); 6] = [
            ("nothing hidden", |_| {}, true),
            (
                "own Hide override",
                |s| s.set_node_override("n1", OverrideState::Hide),
                false,
            ),
            (
                "hidden petal ancestor",
                |s| s.toggle_petal_hidden("p1"),
                false,
            ),
            (
                "hidden verse ancestor",
                |s| s.toggle_verse_hidden("v1"),
                false,
            ),
            (
                "solo excludes it",
                |s| s.set_solo(HashSet::from(["n2".to_string()])),
                false,
            ),
            (
                "Show override beats a hidden ancestor",
                |s| {
                    s.toggle_fractal_hidden("f1");
                    s.set_node_override("n1", OverrideState::Show);
                },
                true,
            ),
        ];
        for (label, setup, expected) in cases {
            let mut state = VisibilityState::default();
            setup(&mut state);
            assert_eq!(pickable("n1", &nav, &state), expected, "{label}");
        }
    }

    #[test]
    fn show_all_in_clears_the_branch_hides_and_solo_in_one_epoch_and_reports_cleared_ids() {
        let mut state = VisibilityState::default();
        state.set_node_override("n1", OverrideState::Hide);
        state.set_node_override("n2", OverrideState::Show);
        state.set_node_override("other-petal-node", OverrideState::Hide);
        state.toggle_petal_hidden("p1");
        state.toggle_fractal_hidden("f1");
        state.toggle_verse_hidden("v1");
        state.toggle_fractal_hidden("f2"); // another branch — untouched
        state.set_solo(HashSet::from(["n2".to_string()]));
        let before = state.epoch();

        let cleared = state.show_all_in(active_ancestry(&nav_in_p1()), ["n1", "n2"]);

        assert_eq!(cleared, vec!["n1".to_string()], "only Hide overrides clear");
        assert_eq!(state.epoch(), before + 1);
        assert_eq!(state.node_override("n2"), OverrideState::Show);
        assert_eq!(
            state.node_override("other-petal-node"),
            OverrideState::Hide,
            "nodes outside the branch keep their override"
        );
        assert!(state.solo().is_none());
        assert!(pickable("n1", &nav_in_p1(), &state));
        assert!(state.hidden_fractals.contains("f2"));
    }

    #[test]
    fn show_all_also_restores_hidden_stamps_of_the_branch_tracks_without_persisting_them() {
        let mut state = VisibilityState::default();
        state.set_node_override("t1", OverrideState::Hide);
        state.set_node_override("t1::stamp::3", OverrideState::Hide);
        state.set_node_override("elsewhere::stamp::0", OverrideState::Hide);
        let mut ui_mgr = crate::actions::UiManager::default();

        show_all_and_persist(
            &mut state,
            active_ancestry(&nav_in_p1()),
            ["t1"],
            &mut ui_mgr,
        );

        assert!(pickable("t1::stamp::3", &nav_in_p1(), &state));
        assert_eq!(
            state.node_override("elsewhere::stamp::0"),
            OverrideState::Hide,
            "a stamp of a track outside the branch keeps its override"
        );
        let writes: Vec<String> = ui_mgr
            .drain_actions()
            .into_iter()
            .filter_map(|a| match a {
                crate::actions::UiAction::SetNodeView { node_id, .. } => Some(node_id),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes,
            vec!["t1".to_string()],
            "stamps have no row to persist"
        );
    }

    #[test]
    fn hide_others_persists_a_hide_for_every_other_not_yet_hidden_node() {
        let mut state = VisibilityState::default();
        state.set_node_override("n3", OverrideState::Hide);
        let mut ui_mgr = crate::actions::UiManager::default();

        hide_others_and_persist(&state, ["n1", "n2", "n3", "n4"], "n1", &mut ui_mgr);

        let writes: Vec<(String, OverrideState)> = ui_mgr
            .drain_actions()
            .into_iter()
            .filter_map(|a| match a {
                crate::actions::UiAction::SetNodeView { node_id, state } => Some((node_id, state)),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes,
            vec![
                ("n2".to_string(), OverrideState::Hide),
                ("n4".to_string(), OverrideState::Hide),
            ],
            "the kept node and the already-hidden n3 get no write"
        );
    }
}
