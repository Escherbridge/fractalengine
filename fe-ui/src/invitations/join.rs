//! Addressed join state machine; see AGENTS.md.

use crate::{
    dialogs::ActiveDialog, navigation_manager::NavigationManager, verse_manager::VerseManager,
};
use crossbeam::channel::{Receiver, Sender, TryRecvError};
use fe_database::join_invite::{JoinInvitation, VerifiedJoinInvitation};
use fe_runtime::{
    join_invite::{JoinAdmission, JoinCredential},
    messages::{DbCommand, DbResult},
};
use fe_sync::{
    join_ticket::{
        JoinReplicaOpened, JoinReplicaReadiness, JoinReplicaRequest, JoinedReplicaLease,
    },
    SyncCommand, SyncCommandSenderRes,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    verse: Option<String>,
    fractal: Option<String>,
    petal: Option<String>,
}
impl Cursor {
    fn capture(nav: &NavigationManager) -> Self {
        Self {
            verse: nav.active_verse_id.clone(),
            fractal: nav.active_fractal_id.clone(),
            petal: nav.active_petal_id.clone(),
        }
    }
    fn matches(&self, nav: &NavigationManager) -> bool {
        self.verse == nav.active_verse_id
            && self.fractal == nav.active_fractal_id
            && self.petal == nav.active_petal_id
    }
}

#[derive(Debug)]
enum Stage {
    Admitting,
    Admitted,
    Opening(Receiver<JoinReplicaOpened>),
    Waiting(Instant),
    Transport {
        reply: Receiver<JoinReplicaReadiness>,
        after_role: bool,
    },
    Role {
        request_id: String,
    },
    RoleReady,
}

#[derive(Debug)]
struct Attempt {
    generation: String,
    revision: u64,
    credential: JoinCredential,
    invitation: VerifiedJoinInvitation,
    deadline: Instant,
    cursor: Cursor,
    stage: Stage,
    replica_requested: bool,
    hierarchy_requested: bool,
    check_round: u32,
}

