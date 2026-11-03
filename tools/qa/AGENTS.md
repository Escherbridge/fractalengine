# tools/qa/AGENTS.md — staged, isolated QA launcher

Windows-native PowerShell 5.1 scripts that stage a build, launch it in an
isolated run dir with a deterministic fixture, and tear it down — so an
agent-driven QA loop (screenshot + synthetic input, `agent_vision_20261031`)
never fights a polish-wave build for `target/debug/fractalengine.exe`'s file
lock, and never shares a DB with a previous run.

Files: `_lib.ps1` (shared helpers, dot-sourced), `stage.ps1`, `launch.ps1`,
`stop.ps1`, `clean.ps1`. `.qa/stage/<short-sha>/` and `.qa/run/<timestamp>/`
are gitignored (machine-local); `tools/qa/*.ps1` are explicitly un-ignored
(`.gitignore` carves an exception out of the blanket `/tools/` rule).

## Storage isolation

`FE_QA_STORAGE_ROOT=<absolute run directory>` is the opt-in GUI storage boundary.
Startup rejects redirected roots, enters that root before touching relative
state, forces blob/P2P/hexon overrides beneath it, and skips legacy per-user
blob migration. Settings resolve to `<run>/config/settings.ron`. The node key
is durable only under `<run>/identity`; all service/account secrets persist in an owner-only run-local store.
The QA branch never constructs the OS-keystore backend or migrates user keys.
Unset preserves normal desktop paths and identity behavior.

The launcher copies only root asset files, `branding/` and `samples/` into
`<run>/fractalengine/assets`. It never junctions writable repository assets
into a run. The staged exe has no adjacent `assets` directory. Normal desktop
dialogs remain user-selected; synthetic-input runs use the guarded queued
provider described below and never fall back to native blocking pickers.

## Stage provenance

`stage.ps1` explicitly builds the GUI binary and installer example in separate
serial Cargo calls. Each immutable stage gets a unique `<sha>-<timestamp>`
name and `stage.json` recording commit, dirty state, executable SHA-256 and
build-input fingerprint. The fingerprint hashes tracked and untracked Rust,
Cargo, WGSL and asset files, including missing tracked paths. Source changes
during staging fail the stage. Launch validates the executable and input
fingerprint; mismatches require explicit `-AllowStaleStage` and are recorded
as `stage_matches_source: false`, never current-source verification.

## Bootstrap (H2 history; QA identity is now run-local)

The original plan was to pre-seed a verse row directly into the (not yet
opened) SurrealKV file via `fe-database/examples/seed_join_verse.rs`, with
`--created-by` set to the node's own DID so `role_manager::resolve_role`
would grant it Owner once the GUI's dev-token writer looked it up. Two
things killed that plan during verification:

