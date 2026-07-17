//! Peer session role cache + session revocation (see AGENTS.md §session-cache).
use crate::{DbHandle, NodeId, OpLogEntry, OpType, PetalId, RoleId};
use std::collections::HashMap;
use std::time::Instant;

pub const SESSION_TTL_SECS: u64 = 60;

pub struct SessionCache {
    entries: HashMap<[u8; 32], (RoleId, Instant)>,
}

impl SessionCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn get(&self, pub_key: &[u8; 32]) -> Option<&RoleId> {
        self.entries
            .get(pub_key)
            .filter(|(_, t): &&(RoleId, Instant)| t.elapsed().as_secs() < SESSION_TTL_SECS)
            .map(|(role, _)| role)
    }

    pub fn insert(&mut self, pub_key: [u8; 32], role: RoleId) {
        self.entries.insert(pub_key, (role, Instant::now()));
    }

    pub fn revoke(&mut self, pub_key: &[u8; 32]) {
        self.entries.remove(pub_key);
    }

    pub fn prune_expired(&mut self) {
        self.entries
            .retain(|_, (_, t): &mut (RoleId, Instant)| t.elapsed().as_secs() < SESSION_TTL_SECS);
    }
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::new()
    }
}

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
        hlc_timestamp: String::new(),
    };
    crate::op_log::write_op_log(&db_handle.0, entry).await?;
    // CROSS-CRATE: send NetworkCommand::BroadcastRevocation to network thread — deferred Sprint 5B
    cache.revoke(&peer_pub_key_bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SessionCache, SESSION_TTL_SECS};
    use crate::RoleId;

    #[test]
    fn test_session_cache_insert_and_get() {
        let mut cache = SessionCache::new();
        let key = [1u8; 32];
        cache.insert(key, RoleId("admin".to_string()));
        assert!(cache.get(&key).is_some());
        assert_eq!(cache.get(&key).unwrap().0, "admin");
    }

    #[test]
    fn test_session_cache_revoke() {
        let mut cache = SessionCache::new();
        let key = [2u8; 32];
        cache.insert(key, RoleId("public".to_string()));
        cache.revoke(&key);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_session_cache_different_keys() {
        let mut cache = SessionCache::new();
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        cache.insert(key1, RoleId("admin".to_string()));
        cache.insert(key2, RoleId("public".to_string()));
        assert!(cache.get(&key1).is_some());
        assert!(cache.get(&key2).is_some());
        cache.revoke(&key1);
        assert!(cache.get(&key1).is_none());
        assert!(cache.get(&key2).is_some());
    }

    // test_session_cache_ttl: requires mock Instant — DEFERRED TO WAVE 6 VALIDATION

    const _: u64 = SESSION_TTL_SECS;
}
