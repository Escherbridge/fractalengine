# Hand-off: finish terrain per-petal maps + unblock the workspace build

## Mission
A large slice of terrain work is **implemented and unit-tested but uncommitted**, and the
full workspace **cannot build end-to-end** because `fe-sync` has pre-existing iroh-0.35
migration breakage (not from this work). Your job: (1) fix `fe-sync` so the workspace
compiles, (2) run one full build + test sweep, (3) build the `fractalengine` binary, and
(4) commit the whole scope on a branch. Do **not** re-architect the terrain work — it is
reviewed and green in isolation; you are unblocking and landing it.

Repo: `c:\Users\atooz\Programming\fractalengine-workspace\fractalengine`
Branch: `checkpoint/pre-tauri-impl` (create a feature branch before committing).
Rust workspace, Bevy 0.18, SurrealDB 3.0, iroh 0.35. Windows host; containers via podman.

---

## What is already done (verify, don't redo)

All of the following is on disk, uncommitted (~60 changed/new paths), and passed tests in an
isolated worktree:

1. **Per-petal map selection ("choose a map for this petal")** — end to end:
   - `DbCommand::{SetPetalTerrain, GetPetalTerrain}` + `DbResult::PetalTerrainLoaded`
     in `fe-runtime/src/messages.rs`; handler `fe-database/src/handlers/petal_terrain.rs`
     (dispatch arms in `fe-database/src/lib.rs`).
   - UI "Map" column in `fe-ui/src/hexon_manager.rs`; `UiAction::PetalSetMap`,
     `PetalMapState`, `PendingHexonOps`/`HexonOp` in `fe-ui/src/plugin.rs`.
   - Bridge `fractalengine/src/terrain_bridge.rs` (`bridge_petal_terrain`, `drain_hexon_ops`);
     `TerrainPlugin` + `SharedTilesetRegistry` + registry now wired in `fractalengine/src/main.rs`
     (closed the old `tileset_registry: None` TODO).
   - Runtime `fe-terrain/src/petal_binding.rs` (`terrain_config_from_petal_json`,
     `config_for_tileset`, render-gated `ActivePetalTerrain`/`ActiveTileSource`/`apply_terrain_assignments`).
2. **Hardened terrain rendering** in `fe-terrain/src/terrain_plugin.rs` (fixed per-frame
   asset leak, GeoJSON infinite respawn, dead LOD code, offline-first chunk spawner) +
   `get_satellite_tile_sync` in `fe-terrain/src/tiles/composite.rs`. Design notes in
   `fe-terrain/src/AGENTS.md`.
