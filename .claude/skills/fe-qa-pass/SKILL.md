---
name: fe-qa-pass
description: Run one agent-driven QA pass over the live FractalEngine desktop app — stage/launch an isolated build, drive gold-task scenarios with synthetic pointer/keyboard input through tools/qa/fe.py, read every screenshot, judge it against the scenario's expected visuals, then stop the app and write a pass verdict, findings ledger and polish-lane intake. Use for "run a FractalEngine QA pass", "QA pass", "fe-qa-pass", "drive the app and screenshot it", "agent QA loop", "gold tasks for FractalEngine", or after a polish wave to re-verify UI fixes in the real app.
---

# fe-qa-pass — one QA pass over the running app

The native-app analogue of the `ux-qa-research` method (gold tasks,
scenario rotation, recorded pass verdicts, a two-pass convergence exit)
without its Playwright machinery: the "browser" is the real Bevy + egui
window, driven over the agent surfaces (`POST /api/v1/debug/screenshot`,
`GET /api/v1/debug/camera`, `POST /api/v1/debug/input`) by
`tools/qa/fe.py`. You are the persona and the judge: you are multimodal,
so **Read every PNG you take** and judge it against the scenario's
checkpoint text before taking the next step.

Files:

```
tools/qa/{stage,launch,stop,clean}.ps1   isolated launcher (tools/qa/AGENTS.md)
tools/qa/fe.py                           drive client (tools/qa/AGENTS.md §fe-py)
qa/scenarios/S*.md                       type: Scenario — gold tasks + checklist
qa/passes/<NN>/verdict.md                type: QA Pass Verdict (committed)
qa/passes/<NN>/shots/                    PNGs (gitignored)
qa/findings.jsonl                        ledger, one JSON object per line (committed)
conductor/tracks/polish_lane_20261031/plan.md §QA-intake   S1/S2 routing
```

`F` below = `python tools/qa/fe.py` from the repo root. Exit codes: 0 ok,
2 refused (typed JSON refusal on stdout), 3 app not running / died.

## Procedure

### (a) Stage — only if the build inputs moved

```powershell
. tools/qa/_lib.ps1
$qaStage = Get-QaLatestStageDir
if ($qaStage) {
    $qaManifest = Get-QaStageManifest -StageDir $qaStage
    $qaManifest.source_commit                    # actual source commit
    $qaManifest.source_fingerprint -eq (Get-QaSourceFingerprint)
} else { $false }
```

