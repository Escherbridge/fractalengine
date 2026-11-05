---
type: ProgressCheckpoint
date: 2026-11-04
status: in_progress
title: Current completion state and next actions
last_verified_utc: 2026-11-05T00:29:09.875304+00:00
---

# Current completion state

The full goal remains active. Read [plan.md](./plan.md), [spec.md](./spec.md) and
[goal-service.txt](./goal-service.txt) for the complete scope. This file describes
current state; prior snapshots are retained verbatim in [checkpoint-history.md](./checkpoint-history.md).

- **Main source:** `2dac9033`. Typed publication witness `1bdff9fd`, bounded observe diagnostic `71cd5f8e`, exact native service style `ca29b15d`, and camera command ownership `2dac9033` have independent source reviews and exact allowlist commits. No production source is pending.
- **Latest matching production stage:** `.qa/stage/2dac9033-20261104170849379`, session26788 terminal exit0,172.9 seconds, zero Rust/MSVC compiler warning or error diagnostics, application/helper hashes verified. Earlier committed stages a16eab87 (110.5s) and71cd5f8e (15.4s) also passed. All raw logs/manifests/receipts are under evidence/build-attempts.
- **Excluded stage:** a16eab87-20261104164325115/session8461 compiled uncommitted native source after a root bookkeeping-key error. No app was launched and it supplies no acceptance. Raw log/manifest/exclusion receipt are retained; proper71cd committed build superseded it.
- **Owned apps:** none. QA47 through52 are stopped. QA51/52's eight recorded identities each were absent at2026-11-05T00:20:58.1728898Z/00:21:03.5349869Z; their native workers have separate terminal receipts. These are timestamped recorded-identity checks, not a claim about every possible unrecorded descendant. Retained campaign stores remain untouched and on hold.
- **Final gates:** no final integrated test sweep, push, CI, tag or release has occurred. Production builds and focused live cases do not replace those gates.

## Reviewed focused evidence

QA38-41 are committed in9a9450b9, QA42/43 ina77cabbe, QA44 in723cb1d4 and
QA45/46 ina16eab87. All raw packet blobs were checked before their exact commits.
QA47-52 are now frozen and independently reviewed for publication; none is a FULL pass.

| Pass | Preserved outcome | Manifest SHA-256 |
| --- | --- | --- |
| 47 | Local recovery5 enqueued, numeric attempt used invalid inserted text; no valid-transform regression established | 82a0d0125e0a3a64b5ae27b08bd8710c46b13baebc20b216aa8a5df64d35c9c4 |
| 48 | Native style refusal; exact bits not yet returned | ce704ae4e7e739fa69def7b88f835636d44c61c5c6400c5b924dd7b001df8c5a |
| 49 | Valid Apply/Undo/Redo data/history; newS2 camera shortcut collision | e65926480471830c92d98a9302bcc1c228c2871b873238a02d30f3db6ac323d1 |
| 50 | Native style measured0x94000000; refused, no Invoke | 76ee90cc40cb5198fe2fdbb808c3854fca8e04a7297744c48b370ac832657d6f |
| 51 | Exact style/exstyle pass; outer zero-rectangle check refuses | 706a4d0e81340ea0830eda29389d2d1b290a1e9013e37991f2fe2daf83200598 |
| 52 | Valid single-object replay and exact camera stability through command shortcuts | 5b64160f777d41757931fae9999edffa8c4e6cd697ee91d607a75adf1cd90ddd |

Each pass lives in `qa/passes/<N>/`; full reviews are in this track's `reviews/`.
The six packets contain170 payloads plus six manifests and13 actual screenshots.
All images were inspected; the matching state and limits remain in each packet.

**QA47 interpretation:** the complete numeric draft was `0.002.5000`; the screenshot
painted a scrolled substring. `type` inserts; explicit select-all was missing.
The mandatory [clarification](./evidence/qa47-numeric-input-clarification.json) and
[source trace](./evidence/input/qa47-numeric-transform-source-trace.md) preserve that
fact without rewriting the packet. Invalid-input validation remains separate work.

**QA49/52 replay:** a valid complete draft2.500 and one Apply/Undo/Redo produced
X 0 -> 2.5 -> 0 -> 2.5 and history 4 -> 5 -> 6 -> 7, with exact original history prefix and other row
fields preserved except the intended edit sequence. QA49 also moved camera focusY
0 -> -0.74088 -> -2.25264 because CtrlLeft doubled as descent and Shift sped it up.
Reviewed2dac source reserves either-sideCtrl/Super from fly intent, retainsQ and
plain navigation/Shift, and preserves mouse/capture/easing/terrain. QA52 independently
verified all seven complete camera responses byte-identical, including restoration
after a wider posed capture. This closes the reproduced CtrlLeft case at focused
scope, not all command variants, inverse-link receipts, groups or native mesh picking.
D-QA-026 records the provisional editing/camera convention.

