//! Bounded recovery reads for admitted replica state; see AGENTS.md.

use std::collections::{HashMap, VecDeque};

use fe_runtime::messages::{DbCommand, DbResult};
use fe_runtime::scene_replication::ReplicatedNodeCurrent;

use super::VerseManager;
use crate::navigation_manager::NavigationManager;

const CAPACITY: usize = 256;
const PREFIX: &str = "ui-replica-current-";
const READ_DEADLINE: f64 = 5.0;
const PARENT_DEADLINE: f64 = 30.0;

#[derive(Clone, PartialEq, Eq)]
struct Navigation {
    revision: u64,
    verse: Option<String>,
}

impl Navigation {
    fn current(nav: &NavigationManager) -> Self {
        Self {
            revision: nav.intent_revision(),
            verse: nav.active_verse_id.clone(),
        }
    }
}

struct Owner {
    petal_id: String,
    epoch: u64,
}

impl Owner {
    fn current(node: &str, manager: &VerseManager) -> Option<Self> {
        Some(Self {
            petal_id: manager.petal_id_of(node)?.to_owned(),
            epoch: *manager.node_cache_epochs.get(node)?,
        })
    }

    fn matches(&self, node: &str, manager: &VerseManager) -> bool {
        manager.petal_id_of(node) == Some(self.petal_id.as_str())
            && manager.node_cache_epochs.get(node) == Some(&self.epoch)
    }
}

enum Phase {
    Waiting {
        until: f64,
        parent_deadline: f64,
    },
    InFlight {
        request: String,
        owner: Owner,
        deadline: f64,
    },
}

struct Read {
    navigation: Navigation,
    failures: u8,
    phase: Phase,
}

impl Read {
    fn retry(&mut self, now: f64) {
        self.failures = self.failures.saturating_add(1);
        self.phase = Phase::Waiting {
            until: now + f64::from(1_u32 << self.failures.min(5)),
            parent_deadline: now + PARENT_DEADLINE,
        };
    }
}

#[derive(Default)]
pub(super) struct ReplicaReads {
    pending: HashMap<String, Read>,
    navigation: Option<Navigation>,
    blocked: bool,
    role_refresh: Option<String>,
    role_deadline: Option<f64>,
}

#[derive(Default)]
pub(super) struct RefreshIntents(VecDeque<(String, Navigation)>);

impl RefreshIntents {
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn remove(&mut self, node_id: &str) {
        self.0.retain(|(node, _)| node != node_id);
    }
}

impl VerseManager {
    pub(super) fn invalidate_scene_access(&mut self, nav: &NavigationManager) {
        self.scene_sources.clear();
        self.current_paths.clear();
        self.scene_refresh.0.clear();
        self.replica_reads.synchronize(nav);
        self.replica_reads.pending.clear();
        self.replica_reads.blocked = true;
        self.replica_reads.role_refresh = nav.active_scope();
        self.replica_reads.role_deadline = None;
    }

    pub(super) fn accept_scene_role(&mut self, scope: &str, role: &str, nav: &NavigationManager) {
        if nav.active_scope().as_deref() != Some(scope) {
            return;
        }
        self.replica_reads.synchronize(nav);
        self.replica_reads.blocked = !fe_policy::RoleLevel::from(role).can_view();
        self.replica_reads.role_refresh = None;
        self.replica_reads.role_deadline = None;
        if self.replica_reads.blocked {
            self.replica_reads.pending.clear();
            self.scene_sources.clear();
            self.current_paths.clear();
            self.scene_refresh.0.clear();
        }
    }

    pub(super) fn queue_replica_read(
        &mut self,
        node_id: &str,
        nav: &NavigationManager,
        now: f64,
    ) -> bool {
        self.replica_reads.queue(node_id, nav, now)
    }

    pub(super) fn supersede_replica_read(&mut self, node_id: &str) {
        self.replica_reads.supersede(node_id);
    }

    pub(super) fn route_replica_read<'a>(
        &mut self,
        result: &'a DbResult,
        nav: &NavigationManager,
        now: f64,
    ) -> Reply<'a> {
        let mut reads = std::mem::take(&mut self.replica_reads);
        let reply = reads.route(result, self, nav, now);
        self.replica_reads = reads;
        reply
    }

    pub(super) fn pump_replica_reads(
        &mut self,
        nav: &NavigationManager,
        sender: &crossbeam::channel::Sender<DbCommand>,
        now: f64,
    ) -> bool {
        let mut reads = std::mem::take(&mut self.replica_reads);
        let failed = reads.pump(self, nav, sender, now);
        self.replica_reads = reads;
        failed
    }

    pub fn pending_scene_operations(&self) -> usize {
        self.replica_reads.pending.len()
            + self.scene_refresh.0.len()
            + self.current_paths.len()
            + usize::from(
                self.replica_reads.role_refresh.is_some()
                    || self.replica_reads.role_deadline.is_some(),
            )
    }

    /// Retain a bounded current-read intent from a native bridge.
    pub fn request_scene_refresh(&mut self, node_id: &str, nav: &NavigationManager) -> bool {
        let navigation = Navigation::current(nav);
        self.scene_refresh
            .0
            .retain(|(_, owner)| *owner == navigation);
        if self.scene_refresh.0.iter().any(|(node, _)| node == node_id) {
            return true;
        }
        if self.scene_refresh.0.len() >= CAPACITY {
            return false;
        }
        self.scene_refresh
            .0
            .push_back((node_id.to_owned(), navigation));
        true
    }

    pub(super) fn take_scene_refresh(&mut self, nav: &NavigationManager) -> Option<String> {
        let navigation = Navigation::current(nav);
        while let Some((node, owner)) = self.scene_refresh.0.pop_front() {
            if owner == navigation {
                return Some(node);
            }
        }
        None
    }
}

