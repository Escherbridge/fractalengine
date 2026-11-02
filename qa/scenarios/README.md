---
type: ScenarioCatalog
title: FractalEngine complete region journeys
date: 2026-11-01
execution_status: NOT_RUN
---

# Region journeys

The eleven J journeys own the current feature-variant denominator in [features.md](../features.md) and
[feature-checkpoints.json](../feature-checkpoints.json). Their presence is a plan, not execution evidence.
Every current GUI variant and mandatory API/OS case needs its own current-pass checkpoint record. Legacy S01–S10
are aliases or focused subjourneys, not an alternate smaller denominator. Existing historical passes remain valid
only for their recorded scope; none establishes full coverage or convergence for the expanded catalog.

| Journey | Region | User's task | Fixture |
|---|---|---|---|
| [J01](J01.md) / G1 | R1 hierarchy/navigation | Build and navigate a world without losing its scene | BASIC/disposable hierarchy |
| [J02](J02.md) / G2 | R2 scene editing | Import and transform a real object | BASIC/GLB/WEB |
| [J03](J03.md) / G3 | R3 data/query | Edit typed data and query three known values | S7 BASIC or TERRAIN |
| [J04](J04.md) / G4 | R4 terrain/measurement | Sculpt and measure known cut/fill | fresh TERRAIN/two petals |
| [J05](J05.md) / G5 | R5 paths/stamps | Draw a path, place models and export it | PATH/GLB |
| [J06](J06.md) / G6 | R6 shell/settings/layout | Keep every panel usable while resizing | BASIC/all modal fixtures |
| [J07](J07.md) / G7 | R7 access/identity | Share with a real second identity | SHARING |
| [J08](J08.md) / G8 | R8 P2P | Collaborate, disconnect and recover | SHARING/controlled relay |
| [J09](J09.md) / G9 | R9 agent/API/accessibility | Verify API, agent and normal OS contracts | owned gate-on/off roots/OS helpers |
| [J10](J10.md) / G10 | R10 persistence | Reopen the same saved world | RESTART seam |
| [J11](J11.md) / G11 | R11 content packages | Install, share and roundtrip real content | REGISTRY/S7 archive |

Historical G1–G7 belonged to the old seven-task rotation; current G1–G11 are defined by the canonical JSON catalog.
S supplements carry `legacy_gold`, not current gold membership, so an old narrow success cannot satisfy a new task.

| Legacy scenario/task | Current journey |
|---|---|
| S01/G1 create hierarchy | J01/G1 |
| S02/G2 gimbal | J02/G2 |
| S03/G3 petal navigation | J01/G1 |
| S04/G4 map installation | J04/G4 plus J11/G11 |
| S05/G5 sculpt volume | J04/G4 |
| S06/G6 query template | J03/G3 |
| S07/G7 window chrome | J06/G6 |
| S08 exploratory checklist, S09 dialogs, S10 layout | J06 plus their explicit cross-region checkpoints |

## Shared procedure

Use the project's `.claude/skills/fe-qa-pass/SKILL.md`. Only the designated main source/Cargo/live owner launches,
builds or stops apps; code-only documentation work never does. Apply all code fixes first and reserve the integrated
test/lint sweep for campaign end. Existing journeys may be extended/swapped; no small new test functions.

Stage only when the tracked/untracked build-input fingerprint moved; read the actual `stage.json.source_commit`
and fingerprint. A stage directory timestamp is not a Git SHA. Never silently reuse stale source. If another owner
is building, coordinate ownership rather than starting a competing build/app. One owner may drive two explicitly
owned hosts for SHARING, each with separate root/identity/port and process provenance.

Launch the required fixture bare through `tools/qa/launch.ps1`, without piping or redirecting its inherited handles.
Save its returned run path immediately. A new launch can replace `.qa/current`; it must never retarget an existing
pass's commands or cleanup. Define this PowerShell helper from the main repo root:

