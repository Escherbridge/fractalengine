//! Bounded identity translation of the actual full provider tree; see AGENTS.md.

use bevy_egui::egui::accesskit as ak;
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
};

const MAX_NODES: usize = 4096;
const MAX_TARGETS: usize = 16;
const MAX_REFERENCES: usize = MAX_NODES * 16;
const MAX_ISSUED: usize = 4096;
static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetIdentity {
    pub physical: ak::NodeId,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alias {
    pub physical: ak::NodeId,
    pub provider: ak::NodeId,
    pub epoch: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    set_epoch: u64,
    target: TargetIdentity,
}

struct Issued {
    key: Key,
    alias: Alias,
}

struct Ledger {
    next: u64,
    issued: Vec<Issued>,
    exhausted: bool,
}

impl Ledger {
    fn new() -> Self {
        Self { next: u64::MAX, issued: Vec::new(), exhausted: false }
    }

    fn aliases(
        &mut self,
        set_epoch: u64,
        targets: &[TargetIdentity],
        nodes: &HashMap<ak::NodeId, &ak::Node>,
    ) -> Option<Vec<Alias>> {
        if self.exhausted || self.issued.iter().any(|issued| nodes.contains_key(&issued.alias.provider)) {
            return None;
        }
        let mut next = self.next;
        let mut staged = Vec::new();
        let mut aliases = Vec::with_capacity(targets.len());
        for target in targets {
            let key = Key { set_epoch, target: *target };
            if let Some(issued) = self.issued.iter().find(|issued| issued.key == key) {
                aliases.push(issued.alias);
                continue;
            }
            if self.issued.len().checked_add(staged.len())? >= MAX_ISSUED {
                self.exhausted = true;
                return None;
            }
            let provider = ak::NodeId(next);
            let Some(after) = next.checked_sub(1).filter(|value| *value != 0) else {
                self.exhausted = true;
                return None;
            };
            if nodes.contains_key(&provider) {
                return None;
            }
            next = after;
            let alias = Alias { physical: target.physical, provider, epoch: target.epoch };
            aliases.push(alias);
            staged.push(Issued { key, alias });
        }
        self.next = next;
        self.issued.extend(staged);
        Some(aliases)
    }
}

struct FullTree<'a> {
    root: ak::NodeId,
    nodes: HashMap<ak::NodeId, &'a ak::Node>,
    blocked: HashSet<ak::NodeId>,
}

fn references(node: &ak::Node) -> impl Iterator<Item = ak::NodeId> + '_ {
    [
        node.children(), node.controls(), node.details(), node.described_by(),
        node.flow_to(), node.labelled_by(), node.owns(), node.radio_group(),
    ]
    .into_iter()
    .flatten()
    .copied()
    .chain([
        node.active_descendant(), node.error_message(), node.in_page_link_target(),
        node.member_of(), node.next_on_line(), node.previous_on_line(), node.popup_for(),
    ].into_iter().flatten())
    .chain(node.text_selection().into_iter().flat_map(|selection| {
        [selection.anchor.node, selection.focus.node]
    }))
}

fn validate(update: &ak::TreeUpdate) -> Option<FullTree<'_>> {
    if update.nodes.is_empty() || update.nodes.len() > MAX_NODES {
        return None;
    }
    let root = update.tree.as_ref()?.root;
    let mut nodes = HashMap::with_capacity(update.nodes.len());
    for (id, node) in &update.nodes {
        if nodes.insert(*id, node).is_some() {
            return None;
        }
    }
    if !nodes.contains_key(&root) || !nodes.contains_key(&update.focus) {
        return None;
    }
    let mut parents = HashSet::with_capacity(nodes.len());
    let mut reference_count = 0usize;
    for node in nodes.values() {
        for reference in references(node) {
            reference_count = reference_count.checked_add(1)?;
            if reference_count > MAX_REFERENCES || !nodes.contains_key(&reference) {
                return None;
            }
        }
        for child in node.children() {
            if *child == root || !parents.insert(*child) {
                return None;
            }
        }
    }
    if parents.len().checked_add(1)? != nodes.len() {
        return None;
    }
    let mut seen = HashSet::with_capacity(nodes.len());
    let mut blocked = HashSet::new();
    let mut stack = vec![(root, false)];
    while let Some((id, inherited)) = stack.pop() {
        if !seen.insert(id) {
            return None;
        }
        let node = *nodes.get(&id)?;
        let disabled = inherited || node.is_hidden() || node.is_disabled();
        if disabled {
            blocked.insert(id);
        }
        stack.extend(node.children().iter().map(|child| (*child, disabled)));
    }
    (seen.len() == nodes.len()).then_some(FullTree { root, nodes, blocked })
}

