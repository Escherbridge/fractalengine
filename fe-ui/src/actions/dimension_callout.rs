//! Dimension callouts (HS-P6-CALLOUT, hexon_scale FR-9 follow-on): promote a
//! completed Measure tool reading (Tape or closed Area) into an addressable
//! `dimension_callout` node, the same "endpoint node" pattern T3 established
//! for earthwork regions (`terrain_proposal.rs` §sculpt). See
//! `fe-ui/src/actions/AGENTS.md` §dimension-callout and
//! `fe-ui/src/node_manager/AGENTS.md` §measure-tool.

use std::collections::HashMap;

use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::gimbal::GimbalGizmoGroup;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::measure::{MeasureMode, MeasureState};
use crate::visibility::VisibilityState;

/// `node_kind` contract value for dimension callout nodes (mirror of
/// fe-query's literal key — do not import fe-query).
pub(crate) const DIMENSION_CALLOUT_NODE_KIND: &str = "dimension_callout";
/// `CreateNode.correlation_id` prefix binding a created node to its callout.
pub(crate) const CALLOUT_CORRELATION_PREFIX: &str = "callout:";
/// Defense-in-depth cap on a hydrated callout's anchor count (mirrors
/// `measure::MAX_MEASURE_ANCHORS`).
pub(crate) const MAX_CALLOUT_ANCHORS: usize = 256;

/// A completed measurement's shape, independent of the live `MeasureState` it
/// was promoted from (A12: the readout is recomputed from this, never stored).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CalloutGeometry {
    pub(crate) mode: MeasureMode,
    pub(crate) anchors: Vec<[f64; 3]>,
    pub(crate) closed: bool,
}

/// `"tape"`/`"area"` — the wire/property-bag spelling of a [`MeasureMode`].
/// `pub(crate)` so the Options panel can spell `UiAction::SaveMeasureAsCallout`'s
/// plain-data `mode` field without naming `MeasureMode` itself.
pub(crate) fn mode_str(mode: MeasureMode) -> &'static str {
    match mode {
        MeasureMode::Tape => "tape",
        MeasureMode::Area => "area",
    }
}

/// The inverse of [`mode_str`]. `None` for anything else (strictest-reader
/// rule — an unrecognized mode is ignored, never guessed at).
fn mode_from_str(s: &str) -> Option<MeasureMode> {
    match s {
        "tape" => Some(MeasureMode::Tape),
        "area" => Some(MeasureMode::Area),
        _ => None,
    }
}

/// Only a COMPLETE measurement promotes: Tape with exactly 2 anchors, or an
/// Area that has been closed with at least 3. Anything else (in-progress,
/// too few anchors, an open polygon) is `None` — the Options panel's "Save as
/// callout" button is enabled exactly when this is `Some`.
pub(crate) fn callout_from_measure(state: &MeasureState) -> Option<CalloutGeometry> {
    let anchors_f64 = || {
        state
            .anchors
            .iter()
            .map(|a| [a[0] as f64, a[1] as f64, a[2] as f64])
            .collect::<Vec<_>>()
    };
    match state.mode {
        MeasureMode::Tape if state.anchors.len() == 2 => Some(CalloutGeometry {
            mode: MeasureMode::Tape,
            anchors: anchors_f64(),
            closed: false,
        }),
        MeasureMode::Area if state.closed && state.anchors.len() >= 3 => Some(CalloutGeometry {
            mode: MeasureMode::Area,
            anchors: anchors_f64(),
            closed: true,
        }),
        _ => None,
    }
}

/// The property bag a callout node carries, in a stable key order (the
/// `SetNodeProperty` echo send order, and the order the correlation-echo test
/// asserts). Pure — the endpoint contract is testable without a DB.
pub(crate) fn callout_properties(geometry: &CalloutGeometry) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("node_kind", serde_json::json!(DIMENSION_CALLOUT_NODE_KIND)),
        ("dimension.mode", serde_json::json!(mode_str(geometry.mode))),
        ("dimension.anchors", serde_json::json!(geometry.anchors)),
        ("dimension.closed", serde_json::json!(geometry.closed)),
    ]
}

