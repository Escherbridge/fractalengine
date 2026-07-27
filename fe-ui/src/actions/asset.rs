//! Node asset download action — fe-ui only queues the request; the main
//! binary (owns `BlobStoreHandle` + the DB channel) resolves it and writes
//! the outcome to `crate::asset_ops::AssetDownloadStatus`.
//!
//! Also homes the Wave-1 stamped-asset (T2) interaction state + per-verb
//! handlers: individual stamp selection (FR-2, promote-on-first-select) and the
//! per-stamp scale/rotation overrides (FR-3). Arc-length "slide along path"
//! (FR-3, curve domain) lives in `actions/path.rs`. See
//! `fe-ui/src/actions/AGENTS.md` §stamped-assets. T2 fills these bodies; it
//! edits no central file.

use std::collections::HashMap;

use bevy::prelude::Resource;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::asset_ops::{AssetOp, PendingAssetOps};

pub(crate) fn request_download(asset_ops: &mut PendingAssetOps, node_id: String) {
    bevy::log::info!("Asset: download requested for node {}", node_id);
    asset_ops.0.push(AssetOp::Download { node_id });
}

// ---------------------------------------------------------------------------
// Stamp override JSON contract (T2 FR-3). fe-ui must NOT depend on fe-terrain,
// so a promoted stamp node's sparse per-instance overrides ride on plain node
// properties the materializer reads back (mirror-enum / JSON-contract idiom,
// per the ui_shell NFR-2 precedent). Values are petal-local meters/radians —
// no `world_scale` (N-1). Position is NEVER stored: it stays path-derived.
// ---------------------------------------------------------------------------

/// Property key: per-stamp scale override `[sx, sy, sz]` (JSON array).
pub(crate) const STAMP_SCALE_KEY: &str = "stamp.override.scale";
/// Property key: per-stamp rotation override quaternion `[x, y, z, w]`.
pub(crate) const STAMP_ROTATION_KEY: &str = "stamp.override.rotation";
/// Property key: per-stamp arc-length offset (meters) along its owning curve —
/// the "slide along path" reposition (FR-3, Q-1). Position stays path-locked;
/// this only shifts WHERE on the curve the base transform is sampled.
pub(crate) const STAMP_ARC_KEY: &str = "stamp.override.arc_m";

/// A stamp identified pre-promotion by `(owning path/track node, instance index)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StampRef {
    pub track_node_id: String,
    pub stamp_index: usize,
}

impl StampRef {
    fn new(track_node_id: String, stamp_index: usize) -> Self {
        Self {
            track_node_id,
            stamp_index,
        }
    }
}

/// Sparse per-stamp overrides (FR-3). Any `None` field means "inherit the
/// path-following base"; only touched controls persist so an untouched axis
/// never clobbers a stored value (mirrors `path::style_property_writes`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct StampOverride {
    pub scale: Option<[f32; 3]>,
    pub rotation: Option<[f32; 4]>,
    pub arc_offset_m: Option<f32>,
}

/// Wave 1 (T2 stamped_asset_nodes): stamp interaction state — which stamp is
/// individually selected, the lazy-promotion bookkeeping (N-9: no per-instance
/// store cost until a stamp is addressed), and the sparse scale/rotate/slide
/// overrides. Registered in `plugin.rs`; mutated by the handlers below +
/// `path::handle_slide_stamp`. See `fe-ui/src/actions/AGENTS.md` §stamped-assets.
// `pub` (not `pub(crate)`): appears in the `gardener_console`/
// `render_context_menu` pub signatures — mirrors `SculptToolState`.
#[derive(Resource, Default)]
pub struct StampInteractionState {
    /// The individually-selected stamp (viewport click), if any. Distinct from
    /// `NodeManager.selected` and `PathEditorState.editing_track_id` (N-3): a
    /// stamp selection is its own authority until promotion yields a node id.
    selected: Option<StampRef>,
    /// A stamp select that has not yet been promoted — drained by the UI, which
    /// queues `UiAction::PromoteStamp` (routes to T1 `PromoteInstance` via the
    /// T4 `node::handle_promote_stamp` handler; N-5: the UI never asserts a role).
    pending_promotion: Option<StampRef>,
    /// `(track, index) → promoted node id`, recorded when `DbResult::NodePromoted`
    /// comes back (cross-boundary: the db-results handler calls `mark_promoted`).
    promoted: HashMap<StampRef, String>,
    /// Sparse per-stamp overrides, kept in-memory so a gesture updates live even
    /// before promotion completes; flushed to node properties once promoted.
    overrides: HashMap<StampRef, StampOverride>,
}

