//! Current-issuer invitation state machine; see AGENTS.md.

use crossbeam::channel::{Receiver, Sender};
use fe_database::join_invite::{JoinManifest, JoinRole};
use fe_runtime::{
    entity_access::EntityAccessTarget,
    join_invite::{InviteAuthority, InviteSource, IssuedInvitation, JoinCredential},
    messages::{DbCommand, DbResult},
};
use fe_sync::{
    join_ticket::{ExportJoinTicket, JoinTicketExported},
    SyncCommand, SyncCommandSenderRes,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    target: EntityAccessTarget,
    recipient: String,
    role: String,
    expiry: u32,
}

#[derive(Debug, Clone)]
enum Phase {
    Idle,
    Queued,
    Preparing,
    Exporting {
        source: InviteSource,
        reply: Receiver<JoinTicketExported>,
    },
    Issuing {
        source: InviteSource,
    },
    Ready(IssuedInvitation),
    Failed,
}

#[derive(Debug, Clone)]
pub(crate) struct IssueState {
    pub recipient: String,
    selection: Option<Selection>,
    generation: String,
    phase: Phase,
    deadline: Option<Instant>,
    consume_deadline: Option<Instant>,
    consume_expires_at: Option<u64>,
    navigation_revision: Option<u64>,
    pub error: Option<&'static str>,
}

impl Default for IssueState {
    fn default() -> Self {
        Self {
            recipient: String::new(),
            selection: None,
            generation: String::new(),
            phase: Phase::Idle,
            deadline: None,
            consume_deadline: None,
            consume_expires_at: None,
            navigation_revision: None,
            error: None,
        }
    }
}

