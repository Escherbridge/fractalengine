---
type: Verification Sweep
track: role_propagation_20261013
phase: refine R5 (the campaign's single full verification sweep)
reviewed_range: 4d3a1a6..HEAD (through R4 9f13da4)
date: 2026-10-25
verdict: FIXED
---

# Refine R5: full verification sweep — triage to green

**Verdict: FIXED.** The first full workspace run was 3914 passed / **81 failed** across 18 targets.
Every failure was triaged to a root cause: **6 real product defects** (all fixed), and the rest
deferred tests that had never run (stale fixtures or expectations, all fixed honestly — no
assertion was weakened to green a real bug). The final runs are fully green. No CRITICAL/HIGH
left open. Push stays HELD (user-gated).

## Final evidence (all on the R5 tip)

| Step | Command | Result |
|---|---|---|
| Pre-flight | RAM 2.8 GB avail / 543 GB disk; no LM Studio, no zombie test exes, no cargo running | clean (VS Code holds RAM — not a capacity-kill target) |
| 1. Workspace tests | `CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728 cargo test -j4 --workspace --no-fail-fast` | **4008 passed / 0 failed / 19 ignored** (exit 0) |
| 1b. Post-run deltas | `cargo test -p fe-database --test dcl20_bypass_counter_test` (clippy fix); `cargo test -p fractalengine-test-harness` (scenario edits) | 3/3; 30/30 |
| 2. Harness | `cargo run -p fractalengine-test-harness` (FE_SYNC_RELAY=disabled) ×2 | **18/18 ×2** (was 15/18) |
| 3. Relay acceptance (W6-8) | `FE_REQUIRE_RELAY_CONTAINER=1 FE_CONTAINER_RUNTIME=podman cargo test -p fe-sync --test relay_assisted_test -- --nocapture` | **PASS** — all 4 by name, 4 containers started + removed by exact name, zero `skipping:`, `podman ps -a --filter name=fe-relay-test-` empty |
| 4a. BI egress e2e | `scripts/bi-egress-verify.ps1` | **59/59** (incl. W6 ENU asserts; `INSTALL spatial` succeeded) |
| 4b. Hexon install e2e | `scripts/hexon-install-verify.ps1` | **15/15** |
| 4c. Hexon serve e2e | `scripts/hexon-serve-verify.ps1` | **40/40** |
| 4d. Relay health | `scripts/relay-health-verify.ps1` | **12/12** |
| 4e. Feature matrix (AC-19) | `-p fe-terrain --features fetch,render` / `-p fe-hexon --all-features` / `-p fe-format --no-default-features` | 347 / 81 / 48 passed, 0 failed; `http_source_passes_conformance`, `in_memory_p2p_source_passes_conformance`, `a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling` present **by name** |
| 4f. Store migration | `FE_REQUIRE_STORE_FIXTURE=1 cargo test -p fe-sync --lib p2p_store_migration` | 3/3, `legacy_v035_fixture_migrates_and_reads_back ... ok` (not a skip) |
| 4g. fe-database ignored | `cargo test -p fe-database -- --ignored` | 15/15 PASS (11 space_manager/api_token + 4 schema) — none needs a fix |
| 4h. Relay binary smoke (AC-15) | `target/debug/fe-relay.exe` from a temp CWD, temp FE_DB_PATH/FE_P2P_DIR, FE_SYNC_RELAY=disabled | `/ready` 200, data only under the temp dirs, 0 ERROR-level lines, exact PID killed + confirmed gone |
| 4i. Packaged GUI smoke (AC-9) | — | **USER-GATED** (opens a window; in-app verification stays with the user) |
| Lint | `cargo +1.99.0 clippy -p <10 touched crates> --all-targets -- -D warnings`; `cargo +1.99.0 fmt --all -- --check` | exit 0; 0 diffs |
| Gate | `cargo check -j4 -p <every touched crate> --tests` | exit 0 |

Logs: `.omc/research/r5-sweep/` (`test1.log` first run, `final1.log` green run, `harness3/4.log`,
`relay.log`, `e2e-*.log`, `fm-*.log`, `db-ignored.log`).

## Real product defects (fixed, each with a guarding test)

1. **HIGH — replicated petal settings never apply on a peer that already has the petal.**
   `SetPetalTerrain`/`SetPetalRenderDistance` replicate their `RETURN AFTER` read-back, which carries
   the author's SurrealDB `id`; `apply_static_row` MERGEd it verbatim → "Found 'petal:…' for the
   `id` field, but a specific record has been specified" on every peer (and on CREATE a peer could
   choose the local record id). Fix: inbound static rows strip `id`
   (`fe-database/src/handlers/replicated_row.rs` `apply_static_row`). Guards:
   `set_petal_terrain_replicates_and_a_peer_converges_on_it`,
   `an_editor_authored_replicated_petal_row_applies_and_a_viewer_authored_one_is_denied` (both red
   before, green after).
