//! Analytics-first terrain proposal editor state (terrain_editor_overhaul FR-5):
//! a fe-ui-LOCAL record set of NON-destructive proposed terrain edits (raise /
//! lower / flatten / ramp / slope / pad / cut / fill volumes). These are a
//! mirror of the persisted JSON contract — NOT `fe_terrain::TerrainProposal`
//! (fe-ui must not depend on fe-terrain). Persisted additively under the petal
//! terrain config's `proposals` key. See `fe-ui/src/AGENTS.md`
//! §terrain-proposal-editor.

use bevy::prelude::Resource;

/// One proposed terrain operation kind. Serde `snake_case` matches the JSON
/// contract (`"raise"|"lower"|"flatten"|"ramp"|"slope"|"pad"|"cut"|"fill"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOp {
    Raise,
    Lower,
    Flatten,
    Ramp,
    Slope,
    Pad,
    Cut,
    Fill,
}

/// A single proposed terrain edit. `footprint` is a closed XZ polygon (world
/// units); `target_height`/`delta` are op-dependent (flatten uses a target,
/// raise/lower use a delta). Serde field names match the persisted JSON
/// contract exactly. `target_height`/`delta` omit-when-`None`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProposalRecord {
    pub id: String,
    pub op: ProposalOp,
    pub footprint: Vec<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f32>,
}

/// Editor state for the proposed-overlay terrain editor (FR-5). Holds the live
/// proposal set, the current selection, and the armed brush op. In-memory
/// working state; the persisted source of truth is the petal terrain config's
/// `proposals` block (round-tripped by `actions::terrain_proposal`).
#[derive(Resource, Default)]
pub struct ProposalEditState {
    pub proposals: Vec<ProposalRecord>,
    pub selected: Option<String>,
    pub active_op: Option<ProposalOp>,
    /// Monotonic id counter (no `uuid`/`rand` dep — mirrors `gis::next_pen_correlation_id`).
    next_id: u64,
    /// `true` once `proposals` reflects the active petal's persisted set
    /// (via `replace_all`) — `false` is the "don't trust me yet" state a
    /// not-yet-loaded or just-switched-away-from petal leaves behind.
    /// `actions::terrain_proposal::embed_proposals` must not treat `false`
    /// as "there is nothing to lose" (data-loss guard,
    /// `ui_semantics_unification_20260808` finding #1). See §terrain-proposal-editor.
    pub hydrated: bool,
    /// Ids deleted locally while `hydrated` is `false` (finding #13 fix,
    /// `ui_semantics_unification_20260808`): the unhydrated merge in
    /// `actions::terrain_proposal::embed_proposals` excludes these ids from
    /// the doc's existing entries even though the mirror has no authoritative
    /// knowledge of whether they were ever persisted. Without this, deleting
    /// an id that predates hydration was silently resurrected by the
    /// keep-every-existing-doc-entry union-merge rule. Cleared by
    /// `replace_all`/`reset_for_petal_switch` — a hydrated mirror needs no
    /// tombstones, its wholesale replace already reflects every deletion.
    pub(crate) pending_deletes: std::collections::HashSet<String>,
}

impl ProposalEditState {
    /// Append a new proposal with a freshly-minted id; returns the new id.
    pub fn push_new(
        &mut self,
        op: ProposalOp,
        footprint: Vec<[f32; 2]>,
        target_height: Option<f32>,
        delta: Option<f32>,
    ) -> String {
        self.next_id += 1;
        let id = format!("p{}", self.next_id);
        self.proposals.push(ProposalRecord {
            id: id.clone(),
            op,
            footprint,
            target_height,
            delta,
        });
        id
    }

