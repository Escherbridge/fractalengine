//! One-use GUI invitation transfer contract; see src/AGENTS.md section qa-invitation.

use crate::{
    bridge::{bounded_bridge, BridgeReceiver, BridgeSender},
    join_invite::JoinCredential,
    qa_debug::QaFailure,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const JOB_LIMIT: Duration = Duration::from_secs(3);
pub const CONSUME_TTL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferMetadata {
    pub generation: String,
    pub issuer_did: String,
    pub recipient_did: String,
    pub verse_id: String,
    pub entity_kind: String,
    pub entity_id: String,
    pub scope: String,
    pub role: String,
    pub expiry_hours: u32,
    pub envelope_expires_at: u64,
    pub consume_expires_at: u64,
}

impl TransferMetadata {
    pub fn validate(&self) -> bool {
        self.generation.len() <= 64
            && self.generation.starts_with("invite-")
            && self.issuer_did.len() <= 128
            && self.issuer_did.starts_with("did:key:")
            && self.recipient_did.len() <= 128
            && self.recipient_did.starts_with("did:key:")
            && !self.verse_id.is_empty()
            && self.verse_id.len() <= 128
            && !self.entity_id.is_empty()
            && self.entity_id.len() <= 128
            && self.scope.len() <= 512
            && self.scope.starts_with("VERSE#")
            && matches!(self.entity_kind.as_str(), "verse" | "fractal" | "petal")
            && matches!(self.role.as_str(), "viewer" | "editor" | "manager")
            && (1..=720).contains(&self.expiry_hours)
            && self.consume_expires_at <= self.envelope_expires_at
    }
}

#[derive(Debug, Clone)]
pub enum TransferOperation {
    Observe,
    Consume(TransferMetadata),
}

#[derive(Debug, Clone)]
pub struct TransferCommand {
    pub operation: TransferOperation,
    pub issuer_did: String,
    pub deadline: Instant,
}
impl TransferCommand {
    pub fn is_current(&self) -> bool {
        let now = Instant::now();
        self.deadline > now && self.deadline.duration_since(now) <= JOB_LIMIT
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinInputWindow {
    pub window_id: u64,
    pub width: u32,
    pub height: u32,
    pub scale_bits: u64,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct JoinInputLease {
    pub lease_id: String,
    pub input_widget: String,
    pub join_widget: String,
    pub input_bounds: [f32; 4],
    pub join_bounds: [f32; 4],
}

#[derive(Debug)]
pub enum TransferReply {
    Observed {
        local_did: String,
        invitation: Option<TransferMetadata>,
        join_ready_for_input: bool,
        join_input_lease: Option<JoinInputLease>,
    },
    Consumed {
        metadata: TransferMetadata,
        credential: JoinCredential,
    },
}
pub type TransferOutcome = Result<TransferReply, QaFailure>;
pub type TransferSender = BridgeSender<TransferCommand, TransferOutcome>;
pub type TransferReceiver = BridgeReceiver<TransferCommand, TransferOutcome>;
pub fn invitation_bridge() -> (TransferSender, TransferReceiver) {
    bounded_bridge(1)
}