impl StampInteractionState {
    /// The currently individually-selected stamp, if any. Read by the
    /// right-click context menu to mark its stamp header (T4).
    pub(crate) fn selected(&self) -> Option<&StampRef> {
        self.selected.as_ref()
    }

    /// `true` when this stamp already has a materialized node (idempotent
    /// promotion guard — N-9: promote only on first individual address).
    pub(crate) fn is_promoted(&self, stamp: &StampRef) -> bool {
        self.promoted.contains_key(stamp)
    }

    /// The promoted node id for a stamp, once known.
    pub(crate) fn promoted_node_id(&self, stamp: &StampRef) -> Option<&str> {
        self.promoted.get(stamp).map(String::as_str)
    }

    /// Record a completed promotion (called by the db-results handler on
    /// `NodePromoted`). Flushes any overrides captured before the id was known.
    pub(crate) fn mark_promoted(&mut self, stamp: StampRef, node_id: String) {
        self.promoted.insert(stamp, node_id);
    }

    /// Take the pending-promotion marker (the UI drains this to queue
    /// `PromoteStamp`). `None` once consumed.
    pub(crate) fn take_pending_promotion(&mut self) -> Option<StampRef> {
        self.pending_promotion.take()
    }

    /// The sparse override for a stamp, if any control has been touched.
    pub(crate) fn override_for(&self, stamp: &StampRef) -> Option<&StampOverride> {
        self.overrides.get(stamp)
    }

    /// One track's overrides as an index-sorted list — the materializer's read
    /// view for override application + its applied-gate change fingerprint.
    pub(crate) fn overrides_for_track(&self, track: &str) -> Vec<(usize, StampOverride)> {
        let mut list: Vec<(usize, StampOverride)> = self
            .overrides
            .iter()
            .filter(|(stamp, _)| stamp.track_node_id == track)
            .map(|(stamp, ov)| (stamp.stamp_index, ov.clone()))
            .collect();
        list.sort_by_key(|(index, _)| *index);
        list
    }

    /// Record the arc-length "slide" offset (meters) for a stamp — set by
    /// `path::handle_slide_stamp` (curve domain). Position stays path-locked;
    /// this shifts WHERE on the curve the base transform samples (FR-3, Q-1).
    pub(crate) fn set_arc_offset(&mut self, stamp: &StampRef, arc_offset_m: f32) {
        self.override_mut(stamp).arc_offset_m = Some(arc_offset_m);
    }

    fn override_mut(&mut self, stamp: &StampRef) -> &mut StampOverride {
        self.overrides.entry(stamp.clone()).or_default()
    }

    /// FR-5 re-flow on stamp delete: after instance `deleted_index` of `track`
    /// is tombstoned (T1) and its path re-flows (T1 `PathReflow` event), the
    /// remaining stamps shift down one index. Remap the sparse overrides + the
    /// promotion map so every surviving stamp KEEPS its override under its new
    /// index (indices `> deleted_index` decrement by 1; the deleted index is
    /// dropped). Positions are re-derived by re-running the sampler for the new
    /// count — this only fixes the identity→override binding. Idempotent-safe.
    pub(crate) fn reflow_after_delete(&mut self, track: &str, deleted_index: usize) {
        self.overrides =
            remap_after_delete(std::mem::take(&mut self.overrides), track, deleted_index);
        self.promoted =
            remap_after_delete(std::mem::take(&mut self.promoted), track, deleted_index);
        // Drop / shift the live selection + pending promotion for the same track.
        self.selected = shift_ref(self.selected.take(), track, deleted_index);
        self.pending_promotion = shift_ref(self.pending_promotion.take(), track, deleted_index);
    }
}

