//! Target-bound access state and confirmed controls; see dialogs/AGENTS.md.

use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use bevy_egui::egui;
use fe_runtime::{
    entity_access::{
        EntityAccessAction, EntityAccessSnapshot, EntityAccessTarget, EntityGrantRecord,
    },
    messages::DbCommand,
};

use super::{ActiveDialog, SettingsTab};
use crate::{accessibility::ResponseExt, actions::UiManager, theme};

#[derive(Debug, Clone)]
struct Pending {
    id: String,
    target: EntityAccessTarget,
    started: Instant,
    secret: bool,
}

#[derive(Debug, Clone)]
pub struct EntityAccessState {
    target: Option<EntityAccessTarget>,
    pending: Option<Pending>,
    pub(crate) snapshot: Option<EntityAccessSnapshot>,
    queued_read: bool,
    read_required: bool,
    pub(crate) error: Option<&'static str>,
    role_drafts: BTreeMap<String, String>,
    confirmed_request_id: String,
    api_open: bool,
    reveal_secret: bool,
    pub(crate) invitation: crate::invitations::IssueState,
    /// Peers still to revoke for the Managers-only preset; one request in flight at a time.
    bulk_revokes: VecDeque<String>,
    bulk_active: bool,
    /// The scope's verse has no replica namespace (seeded local-only verse), so it cannot be shared.
    verse_local_only: bool,
}

/// One of the three permission presets of the Access tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessPreset {
    Public,
    MembersOnly,
    ManagersOnly,
}

const LOCAL_ONLY_REASON: &str = "This verse is local only. Create a new verse to share with peers.";

impl Default for EntityAccessState {
    fn default() -> Self {
        Self {
            target: None,
            pending: None,
            snapshot: None,
            queued_read: false,
            read_required: true,
            error: None,
            role_drafts: BTreeMap::new(),
            confirmed_request_id: String::new(),
            api_open: false,
            reveal_secret: false,
            invitation: crate::invitations::IssueState::default(),
            bulk_revokes: VecDeque::new(),
            bulk_active: false,
            verse_local_only: false,
        }
    }
}

impl EntityAccessState {
    pub(crate) fn refresh(&mut self) {
        self.bulk_revokes.clear();
        self.bulk_active = false;
        self.invitation.reset();
        self.queued_read = true;
        self.error = None;
    }
    pub(crate) fn pending_work(&self) -> bool {
        self.waiting() || self.queued_read || self.invitation.busy() || !self.bulk_revokes.is_empty()
    }
    /// Queues a revoke for every peer whose strongest live grant here is below Manager.
    pub(crate) fn start_managers_only(&mut self) {
        self.bulk_revokes = self.below_manager_peers().into();
        self.bulk_active = !self.bulk_revokes.is_empty();
    }
    /// Distinct peers (sorted) a Revoke here would demote: an Owner's revoke blocks the cell, so any
    /// peer whose strongest live grant is viewer/editor; a Manager's revoke withdraws only its own statement.
    fn below_manager_peers(&self) -> Vec<String> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let local_is_owner = fe_database::RoleLevel::from(snapshot.local_role.as_str())
            == fe_database::RoleLevel::Owner;
        let effective_owner = |did: &str| {
            snapshot
                .roles
                .iter()
                .any(|(peer, role)| peer == did && role == "owner")
        };
        let mut peers: Vec<String> = snapshot
            .grants
            .iter()
            .filter(|grant| !grant.revoked && matches!(grant.role.as_str(), "viewer" | "editor"))
            .filter(|grant| local_is_owner || grant.grantor_did == snapshot.local_did)
            .map(|grant| grant.peer_did.clone())
            .filter(|did| *did != snapshot.local_did && !effective_owner(did.as_str()))
            .filter(|did| {
                !local_is_owner
                    || matches!(
                        strongest_live_grant(&snapshot.grants, did),
                        Some("viewer" | "editor")
                    )
            })
            .collect();
        peers.sort();
        peers.dedup();
        peers
    }
    pub(crate) fn waiting(&self) -> bool {
        self.pending.is_some()
    }
    fn leave_api(&mut self) {
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.minted = None;
        }
        if self.pending.as_ref().is_some_and(|p| p.secret) {
            self.pending = None;
            self.read_required = true;
        }
        self.reveal_secret = false;
    }
    fn synchronize(&mut self, target: &EntityAccessTarget, is_api: bool) {
        if self.target.as_ref() != Some(target) {
            let queued = self.queued_read;
            let local_only = self.verse_local_only;
            *self = Self::default();
            self.target = Some(target.clone());
            self.queued_read = queued;
            self.verse_local_only = local_only;
        }
        if self.api_open && !is_api {
            self.leave_api();
        }
        self.api_open = is_api;
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.started.elapsed() >= Duration::from_secs(30))
        {
            self.pending = None;
            self.read_required = true;
            self.snapshot = None;
            self.bulk_revokes.clear();
            self.bulk_active = false;
            self.error=Some("Access request timed out; its outcome is unconfirmed. Refresh before making another change.");
        }
    }
}

/// Records whether the open dialog's verse is local only; `namespace` maps a verse id to `Some(true)` when it has no replica namespace.
pub(crate) fn mark_verse_local_only(
    dialog: &mut ActiveDialog,
    namespace: impl FnOnce(&str) -> Option<bool>,
) {
    let ActiveDialog::EntitySettings {
        access, details, ..
    } = dialog
    else {
        return;
    };
    access.verse_local_only = details
        .loaded
        .as_ref()
        .and_then(|stored| namespace(&stored.verse_id))
        .unwrap_or(false);
}

