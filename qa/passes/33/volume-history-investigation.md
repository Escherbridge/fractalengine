---
type: FocusedHistoryProducerInvestigation
date: 2026-11-03
source: 24b5660c
execution_status: SOURCE_AND_SAVED_EVIDENCE_ONLY
finding: QA-0069
---

# Earthwork volume property churn during Petal re-entry

The four newly observed history facts have a direct production producer: the terrain bake publishes cut/fill numbers while resident terrain coverage is absent or incomplete, and the UI persists every changed numeric pair as two independent ordinary property commands. Navigation clears the height field and changes terrain revision, so this path can temporarily replace an established volume with zero and later restore it as chunks return. The saved facts and navigation timing fit this exact sequence. Runtime sample coverage at those particular frames was not captured, so this is a source-supported mechanism plus matching durable evidence, not a measured reconstruction of every intermediate terrain sample.

Main source stayed frozen; root owns the live QA33 app. No app/API/DB/OS/process/network calls, builds or tests were made. Only this report was written.

## Durable timeline

Node `01M6D1HAXR53SVHRX1360QZHWC` is `Earthwork raise r1` in PetalA `01M6D0VZJ6383Q4HNNET8RSEYF`. Saved QA33 full/tail history reports current_version106/current_state_verifiedtrue. All four facts use the same actual local DID, are ordinary `property_set`, and retain the same node/Petal/TRS/material/region identity.

| Version | UTC record time | Property value | Effect |
|---:|---|---|---|
|102|2026-11-03 03:36:07.4038226|fill_volume_m3=38.83056640625|Previous retained nonzero fill; cut already0.|
|103|2026-11-03 11:39:47.1050875|cut_volume_m3=0|Same-value cut fact; fill still38.83056640625.|
|104|2026-11-03 11:39:47.1808408|fill_volume_m3=0|Transient complete node state has cut0/fill0.|
|105|2026-11-03 11:39:48.0816131|cut_volume_m3=0|Another same-value cut fact.|
|106|2026-11-03 11:39:48.1487991|fill_volume_m3=38.83056640625|Final property bag returns to its previous values.|

Sources: `qa/passes/33/owner-raise-history-tail-initial.jsonl`, `owner-raise-history-full-initial.jsonl`, `owner-raise-row-initial.jsonl`. QA32 final saved history already reports current_version106, although its first100-entry page stops at100 and next_after_version100. Thus these are newly observed tail facts, **not newly created by QA33 startup**. Root observed them stable at the first QA33 reads.

QA32 `owner-leave-path-petal.jsonl` records the actual `Back to petal list` click applied with25wait frames. `owner-return-petal-a.jsonl` records a504 input timeout and subsequent log events; root separately observed the PetalA return applied. Treat it as applied/unknown completion, never repeat it merely because504 occurred. The durable fact times coincide with this navigation window according to root's retained execution context; the input JSON itself has no wall-clock timestamp or height-coverage census. The present investigation does not infer exact sample readiness from the timeout.

## Producer, scheduling and coverage loss

`fe-terrain/src/petal_binding.rs:203â€“205` installs a terrain assignment, changes active Petal/config and increments `active.revision`. At:329â€“338 it despawns terrain-derived entities, clears `TerrainHeightField`, and resets residency. The terrain Update chain (`terrain_plugin.rs:610â€“628`) runs assignment â†’ LOD/fetch/spawn â†’ overlays â†’ `bake_earthwork_regions`. Chunk IO/materialization is asynchronous, so a cleared field can persist when the bake runs in that revision.

`bake_earthwork_regions` (`terrain_plugin.rs:1845â€“1977`) records revision_changed immediately, builds the base sampler from `height_field.height_at`, and publishes one `EarthworkVolumeReport` per configured region when **revision_changed OR baked_any**. It does not require full footprint sample coverage. Its missing-field warning only checks whether the Resource is absent; the plugin normally initializes an existing but empty field, which passes that warning condition. Missing mesh vertices are skipped during baking, and later available chunks cause another bake/report.

`EarthworkRegion::cut_fill` (`sculpt/mod.rs:462`) calls `sculpt_cut_fill`, which uses `cut_fill_volume` (:330â€“359). That integrator initializes cut/fill0 and adds a cell only when both base and proposed samples exist. Uncovered cells are silently skipped; no expected/sampled cell count or completeness bit is returned. Therefore zero can mean either a real fully covered zero-change surface **or no samples at all**. Partially covered footprints can similarly produce a partial volume. `TerrainHeightField::clear`, `is_empty`, and `height_at` (`fe-renderer/src/terrain_height.rs:83â€“116`) show why Resource presence or a nonempty tile count alone does not establish coverage of this particular region. SceneAnalysisSnapshot/report-selection staleness from QA30 is unrelated to this producer: it samples the pristine height field directly.

The shared `EarthworkVolumeReport` (`fe-renderer/src/terrain_overlay.rs:107`) carries only petal_id,region_id,cut_m3,fill_m3. It carries no terrain revision/source identity, world-scale validity, finite/sample coverage or explicit unavailable state. Transient unavailable data is therefore indistinguishable from a valid numeric result at the receiving seam.

## UI persistence and exact four-fact mechanism

