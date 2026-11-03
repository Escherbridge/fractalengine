//! Bounded scene-property read ownership; see AGENTS.md section scene-property-reads.

use std::collections::HashMap;

use bevy::prelude::*;
use fe_runtime::{
    app::DbCommandSender,
    messages::{DbCommand, DbResult},
};

use super::{PrimitiveDescriptorCache, VerseManager};

const PREFIX: &str = "ui-scene-properties-";
const CAPACITY: usize = 256;
const DEADLINE_SECONDS: f64 = 5.0;

#[derive(Clone, PartialEq, Eq)]
struct Owner {
    petal: String,
    epoch: u64,
}

impl Owner {
    fn matches(&self, hierarchy: &VerseManager, node: &str) -> bool {
        hierarchy.petal_id_of(node) == Some(self.petal.as_str())
            && hierarchy.node_cache_epochs.get(node) == Some(&self.epoch)
    }

    fn current(hierarchy: &VerseManager, node: &str) -> Option<Self> {
        Some(Self {
            petal: hierarchy.petal_id_of(node)?.to_owned(),
            epoch: *hierarchy.node_cache_epochs.get(node)?,
        })
    }
}

enum Phase {
    Queued,
    InFlight {
        id: String,
        owner: Owner,
        deadline: f64,
    },
    Backoff {
        until: f64,
    },
}

struct Read {
    phase: Phase,
    failures: u8,
}

impl Read {
    fn fail(&mut self, now: f64) {
        self.failures = self.failures.saturating_add(1);
        self.phase = Phase::Backoff {
            until: now + f64::from(1_u32 << self.failures.min(5)),
        };
    }
}

#[derive(Default)]
pub(super) struct ScenePropertyReads {
    pending: HashMap<String, Read>,
    capacity_refused: bool,
    next_capacity_notice: f64,
}

pub(super) enum Reply {
    Unowned,
    Accepted,
    Stale,
    Failed,
}

impl ScenePropertyReads {
    pub(super) fn queue(&mut self, node: &str) -> bool {
        if self.pending.contains_key(node) {
            return true;
        }
        if self.pending.len() >= CAPACITY {
            self.capacity_refused = true;
            return false;
        }
        self.pending.insert(
            node.to_owned(),
            Read {
                phase: Phase::Queued,
                failures: 0,
            },
        );
        true
    }

    pub(super) fn remove(&mut self, node: &str) {
        self.pending.remove(node);
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
        self.capacity_refused = false;
        self.next_capacity_notice = 0.0;
    }

    #[cfg(test)]
    pub(super) fn contains(&self, node: &str) -> bool {
        self.pending.contains_key(node)
    }

    pub(super) fn route(&mut self, result: &DbResult, hierarchy: &VerseManager, now: f64) -> Reply {
        let (node, id, success) = match result {
            DbResult::NodePropertiesLoaded {
                node_id,
                correlation_id: Some(id),
                ..
            } if id.starts_with(PREFIX) => (Some(node_id.as_str()), id.as_str(), true),
            DbResult::RequestFailed { correlation_id, .. }
                if correlation_id.starts_with(PREFIX) =>
            {
                (None, correlation_id.as_str(), false)
            }
            _ => return Reply::Unowned,
        };
        let matching = self.pending.iter().find_map(|(node_id, read)| {
            matches!(&read.phase, Phase::InFlight { id: current, .. } if current == id)
                .then(|| node_id.clone())
        });
        let Some(matching) = matching else {
            return Reply::Stale;
        };
        let Some(read) = self.pending.get_mut(&matching) else {
            return Reply::Stale;
        };
        let Phase::InFlight {
            owner, deadline, ..
        } = &read.phase
        else {
            return Reply::Stale;
        };
        if !owner.matches(hierarchy, &matching) {
            read.phase = Phase::Queued;
            return Reply::Stale;
        }
        if !success || now >= *deadline {
            read.fail(now);
            return Reply::Failed;
        }
        if node != Some(matching.as_str()) {
            return Reply::Stale;
        }
        self.pending.remove(&matching);
        Reply::Accepted
    }

    pub(super) fn poll(
        &mut self,
        sender: &DbCommandSender,
        hierarchy: &VerseManager,
        now: f64,
    ) -> bool {
        let mut failed = false;
        self.pending.retain(|node, _| {
            hierarchy.petal_id_of(node).is_some() && hierarchy.node_cache_epochs.contains_key(node)
        });
        for (node, read) in &mut self.pending {
            match &read.phase {
                Phase::InFlight { owner, .. } if !owner.matches(hierarchy, node) => {
                    read.phase = Phase::Queued;
                }
                Phase::InFlight { deadline, .. } if now >= *deadline => {
                    read.fail(now);
                    failed = true;
                }
                Phase::Backoff { until } if now >= *until => read.phase = Phase::Queued,
                _ => {}
            }
            if !matches!(&read.phase, Phase::Queued) {
                continue;
            }
            let Some(owner) = Owner::current(hierarchy, node) else {
                continue;
            };
            let id = format!("{PREFIX}{}", ulid::Ulid::new());
            match sender.0.try_send(DbCommand::GetNodeProperties {
                node_id: node.clone(),
                correlation_id: Some(id.clone()),
            }) {
                Ok(()) => {
                    read.phase = Phase::InFlight {
                        id,
                        owner,
                        deadline: now + DEADLINE_SECONDS,
                    }
                }
                Err(crossbeam::channel::TrySendError::Full(_)) => break,
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    read.fail(now);
                    failed = true;
                }
            }
        }
        failed
    }
}

pub(super) fn flush_scene_property_reads(
    mut cache: ResMut<PrimitiveDescriptorCache>,
    sender: Res<DbCommandSender>,
    hierarchy: Res<VerseManager>,
    time: Res<crate::notice::NoticeClock>,
    mut ui: ResMut<crate::actions::UiManager>,
) {
    if cache
        .bypass_change_detection()
        .reads
        .poll(&sender, &hierarchy, time.elapsed_secs_f64())
    {
        ui.show_error(
            "Object properties could not be loaded; retrying shortly",
            time.elapsed_secs_f64(),
        );
    }
    let reads = &mut cache.bypass_change_detection().reads;
    if std::mem::take(&mut reads.capacity_refused)
        && time.elapsed_secs_f64() >= reads.next_capacity_notice
    {
        reads.next_capacity_notice = time.elapsed_secs_f64() + 5.0;
        ui.show_error(
            "Object property reads are busy; select the object again after current reads finish",
            time.elapsed_secs_f64(),
        );
    }
}
