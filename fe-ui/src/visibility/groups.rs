//! Persisted Group registry (`petal.visibility_groups`) and scope eyes
//! (`view_hidden`) — hierarchy_visibility_groups Phase 4: hydration from DB
//! results and the authed writes behind `UiAction::{SetGroupRegistry,
//! SetScopeHidden, SetNodeGroups}`. See `fe-ui/src/AGENTS.md` §visibility.

use std::collections::HashSet;

use crossbeam::channel::TrySendError;

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

/// A fresh stable identity, independent of the display name.
pub(crate) fn next_group_id(_name: &str, existing: &[Group]) -> String {
    loop {
        let id = ulid::Ulid::new().to_string();
        if !existing.iter().any(|group| group.id == id) {
            return id;
        }
    }
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
#[cfg(test)]
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

#[derive(Debug, Clone, Default)]
pub(super) struct RegistryWrites {
    confirmed: Option<(String, Vec<VisibilityGroupDto>)>,
    pending_write: Option<String>,
    pending_read: Option<(String, std::time::Instant)>,
}

impl VisibilityState {
    pub(crate) fn registry_write_pending(&self) -> bool {
        self.registry_writes.pending_write.is_some()
    }

    pub(crate) fn registry_read_pending(&self) -> bool {
        self.registry_writes
            .pending_read
            .as_ref()
            .is_some_and(|(_, queued)| queued.elapsed() < std::time::Duration::from_secs(15))
    }

    pub(crate) fn registry_ready(&self, petal_id: &str) -> bool {
        !self.registry_write_pending()
            && self.registry_writes.pending_read.is_none()
            && self
                .registry_writes
                .confirmed
                .as_ref()
                .is_some_and(|(id, _)| id == petal_id)
    }
}

/// Admit a checked registry edit without changing its confirmed display.
pub(crate) fn set_group_registry(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    petal_id: String,
    groups: Vec<Group>,
) -> Result<bool, String> {
    if !state.registry_ready(&petal_id) {
        return Err("Wait for stored Groups, or check stored Groups before editing again".into());
    }
    let expected = &state.registry_writes.confirmed.as_ref().unwrap().1;
    if expected
        .iter()
        .any(|old| !groups.iter().any(|next| next.id == old.id))
    {
        return Err(
            "Group deletion requires a checked membership transaction; no Groups were changed"
                .into(),
        );
    }
    let proposed = dtos_from_groups(&groups);
    if groups == state.groups() {
        return Ok(false);
    }
    let correlation = format!("{GROUPS_CORRELATION_PREFIX}{}", ulid::Ulid::new());
    db_sender
        .0
        .try_send(DbCommand::ComparePetalVisibilityGroups {
            petal_id,
            expected: expected.clone(),
            groups: proposed,
            auth: CallerAuth::Local,
            correlation_id: Some(correlation.clone()),
        })
        .map_err(|error| match error {
            TrySendError::Full(_) => "Storage is busy; the Groups edit was not queued".to_owned(),
            TrySendError::Disconnected(_) => {
                "Storage is disconnected; the Groups edit was not queued".to_owned()
            }
        })?;
    state.registry_writes.pending_write = Some(correlation);
    Ok(true)
}

/// Read current storage without retrying an accepted registry write.
pub(crate) fn check_registry(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    petal_id: String,
) -> Result<(), String> {
    if state.registry_read_pending() {
        return Err("A stored Groups check is already queued".into());
    }
    queue_registry_read(db_sender, state, petal_id).map_err(|error| match error {
        TrySendError::Full(_) => "Storage is busy; the Groups check was not queued".to_owned(),
        TrySendError::Disconnected(_) => {
            "Storage is disconnected; the Groups check was not queued".to_owned()
        }
    })
}

#[allow(clippy::result_large_err)] // TrySendError hands the unsent command back to the caller
fn queue_registry_read(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    petal_id: String,
) -> Result<(), TrySendError<DbCommand>> {
    let correlation = format!("{GROUPS_LOAD_CORRELATION_PREFIX}{}", ulid::Ulid::new());
    db_sender.0.try_send(DbCommand::GetPetalVisibilityGroups {
        petal_id,
        correlation_id: Some(correlation.clone()),
    })?;
    state.registry_writes.pending_read = Some((correlation, std::time::Instant::now()));
    Ok(())
}

/// A peer's petal row landed for the active Petal (QA-0123): re-read the stored
/// registry so a replicated Group shows live. Skipped while a read is queued.
pub(crate) fn refresh_registry_from_replica(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    petal_id: String,
) {
    if state.registry_read_pending() {
        return;
    }
    if let Err(error) = queue_registry_read(db_sender, state, petal_id) {
        bevy::log::warn!("Replicated Group registry re-read not queued: {error}");
    }
}

/// Admit the active Petal's authoritative registry, retaining exact CAS order values.
pub(crate) fn apply_registry_loaded(
    state: &mut VisibilityState,
    active_petal_id: Option<&str>,
    petal_id: &str,
    dtos: &[VisibilityGroupDto],
) {
    if active_petal_id == Some(petal_id) {
        state.registry_writes.confirmed = Some((petal_id.into(), dtos.to_vec()));
        state.replace_groups(groups_from_dtos(dtos));
    }
}

pub(crate) fn apply_registry_reply(
    state: &mut VisibilityState,
    active_petal_id: Option<&str>,
    petal_id: &str,
    dtos: &[VisibilityGroupDto],
    correlation_id: Option<&str>,
) {
    if active_petal_id != Some(petal_id) {
        return;
    }
    if correlation_id.is_some_and(|id| state.registry_writes.pending_write.as_deref() == Some(id)) {
        state.registry_writes.pending_write = None;
    }
    if correlation_id.is_some_and(|id| {
        state
            .registry_writes
            .pending_read
            .as_ref()
            .is_some_and(|(pending, _)| pending == id)
    }) {
        state.registry_writes.pending_read = None;
        state.registry_writes.pending_write = None;
    }
    apply_registry_loaded(state, active_petal_id, petal_id, dtos);
}

pub(crate) fn apply_registry_failure(state: &mut VisibilityState, correlation_id: &str) {
    if state
        .registry_writes
        .pending_read
        .as_ref()
        .is_some_and(|(pending, _)| pending == correlation_id)
    {
        state.registry_writes.pending_read = None;
        bevy::log::warn!("Stored Groups check failed; confirmed registry was retained");
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

/// Copy for owned Group/eye failures; see visibility/AGENTS.md.
pub(crate) fn registry_write_failure_toast(
    correlation_id: &str,
    error: &str,
) -> Option<&'static str> {
    if !correlation_id.starts_with(GROUPS_CORRELATION_PREFIX)
        && !correlation_id.starts_with(SCOPE_EYE_CORRELATION_PREFIX)
    {
        return None;
    }
    if correlation_id.starts_with(GROUPS_CORRELATION_PREFIX) {
        return Some("Groups save not confirmed; check stored Groups before editing again");
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
    state.registry_writes = RegistryWrites::default();
    state.replace_groups(Vec::new());
    let Some(petal_id) = nav.active_petal_id.clone() else {
        last_petal.clone_from(&nav.active_petal_id);
        return;
    };
    match queue_registry_read(&db_sender, &mut state, petal_id) {
        Ok(()) => last_petal.clone_from(&nav.active_petal_id),
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {
            last_petal.clone_from(&nav.active_petal_id);
            bevy::log::warn!("Storage disconnected; stored Groups were not requested");
        }
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
                    render_distance: None,
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
    fn registry_readback_is_checked_and_eyes_remain_authed() {
        let (tx, rx) = crossbeam::channel::bounded(1);
        let sender = DbCommandSender(tx);
        let mut state = VisibilityState::default();
        let before = vec![dto("b", true, 8.5), dto("a", false, -3.25)];
        let proposed = vec![group("b", "Renamed", true), group("a", "A", false)];
        assert!(
            set_group_registry(&sender, &mut state, "p1".into(), proposed.clone()).is_err(),
            "unloaded registry cannot be an expected snapshot"
        );
        apply_registry_loaded(&mut state, Some("p1"), "p1", &before);
        let display = state.groups().to_vec();
        assert_eq!(
            set_group_registry(&sender, &mut state, "p1".into(), proposed.clone()),
            Ok(true)
        );
        assert_eq!(
            state.groups(),
            display,
            "accepted queue admission is not confirmed persistence"
        );
        assert!(state.registry_write_pending());
        assert!(set_group_registry(&sender, &mut state, "p1".into(), proposed.clone()).is_err());
        let (written, write_id) = match rx.try_recv().unwrap() {
            DbCommand::ComparePetalVisibilityGroups {
                petal_id,
                expected,
                groups,
                auth,
                correlation_id,
            } => {
                assert_eq!(petal_id, "p1");
                assert_eq!(
                    expected, before,
                    "CAS retains original stored row order and nonsequential order values"
                );
                assert_eq!(auth, CallerAuth::Local);
                (groups, correlation_id.unwrap())
            }
            other => panic!("expected checked registry edit, got {other:?}"),
        };
        apply_registry_reply(&mut state, Some("p1"), "p1", &before, Some("other-client"));
        assert!(
            state.registry_write_pending(),
            "unrelated readback cannot release owned write"
        );
        apply_registry_failure(&mut state, &write_id);
        assert!(
            state.registry_write_pending(),
            "unknown write outcome requires a read, never a retry"
        );
        check_registry(&sender, &mut state, "p1".into()).unwrap();
        assert!(
            check_registry(&sender, &mut state, "p1".into()).is_err(),
            "coalesce pure reads"
        );
        let read_id = match rx.try_recv().unwrap() {
            DbCommand::GetPetalVisibilityGroups { correlation_id, .. } => correlation_id.unwrap(),
            other => panic!("checking does not retry write: {other:?}"),
        };
        apply_registry_reply(&mut state, Some("p2"), "p1", &written, Some(&read_id));
        assert!(
            state.registry_write_pending(),
            "wrong Petal cannot complete active ownership"
        );
        apply_registry_reply(&mut state, Some("p1"), "p1", &written, Some(&read_id));
        assert!(state.registry_ready("p1"));
        assert_eq!(state.groups(), proposed);
        assert!(
            set_group_registry(&sender, &mut state, "p1".into(), Vec::new()).is_err(),
            "registry CAS must not orphan memberships through delete"
        );
        assert!(rx.try_recv().is_err());
        sender
            .0
            .try_send(DbCommand::GetPetalVisibilityGroups {
                petal_id: "busy".into(),
                correlation_id: None,
            })
            .unwrap();
        let changed = with_group_renamed(state.groups(), "b", "Busy rename");
        assert!(
            set_group_registry(&sender, &mut state, "p1".into(), changed.clone())
                .unwrap_err()
                .contains("busy")
        );
        assert!(
            !state.registry_write_pending(),
            "unsent writes never own pending state"
        );
        assert_eq!(state.groups(), proposed);
        rx.try_recv().unwrap();
        assert_eq!(
            registry_write_failure_toast(&write_id, "denied"),
            Some("Groups save not confirmed; check stored Groups before editing again")
        );
        let (closed_tx, closed_rx) = crossbeam::channel::bounded(1);
        drop(closed_rx);
        assert!(set_group_registry(
            &DbCommandSender(closed_tx),
            &mut state,
            "p1".into(),
            changed
        )
        .unwrap_err()
        .contains("disconnected"));
        assert!(!state.registry_write_pending());
        assert_eq!(state.groups(), proposed);
        let next = with_group_renamed(state.groups(), "b", "Confirmed");
        set_group_registry(&sender, &mut state, "p1".into(), next.clone()).unwrap();
        let (after, correlation) = match rx.try_recv().unwrap() {
            DbCommand::ComparePetalVisibilityGroups {
                groups,
                correlation_id,
                ..
            } => (groups, correlation_id.unwrap()),
            other => panic!("expected one admitted write: {other:?}"),
        };
        apply_registry_reply(&mut state, Some("p1"), "p1", &after, Some(&correlation));
        assert!(state.registry_ready("p1"));
        assert_eq!(state.groups(), next);
        check_registry(&sender, &mut state, "p1".into()).unwrap();
        rx.try_recv().unwrap();
        state.registry_writes.pending_read.as_mut().unwrap().1 -=
            std::time::Duration::from_secs(16);
        assert!(
            !state.registry_read_pending(),
            "a lost pure-read reply can be checked again after its bounded wait"
        );
        assert!(!state.registry_ready("p1"), "an expired read permits another check, never a competing write before current readback");
        check_registry(&sender, &mut state, "p1".into()).unwrap();
        let id = match rx.try_recv().unwrap() {
            DbCommand::GetPetalVisibilityGroups { correlation_id, .. } => correlation_id.unwrap(),
            other => panic!("only reads may retry: {other:?}"),
        };
        apply_registry_failure(&mut state, &id);
        assert!(!state.registry_read_pending());
        assert!(state.registry_ready("p1"));

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
        let fresh = added.last().unwrap();
        assert_eq!(fresh.name, "Roads");
        assert!(fresh.visible);
        assert!(ulid::Ulid::from_string(&fresh.id).is_ok());
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
    fn new_group_ids_are_fresh_and_never_reconnect_deleted_memberships() {
        let existing = vec![
            group("utilities", "Utilities", true),
            group("utilities-2", "U", true),
        ];
        let first = next_group_id("Utilities", &existing);
        let second = next_group_id("Utilities", &existing);
        assert_ne!(first, second);
        assert!(ulid::Ulid::from_string(&first).is_ok());
        assert!(!existing.iter().any(|group| group.id == first));
        let mut state = VisibilityState::default();
        state.set_node_group_membership("old-node", vec![first.clone()]);
        state.replace_groups(vec![group(&second, "Utilities", false)]);
        assert_eq!(
            state.group_member_count(&second),
            0,
            "same-name replacement cannot reconnect old membership"
        );
    }
}
