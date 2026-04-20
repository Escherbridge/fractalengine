//! Per-verse peer tracking for P2P Mycelium Phase F.
//!
//! [`VersePeers`] maintains a map from verse ID to the set of known peer
//! node addresses. This map is now populated from
//! [`SyncEvent::PeerConnected`] / [`SyncEvent::PeerDisconnected`] events
//! emitted by the sync thread. Previously this was an empty stub; it is now
//! fully functional.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

/// Bevy resource tracking known peers per verse.
///
/// Keyed by verse ID; values are sets of peer node ID strings (hex-encoded or
/// `did:key` format). Populated by handling [`SyncEvent::PeerConnected`] and
/// [`SyncEvent::PeerDisconnected`] events from the sync thread.
#[derive(Resource, Default, Debug, Clone)]
pub struct VersePeers {
    pub peers: HashMap<String, HashSet<String>>,
}

impl VersePeers {
    // ── Per-verse map API (original) ─────────────────────────────────────────

    /// Get the set of peer node IDs for a verse.
    pub fn get_peers(&self, verse_id: &str) -> Option<&HashSet<String>> {
        self.peers.get(verse_id)
    }

    /// Add a peer to a verse's peer set.
    pub fn add_peer(&mut self, verse_id: &str, node_id: String) {
        self.peers
            .entry(verse_id.to_string())
            .or_default()
            .insert(node_id);
    }

    /// Remove a peer from a verse's peer set.
    pub fn remove_peer(&mut self, verse_id: &str, node_id: &str) {
        if let Some(set) = self.peers.get_mut(verse_id) {
            set.remove(node_id);
        }
    }

    /// Total peer count across all verses (deduplicated by peer ID).
    pub fn total_unique_peers(&self) -> usize {
        let mut all: HashSet<&String> = HashSet::new();
        for set in self.peers.values() {
            for peer in set {
                all.insert(peer);
            }
        }
        all.len()
    }

    // ── Single-verse convenience API ─────────────────────────────────────────

    /// Add a peer (by `did:key` string) to the given verse.
    ///
    /// Returns `true` if the peer was newly added, `false` if already present.
    pub fn add(&mut self, verse_id: &str, peer_did: String) -> bool {
        self.peers
            .entry(verse_id.to_string())
            .or_default()
            .insert(peer_did)
    }

    /// Remove a peer from the given verse.
    ///
    /// Returns `true` if the peer was present and removed, `false` otherwise.
    pub fn remove(&mut self, verse_id: &str, peer_did: &str) -> bool {
        if let Some(set) = self.peers.get_mut(verse_id) {
            set.remove(peer_did)
        } else {
            false
        }
    }

    /// Check if a peer is currently connected to the given verse.
    pub fn is_connected(&self, verse_id: &str, peer_did: &str) -> bool {
        self.peers
            .get(verse_id)
            .map(|s| s.contains(peer_did))
            .unwrap_or(false)
    }

    /// Get count of connected peers for the given verse.
    pub fn count(&self, verse_id: &str) -> usize {
        self.peers.get(verse_id).map(|s| s.len()).unwrap_or(0)
    }

    /// Iterate over all connected peer DIDs for the given verse.
    pub fn iter_verse<'a>(
        &'a self,
        verse_id: &str,
    ) -> impl Iterator<Item = &'a String> {
        self.peers
            .get(verse_id)
            .into_iter()
            .flat_map(|s| s.iter())
    }

    /// Check if no peers are connected to the given verse.
    pub fn is_empty_verse(&self, verse_id: &str) -> bool {
        self.peers
            .get(verse_id)
            .map(|s| s.is_empty())
            .unwrap_or(true)
    }

    /// Remove all peers from the given verse (e.g., on verse switch).
    pub fn clear_verse(&mut self, verse_id: &str) {
        if let Some(set) = self.peers.get_mut(verse_id) {
            set.clear();
        }
    }
}

