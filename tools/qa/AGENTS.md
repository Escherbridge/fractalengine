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
into a run. The staged exe has no adjacent `assets` directory. Native save
locations remain user-selected until the file-dialog seam is supplied.

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

## §fixture-basic

`launch.ps1 -Fixture basic` (the default) builds, over REST, using the
bootstrap flow above:

```
QA Basic (verse)
└── QA Fractal
    ├── Petal A — "Cube 1/2/3" nodes, all bound to one uploaded
    │             fractalengine/assets/samples/info_panel.glb asset
    └── Petal B — terrain-bound IF a TerrainTileset hexon is installed
                  from sample-hexons/dist/ (install_sample_hexons.exe,
                  staged alongside the exe), else left plain and logged.
```

As of this writing all three sample hexons under `sample-hexons/dist/`
(`alpine-demo-terrain`, `lakeside-scene`, `morning-run-gpx`) are `Terrain`,
`Scene` and `GpxCollection` kind respectively — `install_sample_hexons`
expects `TerrainTileset` and skips all three. So the terrain-binding step
is exercised as a no-op path today; `fixture_detail.terrain_bound` in
`current.json` says whether it actually ran. The PUT body it would send
(`fe_terrain::config::TerrainConfig`, DEC-C22's strictest-reader contract)
is `{enabled, origin:{origin_lat,origin_lon,origin_ele}, tile_source_url,
elevation_source, tile_source_mode, tileset_hexon_uris:[<hexon_id>]}` —
`tileset_hexon_uris` takes the bare hexon id (`fe-ui/src/terrain_map/mod.rs`
confirms this, no `hexon://` scheme prefix).

`current.json` (written to the run dir, pointed at by `.qa/current`) records
`fixture_detail` with every id a later QA-loop script needs:
`verse_id`, `fractal_id`, `petal_a_id`, `petal_b_id`, `asset_id`,
`node_ids`, `terrain_bound`.

## §fe-py

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