#[derive(Debug)]
struct Input {
    credential: JoinCredential,
    revision: u64,
    deadline: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct JoinState {
    revision: u64,
    input: Option<Input>,
    attempt: Option<Attempt>,
    submitted_nonces: std::collections::HashSet<String>,
    cleanup_lease: Option<JoinedReplicaLease>,
    admission_navigation_revision: Option<u64>,
    pub error: Option<&'static str>,
    status: Option<&'static str>,
}

pub(crate) struct ReadyJoin {
    pub scope: fe_database::ScopeParts,
    pub verse_name: String,
    pub fractal_name: Option<String>,
    pub lease: JoinedReplicaLease,
}

impl JoinState {
    pub(crate) fn retain_admission_navigation(&mut self, nav: &NavigationManager) {
        self.admission_navigation_revision = Some(nav.intent_revision());
    }
    pub(crate) fn allow_initial_navigation(&self, nav: &NavigationManager) -> bool {
        !self.active() && self.admission_navigation_revision != Some(nav.intent_revision())
    }
    pub fn active(&self) -> bool {
        self.input.is_some() || self.attempt.is_some()
    }
    pub(crate) fn wait_for_handoff(&mut self) {
        if self.input.is_some() {
            self.status = Some("Waiting for the previous native replica handoff or cleanup...");
        }
    }
    pub fn progress(&self) -> Option<&'static str> {
        self.status
    }
    pub fn queue(&mut self, credential: JoinCredential) -> bool {
        if self.active() || self.cleanup_lease.is_some() {
            return false;
        }
        self.input = Some(Input {
            credential,
            revision: self.revision,
            deadline: Instant::now() + Duration::from_secs(30),
        });
        self.status = Some("Validating the addressed invitation...");
        self.error = None;
        true
    }
    pub fn input_changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.cancel();
        self.error = None;
    }
    pub fn cancel(&mut self) {
        self.input = None;
        if let Some(attempt) = self.attempt.take() {
            if attempt.replica_requested {
                self.retain_cleanup(JoinedReplicaLease {
                    request_id: attempt.generation,
                    verse_id: attempt.invitation.manifest().verse_id.clone(),
                    namespace_id: attempt.invitation.manifest().namespace_id.clone(),
                });
            }
        }
        self.status = None;
    }
    fn fail(&mut self, error: &'static str) {
        self.cancel();
        self.error = Some(error);
    }
    pub fn synchronize(
        &mut self,
        dialog: &ActiveDialog,
        nav: &NavigationManager,
        did: Option<&str>,
    ) {
        let ActiveDialog::JoinDialog { invite_buf } = dialog else {
            self.cancel();
            return;
        };
        if let Some(input) = &self.input {
            if Instant::now() >= input.deadline {
                self.fail("Join submission expired while waiting for the previous native cleanup. Nothing was resubmitted.");
                return;
            }
            if input.revision != self.revision || input.credential.as_str() != invite_buf.trim() {
                self.fail("Join input changed; the previous request was canceled.");
                return;
            }
        }
        if let Some(attempt) = &self.attempt {
            if attempt.revision != self.revision
                || attempt.credential.as_str() != invite_buf.trim()
                || !attempt.cursor.matches(nav)
            {
                self.fail("Join context changed; its admission may remain stored. No late reply will navigate.");
                return;
            }
            if Instant::now() >= attempt.deadline {
                self.fail("Join timed out and is unconfirmed; accepted metadata may remain stored. Refresh stored access or obtain a new invitation instead of replaying this request.");
                return;
            }
            if did.is_none_or(|did| {
                unix_now().is_none_or(|now| attempt.invitation.revalidate(did, now).is_err())
            }) {
                self.fail("The invitation expired or the recipient identity changed. Acceptance may remain stored; obtain a new invitation.");
            }
        }
    }
    pub(crate) fn retain_cleanup(&mut self, lease: JoinedReplicaLease) {
        debug_assert!(self.cleanup_lease.is_none());
        self.cleanup_lease = Some(lease);
    }
    pub fn cleanup(&mut self, nav: &NavigationManager, sync: Option<&SyncCommandSenderRes>) {
        let Some(lease) = &self.cleanup_lease else {
            return;
        };
        // The active navigation cursor owns its replica independently of this dialog.
        if nav.active_verse_id.as_deref() == Some(lease.verse_id.as_str()) {
            self.cleanup_lease = None;
            return;
        }
        let Some(sync) = sync else {
            return;
        };
        match sync.0.try_send(SyncCommand::CloseJoinedReplica {
            lease: lease.clone(),
        }) {
            Ok(()) => self.cleanup_lease = None,
            Err(crossbeam::channel::TrySendError::Full(_)) => {
                self.status = Some("Waiting to queue cleanup of the canceled join...");
            }
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                if self.error != Some("Canceled join cleanup is unconfirmed because P2P stopped. No new join was queued.") {
                    bevy::log::warn!("Join cleanup sync channel closed");
                }
                self.error = Some("Canceled join cleanup is unconfirmed because P2P stopped. No new join was queued.");
            }
        }
    }
    pub fn apply(&mut self, result: &DbResult) {
        let Some(attempt) = &mut self.attempt else {
            return;
        };
        match result {
            DbResult::JoinInviteAdmitted { request_id, result }
                if request_id == &attempt.generation
                    && matches!(attempt.stage, Stage::Admitting) =>
            {
                let Ok(admission) = result else {
                    self.fail("Invitation admission was not confirmed. Stored metadata may remain; inspect stored Access before another attempt.");
                    return;
                };
                if !admission_matches(admission, &attempt.invitation) {
                    self.fail("Invitation admission did not match this request. No navigation was applied.");
                    return;
                }
                attempt.stage = Stage::Admitted;
                self.status = Some("Invitation accepted locally; connecting to the issuer...");
            }
            DbResult::JoinReadinessChecked { request_id, result } => {
                let Stage::Role {
                    request_id: expected,
                } = &attempt.stage
                else {
                    return;
                };
                if request_id != expected {
                    return;
                }
                match result {
                    Ok(true) => attempt.stage = Stage::RoleReady,
                    Ok(false) => {
                        attempt.stage = Stage::Waiting(Instant::now()+Duration::from_millis(250));
                        self.status = Some("Connected; waiting for the invited scope and role to converge...");
                    }
                    Err(_) => self.fail("Current scope access is no longer confirmed. Acceptance may remain stored; inspect stored Access before another attempt."),
                }
            }
            _ => {}
        }
    }
    pub fn drive(
        &mut self,
        nav: &NavigationManager,
        hierarchy: &VerseManager,
        did: Option<&str>,
        db: &Sender<DbCommand>,
        sync: Option<&SyncCommandSenderRes>,
    ) -> Option<ReadyJoin> {
        self.cleanup(nav, sync);
        if let Some(input) = self.input.take() {
            let Some((did, clock)) = did.zip(unix_now()) else {
                self.fail("The local identity or clock is unavailable. Nothing was submitted.");
                return None;
            };
            if sync.is_none() {
                self.fail("Live P2P is unavailable. Nothing was submitted.");
                return None;
            }
            let Ok(invitation) = JoinInvitation::parse_owner(input.credential.as_str(), did, clock)
            else {
                self.fail("This addressed invitation is invalid, expired, or intended for another identity. Older links cannot bootstrap a fresh join; ask the Owner for a new addressed invitation.");
                return None;
            };
            if self.submitted_nonces.contains(invitation.nonce()) {
                self.fail("This invitation was already submitted in this session. Inspect stored Access or obtain a new invitation instead of replaying an unconfirmed request.");
                return None;
            }
            if self.submitted_nonces.len() >= 64 {
                self.fail("This session has reached its bounded join-attempt limit. Inspect stored Access before starting another session.");
                return None;
            }
            let generation = format!("join-{}", ulid::Ulid::new());
            let request = DbCommand::AdmitJoinInvite {
                credential: input.credential.clone(),
                request_id: generation.clone(),
            };
            if db.try_send(request).is_err() {
                bevy::log::warn!("Join admission database channel closed");
                self.fail("The database is unavailable. Nothing was submitted.");
                return None;
            }
            self.retain_admission_navigation(nav);
            self.submitted_nonces.insert(invitation.nonce().into());
            self.attempt = Some(Attempt {
                generation,
                revision: input.revision,
                credential: input.credential,
                invitation,
                deadline: input.deadline,
                cursor: Cursor::capture(nav),
                stage: Stage::Admitting,
                replica_requested: false,
                hierarchy_requested: false,
                check_round: 0,
            });
            self.status =
                Some("Valid invitation; admitting pinned Verse identity and membership...");
        }
        let Some(attempt) = &mut self.attempt else {
            return None;
        };
        let Some(sync) = sync else {
            self.fail(
                "Live P2P became unavailable. Join is unconfirmed; acceptance may remain stored.",
            );
            return None;
        };
        match &attempt.stage {
            Stage::Admitted => {
                let (reply, receiver) = crossbeam::channel::bounded(1);
                if sync
                    .0
                    .try_send(SyncCommand::JoinVerseReplica {
                        request: native_request(attempt),
                        reply,
                    })
                    .is_err()
                {
                    bevy::log::warn!("Join native-open sync channel closed");
                    self.fail(
                        "Replica connection could not be submitted. Acceptance may remain stored.",
                    );
                    return None;
                }
                attempt.replica_requested = true;
                attempt.stage = Stage::Opening(receiver);
            }
            Stage::Opening(reply) => match reply.try_recv() {
                Ok(opened) if opened.request_id == attempt.generation && opened.outcome.is_ok() => {
                    attempt.stage = Stage::Waiting(Instant::now());
                    self.status = Some("Replica installed; waiting for a real issuer sync...");
                }
                Ok(_) | Err(TryRecvError::Disconnected) => {
                    self.fail("Replica connection is unconfirmed. Accepted metadata may remain stored; obtain a new invitation before another attempt.");
                    return None;
                }
                Err(TryRecvError::Empty) => {}
            },
            Stage::Waiting(next) if Instant::now() >= *next => {
                if !inspect(attempt, sync, false) {
                    self.fail("Live replica readiness could not be checked. Acceptance may remain stored.");
                    return None;
                }
            }
            Stage::RoleReady => {
                if !attempt.hierarchy_requested {
                    if db.try_send(DbCommand::LoadHierarchy).is_err() {
                        bevy::log::warn!("Join hydration database channel closed");
                        self.fail("Access converged but the stored scene could not be loaded. Join remains unconfirmed.");
                        return None;
                    }
                    attempt.hierarchy_requested = true;
                }
                if !inspect(attempt, sync, true) {
                    self.fail("Live replica readiness could not be confirmed. Acceptance may remain stored.");
                    return None;
                }
            }
            Stage::Transport { reply, after_role } => match reply.try_recv() {
                Ok(checked) if checked.request_id == attempt.generation => {
                    let Ok(ready) = checked.outcome else {
                        self.fail("The current native replica is no longer confirmed. Acceptance may remain stored.");
                        return None;
                    };
                    if !ready {
                        attempt.stage = Stage::Waiting(Instant::now() + Duration::from_millis(250));
                        self.status = Some("Waiting for a successful live sync with the issuer...");
                    } else if *after_role {
                        if let Some(destination) =
                            destination(&attempt.invitation, &attempt.generation, hierarchy)
                        {
                            self.attempt = None;
                            self.status =
                                Some("Ready: live issuer sync and stored scope access confirmed.");
                            self.error = None;
                            return Some(destination);
                        }
                        attempt.stage = Stage::Waiting(Instant::now() + Duration::from_millis(250));
                        self.status =
                            Some("Access converged; loading the invited scene hierarchy...");
                    } else {
                        attempt.check_round = attempt.check_round.saturating_add(1);
                        let request_id =
                            format!("{}-role-{}", attempt.generation, attempt.check_round);
                        if db
                            .try_send(DbCommand::CheckJoinReadiness {
                                credential: attempt.credential.clone(),
                                request_id: request_id.clone(),
                            })
                            .is_err()
                        {
                            bevy::log::warn!("Join readiness database channel closed");
                            self.fail("Stored scope readiness could not be checked. Acceptance may remain stored.");
                            return None;
                        }
                        attempt.stage = Stage::Role { request_id };
                    }
                }
                Ok(_) | Err(TryRecvError::Disconnected) => {
                    self.fail(
                        "Native readiness reply was not confirmed. Acceptance may remain stored.",
                    );
                    return None;
                }
                Err(TryRecvError::Empty) => {}
            },
            _ => {}
        }
        None
    }
}