True → reuse the stage (docs/scripts-only commits never need a rebuild).
Stage directory names contain a timestamp: never pass the whole directory
name to git as a commit. Use `stage.json.source_commit` when inspecting
history and its source fingerprint when deciding whether the binary matches.
Check untracked source files too (`git ls-files --others --exclude-standard`).
The launcher compares a content fingerprint of tracked and untracked build
inputs with `stage.json`, so a dirty working tree cannot silently reuse an
older stage. `-AllowStaleStage` is an explicit fallback only: record
`stage_matches_source: false` and do not call that pass current-source verification.
Otherwise, and only if `Get-Process cargo,rustc` is empty, run
`powershell -NoProfile -ExecutionPolicy Bypass -File tools\qa\stage.ps1`
in the FOREGROUND with the max tool timeout (it sets
`CARGO_PROFILE_DEV_DEBUG=0`, `RUST_MIN_STACK`, `-j4`). If cargo is running
(another lane's build), reuse the newest stage and say so in the verdict.

### (b) Launch

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\qa\launch.ps1 -Fixture basic
```

Run it **bare** — never pipe (`| Select-Object`) or redirect (`> file`) its
output: the app inherits that pipe, so the call does not return until the
app exits. Then `F info` (run dir, pid, fixture ids, token roles) and
`F shot --label launch`. Set `FE_QA_SHOTS_DIR=qa/passes/<NN>/shots` for the
whole pass (Bash: `export FE_QA_SHOTS_DIR=...`; also `export
MSYS_NO_PATHCONV=1` so `/api/...` args survive Git Bash).

Immediately save `F info`'s `run_dir` as the pass's owned run. Keep that
exact value through cleanup; another launch can replace `.qa/current`.

### (c) Drive each scenario in the rotation

Rotation (qa/scenarios/README.md §Rotation): the gold set S01-S07 on its
success path, plus S08 items `((NN-1)*6 % 18)+1` … six at a time. Order
scenarios so any step marked "LAST: it ends the run" runs last, and
blocked scenarios (precondition unmet, e.g. S04 without a tileset) are
recorded `BLOCKED` with the reason instead of improvised.

For each step: run the `F` line, Read the PNG (`--capture` output path or
`F shot`), compare it with the checkpoint text, and note:

- **pass / fail** per checkpoint, with the PNG path;
- anything else wrong in that frame (overlap, stale count, clipped text,
  wrong panel) — a finding even if the checkpoint passed;
- coordinates are 1280x720 guesses: locate the control in the latest PNG
  first; never click blind twice.

Reactions:

| fe.py says | meaning | do |
|---|---|---|
| `view_changed_verse` (exit 0) | your click opened another verse; the host denied the rest of the batch under the old token (by design) | re-shoot; continue |
| exit 2, `429`/`409` after retries | budget/single-flight contention | wait 2 s, retry once, then record |
| exit 2, `not_found_or_denied` | no owner token covers the view (verse browser open, or a joined verse) | navigate into an owned verse; record if the scenario needed that view |
| exit 2, `clipboard_chord_refused` | Ctrl/Super+C/V/X or text under Ctrl — refused by design | not a finding |
| exit 3 | the app died | S1: copy the panic lines from `<run>/stderr.log`, run stop.ps1, relaunch (b), continue with the NEXT scenario |
| gold step fails 2 attempts | abandonment | mark the gold task FAIL, move on |

Native dialogs ("Install from file…", "Browse…") block the frame loop and
cannot be driven: never click them.

### (d) Stop — always, even on failure

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\qa\stop.ps1 -Run <owned-run-dir>
```

It kills the exact recorded pid tree (WebView2 children included), never by
name. Verify it printed `Stopped.` or `already gone`.

### (e) Verdict + findings ledger

`qa/passes/<NN>/verdict.md` (frontmatter `type: QA Pass Verdict`, `pass`,
`date`, `stage`, `run_dirs`, `scenarios`), sections in this order:

1. **Trend line** (first line of the body, verbatim shape):
   `pass NN: gold a/b, new S1/S2 n, coverage c/d` — a = gold tasks passed,
   b = gold tasks attempted (BLOCKED counts in b only when attempted),
   n = new S1+S2 findings this pass, c/d = scenarios driven / scenarios in
   this pass's rotation.
2. **Gold tasks** — table: id, scenario, PASS/FAIL/BLOCKED, evidence PNG,
   one-line reason.
3. **Findings by severity** S1 → S4, each: ledger id, title, scenario+step,
   screenshot path(s), exact repro `F` command lines, root cause `file:line`
   when confirmed in code (else "unconfirmed").
4. **Coverage** — scenarios/S08 items driven vs. skipped (with reason).
5. **Loop health** — fe.py/launcher problems met this pass (they are QA-infra
   findings too) and timings.
6. **Convergence** — this pass's exit check and the previous pass's.

Severity rubric: **S1** crash, data loss, or a gold task impossible;
**S2** a feature broken or lying with no reasonable workaround, or a
QA-infra gap that blocks a gold task; **S3** visible defect with a
workaround (overlap, stale count, Enter not submitting); **S4** cosmetic or
copy.

`qa/findings.jsonl` — append one line per NEW finding:

```json
{"id":"QA-0001","fingerprint":"sidebar-petal-click-no-fractal-panic","severity":"S1","status":"open","title":"…","scenario":"S03","step":7,"first_seen":"01","last_seen":"01","evidence":["qa/passes/01/shots/…png"],"repro":["F …"],"root_cause":"fe-ui/src/panels/sidebar.rs:461","routed":"polish_lane §QA-intake","research_ids":[]}
```

Ids are stable and never reused: next id = max existing + 1. Before adding,
match on `fingerprint` — a finding seen again updates `last_seen` (rewrite
that line) instead of duplicating; a finding whose checkpoint now passes is
set `"status":"closed","closed_in":"<NN>"` (re-verified fixed), never
deleted. Keep the file valid JSONL (one object per line).

### (f) Route S1/S2 into the polish lane

Append (or update) rows under `## §QA-intake` at the end of
`conductor/tracks/polish_lane_20261031/plan.md` — one row per open S1/S2:
`| QA id | sev | title | evidence | root cause | suggested wave |`. The
polish advocates read this section at each wave start; closed findings get
`~~struck~~` with the closing pass.

## Convergence exit

The loop is done when **two consecutive passes** each have every gold task
PASS (BLOCKED counts as not passing) **and** zero new S1/S2. Say in every
verdict whether this pass met it and whether the previous one did.

## Commit

Commit `verdict.md`, `findings.jsonl`, any scenario edits and the plan.md
intake with explicit paths (`git commit -m … -- <paths>`); never the PNGs
(`qa/passes/**/shots/` is gitignored), never `git add -A`, never push.
