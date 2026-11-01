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

## §cwd-isolation

`fractalengine/src/main.rs`'s `DB_PATH` ("data/fractalengine.db") and
`FsBlobStore::default_path()` ("data/blobs") are both CWD-relative, so
launching each run with its own working directory isolates its DB and blob
store — **except** `fe_runtime::app::packaged_root()` checks `<exe
dir>/assets.is_dir()` and, if true, `std::env::set_current_dir`s the whole
process into the exe's own directory before `DB_PATH` is ever touched. Stage
an "assets" folder beside the staged exe (as the task text's "copy
everything it loads at runtime" literally suggested) and EVERY run from
that stage would collide on the same `data/` under the stage dir, silently
defeating the isolation `launch.ps1` exists to provide.

The fix: `stage.ps1` stages the exe ALONE (no assets beside it), and
`launch.ps1` junctions `<run>/fractalengine/assets` -> the repo's real
`fractalengine/assets` (`cmd /c mklink /J`, no copy, no admin rights needed
for a same-volume junction). That keeps `packaged_root()` returning `None`
for every launch, which routes `file_asset_root()` through the
"workspace checkout" branch (`<cwd>/fractalengine/assets`) instead — CWD
stays the run dir, DB/blob isolation holds. Verified empirically (not just
asserted): two consecutive runs from the same stage each got their own
`data/fractalengine.db` with independent Genesis-seed verse ids.

Caveat: the junction makes `<run>/fractalengine/assets/imported/` an alias
for the REAL repo's legacy GLB import cache, not a copy. That subtree is
documented in the repo's own `.gitignore` as a "read-only fallback", and
nothing observed in this slice wrote to it, but a future write there from a
QA run would land in the real working tree. Worth re-checking if an
`import` flow is ever added to the agent-driven QA loop.

## §bootstrap (why there's no "seed a verse before first launch" step)

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

## §process-tree (why `stop.ps1` isn't one `Stop-Process`)

The Tauri webview backend (`fe-webview`, default since 2026-06-30) spawns a
child `msedgewebview2.exe` under the GUI's own pid — verified with
`Get-CimInstance Win32_Process -Filter "ParentProcessId=<pid>"` against a
live run. Stopping only the root pid would strand that child (and whatever
it spawns in turn — WebView2 fans out into several helper processes).
`Get-QaProcessTree` in `_lib.ps1` does a BFS over `ParentProcessId` from the
exact recorded pid and `stop.ps1` kills every id in that result, never by
name or pattern: this machine runs other, unrelated `msedgewebview2.exe`
processes (editor extensions, other browsers) that must never be touched.

## §fixture-basic

`launch.ps1 -Fixture basic` (the default) builds, over REST, using the
bootstrap flow above:

```
QA Basic (verse)
└── QA Fractal
    ├── Petal A — "Cube 1/2/3" nodes, all bound to one uploaded
    │             fe-test-harness/src/fixtures/test_cube.glb asset
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
