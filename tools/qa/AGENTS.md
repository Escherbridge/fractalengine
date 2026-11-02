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

## Locator visibility

Named input intersects a same-pass observed control's hit rectangle, clip and
primary window before choosing its center. Empty, negative or disallowed hits
are refused. Exact names take priority before visibility filtering, so an
invisible exact target cannot fall back to a similarly named visible action.
Scrolling an off-clip menu item into view is a real prerequisite, even when
AccessKit still reports its full bounds and hidden=false. Input results expose
the chosen rectangle and whether geometry came from observed hit/clip data or
an explicit bounds/window fallback with clipping unknown.