    /// Bump the id counter past every `p<N>` id in `ids` — an unhydrated
    /// session must never mint an id that collides with a persisted doc entry
    /// (the merge would treat the collision as an edit and overwrite it).
    pub fn ensure_ids_beyond<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) {
        let max_seen = ids
            .into_iter()
            .filter_map(|id| id.strip_prefix('p').and_then(|n| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_seen);
    }

    /// Remove a proposal by id (clearing selection if it pointed at it).
    /// Returns `true` if a record was removed (idempotent: a repeat is a no-op).
    /// While unhydrated, also tombstones `id` (finding #13 fix) so a
    /// subsequent unhydrated `embed_proposals` merge can't resurrect it from
    /// the doc's existing entries — once hydrated, the mirror's wholesale
    /// replace already reflects the deletion, so no tombstone is needed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.proposals.len();
        self.proposals.retain(|p| p.id != id);
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        }
        if !self.hydrated {
            self.pending_deletes.insert(id.to_string());
        }
        self.proposals.len() != before
    }

    /// Replace the whole set (e.g. rehydrating from a loaded `proposals`
    /// block) and mark the mirror `hydrated` — even an empty `records` marks
    /// it hydrated (a genuinely map-less/proposal-less petal has nothing to
    /// lose, which is itself worth recording explicitly rather than leaving
    /// `hydrated` false forever).
    pub fn replace_all(&mut self, records: Vec<ProposalRecord>) {
        self.proposals = records;
        // Keep the id counter monotonic past any loaded `p{n}` ids so new pushes
        // never collide with a rehydrated one.
        let max_loaded = self
            .proposals
            .iter()
            .filter_map(|r| r.id.strip_prefix('p').and_then(|n| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_loaded);
        self.hydrated = true;
        // The mirror is authoritative again — any pre-hydration session
        // tombstones are moot (finding #13 fix).
        self.pending_deletes.clear();
    }

    /// Clear the mirror for an outgoing petal switch: the old petal's
    /// proposals must never bleed into the next one, and the mirror is
    /// untrusted (`hydrated = false`) until the new petal's terrain load
    /// calls `replace_all` again. Leaves `active_op`/`next_id` alone (tool
    /// arm state and the monotonic counter are not petal-scoped).
    pub fn reset_for_petal_switch(&mut self) {
        self.proposals.clear();
        self.selected = None;
        self.hydrated = false;
        // Tombstones are petal-scoped — the outgoing petal's deletions must
        // never suppress an id on the next petal (finding #13 fix).
        self.pending_deletes.clear();
    }
}

