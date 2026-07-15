//! VerseManager — owns the in-memory verse/fractal/petal/node hierarchy and
//! drives scene-entity spawn/despawn when the active petal changes.
//!
//! Replaces the old `VerseHierarchy` resource and the `apply_db_results_to_hierarchy`
//! + `despawn_and_spawn_on_petal_change` systems in plugin.rs.
//!
//! Key improvement over the old code: petal navigation no longer round-trips
//! through `DbCommand::LoadHierarchy`.  Because `NodeEntry` now stores
//! `asset_path`, the manager can directly spawn entities from its in-memory
//! tree when `NavigationManager::active_petal_id` changes.
//!
//! See `fe-ui/src/verse_manager/AGENTS.md` for the submodule map.

mod db_results;
mod node_index;
mod path_asset_reconcile;
mod petal_respawn;
mod primitive_materialize;
mod primitive_reconcile;
mod spawn;

use bevy::prelude::*;

use crate::plugin::UiSet;

pub use path_asset_reconcile::PathAssetApplied;
pub use primitive_materialize::PrimitiveDescriptorCache;
pub use primitive_reconcile::PrimitiveMaterialAssets;
pub use spawn::{build_primitive_mesh, PrimitiveNode};

// ---------------------------------------------------------------------------
// Hierarchy tree types
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct VerseEntry {
    pub id: String,
    pub name: String,
    /// iroh-docs namespace ID used to open the P2P replica.
    pub namespace_id: Option<String>,
    pub expanded: bool,
    pub fractals: Vec<FractalEntry>,
}

#[derive(Clone, Default)]
pub struct FractalEntry {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub petals: Vec<PetalEntry>,
}

#[derive(Clone, Default)]
pub struct PetalEntry {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub nodes: Vec<NodeEntry>,
}

/// In-memory node record.  `asset_path` is retained (previously dropped) so
/// that the manager can respawn entities on petal switch without a DB round-trip.
#[derive(Clone, Default)]
pub struct NodeEntry {
    pub id: String,
    pub name: String,
    pub has_asset: bool,
    pub position: [f32; 3],
    pub webpage_url: Option<String>,
    /// Path to the GLTF/GLB asset, if any.  Drives scene-entity materialisation.
    pub asset_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

/// Single source of truth for the verse/fractal/petal/node tree.
#[derive(Resource, Default)]
pub struct VerseManager {
    pub verses: Vec<VerseEntry>,
    /// node_id → (verse, fractal, petal) indices; see `node_index.rs` + AGENTS.md §node-index.
    pub(crate) node_index: std::collections::HashMap<String, (usize, usize, usize)>,
}

impl VerseManager {
    /// Build a manager from a verse tree with the node index pre-populated.
    pub fn from_verses(verses: Vec<VerseEntry>) -> Self {
        let mut mgr = Self {
            verses,
            node_index: Default::default(),
        };
        mgr.rebuild_node_index();
        mgr
    }

    /// Iterate every node in every petal of every fractal in every verse.
    pub fn all_nodes(&self) -> impl Iterator<Item = &NodeEntry> {
        self.verses
            .iter()
            .flat_map(|v| &v.fractals)
            .flat_map(|f| &f.petals)
            .flat_map(|p| &p.nodes)
    }

    /// Find a petal by ID (immutable).
    pub fn find_petal<'a>(&'a self, petal_id: &str) -> Option<&'a PetalEntry> {
        self.verses
            .iter()
            .flat_map(|v| &v.fractals)
            .flat_map(|f| &f.petals)
            .find(|p| p.id == petal_id)
    }

    pub fn find_verse_mut(&mut self, id: &str) -> Option<&mut VerseEntry> {
        self.verses.iter_mut().find(|v| v.id == id)
    }

    fn find_petal_mut(&mut self, petal_id: &str) -> Option<&mut PetalEntry> {
        self.verses
            .iter_mut()
            .flat_map(|v| &mut v.fractals)
            .flat_map(|f| &mut f.petals)
            .find(|p| p.id == petal_id)
    }
}

