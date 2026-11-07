//! Target-bound access state and confirmed controls; see dialogs/AGENTS.md.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use bevy_egui::egui;
use fe_runtime::{
    entity_access::{EntityAccessAction, EntityAccessSnapshot, EntityAccessTarget},
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
}

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
        }
    }
}

impl EntityAccessState {
    pub(crate) fn refresh(&mut self) {
        self.invitation.reset();
        self.queued_read = true;
        self.error = None;
    }
    pub(crate) fn pending_work(&self) -> bool {
        self.waiting() || self.queued_read || self.invitation.busy()
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
            *self = Self::default();
            self.target = Some(target.clone());
            self.queued_read = queued;
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
            self.error=Some("Access request timed out; its outcome is unconfirmed. Refresh before making another change.");
        }
    }
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
        }
        Ok(snapshot) if fe_database::RoleLevel::from(snapshot.local_role.as_str()).can_manage() => {
            access.snapshot = Some(snapshot.clone());
            access.read_required = false;
            access.error = None;
            access.role_drafts.clear();
            access.confirmed_request_id = id.to_owned();
            access.reveal_secret = false;
        }
        _ => {
            access.snapshot = None;
            access.read_required = true;
            access.error=Some("The access operation was not confirmed. Refresh stored access before making another change.");
        }
    }
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
    ui.label("Effective roles include inherited access and grants from other managers.");
    for (did, role) in &snapshot.roles {
        ui.push_id(did,|ui|{
            ui.separator();
            let peer=peers.and_then(|registry|registry.get(did));
            let name=peer.and_then(|p|p.display_name.as_deref()).unwrap_or(did);
            ui.add(egui::Label::new(name).truncate()).on_hover_text(did);
            ui.label(format!("Effective role: {role}"));
            if role=="owner" {ui.label("Ownership is managed separately.");return;}
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
    action
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
            ui.label(format!("Expires: {}", token.expires_at));
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
    if ui
        .add_enabled(
            editing && state.invitation.can_request(),
            egui::Button::new("Generate invite"),
        )
        .named(ui, "Generate invite for this entry")
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
        ui.label(format!("Expires: {}", minted.expires_at));
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
}
