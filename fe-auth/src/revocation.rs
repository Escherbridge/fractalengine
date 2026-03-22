use crate::cache::SessionCache;
use fe_database::{DbHandle, NodeId, OpLogEntry, OpType, PetalId};

pub async fn revoke_session(
    peer_pub_key_bytes: [u8; 32],
    petal_id: &PetalId,
    db_handle: &DbHandle,
    cache: &mut SessionCache,
) -> anyhow::Result<()> {
    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: NodeId(hex::encode(peer_pub_key_bytes)),
        op_type: OpType::RevokeSession,
        payload: serde_json::json!({
            "petal_id": petal_id.0.to_string(),
            "pub_key": hex::encode(peer_pub_key_bytes),
        }),
        sig: "00".repeat(64),
    };
    fe_database::op_log::write_op_log(&db_handle.0, entry).await?;
    // CROSS-CRATE: send NetworkCommand::BroadcastRevocation to network thread — deferred Sprint 5B
    cache.revoke(&peer_pub_key_bytes);
    Ok(())
}
