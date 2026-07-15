//! O(1) node lookup index for `VerseManager` — see AGENTS.md §node-index.

use super::{NodeEntry, VerseManager};

impl VerseManager {
    /// Rebuild the node_id → tree-indices map from scratch.
    pub(crate) fn rebuild_node_index(&mut self) {
        self.node_index.clear();
        for (vi, verse) in self.verses.iter().enumerate() {
            for (fi, fractal) in verse.fractals.iter().enumerate() {
                for (pi, petal) in fractal.petals.iter().enumerate() {
                    for node in &petal.nodes {
                        self.node_index.insert(node.id.clone(), (vi, fi, pi));
                    }
                }
            }
        }
    }

    /// Tree indices of a petal by ID.
    fn petal_indices(&self, petal_id: &str) -> Option<(usize, usize, usize)> {
        for (vi, verse) in self.verses.iter().enumerate() {
            for (fi, fractal) in verse.fractals.iter().enumerate() {
                for (pi, petal) in fractal.petals.iter().enumerate() {
                    if petal.id == petal_id {
                        return Some((vi, fi, pi));
                    }
                }
            }
        }
        None
    }

    /// Push a node into a petal and index it; `false` when the petal is unknown.
    pub(crate) fn add_node(&mut self, petal_id: &str, node: NodeEntry) -> bool {
        let Some((vi, fi, pi)) = self.petal_indices(petal_id) else {
            return false;
        };
        let Some(petal) = self
            .verses
            .get_mut(vi)
            .and_then(|v| v.fractals.get_mut(fi))
            .and_then(|f| f.petals.get_mut(pi))
        else {
            return false;
        };
        self.node_index.insert(node.id.clone(), (vi, fi, pi));
        petal.nodes.push(node);
        true
    }

    /// Remove a node from a petal (no-op on unknown petal) and drop its index entry.
    pub(crate) fn remove_node(&mut self, petal_id: &str, node_id: &str) {
        if let Some(petal) = self.find_petal_mut(petal_id) {
            petal.nodes.retain(|n| n.id != node_id);
        }
        self.node_index.remove(node_id);
    }

