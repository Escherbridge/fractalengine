//! O(1) node lookup index for `VerseManager` — see AGENTS.md §node-index.

use super::{NodeEntry, VerseManager};

impl VerseManager {
    /// Replace a current node snapshot without duplicating or retaining an old parent entry.
    pub(crate) fn upsert_node(&mut self, petal_id: &str, node: NodeEntry) -> bool {
        if self.find_petal(petal_id).is_none() {
            return false;
        }
        if let Some(previous) = self.petal_id_of(&node.id).map(str::to_owned) {
            self.remove_node(&previous, &node.id);
        }
        self.add_node(petal_id, node)
    }

    /// Retain all transform components for later petal and residency respawns.
    pub(crate) fn update_node_transform(
        &mut self,
        node_id: &str,
        transform: bevy::prelude::Transform,
    ) {
        if let Some(node) = self.node_entry_mut(node_id) {
            node.position = transform.translation.to_array();
            node.transform = transform;
        }
    }
    /// Rebuild the node_id → tree-indices map from scratch.
    pub(crate) fn rebuild_node_index(&mut self) {
        self.node_index.clear();
        self.node_cache_epochs.clear();
        self.url_asset_ids.clear();
        for (vi, verse) in self.verses.iter().enumerate() {
            for (fi, fractal) in verse.fractals.iter().enumerate() {
                for (pi, petal) in fractal.petals.iter().enumerate() {
                    for node in &petal.nodes {
                        self.node_index.insert(node.id.clone(), (vi, fi, pi));
                        if let Some(epoch) = self.next_node_cache_epoch.checked_add(1) {
                            self.next_node_cache_epoch = epoch;
                            self.node_cache_epochs.insert(node.id.clone(), epoch);
                        }
                    }
                }
            }
        }
    }

    /// The id of the petal containing `node_id`, via the index (with a
    /// self-heal-less full-walk fallback for a stale/missing entry). Read-only —
    /// used to tag a node's `path_asset` cache entry with its owning petal (see
    /// `path_asset_materialize`). `None` when the node isn't in the tree.
    pub(crate) fn petal_id_of(&self, node_id: &str) -> Option<&str> {
        if let Some(&(vi, fi, pi)) = self.node_index.get(node_id) {
            if let Some(petal) = self
                .verses
                .get(vi)
                .and_then(|v| v.fractals.get(fi))
                .and_then(|f| f.petals.get(pi))
            {
                if petal.nodes.iter().any(|n| n.id == node_id) {
                    return Some(&petal.id);
                }
            }
        }
        self.verses
            .iter()
            .flat_map(|v| &v.fractals)
            .flat_map(|f| &f.petals)
            .find(|p| p.nodes.iter().any(|n| n.id == node_id))
            .map(|p| p.id.as_str())
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
        self.url_asset_ids.remove(&node.id);
        if let Some(epoch) = self.next_node_cache_epoch.checked_add(1) {
            self.next_node_cache_epoch = epoch;
            self.node_cache_epochs.insert(node.id.clone(), epoch);
        } else {
            self.node_cache_epochs.remove(&node.id);
        }
        petal.nodes.push(node);
        true
    }