fn current_target(dialog: &ActiveDialog) -> Option<EntityAccessTarget> {
    let ActiveDialog::EntitySettings {
        entity_type,
        entity_id,
        details,
        ..
    } = dialog
    else {
        return None;
    };
    let stored = details.loaded.as_ref()?;
    (stored.entity_type == entity_type.runtime() && stored.entity_id == *entity_id).then(|| {
        EntityAccessTarget {
            entity_type: stored.entity_type,
            entity_id: stored.entity_id.clone(),
            scope: stored.scope.clone(),
        }
    })
}

pub(crate) fn service(
    ui_mgr: &mut UiManager,
    sender: &crossbeam::channel::Sender<DbCommand>,
    peers: Option<&fe_runtime::PeerRegistry>,
) {
    let Some(target) = current_target(&ui_mgr.active_dialog) else {
        return;
    };
    let ActiveDialog::EntitySettings {
        access,
        details,
        active_tab,
        ..
    } = &mut ui_mgr.active_dialog
    else {
        return;
    };
    access.synchronize(&target, *active_tab == SettingsTab::ApiAccess);
    if !details
        .loaded
        .as_ref()
        .is_some_and(|d| fe_database::RoleLevel::from(d.role.as_str()).can_manage())
    {
        access.pending = None;
        access.snapshot = None;
        access.read_required = true;
        return;
    }
    if access.queued_read && !access.waiting() {
        access.queued_read = false;
        let mut peer_dids: Vec<String> = peers
            .into_iter()
            .flat_map(|p| p.iter().map(|(did, _)| did.clone()))
            .take(257)
            .collect();
        peer_dids.sort();
        peer_dids.dedup();
        queue(
            access,
            target,
            EntityAccessAction::Load { peer_dids },
            sender,
        );
    } else if !access.queued_read && !access.waiting() && !access.read_required {
        // Managers-only preset: next revoke only after the previous one's confirmed reply.
        if let Some(peer_did) = access.bulk_revokes.pop_front() {
            queue(access, target, EntityAccessAction::Revoke { peer_did }, sender);
        }
    }
}

fn queue(
    state: &mut EntityAccessState,
    target: EntityAccessTarget,
    action: EntityAccessAction,
    sender: &crossbeam::channel::Sender<DbCommand>,
) {
    if state.waiting() {
        return;
    }
    let id = format!("entity-access-{}", ulid::Ulid::new());
    let secret = matches!(action, EntityAccessAction::MintToken { .. });
    if sender
        .send(DbCommand::EntityAccess {
            target: target.clone(),
            action,
            request_id: id.clone(),
        })
        .is_err()
    {
        bevy::log::warn!("Entity access channel closed");
        state.error = Some("The database is unavailable. Nothing was sent.");
        state.read_required = true;
        return;
    }
    state.pending = Some(Pending {
        id,
        target,
        started: Instant::now(),
        secret,
    });
    state.error = None;
}

pub(crate) fn submit(
    ui_mgr: &mut UiManager,
    action: EntityAccessAction,
    sender: &crossbeam::channel::Sender<DbCommand>,
) {
    let Some(target) = current_target(&ui_mgr.active_dialog) else {
        return;
    };
    let ActiveDialog::EntitySettings {
        access,
        details,
        active_tab,
        ..
    } = &mut ui_mgr.active_dialog
    else {
        return;
    };
    access.synchronize(&target, *active_tab == SettingsTab::ApiAccess);
    if access.read_required
        || access.waiting()
        || !details
            .loaded
            .as_ref()
            .is_some_and(|d| fe_database::RoleLevel::from(d.role.as_str()).can_manage())
    {
        return;
    }
    access.invitation.reset();
    queue(access, target, action, sender);
}

pub(crate) fn apply(
    ui_mgr: &mut UiManager,
    target: &EntityAccessTarget,
    id: &str,
    result: &Result<EntityAccessSnapshot, String>,
) {
    if current_target(&ui_mgr.active_dialog).as_ref() != Some(target) {
        return;
    }
    let ActiveDialog::EntitySettings {
        access, active_tab, ..
    } = &mut ui_mgr.active_dialog
    else {
        return;
    };
    access.synchronize(target, *active_tab == SettingsTab::ApiAccess);
    if !access
        .pending
        .as_ref()
        .is_some_and(|p| p.id == id && p.target == *target)
    {
        return;
    }
    access.pending = None;
    match result {
        Ok(snapshot) if snapshot.minted.is_some() && *active_tab != SettingsTab::ApiAccess => {
            access.leave_api();
            access.read_required = true;
            access.bulk_revokes.clear();
            access.bulk_active = false;
        }
        Ok(snapshot) if fe_database::RoleLevel::from(snapshot.local_role.as_str()).can_manage() => {
            access.snapshot = Some(snapshot.clone());
            access.read_required = false;
            access.error = None;
            access.role_drafts.clear();
            access.confirmed_request_id = id.to_owned();
            access.reveal_secret = false;
            if access.bulk_active && access.bulk_revokes.is_empty() {
                // Last preset revoke confirmed: finish with the normal read-back.
                access.bulk_active = false;
                access.queued_read = true;
            }
        }
        _ => {
            access.snapshot = None;
            access.read_required = true;
            access.error = Some(if access.bulk_active {
                "Managers only stopped: a revoke was not confirmed. Refresh stored access to see what changed."
            } else {
                "The access operation was not confirmed. Refresh stored access before making another change."
            });
            access.bulk_revokes.clear();
            access.bulk_active = false;
        }
    }
}