2. **HIGH — first-run seeding (`DbCommand::Seed`) and every space_manager write denied the verse
   owner.** `rbac::require_write_role` used an exact-scope `role` lookup; R7 removed the seeder's
   implicit petal-owner row, so the creator resolved to Viewer at the petal. Fix: resolve owner →
   scope chain → `default_access`, as `require_manage_role` does (`fe-database/src/rbac.rs`).
   Guard: `role_propagation_local_test::the_seeder_writes_no_implicit_petal_owner_row`.
3. **MED (T12 DoS) — role-statement flood is quadratic.** Every live-version lookup/delete is
   `WHERE statement_key = $k` with no index → a full scan of `role_statement_pending` per inbound
   statement; the 8,800-statement flood test ran >18 min at 100% CPU (killed by PID) before the
   fix, ~60 s after. Fix: `idx_{role_grant,role_statement_pending}_statement` (non-unique, so a
   DB that somehow holds a duplicate still starts) in `fe-database/src/schema.rs`. Guard:
   `a_hostile_flood_cannot_evict_or_starve_a_held_role_statement`.
4. **MED (info leak) — `place_asset` existence oracle.** A nonexistent asset id is
   `AssetReach::Unreferenced`, then hit the new content-type check and answered "not a GLB" while a
   foreign id answers `UNKNOWN_ASSET`. Fix: a missing row (or failed lookup) answers
   `UNKNOWN_ASSET` (`fe-api/src/upload.rs` `place_asset_core`) — and no longer reaches the DB.
   Guard: `mcp_integration::place_asset_rejects_a_foreign_scope_asset` ("same text as a foreign
   id", now also "never sent").
5. **MED (functional) — device-payload blobs locked to the uploading client.** A device-payload
   asset is never placeable, so W6-7's A5 own-read rule made it readable only by the ingesting
   client: a dashboard Viewer (another client) could list a payload but 404'd on every blob.
   Fix: `asset_reach` gives reference reach through referencing `device_payload` rows' petals,
   like node references (`fe-api/src/assets.rs`; fail-closed, capped). New flow test:
   `device_ingest_test::another_clients_viewer_reads_a_device_payloads_blobs_through_its_petal`
   (client Y reads → 200; sibling-petal viewer → 404). Doc: fe-api AGENTS.md §assets step 2.
6. **LOW — `verse_of_scope` accepted `VERSE#v1-RESOURCE#r1` as verse `v1-RESOURCE#r1`.** The shared
   parser folds an unknown trailing segment into the last id; the role model now refuses any id
   containing `#` (`fe-database/src/role_grants.rs`). Guard:
   `interim_authority_verse_ids_are_recognised_and_ulids_are_not`.

## Deferred tests that never ran — fixed (LOW each; no product change)

| Target | Root cause | Fix |
|---|---|---|
| fe-api device_ingest / terrain_mutation / render_distance / mcp device loop | `ApiHarness` never ran `init_hlc` (petal-settings writes now op-log) | once-guarded `init_hlc` in `fe-test-harness/src/api.rs` |
| fe-database observation_plane (6), device_payload (27) | petal seed missing required `node_id` | add `node_id` to seeds |
| fe-database dcl20_bypass_counter (3) | `spawn_db_thread_shared` called inside `#[tokio::test]` | spawn from an OS thread |
| fe-database observation oversized-window | asserted singleton batches; `split_group` halves by anchor HASH order, so a 2-anchor half legitimately seals together | assert the invariant (each honest member sealed once, oversized uncovered) |
| fe-api mcp_integration (5) | mock never answered the new `content_type` lookup; scoped-tool count pinned at 23 (now 28: +S6 render distance, +4 CDL-SPEC10 device tools) | mock records/answers `content_type`; pin 28 |
| fe-api petal_render_distance_api (3) | passed `"local-did"` → AC-46 node cap lowered the editor token to viewer | use the harness node DID (as terrain_mutation_test) |
| fe-canonical-log cross_spec (3) | self-issued chain lacked the SPEC-3 §4.7 kind allowlist required for structural (kind 2) appends; the wire leg then hit §2.4 step 8 "absence is refusal" | allowlist `[1..5]`; wire leg asserts the allowlisted chain is refused and the author's unrestricted chain is authorized |
| fe-format (3), fe-terrain hexon_integrity_flow (1) | in-place byte flips tripped zip's CRC before the signed-digest check ran (GPX is deflated, never verbatim) | CRC-fixing tamperer (`fe-format/src/test_support.rs`, inline in fe-terrain test) — the realistic attacker, so `DigestMismatch` is what fires |
| fe-hexon-registry registry_test (1) | expected the v1.0.0 blob from an unversioned URI; unversioned = latest by design (`index.rs`) | expect v1.2.0 |
| fe-terrain streamed (4), fractalengine `answered_bundles…` | fixtures' y=0 tiles lie above lat 85.0 / outside their own bounds | true Web Mercator bounds |
| fractalengine `a_download_naming_another_tileset…` | registry built without the host's `load_all()` | call it (asserts it indexes the victim) |
| fractalengine RP-4b pin tests (2) | encoded the pre-OF-5 "petal pin is recorded" semantics | OF-5: petal pin decides resolution, never recorded |
| fe-sync relay_assisted (1) | asserted a relay path after the fetch; production endpoints on one host may hole-punch direct and retire it (relay-only is proven by the IP-less-dialer sibling) | assert a live path on the relay-only-dialed connection |
| fe-sim lib (6) | hierarchy created before any replica opened and never published → RP-4 `Parent` deferred every reading; then a 64-slot DB command channel dropped ~20% of a 320-row burst | production setup order + publish fractal/petal/anchors; harness DB command channel = production `CHANNEL_BUFFER` (other channels stay 64 so the DEC-C13 deadlock guard keeps its sensitivity) |
| harness role_propagation | granter's DB lacked the fractal (author rows never echo back; `AssignRole` precondition) | `scaffold::create_local_fractal/petal` then publish |
| harness distributed_query | bob's/carol's anchors never published; alice lacked the scaffold locally | publish every anchor; local hierarchy on alice |
| harness two_peer_tileset_stream (W5s watch item 1) | no manifest published → no owner/`default_access` → publisher's blob gate refused bob ("requester holds no role") | publish alice's manifest |
| scripts/bi-egress-verify.ps1 | PS 5.1: a 1-row result unrolls to a `[pscustomobject]` with no `.Count` | `@(...)` at the two ENU call sites |
| scripts/relay-health-verify.ps1 | non-BOM UTF-8 em-dash → cp1252 parse error (never ran on Windows PowerShell); fixed 1.5 s grace < `HOME_RELAY_WAIT` for an unreachable Custom relay | ASCII-only; bounded poll (20 s) for the health line |

## Dispositions (no fix needed — with evidence)

- **Inbound drop-and-count when the DB command channel is full** — NOT NEW: the drop path dates
  from A2/A4 (`fe-sync/src/sync_thread.rs` "DB command channel full — dropping inbound replicated
  row", `git log -L` → `3f957c4`), production runs it at `CHANNEL_BUFFER = 256`, and dropped rows
  are re-fed by the open-time reconciliation replay (`sync_thread.rs` "Reconciliation: replayed
  replica entries through the inbound apply path"). Only the harness was undersized (fixed above).
- **OF-10a..c** (third-order MEDs on R1's redesign) — queued by the orchestrator for the post-sweep
  R6 slice (`conductor/accountability-log.md` OF-10); outside this sweep's scope, untouched here.
- **Packaged GUI smoke (AC-9) / GUI in-app verification** — USER-GATED by directive.

## Notes

- Commits: `66c76f1` (step-1 fixes) + this commit (harness/e2e fixes + this file). Never pushed.
- Trailer is the executing model ("Claude Opus 5.5 (1M context)"), as W5s did.
- `scripts/{hexon-install,hexon-serve,relay-health}-verify.log` are the scripts' committed
  evidence logs, refreshed by this run.
