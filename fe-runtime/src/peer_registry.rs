use bevy::prelude::*;
use std::collections::HashMap;

/// A single peer's composite state, aggregated from multiple subsystems.
#[derive(Debug, Clone)]
pub struct PeerEntry {
    /// The peer's canonical identifier (did:key format)
    pub peer_did: String,
    /// The peer's role at the current scope (None = not yet resolved)
    pub role: Option<String>,
    /// Human-readable display name (None = not yet received from profile system)
    pub display_name: Option<String>,
    /// Whether the peer is currently connected
    pub is_online: bool,
}

impl PeerEntry {
    /// Creates a new PeerEntry with all optional fields as None and is_online as false.
    pub fn new(peer_did: String) -> Self {
        Self {
            peer_did,
            role: None,
            display_name: None,
            is_online: false,
        }
    }

    /// Returns the display name if present, otherwise a truncated version of the peer_did.
    /// Truncated format: first 8 chars + "..." + last 4 chars.
    pub fn display_name_or_truncated(&self) -> String {
        if let Some(ref name) = self.display_name {
            name.clone()
        } else {
            let did = &self.peer_did;
            let len = did.len();
            if len <= 12 {
                did.clone()
            } else {
                format!("{}...{}", &did[..8], &did[len - 4..])
            }
        }
    }
}

/// Unified peer registry — aggregates role data, display names, and presence.
/// Both Inspector Settings and Profile Manager tracks write to this;
/// the Access tab reads the composite.
#[derive(Resource, Debug, Default)]
pub struct PeerRegistry {
    peers: HashMap<String, PeerEntry>,
}

impl PeerRegistry {
    /// Adds or replaces a peer entry by `peer_did`.
    pub fn insert(&mut self, entry: PeerEntry) {
        self.peers.insert(entry.peer_did.clone(), entry);
    }

    /// Lookup a peer entry by did.
    pub fn get(&self, peer_did: &str) -> Option<&PeerEntry> {
        self.peers.get(peer_did)
    }

    /// Mutable lookup by did.
    pub fn get_mut(&mut self, peer_did: &str) -> Option<&mut PeerEntry> {
        self.peers.get_mut(peer_did)
    }

    /// Remove and return a peer entry.
    pub fn remove(&mut self, peer_did: &str) -> Option<PeerEntry> {
        self.peers.remove(peer_did)
    }

    /// Returns true if the registry contains an entry for the given did.
    pub fn contains(&self, peer_did: &str) -> bool {
        self.peers.contains_key(peer_did)
    }

