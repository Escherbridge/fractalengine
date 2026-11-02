---
name: fe-qa-pass
description: Run one evidence-led FractalEngine QA pass across canonical feature checkpoints and region journeys, drive the owned real app through semantic locators, inspect screenshots/state/persistence/logs, stop exact owned identities and record truthful feature coverage and findings.
---

# fe-qa-pass

Use the real owned Bevy+egui application, not a simulated browser. Read every PNG captured as evidence and judge it
against the named checkpoint. The canonical contract is `qa/features.md`, `qa/feature-checkpoints.json`,
`qa/source-inventory.md` and `qa/scenarios/J01.md`–`J11.md`. Read `qa/scenarios/README.md` for the exact shared
commands, fixture contracts, input/edit semantics, evidence schema and rotation. Legacy S files are aliases or
focused supplements; seven success paths cannot stand in for complete feature coverage.

## Ownership and build

Only the designated main source/Cargo/live owner stages/launches apps. Code-only workers never run Cargo/apps.
Apply all fixes before the integrated test/lint sweep once at campaign end; extend/swap existing journeys, no new
small test functions. This user's current policy overrides older per-slice test wording. Required production builds
and focused live evidence remain allowed. Never stage `conductor/.conductor_session_log`; never push during work.

Before staging read `tools/qa/AGENTS.md` and the actual manifest. Compare tracked and untracked source fingerprint:

```powershell
. tools/qa/_lib.ps1
$qaStage = Get-QaLatestStageDir
$qaManifest = Get-QaStageManifest -StageDir $qaStage
$qaManifest.source_commit
$qaManifest.source_fingerprint -eq (Get-QaSourceFingerprint)
```

Handle the missing-stage case explicitly. A directory timestamp is not a commit. Reuse only an exact matching
fingerprint; stale-stage fallback must be explicit and cannot count as final-current-source acceptance. Coordinate
with a running owner instead of starting competing Cargo/apps. The stage helper is
`powershell -NoProfile -ExecutionPolicy Bypass -File tools/qa/stage.ps1`; obey its bounded resource settings.

## Launch and choose scope

Run `tools/qa/launch.ps1 -Fixture basic|terrain` bare, without pipes/redirection of inherited app handles. Save the
exact returned run path immediately. Define `F = python tools/qa/fe.py --run <owned-run>`; every command and cleanup
retains that binding even if `.qa/current` changes. For two hosts use two explicit roots/identities/ports. Save
`F info`, source commit/fingerprint and secret-free fixture metadata; credentials never enter logs or verdicts.
Set `FE_QA_SHOTS_DIR=qa/passes/<NN>/shots`. Under Git Bash set `MSYS_NO_PATHCONV=1` for `/api/...` arguments.

The first full pass and two final convergence passes run all eleven journeys, every current GUI variant and all
mandatory API/OS acceptance. Bounded exploratory rotation cycles A=J01/J02/J03/J06, B=J04/J05/J09/J11,
C=J07/J08/J10 by `(pass-1)%3`; every region appears within three passes. Every pass additionally runs J04's positive
terrain oracle, J06's full three-size layout sweep, all changed regions and all fixed-finding rechecks. Report
current-pass coverage only; never import old PASS records to manufacture100%. Refresh the catalog when polish
adds/removes/wires controls, preserving retired IDs as explained exclusions/supersession.

## Drive and observe

Resolve actual control name/role/parent from `F ui --interactive` and the latest PNG, then use `click --name`,
`type --into`, `wait-for` and other documented commands. Reject ambiguity; `--nth` requires recorded current
candidates/order. No blind fixed pixel guesses. Only viewport gestures use manually grounded pixels; non-viewport
drags derive from named response bounds or remain a precise missing-surface finding. Existing text requires
click/select-all/type; DragValue needs double-click/edit/Enter. API setup/readback does not exercise a GUI control.

Wait for actual state/data/mesh/report, not fixed sleeps. `settled` excludes unknown DB/sync/terrain completion.
At each checkpoint record actual UI action, read/judge PNG, authoritative state, durable API/file readback when
applicable and new warning/error delta. Record secondary overlap/clipping/stale counts even if the main task worked.
Run layout-check at1280×720,1920×1080,1024×640 with every reachable panel/dialog/tab; include long inputs/UI scaling.
Report measured violations and unknown response/text/clip/layer/scroll facts separately. Zero measured violations
with incomplete observations is not a blanket layout PASS; name exact unverified control variants.

Terrain uses a fresh real S7 map, residency and independent constant-depth oracle: separate petals,10×10m square,
Raise positive2 => area100/fill200/net200; Lower positive1 => cut100/net-100. Raw tolerance0.001. Read actual UI
figures and asynchronous persisted values; initial zeros or missing samples do not pass. Filter seeded node queries
by exact petal/qa_fixture/qa_role: three values10/20/30, count3 and sum60. Marker/earthwork nodes do not inflate it.
Retain the canonical S02–S06 fixture supplements, including grounding offset and fresh basic arrangement.