`EarthworkNodeMap` (`fe-ui/src/actions/terrain_proposal.rs:427â€“488`) maps **region_id alone** to node ID and caches the last-sent pair. `hydrate_earthwork_region` (:555â€“578) seeds that pair from stored node properties; no actual Petal identity is in the map key. The map is a Resource and this path does not clear it merely on Petal navigation.

`persist_earthwork_volumes` (:584) runs in Update (`plugin.rs:952â€“958`). Its helper (:595â€“633):

1. Resolves only report.region_id. It never checks report.petal_id, actual active navigation, node ancestry, current region/config source or terrain revision/readiness.
2. Compares the entire cut/fill pair with cached values (:607).
3. If either differs, sends **both** keys as independent `SetNodeProperty` commands (:611â€“625), even if one value is unchanged.
4. Marks the pair sent once both channel sends succeed (:630â€“631). The comment calls this persisted, but channel acceptance is not an actual DB receipt. A first successful send followed by failure/refusal of the second can leave a partial pair.

This yields the observed order directly: cached(0,38.83056640625) â†’ bake numeric(0,0) sends cut0 then fill0 â†’ later bake(0,38.83056640625) sends cut0 then fill38.83056640625. It explains103/105 same-value cut entries without supposing manual edits, timestamp corruption or a different local identity. Lack of Petal binding is an additional source defect: equal region IDs in different Petals could route a report to the wrong known node. No actual cross-Petal wrong-node write is proved by these saved facts.

The DB correctly records the operations it receives. `handlers/entity_property.rs:14â€“58` always constructs PropertySet and calls ordinary `node_history::commit_mutation`; it has no same-value early return. The common helper writes materialized state, intention and INSERT-only journal facts atomically for **each command**. Two property commands are two saves, with an observable intermediate pair. The rich journal is honestly auditing unwanted producer writes; rewriting or deleting those retained facts would hide the defect.

## Correctness impact and narrow repair requirements

Derived volume persistence is intentional when a valid recalculation actually changes the authoritative current result. It is not a measurement-append time series: these keys are current derived object properties, and their journal timestamps are DB record/commit time. Navigation/reload alone should not generate fictitious numeric observations while data is unavailable. Current behavior pollutes replay with transient false-zero/partial states and redundant saves; aggregates can briefly report zero and lose pair consistency. The final restored row and current_state_verifiedtrue do not erase the intervening history facts or prove those zeros were meaningful measurements. This is a bounded lifecycle/persistence defect, not evidence of arbitrary store corruption.

Required fix, preserving real zero and existing useful architecture:

- Extend the existing bake-report result with explicit readiness/coverage evidence for the **same integration lattice**: expected eligible footprint cells, finite valid base/proposed samples, world-scale validity, and exact terrain/Petal/revision/source/region witness. No samples or incomplete/nonfinite coverage must remain unavailable/partial and must not overwrite the last confirmed complete pair. Full valid coverage with actual delta0 is a legitimate zero and must be allowed. Checking only field existence, any tile, any vertex or cut+fill>0 is insufficient.
- Recompute when coverage/source becomes ready even if no mesh vertex happens to change; retain a bounded source/revision demand rather than relying solely on baked_any. A new valid terrain source may legitimately change the result without a user edit. Preserve the planning-grade integration/tolerance contract and one common sampler; do not add a separate volume formula.
- Bind map/request ownership to actual Petal+region+backing node and source continuity. Check current nav/config/region/source at admission and completion. A late report from a previous Petal/revision cannot write under a current mapping. Stored ancestry and actual Editor authority remain DB-time checks, not UI role labels.
- Coalesce pure recalculation/read intent; distinguish queued, submitted, unknown and confirmed outcomes. Advance the cached confirmed pair **only on the matching actual persisted receipt**, not send/try_send acceptance or an unrelated hydration. Do not auto-replay an accepted unknown mutation. Full/Disconnected should be honest and nonblocking.
- Persist the two keys as one checked atomic save, with no partially written pair and no unchanged-pair save. Keep retained old facts intact. The confirmed resulting current state should drive ordinary property hydration/report caches.

## Existing atomic seam to reuse

No new general mutation protocol is needed. `fe-runtime/src/editor_batch.rs:109â€“119` already supports `EditorTargetMutation::Properties(Vec<EditorPropertyEdit>)`; one captured node can carry both Set edits. `ReadEditorBatch` with target Nodes/read_target supplies exact stored scope/node/history/raw state/Petal snapshot; `CommitCheckedEditorBatch` performs the actual Editor/publisher, schema, full snapshot, ancestry and Petal fences. `handlers/editor_batch/prepare.rs:287â€“315` accepts1..32distinct property edits and prepares all fields before a single transaction; `editor_batch/mod.rs:219` commits the whole prepared batch and returns one exact receipt. Existing ResolveCheckedEditorBatch can reconcile the exact original request without coordinate guesses or replay.

Use a separate bounded background earthwork request owner/correlation prefix; do not commandeer the user's selected-set workflow or create a fictitious Undo entry merely because it is a derived recalculation. The backend batch fact grouping can still expose one completed Save with two field audit facts. The runtime sampled terrain/source witness remains an additional use-time producer check: DB node capture by itself cannot certify missing terrain sample coverage. Preserve exact source continuity while awaiting capture/commit and confirm only the matching whole receipt. This is a repair proposal requiring source review and live no-new-history-on-navigation validation after root stops QA33; no implementation was performed here.