fn native_request(attempt: &Attempt) -> JoinReplicaRequest {
    JoinReplicaRequest {
        request_id: attempt.generation.clone(),
        invitation: attempt.invitation.clone(),
        deadline: attempt.deadline,
    }
}
fn inspect(attempt: &mut Attempt, sync: &SyncCommandSenderRes, after_role: bool) -> bool {
    let (reply, receiver) = crossbeam::channel::bounded(1);
    if sync
        .0
        .try_send(SyncCommand::InspectJoinedReplica {
            request: native_request(attempt),
            reply,
        })
        .is_err()
    {
        bevy::log::warn!("Join native readiness sync channel closed");
        return false;
    }
    attempt.stage = Stage::Transport {
        reply: receiver,
        after_role,
    };
    true
}
fn admission_matches(admission: &JoinAdmission, invitation: &VerifiedJoinInvitation) -> bool {
    admission.nonce == invitation.nonce()
        && admission.verse_id == invitation.manifest().verse_id
        && admission.scope == invitation.scope()
        && admission.role == invitation.role().as_str()
}
fn destination(
    invitation: &VerifiedJoinInvitation,
    generation: &str,
    hierarchy: &VerseManager,
) -> Option<ReadyJoin> {
    let scope = fe_database::parse_scope(invitation.scope()).ok()?;
    let verse = hierarchy.verses.iter().find(|v| {
        v.id == scope.verse_id
            && v.namespace_id.as_deref() == Some(invitation.manifest().namespace_id.as_str())
    })?;
    let fractal_name = if let Some(fractal_id) = &scope.fractal_id {
        let fractal = verse.fractals.iter().find(|f| f.id == *fractal_id)?;
        if let Some(petal_id) = &scope.petal_id {
            fractal.petals.iter().find(|p| p.id == *petal_id)?;
        }
        Some(fractal.name.clone())
    } else {
        None
    };
    Some(ReadyJoin {
        scope,
        verse_name: verse.name.clone(),
        fractal_name,
        lease: JoinedReplicaLease {
            request_id: generation.into(),
            verse_id: invitation.manifest().verse_id.clone(),
            namespace_id: invitation.manifest().namespace_id.clone(),
        },
    })
}
fn unix_now() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs())
}