Queue a confined `dialog-answer --kind open|save|folder` or cancellation before native controls. Empty/wrong-kind
responses remain typed and nonblocking. Never fall back to a native picker during synthetic driving. Window API
commands exercise their seam/setup; the actual caption button/grip route has its own GUI evidence.

Never capture generated token/invite frames. Generate/copy without capture, inspect redacted control/metadata,
leave API tab/close dialog to clear the credential, then capture. Synthetic clipboard writes are intentionally
suppressed; do not read host clipboard. A refused clipboard chord is not proof of CopyText sink isolation.
Normal OS AccessKit/timing uses input-off owned HWND and actual FE samples; input-on debug tree or isolated egui
microbenchmarks cannot substitute. No desktop enumeration, unrelated-app actions or global display/AT changes.

| Client result | Required response |
|---|---|
| `view_changed_verse`, exit0 | Re-read state/PNG; old-scope batch was correctly stopped. |
|409/429 after bounded client retries | Retry once after condition clears; persistent contention is a finding. |
|`not_found_or_denied` | Check actual view/token scope; expected negative case may pass, required success cannot. |
|`clipboard_chord_refused` | Expected planner policy; keep distinct from the sink/UI-copy checkpoint. |
|exit3/app crash | Record S1 panic from owned stderr, exact stop, relaunch and continue next scenario. |
|Required gold step fails twice | Mark FAIL/abandoned, retain evidence and continue; no scenario weakening. |
|Missing controlled prerequisite | BLOCKED with precise dependency; never fake product success or failure. |

## Evidence and verdict

Write one final `qa/passes/<NN>/checkpoints.jsonl` record per checkpoint, bound to current `pass_id` and declared
owned `run_id` even for non-PASS outcomes. Ledger/artifacts remain inside that pass directory. Follow the README schema. Missing IDs are
NOT_RUN; FAIL/BLOCKED/UNKNOWN do not count. GUI PASS requires a judged PNG, assertions and actual actions; persistent
families require durable readback. API/OS have separate mandatory evidence and denominators. Hashes and structured
assertions support review, but cannot themselves judge pixels or establish truth.

Run `python tools/qa/coverage.py --ledger qa/passes/<NN>/checkpoints.jsonl --root . --pass-id <NN> --run-id <owned-run-id>`
and save its report; repeat --run-id for two hosts. Final strict
acceptance adds `--stage-commit <actual-source-commit> --stage-fingerprint <actual-64hex> --require-complete` and must
exit0. It rejects unknown/duplicate IDs, earlier-pass/run evidence, stale/mixed provenance (including supplied
non-PASS provenance), absent/mismatched artifacts and missing judgments. Checkpoints cannot override source-family
region or durable-readback requirements.
The catalog's reviewed source fingerprint must match too; reconcile new/removed UI/API routes before updating it.
Do not interpret a non-strict incomplete report's exit0 as acceptance.

The verdict has OKF `type: QA Pass Verdict`, pass/date/stage/source_fingerprint/run_dirs/journeys/full_coverage.
First body line: `pass NN: gold a/b, new S1/S2 n, GUI coverage c/d, API coverage e/f, OS coverage g/h`.
The denominator is every current applicable checkpoint, not scenarios in a rotation. Report per R1–R11, every
uncovered ID/reason, all gold outcomes, actual PNG/assertion links, findings, loop-health/timings and prior/current
convergence. Focused smoke verdicts explicitly say full_coverage:false and cannot establish convergence.

Append/update `qa/findings.jsonl` by stable fingerprint/ID; never duplicate a repeated finding or delete history.
New IDs are max+1. S1=crash/data loss/gold impossible; S2=broken/lying feature or blocking QA surface; S3=visible
defect with workaround; S4=cosmetic/copy. Store exact commands, evidence and confirmed source root cause or
unconfirmed. Reverified fixed findings become closed with closed_in; source fixes alone do not close them.
Route every open S1/S2 into polish plan §QA-intake; strike closed rows with the closing pass. Secret-free artifacts
only. Do not claim user in-app checks are completed by these agent records.

## Cleanup and convergence

Always `stop.ps1 -Run <exact-owned-run>` in finally, for every host/helper/server owned by the pass. Verify PID,
creation and executable identities, command result and eventual absence. A child timeout can honestly fail stop;
later identity absence is separate evidence. Recycled unrelated PIDs stay untouched, and post-snapshot descendants
remain unknown without a later verified capture. Never kill by process name.

Two consecutive final-code passes each require all G1–G11 gold tasks PASS, current GUI coverage100%, all mandatory
API/OS acceptance complete and zero new S1/S2. State exact previous pass/fingerprint/denominators. A six-iteration
batch cap is an unconverged result if criteria do not hold. Commit explicit verdict/checkpoint/coverage/findings/
intake paths, never PNGs or session log. Final CI/tagged release is a later campaign gate owned by the parent.
