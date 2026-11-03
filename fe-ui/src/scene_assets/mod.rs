//! Native asset intent and exact adoption outcomes; see AGENTS.md.

mod adoption;
mod context;
mod fetch;
#[cfg(test)]
pub(crate) mod journey;
mod view;

use bevy::prelude::*;
use fe_runtime::messages::DbResult;
use std::collections::HashSet;

use crate::{
    actions::UiManager, navigation_manager::NavigationManager, node_manager::NodeManager,
    plugin::LocalUserRole, verse_manager::VerseManager,
};

pub(crate) use view::{PanelView, render};

pub(super) struct Frame<'a> {
    manager: &'a VerseManager,
    nav: &'a NavigationManager,
    nodes: &'a NodeManager,
    role: &'a LocalUserRole,
    now: f64,
}

fn request_id() -> String {
    format!("ui-scene-asset-{}", ulid::Ulid::new())
}

#[derive(Resource, Default)]
pub struct SceneAssets {
    adoption: adoption::Adoption,
    fetches: fetch::Fetches,
    refresh: HashSet<String>,
    refresh_context: Option<(u128, u64, Option<u128>)>,
    status: Option<(String, String)>,
    notice: Option<String>,
}

impl SceneAssets {
    pub fn pending(&self) -> usize {
        self.adoption.pending() + self.fetches.pending() + self.refresh.len()
    }

    pub fn retire_for_terminal_reset(&mut self) {
        self.adoption.retire();
        self.fetches.retire();
        self.refresh.clear();
        self.refresh_context = None;
        self.status = None;
        self.notice = None;
    }

    fn set_status(&mut self, update: (String, String)) {
        if self.status.as_ref() != Some(&update) {
            self.notice = Some(update.1.clone());
            self.status = Some(update);
        }
    }

    pub(crate) fn begin_review(
        &mut self,
        node: &str,
        manager: &mut VerseManager,
        nav: &NavigationManager,
        nodes: &NodeManager,
        role: &LocalUserRole,
        now: f64,
    ) {
        let frame = Frame {
            manager,
            nav,
            nodes,
            role,
            now,
        };
        let result = self.adoption.begin(node, &frame);
        if let Err(error) = result {
            if context::owner(role)
                && manager.petal_id_of(node) == nav.active_petal_id.as_deref()
                && manager
                    .scene_asset_source(node, nav.active_petal_id.as_deref().unwrap_or_default())
                    .is_none()
            {
                if !manager.request_scene_refresh(node, nav) {
                    self.set_status((node.into(), "Current asset refresh queue is full; reopen this Petal to refresh its asset details".into()));
                    return;
                }
            }
            self.set_status((node.into(), error));
        } else {
            self.set_status((
                node.into(),
                "Reading the exact local-copy candidate and reference set".into(),
            ));
        }
    }

    pub(crate) fn confirm_review(
        &mut self,
        id: &str,
        manager: &VerseManager,
        nav: &NavigationManager,
        nodes: &NodeManager,
        role: &LocalUserRole,
        now: f64,
    ) {
        let frame = Frame {
            manager,
            nav,
            nodes,
            role,
            now,
        };
        let node = self
            .adoption
            .preview()
            .map(|preview| preview.candidate.node_id.clone())
            .unwrap_or_default();
        match self.adoption.confirm(id, &frame) {
            Ok(()) => self.set_status((node, "Checking fresh Owner access and the exact reviewed reference set before submission".into())),
            Err(error) => self.set_status((node, error)),
        }
    }

    pub(crate) fn cancel_review(&mut self) {
        let node = self
            .adoption
            .preview()
            .map(|preview| preview.candidate.node_id.clone());
        self.adoption.cancel_review();
        if let Some(node) = node {
            self.set_status((
                node,
                "Local-copy review was cancelled; no new adoption was submitted".into(),
            ));
        }
    }

    pub(crate) fn route_db(
        &mut self,
        result: &DbResult,
        manager: &VerseManager,
        nav: &NavigationManager,
        nodes: &NodeManager,
        role: &LocalUserRole,
        now: f64,
    ) {
        self.synchronize_refresh(manager, nav, role);
        let frame = Frame {
            manager,
            nav,
            nodes,
            role,
            now,
        };
        if let DbResult::SceneAssetNeeded { needed } = result {
            if let Err(error) = self.fetches.observe(needed, &frame) {
                self.set_status((needed.node_id.clone(), error));
            }
            return;
        }
        let refresh = if let DbResult::SceneAssetAdopted { request_id, .. } = result {
            self.adoption
                .accepted_context(request_id)
                .map(|context| (context, self.adoption.accepted_targets(request_id)))
        } else {
            None
        };
        if let Some(update) = self.adoption.route(result, &frame) {
            self.set_status(update);
            if let Some((context, references)) =
                refresh.filter(|(context, _)| context.can_refresh(manager, nav))
            {
                let references = if references.is_empty() {
                    vec![context.node_id]
                } else {
                    references
                };
                self.queue_refresh(references, manager, nav, role);
            }
        }
    }

