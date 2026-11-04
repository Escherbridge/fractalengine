---
type: ProgressCheckpoint
date: 2026-11-04
status: in_progress
title: Current completion state and next actions
last_verified_utc: 2026-11-04T22:10:21.996416+00:00
---

# Current completion state

The goal service is active. Its attached objective matches `goal-service.txt`.
This checkpoint holds the latest state. Earlier entries are preserved verbatim
in [checkpoint-history.md](./checkpoint-history.md); they describe historical
states, not current build or acceptance claims.

- **Main source:** `925d7cba`. Reviewed 47-source native/layout composition is committed in `8968d918`. Reviewed toolchain follow-ups are `1b477696`, `86c53c14`, `4add551c`, and precise BCrypt ULONG bound `925d7cba`. Live acceptance remains distinct.
- **Build:** Combined Rust application/helpers and native observer staged successfully from `925d7cba` in 12.7 seconds at `.qa/stage/925d7cba-20261104150814356`; session 40403 terminal exit 0, zero Rust/MSVC compiler warning or error diagnostics, and application/helper hashes verified. Earlier attempts 71477, 73746 and 99364 failed at toolchain lookup, SDK COM prerequisites and unavailable MAXULONG respectively. Raw logs/receipts remain in `evidence/build-attempts/`; no failed attempt published a stage manifest. Final integrated sweep has not run.
- **Owned apps:** QA42 disposable terrain fixture is live at `.qa/run/20261104-150850-588`, PID 110832. Launch log confirms completed preparation in 15.2 seconds; session 18216 still has an open inherited-output handle. QA36 through QA41 are stopped; retained roots untouched. Inspect current process/run state before further lifecycle actions.
- **Committed evidence:** QA34/35 in `3062a9ab`, QA36 in `9df6a048`, QA37 in `35485826`, and reviewed QA38 through QA41 in `9a9450b9`. All 331 packet blobs were staged byte-for-byte and the session log excluded. Focused packets do not count as FULL passes.
- **QA38:** independently reviewed as a faithful failed run; [review](./reviews/qa38-full-evidence-review.md), [manifest](../../../qa/passes/38/packet-manifest.json). Manifest SHA256 `133024b57f5a2eaf59e3ed27f88a70b1e5ecc8fe9e3f6980dd7e25f4b8f4378a`; 152 payloads and 16 actual screenshots. Publication bookkeeping is approved and committed. Its verdict stays `FAILED_NEW_S1_QA0071`.
- **QA39:** independently approved as focused normal-update evidence; [review](./reviews/qa39-full-evidence-review.md); publication bookkeeping approved and committed; [manifest](../../../qa/passes/39/packet-manifest.json), SHA256 `ec654fd786690db3bc28b41f23681db4356ee2fe9021a2de3d24e0fc315e1ec7`; 26 payloads and one screenshot. Fresh fixture bindings and one normal scoped terrain update succeeded, with complete requested readback and byte-identical control Petal B. This does not prove expiry, revocation, moved-scope, rollback, queue bounds, concurrency or a particular runtime reader mode.

- **QA40:** failed before input: state and an inspected nonblack screenshot succeeded, but three separate Owner UI-tree reads returned `ui_tree_not_ready`. [Manifest](../../../qa/passes/40/packet-manifest.json) SHA256 `e548af07241ec36ba0e6c6af5a044340d0778e9aa283fe74516c893f4116698b`, 17 payloads and one screenshot. [Independent review](./reviews/qa40-full-evidence-review.md) approves the failed-run evidence; new S2 QA0072 remains open. Native handoff could clear shared completed output before the unordered Owner service. Explicit ordering correction `d8da3a46` is independently source-approved; later QA41 records matching live recovery. QA40 itself reached no brush or physical-control acceptance.

- **QA41:** frozen focused packet [manifest](../../../qa/passes/41/packet-manifest.json), SHA256 `3bf5ba5a56cc2dc0a8825a4930d0d6a20c2198ab24ba35b02b39edd91175e359`, 132 payloads, 17 screenshots and 106 original anchors. Owner tree recovered with 102 physical control-ID joins; Circle chrome checks and one legitimate placement passed recorded comparisons. A valid property draft survived resizing and saved the exact string after one narrow-view wheel step. Zero measured narrow violations do not establish visual acceptance: Inspector body is 52 physical pixels, Add has four-pixel bottom clipping, and ruler overlaps chrome. Two completed navigation timeouts were not replayed. [Independent review](./reviews/qa41-full-evidence-review.md) approves this bounded evidence with the mandatory [summary erratum](./evidence/qa41-summary-erratum.json), which corrects the unsupported-command artifact statement without changing the frozen packet.