pub(super) enum Reply<'a> {
    Unowned,
    Consumed,
    Current(&'a ReplicatedNodeCurrent),
}

impl ReplicaReads {
    pub(super) fn invalidate_node(&mut self, node_id: &str) {
        if let Some(read) = self.pending.get_mut(node_id) {
            read.phase = Phase::Waiting {
                until: 0.0,
                parent_deadline: 0.0,
            };
        }
    }

    pub(super) fn synchronize(&mut self, nav: &NavigationManager) {
        let navigation = Navigation::current(nav);
        if self.navigation.as_ref() != Some(&navigation) {
            self.pending.clear();
            self.blocked = false;
            self.role_refresh = None;
            self.role_deadline = None;
            self.navigation = Some(navigation);
        }
    }

    pub(super) fn invalidate_hierarchy(&mut self) {
        for read in self.pending.values_mut() {
            read.phase = Phase::Waiting {
                until: 0.0,
                parent_deadline: 0.0,
            };
        }
    }

    pub(super) fn queue(&mut self, node: &str, nav: &NavigationManager, now: f64) -> bool {
        self.synchronize(nav);
        if self.blocked {
            return false;
        }
        let navigation = Navigation::current(nav);
        if let Some(read) = self.pending.get(node) {
            if read.navigation == navigation {
                return true;
            }
            self.pending.remove(node);
        }
        if self.pending.len() >= CAPACITY {
            return false;
        }
        self.pending.insert(
            node.to_owned(),
            Read {
                navigation,
                failures: 0,
                phase: Phase::Waiting {
                    until: now,
                    parent_deadline: now + PARENT_DEADLINE,
                },
            },
        );
        true
    }

    pub(super) fn supersede(&mut self, node: &str) {
        self.pending.remove(node);
    }

    pub(super) fn route<'a>(
        &mut self,
        result: &'a DbResult,
        manager: &VerseManager,
        nav: &NavigationManager,
        now: f64,
    ) -> Reply<'a> {
        self.synchronize(nav);
        let DbResult::ReplicatedNodeCurrentRead {
            node_id,
            request_id,
            result,
        } = result
        else {
            return Reply::Unowned;
        };
        if !request_id.starts_with(PREFIX) {
            return Reply::Unowned;
        }
        let Some(mut read) = self.pending.remove(node_id) else {
            return Reply::Consumed;
        };
        let Phase::InFlight { request, owner, .. } = &read.phase else {
            self.pending.insert(node_id.clone(), read);
            return Reply::Consumed;
        };
        if request != request_id {
            self.pending.insert(node_id.clone(), read);
            return Reply::Consumed;
        }
        if read.navigation != Navigation::current(nav) {
            return Reply::Consumed;
        }
        if !owner.matches(node_id, manager) {
            self.queue(node_id, nav, now);
            return Reply::Consumed;
        }
        match result {
            Ok(current)
                if current.state.node_id == *node_id
                    && current.state.petal_id == owner.petal_id =>
            {
                Reply::Current(current)
            }
            _ => {
                read.retry(now);
                self.pending.insert(node_id.clone(), read);
                Reply::Consumed
            }
        }
    }

    pub(super) fn pump(
        &mut self,
        manager: &VerseManager,
        nav: &NavigationManager,
        sender: &crossbeam::channel::Sender<DbCommand>,
        now: f64,
    ) -> bool {
        self.synchronize(nav);
        if self.blocked {
            if self.role_deadline.is_some_and(|deadline| now >= deadline) {
                self.role_deadline = None;
                self.role_refresh = nav.active_scope();
            }
            if let Some(scope) = self.role_refresh.as_ref() {
                match sender.try_send(DbCommand::ResolveLocalRole {
                    scope: scope.clone(),
                }) {
                    Ok(()) => {
                        self.role_refresh = None;
                        self.role_deadline = Some(now + READ_DEADLINE);
                    }
                    Err(crossbeam::channel::TrySendError::Full(_)) => {}
                    Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                        self.role_refresh = None;
                        self.role_deadline = Some(now + 2.0);
                    }
                }
            }
            return false;
        }
        let navigation = Navigation::current(nav);
        let mut failed = false;
        self.pending.retain(|node, read| {
            if read.navigation != navigation {
                return false;
            }
            match &read.phase {
                Phase::InFlight { deadline, .. } if *deadline <= now => {
                    read.retry(now);
                    failed = true;
                }
                Phase::Waiting {
                    parent_deadline, ..
                } if *parent_deadline <= now && manager.petal_id_of(node).is_none() => {
                    failed = true;
                    return false;
                }
                _ => {}
            }
            let until = match &read.phase {
                Phase::Waiting { until, .. } => *until,
                _ => return true,
            };
            if now < until {
                return true;
            }
            let Some(owner) = Owner::current(node, manager) else {
                return true;
            };
            let request = format!("{PREFIX}{}", ulid::Ulid::new());
            match sender.try_send(DbCommand::ReadReplicatedNodeCurrent {
                node_id: node.clone(),
                request_id: request.clone(),
            }) {
                Ok(()) => {
                    read.phase = Phase::InFlight {
                        request,
                        owner,
                        deadline: now + READ_DEADLINE,
                    };
                }
                Err(crossbeam::channel::TrySendError::Full(_)) => {}
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    read.retry(now);
                    failed = true;
                }
            }
            true
        });
        failed
    }
}