/// Remap a `StampRef`-keyed map for a delete of `(track, deleted_index)`:
/// drop that key; decrement the index of same-track keys above it; leave other
/// tracks untouched. Shared by the overrides + promotion maps (FR-5).
fn remap_after_delete<V>(
    map: HashMap<StampRef, V>,
    track: &str,
    deleted_index: usize,
) -> HashMap<StampRef, V> {
    map.into_iter()
        .filter_map(|(mut k, v)| {
            if k.track_node_id != track {
                return Some((k, v));
            }
            match k.stamp_index.cmp(&deleted_index) {
                std::cmp::Ordering::Equal => None, // the deleted stamp
                std::cmp::Ordering::Greater => {
                    k.stamp_index -= 1;
                    Some((k, v))
                }
                std::cmp::Ordering::Less => Some((k, v)),
            }
        })
        .collect()
}

/// Shift a single optional `StampRef` for the same delete (`None` if it WAS the
/// deleted stamp). Used for the live selection + pending-promotion markers.
fn shift_ref(reference: Option<StampRef>, track: &str, deleted_index: usize) -> Option<StampRef> {
    let mut r = reference?;
    if r.track_node_id != track {
        return Some(r);
    }
    match r.stamp_index.cmp(&deleted_index) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => {
            r.stamp_index -= 1;
            Some(r)
        }
        std::cmp::Ordering::Less => Some(r),
    }
}

/// Wave 1: T2 FR-2 — select a stamp individually. Records the selection and, on
/// the FIRST individual select of an un-promoted stamp, flags it for promotion
/// (Q-2 ratified: promote per-stamp on select). The actual `PromoteInstance` is
/// sent by the T1/T4 seam (`node::handle_promote_stamp`) — this handler holds no
/// `DbCommandSender` by design (N-5: selection is a pure UI-state transition).
pub(crate) fn handle_select_stamp(
    stamp_state: &mut StampInteractionState,
    track_node_id: String,
    stamp_index: usize,
) {
    let stamp = StampRef::new(track_node_id, stamp_index);
    // Re-selecting the same stamp is idempotent; a new selection replaces it.
    if stamp_state.selected.as_ref() == Some(&stamp) {
        return;
    }
    if !stamp_state.is_promoted(&stamp) {
        // First individual address → queue lazy promotion (T1 FR-5 / N-9).
        stamp_state.pending_promotion = Some(stamp.clone());
    }
    stamp_state.selected = Some(stamp);
}

/// Wave 1: T2 FR-3 — persist a stamp's per-node scale override. Position stays
/// path-derived; only scale/rotation are overridable. The override is recorded
/// in-state immediately (live gesture) and, once the stamp is promoted, written
/// to its node's `stamp.override.scale` property (the materializer applies it
/// on top of the path-following base). Meters/unit-scale — no `world_scale`.
pub(crate) fn handle_set_stamp_scale(
    db_sender: &DbCommandSender,
    stamp_state: &mut StampInteractionState,
    track_node_id: String,
    stamp_index: usize,
    scale: [f32; 3],
) {
    let stamp = StampRef::new(track_node_id, stamp_index);
    stamp_state.override_mut(&stamp).scale = Some(scale);
    persist_override(
        db_sender,
        stamp_state,
        &stamp,
        STAMP_SCALE_KEY,
        serde_json::json!(scale),
    );
}

/// Wave 1: T2 FR-3 — persist a stamp's per-node rotation override (quaternion
/// xyzw). Same sparse-override discipline as `handle_set_stamp_scale`. The
/// arc-length "slide along path" reposition is homed in `actions/path.rs`
/// (curve domain), so this handler covers rotation only.
pub(crate) fn handle_set_stamp_rotation(
    db_sender: &DbCommandSender,
    stamp_state: &mut StampInteractionState,
    track_node_id: String,
    stamp_index: usize,
    rotation: [f32; 4],
) {
    let stamp = StampRef::new(track_node_id, stamp_index);
    stamp_state.override_mut(&stamp).rotation = Some(rotation);
    persist_override(
        db_sender,
        stamp_state,
        &stamp,
        STAMP_ROTATION_KEY,
        serde_json::json!(rotation),
    );
}

