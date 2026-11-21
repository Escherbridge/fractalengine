//! Terrain snapshot the Bevy side publishes for the extension `terrain` import, read on the plugin worker without
//! touching ECS; see src/AGENTS.md §extension-terrain.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use bevy::prelude::{Local, MessageWriter, Res, Resource};
use bevy::window::RequestRedraw;
use fe_plugin::wasm::component::terrain::{surface_at, TerrainPoint, TerrainSource, SLOPE_STEP_M};
use fe_renderer::terrain_height::TerrainHeightField;
use fe_terrain::petal_binding::{ActivePetalTerrain, ActiveTileSource};
use fe_ui::navigation_manager::NavigationManager;

/// Shortest spacing between two snapshot clones while tiles stream in.
pub(super) const SNAPSHOT_DEBOUNCE: Duration = Duration::from_millis(250);
/// Refusal for a binding whose petal is not the one loaded here.
const NOT_RESIDENT: &str = "terrain of this petal is not resident on this host";

/// One published copy of the active petal's resident height field.
pub(super) struct TerrainSnapshot {
    verse_id: String,
    petal_id: String,
    /// Publish counter: the `generation` guests see.
    sequence: u64,
    field: TerrainHeightField,
    world_scale: f64,
    origin_ele: f64,
}

/// The latest snapshot plus how many terrain-granted instances are alive (the publisher idles at zero).
#[derive(Resource, Clone, Default)]
pub(super) struct TerrainSnapshots {
    slot: Arc<RwLock<Option<Arc<TerrainSnapshot>>>>,
    wanted: Arc<AtomicUsize>,
}

impl TerrainSnapshots {
    /// A source for one instance; it counts as wanted until the instance drops it.
    pub(super) fn source(&self) -> Arc<dyn TerrainSource> {
        self.wanted.fetch_add(1, Ordering::AcqRel);
        Arc::new(BridgeTerrainSource { snapshots: self.clone() })
    }

    fn wanted(&self) -> bool {
        self.wanted.load(Ordering::Acquire) > 0
    }

    fn current(&self) -> Option<Arc<TerrainSnapshot>> {
        self.slot.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn replace(&self, next: Option<Arc<TerrainSnapshot>>) {
        *self.slot.write().unwrap_or_else(PoisonError::into_inner) = next;
    }
}

/// The instance-side reader: refuses any verse or petal but the published one.
struct BridgeTerrainSource {
    snapshots: TerrainSnapshots,
}

impl Drop for BridgeTerrainSource {
    fn drop(&mut self) {
        self.snapshots.wanted.fetch_sub(1, Ordering::AcqRel);
    }
}

impl TerrainSource for BridgeTerrainSource {
    fn sample(
        &self,
        verse_id: &str,
        petal_id: &str,
        points: &[(f64, f64)],
        out: &mut Vec<Option<TerrainPoint>>,
    ) -> Result<u64, String> {
        let snapshot = self
            .snapshots
            .current()
            .filter(|snapshot| snapshot.verse_id == verse_id && snapshot.petal_id == petal_id)
            .ok_or_else(|| NOT_RESIDENT.to_string())?;
        let step = SLOPE_STEP_M * snapshot.world_scale;
        let height = |x: f64, z: f64| snapshot.field.height_at(x as f32, z as f32).map(f64::from);
        out.clear();
        out.extend(
            points
                .iter()
                .map(|&(x, z)| surface_at(&height, x, z, step, snapshot.world_scale, snapshot.origin_ele)),
        );
        Ok(snapshot.sequence)
    }
}

/// What the publisher reads this frame: the loaded petal's terrain, borrowed (no allocation).
pub(super) struct TerrainView<'a> {
    pub(super) verse_id: &'a str,
    pub(super) petal_id: &'a str,
    pub(super) field: &'a TerrainHeightField,
    /// `ActivePetalTerrain::revision` (bumped on every applied assignment).
    pub(super) revision: u64,
    pub(super) world_scale: f64,
    pub(super) origin_ele: f64,
}

/// What the last publish was built from; any difference publishes again.
struct PublishedKey {
    verse_id: String,
    petal_id: String,
    field_generation: u64,
    revision: u64,
    world_scale: f64,
    origin_ele: f64,
}

impl PublishedKey {
    fn same_petal(&self, view: &TerrainView<'_>) -> bool {
        self.verse_id == view.verse_id && self.petal_id == view.petal_id
    }

