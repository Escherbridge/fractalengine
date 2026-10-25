//! Pure visibility-lattice resolver — no Bevy, no I/O, fully unit-tested.
//! Implements the RATIFIED compose precedence from
//! `conductor/tracks/hierarchy_visibility_groups_20260808/spec.md` §3.1 /
//! RATIFICATION #5:
//!
//!   solo (transient lens, outermost) > per-node override (Show/Hide) >
//!   GPX-HIDE-CASCADE (a stamp follows its owning track) >
//!   ancestor chain (verse/fractal/petal) > groups (ANY-hides)
//!
//! `Show`/`Hide` beat the ancestor chain AND groups — the ratified "escape
//! hatch" (RATIFICATION table row 5: "override > ancestor chain >
//! ANY-containing-hidden-group hides"), which supersedes the earlier draft's
//! "ancestor chain is non-negotiable" language elsewhere in the spec.
//!
//! GPX-HIDE-CASCADE (`decisions_20261012_impl_20261015` spec A10/A11): a
//! stamp instance's own override (Auto) defers to its OWNING TRACK's own
//! override Hide or hidden-Group membership — never to the track's ancestor
//! chain, which already hides the stamp directly since they share the same
//! petal, and never to solo, which A10 does not extend from a soloed track
//! to its stamps. [`owning_track_id`] finds the track from either the
//! stamp's spawned marker id or, once promoted, the node-id alias A11 wires
//! up (`VisibilityState::promoted_stamp_alias`); [`lattice_hidden_reason`] is
//! the one-level, non-recursive lookup of the track's own state.
//!
//! Consumed by: the sidebar eye renderer (`panels/sidebar.rs`), the
//! `sync_node_visibility` apply system (`visibility/mod.rs`), and — from
//! Phase 2 onward — every picker + camera-focus path (spec RATIFICATION #7).
//! One resolver; no surface re-derives it (spec §5.1 rule 2).

use std::collections::HashSet;

use super::{Group, OverrideState, VisibilityState};

/// Verse/fractal/petal ids a node belongs to. All `SpawnedNodeMarker`
/// entities in the running app belong to the single active petal
/// (respawn-on-switch despawns everything else — see
/// `verse_manager/petal_respawn.rs`), so callers typically build one of these
/// per frame from `NavigationManager` and reuse it for every node.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeAncestry<'a> {
    pub verse_id: &'a str,
    pub fractal_id: &'a str,
    pub petal_id: &'a str,
}

/// Why a node is currently hidden (or not) — drives both the sidebar tooltip
/// and `toggle_node_override`'s reason-aware click cycle. `NotHidden` is the
/// only "effectively visible" case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenReason {
    NotHidden,
    /// Isolate/solo is active and this node isn't a member (spec §3.1: "solo
    /// overlay outermost ... overrides do NOT beat solo").
    Solo,
    /// The node's own tri-state override is `Hide`.
    OverrideHide,
    /// A verse/fractal/petal ancestor is hidden and the override is `Auto`.
    Ancestor,
    /// Membership in a hidden Group (ANY-hides) and the override is `Auto`.
    /// Carries the group's display name for the sidebar tooltip.
    Group(String),
    /// GPX-HIDE-CASCADE (A10): a stamp whose own override is `Auto` and
    /// whose owning track is itself hidden by the track's own `Hide`
    /// override or a hidden Group it belongs to. Carries the track's node
    /// id for the sidebar tooltip. Never produced for the track's ancestor
    /// chain (A10: that already hides the stamp directly, same petal) or
    /// for solo (A10: not extended to a soloed track's stamps).
    OwningPath(String),
}

/// The one pure resolver every consumer must call — see module docs for the
/// precedence order. Delegates to `node_hidden_reason` so the two can never
/// drift apart.
pub fn effective_visibility(
    node_id: &str,
    ancestry: NodeAncestry,
    state: &VisibilityState,
) -> bool {
    node_hidden_reason(node_id, ancestry, state) == HiddenReason::NotHidden
}

