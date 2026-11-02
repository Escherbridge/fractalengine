//! Correlated native invitation transport work; see src/AGENTS.md section join-tickets.

use fe_database::join_invite::{JoinRole, VerifiedJoinInvitation};
pub use fe_database::join_ticket::NativeJoinTicket;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ExportJoinTicket {
    pub request_id: String,
    pub verse_id: String,
    pub namespace_id: String,
    pub issuer_did: String,
    pub role: JoinRole,
    pub deadline: Instant,
}

#[derive(Debug, Clone)]
pub struct JoinTicketExported {
    pub request_id: String,
    pub ticket: Result<NativeJoinTicket, &'static str>,
}

#[derive(Debug, Clone)]
pub struct JoinReplicaRequest {
    pub request_id: String,
    pub invitation: VerifiedJoinInvitation,
    pub deadline: Instant,
}

#[derive(Debug, Clone)]
pub struct JoinReplicaOpened {
    pub request_id: String,
    pub outcome: Result<(), &'static str>,
}

#[derive(Debug, Clone)]
pub struct JoinReplicaReadiness {
    pub request_id: String,
    pub outcome: Result<bool, &'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedReplicaLease {
    pub request_id: String,
    pub verse_id: String,
    pub namespace_id: String,
}

impl JoinedReplicaLease {
    pub fn for_request(request: &JoinReplicaRequest) -> Self {
        Self {
            request_id: request.request_id.clone(),
            verse_id: request.invitation.manifest().verse_id.clone(),
            namespace_id: request.invitation.manifest().namespace_id.clone(),
        }
    }
}

#[derive(Default)]
pub(crate) struct JoinedReplicaLeases(std::collections::HashMap<String, JoinedReplicaLease>);

impl JoinedReplicaLeases {
    pub(crate) fn opened(&mut self, lease: JoinedReplicaLease) {
        self.0.insert(lease.verse_id.clone(), lease);
    }
    pub(crate) fn forget(&mut self, verse: &str) {
        self.0.remove(verse);
    }
    pub(crate) fn owns(&self, lease: &JoinedReplicaLease) -> bool {
        self.0.get(&lease.verse_id) == Some(lease)
    }
    pub(crate) fn take(&mut self, lease: &JoinedReplicaLease) -> bool {
        if !self.owns(lease) {
            return false;
        }
        self.forget(&lease.verse_id);
        true
    }
}
