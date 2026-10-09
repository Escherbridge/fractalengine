---
type: Plan
track: mission_prodready_continuation_20261008
title: "Execution plan — F9–F19"
timestamp: 2026-10-08T23:00:00-07:00
---

# Plan — F9–F19 continuation

Per-feature status board. Statuses: `pending` | `in_progress` | `landed` (committed,
crate gates green) | `validated` (milestone review passed). Each landed feature gets
its commit hash + evidence line appended here.

## M3 — Simulation lab (finish)

- [ ] **F9** (A20+A21) — `in_progress`; **part A landed @ `1f2b6fd`** (factory's partial fe-sync diff kept, DEC-C2)
  - [x] fe-sync gossip-plane seam: `TopicSender`, `VirtualGossipTopic`, subscribe/pump wiring (inherited diff; compile fixes in part A)
  - [x] fe-sim hub-side gossip plane: DID-sorted deterministic fan-out, miss-forever semantics (`net.rs gossip_broadcast`)
  - [x] `SimTransportFactory::join_gossip_topic` wiring + identity tagging (`net.rs:960`)
  - [x] CI scenario 1: `fe-sim/scenarios/sharded_query.json` — merged aggregate == union oracle, full coverage, fingerprint-stable
  - [x] CI scenario 2: `fe-sim/scenarios/offline_degraded.json` — 6/3 missing shards w/ `missing_hosts=["alice"]` during outage, 18/0 after heal
  - [x] f64 1-ULP executed (DEC-C6): serde_json `float_roundtrip` workspace-wide; 0/200k deltas (was 5,395/200k); bit-exact replica asserts
  - [x] **part B landed @ `4854227`**: fe-runtime sim_control contract, fe-sim ScenarioSession (run_scenario = 3 lines over it; equivalence + live-fault-matches-scripted fingerprint tests), control bridge (concurrent start → Conflict before the lock), REST /api/v1/sim/* + 5 MCP tools (EXPECTED_TOOLS 11→16), relay-only default-off `sim-control` feature (dep-tree proof 0)
  - Part A gate (orchestrator-run, serial): fe-sim 33/33, fe-sync 221/221, fe-database 275 pass, clippy -D warnings + fmt clean
  - Part B gate (orchestrator-run, serial): fe-sim 37/37, fe-runtime 92, fe-api 165, harness 26; clippy default+feature clean; fmt clean
  - Part A extras: F8 bugfix (event at_ms now absolute — faults actually fire mid-run), determinism hardening (seeded identities, write-order replays, exact barriers)
  - Open items → F13: sim guard is role-only (add global/admin ScopeRule); fold sim_* arms into the ToolSpec table. → F17: relay README sim warning. Known: process-global HLC override on a sim-control relay (documented, loud log).
- [x] **M3 milestone review** (2026-10-09, adversarial opus, read-only): **PASS-WITH-FIXES** — findings 1-3 mandatory:
  1. MAJOR bounded-channel deadlock cycle: replica `ReplicatedRowApplied` results have no drainer while `TestPeer::send` blocks → fleets retaining ≳129 rows hang forever past the settle deadline (peer.rs:101/876-883, scenario.rs:632-636). CI scenarios stay under threshold — green CI masked it.
  2. MAJOR process-global HLC skew: sim `start_ms` in the future persists into production stamps across restarts (op_log forward-only + init_hlc(max_persisted)); past start_ms breaks monotonicity; ≥2^48 truncates; chrono-range panics abort the relay. Role guard = Owner of ANY verse.
  3. MAJOR F24 ingest correlation: late reply after 10s timeout delivers to the NEXT waiting client (app.rs:338-347 skips closed entries; no petal/correlation check in iot.rs fallback).
  - Minors batched into the fix pass: #4 validate caps (readings/peers), #6 self-echo drift vs real iroh-gossip (skip author; fix the gossip_deliveries>0 assert), #7 exact min/max/count compares + shard-membership assert, #8 merge_actions position test, #10 doc-comment hijack.
  - Deferred with triggers: #5 step idempotency (→ document; revisit if a real client retries), #9 F24 hydration/drain startup race (→ F19 sweep note), MCP sim tools advertised on GUI (→ F13 table refactor), auto_pump dead code (→ F13/F17 decide expose-or-remove), 1-ULP legacy rows (none in prod; no backfill).
  - Claims verified to hold: real-gossip path drift-free; float_roundtrip safe (no canonical-path float parsing); lock poisoning recoverable; teardown order correct; scenarios include_str-coupled (no drift).
  - **Fixes LANDED @ `2023809`** (combined with F11 — change sets interleave in relay main.rs). Verdict now: **M3 VALIDATED**. Gate: fe-runtime 93, fe-database 277, fe-sync 221, fe-sim 43, fe-query 123, fe-api all suites, harness 26 + 14/14 real-transport scenarios.
  - Residual risk (documented, accepted): co-resident production DB-thread stamps DURING a sim session read sim time — restore can't undo those; mitigations are the start_ms<=now validation + FE_SIM_ALLOW + lab-relay-only doctrine (F17 README).

> **Ops discipline (DEC-C12, extended 2026-10-09)**: every gate starts with a
> disk check; if C: free < 10 GB, prune `target/debug/incremental` + stale
> PDBs first (29.7 GB + 128 GB reclaimed this mission; C: is a 1.9 TB drive
> running ~100% full from non-project data too — flagged to the user). Builds
> strictly serialized — INCLUDING no source/Cargo.toml edits while a build
> runs (a mid-build manifest edit invalidated fingerprints and stacked a
> second 100+ GB artifact generation; target/debug hit 236 GB and the disk
> hit 0, killing two F19 sweep attempts with LLVM IO failures). Workspace
> sweeps run with `CARGO_PROFILE_DEV_DEBUG=0` (tests need no symbols;
> several-fold smaller artifacts, lower rustc memory → -j4 safe).
- [ ] **M3 milestone review** — adversarial pass over F24+F8+F9 (scrutiny pattern)

## M4 — BI egress (DuckDB-first) — order F11 → F10 → F12 (DEC-C11); map in m4-bi-egress-design.md

- [x] **F11 LANDED @ `2023809`** (A23+A24) — two-table export whitelist (classify_export_table), batched anchor join (node_id IN $ids, one query per page), latlon via petal projection on the anchor, nullable geometry for dead anchors, f64 bit-exact ReadingSnapshot; readings parquet writer/codec in fe-query; share_signer required ApiConfig field + dedicated keystore slot both binaries (DEC-C9), restart-persistence + foreign-key-401 tests. Open items for F10: nullable WKB cells, Float64 value / Int64 recorded_at_ms schema asserts.
- [x] **F10 LANDED @ `2a4026e`** (A22) — scripts/bi-egress-verify.ps1 + recorded log: 31/31 live checks (real relay + DuckDB 1.5.6 + curl). DEC-C10 → IMPLEMENTED single-range 206 (DuckDB httpfs issues real ranged GETs, observed live). Windows export 503 hole fixed (run_guarded_query_via_state channel fallback; /query + share json branch deferred w/ AGENTS.md note). Mint path: offline mint_api_token example + shared node-keypair seed env (no HTTP route by design). Fold-ins (a)-(e) done; (c) verified genuine misnomer and renamed. NEW: GeoParquet crs spec violation found live → DEC-C16 (PROJJSON-null fix pending); script uses enable_geoparquet_conversion=false meanwhile. Gate: fe-api 182, relay 7/7, clippy/fmt clean.
- [x] **F12 LANDED** (A25) — docs/bi-egress.md (240 lines): every command sourced from the 31/31 verified script/log; DuckDB leg marked live-verified, PowerBI/spreadsheet legs honestly marked derived-not-script-verified; troubleshooting covers the four real failure modes. (/api/v1/query distributed-shape doc fix landed with F10.) README cross-link deferred to F17 (owns README).
- [~] **M4 milestone review** (2026-10-09, adversarial opus, read-only): **FAIL** — fix pass required (DEC-C17, runs after F13 lands due to shared files):
  - BLOCKER 1: scope filter is a bypassable string append (double-space defeats the `FROM NODE`/`FROM IOT_READING` substring match → zero injection; OR-precedence; `--` comments; projection subqueries are whitelisted) — cross-petal/verse leak via export + PUBLIC share URLs (query_guard.rs:230-244).
  - BLOCKER 2: RawQuery/QueryResult carries no correlation_id — F10's Windows fallback can deliver caller A's rows to caller B on the public share surface; on the GUI a share-redeemer can receive the desktop user's ad-hoc Query-tab results (app.rs:398-409 skip-closed + fe-ui db_results routing). Same class M3 fixed for ingest.
  - Majors: no ETag/If-Range (ranged reads can tear across live writes; 3 full exports per read_parquet), fallback lacks a DB-side statement timeout, A24 overstated (test shares an Arc — persistence never exercised; relay silently ephemeral when env unset), docs fail followed-literally (token extraction, step order, 7 factual errors).
  - Holds: Range parser correct incl. edge cases + share path uses same body_response; fallback preserves scope-injection point + caps; anchor join petal-bound; both parquets genuinely read by DuckDB; f64 bit-exactness genuine.
  - DEC-C18 amends A22's "immutable cache" line (live data: ETag + no-store, immutable stays assets-only).
  - **Fix pass LANDED @ `667aa18`** (DEC-C17 executed in full + bonus FROM-expression bypass found & fixed): scope-source substitution (filtered subquery replaces the table — user SQL can't widen it) + egress petal_id re-check (defense in depth); RawQuery/QueryResult/QueryFailed correlation (uncorrelated↔uncorrelated only; fe-ui Query tab can no longer receive API rows either); blake3 ETag + If-Range + private,no-store; SurrealQL TIMEOUT 5s both paths (newline-guarded; empty-success bug fixed via Response::check); A24 restated + relay startup warning + EnvBackend test + harness signer decoupled; docs corrected (read_csv now verified). Acceptance: e2e 48/48; fe-api 211, fe-runtime 95, fe-identity 50, 17 db/query/harness binaries, 0 failures.
  - **Re-review (2026-10-09): STILL-FAILING, narrowly** — all original vectors verified closed (FROM-substitution sound vs homoglyphs/case/strings; egress re-check drops missing-petal_id rows; correlation routing clean; ETag over full body pre-slice; docs fixed). NEW: N1 HIGH node_log side channel (payload carries petal_id/name/position; whitelisted but unscoped — the fix-pass's "only node_id" premise was wrong); N2 MED correlated waiters can receive GUI Error text via deliver_to_oldest; N3 MED fallback timeout ordering false under queueing; N4 LOW e2e probes can't isolate the rewrite from the backstop. Remediation per DEC-C19 in flight (+ independent denylist-vs-grammar check). Lesson named by the reviewer: a scoped-table list is itself a denylist — node_log was the denormalized copy one table over.
  - Remediation (DEC-C19) + grammar-audit R5 (DEC-C20) **LANDED @ `770acd3`**: node_log removed from ALLOWED_TABLES, PETAL/ROOM/MODEL/CRATE_REGISTRY row-scoped, comment ban unconditional in all guard modes + comment-excised needle scan (two independent layers), correlated-error isolation in deliver_to_oldest, 4-permit fallback semaphore w/ honest 503 (share surface included), rewrite-isolation e2e probe. Grammar audit: 18 SurrealQL form classes enumerated vs the vendored 3.0.5 parser — 1 critical (fixed), 17 sound/harmless (unicode homoglyphs impossible by lexer construction). Final combined gate: fe-api 224, fe-runtime 97, fe-identity 50, harness 26 + 15/15 scenarios, **e2e 49/49**.
- [x] **M4 milestone review — VALIDATED** (FAIL → fix pass → re-review STILL-FAILING → remediation + audit → all mandatory items closed, gate green)

## M5 — MCP + API integration

- [x] **F13 LANDED @ `635b8c5`** (A26) — one ToolSpec table drives definitions + dispatch (zero inline require_* in handlers); 29 tools (DEC-C14: 20-name vocabulary complete + 9 documented extras); FOUR authz holes closed w/ denial tests (3 original warts incl. the decoy variant + depth-escalation caught by automated security review on the new code itself — HierarchyArgs(HierarchyTarget) anchors at the write-target level, deeper ids rejected); GLB upload API-side via the StoredGlb capability type (bytes structurally cannot cross the channel) + REST sibling; side fix: REST create_waypoint panicked on every call. Gate: fe-api 199, combined 572+, harness 26, clippy/fmt clean (-j2). Open: MCP install_tileset Manager vs REST Editor+policy; set_petal_terrain validate-then-refuse (needs correlated reply, touches fe-ui); FR-3 optional create_node asset args → F14; asset rows unscoped (deferred w/ trigger).
- [x] **F14 LANDED @ `a5f0b7b`** (A27) — live WS upgrade e2e (real listener, production entity_change channel), hexon multipart install/list/meta/tile round-trip + foreign-petal 403 (+ ApiHarness tileset-registry seam), token lifecycle through the LIVE middleware (per-jti revocation proof; mint_api_token_at defeats jsonwebtoken's 60s leeway), api_db_sync_cross_thread harness scenario (real ApiState w/ db_reader None → real DB thread → real loopback iroh → second-peer durable read-back; TestPeer ResolvePetalScope implemented for real), 725-call seeded MCP fuzz, 21 bare-.ok() conversions. FR-3 optional create_node args deferred w/ rationale (place_asset covers the flow end-to-end).
- [~] **M5 milestone review** (2026-10-09, adversarial opus): **FAIL** — fix pass per DEC-C21 in flight. Mandatory: (1) CRITICAL REST create_node/create_petal still trust URL ancestry ids (the decoy wart's REST twins — the MCP choke point didn't cover the other transport); (2) HIGH /mcp 349 MiB body limit = memory-exhaustion DoS for any token (args cloned; ~1.3 GB per max upload call, no concurrency limit); (3) HIGH uploaded GLBs world-readable (ASSET unscoped in the whitelist + get_asset by-hash unauthenticated + place_asset accepts foreign asset_id — M5 turned local imports into multi-tenant ingest, converting a known gap into a live confidentiality break); (4) HIGH the F14 fuzz test predicted deterministically red (fire-and-forget TransformPersist logged between snapshot and assert; 9 tools never reach their handlers; "725 calls" is really 650). Mediums folded: install_tileset role unify + decode cap + spawn_blocking; import_gpx point cap; promote_instance path-squatting check; mint_api_token_at TTL cap. Holds: dispatcher gate sound, depth hardening works, bytes-never-cross-channel confirmed (via the command variant's shape), RBAC matrix really iterates the table, MCP query inherits the hardened guard cleanly. Reviewer's pattern note: shared cores + separate guards is how drift creeps back — the table protects only what routes through it.
  - **Fix pass LANDED @ `5b06795` — M5 VALIDATED-WITH-NOTES** (orchestrator verdict: all 4 mandatory items fixed with pinning tests; the sweep found a FOURTH hole — field-defs by-id mutations had NO scope check — also fixed; the repaired fuzz test's first real run is green). ancestry_matches is now ONE shared helper across REST+MCP (anti-drift by construction). Client-visible changes logged in the commit. Notes (deferred w/ triggers in AGENTS.md): blob GC/quota, remaining REST existence-leak message unification, /query/elevated scope filter, REST tileset-install 2MiB effective cap (axum default — real tilesets may exceed; trigger: first >2MiB install need), strict missing-path_id promote check. Gate: fe-api 234, fe-identity 51, fe-database 281, fe-terrain 218, harness 26+15/15, e2e 49/49, clippy/fmt clean.

## M6 — GIS hexon examples

- [x] **F15 LANDED** (A28) — gis-tile-etl @ `33eff03`, main-repo provenance @ `dbb6461`: configs/intl-regions.toml (esri-world-imagery + aws-terrarium, DEC-C8), Zurich Alps (10+10 tiles, 1.48 MB) + Mount Fuji (8+8, 0.84 MB) built + `Verify: OK`; 17/17 tests; sink fixed for fe-format TilesetMeta drift (scale fields None → app-side backfill). Flag for fe-format owners: derive(Default) on TilesetMeta would stop sibling-repo struct-literal breakage on future field adds.
- [x] **F16 verified (commit pending w/ M5 fix batch)** (A29) — V1 14/14: idempotent install of both F15 archives + meta/bounds/counts + scale backfill asserted against an INDEPENDENT Web-Mercator GSD oracle (103.57 m/px Zurich, 124.65 Fuji; env-gated test fe-terrain/tests/gis_hexon_install_test.rs skips cleanly without the sibling repo). V2 37/37 live relay: list excludes unbound, elevation PNG + satellite JPEG served, 403 scope-mismatch vs 404 unbound-without-existence-leak (terrain.rs:340-353 ordering). Honest ledger: in-app render + scale-bar WIDGET remain user-gated (the numeric fields they read are verified).
  - **Product gap found (deferred w/ trigger)**: NO authenticated path binds a tileset to a petal on a live relay — TERRAIN_MUTATION_UNAVAILABLE blocks REST PUT/DELETE terrain AND MCP set_petal_terrain pending correlated command replies (the F13 open item, now confirmed to cover REST too). Workaround: stop relay → seed_join_verse bind-terrain (new subcommand) → restart. Trigger to fix: correlate SetPetalTerrain replies (touches fe-ui construction sites).
- [x] **M6 milestone review — VALIDATED** (PASS-WITH-FIXES → fixes landed): HIGH bind-terrain partial-JSON shape (satisfied fe-api's lenient reader, rejected by fe-terrain's strict parser — now serializes a real TerrainConfig via a clean dev-dep; reviewer's rule recorded: a writer of a shared JSON column must satisfy the STRICTEST reader); MEDIUM script assertions now teed into committed logs; LOW env-gated test → #[ignore] + --ignored (no silent skip-as-green; -q dropped so the ran-and-passed assert sees test names). Acceptance re-runs: install 15/15, serve 37/37. A28/A29 claims verified sound incl. Esri attribution honesty and the GSD oracle math (~0.03% agreement).

## M7 — Close-out

- [x] **F17 LANDED @ `7b11c1a`** (A30) — root README P2P section rewritten (real stack; honest legacy-petal-mock residual kept), relay README ops surface (env table, process-lifetime verse secrets + workaround, Windows per-handle-lock note, sim-control double opt-in), new docs/simulation-lab.md, fe-sync §iroh-0.35 de-staled (heading + false gossip-drain claim corrected vs code). bi-egress.md cross-linked. Every claim source-verified; M4-blocker-dependent claims deliberately omitted pending the fix pass.
- [x] **F18 LANDED @ `f7cc47b`** (A31) — 5 absorbed tracks VALIDATED+closed (honest aggregate-coverage note on api_mcp), 3 record tracks created (pointers, never rewrites), tracks.md + roadmap.md corrected, factory board synced. Retro below.
- [x] **F19 GREEN** (A32+A33) — full sweep on the final tree: `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean (5m21s); `cargo test --workspace` **3,245 passed / 0 failed / 21 ignored** across 96 binaries (clean debuginfo=0 rebuild after the disk incident); harness scenarios 15/15 (real loopback iroh); `cargo build --release -p fractalengine-relay` succeeded (30m19s); git: local main ahead 30, NO pushes, stale sibling forks untouched (they are pruned worktree stubs, never written). M6 acceptance: install 15/15, serve 37/37.

## Retro (2026-10-09, mission close)

**Delivered**: F9–F19 (A20–A33), 27 feature/fix commits on local main (unpushed
per A33), continuing factory's F1–F8. Every milestone got an adversarial
review; every review found real issues green CI had missed:
- M3: a bounded-channel deadlock cycle (fleets ≳129 retained rows hung
  forever), permanent production-HLC skew from sim time, ingest reply
  cross-delivery.
- M4: string-append scope filter bypassable 4 ways on PUBLIC share URLs;
  uncorrelated query replies cross-delivering; then the re-review found
  node_log as a side channel and the grammar audit found the comment-split
  needle bypass — two independent layers now guard each.
- M5: the decoy wart's REST twins (the dispatcher protected only MCP), a
  349 MiB DoS surface, world-readable uploaded assets, a deterministically
  red fuzz test; the fix sweep found a FOURTH hole (field-defs unscoped).
- M6: a two-readers-one-column seam bug that would have silently rendered
  no terrain in-app despite 37/37 API checks.

**Lessons (named by reviewers, now track doctrine)**:
1. A scoped-table list is itself a denylist — denormalized copies (node_log)
   live one table over.
2. Shared cores + separate guards = drift; authz chokepoints protect only
   what routes through them (REST twins).
3. A writer of a shared JSON column must satisfy the STRICTEST reader.
4. Serialize builds AND source/manifest edits — a mid-build Cargo.toml edit
   stacked 236 GB of artifacts and zeroed the disk.
5. Workspace sweeps: CARGO_PROFILE_DEV_DEBUG=0 (several-fold smaller,
   -j4-safe); prune incremental+PDBs at thresholds (DEC-C12).

**User-gated residuals** (cannot verify headlessly): in-app terrain render +
scale-bar widget for the installed Zurich/Fuji hexons (numeric fields
machine-verified); the Hexon Manager UI button (its seam is verified);
in-app GUI smoke of the whole build.

**Deferred register (with triggers)** — see spec.md DEC entries: blob
GC/quota (multi-tenant deploy); REST existence-leak message unification;
/query/elevated scope filter; WKB local-frame axes (first GIS consumer);
f32 anchor quantization; SetPetalTerrain reply correlation → unlocks the
live tileset-binding path (TERRAIN_MUTATION_UNAVAILABLE product gap);
per-entry decompression caps (hexon registry go-live); fe-format
TilesetMeta derive(Default) (sibling-repo breakage class); REST tileset
install 2 MiB effective cap (first >2 MiB install).

## Verification evidence log

- F9A `1f2b6fd`, F9B `4854227`: fe-sim 37/37, fe-sync 221, fe-runtime 92, fe-api 165, harness 26; dep-tree proof 0.
- F11+M3fixes `2023809`: combined gate + 14/14 real-transport scenarios.
- F10 `2a4026e`: e2e 31/31 live (first DuckDB-vs-relay verification ever).
- DEC-C16 `1ea14ba`: e2e 31/31 with strict geoparquet validation ON.
- F13 `635b8c5`: fe-api 199; 572 combined; 29-tool authz matrix.
- M4 fixes `667aa18` + remediation `770acd3`: e2e 48/48 → 49/49; grammar audit 18 form classes.
- F12 `a6d23f3`, F15 `33eff03`/`dbb6461`, F17 `7b11c1a`, F18 `f7cc47b`.
- M5 fixes `5b06795` + F16 `0c4bc2d`: fe-api 234, fe-database 281, fe-terrain 218, e2e 49/49.
- F19 final: workspace 3,245/0/21; install 15/15; serve 37/37; harness 15/15; release relay built; no pushes.
