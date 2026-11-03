use std::collections::HashMap;

use fe_runtime::{
    blob_store::BlobHash,
    scene_asset::{MAX_SCENE_ASSET_BYTES, SceneAssetNeeded},
    scene_replication::{ReplicaAssetUnavailable, ReplicaNodeAsset},
};
use fe_sync::{SyncCommand, SyncEvent};

use super::Frame;
use crate::navigation_manager::NavigationManager;
use crate::verse_manager::{SceneAdmission, VerseManager};

const CAPACITY: usize = 256;
const FETCH_SECONDS: f64 = 30.0;

struct Wanted {
    awaiting_current: bool,
    needed: SceneAssetNeeded,
    petal_id: String,
    source: SceneAdmission,
    navigation: u64,
    incarnation: u128,
    access_generation: Option<u128>,
    fractal_id: Option<String>,
}

impl Wanted {
    fn can_refresh(&self, manager: &VerseManager, nav: &NavigationManager) -> bool {
        !manager.scene_access_blocked()
            && nav.intent_revision == self.navigation
            && manager.scene_incarnation() == self.incarnation
            && nav.active_verse_id.as_deref() == Some(self.needed.verse_id.as_str())
            && nav.active_petal_id.as_deref() == Some(self.petal_id.as_str())
            && nav.active_fractal_id == self.fractal_id
            && manager.scene_access_generation() == self.access_generation
            && manager.scene_asset_id(&self.needed.node_id, &self.petal_id)
                == Some(self.needed.asset_id.as_str())
    }

    fn current(&self, manager: &VerseManager, nav: &NavigationManager) -> bool {
        self.can_refresh(manager, nav)
            && manager.is_current_scene_admission(&self.needed.node_id, &self.petal_id, self.source)
            && manager
                .scene_asset_source(&self.needed.node_id, &self.petal_id)
                .is_some_and(|source| {
                    source.scope == self.needed.scope
                        && source.provenance == &self.needed.provenance
                        && matches!(
                            source.asset,
                            ReplicaNodeAsset::Unavailable {
                                reason: ReplicaAssetUnavailable::MissingBlob,
                                ..
                            }
                        )
                })
    }
}

struct Pending {
    request_id: String,
    source: SceneAdmission,
    hash: BlobHash,
    sent_at: Option<f64>,
    not_before: f64,
    verse_id: String,
    navigation: u64,
    incarnation: u128,
    access_generation: Option<u128>,
    retired: bool,
}

impl Pending {
    fn in_context(&self, frame: &Frame<'_>) -> bool {
        self.navigation == frame.nav.intent_revision
            && self.incarnation == frame.manager.scene_incarnation()
            && self.access_generation == frame.manager.scene_access_generation()
            && frame.nav.active_verse_id.as_deref() == Some(self.verse_id.as_str())
    }
}

struct Cooldown {
    hash: BlobHash,
    attempts: u8,
    until: f64,
}

#[derive(Default)]
pub(super) struct Fetches {
    service_available: bool,
    wanted: HashMap<String, Wanted>,
    pending: HashMap<String, Pending>,
    cooldowns: HashMap<String, Cooldown>,
}

impl Fetches {
    pub fn pending(&self) -> usize {
        self.pending.len()
            + self
                .wanted
                .iter()
                .filter(|(node, wanted)| {
                    !wanted.awaiting_current && !self.pending.contains_key(*node)
                })
                .count()
    }
    pub fn retire(&mut self) {
        self.wanted.clear();
        self.pending.clear();
        self.cooldowns.clear();
    }

