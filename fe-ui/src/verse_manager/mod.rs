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

pub(crate) mod db_results;
mod lifecycle_events;
mod node_index;
mod path_asset_materialize;
mod path_asset_reconcile;
pub(crate) use path_asset_reconcile::stamp_editor_defaults;
mod petal_respawn;
mod primitive_materialize;
mod primitive_reconcile;
mod residency_ledger;
mod scene_property_reads;
pub(crate) mod spawn;

use bevy::prelude::*;

use crate::plugin::UiSet;

pub use path_asset_materialize::{
    invalidate_path_stamp_projection, PathAssetApplied, PathAssetCache, StampRenderIndex,
    StampTrackRenderData,
};
// T4 right-click classification: the stamp marker-id format + the per-instance
// marker component, consumed by `node_manager::context_pick`.
/// Hard never-crash backstops that `settings::sanitize` clamps the soft caps to.
pub(crate) use path_asset_materialize::MAX_STAMPS_PER_PETAL;
pub(crate) use path_asset_materialize::{parse_stamp_marker_id, stamp_marker_id};
pub(crate) use path_asset_materialize::StampSelectionSource;
#[cfg(test)]
pub(crate) use path_asset_materialize::StampRenderSource;
pub use primitive_materialize::PrimitiveDescriptorCache;
pub use primitive_reconcile::PrimitiveMaterialAssets;
pub use residency_ledger::{radius_scale_for_pressure, ResidencyLedger};
pub(crate) use spawn::MAX_PETAL_NODES;
pub use spawn::{build_primitive_mesh, PathAssetInstance, PrimitiveNode, ResidentNode};

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
    /// Durable per-petal residency horizon (p2p_asset_streaming S6);
    /// `None` = inherit the global `AppSettings.render_distance`. See
    /// `fe-ui/src/AGENTS.md` §app-settings.
    pub render_distance: Option<f32>,
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
    pub transform: Transform,
    pub webpage_url: Option<String>,
    /// Path to the GLTF/GLB asset, if any.  Drives scene-entity materialisation.
    pub asset_path: Option<String>,
}

impl NodeEntry {
    /// Complete scene transform, using the authoritative tree position.
    pub(crate) fn world_transform(&self) -> Transform {
        Transform {
            translation: Vec3::from_array(self.position),
            ..self.transform
        }
    }
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
    pub(crate) committed_versions: std::collections::HashMap<String, i64>,
    pub(crate) node_cache_epochs: std::collections::HashMap<String, u64>,
    pub(crate) next_node_cache_epoch: u64,
    pub(crate) url_asset_ids: std::collections::HashMap<String, Option<String>>,
    pub(crate) url_storage: std::collections::HashMap<String, fe_runtime::messages::NodeUrlStorage>,
}

impl VerseManager {
    pub fn clear_for_terminal_reset(&mut self) { *self = Self::default(); }

    /// Build a manager from a verse tree with the node index pre-populated.
    pub fn from_verses(verses: Vec<VerseEntry>) -> Self {
        let mut mgr = Self {
            verses,
            node_index: Default::default(),
            committed_versions: Default::default(),
            node_cache_epochs: Default::default(),
            next_node_cache_epoch: 0,
            url_asset_ids: Default::default(),
            url_storage: Default::default(),
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

    /// Updates a petal's in-memory `render_distance` from a
    /// `PetalRenderDistanceLoaded` echo (S6) — no-op if the petal is not (yet)
    /// in the tree.
    pub(super) fn update_petal_render_distance(
        &mut self,
        petal_id: &str,
        render_distance: Option<f32>,
    ) {
        if let Some(petal) = self.find_petal_mut(petal_id) {
            petal.render_distance = render_distance;
        }
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
        app.init_resource::<crate::undo::UndoState>();
        app.init_resource::<TextureRegistryRes>();
        app.init_resource::<PrimitiveMaterialAssets>();
        app.init_resource::<PathAssetApplied>();
        app.init_resource::<PathAssetCache>();
        app.init_resource::<StampRenderIndex>();
        app.init_resource::<PrimitiveDescriptorCache>();
        app.init_resource::<ResidencyLedger>();
        // Idempotent with fe-runtime `setup_core_systems` (same add-twice-is-ok
        // idiom as BrowserCommand) — keeps fe-ui-only test apps runnable.
        app.add_message::<fe_runtime::messages::LifecycleEvent>();
        // Chained: materialize must observe entities spawned via Commands by
        // the earlier systems (chain inserts the needed sync points).
        // `apply_lifecycle_events` runs first so a PathReflow's invalidation
        // lands before the same-frame materializer pass.
        // `reconcile_path_asset` only FEEDS the path-asset cache; the single
        // `materialize_path_assets` (last) does all stamp spawning so no
        // double-stamping is possible. See AGENTS.md §path-asset-materialization.
        // D-74: the residency ledger sees this frame's tree and runs ahead of
        // every spawner; `apply_node_residency` streams nodes in/out after the
        // petal-entry batch. See AGENTS.md §residency-ledger.
        app.add_systems(
            Update,
            (
                crate::undo::observe_results,
                lifecycle_events::apply_lifecycle_events,
                db_results::apply_db_results,
                scene_property_reads::flush_scene_property_reads,
                petal_respawn::remove_standalone_stamp_projection,
                residency_ledger::update_residency_ledger,
                petal_respawn::respawn_on_petal_change,
                residency_ledger::apply_node_residency,
                primitive_materialize::materialize_cached_primitives,
                primitive_reconcile::reconcile_selected_primitive,
                path_asset_reconcile::reconcile_path_asset,
                path_asset_materialize::materialize_path_assets,
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
            transform: Default::default(),
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
            render_distance: None,
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
            render_distance: None,
            nodes: vec![],
        });

        mgr.verses[0].fractals[0].petals[0].nodes.push(NodeEntry {
            transform: Default::default(),
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
