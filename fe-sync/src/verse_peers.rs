//! Per-verse peer tracking for P2P Mycelium Phase F.
//!
//! [`VersePeers`] maintains a map from verse ID to the set of known peer
//! node addresses. In a future phase, gossip presence messages will populate
//! this map. For now it is an empty stub.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

/// Bevy resource tracking known peers per verse.
///
/// Keyed by verse ID; values are sets of peer node ID strings (hex-encoded).
/// Phase F stub: always empty. Will be populated by gossip presence messages.
#[derive(Resource, Default, Debug, Clone)]
pub struct VersePeers {
    pub peers: HashMap<String, HashSet<String>>,
}

impl VersePeers {
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

    /// Total peer count across all verses (deduplicated).
    pub fn total_unique_peers(&self) -> usize {
        let mut all: HashSet<&String> = HashSet::new();
        for set in self.peers.values() {
            for peer in set {
                all.insert(peer);
            }
        }
        all.len()
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
}