/// Segmented Public / Members only / Managers only control; the caller sends the commands.
pub(crate) fn preset_controls(
    ui: &mut egui::Ui,
    state: &mut EntityAccessState,
    current_default: Option<&str>,
    verse_scope: bool,
    can_set_default: bool,
) -> Option<AccessPreset> {
    let manages = state
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| fe_database::RoleLevel::from(snapshot.local_role.as_str()).can_manage());
    if !manages {
        return None;
    }
    let enabled = !state.waiting() && !state.read_required && !state.invitation.busy();
    let revoke_count = state.below_manager_peers().len();
    let mut chosen = None;
    ui.label("Permission preset");
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(enabled && verse_scope && can_set_default, |ui| {
            for (preset, label, stored) in [
                (AccessPreset::Public, "Public", "viewer"),
                (AccessPreset::MembersOnly, "Members only", "none"),
            ] {
                if ui
                    .selectable_label(current_default == Some(stored), label)
                    .named(ui, format_args!("Preset {label}"))
                    .clicked()
                {
                    chosen = Some(preset);
                }
            }
        });
        let sets_default = verse_scope && can_set_default;
        ui.add_enabled_ui(enabled && (sets_default || revoke_count > 0), |ui| {
            let confirm_label = format!(
                "Confirm: {}revoke {revoke_count} grant{}",
                if sets_default { "default none, " } else { "" },
                if revoke_count == 1 { "" } else { "s" }
            );
            if crate::panels::widgets::confirm_button(
                ui,
                ("preset_managers_only", &state.confirmed_request_id, revoke_count),
                "Managers only...",
                &confirm_label,
                "Managers only preset",
            ) {
                chosen = Some(AccessPreset::ManagersOnly);
            }
        });
    });
    if !verse_scope {
        ui.colored_label(
            theme::TEXT_DIM,
            "Public and Members only are set on the verse. Managers only revokes viewer and editor grants here.",
        );
    }
    chosen
}

pub(crate) fn role_controls(
    ui: &mut egui::Ui,
    state: &mut EntityAccessState,
    peers: Option<&fe_runtime::PeerRegistry>,
) -> Option<EntityAccessAction> {
    let mut action = None;
    if ui
        .add_enabled(!state.waiting(), egui::Button::new("Refresh stored access"))
        .named(ui, "Refresh entity access")
        .clicked()
    {
        state.refresh();
    }
    if state.waiting() {
        ui.spinner();
        ui.label("Waiting for stored access...");
    }
    if let Some(error) = state.error {
        ui.colored_label(theme::STATUS_OFFLINE, error);
    }
    let Some(snapshot) = state.snapshot.as_ref() else {
        ui.label("Load current effective roles before changing access.");
        return action;
    };
    if let Some(notice) = &snapshot.notice {
        ui.label(notice);
    }
    let owner =
        fe_database::RoleLevel::from(snapshot.local_role.as_str()) == fe_database::RoleLevel::Owner;
    let enabled = !state.waiting() && !state.read_required && !state.invitation.busy();
    let inherited = inherited_label(state.target.as_ref().map(|target| target.entity_type));
    let mut order: Vec<usize> = (0..snapshot.roles.len()).collect();
    order.sort_by_cached_key(|&index| {
        let (did, role) = &snapshot.roles[index];
        (role_rank(role), peer_label(peers, did))
    });
    for (did, role) in order.into_iter().map(|index| &snapshot.roles[index]) {
        ui.push_id(did,|ui|{
            ui.separator();
            let online=peers.and_then(|registry|registry.get(did)).is_some_and(|p|p.is_online);
            ui.horizontal(|ui|{
                online_dot(ui,online);
                ui.add(egui::Label::new(peer_label(peers,did)).truncate()).on_hover_text(did);
                if role=="owner" {ui.strong("Owner");}
            });
            ui.label(format!("Effective role: {role}"));
            if role=="owner" {ui.label("Ownership is managed separately.");return;}
            match strongest_live_grant(&snapshot.grants,did) {
                Some("none") => {ui.label("Blocked here");}
                Some(granted) => {ui.label(format!("Granted here: {granted}"));}
                None if role!="none" => {ui.colored_label(theme::TEXT_DIM,inherited);}
                None => {}
            }
            ui.add_enabled_ui(enabled,|ui|{
                let draft=state.role_drafts.entry(did.clone()).or_insert_with(||role.clone());
                egui::ComboBox::from_id_salt("role_draft").selected_text(draft.as_str()).show_ui(ui,|ui|{
                    for value in ["viewer","editor","manager"] {ui.selectable_value(draft,value.to_owned(),value).observed(ui);}
                    if owner {ui.selectable_value(draft,"none".into(),"none (owner pin)").observed(ui);}
                }).response.named(ui,format_args!("Role to assign for {did}"));
                let grantable=matches!(draft.as_str(),"viewer"|"editor"|"manager") || (owner && draft=="none");
                ui.add_enabled_ui(grantable,|ui| {
                    if crate::panels::widgets::confirm_button(ui,("assign_role",&state.confirmed_request_id,did,draft.as_str()),"Save role...","Confirm role change",format!("Assign role {draft} to {did}")) {
                        action=Some(EntityAccessAction::Assign {peer_did:did.clone(),role:draft.clone()});
                    }
                });
                let revoke_label=if owner {"Block access here..."}else{"Revoke my grant..."};
                if owner {ui.label("Owner revocation blocks this scope. Other grants may still provide access elsewhere.");}
                if crate::panels::widgets::confirm_button(ui,("revoke_role",&state.confirmed_request_id,did),revoke_label,"Confirm revoke",format!("Revoke role grant for {did}")) {
                    action=Some(EntityAccessAction::Revoke {peer_did:did.clone()});
                }
            });
        });
    }
    ui.separator();
    grant_record(ui, snapshot, peers);
    action
}