/// Vertex-mean of the geometry's anchors — the tape's midpoint, or the area
/// polygon's centroid. A single formula covers both (mean of 2 points IS the
/// midpoint).
pub(crate) fn callout_anchor_position(geometry: &CalloutGeometry) -> [f32; 3] {
    let n = (geometry.anchors.len().max(1)) as f64;
    let sum = geometry
        .anchors
        .iter()
        .fold([0.0f64; 3], |mut acc, a| {
            acc[0] += a[0];
            acc[1] += a[1];
            acc[2] += a[2];
            acc
        });
    [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32]
}

/// Display name for a freshly-created callout node, e.g. `"Dimension 12.5 m"`.
/// Reads the SAME readout row the Options panel already shows, so the node's
/// name never disagrees with what the user just saw.
pub(crate) fn callout_display_name(measure: &MeasureState, world_scale: f64) -> String {
    let out = crate::node_manager::measure::readout(measure, world_scale);
    let rows = crate::node_manager::measure::readout_rows(&out, measure.closed);
    let wanted = match measure.mode {
        MeasureMode::Tape => "Distance",
        MeasureMode::Area => "Area",
    };
    let value = rows
        .into_iter()
        .find(|(label, _)| *label == wanted)
        .map(|(_, v)| v)
        .unwrap_or_default();
    format!("Dimension {value}")
}

/// Extract the callout id from a `callout:{id}` correlation id.
pub(crate) fn callout_id_from_correlation(correlation_id: &str) -> Option<&str> {
    correlation_id
        .strip_prefix(CALLOUT_CORRELATION_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Strictest-reader parse of a persisted/replicated property bag into a
/// [`CalloutGeometry`]. Accepts only a bag that is the right kind, has every
/// key, a known mode, an anchor count that fits the mode, at most
/// [`MAX_CALLOUT_ANCHORS`] anchors, and all-finite `[f64; 3]` numbers.
/// Anything else is `None`, so a malformed peer-authored bag renders nothing
/// (CO-1) rather than crashing or stalling the draw loop.
fn parse_callout_bag(properties: &serde_json::Value) -> Option<CalloutGeometry> {
    let mode = match properties.get("dimension.mode").and_then(|v| v.as_str()) {
        Some("tape") => MeasureMode::Tape,
        Some("area") => MeasureMode::Area,
        _ => return None,
    };
    let closed = properties.get("dimension.closed").and_then(|v| v.as_bool())?;
    let anchors_raw = properties.get("dimension.anchors").and_then(|v| v.as_array())?;
    if anchors_raw.is_empty() || anchors_raw.len() > MAX_CALLOUT_ANCHORS {
        return None;
    }
    let mut anchors = Vec::with_capacity(anchors_raw.len());
    for anchor in anchors_raw {
        let triple = anchor.as_array()?;
        if triple.len() != 3 {
            return None;
        }
        let mut xyz = [0.0f64; 3];
        for (slot, component) in xyz.iter_mut().zip(triple) {
            let n = component.as_f64()?;
            if !n.is_finite() {
                return None;
            }
            *slot = n;
        }
        anchors.push(xyz);
    }
    let mode_consistent = match mode {
        MeasureMode::Tape => anchors.len() == 2 && !closed,
        MeasureMode::Area => closed && anchors.len() >= 3,
    };
    if !mode_consistent {
        return None;
    }
    Some(CalloutGeometry {
        mode,
        anchors,
        closed,
    })
}

/// `NodePropertiesLoaded` hydration (mirrors `hydrate_promoted_stamp` /
/// `hydrate_earthwork_region`): a bag whose `node_kind` is
/// `"dimension_callout"` re-binds `node_id` to its geometry + owning petal.
/// `petal_id` is the dispatcher's `VerseManager::petal_id_of` resolution, same
/// as `PathAssetCache::note_properties`.
pub(crate) fn hydrate_dimension_callout(
    node_id: &str,
    petal_id: Option<&str>,
    properties: &serde_json::Value,
    map: &mut DimensionCalloutMap,
) {
    if properties.get("node_kind").and_then(|v| v.as_str()) != Some(DIMENSION_CALLOUT_NODE_KIND) {
        return;
    }
    let Some(geometry) = parse_callout_bag(properties) else {
        return;
    };
    map.record(node_id, petal_id.map(str::to_string), geometry);
}

/// `node_id`s effectively visible in the active petal, through the one shared
/// resolver (GV-1: never re-derive visibility for a new consumer). The draw
/// system calls this every frame rather than caching it.
pub(crate) fn visible_callouts<'a>(
    map: &'a DimensionCalloutMap,
    nav: &NavigationManager,
    vis: &VisibilityState,
) -> Vec<&'a CalloutGeometry> {
    let ancestry = crate::visibility::active_ancestry(nav);
    map.nodes
        .iter()
        .filter(|(node_id, (petal_id, _))| {
            petal_id.as_deref() == nav.active_petal_id.as_deref()
                && crate::visibility::effective_visibility(node_id, ancestry, vis)
        })
        .map(|(_, (_, geometry))| geometry)
        .collect()
}