```powershell
$qaRun = '<exact owned run_dir returned by launch>'
$qaPass = '<pass number>'
$qaRunId = Split-Path -Leaf $qaRun
$env:FE_QA_SHOTS_DIR = "qa/passes/$qaPass/shots"
function F { & python tools/qa/fe.py --run $qaRun @args }
F info
F state
F ui --interactive
F logs --level warn
```

Replace angle-bracket values before execution and record the expansion. For two-host commands use the full
`python tools/qa/fe.py --run $qaHostA ...` / `$qaHostB` forms. `F info` supplies `fixture` IDs and current `api_base`;
do not assume its token set simulates a different GUI identity. Keep credentials out of commands/evidence.

Resolve controls from a fresh `F ui --find 'visible text' --interactive`, its role, parent and the inspected PNG.
Use a unique full purpose/context name. Name matching is a substring; an ambiguous match must refuse. Use
`--nth` only after recording the actual matching candidate IDs, parents/roles and current order; it is zero-based,
never a fixed blind guess. Missing names/disabled reasons or unreachable required controls are findings. S2 may
improve purpose names; record the real name rather than weakening an assertion or bypassing UI through API.

Blank text fields can use `F type --into $qaExactFieldName 'new value'`. To replace an existing TextEdit:

```powershell
F click --name $qaExactFieldName
F key ctrl+a
F type 'replacement'
```

For a DragValue/spin field, double-click its named control first, select all, type and Enter; a single click/type
is not guaranteed to enter editing mode. For combo fields click the named purpose, inspect its fresh popup, then
choose the exact visible option. Exercise popup/color-picker children and disabled controls too. Do not invent
input names or roles from source variable identifiers. Dynamic row actions identify their node/path/group target.

The S2 source branch `1974bac` supplies these exact purposes; recheck them against the integrated live tree:

| Surface | Current source-confirmed name |
|---|---|
| Create / GLTF input labels | `Name:`, `File Path:` |
| Inspector tabs | `Inspector tab: Properties`, `Inspector tab: API Access`, `Inspector tab: Query` |
| Query editor / submit | `SQL query`, `Run Query` |
| Transform edits / commit | `Position (m) X/Y/Z`, `Rotation (°) X/Y/Z`, `Scale X/Y/Z`, `Apply node transform` |
| Property fields | `New property key`, `New property value`, `New property value type` |
| Field definitions | `New field definition key`, `New field definition type` |
| Results header / grip | `Sort results by <column>`, `Resize result column <column>` |
| GPX import / path name | `Import GPX file`, `New path name` |
| Caption | `Minimize`, `Maximize`, `Restore down`, `Close FractalEngine`, `FractalEngine title bar` |

The X/Y/Z notation names three separate fields, not a literal slash-containing name. Source confirmation is not
live locator verification. Preserve the canonical S7 supplements' arithmetic even if improved naming changes a
locator. Record any disagreement rather than silently substituting a different control.

Only viewport gestures use manually chosen physical pixel points grounded in the latest screenshot. Other drags
(column/divider/caption/grips) must derive endpoints from the named control's current response bounds; no fixed
coordinate placeholders. If such a target has no semantic response or the client cannot express the required
modifier-held drag, record the precise QA-surface gap for the main owner. Do not quietly claim the gesture ran.
`F window size ...` is useful setup/API evidence; it does not exercise a named caption button or native grip.

Wait on authoritative conditions, not fixed sleeps. Examples with actual projected state paths:

```powershell
F wait-for --name 'Run Query' --timeout 10
F wait-for --state 'query_results.pending=false' --timeout 15
F wait-for --state 'selection=null' --timeout 10
F state --path navigation
F state --path selection
F state --path query_results
```

`settled` covers known UI gestures/capture/pending UI work; DB/sync/terrain totals may be null. Verify actual node,
map residency, correlated report and persisted readback before asserting their completion. After each checkpoint:
read/judge its PNG, capture new warn/error delta and explain expected warnings. An unreviewed warning/error cannot
PASS. Check stale/foreign IDs, overlap, clipping and misleading counts even when the main action succeeded.

