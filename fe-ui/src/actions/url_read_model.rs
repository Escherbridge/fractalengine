//! Owned current URL reads; see actions/AGENTS.md section URL read model.

use std::collections::HashMap;

use bevy::prelude::Entity;
use fe_runtime::messages::{DbCommand, DbResult};

use crate::{
    navigation_manager::NavigationManager, node_manager::NodeManager, plugin::InspectorFormState,
    verse_manager::VerseManager,
};

const MAX_PENDING: usize = 256;
const READ_TIMEOUT: f64 = 30.0;
const WRITE_RETENTION: f64 = 300.0;

#[derive(Clone, PartialEq)]
struct CacheAnchor {
    scope: String,
    asset_path: Option<String>,
    has_asset: bool,
    history_version: Option<i64>,
    cache_epoch: u64,
    asset_identity: Option<Option<String>>,
    url: Option<String>,
}

impl CacheAnchor {
    fn capture(hierarchy: &VerseManager, node_id: &str) -> Option<Self> {
        let node = hierarchy.all_nodes().find(|node| node.id == node_id)?;
        if node
            .webpage_url
            .as_ref()
            .is_some_and(|url| url.len() > 8192)
        {
            return None;
        }
        Some(Self {
            scope: super::portal::node_scope(hierarchy, node_id)?,
            asset_path: node.asset_path.clone(),
            has_asset: node.has_asset,
            history_version: hierarchy.committed_versions.get(node_id).copied(),
            cache_epoch: *hierarchy.node_cache_epochs.get(node_id)?,
            asset_identity: hierarchy.url_asset_ids.get(node_id).cloned(),
            url: node.webpage_url.clone(),
        })
    }
}

struct WriteOwner {
    request_id: String,
    anchor: CacheAnchor,
    started: f64,
}

struct FormOwner {
    entity: Entity,
    baseline: String,
}

struct ReadOwner {
    request_id: String,
    anchor: CacheAnchor,
    expected_asset_id: Option<String>,
    form: Option<FormOwner>,
    started: f64,
}

#[derive(Default)]
pub(crate) struct UrlReadModel {
    writes: HashMap<String, WriteOwner>,
    reads: HashMap<String, ReadOwner>,
}