/// Region/node bookkeeping for dimension callouts: the pending stash consumed
/// on the `NodeCreated` echo (mirrors `EarthworkNodeMap`'s pending-material
/// idiom), plus the owning-petal + geometry for every known callout node.
#[derive(Resource, Default)]
pub struct DimensionCalloutMap {
    pending: HashMap<String, CalloutGeometry>,
    nodes: HashMap<String, (Option<String>, CalloutGeometry)>,
    /// Monotonic id counter for minted callout ids (no `uuid`/`rand` dep,
    /// mirrors `SculptToolState::next_region_id`).
    next_callout_id: u64,
}

impl DimensionCalloutMap {
    /// Mint a fresh callout id (`c{n}`), monotonic so ids never collide
    /// within a session.
    fn mint_callout_id(&mut self) -> String {
        self.next_callout_id += 1;
        format!("c{}", self.next_callout_id)
    }

    pub(crate) fn stash_pending(&mut self, callout_id: String, geometry: CalloutGeometry) {
        self.pending.insert(callout_id, geometry);
    }

    /// Consume the stashed geometry for `callout_id` (once, on `NodeCreated`).
    pub(crate) fn take_pending(&mut self, callout_id: &str) -> Option<CalloutGeometry> {
        self.pending.remove(callout_id)
    }

    /// Bind `node_id` to its owning petal + geometry.
    pub(crate) fn record(&mut self, node_id: &str, petal_id: Option<String>, geometry: CalloutGeometry) {
        self.nodes.insert(node_id.to_string(), (petal_id, geometry));
    }

    /// Drop a deleted callout node's bookkeeping. `true` when one was known.
    pub(crate) fn forget(&mut self, node_id: &str) -> bool {
        self.nodes.remove(node_id).is_some()
    }

    #[cfg(test)]
    pub(crate) fn node_geometry(&self, node_id: &str) -> Option<&CalloutGeometry> {
        self.nodes.get(node_id).map(|(_, geometry)| geometry)
    }
}