    fn matches(&self, view: &TerrainView<'_>) -> bool {
        self.same_petal(view)
            && self.field_generation == view.field.generation()
            && self.revision == view.revision
            && self.world_scale.to_bits() == view.world_scale.to_bits()
            && self.origin_ele.to_bits() == view.origin_ele.to_bits()
    }
}

/// The publisher's memory between frames.
#[derive(Default)]
pub(super) struct PublishState {
    key: Option<PublishedKey>,
    at: Option<Instant>,
    sequence: u64,
}

/// One publisher step: idle and allocation-free unless an instance wants terrain and the view changed; a petal
/// switch withdraws the old snapshot at once, a refresh of a live snapshot is cloned at most every
/// [`SNAPSHOT_DEBOUNCE`]. Returns `true` while a debounced publish is still pending.
pub(super) fn publish_step(
    snapshots: &TerrainSnapshots,
    view: Option<TerrainView<'_>>,
    last: &mut PublishState,
    now: Instant,
) -> bool {
    let view = match view {
        Some(view) if snapshots.wanted() => view,
        _ => {
            if last.key.take().is_some() {
                snapshots.replace(None);
            }
            return false;
        }
    };
    if last.key.as_ref().is_some_and(|key| key.matches(&view)) {
        return false;
    }
    if last.key.as_ref().is_some_and(|key| !key.same_petal(&view)) {
        last.key = None;
        snapshots.replace(None);
    }
    // Only a refresh of a live snapshot waits; the first publish after a switch or withdrawal is immediate.
    if last.key.is_some() && last.at.is_some_and(|at| now.saturating_duration_since(at) < SNAPSHOT_DEBOUNCE) {
        return true;
    }
    last.sequence += 1;
    last.at = Some(now);
    last.key = Some(PublishedKey {
        verse_id: view.verse_id.to_string(),
        petal_id: view.petal_id.to_string(),
        field_generation: view.field.generation(),
        revision: view.revision,
        world_scale: view.world_scale,
        origin_ele: view.origin_ele,
    });
    snapshots.replace(Some(Arc::new(TerrainSnapshot {
        verse_id: view.verse_id.to_string(),
        petal_id: view.petal_id.to_string(),
        sequence: last.sequence,
        field: view.field.clone(),
        world_scale: view.world_scale,
        origin_ele: view.origin_ele,
    })));
    false
}

/// The loaded petal's terrain, when navigation, the applied assignment and the projection all agree on it.
fn terrain_view<'a>(
    nav: &'a NavigationManager,
    active: &'a ActivePetalTerrain,
    tiles: &'a ActiveTileSource,
    field: &'a TerrainHeightField,
) -> Option<TerrainView<'a>> {
    let petal_id = active.petal_id.as_deref()?;
    if nav.active_petal_id.as_deref() != Some(petal_id) {
        return None;
    }
    Some(TerrainView {
        verse_id: nav.active_verse_id.as_deref()?,
        petal_id,
        field,
        revision: active.revision,
        world_scale: active.config.as_ref()?.effective_world_scale(),
        origin_ele: tiles.projection.as_ref()?.origin_ele,
    })
}

