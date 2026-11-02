//! Target-bound declaration drafts; see dialogs/AGENTS.md (petal-requirements).

use super::ActiveDialog;
use crate::{actions::UiManager, navigation_manager::NavigationManager};
use fe_runtime::{
    messages::DbCommand,
    petal_requirements::{PetalRequirements, PetalRequirementsSnapshot},
};

#[derive(Debug, Clone)]
enum Pending {
    Load {
        id: String,
        since: f64,
    },
    Save {
        id: String,
        since: f64,
        sent: PetalRequirements,
    },
}

/// Correlated load/save state for one open requirements draft.
#[derive(Debug, Clone)]
pub struct RequirementsState {
    scope: Option<String>,
    queued: bool,
    pending: Option<Pending>,
    loaded: Option<PetalRequirementsSnapshot>,
    pub(crate) error: Option<String>,
    pub(crate) needs_review: bool,
    saved: bool,
}

impl RequirementsState {
    pub(crate) fn new(scope: Option<String>) -> Self {
        Self {
            scope,
            queued: true,
            pending: None,
            loaded: None,
            error: None,
            needs_review: false,
            saved: false,
        }
    }
    pub(crate) fn editable(&self) -> bool {
        self.loaded.is_some() && !self.queued && !matches!(self.pending, Some(Pending::Load { .. }))
    }
    pub(crate) fn can_save(&self) -> bool {
        self.editable()
            && self.pending.is_none()
            && self.error.is_none()
            && !self.needs_review
            && self.loaded.as_ref().is_some_and(|s| s.can_manage)
    }
    pub(crate) fn status(&self) -> &str {
        match self.pending {
            Some(Pending::Load { .. }) => "Loading saved requirements...",
            Some(Pending::Save { .. }) => "Saving; waiting for persisted readback...",
            None if self.queued => "Waiting to load saved requirements...",
            None if self.saved => {
                "Last submitted declaration is confirmed saved. Newer draft changes remain unsaved."
            }
            None => {
                "Persisted declaration only. Content is not installed or fetched automatically."
            }
        }
    }
    pub(crate) fn refresh(&mut self) {
        self.queued = true;
        self.pending = None;
        self.error = None;
    }
    fn current(&mut self, nav: &NavigationManager, petal: &str) -> bool {
        if self.scope.is_none()
            || self.scope != nav.active_scope()
            || nav.active_petal_id.as_deref() != Some(petal)
        {
            self.pending = None;
            self.queued = false;
            self.loaded = None;
            self.error=Some("Navigation changed. Close and reopen requirements for the current Petal; your draft is retained.".into());
            false
        } else {
            true
        }
    }
}

pub(crate) fn request_if_needed(
    ui: &mut UiManager,
    nav: &NavigationManager,
    tx: &crossbeam::channel::Sender<DbCommand>,
    now: f64,
) {
    let ActiveDialog::PetalManifest {
        petal_id,
        requirements_state: state,
        ..
    } = &mut ui.active_dialog
    else {
        return;
    };
    if !state.current(nav, petal_id) {
        return;
    }
    let since = state.pending.as_ref().map(|p| match p {
        Pending::Load { since, .. } | Pending::Save { since, .. } => *since,
    });
    if since.is_some_and(|since| now - since >= 30.0) {
        state.pending = None;
        state.queued = false;
        state.error=Some("Confirmation timed out. Reload saved data before saving again; your draft is retained.".into());
    }
    if !state.queued {
        return;
    }
    let id = format!("ui-requirements-{}", ulid::Ulid::new());
    state.queued = false;
    if tx
        .send(DbCommand::GetPetalRequirements {
            petal_id: petal_id.clone(),
            correlation_id: id.clone(),
        })
        .is_err()
    {
        bevy::log::warn!("GetPetalRequirements channel closed");
        state.error = Some("Database unavailable. Your draft is retained.".into());
    } else {
        state.pending = Some(Pending::Load { id, since: now });
    }
}