/// Same lattice as `effective_visibility`, but returns *why* — powers the
/// sidebar's honest tooltip ("hidden by Group 'x'" / "hidden with ancestors")
/// and `toggle_node_override`'s reason-aware click cycle.
pub fn node_hidden_reason(
    node_id: &str,
    ancestry: NodeAncestry,
    state: &VisibilityState,
) -> HiddenReason {
    // 1. Solo overlay — outermost, overrides do NOT beat it.
    if let Some(solo) = &state.solo {
        return if solo.members.contains(node_id) {
            HiddenReason::NotHidden
        } else {
            HiddenReason::Solo
        };
    }

    // 2. Per-node tri-state override — Show/Hide beat everything below.
    match own_override(node_id, state) {
        OverrideState::Show => return HiddenReason::NotHidden,
        OverrideState::Hide => return HiddenReason::OverrideHide,
        OverrideState::Auto => {}
    }

    // 2.5. GPX-HIDE-CASCADE (A10): a stamp whose own override is Auto
    // follows its owning track's own override Hide or hidden-Group
    // membership. The track's ANCESTOR chain is deliberately not cascaded
    // here — the stamp shares the track's petal, so step 3 below already
    // hides it directly on the same grounds (A10: "the track's ancestor
    // chain already hides the stamp directly").
    if let Some(track_id) = owning_track_id(node_id, state) {
        match lattice_hidden_reason(track_id, ancestry, state) {
            HiddenReason::OverrideHide | HiddenReason::Group(_) => {
                return HiddenReason::OwningPath(track_id.to_string());
            }
            _ => {}
        }
    }

    ancestor_or_group_reason(node_id, ancestry, state)
}

/// A node's own tri-state override. For a stamp passed by its SPAWNED
/// marker id, also checks the promoted node id A11's alias
/// (`VisibilityState::promoted_stamp_alias`) maps it onto — a promoted
/// stamp's persisted `view.state` hydrates onto its real node row id, not
/// the marker, so the marker-keyed lookup alone would miss it.
fn own_override(node_id: &str, state: &VisibilityState) -> OverrideState {
    if let Some(ov) = state.node_overrides.get(node_id) {
        return *ov;
    }
    state
        .promoted_stamp_alias
        .get(node_id)
        .and_then(|alias| state.node_overrides.get(alias))
        .copied()
        .unwrap_or_default()
}

/// The track id that owns `node_id`, if it names a stamp instance — either
/// directly (its spawned marker id, `{track}::stamp::{i}`) or, once
/// promoted, through the A11 alias's reverse index (`promoted node id ->
/// marker id`, `VisibilityState::promoted_stamp_marker`). `None` otherwise.
fn owning_track_id<'a>(node_id: &'a str, state: &'a VisibilityState) -> Option<&'a str> {
    if let Some((track, _)) = crate::verse_manager::parse_stamp_marker_id(node_id) {
        return Some(track);
    }
    let marker = state.promoted_stamp_marker.get(node_id)?;
    crate::verse_manager::parse_stamp_marker_id(marker).map(|(track, _)| track)
}

/// One-level, non-recursive lookup of `node_id`'s own lattice state: own
/// override, then ancestor chain, then groups. Used directly for the
/// GPX-HIDE-CASCADE track lookup — it never consults solo (A10: solo is not
/// extended to a soloed track's stamps) and never cascades itself (a track
/// is never a stamp, so there is only one level).
fn lattice_hidden_reason(node_id: &str, ancestry: NodeAncestry, state: &VisibilityState) -> HiddenReason {
    match own_override(node_id, state) {
        OverrideState::Show => return HiddenReason::NotHidden,
        OverrideState::Hide => return HiddenReason::OverrideHide,
        OverrideState::Auto => {}
    }
    ancestor_or_group_reason(node_id, ancestry, state)
}