## Current blockers and acceptance boundaries

- **Native interaction:** UIA/provider/Invoke/capture have no positive live acceptance. QA51 proves exact style0x94000000 and extended style0x080800a0 before outer geometry refuses. Actual rectangle coordinates, client/child and later gates remain unproved. The [bounded signature-facts plan](./evidence/native-nav23/qa51-service-rectangle-diagnostic-plan.md) passed [design review](./reviews/qa51-service-rectangle-diagnostic-plan-review.md) but is not implemented. It reports only already-read facts on observe refusals; it cannot relax geometry or renew proofs. Preserve the original root/service identities, deadlines, one-attempt ledger and Owner HTTP floor. Capture remains explicitly unavailable.
- **Ordinary peer content /QA0063:** typed nine-field server witness removes the JSON-rebinding fidelity hazard while retaining authorization/provenance/history/clock/asset fences. QA47/49/52 record examined5/enqueued5 with no deferred dependencies and no publication failures. That is local enqueue only. Fresh Viewer nodes, real bytes, native rendering/picking, updates and restart still require actual peer acceptance; QA0063 remains open.
- **Terrain /QA0069-71:** prior focused partial/genuine-zero, analytic cut/fill/material/box intersection, atomic pairs and Circle chrome checks remain preserved. Other tools/held gestures, journals, late replies, authority changes, rollback, scale/general geometry and retained restart remain required. The historical QA0024 trigger is still unproved.
- **Layout /QA0039:** QA42's four incomplete measured states have zero measured violations; narrow2x Inspector is382px and valid property save/tree horizontal reachability/ruler separation were verified. Results, compact menu activation, dividers, modal/cancellation, full text/occlusion and metric/vision calibration matrices remain open. Geometry-only checks do not establish visual acceptance.
- **Owner UI observation /QA0072:** d8da3a46 compiled and QA41/42 Owner tree/state/screenshots recovered. Native observation continues to preserve disabled Owner QA diagnostics; broader native/publication/privacy acceptance is still required.
- **Retained stores:** require-existing startup is not implemented. Current ResumeRun can reach create-capable DB/schema/reconciliation and Seed. Load-only node keys/secrets and exact retained bootstrap Verse.created_by versus actual loaded node DID must be validated by the sole DB worker before logical initialization/handle publication/readiness. Legacy runtime_did is a fresh client JWT sub, not node-owner proof. SDK source confirms create-capable preparation; read-only transactions do not prove strict-existing opening. No offline opener, fake DID, reseed or unsafe retained restart is authorized by these traces.

The register has64 records:16 open/reproduction (3S1/8S2/5S3), six source-fixed/live-pending,
one cleanup-review-pending,40 focused verified and one closed. QA0073 is independently
focused-verified byQA52; that does not pass the full keyboard catalog. Read the exact
finding statuses and evidence in `qa/findings.jsonl`.

## Next actions

1. Author/review the bounded service-signature diagnostic, stage from a committed revision and measure a fresh disposable native fixture. Change geometry policy only after concrete facts and independent review; preserve explicit refusals and no-capture semantics.
2. Complete real Viewer/Editor/provider navigation, ordinary nodes/asset bytes, native render/pick and supported paired restart/pruning/lifecycle/durability tests. Local enqueue and S_OK are not peer or navigation acknowledgements.
3. Design, independently review and implement the require-existing startup path before touching retained campaign roots. Keep the original Owner/recipient stores and raise history intact.
4. Continue all groups/selection, typed schema/grid/append/replay/chart, final Alpha/Beta/Gamma/concurrency/restart, volume and permission matrices in plan.md. Reconcile the final catalog; complete five live UX audits, accepted polish waves and two100% required FULL passes with no newS1/S2.
5. After all implementation/review fixes, run the one integrated sweep and exact18-scenario harness with the required real store/Podman fixtures, then exact-SHA green CI, immutable QA tag, whole Release includingDocker and actual download/hash inspection of all seven archives.

Only root owns Cargo, apps, process lifecycle and Git mutations. The preexisting
conductor/.conductor_session_log stays excluded. No required acceptance or release
gate is waived by the ledger or these focused reviews.