/// Where an effective role without a grant at this scope comes from, by target depth.
fn inherited_label(entity_type: Option<fe_runtime::messages::EntityType>) -> &'static str {
    use fe_runtime::messages::EntityType;
    match entity_type {
        Some(EntityType::Verse) => "Inherited from verse default access",
        Some(EntityType::Fractal) => "Inherited from verse",
        Some(EntityType::Petal) | None => "Inherited from fractal or verse",
    }
}

/// Privilege order for listing: owner first, none last.
fn role_rank(role: &str) -> u8 {
    match role {
        "owner" => 0,
        "manager" => 1,
        "editor" => 2,
        "viewer" => 3,
        _ => 4,
    }
}

/// Strongest unrevoked role any grantor states for `did` at exactly this scope.
fn strongest_live_grant<'a>(grants: &'a [EntityGrantRecord], did: &str) -> Option<&'a str> {
    grants
        .iter()
        .filter(|grant| !grant.revoked && grant.peer_did == did)
        .map(|grant| grant.role.as_str())
        .min_by_key(|role| role_rank(role))
}

/// Registry display name, else a char-safe shortened DID (stored DIDs may be non-ASCII).
fn peer_label(peers: Option<&fe_runtime::PeerRegistry>, did: &str) -> String {
    if let Some(name) = peers
        .and_then(|registry| registry.get(did))
        .and_then(|entry| entry.display_name.as_deref())
    {
        return name.to_owned();
    }
    let count = did.chars().count();
    if count <= 12 {
        return did.to_owned();
    }
    let head: String = did.chars().take(8).collect();
    let tail: String = did.chars().skip(count - 4).collect();
    format!("{head}...{tail}")
}

/// Presence dot: filled when online, hollow when offline or unknown.
fn online_dot(ui: &mut egui::Ui, online: bool) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    if online {
        ui.painter()
            .circle_filled(rect.center(), 4.0, theme::STATUS_ONLINE_DOT);
    } else {
        ui.painter().circle_stroke(
            rect.center(),
            3.5,
            egui::Stroke::new(1.0, theme::STATUS_OFFLINE_DOT),
        );
    }
    response.on_hover_text(if online { "Online" } else { "Offline" });
}

/// Collapsible list of the latest statement per (peer, grantor) at this scope.
fn grant_record(
    ui: &mut egui::Ui,
    snapshot: &EntityAccessSnapshot,
    peers: Option<&fe_runtime::PeerRegistry>,
) {
    egui::CollapsingHeader::new(format!("Grant record ({})", snapshot.grants.len()))
        .id_salt("entity_grant_record")
        .default_open(false)
        .show(ui, |ui| {
            if snapshot.grants.is_empty() {
                ui.label("No grants stated at this scope.");
                return;
            }
            ui.label("Latest statement per grantor, newest first.");
            for grant in &snapshot.grants {
                let status = if grant.revoked { "revoked" } else { grant.role.as_str() };
                let grantor = if grant.grantor_did == snapshot.local_did {
                    "you".to_owned()
                } else {
                    peer_label(peers, &grant.grantor_did)
                };
                ui.add(
                    egui::Label::new(format!(
                        "{}: {status}, granted by {grantor}",
                        peer_label(peers, &grant.peer_did)
                    ))
                    .truncate(),
                )
                .on_hover_text(format!(
                    "{} by {} (clock {})",
                    grant.peer_did, grant.grantor_did, grant.hlc
                ));
            }
        });
}

pub(crate) fn token_controls(
    ui: &mut egui::Ui,
    state: &mut EntityAccessState,
) -> Option<EntityAccessAction> {
    let mut action = None;
    if ui
        .add_enabled(!state.waiting(), egui::Button::new("Refresh token list"))
        .named(ui, "Refresh entity API tokens")
        .clicked()
    {
        state.refresh();
    }
    if let Some(error) = state.error {
        ui.colored_label(theme::STATUS_OFFLINE, error);
    }
    if state.waiting() {
        ui.spinner();
        ui.label("Waiting for stored token metadata...");
    }
    let Some(snapshot) = &state.snapshot else {
        ui.label("Load stored access to view tokens for this entry.");
        return None;
    };
    let enabled = !state.waiting() && !state.read_required && !state.invitation.busy();
    if let Some(notice) = &snapshot.notice {
        ui.label(notice);
    }
    if snapshot.tokens.is_empty() {
        ui.label("No active token metadata in this loaded list.");
    }
    if snapshot.tokens_truncated {
        ui.label("This token list is bounded. Some later active tokens may not be shown.");
    }
    for token in &snapshot.tokens {
        ui.push_id((&token.jti, &token.sub, &token.scope), |ui| {
            ui.separator();
            ui.add(egui::Label::new(token.label.as_deref().unwrap_or(&token.jti)).truncate())
                .on_hover_text(&token.jti);
            ui.add(egui::Label::new(format!("{} / {}", token.max_role, token.scope)).truncate())
                .on_hover_text(&token.scope);
            ui.add(egui::Label::new(format!("Issuer: {}", token.sub)).truncate())
                .on_hover_text(&token.sub);
            ui.label(crate::panels::inspector::token_expiry_label(
                &token.expires_at,
                chrono::Utc::now(),
            ))
            .on_hover_text(&token.expires_at);
            ui.add_enabled_ui(enabled, |ui| {
                if crate::panels::widgets::confirm_button(
                    ui,
                    (
                        "revoke_token",
                        &state.confirmed_request_id,
                        &token.jti,
                        &token.sub,
                        &token.scope,
                    ),
                    "Revoke token...",
                    "Confirm token revoke",
                    format!("Revoke API token {}", token.jti),
                ) {
                    action = Some(EntityAccessAction::RevokeToken {
                        jti: token.jti.clone(),
                        sub: token.sub.clone(),
                        scope: token.scope.clone(),
                    });
                }
            });
        });
    }
    action
}