    fn synchronize_refresh(
        &mut self,
        manager: &VerseManager,
        nav: &NavigationManager,
        role: &LocalUserRole,
    ) {
        let owner = (
            manager.scene_incarnation(),
            nav.intent_revision,
            manager.scene_access_generation(),
        );
        if self.refresh_context != Some(owner)
            || manager.scene_access_blocked()
            || !role.role.is_some_and(|role| role.can_view())
        {
            self.refresh.clear();
            self.refresh_context = Some(owner);
        }
    }

    fn queue_refresh(
        &mut self,
        nodes: Vec<String>,
        manager: &VerseManager,
        nav: &NavigationManager,
        role: &LocalUserRole,
    ) {
        if manager.scene_access_blocked() || !role.role.is_some_and(|role| role.can_view()) {
            return;
        }
        for node in nodes {
            if manager.petal_id_of(&node) != nav.active_petal_id.as_deref() {
                continue;
            }
            if self.refresh.len() >= crate::verse_manager::SCENE_NODE_HARD_LIMIT
                && !self.refresh.contains(&node)
            {
                self.set_status((node, "Current asset refresh backlog is full; reopen this Petal to refresh remaining objects".into()));
                break;
            }
            self.refresh.insert(node);
        }
    }

    fn update_view(&self, ui: &mut UiManager, frame: &Frame<'_>) {
        let node = frame.nodes.primary().map(|node| node.node_id.as_str());
        let preview = self
            .adoption
            .preview()
            .filter(|preview| Some(preview.candidate.node_id.as_str()) == node);
        let status = node.and_then(|node| {
            self.adoption
                .status(node)
                .or_else(|| self.fetches.status(node))
                .or_else(|| {
                    self.status
                        .as_ref()
                        .filter(|(target, _)| target == node)
                        .map(|(_, status)| status.as_str())
                })
        });
        let busy = node.is_some_and(|node| self.adoption.busy_for(node));
        let ready = self.adoption.ready();
        if !ui
            .scene_asset_view
            .matches(node, preview, status, busy, ready)
        {
            ui.scene_asset_view = PanelView {
                node_id: node.map(str::to_owned),
                preview: preview.cloned(),
                status: status.map(str::to_owned),
                busy,
                ready,
            };
        }
    }
}

pub(crate) fn advance(
    mut state: ResMut<SceneAssets>,
    mut manager: ResMut<VerseManager>,
    nav: Res<NavigationManager>,
    nodes: Res<NodeManager>,
    role: Res<LocalUserRole>,
    clock: Res<crate::notice::NoticeClock>,
    db: Res<fe_runtime::app::DbCommandSender>,
    sync: Option<Res<fe_sync::SyncCommandSenderRes>>,
    mut events: Option<ResMut<fe_sync::SceneAssetEventBuffer>>,
    mut ui: ResMut<UiManager>,
) {
    let now = clock.elapsed_secs_f64();
    manager.synchronize_scene_navigation(&nav);
    state.synchronize_refresh(&manager, &nav, &role);
    let frame = Frame {
        manager: &manager,
        nav: &nav,
        nodes: &nodes,
        role: &role,
        now,
    };
    if let Some(update) = state.adoption.pump(&frame, &db.0) {
        state.set_status(update);
    }
    let mut refresh = HashSet::new();
    let mut ready_hashes = HashSet::new();
    if let Some(events) = events.as_deref_mut() {
        if std::mem::take(&mut events.overflowed) {
            refresh.extend(state.fetches.overflow(&frame));
            state.set_status((nodes.primary().map_or_else(String::new, |node| node.node_id.clone()), "Asset transfer results overflowed; reconciling current availability without assuming completion".into()));
        }
        for event in events.events.drain(..) {
            if let fe_sync::SyncEvent::BlobReady { hash } = &event {
                ready_hashes.insert(fe_runtime::blob_store::hash_to_hex(hash));
            }
            let (nodes, update) = state.fetches.event(&event, &frame);
            refresh.extend(nodes);
            if let Some(update) = update {
                state.set_status(update);
            }
        }
    }
    if !ready_hashes.is_empty()
        && !manager.scene_access_blocked()
        && role.role.is_some_and(|role| role.can_view())
    {
        if let Some(petal) = nav.active_petal_id.as_deref() {
            refresh.extend(manager.scene_assets_with_ready_hashes(petal, &ready_hashes));
        }
    }
    let (nodes_to_refresh, update) = state
        .fetches
        .pump(&frame, sync.as_ref().map(|sync| &sync.0));
    refresh.extend(nodes_to_refresh);
    if let Some(update) = update {
        state.set_status(update);
    }
    state.queue_refresh(refresh.into_iter().collect(), &manager, &nav, &role);
    let queued: Vec<_> = state.refresh.iter().take(64).cloned().collect();
    for node in queued {
        if manager.petal_id_of(&node) != nav.active_petal_id.as_deref() {
            state.refresh.remove(&node);
            continue;
        }
        if !manager.request_scene_refresh(&node, &nav) {
            break;
        }
        state.refresh.remove(&node);
    }
    let frame = Frame {
        manager: &manager,
        nav: &nav,
        nodes: &nodes,
        role: &role,
        now,
    };
    state.update_view(&mut ui, &frame);
    if let Some(notice) = state.notice.take() {
        ui.show_advisory(notice, now);
    }
}