    /// Indexed node lookup; stale/missing entries fall back to a full walk and self-heal.
    fn node_entry_mut(&mut self, node_id: &str) -> Option<&mut NodeEntry> {
        if let Some(&(vi, fi, pi)) = self.node_index.get(node_id) {
            let indexed_hit = self
                .verses
                .get(vi)
                .and_then(|v| v.fractals.get(fi))
                .and_then(|f| f.petals.get(pi))
                .is_some_and(|p| p.nodes.iter().any(|n| n.id == node_id));
            if indexed_hit {
                return self
                    .verses
                    .get_mut(vi)?
                    .fractals
                    .get_mut(fi)?
                    .petals
                    .get_mut(pi)?
                    .nodes
                    .iter_mut()
                    .find(|n| n.id == node_id);
            }
        }
        let mut found = None;
        'walk: for (vi, verse) in self.verses.iter().enumerate() {
            for (fi, fractal) in verse.fractals.iter().enumerate() {
                for (pi, petal) in fractal.petals.iter().enumerate() {
                    if petal.nodes.iter().any(|n| n.id == node_id) {
                        found = Some((vi, fi, pi));
                        break 'walk;
                    }
                }
            }
        }
        let (vi, fi, pi) = found?;
        self.node_index.insert(node_id.to_string(), (vi, fi, pi));
        self.verses
            .get_mut(vi)?
            .fractals
            .get_mut(fi)?
            .petals
            .get_mut(pi)?
            .nodes
            .iter_mut()
            .find(|n| n.id == node_id)
    }

    /// Update position of a node by its ID (indexed, O(1) average).
    pub fn update_node_position(&mut self, node_id: &str, position: [f32; 3]) {
        if let Some(node) = self.node_entry_mut(node_id) {
            node.position = position;
        }
    }

    /// Update webpage_url of a node by its ID (indexed, O(1) average).
    pub fn update_node_url(&mut self, node_id: &str, url: Option<String>) {
        if let Some(node) = self.node_entry_mut(node_id) {
            node.webpage_url = url;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};

    fn node(id: &str) -> NodeEntry {
        NodeEntry {
            id: id.to_string(),
            name: id.to_string(),
            has_asset: true,
            position: [1.0, 2.0, 3.0],
            webpage_url: None,
            asset_path: Some("models/cube.glb".to_string()),
        }
    }

    fn petal(id: &str, nodes: Vec<NodeEntry>) -> PetalEntry {
        PetalEntry {
            id: id.to_string(),
            name: id.to_string(),
            expanded: true,
            nodes,
        }
    }

    /// Two verses / three petals, index rebuilt — mirrors post-`HierarchyLoaded` state.
    fn make_tree() -> VerseManager {
        let mut mgr = VerseManager {
            verses: vec![
                VerseEntry {
                    id: "verse-1".to_string(),
                    name: "Verse 1".to_string(),
                    namespace_id: None,
                    expanded: true,
                    fractals: vec![FractalEntry {
                        id: "fractal-1".to_string(),
                        name: "Fractal 1".to_string(),
                        expanded: true,
                        petals: vec![
                            petal("petal-1", vec![node("node-1")]),
                            petal("petal-2", vec![node("node-2"), node("node-3")]),
                        ],
                    }],
                },
                VerseEntry {
                    id: "verse-2".to_string(),
                    name: "Verse 2".to_string(),
                    namespace_id: None,
                    expanded: true,
                    fractals: vec![FractalEntry {
                        id: "fractal-2".to_string(),
                        name: "Fractal 2".to_string(),
                        expanded: true,
                        petals: vec![petal("petal-3", vec![node("node-4")])],
                    }],
                },
            ],
            ..Default::default()
        };
        mgr.rebuild_node_index();
        mgr
    }

    #[test]
    fn rebuild_indexes_every_node() {
        let mgr = make_tree();
        assert_eq!(mgr.node_index.len(), 4);
        assert_eq!(mgr.node_index.get("node-1"), Some(&(0, 0, 0)));
        assert_eq!(mgr.node_index.get("node-3"), Some(&(0, 0, 1)));
        assert_eq!(mgr.node_index.get("node-4"), Some(&(1, 0, 0)));
    }

    #[test]
    fn update_node_position_finds_correct_node() {
        let mut mgr = make_tree();
        mgr.update_node_position("node-4", [9.0, 8.0, 7.0]);
        let node = mgr.all_nodes().find(|n| n.id == "node-4").unwrap();
        assert_eq!(node.position, [9.0, 8.0, 7.0]);
    }

    #[test]
    fn update_node_position_noop_on_missing_id() {
        let mut mgr = make_tree();
        mgr.update_node_position("does-not-exist", [0.0, 0.0, 0.0]);
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert_eq!(
            node.position,
            [1.0, 2.0, 3.0],
            "existing node must be unaffected"
        );
        assert!(!mgr.node_index.contains_key("does-not-exist"));
    }

    #[test]
    fn update_node_url_sets_and_clears() {
        let mut mgr = make_tree();
        mgr.update_node_url("node-1", Some("https://example.com".to_string()));
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert_eq!(node.webpage_url, Some("https://example.com".to_string()));
        mgr.update_node_url("node-1", None);
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert!(node.webpage_url.is_none());
    }

    #[test]
    fn update_works_without_prebuilt_index_and_heals_it() {
        let mut mgr = make_tree();
        mgr.node_index.clear(); // simulate a tree mutated behind the index's back
        mgr.update_node_position("node-2", [4.0, 5.0, 6.0]);
        let node = mgr.all_nodes().find(|n| n.id == "node-2").unwrap();
        assert_eq!(node.position, [4.0, 5.0, 6.0]);
        assert_eq!(
            mgr.node_index.get("node-2"),
            Some(&(0, 0, 1)),
            "index must self-heal"
        );
    }

    #[test]
    fn stale_index_entry_self_heals() {
        let mut mgr = make_tree();
        mgr.node_index.insert("node-4".to_string(), (0, 0, 0)); // wrong petal
        mgr.update_node_url("node-4", Some("https://a.example".to_string()));
        let node = mgr.all_nodes().find(|n| n.id == "node-4").unwrap();
        assert_eq!(node.webpage_url, Some("https://a.example".to_string()));
        assert_eq!(mgr.node_index.get("node-4"), Some(&(1, 0, 0)));
    }

    #[test]
    fn add_node_indexes_new_node() {
        let mut mgr = make_tree();
        assert!(mgr.add_node("petal-3", node("node-5")));
        assert_eq!(mgr.node_index.get("node-5"), Some(&(1, 0, 0)));
        mgr.update_node_position("node-5", [7.0, 7.0, 7.0]);
        let n = mgr.all_nodes().find(|n| n.id == "node-5").unwrap();
        assert_eq!(n.position, [7.0, 7.0, 7.0]);
    }

    #[test]
    fn add_node_unknown_petal_is_rejected() {
        let mut mgr = make_tree();
        assert!(!mgr.add_node("no-such-petal", node("node-9")));
        assert!(!mgr.node_index.contains_key("node-9"));
        assert_eq!(mgr.all_nodes().count(), 4);
    }

    #[test]
    fn remove_node_drops_index_entry() {
        let mut mgr = make_tree();
        mgr.remove_node("petal-2", "node-2");
        assert!(!mgr.node_index.contains_key("node-2"));
        assert!(mgr.all_nodes().all(|n| n.id != "node-2"));
        // A later update on the removed id is a clean no-op.
        mgr.update_node_position("node-2", [0.0; 3]);
        assert!(!mgr.node_index.contains_key("node-2"));
    }

    #[test]
    fn rebuild_after_structural_delete_reassigns_indices() {
        let mut mgr = make_tree();
        mgr.verses.remove(0); // EntityDeleted(Verse) shifts every index
        mgr.rebuild_node_index();
        assert_eq!(mgr.node_index.len(), 1);
        assert_eq!(mgr.node_index.get("node-4"), Some(&(0, 0, 0)));
        mgr.update_node_position("node-4", [1.0, 1.0, 1.0]);
        assert_eq!(mgr.all_nodes().next().unwrap().position, [1.0, 1.0, 1.0]);
    }
}