pub(crate) fn select_tab(tab: &mut SettingsTab, state: &mut EntityAccessState, next: SettingsTab) {
    if *tab == SettingsTab::ApiAccess && next != SettingsTab::ApiAccess {
        state.leave_api();
    }
    if next != SettingsTab::Access {
        state.invitation.reset();
    }
    *tab = next;
    if matches!(next, SettingsTab::Access | SettingsTab::ApiAccess) {
        state.refresh();
    }
}

pub(crate) fn invite_controls(
    ui: &mut egui::Ui,
    state: &mut EntityAccessState,
    role: &mut String,
    expiry: &mut u32,
    peers: Option<&fe_runtime::PeerRegistry>,
) -> bool {
    ui.separator();
    ui.label("Invite a peer to this scope");
    let can_issue = state
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| matches!(snapshot.local_role.as_str(),"owner"|"manager"));
    if !can_issue {
        state.invitation.reset();
        ui.label("Only the current Owner or an authorized Manager can issue addressed invitations.");
        return false;
    }
    if state.snapshot.as_ref().is_some_and(|snapshot|snapshot.local_role=="manager") {
        ui.label("Manager invitations require a current Owner-rooted proof. Older Manager grants need Owner reauthorization; choose a lifetime within that proof.");
    }
    if state.invitation.recipient.is_empty() {
        let owner_did = state.snapshot.as_ref().map(|s| s.local_did.as_str());
        if let Some(did) = peers
            .into_iter()
            .flat_map(|p| p.iter())
            .map(|(did, _)| did)
            .filter(|did| Some(did.as_str()) != owner_did)
            .min()
        {
            state.invitation.recipient = did.clone();
        }
    }
    let Some(target) = state.target.as_ref() else {
        return false;
    };
    state.invitation.synchronize(target, role, *expiry);
    let editing = !state.waiting() && !state.read_required && !state.invitation.busy();
    ui.add_enabled_ui(editing, |ui| {
        ui.label("Recipient DID");
        ui.add(
            crate::ui_shell::layout_policy::text_input(ui, &mut state.invitation.recipient)
                .char_limit(256)
                .desired_width(f32::INFINITY),
        )
        .named(ui, "Invitation recipient DID");
        egui::ComboBox::from_id_salt("entity_invite_role")
            .selected_text(role.as_str())
            .show_ui(ui, |ui| {
                for value in ["viewer", "editor", "manager"] {
                    ui.selectable_value(role, value.into(), value).observed(ui);
                }
            })
            .response
            .named(ui, "Scoped invite role");
        ui.add(egui::DragValue::new(expiry).range(1..=720).suffix(" hours"))
            .named(ui, "Scoped invite expiry hours");
    });
    state.invitation.synchronize(target, role, *expiry);
    let local_only = state.verse_local_only;
    if local_only {
        ui.colored_label(theme::TEXT_DIM, LOCAL_ONLY_REASON);
    }
    if ui
        .add_enabled(
            editing && !local_only && state.invitation.can_request(),
            egui::Button::new("Generate invite"),
        )
        .named(ui, "Generate invite for this entry")
        .on_disabled_hover_text(if local_only { LOCAL_ONLY_REASON } else { "Enter a recipient and wait for stored access to load." })
        .clicked()
    {
        state.invitation.request();
    }
    if let Some(progress) = state.invitation.progress() {
        ui.spinner();
        ui.label(progress);
    }
    if let Some(error) = state.invitation.error {
        ui.colored_label(theme::STATUS_OFFLINE, error);
    }
    ui.label("Expiry limits accepting this envelope. The native document capability is a transferable bearer credential without its own expiry. Scoped roles do not hide other bytes in the Verse; revocation does not erase cached capabilities.");
    let mut copied = false;
    if let Some(issued) = state.invitation.ready() {
        ui.label("Addressed invitation generated. Share it only with the named recipient.");
        if ui
            .button("Copy invite link")
            .named(ui, "Copy scoped invite link")
            .clicked()
        {
            ui.ctx().copy_text(issued.credential.as_str().into());
            copied = true;
        }
    }
    copied
}