/// Checks a standalone full tree, independently of positive navigation eligibility.
pub fn full_tree_is_safe(update: &ak::TreeUpdate) -> bool {
    validate(update).is_some()
}

/// Returns true on unavailable ledger state so native fallback cannot admit an alias.
pub fn was_issued(id: ak::NodeId) -> bool {
    let Ok(ledger) = LEDGER.get_or_init(|| Mutex::new(Ledger::new())).try_lock() else {
        return true;
    };
    ledger.issued.iter().any(|issued| issued.alias.provider == id)
}

/// Remaps the same real buttons; epochs identify exact original immutable bindings.
pub fn remap(
    update: &ak::TreeUpdate,
    set_epoch: u64,
    targets: &[TargetIdentity],
) -> Option<(ak::TreeUpdate, Vec<Alias>)> {
    let mut ledger = LEDGER.get_or_init(|| Mutex::new(Ledger::new())).try_lock().ok()?;
    remap_with_ledger(update, set_epoch, targets, &mut ledger)
}

fn remap_with_ledger(
    update: &ak::TreeUpdate,
    set_epoch: u64,
    targets: &[TargetIdentity],
    ledger: &mut Ledger,
) -> Option<(ak::TreeUpdate, Vec<Alias>)> {
    let full = validate(update)?;
    if set_epoch == 0 || targets.is_empty() || targets.len() > MAX_TARGETS {
        return None;
    }
    let mut widgets = HashSet::new();
    let mut epochs = HashSet::new();
    for target in targets {
        let node = full.nodes.get(&target.physical)?;
        if target.epoch == 0 || !epochs.insert(target.epoch) || !widgets.insert(target.physical)
            || target.physical == full.root || full.blocked.contains(&target.physical)
            || node.role() != ak::Role::Button || !node.supports_action(ak::Action::Click)
        {
            return None;
        }
    }
    let aliases = ledger.aliases(set_epoch, targets, &full.nodes)?;
    let translate = |id: ak::NodeId| {
        aliases.iter().find(|alias| alias.physical == id).map_or(id, |alias| alias.provider)
    };
    let mut remapped = update.clone();
    for (id, node) in &mut remapped.nodes {
        *id = translate(*id);
        rewrite(node, &translate);
    }
    remapped.tree.as_mut()?.root = translate(full.root);
    remapped.focus = translate(update.focus);
    validate(&remapped)?;
    Some((remapped, aliases))
}

fn rewrite(node: &mut ak::Node, translate: &impl Fn(ak::NodeId) -> ak::NodeId) {
    macro_rules! vector {
        ($getter:ident, $setter:ident) => {
            if node.$getter().iter().any(|id| translate(*id) != *id) {
                node.$setter(node.$getter().iter().copied().map(translate).collect::<Vec<_>>());
            }
        };
    }
    vector!(children, set_children);
    vector!(controls, set_controls);
    vector!(details, set_details);
    vector!(described_by, set_described_by);
    vector!(flow_to, set_flow_to);
    vector!(labelled_by, set_labelled_by);
    vector!(owns, set_owns);
    vector!(radio_group, set_radio_group);
    macro_rules! single {
        ($getter:ident, $setter:ident) => {
            if let Some(id) = node.$getter() {
                let mapped = translate(id);
                if mapped != id {
                    node.$setter(mapped);
                }
            }
        };
    }
    single!(active_descendant, set_active_descendant);
    single!(error_message, set_error_message);
    single!(in_page_link_target, set_in_page_link_target);
    single!(member_of, set_member_of);
    single!(next_on_line, set_next_on_line);
    single!(previous_on_line, set_previous_on_line);
    single!(popup_for, set_popup_for);
    if let Some(selection) = node.text_selection().copied() {
        let mut mapped = selection;
        mapped.anchor.node = translate(selection.anchor.node);
        mapped.focus.node = translate(selection.focus.node);
        if mapped != selection {
            node.set_text_selection(mapped);
        }
    }
}

#[cfg(test)]
#[path = "provider_identity_journey.rs"]
mod journey;

#[cfg(test)]
pub(crate) fn authored_journey() {
    journey::run();
}
