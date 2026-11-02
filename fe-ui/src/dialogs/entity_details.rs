//! Target-bound settings requests and draft adoption; see dialogs/AGENTS.md.

use std::sync::atomic::{AtomicU64, Ordering};

use super::{ActiveDialog, EntitySettingsType, SettingsTab};
use crate::actions::UiManager;
use fe_runtime::messages::{DbCommand, EntityDetails, EntityType};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
const LOAD_TIMEOUT: f64 = 30.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DraftValues {
    name: String,
    access: Option<String>,
    description: Option<String>,
}

impl DraftValues {
    fn stored(details: &EntityDetails) -> Self {
        Self {
            name: details.name.clone(),
            access: details.default_access.clone(),
            description: (details.entity_type == EntityType::Fractal)
                .then(|| details.description.clone().unwrap_or_default()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Queued,
    Loading(f64),
    Ready,
    Failed,
}

#[derive(Debug, Clone)]
pub struct EntityDetailsState {
    request_id: u64,
    phase: Phase,
    baseline: Option<DraftValues>,
    pub(crate) loaded: Option<EntityDetails>,
    pub(crate) error: Option<String>,
    pub(crate) conflict: bool,
}

impl Default for EntityDetailsState {
    fn default() -> Self {
        Self {
            request_id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
            phase: Phase::Queued,
            baseline: None,
            loaded: None,
            error: None,
            conflict: false,
        }
    }
}

impl EntityDetailsState {
    pub(crate) fn refresh(&mut self) {
        self.request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        self.phase = Phase::Queued;
        self.error = None;
    }

    pub(crate) fn can_write(&self) -> bool {
        self.phase == Phase::Ready
            && !self.conflict
            && self.loaded.as_ref().is_some_and(|details| {
                fe_database::RoleLevel::from(details.role.as_str()).can_manage()
            })
    }

    pub(crate) fn loading(&self) -> bool {
        matches!(self.phase, Phase::Queued | Phase::Loading(_))
    }

    pub(crate) fn restore(
        &mut self,
        name: &mut String,
        access: &mut Option<String>,
        description: &mut Option<String>,
    ) {
        if let Some(details) = &self.loaded {
            let stored = DraftValues::stored(details);
            *name = stored.name;
            *access = stored.access;
            *description = stored.description;
            self.conflict = false;
        }
    }
}

impl EntitySettingsType {
    pub(crate) fn runtime(self) -> EntityType {
        match self {
            Self::Verse => EntityType::Verse,
            Self::Fractal => EntityType::Fractal,
            Self::Petal => EntityType::Petal,
        }
    }
}

pub(crate) fn dialog(
    kind: EntitySettingsType,
    id: &str,
    name: &str,
    verse_id: &str,
    fractal_id: Option<&str>,
) -> ActiveDialog {
    ActiveDialog::EntitySettings {
        entity_type: kind,
        entity_id: id.into(),
        entity_name: name.into(),
        parent_verse_id: verse_id.into(),
        parent_fractal_id: fractal_id.map(str::to_owned),
        details: EntityDetailsState::default(),
        active_tab: SettingsTab::General,
        name_buf: String::new(),
        default_access_buf: None,
        description_buf: None,
        access: super::entity_access::EntityAccessState::default(),
        invite_role_buf: "viewer".into(),
        invite_expiry_buf: 24,
        pending_delete: false,
        pending_claim_ownership: false,
        api_token_role_buf: "viewer".into(),
        api_token_expiry_buf: 24,
        api_token_client_buf: String::new(),
    }
}

pub(crate) fn request_if_needed(
    ui_mgr: &mut UiManager,
    sender: &crossbeam::channel::Sender<DbCommand>,
    now: f64,
) {
    let ActiveDialog::EntitySettings {
        entity_type,
        entity_id,
        name_buf,
        default_access_buf,
        description_buf,
        pending_delete,
        details,
        ..
    } = &mut ui_mgr.active_dialog
    else {
        return;
    };
    if let Phase::Loading(since) = details.phase {
        if now - since >= LOAD_TIMEOUT {
            details.phase = Phase::Failed;
            details.error =
                Some("Loading settings timed out. Your draft is retained; retry the read.".into());
        }
    }
    if details.phase != Phase::Queued {
        return;
    }
    *pending_delete = false;
    details.baseline = Some(
        details
            .loaded
            .as_ref()
            .map(DraftValues::stored)
            .unwrap_or_else(|| DraftValues {
                name: name_buf.clone(),
                access: default_access_buf.clone(),
                description: description_buf.clone(),
            }),
    );
    details.phase = Phase::Loading(now);
    if sender
        .send(DbCommand::GetEntityDetails {
            entity_type: entity_type.runtime(),
            entity_id: entity_id.clone(),
            request_id: details.request_id,
        })
        .is_err()
    {
        bevy::log::warn!("GetEntityDetails channel closed");
        details.phase = Phase::Failed;
        details.error =
            Some("Settings could not be requested. The database is unavailable.".into());
    }
}

fn adopt<T: PartialEq>(draft: &mut T, before: T, incoming: T) -> bool {
    if *draft == before || *draft == incoming {
        *draft = incoming;
        false
    } else {
        before != incoming
    }
}

pub(crate) fn apply(
    ui_mgr: &mut UiManager,
    kind: EntityType,
    id: &str,
    request_id: u64,
    result: &Result<EntityDetails, String>,
    now: f64,
) {
    let ActiveDialog::EntitySettings {
        entity_type,
        entity_id,
        entity_name,
        parent_verse_id,
        parent_fractal_id,
        name_buf,
        default_access_buf,
        description_buf,
        details,
        ..
    } = &mut ui_mgr.active_dialog
    else {
        return;
    };
    if entity_type.runtime() != kind || entity_id != id || details.request_id != request_id {
        return;
    }
    let Phase::Loading(since) = details.phase else {
        return;
    };
    details.phase = Phase::Failed;
    if now - since >= LOAD_TIMEOUT {
        details.error =
            Some("Loading settings timed out. Your draft is retained; retry the read.".into());
        return;
    }
    let incoming = match result {
        Ok(incoming) => incoming,
        Err(error) => {
            details.error = Some(error.clone());
            return;
        }
    };
    let expected_scope = match kind {
        EntityType::Verse => fe_database::build_scope(id, None, None),
        EntityType::Fractal => fe_database::build_scope(parent_verse_id, Some(id), None),
        EntityType::Petal => {
            fe_database::build_scope(parent_verse_id, parent_fractal_id.as_deref(), Some(id))
        }
    };
    if incoming.entity_type != kind
        || incoming.entity_id != id
        || incoming.verse_id != *parent_verse_id
        || incoming.fractal_id != *parent_fractal_id
        || incoming.scope != expected_scope
    {
        details.error = Some(
            "This entry's location changed. Reload the hierarchy and reopen its settings.".into(),
        );
        return;
    }
    let Some(before) = details.baseline.take() else {
        details.error = Some("Settings request state was lost. Retry the read.".into());
        return;
    };
    let values = DraftValues::stored(incoming);
    details.conflict |= adopt(name_buf, before.name, values.name);
    details.conflict |= adopt(default_access_buf, before.access, values.access);
    details.conflict |= adopt(description_buf, before.description, values.description);
    *entity_name = incoming.name.clone();
    details.loaded = Some(incoming.clone());
    details.error = None;
    details.phase = Phase::Ready;
}

pub(crate) fn refresh_target(ui_mgr: &mut UiManager, kind: EntityType, id: &str) {
    if let ActiveDialog::EntitySettings {
        entity_type,
        entity_id,
        details,
        ..
    } = &mut ui_mgr.active_dialog
    {
        if entity_type.runtime() == kind && entity_id == id {
            details.refresh();
        }
    }
}