Queue `F dialog-answer <run-local path> --kind open|save|folder` or `--cancel` immediately before the matching
real Browse/Import/Save control. Empty/wrong-kind/confined-path refusal must remain typed and nonblocking. Never
open an uncontrolled native picker during synthetic driving. Refer to S09 for the detailed five-site journey.

Generated token/invite frames are deliberately not captured. Perform generation/copy without `--capture`, assert
redacted controls/metadata, leave the API tab or close the dialog to clear secrets, then capture the visible state.
A8 suppresses synthetic host clipboard writes. Do not extract host clipboard contents or claim Copy as TSV reads
back the actual report; use visible labels and the supported scoped API. Expired invites/tokens need real controlled
signed fixtures, not random malformed strings mislabeled expired. No unsolicited external messages are sent.

## Fixture contract and explicit prerequisite gaps

| Fixture | Required reality / current prerequisite |
|---|---|
| BASIC | S7 version: one owned verse/fractal, A three real cubes plus one asset-free marker, B marker plus bound map; A map-free. Values10/20/30 tagged separately from markers. Historical pre-S7 roots differ. |
| TERRAIN | Both petals bound to deterministic offline TerrainRGB at world/native scale1; flat origin oracle, distant8m tent hill, actual resident meshes. Read the generated descriptor and canonical S04/S05/S06. |
| PATH | Owned run-local real sample.gpx and GLB, three-point editable track and disposable destructive track; create at least one via UI. |
| WEB | Owned loopback page with a second link and controlled failure URL; record server identity and stop it. |
| SHARING | Two live owned roots/identities/ports and actual invite/replica; controlled link/relay controls for reconnect. This orchestration was missing at catalog authoring. |
| REGISTRY | Controlled owned local provider with a tiny valid `.hexon`, hash mismatch and slow cancellable transfer. Empty public registry is not coverage. Provider orchestration was missing at authoring. |
| RESTART | Validated same-root relaunch with refreshed PID provenance; fresh-run launcher alone cannot prove persistence. Seam missing at authoring. |
| OS | Input-off isolated app, bounded owned-HWND UIA reader and bounded real-app diagnostics collector. Input-on debug tree/isolated egui microbenchmark cannot substitute. |

Do not count prerequisites as completed until their owner supplies real execution evidence. BLOCKED does not
mean a product defect unless reproduced as one, and it never increases passing coverage. API/CLI-only library
surfaces remain separately mandatory; a source enum or inaccessible helper does not become a GUI feature.

## Evidence schema and reproducible coverage

Write `qa/passes/<NN>/checkpoints.jsonl` with one final record per stable checkpoint ID; duplicate IDs are rejected,
so update a record after a recheck rather than append conflicting outcomes. All missing IDs remain NOT_RUN.
Outcomes are PASS/FAIL/BLOCKED/UNKNOWN/NOT_RUN. Non-PASS needs a concrete reason. Planned/unreachable variants
remain audited exclusions and cannot PASS until the canonical catalog changes.

Every record, including BLOCKED/UNKNOWN/NOT_RUN, carries current `pass_id` and an owned `run_id` declared to the
summary command. Any supplied non-PASS stage provenance must match too. A PASS record must contain actual
`stage_commit`, 64-hex `stage_fingerprint`, owned `run_id`, nonempty `commands`,
nonempty `assertions` (`name`, independently established `expected`, observed `actual`, `passed:true`), and captured
`artifacts` (`path` relative to repo root, `sha256`, `kind`). The ledger and every artifact must be inside this
pass's `qa/passes/<pass_id>/` directory with no redirected pass directory; earlier pass artifacts cannot be reused
in place. Copy actual current-run, secret-free readbacks there before recording them. Kinds are `png`, `api`, `persistence`, `os` or
`tool`; a hash checks integrity, not correctness. GUI needs an actual PNG plus `visual_verdict` with `passed:true`,
`judged_by` and meaningful `notes`. Persistent families also need a durable `persistence` artifact; API/QA cases
need an `api`/`tool` response; OS cases need an `os` report. Store secret-free snapshots/readbacks only.

