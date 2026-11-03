use super::{Guard, MAX_NODES, shared};
use bevy_egui::egui::{self, accesskit as ak};
use std::collections::{HashMap, HashSet};

pub struct Snapshot {
    pub(super) lifetime: u128,
    pub(super) pass: u64,
    pub(super) guard: Guard,
    targets: HashMap<ak::NodeId, (bool, Option<egui::Id>)>,
}

impl Snapshot {
    pub fn lifetime(&self) -> u128 {
        self.lifetime
    }
    pub fn pass(&self) -> u64 {
        self.pass
    }
    pub fn guard(&self) -> Guard {
        self.guard
    }
    pub fn allows(&self, request: &ak::ActionRequest) -> bool {
        request.data.is_none()
            && self
                .targets
                .get(&request.target)
                .is_some_and(|(click, _)| request.action == ak::Action::Click && *click)
    }

    pub(super) fn confirmation(&self, target: ak::NodeId) -> Option<egui::Id> {
        self.targets.get(&target).and_then(|(_, owner)| *owner)
    }
}

pub fn complete(ctx: &egui::Context, update: &ak::TreeUpdate) -> Option<Snapshot> {
    if update.nodes.len() > MAX_NODES {
        return None;
    }
    let shared = shared(ctx)?;
    let pass = shared.try_lock().ok()?;
    if pass.invalid || pass.number.checked_add(1) != Some(ctx.cumulative_pass_nr()) {
        return None;
    }
    let guard = pass.guard?;
    let mut nodes = HashMap::with_capacity(update.nodes.len());
    for (id, node) in &update.nodes {
        if nodes.insert(*id, node).is_some() {
            return None;
        }
    }
    let mut stack = vec![(update.tree.as_ref()?.root, false)];
    let mut visited = HashSet::new();
    let mut targets = HashMap::new();
    while let Some((id, inherited)) = stack.pop() {
        if !visited.insert(id) || visited.len() > MAX_NODES {
            return None;
        }
        let node = *nodes.get(&id)?;
        if matches!(
            node.role(),
            ak::Role::PasswordInput | ak::Role::Dialog | ak::Role::AlertDialog | ak::Role::Menu
        ) {
            return None;
        }
        let disabled = inherited || node.is_disabled() || node.is_hidden();
        if let Some(widget) = pass.widgets.get(&id.0) {
            if widget.id.value() != id.0
                || !ctx.memory(|memory| memory.allows_interaction(widget.layer))
            {
                return None;
            }
            if !disabled && widget.enabled {
                targets.insert(
                    id,
                    (node.supports_action(ak::Action::Click), widget.confirmation),
                );
            }
        }
        for child in node.children().iter().rev() {
            stack.push((*child, disabled));
        }
    }
    if visited.len() != nodes.len() {
        return None;
    }
    Some(Snapshot {
        lifetime: pass.lifetime,
        pass: ctx.cumulative_pass_nr(),
        guard,
        targets,
    })
}