    /// Iterate all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PeerEntry)> {
        self.peers.iter()
    }

    /// Returns the number of peer entries.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Returns true if the registry has no entries.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Update online status for a peer, creating an entry if absent.
    pub fn set_online(&mut self, peer_did: &str, online: bool) {
        let entry = self.peers.entry(peer_did.to_string()).or_insert_with(|| PeerEntry {
            peer_did: peer_did.to_string(),
            role: None,
            display_name: None,
            is_online: false,
        });
        entry.is_online = online;
    }

    /// Update role for a peer, creating an entry if absent.
    pub fn set_role(&mut self, peer_did: &str, role: Option<String>) {
        let entry = self.peers.entry(peer_did.to_string()).or_insert_with(|| PeerEntry {
            peer_did: peer_did.to_string(),
            role: None,
            display_name: None,
            is_online: false,
        });
        entry.role = role;
    }

    /// Update display name for a peer, creating an entry if absent.
    pub fn set_display_name(&mut self, peer_did: &str, name: Option<String>) {
        let entry = self.peers.entry(peer_did.to_string()).or_insert_with(|| PeerEntry {
            peer_did: peer_did.to_string(),
            role: None,
            display_name: None,
            is_online: false,
        });
        entry.display_name = name;
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.peers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(did: &str) -> PeerEntry {
        PeerEntry {
            peer_did: did.to_string(),
            role: Some("admin".to_string()),
            display_name: Some("Alice".to_string()),
            is_online: true,
        }
    }

    #[test]
    fn test_insert_and_get() {
        let mut registry = PeerRegistry::default();
        let entry = make_entry("did:key:alice");
        registry.insert(entry);
        let got = registry.get("did:key:alice").unwrap();
        assert_eq!(got.peer_did, "did:key:alice");
        assert_eq!(got.role, Some("admin".to_string()));
        assert_eq!(got.display_name, Some("Alice".to_string()));
        assert!(got.is_online);
    }

    #[test]
    fn test_get_returns_none_for_unknown_peer() {
        let registry = PeerRegistry::default();
        assert!(registry.get("did:key:unknown").is_none());
    }

    #[test]
    fn test_get_mut_allows_modification() {
        let mut registry = PeerRegistry::default();
        registry.insert(make_entry("did:key:bob"));
        let entry = registry.get_mut("did:key:bob").unwrap();
        entry.is_online = false;
        assert!(!registry.get("did:key:bob").unwrap().is_online);
    }

    #[test]
    fn test_remove_returns_entry_and_subsequent_get_returns_none() {
        let mut registry = PeerRegistry::default();
        registry.insert(make_entry("did:key:carol"));
        let removed = registry.remove("did:key:carol");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().peer_did, "did:key:carol");
        assert!(registry.get("did:key:carol").is_none());
    }

    #[test]
    fn test_contains() {
        let mut registry = PeerRegistry::default();
        assert!(!registry.contains("did:key:dave"));
        registry.insert(make_entry("did:key:dave"));
        assert!(registry.contains("did:key:dave"));
    }

    #[test]
    fn test_iter_returns_all_entries() {
        let mut registry = PeerRegistry::default();
        registry.insert(make_entry("did:key:a"));
        registry.insert(make_entry("did:key:b"));
        registry.insert(make_entry("did:key:c"));
        let dids: Vec<&String> = registry.iter().map(|(k, _)| k).collect();
        assert_eq!(dids.len(), 3);
        assert!(dids.contains(&&"did:key:a".to_string()));
        assert!(dids.contains(&&"did:key:b".to_string()));
        assert!(dids.contains(&&"did:key:c".to_string()));
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut registry = PeerRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        registry.insert(make_entry("did:key:x"));
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_set_online_creates_entry_if_absent() {
        let mut registry = PeerRegistry::default();
        registry.set_online("did:key:new", true);
        let entry = registry.get("did:key:new").unwrap();
        assert_eq!(entry.peer_did, "did:key:new");
        assert!(entry.is_online);
        assert!(entry.role.is_none());
        assert!(entry.display_name.is_none());
    }

    #[test]
    fn test_set_online_updates_existing_entry() {
        let mut registry = PeerRegistry::default();
        let mut e = make_entry("did:key:existing");
        e.is_online = false;
        registry.insert(e);
        registry.set_online("did:key:existing", true);
        assert!(registry.get("did:key:existing").unwrap().is_online);
        // Other fields should be preserved
        assert_eq!(registry.get("did:key:existing").unwrap().role, Some("admin".to_string()));
    }

    #[test]
    fn test_set_role_creates_entry_if_absent() {
        let mut registry = PeerRegistry::default();
        registry.set_role("did:key:newrole", Some("viewer".to_string()));
        let entry = registry.get("did:key:newrole").unwrap();
        assert_eq!(entry.role, Some("viewer".to_string()));
        assert!(!entry.is_online);
        assert!(entry.display_name.is_none());
    }

    #[test]
    fn test_set_role_updates_existing_entry() {
        let mut registry = PeerRegistry::default();
        registry.insert(make_entry("did:key:rolupdate"));
        registry.set_role("did:key:rolupdate", Some("moderator".to_string()));
        assert_eq!(
            registry.get("did:key:rolupdate").unwrap().role,
            Some("moderator".to_string())
        );
        // Other fields preserved
        assert_eq!(
            registry.get("did:key:rolupdate").unwrap().display_name,
            Some("Alice".to_string())
        );
    }

    #[test]
    fn test_set_display_name_creates_entry_if_absent() {
        let mut registry = PeerRegistry::default();
        registry.set_display_name("did:key:newname", Some("Bob".to_string()));
        let entry = registry.get("did:key:newname").unwrap();
        assert_eq!(entry.display_name, Some("Bob".to_string()));
        assert!(!entry.is_online);
        assert!(entry.role.is_none());
    }

    #[test]
    fn test_clear_removes_everything() {
        let mut registry = PeerRegistry::default();
        registry.insert(make_entry("did:key:p1"));
        registry.insert(make_entry("did:key:p2"));
        assert_eq!(registry.len(), 2);
        registry.clear();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_display_name_or_truncated_with_name() {
        let entry = PeerEntry {
            peer_did: "did:key:z6MksomeverylongDIDstring1234".to_string(),
            role: None,
            display_name: Some("Charlie".to_string()),
            is_online: false,
        };
        assert_eq!(entry.display_name_or_truncated(), "Charlie");
    }

    #[test]
    fn test_display_name_or_truncated_without_name() {
        let entry = PeerEntry {
            peer_did: "did:key:z6MksomeverylongDIDstring1234".to_string(),
            role: None,
            display_name: None,
            is_online: false,
        };
        let result = entry.display_name_or_truncated();
        // "did:key:" is 8 chars, last 4 of "did:key:z6MksomeverylongDIDstring1234"
        let did = "did:key:z6MksomeverylongDIDstring1234";
        let expected = format!("{}...{}", &did[..8], &did[did.len() - 4..]);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_peer_entry_new_defaults() {
        let entry = PeerEntry::new("did:key:fresh".to_string());
        assert_eq!(entry.peer_did, "did:key:fresh");
        assert!(entry.role.is_none());
        assert!(entry.display_name.is_none());
        assert!(!entry.is_online);
    }

    #[test]
    fn test_default_for_peer_registry_is_empty() {
        let registry = PeerRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}