/// Write an override property to a stamp's promoted node when its id is known.
/// Pre-promotion the value is held in-state only (flushed after `mark_promoted`);
/// this keeps N-9 (no store row until the stamp is individually addressed).
pub(crate) fn persist_override(
    db_sender: &DbCommandSender,
    stamp_state: &StampInteractionState,
    stamp: &StampRef,
    key: &str,
    value: serde_json::Value,
) {
    let Some(node_id) = stamp_state.promoted_node_id(stamp) else {
        // Not yet promoted → the override lives in-state; the UI will queue
        // `PromoteStamp` and re-apply once the node id lands (no silent loss).
        bevy::log::debug!(
            "stamp {}::{} override buffered pre-promotion ({key})",
            stamp.track_node_id,
            stamp.stamp_index
        );
        return;
    };
    if db_sender
        .0
        .send(DbCommand::SetNodeProperty {
            node_id: node_id.to_string(),
            key: key.to_string(),
            value,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — stamp override {key} not persisted");
    }
}

/// db-results seam: record a completed `PromoteInstance` round-trip
/// (`DbResult::NodePromoted`) and flush every override buffered before the node
/// id was known — buffered values become durable exactly once (N-9). Idempotent
/// replays re-flush the same values (a same-value `SetNodeProperty` is a no-op).
pub(crate) fn handle_node_promoted(
    db_sender: &DbCommandSender,
    stamp_state: &mut StampInteractionState,
    node_id: &str,
    path_id: &str,
    instance_index: usize,
) {
    let stamp = StampRef {
        track_node_id: path_id.to_string(),
        stamp_index: instance_index,
    };
    stamp_state.mark_promoted(stamp.clone(), node_id.to_string());
    flush_buffered_overrides(db_sender, stamp_state, &stamp);
}

/// Write each buffered (`Some`) override field of `stamp` to its promoted node
/// under its matching `stamp.override.*` key. No-op when nothing was touched.
fn flush_buffered_overrides(
    db_sender: &DbCommandSender,
    stamp_state: &StampInteractionState,
    stamp: &StampRef,
) {
    let Some(ov) = stamp_state.override_for(stamp) else {
        return;
    };
    if let Some(scale) = ov.scale {
        persist_override(
            db_sender,
            stamp_state,
            stamp,
            STAMP_SCALE_KEY,
            serde_json::json!(scale),
        );
    }
    if let Some(rotation) = ov.rotation {
        persist_override(
            db_sender,
            stamp_state,
            stamp,
            STAMP_ROTATION_KEY,
            serde_json::json!(rotation),
        );
    }
    if let Some(arc_m) = ov.arc_offset_m {
        persist_override(
            db_sender,
            stamp_state,
            stamp,
            STAMP_ARC_KEY,
            serde_json::json!(arc_m),
        );
    }
}

/// Reload seam (`NodePropertiesLoaded`): a property bag whose `node_kind` is
/// `"stamp"` is a promoted stamp node coming back from the store — re-bind its
/// promoted id and load its persisted `stamp.override.*` values (overwrite: the
/// DB is the durable truth), so overrides survive restart. The identity keys
/// (`node_kind`/`path_id`/`instance_index`) are fe-database's promotion write
/// contract. See `fe-ui/src/actions/AGENTS.md` §stamped-assets.
pub(crate) fn hydrate_promoted_stamp(
    node_id: &str,
    properties: &serde_json::Value,
    stamp_state: &mut StampInteractionState,
) {
    if properties.get("node_kind").and_then(|v| v.as_str()) != Some("stamp") {
        return;
    }
    let Some(path_id) = properties.get("path_id").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(index) = properties.get("instance_index").and_then(|v| v.as_u64()) else {
        return;
    };
    let stamp = StampRef {
        track_node_id: path_id.to_string(),
        stamp_index: index as usize,
    };
    stamp_state.mark_promoted(stamp.clone(), node_id.to_string());
    if let Some(scale) = properties
        .get(STAMP_SCALE_KEY)
        .and_then(json_f32_array::<3>)
    {
        stamp_state.override_mut(&stamp).scale = Some(scale);
    }
    if let Some(rotation) = properties
        .get(STAMP_ROTATION_KEY)
        .and_then(json_f32_array::<4>)
    {
        stamp_state.override_mut(&stamp).rotation = Some(rotation);
    }
    if let Some(arc_m) = properties.get(STAMP_ARC_KEY).and_then(|v| v.as_f64()) {
        stamp_state.override_mut(&stamp).arc_offset_m = Some(arc_m as f32);
    }
}

/// Parse a JSON `[f64; N]` array into `[f32; N]`; `None` on shape mismatch.
fn json_f32_array<const N: usize>(value: &serde_json::Value) -> Option<[f32; N]> {
    let arr = value.as_array()?;
    if arr.len() != N {
        return None;
    }
    let mut out = [0.0f32; N];
    for (slot, v) in out.iter_mut().zip(arr) {
        *slot = v.as_f64()? as f32;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender() -> (DbCommandSender, crossbeam::channel::Receiver<DbCommand>) {
        let (tx, rx) = crossbeam::channel::bounded(8);
        (DbCommandSender(tx), rx)
    }

    #[test]
    fn select_flags_first_unpromoted_stamp_for_promotion() {
        let mut st = StampInteractionState::default();
        handle_select_stamp(&mut st, "track-1".into(), 3);
        assert_eq!(st.selected(), Some(&StampRef::new("track-1".into(), 3)));
        // First select of an un-promoted stamp queues a promotion.
        let pending = st.take_pending_promotion();
        assert_eq!(pending, Some(StampRef::new("track-1".into(), 3)));
        // Drained once.
        assert!(st.take_pending_promotion().is_none());
    }

    #[test]
    fn reselecting_same_stamp_is_idempotent() {
        let mut st = StampInteractionState::default();
        handle_select_stamp(&mut st, "t".into(), 0);
        let _ = st.take_pending_promotion();
        handle_select_stamp(&mut st, "t".into(), 0);
        assert!(
            st.take_pending_promotion().is_none(),
            "re-selecting the same stamp must not re-queue promotion"
        );
    }

    #[test]
    fn selecting_already_promoted_stamp_does_not_promote_again() {
        let mut st = StampInteractionState::default();
        st.mark_promoted(StampRef::new("t".into(), 5), "node-5".into());
        handle_select_stamp(&mut st, "t".into(), 5);
        assert!(
            st.take_pending_promotion().is_none(),
            "already-promoted stamp must not re-promote (idempotent, N-9)"
        );
        assert_eq!(st.selected(), Some(&StampRef::new("t".into(), 5)));
    }

    #[test]
    fn scale_override_buffers_pre_promotion_and_records_in_state() {
        let (tx, rx) = sender();
        let mut st = StampInteractionState::default();
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 2, [2.0, 2.0, 2.0]);
        // Recorded in-state...
        assert_eq!(
            st.override_for(&StampRef::new("t".into(), 2))
                .and_then(|o| o.scale),
            Some([2.0, 2.0, 2.0])
        );
        // ...but NOT sent to the DB (no promoted node id yet — N-9).
        assert!(rx.try_recv().is_err(), "no store write before promotion");
    }

    #[test]
    fn scale_override_persists_after_promotion() {
        let (tx, rx) = sender();
        let mut st = StampInteractionState::default();
        let stamp = StampRef::new("t".into(), 2);
        st.mark_promoted(stamp.clone(), "node-x".into());
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 2, [1.5, 1.0, 1.5]);
        match rx.try_recv() {
            Ok(DbCommand::SetNodeProperty {
                node_id,
                key,
                value,
            }) => {
                assert_eq!(node_id, "node-x");
                assert_eq!(key, STAMP_SCALE_KEY);
                assert_eq!(value, serde_json::json!([1.5, 1.0, 1.5]));
            }
            other => panic!("expected SetNodeProperty, got {other:?}"),
        }
    }

    #[test]
    fn rotation_override_persists_quaternion_after_promotion() {
        let (tx, rx) = sender();
        let mut st = StampInteractionState::default();
        let stamp = StampRef::new("t".into(), 0);
        st.mark_promoted(stamp.clone(), "n0".into());
        use std::f32::consts::FRAC_1_SQRT_2;
        handle_set_stamp_rotation(
            &tx,
            &mut st,
            "t".into(),
            0,
            [0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2],
        );
        assert_eq!(
            st.override_for(&stamp).and_then(|o| o.rotation),
            Some([0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2])
        );
        match rx.try_recv() {
            Ok(DbCommand::SetNodeProperty { node_id, key, .. }) => {
                assert_eq!(node_id, "n0");
                assert_eq!(key, STAMP_ROTATION_KEY);
            }
            other => panic!("expected SetNodeProperty, got {other:?}"),
        }
    }

    #[test]
    fn overrides_are_sparse_untouched_axis_stays_none() {
        let (tx, _rx) = sender();
        let mut st = StampInteractionState::default();
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 1, [3.0, 3.0, 3.0]);
        let ov = st.override_for(&StampRef::new("t".into(), 1)).unwrap();
        assert!(ov.scale.is_some());
        assert!(ov.rotation.is_none(), "rotation untouched → stays None");
        assert!(ov.arc_offset_m.is_none(), "arc untouched → stays None");
    }

    #[test]
    fn promotion_override_flush_and_hydration_round_trip_compose() {
        let (db_sender, rx) = sender();
        let stamp = StampRef::new("path-1".into(), 3);
        let scale = [1.5, 1.0, 1.5];
        let mut live = StampInteractionState::default();

        handle_select_stamp(&mut live, "path-1".into(), 3);
        handle_set_stamp_scale(&db_sender, &mut live, "path-1".into(), 3, scale);
        assert_eq!(live.selected(), Some(&stamp));
        assert_eq!(live.override_for(&stamp).and_then(|o| o.scale), Some(scale));
        assert!(
            live.take_pending_promotion().is_some(),
            "unpromoted override requests lazy promotion"
        );
        let _promotion_commands: Vec<_> = rx.try_iter().collect();

        handle_node_promoted(&db_sender, &mut live, "path-1#inst-3", "path-1", 3);
        assert_eq!(
            live.promoted_node_id(&stamp),
            Some("path-1#inst-3"),
            "promotion binds the same stamp identity"
        );
        let flushed: Vec<_> = rx.try_iter().collect();
        assert!(flushed.iter().any(|command| matches!(
            command,
            DbCommand::SetNodeProperty {
                node_id,
                key,
                value,
            } if node_id == "path-1#inst-3"
                && key == "stamp.override.scale"
                && value == &serde_json::json!(scale)
        )));

        let mut hydrated = StampInteractionState::default();
        hydrate_promoted_stamp(
            "path-1#inst-3",
            &serde_json::json!({
                "node_kind": "stamp",
                "path_id": "path-1",
                "instance_index": 3,
                "stamp.override.scale": scale,
            }),
            &mut hydrated,
        );
        handle_select_stamp(&mut hydrated, "path-1".into(), 3);
        assert_eq!(hydrated.selected(), Some(&stamp));
        assert_eq!(hydrated.promoted_node_id(&stamp), Some("path-1#inst-3"));
        assert_eq!(
            hydrated.override_for(&stamp).and_then(|o| o.scale),
            Some(scale)
        );
    }

    #[test]
    fn node_promoted_flushes_pre_buffered_overrides() {
        let (tx, rx) = sender();
        let mut st = StampInteractionState::default();
        // Buffered pre-promotion: scale + arc touched, rotation untouched.
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 2, [2.0, 2.0, 2.0]);
        st.set_arc_offset(
            &StampRef {
                track_node_id: "t".into(),
                stamp_index: 2,
            },
            7.5,
        );
        assert!(rx.try_recv().is_err(), "nothing persists before promotion");
        // The DbResult::NodePromoted arm lands → mark + flush exactly the
        // touched fields (scale first, then arc; rotation never written).
        handle_node_promoted(&tx, &mut st, "node-2", "t", 2);
        match rx.try_recv() {
            Ok(DbCommand::SetNodeProperty {
                node_id,
                key,
                value,
            }) => {
                assert_eq!(node_id, "node-2");
                assert_eq!(key, STAMP_SCALE_KEY);
                assert_eq!(value, serde_json::json!([2.0, 2.0, 2.0]));
            }
            other => panic!("expected buffered scale flush, got {other:?}"),
        }
        match rx.try_recv() {
            Ok(DbCommand::SetNodeProperty { key, value, .. }) => {
                assert_eq!(key, STAMP_ARC_KEY, "arc override flushes under its key");
                assert_eq!(value, serde_json::json!(7.5));
            }
            other => panic!("expected buffered arc flush, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "untouched rotation never writes");
        assert_eq!(
            st.promoted_node_id(&StampRef {
                track_node_id: "t".into(),
                stamp_index: 2,
            }),
            Some("node-2")
        );
    }

    #[test]
    fn node_promoted_with_no_buffered_overrides_only_marks() {
        let (tx, rx) = sender();
        let mut st = StampInteractionState::default();
        handle_node_promoted(&tx, &mut st, "node-0", "t", 0);
        assert!(rx.try_recv().is_err(), "no overrides → no property writes");
        assert!(st.is_promoted(&StampRef {
            track_node_id: "t".into(),
            stamp_index: 0,
        }));
    }

    // --- hydration on reload (T2 integration 3d) ---

    #[test]
    fn hydrate_promoted_stamp_rebinds_id_and_loads_overrides() {
        let mut st = StampInteractionState::default();
        // A stale in-memory value the DB truth must overwrite.
        let stamp = StampRef {
            track_node_id: "path-1".into(),
            stamp_index: 3,
        };
        st.override_mut(&stamp).scale = Some([9.0, 9.0, 9.0]);
        let props = serde_json::json!({
            "node_kind": "stamp",
            "path_id": "path-1",
            "instance_index": 3,
            "stamp.override.scale": [1.5, 1.0, 1.5],
            "stamp.override.rotation": [0.0, 1.0, 0.0, 0.0],
            "stamp.override.arc_m": 4.25
        });
        hydrate_promoted_stamp("path-1#inst-3", &props, &mut st);
        assert_eq!(st.promoted_node_id(&stamp), Some("path-1#inst-3"));
        let ov = st.override_for(&stamp).expect("overrides hydrated");
        assert_eq!(ov.scale, Some([1.5, 1.0, 1.5]), "DB overwrites in-memory");
        assert_eq!(ov.rotation, Some([0.0, 1.0, 0.0, 0.0]));
        assert_eq!(ov.arc_offset_m, Some(4.25));
    }

    #[test]
    fn hydrate_ignores_non_stamp_and_malformed_bags() {
        let mut st = StampInteractionState::default();
        // Not a stamp node.
        hydrate_promoted_stamp(
            "n1",
            &serde_json::json!({ "node_kind": "sculpt_region" }),
            &mut st,
        );
        // Stamp missing its identity keys.
        hydrate_promoted_stamp("n2", &serde_json::json!({ "node_kind": "stamp" }), &mut st);
        hydrate_promoted_stamp(
            "n3",
            &serde_json::json!({ "node_kind": "stamp", "path_id": "p" }),
            &mut st,
        );
        assert!(
            st.overrides_for_track("p").is_empty()
                && !st.is_promoted(&StampRef {
                    track_node_id: "p".into(),
                    stamp_index: 0,
                }),
            "malformed bags must not touch the state"
        );
    }

    #[test]
    fn overrides_for_track_is_index_sorted_and_track_scoped() {
        let (tx, _rx) = sender();
        let mut st = StampInteractionState::default();
        handle_set_stamp_scale(&tx, &mut st, "a".into(), 5, [5.0, 5.0, 5.0]);
        handle_set_stamp_scale(&tx, &mut st, "a".into(), 1, [1.0, 1.0, 1.0]);
        handle_set_stamp_scale(&tx, &mut st, "b".into(), 0, [9.0, 9.0, 9.0]);
        let list = st.overrides_for_track("a");
        assert_eq!(list.len(), 2, "other tracks excluded");
        assert_eq!(list[0].0, 1, "sorted by stamp index");
        assert_eq!(list[1].0, 5);
    }

    #[test]
    fn reflow_shifts_surviving_overrides_and_drops_deleted() {
        let (tx, _rx) = sender();
        let mut st = StampInteractionState::default();
        // Overrides on stamps 0, 2, 3 of track "t".
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 0, [1.0, 1.0, 1.0]);
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 2, [2.0, 2.0, 2.0]);
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 3, [3.0, 3.0, 3.0]);
        // Delete stamp 1 → 2 and 3 shift down to 1 and 2; 0 unchanged; 1 absent.
        st.reflow_after_delete("t", 1);
        assert!(st.override_for(&StampRef::new("t".into(), 0)).is_some());
        assert_eq!(
            st.override_for(&StampRef::new("t".into(), 1))
                .and_then(|o| o.scale),
            Some([2.0, 2.0, 2.0]),
            "the stamp formerly at index 2 keeps its override at its new index 1"
        );
        assert_eq!(
            st.override_for(&StampRef::new("t".into(), 2))
                .and_then(|o| o.scale),
            Some([3.0, 3.0, 3.0])
        );
        assert!(
            st.override_for(&StampRef::new("t".into(), 3)).is_none(),
            "the top index is vacated after the shift"
        );
    }

    #[test]
    fn reflow_dropping_deleted_index_removes_its_override() {
        let (tx, _rx) = sender();
        let mut st = StampInteractionState::default();
        handle_set_stamp_scale(&tx, &mut st, "t".into(), 2, [5.0, 5.0, 5.0]);
        st.reflow_after_delete("t", 2);
        assert!(
            st.override_for(&StampRef::new("t".into(), 2)).is_none(),
            "the deleted stamp's own override is dropped"
        );
    }

    #[test]
    fn reflow_leaves_other_tracks_untouched() {
        let (tx, _rx) = sender();
        let mut st = StampInteractionState::default();
        handle_set_stamp_scale(&tx, &mut st, "a".into(), 3, [1.0, 1.0, 1.0]);
        handle_set_stamp_scale(&tx, &mut st, "b".into(), 3, [9.0, 9.0, 9.0]);
        st.reflow_after_delete("a", 0);
        // Track "a" index 3 → 2; track "b" index 3 unchanged.
        assert_eq!(
            st.override_for(&StampRef::new("a".into(), 2))
                .and_then(|o| o.scale),
            Some([1.0, 1.0, 1.0])
        );
        assert_eq!(
            st.override_for(&StampRef::new("b".into(), 3))
                .and_then(|o| o.scale),
            Some([9.0, 9.0, 9.0])
        );
    }

    #[test]
    fn reflow_shifts_selection_and_promotion_maps() {
        let mut st = StampInteractionState::default();
        st.mark_promoted(StampRef::new("t".into(), 4), "node-4".into());
        handle_select_stamp(&mut st, "t".into(), 4);
        let _ = st.take_pending_promotion();
        // Delete a lower index → the promoted node id follows to the new index 3.
        st.reflow_after_delete("t", 1);
        assert_eq!(
            st.promoted_node_id(&StampRef::new("t".into(), 3)),
            Some("node-4")
        );
        assert_eq!(st.selected(), Some(&StampRef::new("t".into(), 3)));
    }

    #[test]
    fn reflow_deleting_selected_stamp_clears_selection() {
        let mut st = StampInteractionState::default();
        handle_select_stamp(&mut st, "t".into(), 2);
        st.reflow_after_delete("t", 2);
        assert!(
            st.selected().is_none(),
            "deleting the selected stamp clears the selection"
        );
    }
}