/// Serialize a proposal set to the persisted JSON array contract. Pure.
pub fn to_json(records: &[ProposalRecord]) -> serde_json::Value {
    serde_json::to_value(records).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

/// Parse a persisted `proposals` JSON array back into records, best-effort
/// skipping malformed entries (mirrors `gis::decode_gpx_points`'s lenient
/// decode). Pure.
pub fn from_json(value: &serde_json::Value) -> Vec<ProposalRecord> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|it| serde_json::from_value::<ProposalRecord>(it.clone()).ok())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Vec<ProposalRecord> {
        vec![
            ProposalRecord {
                id: "p1".into(),
                op: ProposalOp::Raise,
                footprint: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
                target_height: Some(12.5),
                delta: Some(3.0),
            },
            ProposalRecord {
                id: "p2".into(),
                op: ProposalOp::Flatten,
                footprint: vec![[1.0, 1.0], [2.0, 1.0], [2.0, 2.0]],
                target_height: Some(5.0),
                delta: None,
            },
        ]
    }

    #[test]
    fn op_serializes_snake_case_per_contract() {
        assert_eq!(
            serde_json::to_value(ProposalOp::Raise).unwrap(),
            json!("raise")
        );
        assert_eq!(serde_json::to_value(ProposalOp::Cut).unwrap(), json!("cut"));
        assert_eq!(
            serde_json::to_value(ProposalOp::Flatten).unwrap(),
            json!("flatten")
        );
    }

    #[test]
    fn json_roundtrip_preserves_records() {
        let records = sample();
        let value = to_json(&records);
        assert!(value.is_array());
        // Field shape matches the contract.
        assert_eq!(value[0]["id"], json!("p1"));
        assert_eq!(value[0]["op"], json!("raise"));
        assert_eq!(value[0]["footprint"][1], json!([10.0, 0.0]));
        assert_eq!(value[0]["target_height"], json!(12.5));
        assert_eq!(value[0]["delta"], json!(3.0));
        // `None` optionals are omitted.
        assert!(value[1].get("delta").is_none());
        // Full roundtrip is lossless.
        let back = from_json(&value);
        assert_eq!(back, records);
    }

    #[test]
    fn from_json_skips_malformed_and_non_arrays() {
        let value = json!([
            { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]] },
            { "id": "bad", "op": "not_an_op", "footprint": [] },
            "garbage"
        ]);
        let out = from_json(&value);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "p1");
        assert_eq!(out[0].op, ProposalOp::Raise);
        // A non-array is empty, not a panic.
        assert!(from_json(&json!({ "proposals": [] })).is_empty());
    }

    #[test]
    fn push_new_assigns_unique_ids_and_appends() {
        let mut state = ProposalEditState::default();
        let a = state.push_new(ProposalOp::Raise, vec![[0.0, 0.0]], None, Some(1.0));
        let b = state.push_new(ProposalOp::Lower, vec![[1.0, 1.0]], None, Some(-1.0));
        assert_ne!(a, b);
        assert_eq!(state.proposals.len(), 2);
        assert_eq!(state.proposals[0].id, a);
    }

    #[test]
    fn remove_is_idempotent_and_clears_selection() {
        let mut state = ProposalEditState::default();
        let id = state.push_new(ProposalOp::Pad, vec![], None, None);
        state.selected = Some(id.clone());
        assert!(state.remove(&id), "first remove reports a deletion");
        assert!(state.selected.is_none(), "selection cleared");
        assert!(!state.remove(&id), "repeat remove is a no-op");
        assert!(state.proposals.is_empty());
    }

    #[test]
    fn replace_all_keeps_id_counter_monotonic() {
        let mut state = ProposalEditState::default();
        state.replace_all(from_json(&to_json(&sample())));
        // Next mint must not collide with the loaded "p2".
        let next = state.push_new(ProposalOp::Slope, vec![], None, None);
        assert_eq!(next, "p3");
    }

    #[test]
    fn replace_all_marks_hydrated_even_when_empty() {
        let mut state = ProposalEditState::default();
        assert!(!state.hydrated, "fresh state starts unhydrated");
        state.replace_all(Vec::new());
        assert!(
            state.hydrated,
            "an empty-but-loaded petal has nothing to lose — still marked hydrated"
        );
        assert!(state.proposals.is_empty());
    }

    #[test]
    fn reset_for_petal_switch_clears_mirror_and_unmarks_hydrated() {
        let mut state = ProposalEditState::default();
        state.replace_all(sample());
        state.selected = Some("p1".into());
        assert!(state.hydrated);

        state.reset_for_petal_switch();

        assert!(
            state.proposals.is_empty(),
            "stale petal's proposals dropped"
        );
        assert!(state.selected.is_none(), "selection cleared with it");
        assert!(
            !state.hydrated,
            "mirror is untrusted again until the next petal's load rehydrates it"
        );
    }

    // --- Finding #13 fix: pre-hydration delete tombstones ---

    #[test]
    fn remove_before_hydration_tombstones_the_id() {
        let mut state = ProposalEditState::default();
        assert!(!state.hydrated);
        let id = state.push_new(ProposalOp::Raise, vec![], None, Some(1.0));
        state.remove(&id);
        assert!(
            state.pending_deletes.contains(&id),
            "unhydrated delete leaves a session tombstone"
        );
    }

    #[test]
    fn remove_after_hydration_does_not_tombstone() {
        // Once hydrated, the mirror's wholesale replace on the next persist
        // already reflects the deletion — no tombstone needed.
        let mut state = ProposalEditState::default();
        state.replace_all(sample());
        assert!(state.hydrated);
        state.remove("p1");
        assert!(
            state.pending_deletes.is_empty(),
            "hydrated deletes don't need a tombstone"
        );
    }

    #[test]
    fn replace_all_clears_pending_deletes() {
        let mut state = ProposalEditState::default();
        let id = state.push_new(ProposalOp::Raise, vec![], None, Some(1.0));
        state.remove(&id);
        assert!(!state.pending_deletes.is_empty());
        state.replace_all(Vec::new());
        assert!(
            state.pending_deletes.is_empty(),
            "rehydration clears stale session tombstones"
        );
    }

    #[test]
    fn reset_for_petal_switch_clears_pending_deletes() {
        let mut state = ProposalEditState::default();
        let id = state.push_new(ProposalOp::Raise, vec![], None, Some(1.0));
        state.remove(&id);
        assert!(!state.pending_deletes.is_empty());
        state.reset_for_petal_switch();
        assert!(
            state.pending_deletes.is_empty(),
            "tombstones are petal-scoped — must not bleed into the next petal"
        );
    }
}