pub(crate) fn invitation_context(
    dialog: &mut ActiveDialog,
) -> Option<(EntityAccessTarget, &mut crate::invitations::IssueState)> {
    let target = current_target(dialog);
    let ActiveDialog::EntitySettings {
        access,
        details,
        active_tab,
        invite_role_buf,
        invite_expiry_buf,
        ..
    } = dialog
    else {
        return None;
    };
    let valid = *active_tab == SettingsTab::Access
        && details.can_write()
        && details.loaded.as_ref().is_some_and(|d| matches!(d.role.as_str(),"owner"|"manager"))
        && !access.read_required
        && !access.waiting()
        && access
            .snapshot
            .as_ref()
            .is_some_and(|s| matches!(s.local_role.as_str(),"owner"|"manager"));
    let Some(target) = target.filter(|_| valid) else {
        access.invitation.reset();
        return None;
    };
    access
        .invitation
        .synchronize(&target, invite_role_buf, *invite_expiry_buf);
    Some((target, &mut access.invitation))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace(char::from(39), "'\\''"))
}

pub(crate) fn scoped_example(
    endpoint: std::net::SocketAddr,
    target: &EntityAccessTarget,
    token: &str,
) -> Option<String> {
    let mut url = url::Url::parse(&format!("http://{endpoint}")).ok()?;
    if target.entity_type == fe_runtime::messages::EntityType::Petal {
        url.path_segments_mut().ok()?.extend([
            "api",
            "v1",
            "petals",
            target.entity_id.as_str(),
            "nodes",
        ]);
        Some(format!(
            "curl -s {} -H {}",
            shell_quote(url.as_str()),
            shell_quote(&format!("Authorization: Bearer {token}"))
        ))
    } else {
        url.set_path("/api/v1/query");
        Some(format!("curl -s -X POST {} -H {} -H 'Content-Type: application/json' --data '{{\"sql\":\"SELECT node_id, petal_id FROM node LIMIT 10\",\"vars\":{{}}}}'",shell_quote(url.as_str()),shell_quote(&format!("Authorization: Bearer {token}"))))
    }
}

pub(crate) fn api_controls(
    ui: &mut egui::Ui,
    state: &mut EntityAccessState,
    role: &mut String,
    expiry: &mut u32,
    client: &mut String,
    endpoint: Option<std::net::SocketAddr>,
) -> (Option<EntityAccessAction>, bool) {
    let mut action = token_controls(ui, state);
    let mut copied = false;
    ui.separator();
    ui.label("Generate a token for this entry");
    if let Some(endpoint) = endpoint {
        ui.label(format!("API listening at http://{endpoint}"));
    } else {
        ui.label("API endpoint is not available yet; authenticated examples appear after the listener confirms its address.");
    }
    ui.add_enabled_ui(!state.waiting() && !state.read_required, |ui| {
        egui::ComboBox::from_id_salt("entity_token_role")
            .selected_text(role.as_str())
            .show_ui(ui, |ui| {
                for value in ["viewer", "editor", "manager"] {
                    ui.selectable_value(role, value.into(), value).observed(ui);
                }
            })
            .response
            .named(ui, "Entity API token maximum role");
        ui.add(egui::DragValue::new(expiry).range(1..=720).suffix(" hours"))
            .named(ui, "Entity API token expiry hours");
        ui.add(
            crate::ui_shell::layout_policy::text_input(ui, client)
                .hint_text("Client DID, or leave blank for a fresh client"),
        )
        .named(ui, "Entity API token client DID");
        let valid_client =
            client.trim().is_empty() || fe_identity::validate_client_did(client.trim()).is_ok();
        if !valid_client {
            ui.colored_label(
                theme::STATUS_OFFLINE,
                "Enter a valid Ed25519 client DID or leave it blank.",
            );
        }
        if ui
            .add_enabled(valid_client, egui::Button::new("Generate API token"))
            .named(ui, "Generate API token for this entry")
            .clicked()
        {
            action = Some(EntityAccessAction::MintToken {
                max_role: role.clone(),
                ttl_hours: *expiry,
                client_did: (!client.trim().is_empty()).then(|| client.trim().to_owned()),
            });
        }
    });
    if let Some(minted) = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.minted.as_ref())
    {
        ui.separator();
        ui.label("Copy this token before leaving API Access or refreshing.");
        ui.checkbox(&mut state.reveal_secret, "Reveal token")
            .named(ui, "Reveal generated entity API token");
        let mut display = minted.token.clone();
        ui.add(
            crate::ui_shell::layout_policy::text_input(ui, &mut display)
                .password(!state.reveal_secret)
                .interactive(false),
        )
        .named(ui, "Generated entity API token");
        ui.label(crate::panels::inspector::token_expiry_label(
            &minted.expires_at,
            chrono::Utc::now(),
        ))
        .on_hover_text(&minted.expires_at);
        if ui
            .button("Copy token")
            .named(ui, "Copy generated entity API token")
            .clicked()
        {
            ui.ctx().copy_text(minted.token.clone());
            copied = true;
        }
        if let Some(command) = endpoint
            .zip(state.target.as_ref())
            .and_then(|(endpoint, target)| scoped_example(endpoint, target, &minted.token))
        {
            let mut preview = if state.reveal_secret {
                command.clone()
            } else {
                command.replace(&minted.token, "<TOKEN>")
            };
            ui.add(
                egui::TextEdit::multiline(&mut preview)
                    .interactive(false)
                    .desired_rows(4),
            )
            .named(ui, "Entity scoped API read example");
            if ui
                .button("Copy authenticated read command")
                .named(ui, "Copy entity scoped API read command")
                .clicked()
            {
                ui.ctx().copy_text(command);
                copied = true;
            }
        }
    }
    (action, copied)
}