3. **sample-hexons/** demo packages + `cargo run -p fe-hexon --example build_sample_hexons`.
4. **fe-hexon-registry** crate (hosted registry HTTP service) + `docker/Dockerfile.hexon-registry`
   + `docker/compose.dev.yml`, mirroring the relay container pattern. `fe-hexon` gained a
   `remote` feature (`RemoteRegistryClient`).
5. Fixed a pre-existing stale test: `fe-database/src/schema.rs` `all_table_names_are_present`
   (was 12, now 14 after the crate-registry track).

**Sibling repo already committed** (do not touch unless asked):
`c:\Users\atooz\Programming\fractalengine-workspace\gis-tile-etl` (commit `c99a4dd`) — the
config-driven US-region tile ETL that produces real terrain hexons from public APIs.

Conventions: terse one-line doc comments; rationale in directory-level `AGENTS.md`, not inline
blocks. `HexonArchive` (fe-format) is the packing path consumers load through — **do not** use
`fe_hexon::HexonPackage` (its manifest type is incompatible).

---

## TASK 1 — Fix `fe-sync` iroh 0.35 migration (the blocker)

`fe-sync/src/{sync_thread.rs, replicator.rs, status.rs, messages.rs}` are pre-existing
uncommitted WIP from the Pears/iroh-upgrade track. `cargo check -p fe-sync` fails with **27
errors**. None are from the terrain work. Root causes and where the iroh-0.35 types now live
(verified against the installed crate sources):

| Broken reference | iroh 0.35 location / fix |
|---|---|
| `iroh_gossip::TopicId` (sync_thread.rs:105,502,575,608,667) | moved to `iroh_gossip::proto::TopicId` |
| `iroh_gossip::Host` (sync_thread.rs:88,501,574,607,666) | removed; the gossip endpoint type is now `iroh_gossip::net::Gossip` — rework call sites to the 0.35 gossip API |
| `iroh_gossip::Topic` (sync_thread.rs:521,591) | removed; subscriptions now go through `iroh_gossip::net::Gossip` (returns a `GossipTopic` handle) |
| `iroh_docs::Engine` (replicator.rs:224,239,250; sync_thread.rs:27,31) | now `iroh_docs::engine::Engine<D>` — **note it is now generic over the store type `D`**; thread that type param through |
| `iroh_docs::Document` (replicator.rs:285) | now `iroh_docs::rpc::client::docs::Doc` (a.k.a. the docs client `Doc`) |
| `[u8;32]::as_bytes` (replicator.rs:352) | `TopicId` wraps `[u8;32]`; use the array directly or `.as_ref()`/`AsRef<[u8]>` |
| type-inference failures (replicator.rs:356,395,396,430; sync_thread.rs:31) | cascade from the above; resolve once the real types are in scope |
| `borrow of moved value: engine_holder` (sync_thread.rs:342) | ownership bug — clone or restructure so it isn't used after move |
| non-exhaustive match `SyncEvent::NodeTransformed` (status.rs:49) | **independent of iroh** — `NodeTransformed` exists in `fe-sync/src/messages.rs:147`; add the missing arm (handle or explicitly ignore) |

Approach: consult the iroh 0.35 / iroh-docs 0.35 / iroh-gossip 0.35 docs before editing (their
APIs restructured significantly from earlier versions — do not guess signatures). The
`iroh-docs` `Engine<D>` generic and the gossip subscription flow are the two substantive
changes; the rest are import-path/rename fixes. Keep behavior equivalent to the WIP intent;
if a construct has no clean 0.35 equivalent, leave a one-line `// TODO(iroh-0.35):` and a note
in `fe-sync`'s `AGENTS.md` rather than inventing semantics.

## TASK 2 — Full verification (ONE sweep, at the end)

Per the standing test policy: make all fixes first, then run the sweep once.

```
# from repo root, do NOT set CARGO_TARGET_DIR to a shared/worktree path (see gotcha below)
cargo build --workspace
cargo test  --workspace
cargo build -p fractalengine            # confirm the GUI binary links (Bevy render)
cargo build -p fractalengine-relay      # headless binary still links
```
Terrain render tests need the feature: `cargo test -p fe-terrain --features render`.
Registry + remote client: `cargo test -p fe-hexon-registry` and
`cargo test -p fe-hexon --features remote`. Expect green:
fe-database (121), fe-terrain (7+2 + helpers), fe-runtime, fe-ui, fe-hexon lib (48),
fe-hexon-registry (11), fe-hexon remote (2). Fix anything that regresses.

## TASK 3 — Commit the scope

Create a feature branch off `checkpoint/pre-tauri-impl`. Group into reviewable commits, e.g.:
`fe-runtime/fe-database petal-terrain messages+handler`, `fe-terrain hardening + petal_binding`,
`fe-ui petal map picker`, `fractalengine terrain wiring + bridge`, `sample-hexons`,
`fe-hexon-registry + docker`, `fe-sync iroh 0.35 migration`, `fe-database schema test fix`.
End commit messages with:
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
Do not push unless the user asks.

---

## Acceptance criteria
- [ ] `cargo build --workspace` and `cargo test --workspace` both green.
- [ ] `fractalengine` and `fractalengine-relay` binaries build.
- [ ] `cargo run -p fe-hexon --example build_sample_hexons` produces `sample-hexons/dist/*.hexon`.
- [ ] No terrain/petal-map behavior changed vs. the isolated-worktree runs.
- [ ] Work committed on a feature branch; `fe-sync` migration notes captured in its `AGENTS.md`.

## Gotchas
- **Do not point `CARGO_TARGET_DIR` at a git-worktree while also building the live tree** —
  it corrupts incremental fingerprints and produced phantom "variant not found in `DbCommand`"
  errors this session. If you see enum variants reported missing that clearly exist in source,
  `cargo clean -p fe-runtime -p fe-database -p fe-sync` and rebuild.
- fe-sync's iroh work is the *only* thing blocking the workspace; every other crate compiles
  and tests clean against a working fe-sync.
- `fe-ui` must not depend on `fe-terrain` (it builds terrain config JSON via serde_json); keep
  that boundary.