1. **The node identity is not deterministic from outside the running
   process in this environment.** `fractalengine/src/main.rs` loads its
   keypair from the OS keystore first (`fe_identity::load_or_generate_keypair_durable`,
   account `"node_keypair"`), falling back to a file tier only on a keystore
   error. A standalone helper calling the same tier-1 `load_or_generate_keypair`
   printed a DIFFERENT `did:key:...` on each of three separate invocations in
   this sandboxed shell, none of which matched the DID the running GUI
   process actually signed tokens with (verified by decoding a minted JWT's
   `sub` claim). Whatever is causing that — DPAPI/Credential-Manager access
   behaving differently for a background/non-interactive process versus an
   interactive one is the leading suspect — it makes "pre-seed with the
   DID I just read" an unreliable foundation. A throwaway
   `fe-identity/examples/print_node_did.rs` was written, used to diagnose
   this, and then DELETED once the better path below was found (no point
   shipping a tool the final flow doesn't call).
2. **It is unnecessary.** The GUI auto-seeds a "Genesis Verse" (→ "Genesis
   Fractal" → "Genesis Petal" → 3 marker nodes) on any empty DB
   (`fe-database::handlers::seed::seed_default_data`, triggered by
   `DbCommand::Seed` right at startup) — ALWAYS created by the node's own
   real identity, whatever that is this run. `launch.ps1` just waits for
   that verse's Owner token to appear in `FE_API_DEV_TOKEN_OUT`'s file, then
   uses it as a bootstrap credential: `POST /api/v1/verses` only checks the
   CALLER's embedded role claim (`require_role(&claims, "manager")`, no
   scope match — there is no scope to match yet, since the verse doesn't
   exist), and the new verse's `created_by` is always the DB thread's own
   `local_did`, never derived from the caller's token. So ANY owner/manager
   token this node ever minted can bootstrap a brand new, cleanly-named
   verse that the SAME identity then owns — no DID discovery needed at all.

This is also just a more faithful rehearsal of the real agent flow
documented in `fractalengine/src/AGENTS.md` §qa-dev-token's "Agent recipe":
launch, poll the token file, re-poll it after a scenario creates a verse.

## Process identity and cleanup

A persistent PID is insufficient because Windows reuses PIDs. `current.json`
records PID, executable path and UTC creation ticks. Shared helpers open a
process handle, verify both identity fields, and retain that handle while
killing/waiting. A stale identity is treated as gone; legacy PID-only records
are refused by stop and retained by clean.

A Toolhelp process snapshot supplies parent IDs without requiring WMI access.
Descendants must be created after their recorded parent; each identity is
rechecked before stopping children first, then the root. A snapshot error is
reported rather than silently claiming no children exist. Launch uses a
`finally` cleanup until it hands off a complete run record. Passes capture
and stop their explicit owned run path; `.qa/current` is only discovery.

A timestamp captured before each Toolhelp snapshot bounds descendant start
creation times from above; a later PID replacement is rejected. Successful
launch persists the exact descendant identities in `current.json`. After a
root crash, stop never infers a tree from its dead/recycled PID: it only reaps
those recorded identities. Clean also retains a run and its stage for recorded
live descendants. Children born after the last snapshot remain unproven after
a crash; the stop command reports that limitation explicitly.

`clean.ps1` confines candidates to the intended QA root, uses literal paths,
retains live runs and their referenced stages, and refuses any candidate tree
containing a reparse point. This conservatively retains old junction-based
runs. Directory checks protect normal local cleanup, not a concurrent hostile
filesystem writer; do not run cleanup against an untrusted shared QA tree.

## Fixture modes and oracle

`launch.ps1 -Fixture basic` binds deterministic offline terrain to Petal B;
`-Fixture terrain` binds it to both Petal A and Petal B. `-Fixture none`
skips generation and all REST fixture writes. Neither seeded mode silently
continues when terrain is unavailable. The staged `build_qa_terrain.exe`
runs before the GUI and uses the real TerrainRGB export, bundling, verified
assembly and installation path. Its descriptor lives at
`<run>/terrain-fixture/descriptor.json`; implementation rationale and numeric
oracle are in `fe-terrain/examples/AGENTS.md`. External GIS data and network
tile sources are not prerequisites.

Both seeded modes create one QA verse, one QA Fractal and two petals. Petal A
holds Cube 1/2/3 using the uploaded run-local sample GLB. Basic preserves
their original (0,0,0), (2,0,0), (-2,0,0) arrangement for S02. Terrain mode
uses (-12,0,0), (0,0,-12), (12,0,0), outside the rectangle oracle. Each cube has
`qa_fixture=terrain`, `qa_role=value` and `qa_value=10/20/30`. Both petals
also contain an asset-free `QA oracle origin` marker at (0,0,0), tagged
`qa_role=marker`, `qa_marker=flat-origin`, `qa_base_m=0`. Marker rows are
excluded from value queries but deliberately appear in all-node queries.

`_fixture.ps1` owns checked REST setup. It requires successful envelopes and
valid created IDs, checks each property echo and readback, compares terrain
PUT echoes and GET readback structurally against the typed descriptor, then
verifies scoped node identities/positions, both markers, count 3 and sum 60.
Raw query positions are GeoJSON Point[x,z] plus separate elevation; fixture
queries project numeric x/y/z aliases and assert every coordinate and
qa_value without accepting strings or booleans as numbers. A fresh Petal A
has four rows and Petal B one; S03 records actual counts after prior edits.
Any failure throws into launch's exact process-tree cleanup. Token strings
are never logged. Persistence readback does not prove rendered residency.

`current.json` records hierarchy, value-node and marker-node IDs,
`terrain_petal_ids`, descriptor path, exact query SQL and terrain oracle.
`terrain_bound=true` means the requested binding echoes and readbacks passed;
S04 still inspects real terrain on screen before S05. Native/world scale 1,
TerrainRGB and offline mode come from serialized TerrainConfig.

Encoded flat elevation is absolute 0m. Runtime grounding subtracts the
center tile's mean (approximately 0.14m), so local terrain Y may be -mean.
Authored Y=0 nodes/markers are not claimed to sit exactly on that surface;
inspect and align live setup if needed, without changing production grounding.

The generator is deterministic at fixed code/dependency versions: fixed
manifest IDs/timestamps, ordered tile generation and bundling. Runtime IDs,
installation timestamps and run paths remain per-run. Source archive BLAKE3
is recorded in its descriptor for later reproducibility checks.

## §fe-py

`launch.ps1 -ApiPort N` selects an isolated QA loopback listener and records its origin. `fe.py --run <run>`
continues to address that run explicitly. `dialog-answer PATH --kind open|save|folder` or `--cancel` queues a
typed answer before clicking Browse. `window size W H`, maximize/restore/minimize, close, and `window ui-scale F`
use the shared QA bridge. Size is physical pixels; UI scale changes only app egui scale. Close opens confirmation.
These controls require input opt-in, the actual loopback listener, and isolated QA storage.
`stage.ps1 -Jobs N` applies the same explicit parallelism to both GUI and fixture-helper builds; default 2
matches the repository configuration and avoids unnecessary memory pressure on this desktop.

`fe.py` is a Python 3 standard-library client for camera read-back,
screenshots and synthetic input. It discovers the run from `.qa/current`
and `current.json`, reads owner tokens without printing them, and probes
`GET /api/v1/debug/camera` to choose the token covering the active verse.
`--verse` pins a token explicitly. A cached verse id accelerates the next
invocation; the token file is always read again so newly created verses
are discoverable.

Commands: `info`, `camera`, `shot`, `move`, `click`, `drag`, `key`, `type`,
`scroll`, `wait-settle`, and `api`. `shot --help` lists camera/orbit/tile/
geo/focus poses. Input commands accept `--capture` for a post-action PNG.
Screenshots go to `<run>/shots`, `FE_QA_SHOTS_DIR`, or explicit `--out`.
Resolved pose metadata is printed alongside the path. The raw `api`
command is for fixture setup and evidence read-back, not UI gold steps.

Exit codes are 0 for success, 2 for typed refusal or malformed command
values, and 3 when the app/run/token file is unavailable. Owner-only
token files written by an unsandboxed app also require the drive client
to run in that owner's execution context; do not weaken the token ACL.
Bounded retries cover 409/429 refusals. A successful click that navigates
to another owned verse is reported as `view_changed_verse`, because the
host correctly denies the remaining old-scope batch. Capture again under
the new verse's token before continuing.

`wait-settle` requires three consecutive equal camera poses by default;
it does not certify asset/terrain completion. The pass method and gold
tasks live in `.claude/skills/fe-qa-pass/SKILL.md` and `qa/scenarios/`.
Run the app lifecycle outside the restricted sandbox when native WebView
creation stalls before the first frame; bootstrap pass 01 observed that
failure and succeeded in the owner context. Stop the exact recorded PID
tree at the end, even if the app has crashed.

## §locators-and-probes

`fe.py --run <directory|timestamp|current.json>` binds a command to an explicit
owned run; without it `.qa/current` remains the convenience pointer. Run/token
files and object-shaped API envelopes use one boundary decoder. Arrays, null
data and malformed JSON produce typed refusals rather than AttributeError.
The API origin must be loopback HTTP before any bearer token is sent.
Authenticated requests use one direct urllib opener with proxies disabled and every redirect refused before
following a Location. `redirect_refused` preserves that loopback boundary without forwarding Authorization or
silently converting a mutation to a redirected GET. The rich-data driver reuses this same transport.

`ui [--find TEXT] [--role ROLE] [--interactive]` reads the primary-window tree.
`click --name TEXT [--role ROLE] [--nth N]` clicks its physical rectangle centre;
exact names take precedence over substrings, ambiguity lists candidates and
exits 2, and nth is zero-based. Hidden/disabled widgets are excluded.
Locator centres outside the physical primary window are also excluded;
`type --into` restricts matches to input roles rather than caption labels.
`type --into NAME TEXT` first focuses the named input. `state [--path a.b]`
reads authoritative state; `logs --since SEQ --level warn` reads diagnostics.
`wait-for --name TEXT [--gone]` and `wait-for --state PATH=JSON_VALUE` poll with
a bounded timeout. State string values can be passed without JSON quotes.

Each pointer/key/text/scroll action reads a log watermark before acting and
prints NEW warn/error entries afterward. QA read budgets are independent of
injection. A generation change is explicit, and unavailable logs do not replace
the original action's outcome. `wait-settle` is camera stability only; use state
or feature-specific persisted read-back for stronger completion assertions.

Stage directories now contain timestamps. Read `stage.json.source_commit` for
git history and compare `source_fingerprint` with Get-QaSourceFingerprint before
reuse. The whole directory name is never a git ref. WebView2 data is run-local
under `webview`; tokens retain owner-only ACLs and the client may need the same
owner execution context as the GUI. Do not weaken those ACLs.


## ?layout-check

`fe.py --run <owned-run> layout-check` transports tree/state into the pure `layout_check.analyze` module.
It emits measured violations, intentional scroll content and explicit unknown coverage. Exit2 means
measured violations; zero violations never certifies unknown hit rectangles, uninstrumented scrolls,
text truncation or overlay occlusion. Parent bounds alone are not clipping boundaries. Scene viewport
share uses authoritative ui-state geometry. Tree/state frame IDs are recorded separately because the
HTTP reads may observe consecutive static frames. Capture the same fixture/size/scale/section/scroll
position before and after layout work and inspect PNGs alongside the report.


S6 cleanup regression: waiting for each WebView child before issuing root termination can skip the
root when a child does not signal exit. Cleanup retains identity-verified process handles, issues all
terminations children-first without intermediate waits, then waits under one 15-second total deadline.
Failures are aggregated after every recorded identity receives its attempt; no unrelated PID is used.
## Read-only normal OS accessibility probe

`accessibility.ps1 -Run <owned-run>` is source preparation for the gate-off
normal OS path. It confines the run path, verifies the recorded PID, executable
and creation ticks, and keeps the process handle. Timing owner/ready/report
markers must identify that same process lifetime: the owner timestamp is no
earlier than process creation, no later than the current time, and the exact
owner timestamp is repeated in ready/report files. A stale ready marker or
reused PID cannot satisfy this contract.

The probe runs in a separate hidden MTA PowerShell child. The parent retains
that exact child's process handle and terminates only that child if the bounded
timeout expires, including when a provider call blocks. The worker uses only
`AutomationElement.FromHandle` for the verified FE HWND and reads descendants
filtered to that process and a fixed whitelist of caption/tool Button names.
It never reads the desktop root, arbitrary field values, invokes actions or
changes focus. Output is a small JSON match report with static names, Button
roles and finite nonempty bounds. No UI tree dump is emitted or persisted.

A passing probe alone does not establish performance acceptance: the collector
must finish its same-process pair, debug input/tree endpoints must remain
denied, fixture/load conditions must match, and screenshots need independent
review. `FRAME_TIME` is full-app wall time, not isolated accessibility CPU cost.
`launch.ps1 -DisableSyntheticInput -A11yTiming` explicitly opts into the
normal gate-off OS/timing path. Add `-A11yContinuous` to request the same
controlled continuous cadence in both measurement phases. Timing requires
gate-off input; continuous requires timing. Defaults preserve ordinary
synthetic QA launches. All three values are written to current.json's
qa_options and explicitly override inherited timing/input environment flags.
The collector still owns phase readiness, same-process conditions and honest
incomplete outcomes; launch success does not establish OS/performance proof.

App environment overrides are scoped to Start-Process with a complete prior-value
snapshot restored in finally, including a failed start. The child inherits its
isolated root/token/input/timing settings, while an invoking PowerShell session
keeps every original process environment value for later commands.

## §feature-coverage

`qa/feature-checkpoints.json` is the canonical edit point for stable checkpoint IDs, current GUI/API/OS/planned/
unreachable classification, durable-readback requirements, source families and gold-task membership. Its family
metadata keeps entry/outcome/source context in one place; checkpoints reference those families. `qa/features.md`
and `qa/scenarios/J01.md`–`J11.md` explain the contract and actual journeys. Refresh all affected representations
when a polish wave changes reachable behavior; never obtain100% by silently removing an uncovered control.

`coverage.py` is an offline standard-library evidence validator and summary command. It reads one canonical
catalog plus one current-pass JSONL ledger; it does not call the app, touch clipboard, inspect processes or access
credentials. It rejects unknown/duplicate IDs and malformed evidence, validates artifact hashes and judged GUI
PNGs, requires structured assertions and durable/API/OS artifacts as classified, and prints per-region counts,
gold outcomes and explicit uncovered IDs. Ledger and artifact paths resolve inside `qa/passes/<pass-id>/` under
the supplied repo root, with no redirected pass directory. Every record belongs to the expected pass ID and one
declared current owned run, including non-PASS records; any supplied stage provenance is checked for those records
too. Checkpoints may not override their source family's region or durable-readback requirement.

Every summary supplies `--pass-id <NN> --run-id <owned-run-id>`; repeat run-id for two hosts. Use
`--require-complete --stage-commit <actual> --stage-fingerprint <actual>` for final acceptance; incomplete or
invalid input exits2. Reporting mode may exit0 while incomplete, so inspect `complete` and keep the strict gate.
The catalog's reviewed `catalog_source_fingerprint` must match the staged source after source/caller reconciliation.
It starts null while main integration is pending. A changed code fingerprint requires reviewing newly reachable/
retired routes before changing this field. A source fingerprint does not prove the catalog itself is exhaustive.

The exact record schema and PowerShell commands live in `qa/scenarios/README.md`. Use explicit UTF-8 when storing
JSON with Windows PowerShell5.1; plain output redirection defaults to UTF-16. One final record per checkpoint
prevents historical PASS rows from overriding current failures. Reused artifacts may support multiple specific
assertions from the same actual state/actions. This validator checks reported evidence and byte integrity; pixel
judgment and truth of assertions still need the agent and independent reviewer. No scenario, file existence or
unreviewed hash alone counts as exercised feature coverage.

### Complete log windows

An ordinary PASS records both `log_watermark:{generation,seq}` before its action
and `log_after:{generation,seq}` after it. Counters are nonnegative integers
(booleans are invalid); generations must match and the sequence delta must be
between0 and2000 inclusive, the current ring capacity. A changed epoch,
regressed counter, missing after probe, explicit `new_logs_unavailable`, or
`retention.possible_gap` prevents PASS even when `new_logs` is empty. Supplied
retention counters must agree with the derived window and capacity. Every
returned warning/error still needs independent expected/reason reconciliation.
Keep the checkpoint's actual action window inside retention; an oversized or
lost window remains UNKNOWN until that checkpoint is independently repeated
with complete evidence. Never manufacture equal counters after losing observations.

Only `F112.gate-off` (API) and `F114.input-debug-denied` (OS) can use the narrow
negative exception. Their watermark is exactly
`{"unavailable_reason":"input_disabled"}`, `new_logs` is empty, and no
`log_after` is supplied. `log_exception` contains exactly
`kind:"expected_input_disabled"`, `refusal_artifact` and `no_effect_artifact`.
Both paths refer to declared hash-validated current-pass artifacts, at most64KiB
each; each JSON object repeats the record's `pass_id`, `run_id`, `checkpoint`.
The exact bounded byte snapshot being parsed is hashed against its unique
declaration again; an earlier artifact hash is never reused to trust a later
unchecked read. On Windows the snapshot also pins its ancestry and file with
read-only sharing. Replacing the file after initial artifact validation cannot
substitute an unhashed refusal payload.
The diagnostic artifact has schema `fe-qa-gate-off-refusal-v1`, the exact request
`{method:"GET",path:"/api/v1/debug/logs"}` and raw response
`{status:403,body:{ok:false,code:"qa_gate_refused",error:"QA diagnostics endpoint is disabled on this node"}}`.
The no-effect artifact has schema `fe-qa-gate-off-no-effect-v1`, request
`{method:"POST",path:"/api/v1/debug/input",body:{commands:[{kind:"pointer_move",x:0,y:0}],wait_frames:0}}`,
raw response `{status:403,body:{ok:false,error:"input injection endpoint is disabled on this node"}}`,
and equal `before`/`after` objects containing nonempty actual `camera` and
`hierarchy` readback objects. Retain the real unmodified API response bodies;
this wrapper adds evidence attribution only. These routes refuse before input
admission; foreign masked404, malformed/busy/non-Owner failures, success bodies,
free-form excuses and an unrelated catalog ID do not satisfy the exception.
Independent review must establish that the captures and no-effect readbacks are
real; a schema validator cannot authenticate invented evidence.

Positive input-off OS cases cannot borrow this negative exception. They need a
trustworthy ordinary log window; the local owned-stdout adapter below supplies
that alternative without enabling input or broadening negative exceptions. A
missing, failed or unsupported window remains UNKNOWN/BLOCKED. An old focused OS
record without the actual window is not FULL acceptance.

### Input-off OS stdout windows

`owned_stdout.py` is a local operator CLI, not an API/browser control. It accepts
only the current catalog's F114/F115 OS checkpoints and a canonical exact QA run
timestamp. The run's current.json must record synthetic input off, current stage
provenance and the exact PID, .NET UTC creation ticks and stage executable. Begin
and end open and retain the live process handle, verify its full executable path
and creation time, and recheck liveness before publication. They never enumerate
processes, change focus, inject input, read tokens or change application state.

The Windows-only file adapter pins every existing ancestor with native handles,
rejects reparse points, aliases, UNC/ADS/device/reserved paths and wrong types.
Directories, immutable receipts and executables permit FILE_SHARE_READ only,
denying both write and delete sharing. Every child's parent is pinned before
its absolute CreateFile call. Only the existing active stdout file explicitly
permits FILE_SHARE_READ|FILE_SHARE_WRITE for the logger; it still denies delete
sharing. A cached path cannot switch sharing roles. The source is only current.json's canonical
`<run>/stdout.log` or `<run>/starts/<current-attempt>/stdout.log`. Volume/file ID,
process lifetime and source/stage binding must remain equal. Each source prefix
scan is capped at64MiB; the captured byte window is at most1MiB. Offsets cannot
regress or split a line. Prefix changes, truncation, replacement, restart,
unknown line syntax and invalid UTF-8 refuse completion. Log lines must have the
native ISO timestamp, explicit TRACE/DEBUG/INFO/WARN/ERROR and target format;
other output is unknown, not silently ignored. ANSI presentation escapes are
removed only for classification. Hashes still cover original bytes.

Receipts are created with CREATE_NEW in a pre-existing, pinned current-pass
directory. No directory is created and no existing receipt is overwritten. The
CLI requires a plain JSON filename; failure/partial output or a nonzero exit is
not successful capture. Keep begin/end filenames unique to their checkpoint.

```powershell
python tools/qa/owned_stdout.py begin --run-id $qaRunId --pass-id $qaPass --checkpoint F115.frame-sample-summary --out os-log-before.json
# Run the actual bounded input-off OS helper/timing action and retain its report.
python tools/qa/owned_stdout.py end --run-id $qaRunId --pass-id $qaPass --checkpoint F115.frame-sample-summary --begin os-log-before.json --out os-log-after.json
```

Both receipts carry pass/run/checkpoint, stage commit/fingerprint, process
identity, canonical source path, volume/file ID, observed-live flag, byte offset
and original prefix SHA256. End additionally binds the begin receipt SHA256,
original window SHA256 and a deterministic redacted projection. Every message
AND target is withheld because stdout has no trustworthy event scope. Only line
number, level and original line hash remain. No original stdout bytes, JWT,
credential, URL or arbitrary message is copied into artifacts or printed. Keep
the original private source for independent recomputation; do not attach it to
the pass. The printed CLI result contains only artifact path/hash and operation.

Ordinary OS PASS uses exactly
`log_source:{kind:"owned_stdout",begin_artifact:"qa/passes/<NN>/os-log-before.json",end_artifact:"qa/passes/<NN>/os-log-after.json"}`.
Both receipts must also be uniquely declared, hash-validated `tool`/`os`
artifacts in that same pass; the independent UIAutomation/timing report is still
required to prove the feature. Do not mix ring watermark/after/retention,
unavailable fields or a negative exception into this record. `new_logs` must
match every WARN/ERROR in the redacted end projection exactly and in order,
adding only `expected:true` and a nonempty independently reviewed reason. Missing
or extra warnings and unreviewed errors refuse PASS. Never copy a secret into a
reason. This inventory covers the recorded stdout interval, not all historical
startup output or a separately written stderr stream.

Coverage audit reopens pinned artifacts and the original owned source, checks
current run/stage/process attribution, file ID, byte bounds, prefix/window hashes
and the entire redacted projection. A later append is allowed; changes within
the captured prefixes are not. Audit may run after cleanup and does not require
the completed process to stay alive. A changed current process receipt after a
restart is refused conservatively: retain dedicated OS roots until their audit.
Receipts are not cryptographic historical process attestation; independent
review must retain successful CLI outcomes and actual owned-lifetime evidence.
The final audit recomputes bytes, not past process liveness.

End must happen while the exact app is alive. A terminal close whose process has
already exited cannot be closed by this ordinary OS adapter. The separate
F080-only retained-handle controller below covers terminal observation without
changing this rule. No dead/reused PID is treated as live and no lingering native
child substitutes for the root process.

### Terminal F080 confirm-close evidence

`terminal_stdout.py watch-close` is a bounded local observer for exactly
`F080.confirm-close` (GUI). It has no click, navigation, process-start, kill or
other application-control operation. Start it in an owned command session before
the actual terminal UI actions, and wait for its flushed `phase:"armed"` output:

```powershell
python tools/qa/terminal_stdout.py watch-close --run-id $qaRunId --pass-id $qaPass --begin close-log-before.json --out close-log-after.json --timeout 90
```

The watch retains the original root process handle verified live through its
exact PID/creation ticks/stage executable. It also retains current-run metadata,
stdout file and ancestor pins. A new random public capture-session ID, native
process binding, UTC arm ticks, byte offset/prefix hash and bounded timeout are
written to an exclusive begin receipt. The end filename is also exclusively
reserved before arming, so an existing output cannot fail only after actual
Close. Only after successful live publication does it print armed. The main
owner then captures a full before state, clicks
the actual caption Close, reads the confirmation state and captures/judges its
PNG, and issues the named modal Close command. Record the real command order;
the watcher never performs these actions for the owner.

The configured watch timeout is1–180 seconds. It waits on that same retained
process object, never reopens a PID after exit, and obtains native PID,
creation/exit FILETIMEs and exit code from it. Only code0 with matching original
creation time and exit after the arm point can complete. The watch then has at
most five seconds to obtain a read-only/no-write-sharing seal on the same stdout
file. Outstanding writers cause bounded retry or refusal, never a false EOF.
The sealed source, prefix and exact final byte window must remain finite, UTF-8,
line-complete and classified under the same byte bounds/redaction rules as the
ordinary adapter. Every WARN/ERROR remains in the inventory. End publication
rechecks native exit facts from the retained object; it never marks a dead root
`observed_live:true`. Nonzero exit, missing writer release, timeout, partial or
unclassified output leaves no successful terminal end. The observer exits2 and
does not kill or change the app. Retained failed/begin-only or empty reserved-end
files are not PASS. Wait for controller exit0 before auditing its closed receipts.

Terminal receipts use distinct `fe-qa-terminal-stdout-begin-v1` and
`fe-qa-terminal-stdout-window-v1` schemas. End binds the begin SHA256 and session,
native numeric exit facts and bounded elapsed milliseconds, plus the ordinary
original-byte hashes and fully withheld projection. Final audit reopens a sealed
read-only source, requires its size to equal the captured EOF exactly and
recomputes all bytes/hashes/projections. A later append is refused for terminal
proof, although ordinary completed OS windows may legitimately have later data.
Normal before/after live gates and the two negative gate-off exceptions remain
unchanged. Terminal source cannot be used for Cancel, dirty-reason-only, API/OS
or another GUI checkpoint. Its metadata records the run's actual input mode.

The GUI ledger uses `log_source` with exactly `kind:"terminal_close_stdout"`,
`begin_artifact`, `end_artifact`, `lifecycle_artifact`. Receipts are declared
current-pass tool artifacts; ordinary judged PNG/assertions/command requirements
still apply. The lifecycle artifact has schema
`fe-qa-terminal-close-lifecycle-v1` and exactly these additional fields:

- `pass_id`, `run_id`, `checkpoint`, original `process_identity`, and
  `begin_receipt_sha256` matching the terminal capture.
- `before_state_artifact`, `close_action_artifact`,
  `confirmation_state_artifact`, `confirmation_png`, `confirm_action_artifact`,
  each referring to a uniquely declared hashed same-pass artifact. State files
  are the actual full `F state` objects. Action files are actual secret-free
  client stdout JSONL output, including its success/refusal/disconnection outcome.
  Save UTF-8 stdout separately from stderr; do not merge plain stderr text into JSONL.
- `controller_artifact` retains the actual two armed/complete stdout objects,
  matching the session, begin/end paths/hashes and native exit facts. Retain the
  owned command session's actual output and ordering, not recreated phase flags.
- `close_command` and `confirm_command` are the actual expanded argument vectors
  `["--run",<absolute-owned-run>,"click","--name","Close FractalEngine","--role","button"]`;
  `confirm_exit_code` records its real CLI status.

The validator requires increasing state frames, unchanged navigation, no close
modal before the caption click and the actual modal afterward. The caption
result must contain the observed hit/clip named-button locator and one applied
left click inside it. The confirmation PNG remains subject to independent visual
judgment. A successful confirm result must match that button/view. A CLI3 caused
by shutdown during the actual `/debug/input` request can instead be retained as
an explicitly interrupted response; a failed camera probe, refused input or
another error cannot substitute. That interrupted request is not an invented
successful HTTP acknowledgement. Clean native exit plus actual GUI/action/
lifecycle evidence must be reviewed together: a stdout receipt alone never
proves GUI intent or that a particular command was applied. Receipts are local
operator evidence, not cryptographic historical attestation.

The offline packet extends actual strict-CLI validation with synthetic clean,
acknowledged and reviewed-warning terminal cases, and nonzero/wrong-lifetime,
exit-before-arm, boolean/schema/session/hash substitutions, later EOF append,
deadline, missing warnings/lifecycle/modal, zero applied Close, unrelated
pre-input error, refused confirm, foreign scope and wrong checkpoint/type cases.
These files execute no app and cannot prove retained live handle, native exit,
deadline or writer-release behavior. Independent source review and root-owned
live close/refusal verification remain required before FULL acceptance.

The existing J09 validator and S10 layout journeys use the retained offline
packet `scenarios/evidence-boundaries.py`. Run it once in the final integrated
sweep with a new `--out qa/passes/<NN>/evidence-boundaries` directory. It invokes
the actual strict coverage CLI and layout classifier over explicit synthetic
cases, saves each input and result, and exits2 on a mismatched expectation.
Its fabricated stage/run and response fixtures are marked offline and contribute
zero application checkpoints; never merge their ledgers into the real pass.
The Windows extension also exercises actual final-audit CLI acceptance/refusal
over retained synthetic stdout files and real file identities: reviewed/empty
inventories, later append, missing warnings, truncation, replacement, modified
prefix, changed lifetime, forged file/projection, partial/unclassified lines,
bounds, input-on, wrong source/executable, mixed ring and non-OS cases. Those
fixtures deliberately skip live-process admission and use no running app. Root
still needs separate live begin/end and exact-lifetime refusal evidence.
The same packet interleaves actual artifact hashing, byte substitution and the
actual refusal evidence parser without a timing-dependent race. The unchanged
snapshot must parse, while a replacement containing a plausible gate-off refusal
must fail its declared hash. Malformed projection shapes and omitted warning
projections also refuse through the actual strict CLI.

### Degenerate nested scroll observations

A recorded zero/nonfinite viewport extent is an explicit diagnostic coverage
gap, including a zero extent on a disabled axis. The classifier labels enabled
and disabled axis degeneration separately and withholds reachability/clip
judgment for dependent controls. Missing ancestry/legacy metadata is likewise
UNKNOWN. This avoids declaring the horizontal property table inaccessible just
because its recorded disabled-Y viewport height is0 inside a vertically scrolled
Inspector. It does not assert that impossible source geometry is correct, change
scroll offsets, lower the20px hit minimum, or certify complete coverage. Ordinary
fully measured scroll ranges and genuine non-scrollable clipping still receive
their existing classifications; modal-background withholding runs first.
Repairing the egui observation itself is separate future work if live evidence
shows the inner rect should have been nonzero. A successful wheel/Edit/Cancel
sequence proves that observed path, not all unknown table states.

## Locator visibility

Named input intersects a same-pass observed control's hit rectangle, clip and
primary window before choosing its center. Empty, negative or disallowed hits
are refused. Exact names take priority before visibility filtering, so an
invisible exact target cannot fall back to a similarly named visible action.
Scrolling an off-clip menu item into view is a real prerequisite, even when
AccessKit still reports its full bounds and hidden=false. Input results expose
the chosen rectangle and whether geometry came from observed hit/clip data or
an explicit bounds/window fallback with clipping unknown.

Action log output retains the observed starting watermark and ending counters.
It flags changed generations or more than the source-derived2000 all-level
ring entries as possible retention gaps. An empty warning delta with a gap is
not proof of a quiet action. Capture these counters before stopping the app;
they cannot be reconstructed from old warning-only output afterward.

## §rich-data

Derived provenance uses `qa.fractalengine/<scenario>@1`. The SDK device-format
grammar requires at least two namespace labels; a single `qa` label is invalid.
Keep failed-attempt receipts and use a fresh Petal after partial writes rather
than repeating non-idempotent scenario steps.
Raw journal snapshots and typed replay snapshots compare after shallow defaults
for the three optional NodeRowSnapshot fields: display_name, asset_id, tombstone.
Only absent top-level fields become null; extra keys, nested values and the
existing numeric comparator remain unchanged. Identity/scope/version/schema
provenance assertions are still required before writing derived measurements.

`scenarios/rich-fleet.json` is the editable rich-data intent and independent oracle set. Logical Alpha/Beta/Gamma
names are mapped to actual returned object IDs. Schemas, initial objects and expected event-time arrays are data;
the ordered steps describe explicit API operations. `rich_data_scenario.py` validates this intent before effects:
bounded file/depth/array/step sizes, unique step IDs, declared references, finite XYZ/ranges/timeouts, UTC event
timestamps, consecutive schema revisions and a fixed operation vocabulary. Unknown fields/operations are refused.
The validator is structural, not a replacement for the authoritative bounded schema validator or a passing run.

The scenario targets both committed ordinary-object history and appended measurements. Alpha's baseline v1,
calibration v2 and transform v3 are distinct commits; later current calibration gain3 must not rewrite v3 gain2.
Checkpoint version maps describe explicit per-object cuts, not a global wall-time snapshot. Measurement times are
separate sensor event times, delivered T0/T2/T1. Affine derivation is an explicit driver action pinned to the actual
historical calibration; no automatic rule engine, HLC override, canonical migration or plugin receipt is implied.

The independent oracles require per-anchor calibrated values21/23/25 and9/10/11, means23/10, sums69/30 and stored
three-dimensional travel10/8/4m. A fleet-only sum99 is insufficient because the raw values also sum99. Schema
evolution is additive optional metadata, followed by a rejected incompatible revision. Definitions use the new
bounded `{version,schema}` vocabulary, not unrestricted JSON Schema. Current source/receipt contracts must be
reconciled before execution; unavailable committed/history APIs cannot fall back to queued legacy mutations.

`rich_data.py` separates validated intent from fixed effect handlers and actual evidence. It reuses `fe.py`
token loading and its redirect-refusing loopback HTTP transport. Read-only requests can retry rate limiting;
mutations never retry. A timeout or unconfirmed write stops with an unknown outcome: inspect persisted state,
then use a fresh isolated scope if repeating the write scenario. Initial create/property/full-transform writes
require `persist=committed`, exact expected per-object versions, current properties/transform readback and a
verified journal entry before advancing. Readings require stored row IDs, values, units and event-time readback.
The driver bounds raw measurement magnitudes to1e12 and derived values to1e15 for predictable QA/report math;
these are driver limits, not product storage limits. Typed object properties retain signed64-bit integer text.

Structural validation performs no app operations:

```powershell
python tools/qa/rich_data.py --scenario tools/qa/scenarios/rich-fleet.json --validate-only
```

After the owned run is staged from the reviewed source, create a fresh owned verse/fractal/petal and supply its
actual IDs. The launcher must have copied the renderable `fractalengine/assets/samples/info_panel.glb` into that
run. Execution is explicit:

```powershell
python tools/qa/rich_data.py --scenario tools/qa/scenarios/rich-fleet.json --run <absolute-owned-run> --pass-id <current-pass> --verse-id <actual-verse-ULID> --petal-id <actual-petal-ULID>
```

The pass/run/scenario evidence directory is exclusive. Existing evidence refuses another non-idempotent run;
never delete it to hide an earlier failure. A fresh scenario execution needs a new pass/run binding. Reports
remain within `qa/passes/<pass>/rich-data/<run>/<scenario>/`; step receipts, context and index include stage
commit/fingerprint, scenario digest and a fingerprint of all driver/transport/report modules. Tokens must live
inside the owned run, and requests/artifacts scrub encoded payloads and credentials. A log gap, generation
change or new warn/error leaves coverage UNKNOWN for independent review even when the data assertions succeed.
Both log sequence bounds are preserved. A regressed/invalid bound, changed generation or delta over the2000-entry
ring capacity also leaves coverage UNKNOWN: info-level traffic can evict an earlier warning even when the filtered
warn/error response is empty. An empty response alone never establishes uninterrupted diagnostic coverage.
`coverage-candidates.jsonl` is partial API evidence, never a whole-pass acceptance claim; merge only one final
reviewed record per checkpoint into the current pass ledger. It does not produce GUI PASS records.

After the main owner stops/relaunches the exact same owned root and refreshes `current.json` process identity,
repeat with the same options plus `--verify-state <absolute-current-pass-context.json>`. This hook performs only
reads. It requires a changed recorded process identity with unchanged source/pass/run/scope/driver provenance,
checks current and earlier snapshot hashes, schemas, uploaded GLB/provenance bytes and trajectory positions,
then repeats the row/aggregate/window/latest/history oracles. It neither restarts nor launches the app. The
reviewer must pair it with actual stop/relaunch evidence; changing a JSON process identity alone is no proof.

## §rich-data-report

`rich_data_report.py` exports collected rows only: queryable `measurements.json`, measurement/trajectory CSV and
an offline `report.html`. Native SVG charts use recorded sensor time and stored object positions, while the
history selector uses collected committed-version snapshots. This report is a QA artifact; product charts,
Properties, Query and History UI still require their own observed interactions and judged screenshots.
Repeated query views of the same reading ID share one export row; distinct reading IDs remain distinct so
duplicates cannot disappear. JSON embedded in HTML escapes `<` and Unicode line separators; dynamic labels and
states use DOM textContent, never HTML interpolation. No CDN, network fetch, API token or expected oracle point
is embedded. Failed executions still export the actual partial observations and clearly label incompleteness.

Each affine step independently declares `calibration_schema_versions` per anchor (revision1 in the example).
The selected fact must match its requested node/committed version/scope and replayed after-state, and its schema
revision must equal the declared expectation before provenance bytes or derived readings are written. Integer
presence or copying whatever revision the server returned is insufficient provenance evidence.

## Same-root restart and persistence proof

`restart.ps1 -Run <absolute-directory|yyyyMMdd-HHmmss-fff>` delegates to the
existing `launch.ps1 -ResumeRun` lifecycle. The directory must already exist
as a direct child of this checkout's `.qa/run`; relative `.qa/run/...` paths,
redirected storage and mismatched manifest paths are refused. Restart checks
recorded stage commit/fingerprint/executable SHA against its stage, binds the
recorded root identity to that executable, and validates loopback origin and
QA options before stopping anything. Missing/empty DB, durable identity, secret-store
or copied-asset storage is refused so a lost dataset cannot masquerade as a restart. An exclusive per-run lock serializes
restarts. No unrecorded PID, process-name pattern or other run is stopped.

The default stage is the previous stage, not the latest directory. `-Stage`
accepts a staged directory name when intentionally moving to a newer binary.
The selected stage must match current build inputs unless `-AllowStaleStage`
is explicit; both the mismatch and allowance are recorded. Updated binaries
can change storage behavior, so the same-stage restart is the cleanest
persistence proof. The previous start's provenance is kept in the new start's
`previous-start.json`; no token contents or run-local secrets are archived.

Restart issues cleanup only for the exact recorded root/descendant identities
using the shared aggregated cleanup helper. If the root is already gone, the
recorded descendants are still checked/cleaned and a warning identifies the
post-snapshot child coverage limit. A cleanup failure aborts startup. A PID
reused with a different executable or creation time is left untouched.

A resumed run never invokes the fixture generator, REST fixture mutations,
asset copying or fresh-root creation. Data, identity, configuration, WebView
profile, run-local secrets, installed terrain, copied assets and fixture IDs
stay in the same directory. `-Fixture` is refused with `-ResumeRun`. The old
port and input/timing options are inherited unless explicitly overridden.
All nine existing app environment keys are saved/restored around Start-Process
on success or failure. A missing bootstrap verse is a failure, not a reason to
create another scope. Startup readiness itself reads only; the application
may perform its ordinary startup migrations.

Only `tokens.json` is removed after prior cleanup to require a fresh startup
token emission at the same confined path. Startup waits for HTTP, a fresh
Owner token for the prior bootstrap verse, a bounded authenticated hierarchy read containing
that bootstrap verse, and a still-live exact new process before handoff.
Each resumed start writes new stdout/stderr under `starts/<timestamp-guid>/`
and records process identity, stage provenance and ready/failed status there.
`current.json` is replaced from a pending file only after successful handoff
preparation. A failed new start rolls back only that start's captured identity
tree and preserves the persisted dataset and previous-start record. Inspect
any failed/stopping attempt and its cleanup outcome before another operation;
there is no automatic retry of startup or mutations. Failures before process
creation may leave a `stopping_previous` attempt for explicit diagnosis.

From the main checkout, the operator sequence for both committed history and
measurement durability is:

```powershell
$ownedRun = 'C:\Users\atooz\Programming\fractalengine-workspace\fractalengine\.qa\run\<actual-timestamp>'
$before = Get-Content -LiteralPath (Join-Path $ownedRun 'current.json') -Raw | ConvertFrom-Json
powershell -NoProfile -ExecutionPolicy Bypass -File tools/qa/restart.ps1 -Run $ownedRun
if ($LASTEXITCODE -ne 0) { throw 'Restart failed; inspect this start before continuing' }
$after = Get-Content -LiteralPath (Join-Path $ownedRun 'current.json') -Raw | ConvertFrom-Json
if ($after.run_dir -ne $before.run_dir -or $after.tokens_path -ne $before.tokens_path) { throw 'Root changed' }
if ($after.process_identity.start_ticks -eq $before.process_identity.start_ticks) { throw 'No new process lifetime' }
if (($after.fixture_detail | ConvertTo-Json -Depth 12 -Compress) -cne ($before.fixture_detail | ConvertTo-Json -Depth 12 -Compress)) { throw 'Fixture IDs changed' }
$after | Select-Object run_dir, process_identity, api_base, stage, stage_manifest, stage_matches_source, restart
```

Use the same current-pass rich-data context and original command options,
adding `--verify-state <absolute-current-pass-context.json>` to `rich_data.py`
and keeping `--run $ownedRun`. That read-only driver hook checks both earlier
committed object snapshots and measurement rows/aggregates/windows/history;
it never recreates the data or retries a mutation. Pair the driver artifacts
with actual restart stdout, the two process lifetimes and start manifests.
A changed JSON identity or successful startup alone proves neither durability
nor that a restarted binary matches current source.

No PowerShell smoke-test file exists in this harness. This seam is statically
parser-checked; actual same-root startup, inherited/overridden port, preserved
fixture/config/identity/WebView state, failure rollback and the dual durability
readback remain required live checks by the main owner after review.

### Lifecycle serialization and timing resumes

Fresh launch, resume, stage, stop and clean acquire the same exclusive
`.qa/lifecycle.lock` before selecting any run/stage, reading a current pointer,
starting/stopping processes or deleting directories. The lease is outside both
`.qa/run` and `.qa/stage`, so recursive cleanup never reaches it. Acquisition
is fail-fast: a busy/unavailable lease refuses the operation before lifecycle
effects. Startup retains it through publication of the new manifest; this
protects both the previous run/stage and a selected new stage during the gap
between old-process exit and new-current publication. Clean cannot partially
delete a run and then discover an open lock inside it. Every entry releases
its handle in finally. `restart.ps1` delegates directly to launch, and launch
calls internal cleanup helpers, so no public entry reacquires its own lease.
The per-run restart lock remains an additional guard, not the cleanup boundary.

The collector's timing owner marker is exclusive to one process lifetime.
Resume with inherited or explicitly requested timing refuses existing
`a11y-timing-owner.json`, `a11y-baseline-ready.json` or `a11y-timing.json` before
stopping the prior process. Prior proof is neither removed nor overwritten.
From the current owner PowerShell, disable timing explicitly when continuing
the persisted dataset:

```powershell
& .\tools\qa\restart.ps1 -Run $ownedRun -A11yTiming:$false
```

Explicitly disabling timing also disables inherited continuous timing unless
`-A11yContinuous` is explicitly supplied. Requesting continuous=true with
timing=false is refused before stop. The new manifest records the actual
false values, while previous-start metadata and timing artifacts preserve the
old proof. A fresh isolated timing run is required for another timed lifetime;
a resumed manifest never claims that old timing output belongs to its process.
Windows PowerShell5.1 callers should use the direct script invocation above
for explicit false switches rather than the external `powershell -File`
argument format, which cannot reliably convey false switch values.

### Bounded authenticated startup readiness

Authenticated readiness uses only `GET /api/v1/hierarchy`, which is available
with synthetic input enabled or disabled. The earlier GET-fractals attempt
returned405 because that route is POST-only; the first hierarchy attempt
returned HTTP200 with ok=false and rolled back without retaining its reason.
Those failed starts remain historical failures, not successful restart proof.

The fixed read probe polls for at most35 seconds, with a maximum6-second HTTP
timeout clipped to the remaining whole-second budget and a200ms backoff. It
pins/checks the exact new process identity before each request, checks the
retained handle after each response, and verifies identity again before
success. Handoff requires an actual boolean ok=true with an array hierarchy
containing the existing bootstrap verse. There is no write or mutation retry.
Only known transient API errors (`request timed out`, `request cancelled`),
transport timeouts,408/429 and server errors can retry. Permanent HTTP auth,
redirect/path failures, malformed responses, permission denial, a closed
internal channel or unrecognized API errors fail closed. A temporarily empty
hierarchy may retry within the same bound; it cannot pass as persisted data.

Readiness metadata retains attempts, elapsed time, last HTTP status, static
reason and terminal reason. API error text is preserved only on an exact
allowlist of the four fixed strings emitted by the hierarchy handler; all
other error text becomes `unrecognized_api_error`. Response data, headers,
Bearer tokens and exception messages are never copied into these diagnostics.
Failure provenance captures a fresh descendant snapshot before rollback when
possible, keeps known identities if enumeration fails, and records capture
start/end, status, exception type and available native error code. The failed
start file is written before cleanup and updated with the cleanup outcome.
`completed_for_captured_identities` does not certify children absent beyond an
unavailable snapshot; pair it with captured identities and independent exact
process inspection. Cleanup still runs if writing diagnostic provenance fails.

Scroll geometry version2 adds `viewport`, `content_rect` and `parent_id` to
ui-tree layout scroll observations, retaining `clip` as the actual child Ui
painting clip. The checker follows exact AccessKit content-ID/parent ancestry;
wide clips never associate a parent header or neighboring table by position.
Range is content_size versus viewport on enabled axes, not versus a clip that
an ancestor might shrink. Content bounds describe layout, not a scissor.
Local control clips stricter than every known scroll clip remain measured
clipping violations; an ancestor's scroll range cannot reveal a control past
its own moving local scissor. Existing hit-target and truncation rules remain.
Legacy observations without the additive fields, or unresolved parent chains,
produce explicit unknown geometry instead of false scroll clipping or PASS.
Old clients may ignore new fields; new clients must not reinterpret historical
inner_rect-derived clip observations as actual painting evidence.

On enabled axes, historical inner_rect/parent intersections are lower bounds
on the old child painting clip; disabled-axis scrollbar reservations make
that inference unsafe. Exact-chain legacy enabled-axis observations can still
prove a local control scissor stricter than the lower bound (armed Cancel),
while their own visibility/range interpretation remains explicitly unknown.
Different observed control/scroll layers also remain unknown and cannot lend
scroll range to each other. Actual locator hit/clip intersection is unchanged.

An inner area's clip can inherit an outer scroll scissor on an axis where the
inner area has no range. If an exact outer ancestor has range, that offscreen
case is `inherited_scroll_clip_path_unresolved`, not a clipping violation or
intentional-scroll PASS. The checker has not measured which inherited scissor
moves relative to all nested local allocations. Actual local control scissors
remain violations and current-offset locator hit/clip refusal is unchanged.

## Controlled WEB and registry fixtures

`fixtures.ps1 -Action Start -Run <explicit-owned-run> -Port 8890` owns a bounded
Python standard-library server on exactly `127.0.0.1`. It reuses `_lib.ps1`'s
path confinement, exclusive lifecycle lease, retained process handles and exact
PID/start-ticks/executable cleanup. Start requires the app's recorded lifetime
to be running, the generated run-local `terrain-fixture/source.hexon`, and a
stage containing a hash-verified `seal_hexon.exe`. The stage builds that existing
fe-format example serially after the other helpers and records its SHA256.
Older stages remain valid for ordinary app launch but cannot start this fixture.

Default Python discovery enumerates `Get-Command python.exe` applications in
command precedence order and selects the first valid absolute native executable.
Each candidate passes the same basename, real-file and reparse-ancestor checks
as an explicit `-PythonExe`; redirected application aliases are refused. Multiple
installed versions must never be string-joined into a single path. An invalid
explicit path fails without falling back to another installation. Discovery
inspects filesystem metadata only and never starts an interpreter to probe it.

Preparation creates a unique `<run>/controlled-fixtures/<instance>/`. It signs
the small existing analytical TerrainTileset through production `seal_hexon`,
which verifies `RequireSealed` with a publisher pin before returning. A fresh
32-byte signing seed enters only that child's environment, never its argv, the
parent environment, a file, or a log. The random publisher intentionally differs
between starts; keep the same frozen archive when byte-for-byte restart evidence
is required. The analytical terrain's original descriptor continues to describe
the unsigned source, while `controlled-fixtures.json` records the sealed SHA256
and verified stage helper provenance. The server parses bounded ZIP members,
checks the frozen SHA256 receipt, and serves raw stored manifest/header text;
it does not implement a replacement signature or BLAKE3 verifier. Production
consumers still enforce the actual seal and signed BLAKE3 member roots.

The server never serves a filesystem directory, executes request code, accepts
uploads, follows redirects, or logs request paths/headers. Authorization-bearing
requests are refused; this public loopback fixture never needs an app token.
Only an exact Host header for its origin is accepted. Reparse paths, privileged
ports, the owned API port and ambiguous prior ownership records are refused.
The listener uses Windows exclusive-address binding, so an occupied port fails
without taking over another service. It has a maximum of 2,000 requests, a
30–3,600-second lifetime (default 900), 2-second socket reads and an 8 MiB archive
budget; no blind mutation retries occur. Run-local stdout/stderr contain no
tokens, signing seeds or HTTP request data.

Every accepted connection consumes the request budget before request-line/header
parsing, including incomplete, malformed and unsupported requests. One request
is admitted per connection. An absolute two-second request-line/header deadline
actively shuts down that socket even when bytes trickle in faster than the idle
timeout. After valid headers, the socket deadline switches to the original global
expiry so deliberately slow bodies remain possible. Global expiry also actively
closes an in-flight response; neither malformed requests nor stalled writes can
extend the owned provider's lifetime. Completed connections cancel their timers.
Exceptional sealer timeout cleanup retains its exact available identity, kill/
wait outcomes and explicit failures in the instance's `sealer-cleanup.json`;
an unsuccessful bounded wait is reported as incomplete cleanup.

`/web/one` links to `/web/two` and `/web/three`. The three static pages have
distinct visible titles, no scripts/resources/forms and no-store headers. This
provides a deterministic two-link history fixture. Canonical Portal policy
currently refuses loopback URLs. Starting a server does not override that rule
or prove embedded loading. WEB remains BLOCKED until a separately reviewed,
strictly QA-only exact-origin allowance exists at every save/navigation boundary;
do not use DNS aliases, remapping or a broad localhost exception to bypass it.

The HTTP registry implements the existing search, manifest, entries, `/meta`,
monolithic `/chunk/0`, member and full-download read routes. Its HeaderDoc keeps
raw UTF-8 member text and the real signed manifest; query filtering matches the
production client's q/tags/type contract. Binary routes use attachment/nosniff
headers. Unknown URIs/members are 404. `-Mode slow` sends 512-byte binary pieces
with a 250ms pause for bounded progress observation. `-Mode corrupt` retains
the real header but changes one actual tile member in a valid ZIP, so the
production signed digest check must fail. It is an intentional integrity failure
fixture, never a different successful package. Stop and start a new instance
explicitly to change mode; there is no unaudited mutable HTTP control route.

Set the app's Registry URL through Settings → Network to the recorded `origin`,
then use the recorded `registry_uri` in Install from registry. The same analytical
ID may already be installed on the source host: use the pair's unseeded second
host or a disposable remove/reinstall branch. This HTTP route does not populate
Maps Available. Available comes from `PeerTilesetAdvertisement`, and its Download/
Cancel routes drive P2P `TilesetTransfer`. A slow HTTP registry response cannot
certify P2P cancellation. Use a real joined peer with seeding enabled, a verified
sealed P2P archive, and a separately controlled transfer interval for those cases;
record BLOCKED if the tiny transfer finishes before Cancel can be exercised.

Always stop fixtures explicitly before app cleanup:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/qa/fixtures.ps1 -Action Start -Run $qaHostA -Port 8890 -Mode normal
powershell -NoProfile -ExecutionPolicy Bypass -File tools/qa/fixtures.ps1 -Action Stop -Run $qaHostA
```

Start/Stop require the same owner execution context as launch; never weaken run
token ACLs. The root app may already be gone when Stop is called; the fixture's
separate retained identity is still used. Expiry never certifies cleanup: retain
and independently check its exact identity. Per-instance evidence is preserved.

## Two-root sharing orchestration

`pair.ps1 -Action Start -Stage <immutable-stage-name> -OwnerPort 8882 -ViewerPort 8883`
calls the existing launcher serially: Owner gets BASIC and the second identity
gets `none`. The second launch pins the first's stage. Both fresh roots, distinct
API ports, source fingerprints, executable hashes and process/descendant identities
are saved in `<owner-run>/pair.json`. Run-local identity-file SHA256 values are
compared only in memory and discarded. Public issuer DIDs are extracted from
the protected, already bootstrapped Owner credentials and must differ too; token
strings and private key bytes are never emitted or stored in pair artifacts.
This does not independently verify JWT signatures; the existing authenticated
launcher handshake supplies that trust boundary. The actual UI Join/RBAC/replica
outcomes still require the real second identity, never a different bearer token
in the first GUI. Both roots initially own separate Genesis scopes; the name
"Viewer" describes its intended role in the Owner's shared verse after joining.

Pair publication acquires the lifecycle lease only after both launch calls have
released theirs. Under that lease it re-reads provenance, rechecks both original
exact live root identities, retains their process handles and refreshes descendant
snapshots before publishing through `pair.pending.json`. This is a point-in-time
liveness check, not a guarantee that either process stays alive afterward. All
handles and the publication lease are released before failure cleanup takes its
own lease. Failure cleanup addresses only captured original identities, never a
new lifetime later written at the same run path; unavailable original ownership
is explicitly retained for inspection rather than guessed from a replacement.

The launcher deliberately disables relays. A direct local peer connection must
be observed after real GUI invite generation/join; no global network settings,
public services or external recipients are touched. No invite transfer seam is
added here: synthetic Copy remains suppressed, and absence of a safe explicitly
owned in-memory invite transfer keeps that checkpoint BLOCKED. Do not read the
host clipboard or persist an invite to manufacture success.

Every command remains bound to the explicit owner/viewer paths in pair.json;
`.qa/current` will point to the second launch and is never used by pair Info/Stop.
`pair.ps1 -Action Info -OwnerRun <explicit-owner>` prints credential-free ownership
metadata. `-Action Stop` validates both current records before attempting either
stop, then reuses exact children-first cleanup for both even if one stop fails.
It refuses changed/restarted lifetimes. After deliberately stopping/restarting a
peer for disconnect/reconnect coverage, use the existing `restart.ps1 -Run` and
`stop.ps1 -Run` separately and keep those new lifetime records; the old pair
record is retained as historical evidence, never silently refreshed. Process-name
kills, database copies, real user stores and keystore calls are outside this flow.

If startup of the second host or identity validation fails, the wrapper attempts
cleanup of every successfully launched root and reports all cleanup failures.
Existing stop limitations still apply after a root crash: recorded descendants
are checked, but children born after the last snapshot cannot be proven absent.
Fixture servers have separate ownership records and must be stopped first.

## Native Portal fixture client

`fe.py portal-fixture observe` reads only the fixed native fixture document facts. `second-link` invokes its actual second anchor and waits for a current observed page two. `refused-third-link` invokes the actual third anchor and reports the exact native refusal with the original page retained. Neither action submits a Navigate URL. These routes require input opt-in, Owner, the current selected Portal/Petal and a captured live helper. A loading document is a retryable refusal; wrong content is never reported as a successful page identity.

`fe.py portal-fixture capture --out shots/portal-one.png` saves actual owned WebView2 CapturePreview bytes plus `portal-one.png.json`. Output requires an existing destination directory beneath the selected run; the client never creates directories. It rejects Windows ADS, reserved device names, drive-relative paths, control characters and trailing-dot/space components before path resolution, and refuses overwrite, traversal outside the run and redirected ancestors. Windows directory handles pin every existing ancestor before either exclusive PNG/receipt creation, with both evidence handles retained through their writes. Failed writes may leave partial new evidence; they never report success or replace prior evidence. J02 lists the pending negative path cases. The client validates the native source marker, PNG dimensions and SHA-256 against the response receipt and prints paths/metadata without image base64. A capture proves the native source and bytes, not visual correctness; inspect the actual PNG independently. All four commands retain ordinary before/after log observations. The third-link negative may intentionally produce the native policy error, which still needs an explicit expected-refusal log review.

For J02: open saved fixture page one in the app, observe and capture, second-link then observe/capture page two, use the actual app Back and Forward controls and repeat native observations/captures, Reload, then refused-third-link and verify its denial plus the unchanged native page. Close and ensure further native operations refuse. Input off, Viewer, foreign scope, missing/expired helper, changed page/popup/scope and callback timeout remain separate negative checkpoints. Source authoring and journey extensions are not execution evidence.

## Catalog source census

`qa/feature-checkpoints.json` remains the canonical capability/variant denominator. Its families own regions,
journeys, concrete entries/outcomes and source paths. `checkpoint_notes` preserves retired IDs and reasons;
excluded IDs are never PASS and do not remain in executable gold membership. New source controls extend the
catalog before any coverage claim. `audited_source_commit` identifies this source review; the final staged
fingerprint stays null until the main owner reconciles the final build inputs. No historical PASS is imported.

`catalog_source_rules.json` supplies explicit shared-source and action ownership where a file serves multiple
families. `catalog_sources.py` uses only stdlib file reads and writes the catalog views; it never calls Git,
launches processes/apps, reads token stores or accesses network. After reviewing source and the canonical JSON,
use the actual audited full commit (this snapshot is 84ca20af4a47951cf9dbd22d457526a0840291e6):

```powershell
python tools/qa/catalog_sources.py --source-commit 84ca20af4a47951cf9dbd22d457526a0840291e6 --write
```

The command refreshes `qa/catalog-source-census.json`, `qa/source-inventory.md`, the feature table and each
J01–J11 checkpoint matrix. Hand-authored journey steps remain intact. Without `--write` it prints only census
counts, unmapped owners and missing endpoint IDs. It does not mutate the canonical catalog or fingerprint.
Review every unmapped owner and endpoint delta; fail an audit rather than assigning a convenient unrelated
family. A family owner is responsibility, not evidence that every caller's feature was exercised.

The census is deliberately lexical: literal cfg(test) items are blanked with line numbers preserved; string
delimiters are balanced; comments/macros/dynamic wrappers are not a Rust AST. A candidate can be a naming wrapper
or helper for the same control. Some legacy UiAction adapters have no current GUI constructor. Actual dynamic
children, disabled reasons, overlay content, pixel geometry and per-control state variants require source plus
the live tree and judged screenshots. Candidate count is never a coverage denominator or an execution result.
Normalized production-source hashes are census provenance only, not the campaign build-input fingerprint.

For current Portal catalog obligations see J02: captured exact `/web/one` and `/web/two` QA capabilities now
exist in the audited source. The older fixture paragraph above describes the pre-capability integration state;
starting the helper still cannot itself grant permission or prove embedded browsing. Only the main owner may
prepare/relaunch the exact controlled fixture and test the live listener/policy boundaries.

The current followup includes terminal Settings Reset, typed Portal storage/explicit initialization, fixed native fixture APIs and durable REST/legacy WebSocket credential validity. Explicit action ownership keys must name a parsed UiAction variant; stale names fail generation instead of falling through silently. The current stamp viewport/context/index producers carry live origins and the terminal watcher retains its original process/EOF evidence. F143 separates actual observer admission from synthetic validator-only cases; the latter provide zero application coverage. P2P QA-0050 remains pending source reconciliation. Generation does not clear that list or grant a final fingerprint.

## One-use invitation transfer

fe.py --run <owner-run> transfer-invite --recipient-run <recipient-run> consumes only the
latest actual GUI-generated invitation. The Owner Access panel must remain open, and the
recipient must have a clean empty masked Join dialog. Observe the recipient actual DID from
GET /api/v1/debug/invitation before generating the addressed invitation for that DID. The
separate consume endpoint is default-off with input and Owner gates; its fifteen-second
lease is not the envelope lifetime. Expiry or a changed target requires explicit generation.

invite_transfer.py retains both Windows process handles, matching the recorded executable
and creation ticks before use and checking liveness around requests. SDK signatures follow
local Windows SDK10.0.22621.0 processthreadsapi.h, winbase.h and synchapi.h. It pins actual
host-returned DIDs, exact generation/target/recipient/role and distinct run origins. Consumption
and the actual masked text-plus-Join-button input batch are each sent once, without call()
retry, clipboard reads, argument echo, log capture, screenshots or credential files/artifacts.
Only public metadata and an unconfirmed-until-real-readiness submission receipt are printed.
The credential response and input text remain in process memory; failure after consumption
requires a new invitation and must not replay the batch. Existing A8 clipboard suppression
remains in force. Root owns eventual two-app runtime execution; source presence is not QA PASS.