    pub fn observe(&mut self, needed: &SceneAssetNeeded, frame: &Frame<'_>) -> Result<(), String> {
        if frame.manager.scene_access_blocked()
            || !frame.role.role.is_some_and(|role| role.can_view())
        {
            return Ok(());
        }
        let Some(petal) = frame.nav.active_petal_id.as_deref() else {
            return Ok(());
        };
        if frame.nav.active_verse_id.as_deref() != Some(needed.verse_id.as_str())
            || frame.nav.active_scope().as_deref() != Some(needed.scope.as_str())
        {
            return Ok(());
        }
        let Some(source) = frame.manager.scene_asset_source(&needed.node_id, petal) else {
            return Ok(());
        };
        if source.scope != needed.scope
            || source.asset.asset_id() != Some(needed.asset_id.as_str())
            || source.provenance != &needed.provenance
            || !matches!(
                source.asset,
                ReplicaNodeAsset::Unavailable {
                    reason: ReplicaAssetUnavailable::MissingBlob,
                    ..
                }
            )
        {
            return Ok(());
        }
        if needed.size_bytes == 0 || needed.size_bytes > MAX_SCENE_ASSET_BYTES {
            return Err(
                "Asset exceeds the bounded peer transfer size; no download was requested".into(),
            );
        }
        if self.wanted.len() >= CAPACITY && !self.wanted.contains_key(&needed.node_id) {
            return Err(
                "Scene asset refresh queue is full; reopen this Petal to request its assets again"
                    .into(),
            );
        }
        let same_context = self.pending.get(&needed.node_id).is_some_and(|pending| {
            pending.navigation == frame.nav.intent_revision
                && pending.incarnation == frame.manager.scene_incarnation()
                && pending.access_generation == frame.manager.scene_access_generation()
        });
        if !same_context {
            self.pending.remove(&needed.node_id);
        }
        self.wanted.insert(
            needed.node_id.clone(),
            Wanted {
                awaiting_current: false,
                needed: needed.clone(),
                petal_id: petal.into(),
                source: source.admission,
                navigation: frame.nav.intent_revision,
                incarnation: frame.manager.scene_incarnation(),
                access_generation: frame.manager.scene_access_generation(),
                fractal_id: frame.nav.active_fractal_id.clone(),
            },
        );
        Ok(())
    }