/// `NodeCreated`: a `callout:{id}` correlation moves `pending → nodes[node_id]`
/// and sends one `SetNodeProperty` per bag key — the same pen-tool "echoed-id
/// match only" idiom as the earthwork bind, so a concurrent foreign create
/// can never hijack the stash. Any other correlation (`None`, `earthwork:...`,
/// a pen track, ...) is a no-op.
pub(crate) fn bind_created_callout(
    db_sender: &DbCommandSender,
    map: &mut DimensionCalloutMap,
    node_id: &str,
    petal_id: &str,
    correlation_id: Option<&str>,
) {
    let Some(callout_id) = correlation_id.and_then(callout_id_from_correlation) else {
        return;
    };
    let Some(geometry) = map.take_pending(callout_id) else {
        return;
    };
    map.record(node_id, Some(petal_id.to_string()), geometry.clone());
    for (key, value) in callout_properties(&geometry) {
        if db_sender
            .0
            .send(DbCommand::SetNodeProperty {
                node_id: node_id.to_string(),
                key: key.to_string(),
                value,
            })
            .is_err()
        {
            bevy::log::warn!(
                "db_sender channel closed — dimension callout contract keys not written"
            );
            break;
        }
    }
}

/// Options panel "Save as callout" (`UiAction::SaveMeasureAsCallout`): mint a
/// fresh callout id, stash the geometry, and send the `CreateNode` whose echo
/// `bind_created_callout` consumes. Mirrors `terrain_proposal::create_region_node`.
/// Takes the plain-data shape the (`pub`) `UiAction` variant carries — see
/// its doc comment for why that is not [`CalloutGeometry`] directly. An
/// unrecognized `mode` is a no-op (defensive; the Options panel only ever
/// sends a value `mode_str` produced).
pub(crate) fn save_measure_as_callout(
    db_sender: &DbCommandSender,
    map: &mut DimensionCalloutMap,
    petal_id: String,
    name: String,
    mode: &str,
    anchors: Vec<[f64; 3]>,
    closed: bool,
) {
    let Some(mode) = mode_from_str(mode) else {
        bevy::log::warn!("dimension callout: unrecognized mode {mode:?}, nothing saved");
        return;
    };
    let geometry = CalloutGeometry {
        mode,
        anchors,
        closed,
    };
    let callout_id = map.mint_callout_id();
    let position = callout_anchor_position(&geometry);
    map.stash_pending(callout_id.clone(), geometry);
    if db_sender
        .0
        .send(DbCommand::CreateNode {
            petal_id,
            name,
            position,
            correlation_id: Some(format!("{CALLOUT_CORRELATION_PREFIX}{callout_id}")),
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — dimension callout node not created");
    }
}

/// Draws every visible callout in the active petal as gizmo line segments
/// (+ the closing edge for Area), always — not mode-gated, outside the
/// selection chain (`node_manager::mod` registers this next to
/// `billboard::billboard_face_camera`). The in-world readout LABEL (A12:
/// recomputed here, never stored — see `callout_display_name`) is deferred to
/// a future egui-pass wiring (`fe-ui/src/viewport_labels.rs`'s pattern); only
/// the geometry is drawn by this slice (RUNBOOK §4 user-gated in-app pass).
pub(crate) fn draw_dimension_callouts(
    map: Res<DimensionCalloutMap>,
    nav: Res<NavigationManager>,
    vis: Res<VisibilityState>,
    mut gizmos: Gizmos<GimbalGizmoGroup>,
) {
    const CALLOUT_COLOR: Color = Color::srgba(0.4, 0.85, 1.0, 0.9);
    for geometry in visible_callouts(&map, &nav, &vis) {
        let points: Vec<Vec3> = geometry
            .anchors
            .iter()
            .map(|a| Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32))
            .collect();
        for pair in points.windows(2) {
            gizmos.line(pair[0], pair[1], CALLOUT_COLOR);
        }
        if geometry.closed {
            if let (Some(first), Some(last)) = (points.first(), points.last()) {
                gizmos.line(*last, *first, CALLOUT_COLOR);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::OverrideState;

    fn tape(anchors: Vec<[f32; 3]>, closed: bool) -> MeasureState {
        MeasureState {
            mode: MeasureMode::Tape,
            anchors,
            closed,
        }
    }

    fn area(anchors: Vec<[f32; 3]>, closed: bool) -> MeasureState {
        MeasureState {
            mode: MeasureMode::Area,
            anchors,
            closed,
        }
    }

    #[test]
    fn promote_builds_a_callout_bag_only_from_a_complete_measurement() {
        let complete_tape = tape(vec![[0.0, 0.0, 0.0], [3.0, 0.0, 4.0]], false);
        let geometry = callout_from_measure(&complete_tape).expect("tape with 2 anchors");
        assert_eq!(geometry.mode, MeasureMode::Tape);
        assert_eq!(geometry.anchors, vec![[0.0, 0.0, 0.0], [3.0, 0.0, 4.0]]);
        assert!(!geometry.closed);
        assert_eq!(
            callout_properties(&geometry),
            vec![
                ("node_kind", serde_json::json!(DIMENSION_CALLOUT_NODE_KIND)),
                ("dimension.mode", serde_json::json!("tape")),
                (
                    "dimension.anchors",
                    serde_json::json!([[0.0, 0.0, 0.0], [3.0, 0.0, 4.0]])
                ),
                ("dimension.closed", serde_json::json!(false)),
            ],
            "the bag keys and values are exact"
        );

        let incomplete_tape = tape(vec![[0.0, 0.0, 0.0]], false);
        assert!(
            callout_from_measure(&incomplete_tape).is_none(),
            "tape with 1 anchor is incomplete"
        );

        let open_area = area(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            false,
        );
        assert!(
            callout_from_measure(&open_area).is_none(),
            "an open area polygon is incomplete"
        );

        let closed_area = area(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            true,
        );
        let geometry = callout_from_measure(&closed_area).expect("closed area with 3 anchors");
        assert_eq!(geometry.mode, MeasureMode::Area);
        assert!(geometry.closed);
        assert_eq!(geometry.anchors.len(), 3);
    }

    #[test]
    fn hydrate_dimension_callout_accepts_only_a_complete_finite_bounded_bag() {
        fn bag(mode: &str, anchors: serde_json::Value, closed: bool) -> serde_json::Value {
            serde_json::json!({
                "node_kind": DIMENSION_CALLOUT_NODE_KIND,
                "dimension.mode": mode,
                "dimension.anchors": anchors,
                "dimension.closed": closed,
            })
        }

        let complete = bag(
            "tape",
            serde_json::json!([[0.0, 0.0, 0.0], [3.0, 0.0, 4.0]]),
            false,
        );
        let mut map = DimensionCalloutMap::default();
        hydrate_dimension_callout("n1", Some("p1"), &complete, &mut map);
        assert!(map.node_geometry("n1").is_some(), "a complete bag hydrates");

        let cases: Vec<(&str, serde_json::Value)> = vec![
            (
                "a missing key is ignored",
                serde_json::json!({
                    "node_kind": DIMENSION_CALLOUT_NODE_KIND,
                    "dimension.mode": "tape",
                    "dimension.anchors": [[0.0, 0.0, 0.0], [3.0, 0.0, 4.0]],
                }),
            ),
            (
                // JSON cannot encode NaN; a null in its place exercises the
                // same numeric-rejection path a NaN would also hit (serde_json
                // can never deserialize a literal NaN either).
                "a non-finite anchor component is ignored",
                bag(
                    "tape",
                    serde_json::json!([[0.0, 0.0, 0.0], [null, 0.0, 4.0]]),
                    false,
                ),
            ),
            (
                "257 anchors exceeds the cap",
                bag(
                    "area",
                    serde_json::Value::Array(
                        (0..257)
                            .map(|i| serde_json::json!([i as f64, 0.0, 0.0]))
                            .collect(),
                    ),
                    true,
                ),
            ),
            (
                "a tape with 3 anchors is not a complete tape measurement",
                bag(
                    "tape",
                    serde_json::json!([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]),
                    false,
                ),
            ),
            (
                "the wrong node_kind is ignored",
                serde_json::json!({
                    "node_kind": "stamp",
                    "dimension.mode": "tape",
                    "dimension.anchors": [[0.0, 0.0, 0.0], [3.0, 0.0, 4.0]],
                    "dimension.closed": false,
                }),
            ),
        ];
        for (label, properties) in cases {
            let mut map = DimensionCalloutMap::default();
            hydrate_dimension_callout("n", Some("p1"), &properties, &mut map);
            assert!(map.node_geometry("n").is_none(), "{label}");
        }
    }

    #[test]
    fn a_callout_correlation_echo_writes_the_bag_and_nothing_else_does() {
        let (tx, rx) = crossbeam::channel::bounded(8);
        let db_sender = DbCommandSender(tx);
        let geometry = CalloutGeometry {
            mode: MeasureMode::Tape,
            anchors: vec![[0.0, 0.0, 0.0], [3.0, 0.0, 4.0]],
            closed: false,
        };

        let mut map = DimensionCalloutMap::default();
        map.stash_pending("c1".to_string(), geometry.clone());
        bind_created_callout(&db_sender, &mut map, "node-7", "p1", Some("callout:c1"));

        assert_eq!(map.node_geometry("node-7"), Some(&geometry));
        let mut keys = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                DbCommand::SetNodeProperty { node_id, key, .. } => {
                    assert_eq!(node_id, "node-7");
                    keys.push(key);
                }
                other => panic!("unexpected command: {other:?}"),
            }
        }
        assert_eq!(
            keys,
            vec![
                "node_kind".to_string(),
                "dimension.mode".to_string(),
                "dimension.anchors".to_string(),
                "dimension.closed".to_string(),
            ]
        );

        // A plain echo (no correlation) emits no callout keys.
        let mut map = DimensionCalloutMap::default();
        bind_created_callout(&db_sender, &mut map, "node-8", "p1", None);
        assert!(rx.try_recv().is_err(), "a plain echo must not write anything");

        // A foreign (earthwork) correlation emits no callout keys either.
        let mut map = DimensionCalloutMap::default();
        bind_created_callout(&db_sender, &mut map, "node-9", "p1", Some("earthwork:r2"));
        assert!(
            rx.try_recv().is_err(),
            "a foreign correlation must not write anything"
        );
    }

    #[test]
    fn visible_callouts_skip_hidden_and_foreign_petal_callouts() {
        let visible_geo = CalloutGeometry {
            mode: MeasureMode::Tape,
            anchors: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            closed: false,
        };
        let hidden_geo = CalloutGeometry {
            mode: MeasureMode::Tape,
            anchors: vec![[2.0, 0.0, 0.0], [3.0, 0.0, 0.0]],
            closed: false,
        };
        let foreign_geo = CalloutGeometry {
            mode: MeasureMode::Tape,
            anchors: vec![[4.0, 0.0, 0.0], [5.0, 0.0, 0.0]],
            closed: false,
        };

        let mut map = DimensionCalloutMap::default();
        map.record("visible-node", Some("p1".into()), visible_geo.clone());
        map.record("hidden-node", Some("p1".into()), hidden_geo);
        map.record("foreign-node", Some("p2".into()), foreign_geo);

        let nav = NavigationManager {
            active_petal_id: Some("p1".into()),
            ..Default::default()
        };
        let mut vis = VisibilityState::default();
        vis.set_node_override("hidden-node", OverrideState::Hide);

        let result = visible_callouts(&map, &nav, &vis);
        assert_eq!(result.len(), 1, "only the visible, same-petal callout survives");
        assert_eq!(result[0], &visible_geo);
    }

    #[test]
    fn a_deleted_callout_node_is_dropped_from_the_map() {
        let mut map = DimensionCalloutMap::default();
        let geometry = CalloutGeometry {
            mode: MeasureMode::Tape,
            anchors: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            closed: false,
        };
        map.record("n1", Some("p1".into()), geometry);
        assert!(map.node_geometry("n1").is_some());

        assert!(map.forget("n1"));
        assert!(map.node_geometry("n1").is_none());
        assert!(
            !map.forget("n1"),
            "forgetting an already-unknown node reports false"
        );
    }
}