/// Bevy system: publish the active petal's height field for terrain-granted instances.
pub(super) fn publish_terrain_snapshot(
    snapshots: Res<TerrainSnapshots>,
    nav: Option<Res<NavigationManager>>,
    active: Option<Res<ActivePetalTerrain>>,
    tiles: Option<Res<ActiveTileSource>>,
    field: Option<Res<TerrainHeightField>>,
    mut last: Local<PublishState>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    if !snapshots.wanted() && last.key.is_none() {
        return;
    }
    let view = match (nav.as_deref(), active.as_deref(), tiles.as_deref(), field.as_deref()) {
        (Some(nav), Some(active), Some(tiles), Some(field)) => terrain_view(nav, active, tiles, field),
        _ => None,
    };
    // Trailing edge: keep the reactive loop ticking until the held-back publish lands.
    if publish_step(&snapshots, view, &mut last, Instant::now()) {
        redraw.write(RequestRedraw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Vec3;
    use fe_renderer::terrain_height::HeightTile;

    /// A 100 m tile at the origin rising 1 m per 10 m east, at `zoom`.
    fn east_ramp(field: &mut TerrainHeightField, zoom: u8, lift: f32) {
        let tile = HeightTile { anchor: Vec3::new(0.0, lift, 0.0), tile_size: 100.0, grid: vec![0.0, 10.0, 0.0, 10.0], width: 2, height: 2 };
        field.insert((zoom, 0, 0), tile);
    }

    fn view<'a>(verse_id: &'a str, petal_id: &'a str, field: &'a TerrainHeightField) -> TerrainView<'a> {
        TerrainView { verse_id, petal_id, field, revision: 1, world_scale: 1.0, origin_ele: 100.0 }
    }

    fn surface(source: &Arc<dyn TerrainSource>, verse_id: &str, petal_id: &str) -> Result<(u64, Option<f64>), String> {
        let mut out = Vec::new();
        let generation = source.sample(verse_id, petal_id, &[(50.0, 50.0), (500.0, 50.0)], &mut out)?;
        assert!(out[1].is_none(), "outside every resident tile is none");
        Ok((generation, out[0].map(|point| point.surface_y)))
    }

    /// Unwanted means no publish; a generation change publishes after the debounce (trailing edge reported);
    /// another petal or verse is refused; a switch or withdrawal skips the debounce; leaving withdraws at once.
    #[test]
    fn snapshot_follows_active_petal_and_generation() {
        let snapshots = TerrainSnapshots::default();
        let mut last = PublishState::default();
        let mut field = TerrainHeightField::default();
        east_ramp(&mut field, 10, 0.0);
        let start = Instant::now();

        publish_step(&snapshots, Some(view("v1", "p1", &field)), &mut last, start);
        assert!(snapshots.current().is_none(), "no terrain-granted instance: nothing is cloned");

        let source = snapshots.source();
        publish_step(&snapshots, Some(view("v1", "p1", &field)), &mut last, start);
        let (first, height) = surface(&source, "v1", "p1").expect("published");
        assert!(height.is_some_and(|y| (y - 5.0).abs() < 1e-4), "{height:?}");
        let mut out = Vec::new();
        source.sample("v1", "p1", &[(50.0, 50.0)], &mut out).unwrap();
        let point = out[0].unwrap();
        assert!((point.elevation_m - 105.0).abs() < 1e-4 && (point.slope_pct - 10.0).abs() < 1e-3, "{point:?}");

        east_ramp(&mut field, 11, 2.0);
        let pending = publish_step(&snapshots, Some(view("v1", "p1", &field)), &mut last, start + Duration::from_millis(100));
        assert!(pending, "a debounced change asks the caller to wake the loop");
        assert_eq!(surface(&source, "v1", "p1").unwrap().0, first, "a change inside the debounce waits");
        let pending = publish_step(&snapshots, Some(view("v1", "p1", &field)), &mut last, start + SNAPSHOT_DEBOUNCE);
        assert!(!pending, "the trailing publish lands");
        let (second, height) = surface(&source, "v1", "p1").unwrap();
        assert!(second > first && height.is_some_and(|y| (y - 7.0).abs() < 1e-4), "the higher-zoom tile now answers");

        assert_eq!(surface(&source, "v1", "p2"), Err(NOT_RESIDENT.to_string()));
        assert_eq!(surface(&source, "v2", "p1"), Err(NOT_RESIDENT.to_string()));

        // A petal switch inside the debounce window publishes the new petal at once.
        let switched_at = start + SNAPSHOT_DEBOUNCE + Duration::from_millis(1);
        assert!(!publish_step(&snapshots, Some(view("v1", "p2", &field)), &mut last, switched_at));
        assert!(surface(&source, "v1", "p2").unwrap().0 > second, "the switched-to petal is published immediately");
        assert_eq!(surface(&source, "v1", "p1"), Err(NOT_RESIDENT.to_string()));

        publish_step(&snapshots, None, &mut last, switched_at + Duration::from_millis(1));
        assert_eq!(surface(&source, "v1", "p2"), Err(NOT_RESIDENT.to_string()), "a petal left is withdrawn at once");
        assert!(!publish_step(&snapshots, Some(view("v1", "p1", &field)), &mut last, switched_at + Duration::from_millis(2)));
        assert!(surface(&source, "v1", "p1").is_ok(), "the first publish after a withdrawal skips the debounce");
        publish_step(&snapshots, None, &mut last, start + SNAPSHOT_DEBOUNCE * 2);

        drop(source);
        publish_step(&snapshots, Some(view("v1", "p1", &field)), &mut last, start + SNAPSHOT_DEBOUNCE * 4);
        assert!(snapshots.current().is_none(), "the last instance gone: the publisher idles");
    }
}