impl UrlReadModel {
    pub(crate) fn is_reading(&self, node_id: &str) -> bool {
        self.reads.contains_key(node_id)
    }
    pub(crate) fn can_write(
        &mut self,
        hierarchy: &VerseManager,
        node_id: &str,
        now: f64,
    ) -> Result<(), String> {
        self.writes
            .retain(|_, owner| now - owner.started <= WRITE_RETENTION);
        if CacheAnchor::capture(hierarchy, node_id).is_none() {
            return Err(
                "The URL object's cache identity is unavailable; reopen the object before saving"
                    .into(),
            );
        }
        if !self.writes.contains_key(node_id) && self.writes.len() >= MAX_PENDING {
            return Err(
                "Too many URL saves are waiting; retry after existing requests finish".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn track_write(
        &mut self,
        hierarchy: &VerseManager,
        node_id: &str,
        request_id: String,
        now: f64,
    ) {
        self.reads.remove(node_id);
        if let Some(anchor) = CacheAnchor::capture(hierarchy, node_id) {
            self.writes.insert(
                node_id.to_owned(),
                WriteOwner {
                    request_id,
                    anchor,
                    started: now,
                },
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn request_current(
    sender: &crossbeam::channel::Sender<DbCommand>,
    ui: &mut super::UiManager,
    hierarchy: &VerseManager,
    nav: &NavigationManager,
    nodes: &NodeManager,
    inspector: &InspectorFormState,
    node_id: &str,
    expected_asset_id: Option<String>,
    now: f64,
) -> Result<(), String> {
    request_with_baseline(
        sender,
        ui,
        hierarchy,
        nav,
        nodes,
        inspector,
        node_id,
        expected_asset_id,
        now,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn request_after_model_change(
    sender: &crossbeam::channel::Sender<DbCommand>,
    ui: &mut super::UiManager,
    hierarchy: &VerseManager,
    nav: &NavigationManager,
    nodes: &NodeManager,
    inspector: &InspectorFormState,
    node_id: &str,
    expected_asset_id: Option<String>,
    previous_url: Option<&str>,
    now: f64,
) -> Result<(), String> {
    request_with_baseline(
        sender,
        ui,
        hierarchy,
        nav,
        nodes,
        inspector,
        node_id,
        expected_asset_id,
        now,
        Some(previous_url.unwrap_or_default()),
    )
}

#[allow(clippy::too_many_arguments)]
fn request_with_baseline(
    sender: &crossbeam::channel::Sender<DbCommand>,
    ui: &mut super::UiManager,
    hierarchy: &VerseManager,
    nav: &NavigationManager,
    nodes: &NodeManager,
    inspector: &InspectorFormState,
    node_id: &str,
    expected_asset_id: Option<String>,
    now: f64,
    clean_baseline: Option<&str>,
) -> Result<(), String> {
    ui.url_read_model
        .reads
        .retain(|_, owner| now - owner.started <= READ_TIMEOUT);
    if !ui.url_read_model.reads.contains_key(node_id)
        && ui.url_read_model.reads.len() >= MAX_PENDING
    {
        return Err("Too many URL refreshes are waiting; reselect this object to retry".into());
    }
    let anchor = CacheAnchor::capture(hierarchy, node_id).ok_or_else(|| {
        "URL cache identity is unavailable or its current value exceeds the read limit".to_owned()
    })?;
    let form = nodes
        .selected
        .as_ref()
        .filter(|selected| selected.node_id == node_id)
        .filter(|_| {
            nav.active_scope().as_ref() == Some(&anchor.scope)
                && inspector.external_url
                    == clean_baseline.unwrap_or_else(|| anchor.url.as_deref().unwrap_or_default())
        })
        .map(|selected| FormOwner {
            entity: selected.entity,
            baseline: inspector.external_url.clone(),
        });
    let request_id = ulid::Ulid::new().to_string();
    let command = DbCommand::ReadNodeUrl {
        node_id: node_id.to_owned(),
        expected_scope: anchor.scope.clone(),
        expected_asset_id: expected_asset_id.clone(),
        request_id: request_id.clone(),
    };
    sender.send(command).map_err(|_| {
        bevy::log::warn!("DB channel closed while refreshing a Portal URL");
        "Could not refresh the URL; reselect this object after reconnecting".to_owned()
    })?;
    ui.url_read_model.reads.insert(
        node_id.to_owned(),
        ReadOwner {
            request_id,
            anchor,
            expected_asset_id,
            form,
            started: now,
        },
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn committed_write(
    sender: &crossbeam::channel::Sender<DbCommand>,
    ui: &mut super::UiManager,
    hierarchy: &VerseManager,
    nav: &NavigationManager,
    nodes: &NodeManager,
    inspector: &InspectorFormState,
    result: &DbResult,
    now: f64,
) {
    let DbResult::NodeUrlUpdated {
        node_id,
        request_id,
        expected_scope,
        asset_id,
        ..
    } = result
    else {
        return;
    };
    let Some(owner) = ui.url_read_model.writes.get(node_id) else {
        return;
    };
    if owner.request_id != *request_id || owner.anchor.scope != *expected_scope {
        return;
    }
    let owner = ui
        .url_read_model
        .writes
        .remove(node_id)
        .expect("owned URL write");
    let Some(current) = CacheAnchor::capture(hierarchy, node_id)
        .filter(|anchor| anchor.scope == owner.anchor.scope)
    else {
        return;
    };
    let same_cached_model = current.asset_path == owner.anchor.asset_path
        && current.has_asset == owner.anchor.has_asset
        && current.asset_identity == owner.anchor.asset_identity;
    let expected_asset = if same_cached_model {
        asset_id.clone()
    } else {
        None
    };
    if let Err(reason) = request_current(
        sender,
        ui,
        hierarchy,
        nav,
        nodes,
        inspector,
        node_id,
        expected_asset,
        now,
    ) {
        ui.show_advisory(reason, now);
    }
}

pub(crate) fn finish_read(
    ui: &mut super::UiManager,
    hierarchy: &mut VerseManager,
    nav: &NavigationManager,
    nodes: &NodeManager,
    inspector: &mut InspectorFormState,
    result: &DbResult,
    now: f64,
    sender: &crossbeam::channel::Sender<DbCommand>,
) {
    let DbResult::NodeUrlRead {
        node_id,
        expected_scope,
        request_id,
        outcome,
    } = result
    else {
        return;
    };
    let Some(owner) = ui.url_read_model.reads.get(node_id) else {
        return;
    };
    if owner.request_id != *request_id || owner.anchor.scope != *expected_scope {
        return;
    }
    let owner = ui
        .url_read_model
        .reads
        .remove(node_id)
        .expect("owned URL read");
    if now - owner.started > READ_TIMEOUT {
        return;
    }
    if CacheAnchor::capture(hierarchy, node_id).as_ref() != Some(&owner.anchor) {
        if super::portal::node_scope(hierarchy, node_id).as_ref() == Some(expected_scope) {
            if let Err(reason) = request_with_baseline(
                sender,
                ui,
                hierarchy,
                nav,
                nodes,
                inspector,
                node_id,
                None,
                now,
                owner.form.as_ref().map(|form| form.baseline.as_str()),
            ) {
                ui.show_advisory(reason, now);
            }
        }
        return;
    }
    let snapshot = match outcome {
        Ok(snapshot) => snapshot,
        Err(_) => {
            if owner.expected_asset_id.is_some() {
                if let Err(reason) = request_with_baseline(
                    sender,
                    ui,
                    hierarchy,
                    nav,
                    nodes,
                    inspector,
                    node_id,
                    None,
                    now,
                    owner.form.as_ref().map(|form| form.baseline.as_str()),
                ) {
                    ui.show_advisory(reason, now);
                }
            } else if nodes
                .selected
                .as_ref()
                .is_some_and(|selected| selected.node_id == *node_id)
                && nav.active_scope().as_ref() == Some(expected_scope)
            {
                ui.show_advisory(
                    "Stored URL refresh was not confirmed; your draft is unchanged. Reselect the object to retry",
                    now,
                );
            }
            return;
        }
    };
    if snapshot.node_id != *node_id
        || snapshot.scope != *expected_scope
        || owner
            .expected_asset_id
            .as_ref()
            .is_some_and(|asset| snapshot.asset_id.as_ref() != Some(asset))
        || snapshot.asset_id.is_some() != owner.anchor.has_asset
        || snapshot.url.as_ref().is_some_and(|url| url.len() > 8192)
    {
        return;
    }
    hierarchy.confirm_node_url(node_id, snapshot.url.clone(), snapshot.asset_id.clone());
    if let Some(form) = owner.form {
        if nodes
            .selected
            .as_ref()
            .is_some_and(|selected| selected.entity == form.entity && selected.node_id == *node_id)
            && nav.active_scope().as_ref() == Some(expected_scope)
            && inspector.external_url == form.baseline
        {
            inspector.external_url = snapshot.url.clone().unwrap_or_default();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn maintain(
    sender: &crossbeam::channel::Sender<DbCommand>,
    ui: &mut super::UiManager,
    hierarchy: &VerseManager,
    nav: &NavigationManager,
    nodes: &NodeManager,
    inspector: &InspectorFormState,
    now: f64,
) {
    let obsolete = ui.pending_url_save.as_ref().is_some_and(|pending| {
        now - pending.started > READ_TIMEOUT
            || nav.active_scope().as_deref() != Some(pending.scope.as_str())
            || nodes
                .selected
                .as_ref()
                .map(|selected| selected.node_id.as_str())
                != pending.selection.as_deref()
    });
    if obsolete {
        let pending = ui.pending_url_save.take().expect("obsolete URL form owner");
        if now - pending.started > READ_TIMEOUT {
            if let Err(reason) = request_current(
                sender,
                ui,
                hierarchy,
                nav,
                nodes,
                inspector,
                &pending.node_id,
                None,
                now,
            ) {
                ui.show_advisory(reason, now);
            }
            ui.show_advisory(
                "URL save confirmation timed out; the current stored URL is being refreshed",
                now,
            );
        }
    }
    ui.url_read_model
        .writes
        .retain(|_, owner| now - owner.started <= WRITE_RETENTION);
    let read_timed_out = ui
        .url_read_model
        .reads
        .values()
        .any(|owner| now - owner.started > READ_TIMEOUT && owner.form.is_some());
    ui.url_read_model
        .reads
        .retain(|_, owner| now - owner.started <= READ_TIMEOUT);
    if read_timed_out {
        ui.show_advisory("URL refresh timed out; reselect the object to retry", now);
    }
}
