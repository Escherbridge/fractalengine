//! API-to-sync bridge for P2P identity, addressed invite ticketing, replica
//! join orchestration, and runtime sync state (`fe_runtime::peering::PeeringCall`).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fe_database::join_invite::{JoinInvitation, JoinRole};
use fe_runtime::peering::{PeeringCall, PeeringCallReceiver};

use crate::join_ticket::{ExportJoinTicket, JoinReplicaRequest};
use crate::messages::{SyncCommand, SyncCommandSender};

const FAST_CALL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONCURRENT_PEERING_WAITS: usize = 16;

fn unix_now_secs() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "system clock is unavailable".to_owned())
}

/// Spawns the API-to-sync peering bridge until `receiver` closes.
///
/// Runs reply waits on bounded worker slots so slow replica imports or ticket
/// exports never head-of-line block identity or sync-state lookups.
pub fn bridge_peering_calls(
    receiver: PeeringCallReceiver,
    sync_command_sender: SyncCommandSender,
    local_did: String,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("peering-api-bridge".into())
        .spawn(move || {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_PEERING_WAITS));
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                tracing::error!("peering API bridge failed to create tokio runtime");
                return;
            };
            rt.block_on(async move {
                while let Ok(call) = receiver.recv() {
                    let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                        tracing::warn!("peering bridge concurrency cap reached; dropping call");
                        reject_overloaded(call);
                        continue;
                    };
                    let sender = sync_command_sender.clone();
                    let did = local_did.clone();
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        handle_call(call, &sender, &did);
                    });
                }
            });
        })
}

fn reject_overloaded(call: PeeringCall) {
    match call {
        PeeringCall::ExportJoinTicket { reply, .. } => {
            let _ = reply.send(Err("peering bridge busy".into()));
        }
        PeeringCall::JoinVerseReplica { reply, .. } => {
            let _ = reply.send(Err("peering bridge busy".into()));
        }
        PeeringCall::InspectJoinedReplica { reply, .. } => {
            let _ = reply.send(Err("peering bridge busy".into()));
        }
        PeeringCall::GetIdentity { .. } | PeeringCall::GetSyncState { .. } => {}
    }
}

fn handle_call(call: PeeringCall, sync_sender: &SyncCommandSender, local_did: &str) {
    match call {
        PeeringCall::GetIdentity { detail, reply } => {
            let (tx, rx) = crossbeam::channel::bounded(1);
            if sync_sender
                .send(SyncCommand::GetPeeringIdentity { detail, reply: tx })
                .is_err()
            {
                return;
            }
            if let Ok(identity) = rx.recv_timeout(FAST_CALL_TIMEOUT) {
                let _ = reply.send(identity);
            }
        }
        PeeringCall::GetSyncState { verse_id, reply } => {
            let (tx, rx) = crossbeam::channel::bounded(1);
            if sync_sender
                .send(SyncCommand::GetSyncState {
                    verse_id,
                    reply: tx,
                })
                .is_err()
            {
                return;
            }
            if let Ok(state) = rx.recv_timeout(FAST_CALL_TIMEOUT) {
                let _ = reply.send(state);
            }
        }
        PeeringCall::ExportJoinTicket {
            request_id,
            verse_id,
            namespace_id,
            issuer_did,
            role,
            deadline_secs,
            reply,
        } => {
            let result = export_ticket(
                sync_sender,
                request_id,
                verse_id,
                namespace_id,
                issuer_did,
                &role,
                deadline_secs,
            );
            let _ = reply.send(result);
        }
        PeeringCall::JoinVerseReplica {
            request_id,
            credential,
            deadline_secs,
            reply,
        } => {
            let result =
                join_replica(sync_sender, local_did, request_id, &credential, deadline_secs);
            let _ = reply.send(result);
        }
        PeeringCall::InspectJoinedReplica {
            request_id,
            credential,
            deadline_secs,
            reply,
        } => {
            let result =
                inspect_replica(sync_sender, local_did, request_id, &credential, deadline_secs);
            let _ = reply.send(result);
        }
    }
}

fn export_ticket(
    sync_sender: &SyncCommandSender,
    request_id: String,
    verse_id: String,
    namespace_id: String,
    issuer_did: String,
    role: &str,
    deadline_secs: u64,
) -> Result<String, String> {
    let parsed_role = JoinRole::parse(role).map_err(|e| format!("Unsupported role: {e}"))?;
    let wait = Duration::from_secs(deadline_secs.clamp(1, 120));
    let (tx, rx) = crossbeam::channel::bounded(1);
    sync_sender
        .send(SyncCommand::ExportJoinTicket {
            request: ExportJoinTicket {
                request_id,
                verse_id,
                namespace_id,
                issuer_did,
                role: parsed_role,
                deadline: Instant::now() + wait,
            },
            reply: tx,
        })
        .map_err(|_| "Live P2P is unavailable".to_owned())?;
    let exported = rx
        .recv_timeout(wait + Duration::from_secs(2))
        .map_err(|_| "Timed out waiting for P2P ticket export".to_owned())?;
    exported
        .ticket
        .map(|ticket| ticket.encoded().to_owned())
        .map_err(str::to_owned)
}

fn parse_join_request(
    local_did: &str,
    request_id: String,
    credential: &str,
    deadline_secs: u64,
) -> Result<(JoinReplicaRequest, Duration), String> {
    let now = unix_now_secs()?;
    let invitation = JoinInvitation::parse(credential, local_did, now)
        .map_err(|e| format!("Invalid or expired invitation: {e}"))?;
    let wait = Duration::from_secs(deadline_secs.clamp(1, 120));
    Ok((
        JoinReplicaRequest {
            request_id,
            invitation,
            deadline: Instant::now() + wait,
        },
        wait,
    ))
}

fn join_replica(
    sync_sender: &SyncCommandSender,
    local_did: &str,
    request_id: String,
    credential: &str,
    deadline_secs: u64,
) -> Result<(), String> {
    let (request, wait) = parse_join_request(local_did, request_id, credential, deadline_secs)?;
    let (tx, rx) = crossbeam::channel::bounded(1);
    sync_sender
        .send(SyncCommand::JoinVerseReplica { request, reply: tx })
        .map_err(|_| "Live P2P is unavailable".to_owned())?;
    let opened = rx
        .recv_timeout(wait + Duration::from_secs(2))
        .map_err(|_| "Timed out waiting for replica open".to_owned())?;
    opened.outcome.map_err(str::to_owned)
}

fn inspect_replica(
    sync_sender: &SyncCommandSender,
    local_did: &str,
    request_id: String,
    credential: &str,
    deadline_secs: u64,
) -> Result<bool, String> {
    let (request, wait) = parse_join_request(local_did, request_id, credential, deadline_secs)?;
    let (tx, rx) = crossbeam::channel::bounded(1);
    sync_sender
        .send(SyncCommand::InspectJoinedReplica { request, reply: tx })
        .map_err(|_| "Live P2P is unavailable".to_owned())?;
    let checked = rx
        .recv_timeout(wait + Duration::from_secs(2))
        .map_err(|_| "Timed out waiting for replica readiness check".to_owned())?;
    checked.outcome.map_err(str::to_owned)
}