/// The shared tail of the lattice: ancestor chain, then groups. Shared by
/// [`node_hidden_reason`] (for the node itself) and [`lattice_hidden_reason`]
/// (for a cascade's owning track).
fn ancestor_or_group_reason(
    node_id: &str,
    ancestry: NodeAncestry,
    state: &VisibilityState,
) -> HiddenReason {
    // Ancestor chain — any hidden verse/fractal/petal hides.
    if state.hidden_verses.contains(ancestry.verse_id)
        || state.hidden_fractals.contains(ancestry.fractal_id)
        || state.hidden_petals.contains(ancestry.petal_id)
    {
        return HiddenReason::Ancestor;
    }

    // Groups — hidden if ANY containing group is hidden.
    if let Some(group_ids) = state.group_membership.get(node_id) {
        for group in group_ids
            .iter()
            .filter_map(|gid| find_group(&state.groups, gid))
        {
            if !group.visible {
                return HiddenReason::Group(group.name.clone());
            }
        }
    }

    HiddenReason::NotHidden
}

fn find_group<'a>(groups: &'a [Group], id: &str) -> Option<&'a Group> {
    groups.iter().find(|g| g.id == id)
}

/// Whether a petal is effectively visible from the ancestor-chain +
/// own-hidden perspective only (node overrides/groups are node-scoped and
/// aren't consulted here). Used for the petal-row eye glyph and the
/// fractal-row rollup.
pub fn petal_effective_visible(
    petal_id: &str,
    fractal_id: &str,
    verse_id: &str,
    state: &VisibilityState,
) -> bool {
    !state.hidden_petals.contains(petal_id)
        && !state.hidden_fractals.contains(fractal_id)
        && !state.hidden_verses.contains(verse_id)
}

/// Whether a fractal is effectively visible from the ancestor-chain +
/// own-hidden perspective. Used for the fractal-row eye glyph (empty-petal
/// fallback) and the verse-row rollup.
pub fn fractal_effective_visible(
    fractal_id: &str,
    verse_id: &str,
    state: &VisibilityState,
) -> bool {
    !state.hidden_fractals.contains(fractal_id) && !state.hidden_verses.contains(verse_id)
}

/// Whether a verse is effectively visible (own hidden flag only — verses have
/// no ancestor).
pub fn verse_effective_visible(verse_id: &str, state: &VisibilityState) -> bool {
    !state.hidden_verses.contains(verse_id)
}

/// Tri-state rollup for a fractal/verse row's eye glyph — "never fake
/// indeterminate with a boolean" (spec §4.1). `own_effective_visible_if_empty`
/// is consulted only when `children` is empty (an empty branch has no
/// descendant signal, so it falls back to its own effective-visible flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollupState {
    AllVisible,
    AllHidden,
    Mixed,
}

pub fn rollup(
    children: impl Iterator<Item = bool>,
    own_effective_visible_if_empty: bool,
) -> RollupState {
    let mut any_visible = false;
    let mut any_hidden = false;
    let mut has_children = false;
    for visible in children {
        has_children = true;
        if visible {
            any_visible = true;
        } else {
            any_hidden = true;
        }
    }
    if !has_children {
        return if own_effective_visible_if_empty {
            RollupState::AllVisible
        } else {
            RollupState::AllHidden
        };
    }
    match (any_visible, any_hidden) {
        (true, true) => RollupState::Mixed,
        (true, false) => RollupState::AllVisible,
        (false, true) => RollupState::AllHidden,
        (false, false) => unreachable!("has_children implies at least one bool was observed"),
    }
}

/// Click-cycle for the node-row eye, reason-aware per spec §4.2 ("expose the
/// tri-state honestly"): a node hidden by ancestor/group sets `Show` (the
/// escape hatch) rather than blindly toggling; a node hidden by its own
/// override returns to `Auto`; an otherwise-visible node gets `Hide`. Solo is
/// a transient lens — clicking during solo is a documented no-op (there is no
/// v1 UI path that reaches this arm, since Isolate is a Phase 2 right-click
/// gesture per RATIFICATION #8).
pub fn toggle_node_override(reason: &HiddenReason, current: OverrideState) -> OverrideState {
    match reason {
        HiddenReason::NotHidden => OverrideState::Hide,
        HiddenReason::OverrideHide => OverrideState::Auto,
        HiddenReason::Ancestor | HiddenReason::Group(_) | HiddenReason::OwningPath(_) => {
            OverrideState::Show
        }
        HiddenReason::Solo => current,
    }
}