pub(crate) fn save(
    ui: &mut UiManager,
    nav: &NavigationManager,
    tx: &crossbeam::channel::Sender<DbCommand>,
    petal: &str,
    draft: PetalRequirements,
    now: f64,
) -> Result<(), String> {
    draft.validate()?;
    let ActiveDialog::PetalManifest {
        petal_id,
        manifest,
        requirements_state: state,
        ..
    } = &mut ui.active_dialog
    else {
        return Err("Requirements dialog is no longer open".into());
    };
    if petal_id.as_str() != petal
        || *manifest != draft
        || !state.current(nav, petal)
        || !state.can_save()
    {
        return Err("Requirements are not ready to save. Reload the saved data first.".into());
    }
    let expected = state
        .loaded
        .clone()
        .ok_or("Saved requirements have not loaded")?;
    let id = format!("ui-requirements-{}", ulid::Ulid::new());
    tx.send(DbCommand::SetPetalRequirements {
        expected,
        requirements: draft.clone(),
        correlation_id: id.clone(),
    })
    .map_err(|_| {
        bevy::log::warn!("SetPetalRequirements channel closed");
        "Database unavailable. Requirements were not queued.".to_string()
    })?;
    state.pending = Some(Pending::Save {
        id,
        since: now,
        sent: draft,
    });
    Ok(())
}

pub(crate) fn apply(
    ui: &mut UiManager,
    nav: &NavigationManager,
    id: &str,
    result: Result<&PetalRequirementsSnapshot, &str>,
) {
    let ActiveDialog::PetalManifest {
        petal_id,
        manifest,
        dirty,
        add_hexon_id_buf,
        add_hexon_type_buf,
        requirements_state: state,
        ..
    } = &mut ui.active_dialog
    else {
        return;
    };
    if !state.current(nav, petal_id) {
        return;
    }
    let matches = state.pending.as_ref().is_some_and(|pending| match pending {
        Pending::Load { id: request, .. } | Pending::Save { id: request, .. } => request == id,
    });
    if !matches {
        return;
    }
    let pending = state.pending.take().unwrap();
    let snapshot = match result {
        Ok(snapshot)
            if snapshot.petal_id == *petal_id && Some(&snapshot.scope) == state.scope.as_ref() =>
        {
            snapshot
        }
        Ok(_) => {
            state.error = Some(
                "Requirements reply did not match the requested Petal. Reload before saving."
                    .into(),
            );
            return;
        }
        Err(error) => {
            state.error = Some(format!(
                "{error}. Your draft is retained; reload saved data before saving again."
            ));
            return;
        }
    };
    state.saved = false;
    if let Pending::Save { sent, .. } = pending {
        if snapshot.requirements != sent {
            state.error = Some(
                "Persisted readback did not match the submitted declaration. Reload before saving."
                    .into(),
            );
            return;
        }
        state.saved = true;
    } else if !*dirty {
        *manifest = snapshot.requirements.clone();
        state.needs_review = false;
    } else {
        state.needs_review = *manifest != snapshot.requirements;
    }
    *dirty = *manifest != snapshot.requirements
        || !add_hexon_id_buf.is_empty()
        || !add_hexon_type_buf.is_empty();
    state.loaded = Some(snapshot.clone());
    state.error = None;
}

/// Resolve a retained-draft conflict only through an explicit user choice.
pub(crate) fn review(ui: &mut UiManager, use_saved: bool) {
    let ActiveDialog::PetalManifest {
        manifest,
        dirty,
        add_hexon_id_buf,
        add_hexon_type_buf,
        requirements_state: state,
        ..
    } = &mut ui.active_dialog
    else {
        return;
    };
    if let Some(loaded) = &state.loaded {
        if use_saved {
            *manifest = loaded.requirements.clone();
            add_hexon_id_buf.clear();
            add_hexon_type_buf.clear();
        }
        *dirty = *manifest != loaded.requirements
            || !add_hexon_id_buf.is_empty()
            || !add_hexon_type_buf.is_empty();
        state.needs_review = false;
    }
}
