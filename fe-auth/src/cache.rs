use fe_database::RoleId;
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