## Acceptance findings

The register has 63 records. Sixteen remain in open/reproduction states: three
S1, eight S2 and five S3. Source fixes and focused results do not waive remaining
acceptance. Read `qa/findings.jsonl` for exact statuses and evidence.

- **QA0071, S1:** QA38 armed Circle, then clicked actual Back once. Navigation left the petal and also persisted an unintended 24-vertex region and node. Configuration and arming had left terrain unchanged. Shared-pointer source is corrected and reviewed in `f9365d98`; QA41 records unchanged raw terrain/hierarchy after exact Circle/Back, hint and one ancestor, plus exactly one intended scene Circle and node. Independent focused review is approved; broader tools/gestures and journal comparisons remain required. The historical QA0024 trigger remains unproved.
- **QA0039 and layout:** QA38 measured actual 20-pixel side dividers across six size/scale states and restored 260/360 preferences. A stable camera baseline preserved pose, selection and full transform through repeated drags. The initial changing camera baseline remains preserved as a failed comparison. Other failures were 101 small targets at 0.75x, ten Inspector clipping cases at 1024x720/2x, and four six-pixel Results column handles. Source correction `6f473264` compiled and QA41 reduced the observed 0.75-scale failures to 13 in its different populated scene; twelve narrow tree targets and one 19.999992-pixel divider remain. Results controls and broad interaction acceptance remain pending.
- **Inspector:** the old bottom scroll offset was confirmed at 3530.5. Add Property was visible but disabled in the empty form; do not claim it was vertically unreachable. Horizontal overflow and a reported 128-pixel viewport versus 58-pixel scissor were real distinct issues.
- **Visuals:** opaque active-Petal headers were readable in QA38, including wrapping. The ruler still overlaps hierarchy/footer at high scale and needs separately owned viewport integration. Overall UI polish is unfinished.
- **Terrain:** QA36/37 independently verified partial coverage withholding, genuine zero, analytic 200-cubic-metre fill and 100-cubic-metre cut, atomic saved pairs, byte-stable histories through observed departures/returns, material and a four-cubic-metre sampled solid-box overlap. Retained restart, late receipts, rollback, authority transitions, scale/general geometry and final-build coverage remain open.
- **Native interaction:** foundation and NAV-1B v3 are integrated. Fresh accepted-role generation, permanent alias retirement, immutable one-second proofs, actual provider transformation and typed callbacks have source approval. Combined compilation passed in `5a9eeb93`; real Viewer/Editor/provider execution is pending. Operator staging, durable attempt ledger and retained COM helper are source-frozen as a 28-file tooling slice in `.omc/worktrees/qa-native-navigation-helper`, whose v3 passed both full reviews and exact integration in `8968d918`; reviewed toolchain follow-ups through `925d7cba` compile successfully; actual execution remains pending; capture remains unproved and must refuse.

## Next actions

1. Complete QA42 fresh launch and inspect matching screenshots/actions against successful925d7cba stage. Preserve failed/successful build evidence; no final integrated sweep has run.
2. Run fresh disposable native/input-off and ordinary input-enabled fixtures. Verify actual helper observation/invocation, native/ordinary paired restart and confirmed historical stage pruning without weakening Owner gates.
3. Inspect matching screenshots/actions for the19-file layout correction: tree horizontal reachability, physical dividers, compact Inspector valid forms, Results, scene-contained ruler disjoint from real hints, and first camera/vision apply/restore behavior. Recheck Circle chrome protection after the readonly geometry accessor.
4. Keep reviewed QA38 through QA41 publication `9a9450b9`, all raw evidence and external erratum. Focused cases remain separate from full acceptance and historical journal proof.
5. Continue every remaining item in [plan.md](./plan.md), including retained-store require-existing startup, rich data/groups/permissions, complete catalog, five live UX audits, two clean FULL passes, the single final integrated sweep, exact-SHA CI and the entire verified tagged release.

Only root runs Cargo, apps and process lifecycle operations. Authors use isolated
worktrees. No final test sweep, push, CI, tag or release has been performed.