/// Toggles membership of `id` in a session-hidden scope set (petal/fractal/
/// verse eyes are plain booleans, not tri-state — see `VisibilityState`
/// docs).
pub fn toggle_scope_hidden(set: &mut HashSet<String>, id: &str) {
    if !set.remove(id) {
        set.insert(id.to_string());
    }
}

// ---------------------------------------------------------------------------
// Eye glyphs — BMP-safe filled/hollow/indeterminate trio (spec §4.1).
// ---------------------------------------------------------------------------

/// Filled circle — effectively visible.
pub const GLYPH_VISIBLE: &str = "\u{25CF}";
/// Hollow circle — effectively hidden (any reason; the tooltip carries why).
pub const GLYPH_HIDDEN: &str = "\u{25CB}";
/// Half-filled circle — a fractal/verse whose descendants disagree.
pub const GLYPH_INDETERMINATE: &str = "\u{25D0}";

pub fn glyph_for_bool(visible: bool) -> &'static str {
    if visible {
        GLYPH_VISIBLE
    } else {
        GLYPH_HIDDEN
    }
}

pub fn glyph_for_rollup(state: RollupState) -> &'static str {
    match state {
        RollupState::AllVisible => GLYPH_VISIBLE,
        RollupState::AllHidden => GLYPH_HIDDEN,
        RollupState::Mixed => GLYPH_INDETERMINATE,
    }
}

// ---------------------------------------------------------------------------
// Node eye tooltip text (spec §4.2: "expose the tri-state honestly")
// ---------------------------------------------------------------------------

pub fn node_eye_tooltip(reason: &HiddenReason) -> String {
    match reason {
        HiddenReason::NotHidden => "Hide".to_string(),
        HiddenReason::Solo => "Hidden — isolate mode is active".to_string(),
        HiddenReason::OverrideHide => "Hidden — click to show".to_string(),
        HiddenReason::Ancestor => "Hidden with ancestors — click to show anyway".to_string(),
        HiddenReason::Group(name) => format!("Hidden by Group '{name}' — click to show anyway"),
        HiddenReason::OwningPath(track_id) => {
            format!("Hidden with its path '{track_id}' — click to show anyway")
        }
    }
}

// ---------------------------------------------------------------------------
// Status-bar "N hidden" chip (spec §4.1, RATIFICATION #16)
// ---------------------------------------------------------------------------