    pub fn pump(
        &mut self,
        frame: &Frame<'_>,
        sender: Option<&crossbeam::channel::Sender<SyncCommand>>,
    ) -> (Vec<String>, Option<(String, String)>) {
        self.service_available = sender.is_some();
        if frame.manager.scene_access_blocked()
            || !frame.role.role.is_some_and(|role| role.can_view())
        {
            self.retire();
            return (vec![], None);
        }
        self.wanted.retain(|_, wanted| {
            wanted.can_refresh(frame.manager, frame.nav)
                && frame
                    .manager
                    .scene_asset_source(&wanted.needed.node_id, &wanted.petal_id)
                    .is_none_or(|source| {
                        !matches!(
                            source.asset,
                            ReplicaNodeAsset::LocalBlob { .. } | ReplicaNodeAsset::NoAsset
                        )
                    })
        });
        self.pending.retain(|node, pending| {
            pending.navigation == frame.nav.intent_revision
                && pending.incarnation == frame.manager.scene_incarnation()
                && pending.access_generation == frame.manager.scene_access_generation()
                && (pending.sent_at.is_some()
                    || self.wanted.get(node).is_some_and(|wanted| {
                        wanted.source == pending.source && wanted.current(frame.manager, frame.nav)
                    }))
        });
        for (node, pending) in &mut self.pending {
            pending.retired = self.wanted.get(node).is_none_or(|wanted| {
                wanted.source != pending.source || !wanted.current(frame.manager, frame.nav)
            });
        }
        for (node, wanted) in &self.wanted {
            if self.pending.len() >= CAPACITY {
                break;
            }
            if wanted.awaiting_current
                || self.pending.contains_key(node)
                || !wanted.current(frame.manager, frame.nav)
            {
                continue;
            }
            let not_before = self
                .cooldowns
                .get(node)
                .filter(|cooldown| cooldown.hash == wanted.needed.content_hash)
                .map_or(frame.now, |cooldown| cooldown.until);
            self.pending.insert(
                node.clone(),
                Pending {
                    request_id: super::request_id(),
                    source: wanted.source,
                    hash: wanted.needed.content_hash,
                    sent_at: None,
                    not_before,
                    verse_id: wanted.needed.verse_id.clone(),
                    navigation: wanted.navigation,
                    incarnation: wanted.incarnation,
                    access_generation: wanted.access_generation,
                    retired: false,
                },
            );
        }
        self.cooldowns
            .retain(|node, _| self.wanted.contains_key(node));
        let mut refresh = Vec::new();
        let mut notice = None;
        let mut finished = Vec::new();
        for (node, pending) in &mut self.pending {
            if pending
                .sent_at
                .is_some_and(|sent| frame.now >= sent + FETCH_SECONDS)
            {
                refresh.push(node.clone());
                finished.push(node.clone());
                notice = Some((node.clone(), "Asset transfer outcome is unconfirmed; refreshing current availability without retrying an accepted request".into()));
                continue;
            }
            if pending.sent_at.is_some() || frame.now < pending.not_before {
                continue;
            }
            let Some(sender) = sender else {
                continue;
            };
            let wanted = &self.wanted[node];
            match sender.try_send(SyncCommand::FetchSceneAsset {
                hash: pending.hash,
                verse_id: wanted.needed.verse_id.clone(),
                request_id: pending.request_id.clone(),
            }) {
                Ok(()) => pending.sent_at = Some(frame.now),
                Err(crossbeam::channel::TrySendError::Full(_)) => {}
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    refresh.push(node.clone());
                    finished.push(node.clone());
                    notice = Some((
                        node.clone(),
                        "Scene transfer service is unavailable; refreshing stored availability"
                            .into(),
                    ));
                }
            }
        }
        for node in finished {
            self.defer(&node, frame.now);
            self.pending.remove(&node);
            if let Some(wanted) = self.wanted.get_mut(&node) {
                wanted.awaiting_current = true;
            }
        }
        (refresh, notice)
    }

    fn defer(&mut self, node: &str, now: f64) {
        let Some(wanted) = self.wanted.get(node) else {
            return;
        };
        let attempts = self
            .cooldowns
            .get(node)
            .filter(|cooldown| cooldown.hash == wanted.needed.content_hash)
            .map_or(1, |cooldown| cooldown.attempts.saturating_add(1));
        self.cooldowns.insert(
            node.into(),
            Cooldown {
                hash: wanted.needed.content_hash,
                attempts,
                until: now + f64::from(1_u32 << attempts.min(5)),
            },
        );
    }

    pub fn event(
        &mut self,
        event: &SyncEvent,
        frame: &Frame<'_>,
    ) -> (Vec<String>, Option<(String, String)>) {
        if frame.manager.scene_access_blocked()
            || !frame.role.role.is_some_and(|role| role.can_view())
        {
            self.retire();
            return (vec![], None);
        }
        let mut refresh = Vec::new();
        let mut notice = None;
        match event {
            SyncEvent::SceneAssetFetchSettled {
                verse_id,
                hash,
                request_ids,
                result,
            } => {
                let matched: Vec<_> = self
                    .pending
                    .iter()
                    .filter(|(_, pending)| {
                        pending.hash == *hash
                            && pending.in_context(frame)
                            && pending.sent_at.is_some()
                            && request_ids.iter().any(|id| id == &pending.request_id)
                            && pending.verse_id == *verse_id
                    })
                    .map(|(node, _)| node.clone())
                    .collect();
                for node in matched {
                    self.pending.remove(&node);
                    if self.wanted.get(&node).is_some_and(|wanted| wanted.can_refresh(frame.manager, frame.nav)) { refresh.push(node.clone()); }
                    else if frame.nav.active_verse_id.as_deref() == Some(verse_id.as_str()) && frame.nav.active_petal_id.as_deref().is_some_and(|petal| {
                        frame.manager.scene_asset_source(&node, petal).is_some_and(|source| matches!(source.asset, ReplicaNodeAsset::LocalBlob { content_hash, .. }
                            if fe_runtime::blob_store::hash_from_hex(content_hash).is_ok_and(|known| known == *hash)))
                    }) { refresh.push(node.clone()); }
                    if let Some(wanted) = self.wanted.get_mut(&node) {
                        wanted.awaiting_current = true;
                    }
                    if let Err(error) = result {
                        self.defer(&node, frame.now);
                        notice = Some((
                            node,
                            format!(
                                "Asset transfer did not confirm readiness: {error}. Current availability is being refreshed."
                            ),
                        ));
                    }
                }
            }
            SyncEvent::BlobReady { hash } => {
                for (node, wanted) in &self.wanted {
                    if wanted.needed.content_hash == *hash
                        && wanted.can_refresh(frame.manager, frame.nav)
                    {
                        refresh.push(node.clone());
                    }
                }
            }
            _ => {}
        }
        (refresh, notice)
    }

    pub fn overflow(&mut self, frame: &Frame<'_>) -> Vec<String> {
        let refresh = self
            .pending
            .iter()
            .filter(|(node, pending)| {
                pending.in_context(frame)
                    && frame
                        .nav
                        .active_petal_id
                        .as_deref()
                        .is_some_and(|petal| frame.manager.petal_id_of(node) == Some(petal))
            })
            .map(|(node, _)| node.clone())
            .collect();
        let nodes: Vec<_> = self.pending.keys().cloned().collect();
        for node in nodes {
            self.defer(&node, frame.now);
        }
        for wanted in self.wanted.values_mut() {
            wanted.awaiting_current = true;
        }
        self.pending.clear();
        refresh
    }

    pub fn status(&self, node: &str) -> Option<&'static str> {
        self.pending.get(node).map(|pending| if pending.retired { "A transfer from an earlier object state is awaiting its exact outcome; current availability is separate" }
            else if pending.sent_at.is_some() { "Downloading scene asset; awaiting an owned transfer result" } else if !self.service_available { "Waiting for the scene transfer service; no download was submitted" } else { "Scene asset download is queued" })
    }
}
