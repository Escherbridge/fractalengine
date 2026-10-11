//! Persisted Group registry (`petal.visibility_groups`) and scope eyes
//! (`view_hidden`) — hierarchy_visibility_groups Phase 4: hydration from DB
//! results and the authed writes behind `UiAction::{SetGroupRegistry,
//! SetScopeHidden, SetNodeGroups}`. See `fe-ui/src/AGENTS.md` §visibility.

use std::collections::HashSet;

use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{
    CallerAuth, DbCommand, ScopeLevel, VerseHierarchyData, VisibilityGroupDto,
};

use super::{Group, VisibilityState};
use crate::navigation_manager::NavigationManager;

/// Namespaces this GUI's own registry writes.
const GROUPS_CORRELATION_PREFIX: &str = "fe-ui:groups:";
/// Namespaces this GUI's own registry loads (a failed READ is not "not saved").
const GROUPS_LOAD_CORRELATION_PREFIX: &str = "fe-ui:groups-load:";
/// Namespaces this GUI's own scope-eye writes.
const SCOPE_EYE_CORRELATION_PREFIX: &str = "fe-ui:scope-eye:";
/// Toast for a denied Group or eye write (ui_ux §6 — never fake persistence).
pub(crate) const GROUPS_NOT_SAVED_DENIED_TOAST: &str = "Groups not saved (you need Editor access)";
/// Toast for any other failed Group or eye write.
pub(crate) const GROUPS_NOT_SAVED_TOAST: &str =
    "Groups not saved — the change only lasts this session";

/// Registry rows in display order (ascending `order`, stable). Pure.
pub(crate) fn groups_from_dtos(dtos: &[VisibilityGroupDto]) -> Vec<Group> {
    let mut sorted: Vec<&VisibilityGroupDto> = dtos.iter().collect();
    sorted.sort_by(|a, b| a.order.total_cmp(&b.order));
    sorted
        .into_iter()
        .map(|dto| Group {
            id: dto.id.clone(),
            name: dto.name.clone(),
            visible: dto.visible,
        })
        .collect()
}

/// Wire rows; `order` is the display index. Pure.
pub(crate) fn dtos_from_groups(groups: &[Group]) -> Vec<VisibilityGroupDto> {
    groups
        .iter()
        .enumerate()
        .map(|(index, g)| VisibilityGroupDto {
            id: g.id.clone(),
            name: g.name.clone(),
            visible: g.visible,
            order: index as f64,
        })
        .collect()
}

/// A fresh Group id: the name slugged (`a-z0-9-`, ≤48 chars), suffixed `-2`,
/// `-3`… until unused. Pure.
pub(crate) fn next_group_id(name: &str, existing: &[Group]) -> String {
    let mut base: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    base = base
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    base.truncate(48);
    if base.is_empty() {
        base = "group".to_string();
    }
    let taken = |id: &str| existing.iter().any(|g| g.id == id);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded suffix search always finds a free id")
}

/// Registry with a new visible Group named `name` appended. Pure.
pub(crate) fn with_group_added(groups: &[Group], name: &str) -> Vec<Group> {
    let mut next = groups.to_vec();
    next.push(Group {
        id: next_group_id(name, groups),
        name: name.trim().to_string(),
        visible: true,
    });
    next
}

/// Registry with `id`'s visibility set. Pure.
pub(crate) fn with_group_visible(groups: &[Group], id: &str, visible: bool) -> Vec<Group> {
    groups
        .iter()
        .map(|g| Group {
            visible: if g.id == id { visible } else { g.visible },
            ..g.clone()
        })
        .collect()
}

/// Registry with `id` renamed (the id is stable — membership keeps working). Pure.
pub(crate) fn with_group_renamed(groups: &[Group], id: &str, name: &str) -> Vec<Group> {
    groups
        .iter()
        .map(|g| Group {
            name: if g.id == id {
                name.to_string()
            } else {
                g.name.clone()
            },
            ..g.clone()
        })
        .collect()
}

/// Registry without `id` (members keep the dangling id, which hides nothing). Pure.
pub(crate) fn with_group_removed(groups: &[Group], id: &str) -> Vec<Group> {
    groups.iter().filter(|g| g.id != id).cloned().collect()
}

/// Registry with the Group at `index` moved by `delta` places (clamped). Pure.
pub(crate) fn with_group_moved(groups: &[Group], index: usize, delta: isize) -> Vec<Group> {
    let mut next = groups.to_vec();
    if index < next.len() {
        let target = index
            .saturating_add_signed(delta)
            .min(next.len().saturating_sub(1));
        let group = next.remove(index);
        next.insert(target, group);
    }
    next
}