/// `None` when nothing is hidden — the chip is absent, not a "0 hidden" label
/// (spec: "absent when zero").
pub fn hidden_chip_label(hidden_count: usize) -> Option<String> {
    if hidden_count == 0 {
        None
    } else {
        Some(format!("{hidden_count} hidden"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::SoloSet;

    fn ancestry<'a>() -> NodeAncestry<'a> {
        NodeAncestry {
            verse_id: "v1",
            fractal_id: "f1",
            petal_id: "p1",
        }
    }

    // --- effective_visibility / node_hidden_reason: precedence pairs ---

    #[test]
    fn empty_state_defaults_to_visible() {
        let state = VisibilityState::default();
        assert!(effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::NotHidden
        );
    }

    #[test]
    fn override_hide_beats_everything() {
        let mut state = VisibilityState::default();
        state
            .node_overrides
            .insert("n1".into(), OverrideState::Hide);
        assert!(!effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::OverrideHide
        );
    }

    #[test]
    fn override_show_beats_hidden_ancestor_chain() {
        let mut state = VisibilityState::default();
        state.hidden_petals.insert("p1".into());
        state
            .node_overrides
            .insert("n1".into(), OverrideState::Show);
        assert!(effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::NotHidden
        );
    }

    #[test]
    fn override_show_beats_hidden_group() {
        let mut state = VisibilityState::default();
        state.groups.push(Group {
            id: "g1".into(),
            name: "Utilities".into(),
            visible: false,
        });
        state
            .group_membership
            .insert("n1".into(), vec!["g1".into()]);
        state
            .node_overrides
            .insert("n1".into(), OverrideState::Show);
        assert!(effective_visibility("n1", ancestry(), &state));
    }

    #[test]
    fn auto_hides_when_verse_ancestor_hidden() {
        let mut state = VisibilityState::default();
        state.hidden_verses.insert("v1".into());
        assert!(!effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::Ancestor
        );
    }

    #[test]
    fn auto_hides_when_fractal_ancestor_hidden() {
        let mut state = VisibilityState::default();
        state.hidden_fractals.insert("f1".into());
        assert!(!effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::Ancestor
        );
    }

    #[test]
    fn auto_hides_when_petal_ancestor_hidden() {
        let mut state = VisibilityState::default();
        state.hidden_petals.insert("p1".into());
        assert!(!effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::Ancestor
        );
    }

    #[test]
    fn auto_visible_when_all_containing_groups_visible() {
        let mut state = VisibilityState::default();
        state.groups.push(Group {
            id: "g1".into(),
            name: "Utilities".into(),
            visible: true,
        });
        state
            .group_membership
            .insert("n1".into(), vec!["g1".into()]);
        assert!(effective_visibility("n1", ancestry(), &state));
    }

    #[test]
    fn multi_group_membership_any_hidden_group_hides() {
        let mut state = VisibilityState::default();
        state.groups.push(Group {
            id: "g1".into(),
            name: "A".into(),
            visible: true,
        });
        state.groups.push(Group {
            id: "g2".into(),
            name: "Phase-2".into(),
            visible: false,
        });
        state
            .group_membership
            .insert("n1".into(), vec!["g1".into(), "g2".into()]);
        // ANY-hides, not ALL — visible in g1 doesn't save it from hidden g2.
        assert!(!effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::Group("Phase-2".into())
        );
    }

    #[test]
    fn multi_group_membership_all_visible_stays_visible() {
        let mut state = VisibilityState::default();
        state.groups.push(Group {
            id: "g1".into(),
            name: "A".into(),
            visible: true,
        });
        state.groups.push(Group {
            id: "g2".into(),
            name: "B".into(),
            visible: true,
        });
        state
            .group_membership
            .insert("n1".into(), vec!["g1".into(), "g2".into()]);
        assert!(effective_visibility("n1", ancestry(), &state));
    }

    #[test]
    fn solo_vs_show_override_non_member_stays_hidden() {
        let mut state = VisibilityState::default();
        state
            .node_overrides
            .insert("n1".into(), OverrideState::Show);
        state.solo = Some(SoloSet {
            members: HashSet::from(["n2".to_string()]),
        });
        // n1 has an explicit Show override but isn't in the solo set — solo wins.
        assert!(!effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::Solo
        );
    }

    #[test]
    fn solo_vs_hide_override_member_stays_visible() {
        let mut state = VisibilityState::default();
        state
            .node_overrides
            .insert("n1".into(), OverrideState::Hide);
        state.solo = Some(SoloSet {
            members: HashSet::from(["n1".to_string()]),
        });
        // Solo membership wins over the node's own Hide override.
        assert!(effective_visibility("n1", ancestry(), &state));
        assert_eq!(
            node_hidden_reason("n1", ancestry(), &state),
            HiddenReason::NotHidden
        );
    }

    #[test]
    fn solo_active_hides_everything_else_regardless_of_ancestors_and_groups() {
        let mut state = VisibilityState::default();
        // Would otherwise be fully visible with no overrides/ancestors/groups set.
        state.set_solo(HashSet::from(["other".to_string()]));
        assert!(!effective_visibility("n1", ancestry(), &state));
    }

    // --- GPX-HIDE-CASCADE (W6-5): a stamp follows its owning track ---

    #[test]
    fn a_stamp_follows_its_owning_tracks_visibility_unless_its_own_override_wins() {
        type Setup = fn(&mut VisibilityState);
        let stamp = "track1::stamp::0";
        let cases: [(&str, &str, Setup, HiddenReason); 9] = [
            (
                "track Hide -> stamp OwningPath",
                stamp,
                |s| s.set_node_override("track1", OverrideState::Hide),
                HiddenReason::OwningPath("track1".to_string()),
            ),
            (
                "track Hide + stamp Show -> visible",
                stamp,
                |s| {
                    s.set_node_override("track1", OverrideState::Hide);
                    s.set_node_override("track1::stamp::0", OverrideState::Show);
                },
                HiddenReason::NotHidden,
            ),
            (
                "track in a hidden group -> stamp hidden",
                stamp,
                |s| {
                    s.groups.push(Group {
                        id: "g1".into(),
                        name: "G".into(),
                        visible: false,
                    });
                    s.group_membership.insert("track1".into(), vec!["g1".into()]);
                },
                HiddenReason::OwningPath("track1".to_string()),
            ),
            (
                "track visible + stamp Hide -> OverrideHide",
                stamp,
                |s| s.set_node_override("track1::stamp::0", OverrideState::Hide),
                HiddenReason::OverrideHide,
            ),
            (
                "hidden petal -> Ancestor (unchanged)",
                stamp,
                |s| s.toggle_petal_hidden("p1"),
                HiddenReason::Ancestor,
            ),
            (
                "solo with the stamp a member -> visible",
                stamp,
                |s| s.set_solo(HashSet::from(["track1::stamp::0".to_string()])),
                HiddenReason::NotHidden,
            ),
            (
                "solo with the track a member but the stamp not -> Solo",
                stamp,
                |s| s.set_solo(HashSet::from(["track1".to_string()])),
                HiddenReason::Solo,
            ),
            (
                "a non-stamp node with a ::stamp::-free id -> unaffected",
                "plain-node",
                |s| s.set_node_override("track1", OverrideState::Hide),
                HiddenReason::NotHidden,
            ),
            (
                "track override Show -> stamp visible even with the track's group hidden",
                stamp,
                |s| {
                    s.groups.push(Group {
                        id: "g1".into(),
                        name: "G".into(),
                        visible: false,
                    });
                    s.group_membership.insert("track1".into(), vec!["g1".into()]);
                    s.set_node_override("track1", OverrideState::Show);
                },
                HiddenReason::NotHidden,
            ),
        ];
        for (label, node_id, setup, expected) in cases {
            let mut state = VisibilityState::default();
            setup(&mut state);
            assert_eq!(node_hidden_reason(node_id, ancestry(), &state), expected, "{label}");
        }
    }

    #[test]
    fn owning_path_toggles_to_show_like_ancestor_and_group() {
        assert_eq!(
            toggle_node_override(&HiddenReason::OwningPath("track1".into()), OverrideState::Auto),
            OverrideState::Show
        );
    }

    #[test]
    fn owning_path_tooltip_names_the_track() {
        let t = node_eye_tooltip(&HiddenReason::OwningPath("track1".into()));
        assert!(t.contains("track1"));
    }

    // --- petal/fractal/verse scope helpers ---

    #[test]
    fn petal_effective_visible_true_by_default() {
        let state = VisibilityState::default();
        assert!(petal_effective_visible("p1", "f1", "v1", &state));
    }

    #[test]
    fn petal_effective_visible_false_when_own_hidden() {
        let mut state = VisibilityState::default();
        state.hidden_petals.insert("p1".into());
        assert!(!petal_effective_visible("p1", "f1", "v1", &state));
    }

    #[test]
    fn petal_effective_visible_false_when_fractal_ancestor_hidden() {
        let mut state = VisibilityState::default();
        state.hidden_fractals.insert("f1".into());
        assert!(!petal_effective_visible("p1", "f1", "v1", &state));
    }

    #[test]
    fn petal_effective_visible_false_when_verse_ancestor_hidden() {
        let mut state = VisibilityState::default();
        state.hidden_verses.insert("v1".into());
        assert!(!petal_effective_visible("p1", "f1", "v1", &state));
    }

    #[test]
    fn fractal_effective_visible_respects_verse_ancestor() {
        let mut state = VisibilityState::default();
        state.hidden_verses.insert("v1".into());
        assert!(!fractal_effective_visible("f1", "v1", &state));
    }

    #[test]
    fn verse_effective_visible_is_own_flag_only() {
        let mut state = VisibilityState::default();
        assert!(verse_effective_visible("v1", &state));
        state.hidden_verses.insert("v1".into());
        assert!(!verse_effective_visible("v1", &state));
    }

    // --- rollup ---

    #[test]
    fn rollup_all_visible() {
        assert_eq!(
            rollup([true, true, true].into_iter(), true),
            RollupState::AllVisible
        );
    }

    #[test]
    fn rollup_all_hidden() {
        assert_eq!(
            rollup([false, false].into_iter(), true),
            RollupState::AllHidden
        );
    }

    #[test]
    fn rollup_mixed() {
        assert_eq!(rollup([true, false].into_iter(), true), RollupState::Mixed);
    }

    #[test]
    fn rollup_empty_falls_back_to_own_visible_flag() {
        assert_eq!(rollup(std::iter::empty(), true), RollupState::AllVisible);
        assert_eq!(rollup(std::iter::empty(), false), RollupState::AllHidden);
    }

    // --- toggle_node_override ---

    #[test]
    fn toggle_visible_node_hides_it() {
        assert_eq!(
            toggle_node_override(&HiddenReason::NotHidden, OverrideState::Auto),
            OverrideState::Hide
        );
    }

    #[test]
    fn toggle_override_hidden_node_returns_to_auto() {
        assert_eq!(
            toggle_node_override(&HiddenReason::OverrideHide, OverrideState::Hide),
            OverrideState::Auto
        );
    }

    #[test]
    fn toggle_ancestor_hidden_node_sets_show_escape_hatch() {
        assert_eq!(
            toggle_node_override(&HiddenReason::Ancestor, OverrideState::Auto),
            OverrideState::Show
        );
    }

    #[test]
    fn toggle_group_hidden_node_sets_show_escape_hatch() {
        assert_eq!(
            toggle_node_override(
                &HiddenReason::Group("Utilities".into()),
                OverrideState::Auto
            ),
            OverrideState::Show
        );
    }

    #[test]
    fn toggle_during_solo_is_a_documented_no_op() {
        assert_eq!(
            toggle_node_override(&HiddenReason::Solo, OverrideState::Show),
            OverrideState::Show
        );
    }

    // --- toggle_scope_hidden ---

    #[test]
    fn toggle_scope_hidden_inserts_then_removes() {
        let mut set = HashSet::new();
        toggle_scope_hidden(&mut set, "p1");
        assert!(set.contains("p1"));
        toggle_scope_hidden(&mut set, "p1");
        assert!(!set.contains("p1"));
    }

    // --- glyphs ---

    #[test]
    fn glyph_for_bool_maps_correctly() {
        assert_eq!(glyph_for_bool(true), GLYPH_VISIBLE);
        assert_eq!(glyph_for_bool(false), GLYPH_HIDDEN);
    }

    #[test]
    fn glyph_for_rollup_maps_all_three_states() {
        assert_eq!(glyph_for_rollup(RollupState::AllVisible), GLYPH_VISIBLE);
        assert_eq!(glyph_for_rollup(RollupState::AllHidden), GLYPH_HIDDEN);
        assert_eq!(glyph_for_rollup(RollupState::Mixed), GLYPH_INDETERMINATE);
    }

    // --- tooltip text ---

    #[test]
    fn node_eye_tooltip_names_the_group() {
        let t = node_eye_tooltip(&HiddenReason::Group("Utilities".into()));
        assert!(t.contains("Utilities"));
    }

    #[test]
    fn node_eye_tooltip_mentions_ancestors() {
        let t = node_eye_tooltip(&HiddenReason::Ancestor);
        assert!(t.to_lowercase().contains("ancestor"));
    }

    #[test]
    fn node_eye_tooltip_visible_state_offers_hide_action() {
        assert_eq!(node_eye_tooltip(&HiddenReason::NotHidden), "Hide");
    }

    // --- status chip ---

    #[test]
    fn hidden_chip_label_absent_when_zero() {
        assert_eq!(hidden_chip_label(0), None);
    }

    #[test]
    fn hidden_chip_label_present_when_nonzero() {
        assert_eq!(hidden_chip_label(3), Some("3 hidden".to_string()));
    }
}