/// Compute the gossip topic hash for a verse.
///
/// The topic is derived deterministically from the verse ID so all peers
/// joining the same verse converge on the same gossip channel.
pub fn verse_gossip_topic(verse_id: &str) -> [u8; 32] {
    *blake3::hash(format!("fractalengine:verse:{verse_id}").as_bytes()).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Original tests (preserved) ────────────────────────────────────────────

    #[test]
    fn verse_peers_add_and_get() {
        let mut vp = VersePeers::default();
        vp.add_peer("v1", "node-a".into());
        vp.add_peer("v1", "node-b".into());
        vp.add_peer("v2", "node-a".into());

        assert_eq!(vp.get_peers("v1").unwrap().len(), 2);
        assert_eq!(vp.get_peers("v2").unwrap().len(), 1);
        assert!(vp.get_peers("v3").is_none());
        assert_eq!(vp.total_unique_peers(), 2); // node-a deduplicated
    }

    #[test]
    fn verse_peers_remove() {
        let mut vp = VersePeers::default();
        vp.add_peer("v1", "node-a".into());
        vp.remove_peer("v1", "node-a");
        assert!(vp.get_peers("v1").unwrap().is_empty());
    }

    #[test]
    fn gossip_topic_deterministic() {
        let t1 = verse_gossip_topic("verse-123");
        let t2 = verse_gossip_topic("verse-123");
        assert_eq!(t1, t2);

        let t3 = verse_gossip_topic("verse-456");
        assert_ne!(t1, t3);
    }

    // ── New single-verse convenience API tests ────────────────────────────────

    #[test]
    fn new_creates_empty() {
        let vp = VersePeers::default();
        assert!(vp.peers.is_empty());
        assert!(vp.is_empty_verse("v1"));
        assert_eq!(vp.count("v1"), 0);
    }

    #[test]
    fn add_inserts_peer_is_connected_true() {
        let mut vp = VersePeers::default();
        let inserted = vp.add("v1", "did:key:z6MkAAA".into());
        assert!(inserted, "first insert should return true");
        assert!(vp.is_connected("v1", "did:key:z6MkAAA"));
    }

    #[test]
    fn remove_removes_peer_is_connected_false() {
        let mut vp = VersePeers::default();
        vp.add("v1", "did:key:z6MkBBB".into());
        let removed = vp.remove("v1", "did:key:z6MkBBB");
        assert!(removed, "remove of existing peer should return true");
        assert!(!vp.is_connected("v1", "did:key:z6MkBBB"));
    }

    #[test]
    fn add_returns_false_for_duplicate() {
        let mut vp = VersePeers::default();
        vp.add("v1", "did:key:z6MkCCC".into());
        let duplicate = vp.add("v1", "did:key:z6MkCCC".into());
        assert!(!duplicate, "inserting duplicate should return false");
        assert_eq!(vp.count("v1"), 1);
    }

    #[test]
    fn remove_returns_false_for_unknown_peer() {
        let mut vp = VersePeers::default();
        let result = vp.remove("v1", "did:key:unknown");
        assert!(!result);
    }

    #[test]
    fn count_reflects_actual_count() {
        let mut vp = VersePeers::default();
        assert_eq!(vp.count("v1"), 0);
        vp.add("v1", "did:key:z6MkDDD".into());
        assert_eq!(vp.count("v1"), 1);
        vp.add("v1", "did:key:z6MkEEE".into());
        assert_eq!(vp.count("v1"), 2);
        vp.remove("v1", "did:key:z6MkDDD");
        assert_eq!(vp.count("v1"), 1);
    }

    #[test]
    fn iter_verse_returns_all_peers() {
        let mut vp = VersePeers::default();
        vp.add("v1", "did:key:z6MkFFF".into());
        vp.add("v1", "did:key:z6MkGGG".into());
        let mut collected: Vec<String> = vp.iter_verse("v1").cloned().collect();
        collected.sort();
        assert_eq!(
            collected,
            vec!["did:key:z6MkFFF".to_string(), "did:key:z6MkGGG".to_string()]
        );
    }

    #[test]
    fn is_empty_verse_true_initially_false_after_add() {
        let mut vp = VersePeers::default();
        assert!(vp.is_empty_verse("v1"));
        vp.add("v1", "did:key:z6MkHHH".into());
        assert!(!vp.is_empty_verse("v1"));
    }

    #[test]
    fn clear_verse_removes_all_peers() {
        let mut vp = VersePeers::default();
        vp.add("v1", "did:key:z6MkIII".into());
        vp.add("v1", "did:key:z6MkJJJ".into());
        assert_eq!(vp.count("v1"), 2);
        vp.clear_verse("v1");
        assert_eq!(vp.count("v1"), 0);
        assert!(vp.is_empty_verse("v1"));
    }
}