impl IssueState {
    #[cfg(test)]
    pub(crate) fn transfer_fixture(
        source: InviteSource,
        credential: JoinCredential,
        expired: bool,
    ) -> Self {
        let clock = unix_now().unwrap();
        Self {
            recipient: source.recipient_did.clone(),
            selection: Some(Selection {
                target: source.target.clone(),
                recipient: source.recipient_did.clone(),
                role: source.role.clone(),
                expiry: source.expiry_hours,
            }),
            generation: format!("invite-{}", ulid::Ulid::new()),
            phase: Phase::Ready(IssuedInvitation {
                source,
                credential,
                expires_at: clock + 3600,
            }),
            consume_deadline: Some(if expired {
                Instant::now() - Duration::from_secs(1)
            } else {
                Instant::now() + fe_runtime::qa_invitation::CONSUME_TTL
            }),
            consume_expires_at: Some(clock + 15),
            ..Self::default()
        }
    }
    pub(crate) fn bind_navigation(&mut self, revision: u64) {
        if self
            .navigation_revision
            .is_some_and(|current| current != revision)
        {
            self.reset();
        }
        if self.busy() || matches!(self.phase, Phase::Ready(_)) {
            self.navigation_revision = Some(revision);
        }
    }
    pub fn reset(&mut self) {
        let recipient = std::mem::take(&mut self.recipient);
        *self = Self {
            recipient,
            ..Self::default()
        };
    }
    pub fn synchronize(&mut self, target: &EntityAccessTarget, role: &str, expiry: u32) {
        let same = self.selection.as_ref().is_some_and(|s| {
            s.target == *target
                && s.recipient == self.recipient
                && s.role == role
                && s.expiry == expiry
        });
        if !same {
            self.reset();
            self.selection = Some(Selection {
                target: target.clone(),
                recipient: self.recipient.clone(),
                role: role.into(),
                expiry,
            });
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) && self.busy() {
            self.fail("Invitation issuance timed out and is unconfirmed; a grant may remain stored. Refresh Access before another change.");
        }
        if matches!(&self.phase, Phase::Ready(issued) if unix_now().is_none_or(|now| now >= issued.expires_at))
        {
            self.reset();
        }
    }
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Queued | Phase::Preparing | Phase::Exporting { .. } | Phase::Issuing { .. }
        )
    }
    pub fn can_request(&self) -> bool {
        !self.busy()
            && !matches!(self.phase, Phase::Failed)
            && fe_identity::did_key::public_key_from_did_key(&self.recipient).is_ok()
    }
    pub fn request(&mut self) {
        if self.can_request() {
            self.generation = format!("invite-{}", ulid::Ulid::new());
            self.phase = Phase::Queued;
            self.deadline = Some(Instant::now() + Duration::from_secs(30));
            self.error = None;
        }
    }
    fn fail(&mut self, error: &'static str) {
        self.consume_deadline = None;
        self.consume_expires_at = None;
        self.phase = Phase::Failed;
        self.deadline = None;
        self.error = Some(error);
    }
    pub fn ready(&self) -> Option<&IssuedInvitation> {
        match &self.phase {
            Phase::Ready(issued) if unix_now().is_some_and(|now| now < issued.expires_at) => {
                Some(issued)
            }
            _ => None,
        }
    }
    pub(crate) fn transfer_metadata(
        &self,
        issuer: &str,
    ) -> Option<fe_runtime::qa_invitation::TransferMetadata> {
        if self
            .consume_deadline
            .is_none_or(|deadline| Instant::now() >= deadline)
        {
            return None;
        }
        let issued = self.ready()?;
        if !issued.source.authority.valid_for(issuer) {
            return None;
        }
        let owner = issued.source.manifest.get("created_by")?.as_str()?;
        if match &issued.source.authority {
            InviteAuthority::Owner { .. } => owner != issuer,
            InviteAuthority::DelegatedManager { .. } => owner == issuer,
        } {
            return None;
        }
        let entity_kind = match issued.source.target.entity_type {
            fe_runtime::messages::EntityType::Verse => "verse",
            fe_runtime::messages::EntityType::Fractal => "fractal",
            fe_runtime::messages::EntityType::Petal => "petal",
            _ => return None,
        };
        Some(fe_runtime::qa_invitation::TransferMetadata {
            authority: issued.source.authority.clone(),
            generation: self.generation.clone(),
            issuer_did: issuer.into(),
            recipient_did: issued.source.recipient_did.clone(),
            verse_id: issued.source.manifest.get("verse_id")?.as_str()?.into(),
            entity_kind: entity_kind.into(),
            entity_id: issued.source.target.entity_id.clone(),
            scope: issued.source.target.scope.clone(),
            role: issued.source.role.clone(),
            expiry_hours: issued.source.expiry_hours,
            envelope_expires_at: issued.expires_at,
            consume_expires_at: self.consume_expires_at?,
        })
    }
    pub(crate) fn transfer_source(
        &self,
        expected: &fe_runtime::qa_invitation::TransferMetadata,
        issuer: &str,
    ) -> Option<InviteSource> {
        (self.transfer_metadata(issuer).as_ref() == Some(expected))
            .then(|| self.ready().map(|issued| issued.source.clone()))
            .flatten()
    }
    pub(crate) fn consume_transfer(
        &mut self,
        expected: &fe_runtime::qa_invitation::TransferMetadata,
        fresh: &InviteSource,
        issuer: &str,
    ) -> Option<JoinCredential> {
        let source = self.transfer_source(expected, issuer)?;
        if source.target != fresh.target
            || source.recipient_did != fresh.recipient_did
            || source.role != fresh.role
            || source.expiry_hours != fresh.expiry_hours
            || source.manifest != fresh.manifest
            || source.authority != fresh.authority
        {
            return None;
        }
        let Phase::Ready(issued) = std::mem::replace(&mut self.phase, Phase::Idle) else {
            return None;
        };
        self.consume_deadline = None;
        self.consume_expires_at = None;
        self.generation.clear();
        Some(issued.credential)
    }
    pub fn progress(&self) -> Option<&'static str> {
        match self.phase {
            Phase::Queued | Phase::Preparing => {
                Some("Checking current issuer authority and scope...")
            }
            Phase::Exporting { .. } => Some("Preparing the live replica capability..."),
            Phase::Issuing { .. } => {
                Some("Storing the checked grant and signing the invitation...")
            }
            _ => None,
        }
    }
    fn source_matches(&self, source: &InviteSource, target: &EntityAccessTarget) -> bool {
        self.selection.as_ref().is_some_and(|s| {
            s.target == *target
                && source.target == *target
                && source.recipient_did == s.recipient
                && source.role == s.role
                && source.expiry_hours == s.expiry
        })
    }
    pub fn apply(
        &mut self,
        result: &DbResult,
        target: &EntityAccessTarget,
        did: Option<&str>,
        sync: Option<&SyncCommandSenderRes>,
    ) {
        match result {
            DbResult::JoinInvitePrepared { request_id, result }
                if request_id.strip_suffix("-prepare") == Some(self.generation.as_str())
                    && matches!(self.phase, Phase::Preparing) =>
            {
                let Ok(source) = result else {
                    let needs_reauthorization = result
                        .as_ref()
                        .err()
                        .is_some_and(|message| message.contains("reauthorization"));
                    self.fail(if needs_reauthorization {
                        "Manager authority needs Owner reauthorization or a shorter invitation lifetime. Refresh stored Access before another change."
                    } else {"Invitation preparation was refused. Refresh stored Access and check the recipient."});
                    return;
                };
                if !self.source_matches(source, target) {
                    self.fail("Invitation source changed. Refresh Access.");
                    return;
                }
                let Some((sync, did, deadline)) = sync
                    .zip(did)
                    .zip(self.deadline)
                    .map(|((sync, did), deadline)| (sync, did, deadline))
                else {
                    self.fail(
                        "Live P2P identity is unavailable. Refresh Access before trying again.",
                    );
                    return;
                };
                let Ok(manifest) = serde_json::from_value::<JoinManifest>(source.manifest.clone())
                else {
                    self.fail("Invitation source metadata is unavailable. Refresh Access.");
                    return;
                };
                let Ok(role) = JoinRole::parse(&source.role) else {
                    self.fail("Unsupported invitation role.");
                    return;
                };
                let current_authority = source.authority.valid_for(did)
                    && match &source.authority {
                        InviteAuthority::Owner { .. } => manifest.created_by == did,
                        InviteAuthority::DelegatedManager { .. } => manifest.created_by != did,
                    };
                if !current_authority {
                    self.fail("The prepared authority does not match this local issuer. Refresh stored Access.");
                    return;
                }
                let (reply, receiver) = crossbeam::channel::bounded(1);
                if sync
                    .0
                    .try_send(SyncCommand::ExportJoinTicket {
                        request: ExportJoinTicket {
                            request_id: self.generation.clone(),
                            verse_id: manifest.verse_id,
                            namespace_id: manifest.namespace_id,
                            issuer_did: did.into(),
                            role,
                            deadline,
                        },
                        reply,
                    })
                    .is_err()
                {
                    bevy::log::warn!("Invitation sync channel closed");
                    self.fail("Live P2P is unavailable. Refresh Access before trying again.");
                    return;
                }
                self.phase = Phase::Exporting {
                    source: source.clone(),
                    reply: receiver,
                };
            }
            DbResult::JoinInviteIssued { request_id, result }
                if request_id.strip_suffix("-issue") == Some(self.generation.as_str()) =>
            {
                let Phase::Issuing { source } = &self.phase else {
                    return;
                };
                let Ok(issued) = result else {
                    self.fail("Invitation issuance is unconfirmed; a checked grant may already be stored. Refresh Access before another change. Do not automatically replay.");
                    return;
                };
                if !self.source_matches(&issued.source, target)
                    || issued.source.nonce != source.nonce
                    || issued.source.prepared_at != source.prepared_at
                    || issued.source.manifest != source.manifest
                    || issued.source.authority != source.authority
                {
                    self.fail("Invitation issuance is unconfirmed because its source changed. Refresh Access before another change.");
                    return;
                }
                let Some(clock) = unix_now() else {
                    self.fail(
                        "Invitation issuance is unconfirmed. Refresh Access before another change.",
                    );
                    return;
                };
                let valid = fe_database::join_invite::JoinInvitation::parse(
                    issued.credential.as_str(),
                    &issued.source.recipient_did,
                    clock,
                )
                .is_ok_and(|invitation| {
                    let authority_matches = match (
                        &issued.source.authority,
                        invitation.issuer_delegation(clock),
                    ) {
                        (InviteAuthority::Owner { .. }, Ok(None)) => {
                            invitation.manifest().created_by == did.unwrap_or_default()
                        }
                        (InviteAuthority::DelegatedManager { proof_id, .. }, Ok(Some(proof))) => {
                            proof.proof_id().ok().as_deref() == Some(proof_id.as_str())
                        }
                        _ => false,
                    };
                    invitation.issuer_did() == did.unwrap_or_default()
                        && issued.source.authority.valid_for(invitation.issuer_did())
                        && authority_matches
                        && invitation.scope() == issued.source.target.scope
                        && invitation.role().as_str() == issued.source.role
                        && serde_json::to_value(invitation.manifest()).ok().as_ref()
                            == Some(&issued.source.manifest)
                        && invitation.expires_at() == issued.expires_at
                });
                if !valid {
                    self.fail(
                        "Invitation issuance is unconfirmed. Refresh Access before another change.",
                    );
                    return;
                }
                self.consume_deadline =
                    Some(Instant::now() + fe_runtime::qa_invitation::CONSUME_TTL);
                self.consume_expires_at =
                    Some(issued.expires_at.min(
                        clock.saturating_add(fe_runtime::qa_invitation::CONSUME_TTL.as_secs()),
                    ));
                self.phase = Phase::Ready(issued.clone());
                self.deadline = None;
            }
            _ => {}
        }
    }
    pub fn drive(
        &mut self,
        target: &EntityAccessTarget,
        did: Option<&str>,
        db: &Sender<DbCommand>,
        sync: Option<&SyncCommandSenderRes>,
    ) {
        if did.is_none() || sync.is_none() {
            if self.busy() {
                self.fail("Live P2P identity is unavailable; the invitation outcome is unconfirmed. Refresh Access.");
            }
            return;
        }
        if matches!(self.phase, Phase::Queued) {
            let Some(selection) = &self.selection else {
                self.fail("Select an invitation recipient.");
                return;
            };
            if selection.target != *target || selection.recipient == did.unwrap_or_default() {
                self.fail("Choose a different recipient for this invitation.");
                return;
            }
            if db
                .try_send(DbCommand::PrepareJoinInvite {
                    target: target.clone(),
                    recipient_did: selection.recipient.clone(),
                    role: selection.role.clone(),
                    expiry_hours: selection.expiry,
                    request_id: format!("{}-prepare", self.generation),
                })
                .is_err()
            {
                bevy::log::warn!("Invitation database channel closed");
                self.fail("The database is unavailable. Nothing was sent.");
                return;
            }
            self.phase = Phase::Preparing;
        }
        if let Phase::Exporting { source, reply } = &self.phase {
            match reply.try_recv() {
                Ok(exported) if exported.request_id == self.generation => {
                    let Ok(ticket) = exported.ticket else {
                        self.fail(
                            "A live source capability could not be confirmed. Refresh Access.",
                        );
                        return;
                    };
                    let Ok(ticket) = JoinCredential::new(ticket.encoded().into()) else {
                        self.fail("The source capability exceeds its size limit.");
                        return;
                    };
                    if !self.source_matches(source, target) {
                        self.fail("Invitation source changed. Refresh Access.");
                        return;
                    }
                    let source = source.clone();
                    if db
                        .try_send(DbCommand::IssueJoinInvite {
                            source: source.clone(),
                            ticket,
                            request_id: format!("{}-issue", self.generation),
                        })
                        .is_err()
                    {
                        bevy::log::warn!("Invitation database channel closed");
                        self.fail("Invitation issuance is unconfirmed. Refresh Access before another change.");
                        return;
                    }
                    self.phase = Phase::Issuing { source };
                }
                Ok(_) | Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    self.fail("Invitation capability export is unconfirmed. Refresh Access.")
                }
                Err(crossbeam::channel::TryRecvError::Empty) => {}
            }
        }
    }
}

fn unix_now() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs())
}