    /// Remove a node from a petal (no-op on unknown petal) and drop its index entry.
    pub(crate) fn remove_node(&mut self, petal_id: &str, node_id: &str) {
        if let Some(petal) = self.find_petal_mut(petal_id) {
            petal.nodes.retain(|n| n.id != node_id);
        }
        self.node_index.remove(node_id);
        self.committed_versions.remove(node_id);
        self.node_cache_epochs.remove(node_id);
        self.url_asset_ids.remove(node_id);
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

    /// Retain the actual model identity proven by an authorized current URL read.
    pub(crate) fn confirm_node_url(
        &mut self,
        node_id: &str,
        url: Option<String>,
        asset_id: Option<String>,
    ) {
        if self.petal_id_of(node_id).is_some() {
            self.update_node_url(node_id, url);
            self.url_asset_ids.insert(node_id.to_owned(), asset_id);
        }
    }

    /// Update display name of a node by its ID (`DbResult::NodeRenamed`).
    pub fn update_node_name(&mut self, node_id: &str, name: &str) {
        if let Some(node) = self.node_entry_mut(node_id) {
            node.name = name.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};

    fn node(id: &str) -> NodeEntry {
        NodeEntry {
            transform: Default::default(),
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
            render_distance: None,
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
    fn node_url_receipts_refresh_current_cache_and_preserve_drafts() {
        let raw = "http://127.1:49152/web/one";
        let command: fe_webview::ipc::BrowserCommand = serde_json::from_value(serde_json::json!({ "cmd": "navigate", "url": raw })).unwrap();
        let fe_webview::ipc::BrowserCommand::Navigate { url } = command else { panic!("Navigate command"); };
        assert_eq!(url, raw, "the command boundary must preserve alias spelling before policy validation");
        assert!(!fe_runtime::portal_url::embedded_allowed(&url));
        let mut mgr = make_tree();
        mgr.update_node_url("node-1", Some("https://example.com".to_string()));
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert_eq!(node.webpage_url, Some("https://example.com".to_string()));
        mgr.update_node_url("node-1", None);
        let node = mgr.all_nodes().find(|n| n.id == "node-1").unwrap();
        assert!(node.webpage_url.is_none());
        use crate::actions::{portal, url_read_model, UiManager};
        use fe_runtime::messages::{DbCommand, DbResult, NodeUrlSnapshot};
        let mut nav = crate::navigation_manager::NavigationManager::default();
        nav.navigate_to_petal_in_fractal("verse-1", "V", "fractal-1", "F", "petal-1");
        let mut nodes = crate::node_manager::NodeManager::default();
        nodes.select(bevy::prelude::Entity::from_bits(1), "node-1");
        let role = crate::plugin::LocalUserRole {
            role: Some(fe_policy::RoleLevel::Editor),
        };
        let mut ui = UiManager::default();
        let mut inspector = crate::plugin::InspectorFormState::default();
        let (tx, rx) = crossbeam::channel::bounded(16);
        let ack = |command| {
            let DbCommand::UpdateNodeUrl {
                node_id,
                url,
                request_id,
                expected_scope,
            } = command
            else {
                panic!("write request")
            };
            DbResult::NodeUrlUpdated {
                node_id,
                request_id,
                expected_scope,
                outcome: Ok(url),
                asset_id: Some("asset-a".into()),
            }
        };
        let read = |command, url: &str| {
            let DbCommand::ReadNodeUrl {
                node_id,
                request_id,
                expected_scope,
                ..
            } = command
            else {
                panic!("typed read request")
            };
            DbResult::NodeUrlRead {
                node_id: node_id.clone(),
                request_id,
                expected_scope: expected_scope.clone(),
                outcome: Ok(NodeUrlSnapshot {
                    node_id,
                    scope: expected_scope,
                    asset_id: Some("asset-a".into()),
                    url: Some(url.into()),
                }),
            }
        };
        inspector.external_url = "https://example.com/saved".into();
        portal::save_url(
            &tx,
            &mut ui,
            &mgr,
            &nav,
            &nodes,
            &role,
            "node-1".into(),
            Some(inspector.external_url.clone()),
            0.0,
        );
        assert!(ui.active_toast(0.0).unwrap().0.starts_with("Saving URL"));
        let saved = ack(rx.try_recv().unwrap());
        nodes.select(bevy::prelude::Entity::from_bits(2), "node-2");
        nav.navigate_to_petal("petal-2");
        inspector.external_url = "https://example.com/draft-b".into();
        url_read_model::maintain(&tx, &mut ui, &mgr, &nav, &nodes, &inspector, 1.0);
        assert!(ui.pending_url_save.is_none());
        portal::finish_url_save(
            &mut ui, &mut mgr, &nav, &nodes, &saved, 2.0, &tx, &inspector,
        );
        assert!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .is_none(),
            "write ack alone never hydrates the cache"
        );
        let current = read(rx.try_recv().unwrap(), "https://example.com/saved");
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &current,
            3.0,
            &tx,
        );
        assert_eq!(
            inspector.external_url, "https://example.com/draft-b",
            "A read cannot alter B's draft"
        );
        nodes.select(bevy::prelude::Entity::from_bits(1), "node-1");
        nav.navigate_to_petal("petal-1");
        inspector.external_url = mgr
            .all_nodes()
            .find(|n| n.id == "node-1")
            .unwrap()
            .webpage_url
            .clone()
            .unwrap();
        assert_eq!(
            inspector.external_url, "https://example.com/saved",
            "reselect A sees its committed current URL"
        );
        url_read_model::request_current(
            &tx, &mut ui, &mgr, &nav, &nodes, &inspector, "node-1", None, 4.0,
        )
        .unwrap();
        let clean_read = read(
            rx.try_recv().unwrap(),
            "https://example.com/external-change",
        );
        inspector.external_url = "a newer unsaved draft".into();
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &clean_read,
            5.0,
            &tx,
        );
        assert_eq!(inspector.external_url, "a newer unsaved draft");
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/external-change")
        );
        inspector.external_url = "https://example.com/timed-out".into();
        portal::save_url(
            &tx,
            &mut ui,
            &mgr,
            &nav,
            &nodes,
            &role,
            "node-1".into(),
            Some(inspector.external_url.clone()),
            6.0,
        );
        let old_ack = ack(rx.try_recv().unwrap());
        url_read_model::maintain(&tx, &mut ui, &mgr, &nav, &nodes, &inspector, 37.0);
        assert!(ui.pending_url_save.is_none());
        let old_read = read(rx.try_recv().unwrap(), "https://example.com/timed-out");
        inspector.external_url = "https://example.com/newest".into();
        portal::save_url(
            &tx,
            &mut ui,
            &mgr,
            &nav,
            &nodes,
            &role,
            "node-1".into(),
            Some(inspector.external_url.clone()),
            38.0,
        );
        let newest_ack = ack(rx.try_recv().unwrap());
        portal::finish_url_save(
            &mut ui, &mut mgr, &nav, &nodes, &old_ack, 39.0, &tx, &inspector,
        );
        assert!(
            rx.try_recv().is_err(),
            "an older write cannot schedule over the newest write owner"
        );
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &old_read,
            39.0,
            &tx,
        );
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/external-change")
        );
        inspector.external_url = "yet another unsaved draft".into();
        portal::finish_url_save(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &newest_ack,
            50.0,
            &tx,
            &inspector,
        );
        assert!(ui
            .active_toast(50.0)
            .unwrap()
            .0
            .contains("newer draft is still unsaved"));
        let newest_read = read(rx.try_recv().unwrap(), "https://example.com/newest");
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &newest_read,
            51.0,
            &tx,
        );
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/newest")
        );
        assert_eq!(inspector.external_url, "yet another unsaved draft");
        url_read_model::request_current(
            &tx,
            &mut ui,
            &mgr,
            &nav,
            &nodes,
            &inspector,
            "node-1",
            Some("asset-a".into()),
            52.0,
        )
        .unwrap();
        let owned_read = read(rx.try_recv().unwrap(), "https://example.com/wrong-asset");
        let mut foreign = owned_read.clone();
        if let DbResult::NodeUrlRead { request_id, .. } = &mut foreign {
            request_id.push('x');
        }
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &foreign,
            53.0,
            &tx,
        );
        let mut foreign_asset = owned_read;
        if let DbResult::NodeUrlRead {
            outcome: Ok(snapshot),
            ..
        } = &mut foreign_asset
        {
            snapshot.asset_id = Some("asset-b".into());
        }
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &foreign_asset,
            53.0,
            &tx,
        );
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/newest")
        );
        url_read_model::request_current(
            &tx, &mut ui, &mgr, &nav, &nodes, &inspector, "node-1", None, 54.0,
        )
        .unwrap();
        let before_change = read(rx.try_recv().unwrap(), "https://example.com/old-model");
        mgr.find_petal_mut("petal-1").unwrap().nodes[0].asset_path =
            Some("models/replaced.glb".into());
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &before_change,
            55.0,
            &tx,
        );
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/newest")
        );
        let replacement = read(rx.try_recv().unwrap(), "https://example.com/current-model");
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &replacement,
            56.0,
            &tx,
        );
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/current-model")
        );
        assert_eq!(inspector.external_url, "yet another unsaved draft");
        url_read_model::request_current(
            &tx, &mut ui, &mgr, &nav, &nodes, &inspector, "node-1", None, 57.0,
        )
        .unwrap();
        let before_rebuild = read(
            rx.try_recv().unwrap(),
            "https://example.com/obsolete-incarnation",
        );
        mgr.rebuild_node_index();
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &before_rebuild,
            58.0,
            &tx,
        );
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/current-model")
        );
        let after_rebuild = read(
            rx.try_recv().unwrap(),
            "https://example.com/refreshed-incarnation",
        );
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &after_rebuild,
            59.0,
            &tx,
        );
        assert_eq!(
            mgr.all_nodes()
                .find(|n| n.id == "node-1")
                .unwrap()
                .webpage_url
                .as_deref(),
            Some("https://example.com/refreshed-incarnation")
        );
        let previous = mgr
            .all_nodes()
            .find(|n| n.id == "node-1")
            .unwrap()
            .webpage_url
            .clone();
        inspector.external_url = previous.clone().unwrap();
        let mut replacement = mgr.all_nodes().find(|n| n.id == "node-1").unwrap().clone();
        replacement.webpage_url = None;
        replacement.asset_path = Some("models/new-model.glb".into());
        assert!(mgr.upsert_node("petal-1", replacement));
        url_read_model::request_after_model_change(
            &tx,
            &mut ui,
            &mgr,
            &nav,
            &nodes,
            &inspector,
            "node-1",
            Some("asset-b".into()),
            previous.as_deref(),
            60.0,
        )
        .unwrap();
        let mut replacement_url = read(rx.try_recv().unwrap(), "https://example.com/new-model");
        if let DbResult::NodeUrlRead {
            outcome: Ok(snapshot),
            ..
        } = &mut replacement_url
        {
            snapshot.asset_id = Some("asset-b".into());
        }
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &replacement_url,
            61.0,
            &tx,
        );
        assert_eq!(
            inspector.external_url, "https://example.com/new-model",
            "an unchanged clean form follows the new model's authoritative URL"
        );
        assert_eq!(
            mgr.url_asset_ids
                .get("node-1")
                .and_then(|asset| asset.as_deref()),
            Some("asset-b")
        );
        url_read_model::request_current(
            &tx,
            &mut ui,
            &mgr,
            &nav,
            &nodes,
            &inspector,
            "node-1",
            Some("asset-b".into()),
            62.0,
        )
        .unwrap();
        let DbCommand::ReadNodeUrl {
            node_id,
            expected_scope,
            request_id,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("anchored read")
        };
        let rebound = DbResult::NodeUrlRead {
            node_id,
            expected_scope,
            request_id,
            outcome: Err("model changed".into()),
        };
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &rebound,
            63.0,
            &tx,
        );
        let recovery_command = rx.try_recv().unwrap();
        assert!(
            matches!(
                &recovery_command,
                DbCommand::ReadNodeUrl {
                    expected_asset_id: None,
                    ..
                }
            ),
            "recovery is another actual current Viewer read, never a hierarchy/API fallback"
        );
        let mut recovered = read(recovery_command, "https://example.com/rebound-current");
        if let DbResult::NodeUrlRead {
            outcome: Ok(snapshot),
            ..
        } = &mut recovered
        {
            snapshot.asset_id = Some("asset-c".into());
        }
        url_read_model::finish_read(
            &mut ui,
            &mut mgr,
            &nav,
            &nodes,
            &mut inspector,
            &recovered,
            64.0,
            &tx,
        );
        assert_eq!(
            inspector.external_url,
            "https://example.com/rebound-current"
        );
    }

    #[test]
    fn update_node_name_renames_and_ignores_missing() {
        let mut mgr = make_tree();
        mgr.update_node_name("node-3", "Renamed");
        assert_eq!(
            mgr.all_nodes().find(|n| n.id == "node-3").unwrap().name,
            "Renamed"
        );
        mgr.update_node_name("missing", "X");
        assert!(mgr.all_nodes().all(|n| n.name != "X"));
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
        let rotation = bevy::prelude::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let transform = bevy::prelude::Transform {
            translation: bevy::prelude::Vec3::new(3.0, 0.0, 4.0),
            rotation,
            scale: bevy::prelude::Vec3::new(2.0, 1.0, 0.5),
        };
        mgr.update_node_transform("node-5", transform);
        let snapshot = mgr
            .all_nodes()
            .find(|node| node.id == "node-5")
            .unwrap()
            .clone();
        assert_eq!(snapshot.world_transform(), transform);
        assert!(mgr.upsert_node("petal-2", snapshot.clone()));
        assert!(mgr.upsert_node("petal-2", snapshot));
        assert_eq!(
            mgr.all_nodes().filter(|node| node.id == "node-5").count(),
            1
        );
        assert_eq!(mgr.petal_id_of("node-5"), Some("petal-2"));
        assert_eq!(
            mgr.find_petal("petal-2")
                .unwrap()
                .nodes
                .iter()
                .find(|node| node.id == "node-5")
                .unwrap()
                .world_transform(),
            transform
        );
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
    fn petal_id_of_resolves_via_index_and_walk_fallback() {
        let mut mgr = make_tree();
        assert_eq!(mgr.petal_id_of("node-1"), Some("petal-1"));
        assert_eq!(mgr.petal_id_of("node-3"), Some("petal-2"));
        assert_eq!(mgr.petal_id_of("node-4"), Some("petal-3"));
        assert_eq!(mgr.petal_id_of("missing"), None);
        // A cleared/stale index still resolves via the full-walk fallback.
        mgr.node_index.clear();
        assert_eq!(mgr.petal_id_of("node-2"), Some("petal-2"));
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