#[cfg(test)]
pub(crate) fn role_confirmation_refresh_journey(
    manager: &mut UiManager,
    sender: &crossbeam::channel::Sender<DbCommand>,
    receiver: &crossbeam::channel::Receiver<DbCommand>,
    target: &EntityAccessTarget,
    snapshot: &EntityAccessSnapshot,
) {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let mut frame = |manager: &mut UiManager, events: Vec<egui::Event>| {
        time += 0.05;
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog
                    {
                        action = role_controls(ui, access, None);
                    }
                });
            },
        );
        assert!(
            action.is_none(),
            "An arm from an older readback must never confirm a role mutation"
        );
        let mut texts = Vec::new();
        let mut shapes: Vec<_> = output.shapes.iter().map(|shape| &shape.shape).collect();
        while let Some(shape) = shapes.pop() {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.job.text.clone(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()).center(),
                )),
                egui::Shape::Vec(children) => shapes.extend(children.iter()),
                _ => {}
            }
        }
        texts
    };
    frame(manager, vec![]);
    let first = frame(manager, vec![]);
    assert!(
        first.iter().any(|(text, _)| text.starts_with("Inherited"))
            && first.iter().all(|(text, _)| !text.starts_with("Granted here")),
        "An effective role with no grant at this scope reads as inherited"
    );
    for (label, confirm) in [
        ("Save role...", "Confirm role change"),
        ("Block access here...", "Confirm revoke"),
    ] {
        frame(manager, vec![]);
        let labels = frame(manager, vec![]);
        let point = labels
            .iter()
            .find(|(text, _)| text == label)
            .expect("Unarmed role button is painted")
            .1;
        for pressed in [true, false] {
            frame(
                manager,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(
            frame(manager, vec![])
                .iter()
                .any(|(text, _)| text == confirm),
            "Actual role button is armed before readback changes"
        );
        if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog {
            access.refresh();
        }
        service(manager, sender, None);
        let DbCommand::EntityAccess {
            request_id,
            action: EntityAccessAction::Load { .. },
            ..
        } = receiver
            .try_recv()
            .expect("Refresh queues one authoritative read")
        else {
            panic!("Refresh cannot mutate")
        };
        apply(manager, target, &request_id, &Ok(snapshot.clone()));
        let labels = frame(manager, vec![]);
        assert!(
            labels.iter().all(|(text, _)| text != confirm),
            "A new readback disarms the prior role confirmation"
        );
        let point = labels
            .iter()
            .find(|(text, _)| text == label)
            .expect("New readback restores the unarmed role button")
            .1;
        for pressed in [true, false] {
            frame(
                manager,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(
            receiver.try_recv().is_err(),
            "Rearming after refresh emits no mutation"
        );
    }
    if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog {
        access.refresh();
    }
    service(manager, sender, None);
    let DbCommand::EntityAccess {
        request_id,
        action: EntityAccessAction::Load { .. },
        ..
    } = receiver.try_recv().expect("Refresh queues a read")
    else {
        panic!("Refresh cannot mutate")
    };
    let mut granted = snapshot.clone();
    let (peer, role) = granted
        .roles
        .iter_mut()
        .find(|(_, role)| role != "owner")
        .expect("Fixture lists a non-owner peer");
    *role = "editor".into();
    let peer = peer.clone();
    granted.grants = vec![
        EntityGrantRecord {
            peer_did: peer.clone(),
            grantor_did: granted.local_did.clone(),
            role: "editor".into(),
            revoked: false,
            hlc: 2,
        },
        EntityGrantRecord {
            peer_did: peer,
            grantor_did: "did:key:former-manager".into(),
            role: "manager".into(),
            revoked: true,
            hlc: 1,
        },
    ];
    apply(manager, target, &request_id, &Ok(granted));
    let labels = frame(manager, vec![]);
    assert!(
        labels.iter().any(|(text, _)| text == "Granted here: editor")
            && labels.iter().all(|(text, _)| !text.starts_with("Inherited")),
        "A live grant at this scope wins over a revoked one and is not labelled inherited"
    );
    assert!(
        labels.iter().any(|(text, _)| text == "Grant record (2)"),
        "The grant record lists every current statement at this scope"
    );

    // Managers-only preset: two viewer grants + one manager -> two sequential Revokes, manager untouched.
    let preset_ctx = egui::Context::default();
    let mut preset_time = 0.0;
    let mut preset_frame = |manager: &mut UiManager, events: Vec<egui::Event>| {
        preset_time += 0.05;
        let mut chosen = None;
        let output = preset_ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                time: Some(preset_time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog {
                        chosen = chosen.or(preset_controls(ui, access, None, false, false));
                    }
                });
            },
        );
        let mut texts = Vec::new();
        let mut shapes: Vec<_> = output.shapes.iter().map(|shape| &shape.shape).collect();
        while let Some(shape) = shapes.pop() {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.job.text.clone(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()).center(),
                )),
                egui::Shape::Vec(children) => shapes.extend(children.iter()),
                _ => {}
            }
        }
        (texts, chosen)
    };
    let grant = |peer: &str, role: &str, hlc: u64, local_did: &str| EntityGrantRecord {
        peer_did: peer.into(),
        grantor_did: local_did.into(),
        role: role.into(),
        revoked: false,
        hlc,
    };
    let mut bulk = snapshot.clone();
    bulk.roles = vec![
        (bulk.local_did.clone(), "owner".into()),
        ("did:key:viewer-a".into(), "viewer".into()),
        ("did:key:viewer-b".into(), "viewer".into()),
        ("did:key:manager-m".into(), "manager".into()),
    ];
    bulk.grants = vec![
        grant("did:key:viewer-a", "viewer", 3, &bulk.local_did),
        grant("did:key:viewer-b", "viewer", 4, &bulk.local_did),
        grant("did:key:manager-m", "manager", 5, &bulk.local_did),
    ];
    for refused in [false, true] {
        if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog {
            access.refresh();
        }
        service(manager, sender, None);
        let DbCommand::EntityAccess { request_id, action: EntityAccessAction::Load { .. }, .. } =
            receiver.try_recv().expect("Refresh queues a read")
        else {
            panic!("Refresh cannot mutate")
        };
        apply(manager, target, &request_id, &Ok(bulk.clone()));
        preset_frame(manager, vec![]);
        let (labels, _) = preset_frame(manager, vec![]);
        let point = labels.iter().find(|(text, _)| text == "Managers only...").expect("Managers only preset is painted").1;
        let mut chosen = None;
        for pressed in [true, false] {
            let (_, fired) = preset_frame(manager, vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
            ]);
            chosen = chosen.or(fired);
        }
        assert!(chosen.is_none(), "The first click only arms the Managers only preset");
        let (labels, _) = preset_frame(manager, vec![]);
        let confirm = labels.iter().find(|(text, _)| text == "Confirm: revoke 2 grants").expect("Confirmation counts the two below-manager grants").1;
        for pressed in [true, false] {
            let (_, fired) = preset_frame(manager, vec![
                egui::Event::PointerMoved(confirm),
                egui::Event::PointerButton { pos: confirm, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
            ]);
            chosen = chosen.or(fired);
        }
        assert_eq!(chosen, Some(AccessPreset::ManagersOnly), "The confirmed click selects the preset");
        if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog {
            access.start_managers_only();
        }
        service(manager, sender, None);
        let DbCommand::EntityAccess { request_id: first_id, action: EntityAccessAction::Revoke { peer_did }, .. } =
            receiver.try_recv().expect("The first revoke is sent")
        else {
            panic!("Managers only emits revokes only")
        };
        assert_eq!(peer_did, "did:key:viewer-a");
        service(manager, sender, None);
        assert!(receiver.try_recv().is_err(), "The second revoke waits for the first confirmed reply");
        if refused {
            apply(manager, target, &first_id, &Err("refused".into()));
            service(manager, sender, None);
            assert!(receiver.try_recv().is_err(), "A refused revoke halts the queue");
            continue;
        }
        let mut after_first = bulk.clone();
        after_first.grants.remove(0);
        after_first.roles[1].1 = "none".into();
        apply(manager, target, &first_id, &Ok(after_first.clone()));
        service(manager, sender, None);
        let DbCommand::EntityAccess { request_id: second_id, action: EntityAccessAction::Revoke { peer_did }, .. } =
            receiver.try_recv().expect("The second revoke follows the first confirmed reply")
        else {
            panic!("Managers only emits revokes only")
        };
        assert_eq!(peer_did, "did:key:viewer-b");
        let mut after_second = after_first;
        after_second.grants.remove(0);
        apply(manager, target, &second_id, &Ok(after_second));
        service(manager, sender, None);
        let Ok(DbCommand::EntityAccess { request_id: readback_id, action: EntityAccessAction::Load { .. }, .. }) = receiver.try_recv() else {
            panic!("The queue finishes with the normal read-back and never revokes the manager")
        };
        assert!(receiver.try_recv().is_err());
        apply(manager, target, &readback_id, &Ok(bulk.clone()));
    }

    // A non-owner Manager's revoke withdraws only its own statement: another grantor's viewer grant is not queued.
    let mut managed = bulk.clone();
    managed.local_role = "manager".into();
    managed.grants[0].grantor_did = "did:key:other-grantor".into();
    if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog {
        access.refresh();
    }
    service(manager, sender, None);
    let DbCommand::EntityAccess { request_id, action: EntityAccessAction::Load { .. }, .. } =
        receiver.try_recv().expect("Refresh queues a read")
    else {
        panic!("Refresh cannot mutate")
    };
    apply(manager, target, &request_id, &Ok(managed.clone()));
    if let ActiveDialog::EntitySettings { access, .. } = &mut manager.active_dialog {
        access.start_managers_only();
    }
    service(manager, sender, None);
    let DbCommand::EntityAccess { request_id: only_id, action: EntityAccessAction::Revoke { peer_did }, .. } =
        receiver.try_recv().expect("The Manager's own below-manager grant is revoked")
    else {
        panic!("Managers only emits revokes only")
    };
    assert_eq!(peer_did, "did:key:viewer-b", "the other grantor's grant to viewer-a is not this Manager's to withdraw");
    apply(manager, target, &only_id, &Ok(managed));
    service(manager, sender, None);
    let Ok(DbCommand::EntityAccess { request_id: final_read, action: EntityAccessAction::Load { .. }, .. }) = receiver.try_recv() else {
        panic!("One revoke, then the normal read-back")
    };
    assert!(receiver.try_recv().is_err());
    apply(manager, target, &final_read, &Ok(bulk));
}