/// `UiAction::SetGroupRegistry`: optimistic replace of the active registry,
/// then the authed whole-registry write (`CallerAuth::Local` — the DB resolves
/// the real role, N-5).
pub(crate) fn set_group_registry(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    petal_id: String,
    groups: Vec<Group>,
) {
    let dtos = dtos_from_groups(&groups);
    state.replace_groups(groups);
    let correlation_id = Some(format!("{GROUPS_CORRELATION_PREFIX}{petal_id}"));
    if db_sender
        .0
        .send(DbCommand::SetPetalVisibilityGroups {
            petal_id,
            groups: dtos,
            auth: CallerAuth::Local,
            correlation_id,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — SetPetalVisibilityGroups not persisted");
    }
}

/// `PetalVisibilityGroupsLoaded` (load or write echo, from this GUI or any API
/// caller): authoritative for the ACTIVE petal only.
pub(crate) fn apply_registry_loaded(
    state: &mut VisibilityState,
    active_petal_id: Option<&str>,
    petal_id: &str,
    dtos: &[VisibilityGroupDto],
) {
    if active_petal_id == Some(petal_id) {
        state.replace_groups(groups_from_dtos(dtos));
    }
}

/// `UiAction::SetScopeHidden`: optimistic eye flip, then the authed write.
pub(crate) fn set_scope_hidden(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    level: ScopeLevel,
    id: String,
    hidden: bool,
) {
    state.set_scope_hidden(level, &id, hidden);
    let correlation_id = Some(format!("{SCOPE_EYE_CORRELATION_PREFIX}{id}"));
    if db_sender
        .0
        .send(DbCommand::SetScopeHidden {
            level,
            id,
            hidden,
            auth: CallerAuth::Local,
            correlation_id,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — SetScopeHidden not persisted");
    }
}

/// `HierarchyLoaded`: the persisted eyes are authoritative for all three
/// levels (a refused session-only eye lasts until the next hierarchy load).
pub(crate) fn hydrate_scope_hidden(state: &mut VisibilityState, verses: &[VerseHierarchyData]) {
    let mut hidden_verses = HashSet::new();
    let mut hidden_fractals = HashSet::new();
    let mut hidden_petals = HashSet::new();
    for verse in verses {
        if verse.view_hidden {
            hidden_verses.insert(verse.id.clone());
        }
        for fractal in &verse.fractals {
            if fractal.view_hidden {
                hidden_fractals.insert(fractal.id.clone());
            }
            for petal in &fractal.petals {
                if petal.view_hidden {
                    hidden_petals.insert(petal.id.clone());
                }
            }
        }
    }
    state.replace_hidden_scopes(hidden_verses, hidden_fractals, hidden_petals);
}

/// `UiAction::SetNodeGroups`: optimistic membership replace, then the authed
/// `SetNodeView { groups }` (Phase 3's command — membership lives on the node).
pub(crate) fn set_node_groups(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    node_id: String,
    groups: Vec<String>,
) {
    state.set_node_group_membership(&node_id, groups.clone());
    let correlation_id = Some(super::cache::view_write_correlation(&node_id));
    if db_sender
        .0
        .send(DbCommand::SetNodeView {
            node_id,
            state: None,
            groups: Some(groups),
            auth: CallerAuth::Local,
            correlation_id,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — SetNodeView (groups) not persisted");
    }
}

/// `node_groups` with `group_id` added (`member`) or removed; `None` when the
/// membership would not change. Pure.
pub(crate) fn membership_with(
    node_groups: &[String],
    group_id: &str,
    member: bool,
) -> Option<Vec<String>> {
    let present = node_groups.iter().any(|id| id == group_id);
    match (member, present) {
        (true, false) => {
            let mut next = node_groups.to_vec();
            next.push(group_id.to_string());
            Some(next)
        }
        (false, true) => Some(
            node_groups
                .iter()
                .filter(|id| *id != group_id)
                .cloned()
                .collect(),
        ),
        _ => None,
    }
}

/// Toast copy for a failed GUI Group/eye write, or `None` when the failure
/// belongs to another request. The session value is kept either way.
pub(crate) fn registry_write_failure_toast(
    correlation_id: &str,
    error: &str,
) -> Option<&'static str> {
    if !correlation_id.starts_with(GROUPS_CORRELATION_PREFIX)
        && !correlation_id.starts_with(SCOPE_EYE_CORRELATION_PREFIX)
    {
        return None;
    }
    Some(if error.contains("denied") {
        GROUPS_NOT_SAVED_DENIED_TOAST
    } else {
        GROUPS_NOT_SAVED_TOAST
    })
}

/// On petal entry/switch: drop the previous petal's registry and request the
/// new one (sits next to `cache::load_petal_view_state_on_nav_change`).
pub(crate) fn load_petal_groups_on_nav_change(
    nav: Res<NavigationManager>,
    db_sender: Res<DbCommandSender>,
    mut state: ResMut<VisibilityState>,
    mut last_petal: Local<Option<String>>,
) {
    if *last_petal == nav.active_petal_id {
        return;
    }
    last_petal.clone_from(&nav.active_petal_id);
    state.replace_groups(Vec::new());
    let Some(petal_id) = nav.active_petal_id.clone() else {
        return;
    };
    let correlation_id = Some(format!("{GROUPS_LOAD_CORRELATION_PREFIX}{petal_id}"));
    if db_sender
        .0
        .send(DbCommand::GetPetalVisibilityGroups {
            petal_id,
            correlation_id,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — GetPetalVisibilityGroups not dispatched");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::messages::{FractalHierarchyData, PetalHierarchyData};

    fn group(id: &str, name: &str, visible: bool) -> Group {
        Group {
            id: id.into(),
            name: name.into(),
            visible,
        }
    }

    fn dto(id: &str, visible: bool, order: f64) -> VisibilityGroupDto {
        VisibilityGroupDto {
            id: id.into(),
            name: id.to_uppercase(),
            visible,
            order,
        }
    }

    #[test]
    fn a_loaded_registry_hydrates_the_active_petal_in_order_and_hides_members() {
        let mut state = VisibilityState::default();
        state.set_node_group_membership("n1", vec!["utilities".into()]);
        let ancestry = super::super::NodeAncestry {
            verse_id: "v",
            fractal_id: "f",
            petal_id: "p1",
        };
        assert!(super::super::effective_visibility("n1", ancestry, &state));

        // Another petal's registry never lands on the active one.
        apply_registry_loaded(
            &mut state,
            Some("p1"),
            "p2",
            &[dto("utilities", false, 0.0)],
        );
        assert!(state.groups().is_empty());

        apply_registry_loaded(
            &mut state,
            Some("p1"),
            "p1",
            &[dto("phase-2", true, 3.0), dto("utilities", false, -1.0)],
        );
        let ids: Vec<&str> = state.groups().iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["utilities", "phase-2"], "sorted by order");
        assert!(
            !super::super::effective_visibility("n1", ancestry, &state),
            "a member of a hidden loaded Group is hidden"
        );

        // A re-delivered identical registry is not a mutation.
        let settled = state.epoch();
        apply_registry_loaded(
            &mut state,
            Some("p1"),
            "p1",
            &[dto("utilities", false, -1.0), dto("phase-2", true, 3.0)],
        );
        assert_eq!(state.epoch(), settled);
    }

    #[test]
    fn hierarchy_eyes_replace_the_session_hidden_sets() {
        let mut state = VisibilityState::default();
        state.set_scope_hidden(ScopeLevel::Petal, "stale", true);
        let verses = vec![VerseHierarchyData {
            id: "v1".into(),
            name: "V".into(),
            namespace_id: None,
            timeseries: Default::default(),
            view_hidden: false,
            fractals: vec![FractalHierarchyData {
                id: "f1".into(),
                name: "F".into(),
                view_hidden: true,
                petals: vec![PetalHierarchyData {
                    id: "p1".into(),
                    name: "P".into(),
                    view_hidden: true,
                    nodes: Vec::new(),
                }],
            }],
        }];
        hydrate_scope_hidden(&mut state, &verses);
        assert!(!state.is_scope_hidden(ScopeLevel::Verse, "v1"));
        assert!(state.is_scope_hidden(ScopeLevel::Fractal, "f1"));
        assert!(state.is_scope_hidden(ScopeLevel::Petal, "p1"));
        assert!(!state.is_scope_hidden(ScopeLevel::Petal, "stale"));
    }

    #[test]
    fn registry_and_eye_writes_are_optimistic_and_authed() {
        let (tx, rx) = crossbeam::channel::bounded(4);
        let sender = DbCommandSender(tx);
        let mut state = VisibilityState::default();

        set_group_registry(
            &sender,
            &mut state,
            "p1".into(),
            vec![group("b", "B", true), group("a", "A", false)],
        );
        assert_eq!(state.groups().len(), 2, "optimistic");
        match rx.try_recv() {
            Ok(DbCommand::SetPetalVisibilityGroups {
                petal_id,
                groups,
                auth,
                correlation_id,
            }) => {
                assert_eq!(petal_id, "p1");
                let wire: Vec<(&str, f64)> =
                    groups.iter().map(|g| (g.id.as_str(), g.order)).collect();
                assert_eq!(wire, vec![("b", 0.0), ("a", 1.0)], "order = display index");
                assert_eq!(auth, CallerAuth::Local);
                let id = correlation_id.expect("GUI writes are correlated");
                assert_eq!(
                    registry_write_failure_toast(&id, "SetPetalVisibilityGroups denied for x"),
                    Some(GROUPS_NOT_SAVED_DENIED_TOAST)
                );
            }
            other => panic!("expected SetPetalVisibilityGroups, got {other:?}"),
        }

        set_scope_hidden(&sender, &mut state, ScopeLevel::Fractal, "f1".into(), true);
        assert!(state.is_scope_hidden(ScopeLevel::Fractal, "f1"));
        match rx.try_recv() {
            Ok(DbCommand::SetScopeHidden {
                level,
                id,
                hidden,
                auth,
                correlation_id,
            }) => {
                assert_eq!(
                    (level, id.as_str(), hidden),
                    (ScopeLevel::Fractal, "f1", true)
                );
                assert_eq!(auth, CallerAuth::Local);
                assert_eq!(
                    registry_write_failure_toast(&correlation_id.unwrap(), "db down"),
                    Some(GROUPS_NOT_SAVED_TOAST)
                );
            }
            other => panic!("expected SetScopeHidden, got {other:?}"),
        }
        assert_eq!(
            registry_write_failure_toast("fe-ui:view:n1", "denied"),
            None,
            "another request's failure is not ours"
        );
        assert_eq!(
            registry_write_failure_toast(
                &format!("{GROUPS_LOAD_CORRELATION_PREFIX}p1"),
                "GetPetalVisibilityGroups: no petal p1"
            ),
            None,
            "a failed registry READ never claims a write was lost"
        );
    }

    #[test]
    fn membership_edits_change_only_what_they_name() {
        let groups = vec!["a".to_string()];
        assert_eq!(
            membership_with(&groups, "b", true),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            membership_with(&groups, "a", true),
            None,
            "already a member"
        );
        assert_eq!(membership_with(&groups, "a", false), Some(Vec::new()));
        assert_eq!(membership_with(&groups, "b", false), None, "not a member");
    }

    #[test]
    fn registry_edits_change_exactly_one_thing() {
        let base = vec![
            group("a", "A", true),
            group("b", "B", true),
            group("c", "C", true),
        ];
        let ids = |gs: &[Group]| gs.iter().map(|g| g.id.clone()).collect::<Vec<_>>();

        let added = with_group_added(&base, " Roads ");
        assert_eq!(added.last(), Some(&group("roads", "Roads", true)));
        assert_eq!(&added[..3], &base[..]);

        let hidden = with_group_visible(&base, "b", false);
        let flags: Vec<bool> = hidden.iter().map(|g| g.visible).collect();
        assert_eq!(flags, vec![true, false, true]);

        let renamed = with_group_renamed(&base, "c", "Cee");
        assert_eq!(renamed[2], group("c", "Cee", true), "the id is stable");

        assert_eq!(ids(&with_group_removed(&base, "a")), vec!["b", "c"]);
        assert_eq!(ids(&with_group_moved(&base, 2, -1)), vec!["a", "c", "b"]);
        assert_eq!(ids(&with_group_moved(&base, 0, 1)), vec!["b", "a", "c"]);
        assert_eq!(
            ids(&with_group_moved(&base, 0, -1)),
            vec!["a", "b", "c"],
            "clamped"
        );
        assert_eq!(
            ids(&with_group_moved(&base, 2, 5)),
            vec!["a", "b", "c"],
            "clamped"
        );
    }

    #[test]
    fn new_group_ids_are_slugged_and_never_collide() {
        let existing = vec![
            group("utilities", "Utilities", true),
            group("utilities-2", "U", true),
        ];
        assert_eq!(next_group_id("Utilities", &existing), "utilities-3");
        assert_eq!(
            next_group_id("  Phase 2: Roads ", &existing),
            "phase-2-roads"
        );
        assert_eq!(next_group_id("!!!", &[]), "group");
        assert!(next_group_id(&"x".repeat(200), &[]).len() <= 48);
    }
}
