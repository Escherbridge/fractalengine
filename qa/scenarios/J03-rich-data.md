---
type: ScenarioSupplement
id: J03-rich-data
covered_by: [J03, J09, J10]
regions: [R3, R9, R10]
execution_status: NOT_RUN
source_contract_commit: 1fe217f
---

# Rich object replay and appended measurements

Require both rich object-history replay and charts of appended measurements. This follow-up adds API cases
F129–F132 and eleven F116 endpoint/tool cases. The editable intent/oracles live in
[rich-fleet.json](../../tools/qa/scenarios/rich-fleet.json); effect/report boundaries and exact CLI are in
[tools/qa/AGENTS.md](../../tools/qa/AGENTS.md#rich-data). This document is not execution evidence.

Use a fresh owned Verse/Fractal/Petal in the current isolated run, staged from reviewed integrated source.
The committed/schema/history APIs must exist; absence is BLOCKED, never a queued legacy fallback. The run needs
owner token files inside its root and the launcher's real renderable `fractalengine/assets/samples/info_panel.glb`.
Missing owned scope/assets or controlled same-root restart evidence are explicit prerequisites.

1. Validate intent: `python tools/qa/rich_data.py --scenario tools/qa/scenarios/rich-fleet.json --validate-only`.
   This executes zero operations. Run without validate-only and with `--run <absolute-owned-run>`
   `--pass-id <current-pass> --verse-id <actual-ULID> --petal-id <actual-ULID>`. Preserve real returned IDs.
   The fixed verbs upload GLB, install bounded schemas and create Alpha/Beta/Gamma. Each write needs committed
   version plus stored properties/transform and journal readback. Nested objects/arrays/null/bool and integer
   9007199254740993 must keep exact values/types. Existing evidence refuses repeated non-idempotent execution.
2. Alpha baselinev1 gains calibration atv2 and position[3,0,4] atv3. Record the actual per-object checkpoint map
   before Gamma exists. Later change Alpha gain2 to3 and position to[6,0,8], move Beta through[10,2,0] to[10,2,6],
   and Gamma from[20,0,0] to[20,0,4]. Stored XYZ travel is10/8/4m. Apply full Euler rotation[0,pi/2,0] and
   scale[2,1,0.5]; these add zero travel. Delete only the intended disposable outer key.
3. Append raw A10/11/12,B20/22/24 at T0/T1/T2, delivered T0,T2,T1. T0=1793404800000 UTC epoch ms; T1/T2 are
   10/20seconds later. Explicit affine steps read historical A v2/B v1 calibration, persist opaque provenance
   bytes referencing source reading/object version/schema revision, verify stored hashes, then append derived
   scalars only after checking the fact's node/commit/scope/after-state identity and independently declared
   calibration schema revision1 for each anchor. Merely receiving an integer revision is insufficient.
   Current gain3 must not alter the old gain2 result. This is driver computation, with no automatic
   rule engine, HLC override or declared effective-time object mutation.
4. Require six raw/six derived rows. Chronological calibrated arrays are A[21,23,25],B[9,10,11], means23/10,
   sums69/30. A total-only99 is invalid because raw fleet sum is also99. Check local/distributed per-anchor
   aggregates and MCP parity, no missing/rejected/unverified/truncated coverage, and actual shards/answerers.
   Half-open[T0,T2) gives A[21,23],B[9,10], means22/9.5. Latest stays T2 values25/11 after late T1 arrives.
5. Add optional metadata schema revision2, then its new nested value. Reject an invalid calibration and
   incompatible required-field revision3; schema/state/history versions remain unchanged. Page Alpha history
   with limit2 after_version1 and require versions2–9 exactly once. REST/MCP Alpha v3 must retain position[3,0,4],
   gain2 while current stays position[6,0,8],gain3. Version maps are per-object cuts, not global wall-time snapshots.
6. Review actual current-pass/run receipts, context JSON, measurement JSON/CSV, trajectory CSV and offline HTML.
   Event-time charts and version-state selectors use observations only. The report is not product chart UI.
   New logs/gaps remain UNKNOWN pending independent review. Merge one final reviewed candidate per checkpoint;
   retain before/after global sequence bounds: a delta over2000 can hide an evicted warning despite an empty
   warn/error list and unchanged generation, so it remains UNKNOWN rather than PASS.
   initial/restart files overlap, so never concatenate duplicate IDs or borrow prior-pass successes.
7. Main records exact owned process/root identity, stops/relaunches the same root and refreshes current.json
   with actual new process identity. Re-run identical options plus `--verify-state <absolute-context.json>`.
   This read-only hook checks unchanged source/pass/run/scope, changed recorded process identity, current/old
   state hashes, schema revisions, GLB/provenance bytes and measurement/history oracles. Pair with actual
   lifecycle evidence: a hand-edited process identity cannot prove restart. Complete J10 UI/asset/entity checks.

For product UI evidence, inspect `F ui --interactive`, select actual returned objects through the hierarchy,
and reopen `Inspector tab: Properties`/`Inspector tab: Query`. Verify nested values, per-anchor results, selected
identity and full transforms against stored readbacks; judge screenshots. After separate History/chart/editor
work integrates, source-reconcile concrete labels/actions and add distinct GUI checkpoints. Those new routes
are not established by this API-only slice. Unverified controls stay UNKNOWN/BLOCKED; F129–F132 API success
cannot complete them or silently shrink the GUI denominator.

Before every PNG, close the API Access/token dialog or clear that tab. Record token-free tree/state/commands,
then capture and judge through the shared procedure. No generated token or encoded payload belongs in evidence.