Record `log_watermark:{generation:<number>,seq:<number>}` and `new_logs:[]`; every new warning/error entry needs
`expected:true` and a concrete `reason` or that checkpoint fails. API/OS gate-off cases may instead explain
`log_watermark:{unavailable_reason:"..."}`; this is an explicit expected refusal, not fabricated empty logs.
One current pass uses one actual stage commit/fingerprint across both hosts. Reused artifacts may support several
specific assertions, but they do not prove an action that was never performed. Record each real command/variant.

```powershell
python tools/qa/coverage.py --ledger "qa/passes/$qaPass/checkpoints.jsonl" --root . --pass-id $qaPass --run-id $qaRunId | Out-File -Encoding utf8 "qa/passes/$qaPass/coverage.json"
python tools/qa/coverage.py --ledger "qa/passes/$qaPass/checkpoints.jsonl" --root . --pass-id $qaPass --run-id $qaRunId --stage-commit $qaStageCommit --stage-fingerprint $qaStageFingerprint --require-complete
```

The first command reports per-region GUI/API/OS counts, gold tasks and exact uncovered IDs. Explicit UTF-8 avoids
Windows PowerShell5.1's UTF-16 redirection default. The strict command
returns2 if incomplete/invalid; use actual stage metadata, not literal variable placeholders. It rejects unknown/
duplicate IDs, missing evidence, wrong hashes and mixed/stale provenance. It cannot judge images or establish
truth of a reported assertion; the independent reviewer must read the evidence. An empty ledger is zero coverage.
For SHARING declare both current owned hosts with repeated `--run-id $qaHostAId --run-id $qaHostBId`. An earlier
pass ledger/run/artifact is not current evidence merely because the final code fingerprint is unchanged.
Strict acceptance also requires `catalog_source_fingerprint` to match the actual staged source. It starts null
because this documentation checkout predates main S7/S2 integration. After source/caller reconciliation, the
catalog owner records the reviewed fingerprint; refresh it after later waves. Do not copy a new fingerprint into
an unchanged old catalog without checking newly reachable/removed routes and their checkpoint IDs.

## Full passes, rotation and convergence

The first full pass and both final convergence passes run every current J01–J11 variant and all mandatory API/OS
acceptance. For bounded exploratory passes, run J04's positive volume oracle and J06's full three-size sweep every
pass, plus groups A=J01/J02/J03/J06, B=J04/J05/J09/J11, C=J07/J08/J10 cycling by `(pass-1)%3`. Every region appears
within three passes. Always include every changed region and reverify every fixed finding regardless of rotation.
Rotating coverage is reported for that current pass; never silently inherit previous records to claim100%.

Convergence requires TWO consecutive final-code passes each with every G1–G11 gold task PASS, GUI variants100%,
all mandatory API/OS cases complete and zero new S1/S2. FAIL/BLOCKED/UNKNOWN/NOT_RUN do not pass. State the previous
pass and exact denominator/fingerprint. Refresh after each W2–W6/W7+ implementation before computing final coverage.
Stop after the configured six-iteration batch if necessary and report the exact unconverged state; do not relabel
the cap as success. User in-app verification remains separate from agent convergence.

Always stop each exact owned run in finally, even after crash. `stop.ps1 -Run $qaRun` validates PID/start/exe and
captured descendants. Check its exit/failure and eventual identity state separately; children born after the last
snapshot cannot be asserted absent without evidence. Never terminate by name or inspect unrelated applications.
Commit verdict/checkpoints/coverage/findings/intake with explicit paths, no PNGs, no session log, no broad add-A.
