//! Correlated native invitation transport work; see src/AGENTS.md section join-tickets.

use std::time::Instant;
use fe_database::join_invite::{JoinRole, VerifiedJoinInvitation};
pub use fe_database::join_ticket::NativeJoinTicket;

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