/// Bevy [`Resource`] wrapper around [`fe_sdk::TextureRegistry`] (FR-4) — the
/// SDK type stays engine-decoupled (no bevy dep), so the ECS registration
/// lives here.
#[derive(Resource, Default)]
pub struct TextureRegistryRes(pub fe_sdk::TextureRegistry);

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct VerseManagerPlugin;

impl Plugin for VerseManagerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VerseManager>();
        app.init_resource::<TextureRegistryRes>();
        app.init_resource::<PrimitiveMaterialAssets>();
        app.init_resource::<PathAssetApplied>();
        app.init_resource::<PrimitiveDescriptorCache>();
        // Chained: materialize must observe entities spawned via Commands by
        // the earlier systems (chain inserts the needed sync points).
        app.add_systems(
            Update,
            (
                db_results::apply_db_results,
                petal_respawn::respawn_on_petal_change,
                primitive_materialize::materialize_cached_primitives,
                primitive_reconcile::reconcile_selected_primitive,
                path_asset_reconcile::reconcile_path_asset,
            )
                .chain()
                .before(UiSet::ProcessActions),
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree() -> VerseManager {
        let node = NodeEntry {
            id: "node-1".to_string(),
            name: "Node 1".to_string(),
            has_asset: true,
            position: [1.0, 2.0, 3.0],
            webpage_url: None,
            asset_path: Some("models/cube.glb".to_string()),
        };
        let petal = PetalEntry {
            id: "petal-1".to_string(),
            name: "Petal 1".to_string(),
            expanded: true,
            nodes: vec![node],
        };
        let fractal = FractalEntry {
            id: "fractal-1".to_string(),
            name: "Fractal 1".to_string(),
            expanded: true,
            petals: vec![petal],
        };
        let verse = VerseEntry {
            id: "verse-1".to_string(),
            name: "Verse 1".to_string(),
            namespace_id: None,
            expanded: true,
            fractals: vec![fractal],
        };
        VerseManager {
            verses: vec![verse],
            ..Default::default()
        }
    }

    #[test]
    fn find_petal_returns_correct_petal() {
        let mgr = make_tree();
        let petal = mgr.find_petal("petal-1");
        assert!(petal.is_some());
        assert_eq!(petal.unwrap().id, "petal-1");
        assert_eq!(petal.unwrap().nodes.len(), 1);
    }

    #[test]
    fn find_petal_returns_none_for_missing_id() {
        let mgr = make_tree();
        assert!(mgr.find_petal("does-not-exist").is_none());
    }

    #[test]
    fn add_verse_fractal_petal_node_chain() {
        let mut mgr = VerseManager::default();
        assert!(mgr.verses.is_empty());

        mgr.verses.push(VerseEntry {
            id: "v1".to_string(),
            name: "V1".to_string(),
            namespace_id: None,
            expanded: true,
            fractals: vec![],
        });
        assert_eq!(mgr.verses.len(), 1);

        mgr.find_verse_mut("v1")
            .unwrap()
            .fractals
            .push(FractalEntry {
                id: "f1".to_string(),
                name: "F1".to_string(),
                expanded: true,
                petals: vec![],
            });
        assert_eq!(mgr.verses[0].fractals.len(), 1);

        mgr.verses[0].fractals[0].petals.push(PetalEntry {
            id: "p1".to_string(),
            name: "P1".to_string(),
            expanded: true,
            nodes: vec![],
        });

        mgr.verses[0].fractals[0].petals[0].nodes.push(NodeEntry {
            id: "n1".to_string(),
            name: "N1".to_string(),
            has_asset: false,
            position: [0.0; 3],
            webpage_url: None,
            asset_path: None,
        });

        assert!(mgr.find_petal("p1").is_some());
        assert_eq!(mgr.all_nodes().count(), 1);
    }
}
