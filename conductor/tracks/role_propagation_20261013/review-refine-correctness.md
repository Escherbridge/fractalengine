---
type: review
track: role_propagation_20261013
phase: refine R3 (adversarial correctness, whole campaign diff)
base: 823f9e6
reviewer: Claude (Fable 5 orchestration, fresh context; refute-not-confirm)
date: 2026-10-25
verdict: FIXED
---

# Refine R3: correctness review-and-fix over the campaign diff

Scope: `4d3a1a6..823f9e6`, covering W5 role propagation, W5s, W6 decisions, and R1/R2. This
pass looked for correctness bugs, not security holes (R1 and R2 owned security). Gate:
`cargo check -j4 -p fe-policy -p fe-runtime -p fe-sync -p fe-ui -p fractalengine-test-harness --tests`
exited 0, with no warnings in touched files. Tests were written but not run (FF rhythm; the
R5 sweep runs them). The PowerShell e2e edit is exercised by R5's `bi-egress-verify.ps1`.

**Verdict: FIXED.** Seven real findings, all fixed. There is no CRITICAL. The highest is
R3-1 (MEDIUM): the sync-plane re-read on empowerment silently missed authority gained at a
second scope. Scenario 18 (R3-2) would have failed every R5 harness run. Every other
listed focus was checked and holds; the evidence is below.

## Findings (all fixed)

| # | Sev | Finding | Fix | Flow test (not run) |
|---|-----|---------|-----|---------------------|
| R3-1 | MED | `EffectiveRoles::diff` (RP-1 open note) compared each DID's **best role across scopes**. The sync plane seeds its §2.5 re-read queue from `diff().risen` (`sharding.rs::rematerialize`). A Manager at fractal A newly made Manager at fractal B came out "Equal", so the statements and shards it held at B were never re-read. A freshly pinned owner was never queued either, which made the `learn_owner` doc ("the §2.5 re-read picks up [the owner] as a risen DID") false. The DB plane's `changed_between` was already per-cell and correct. | `fe-policy/src/role_grants.rs::diff` is now per `(scope, did)` cell over pins, blocks, and Manager grants. An owner change counts the new owner as risen and the old one as fallen. A DID may appear in both lists. Documented in fe-policy `AGENTS.md` §role-grants. | `diff_reports_a_rise_at_one_scope_even_when_the_best_role_is_unchanged` (fe-policy). `a_held_role_statement_is_reread_from_the_replica_when_its_grantor_is_empowered` (fe-sync) now asserts the owner is queued at pin time. |
| R3-2 | MED | Harness scenario 18 leg 1 called `scaffold::publish_and_await(&bob, &non_owner, …)`, and `non_owner[0]` is bob. The sync thread never echoes an own-author row to its DB: the pump skips `InsertLocal` (`replicator.rs:1128`), and `handle_inbound_row_change` returns early for own rows (`sync_thread.rs:1331`). Bob's wait therefore always ran out at `SCAFFOLD_TIMEOUT`, failing the scenario on every run. | The receivers are now `[carol, dave, erin, frank]`. `publish_and_await` now refuses an author listed as its own receiver up front, with a clear error instead of a 45 s timeout. | Scenario 18 itself (R5 harness). |
| R3-3 | MED | The `bi-egress-verify.ps1` ENU section ran its DuckDB queries with `$spatialPrelude = "INSTALL spatial; LOAD spatial;"`. Each query is a fresh `duckdb` process, so httpfs for the http share URLs depended on autoload. With spatial loaded, DuckDB's GeoParquet conversion hands `position` back as GEOMETRY, which `ST_GeomFromWKB` does not take. The `docs/bi-egress.md` example had the same problem. | The prelude is now `$prelude INSTALL spatial; LOAD spatial; SET enable_geoparquet_conversion=false;`, so the check reads the raw WKB bytes the codec wrote. The doc example sets the same flag and explains why. | The R5 e2e (`bi-egress-verify.ps1`). |
| R3-4 | LOW | Relay test `wait_healthy` used a blocking `TcpStream::connect` and `read_to_end` with no socket timeouts. The runtime's port forwarder accepts a connection before the relay listens, so one silent socket could block past the 60 s deadline, which is only checked between attempts. | `connect_timeout` plus read and write timeouts of 2 s each. | The relay-assisted tests themselves (R5). |
| R3-5 | LOW | GPX-HIDE-CASCADE `owning_track_id` linearly scanned `promoted_stamp_alias` for every resolved node that is not a marker. That is O(promoted stamps) per resolve, inside the per-frame sidebar eye and apply pass, which AGENTS anti-pattern 7 forbids on a hot path. `register_promoted_stamp_alias` also bumped `epoch` on every properties reload, even when nothing changed. | A reverse index, `promoted_stamp_marker` (row id → marker), is kept in lockstep with the alias map, so the lookup is O(1). Re-registering an unchanged pair is a no-op (no bump, like `clear_solo`). Re-pointing a marker retires the old row. | `a_promoted_stamps_row_id_follows_its_track_through_the_alias_index` (fe-ui visibility). |
| R3-6 | LOW | The flow test could not fail: `the_ground_stamp_tier_skips_hidden_stamps_and_other_petals` had a "hidden track (W6-5 cascade)" case that built the cascade in the test's own predicate closure, not production's `pickable`. Breaking the resolver's cascade would have left it green. | The case now drives `crate::visibility::pickable(id, &nav, &visibility)`, the predicate `context_pick.rs:449` passes, with track-a's own override set to Hide. | The rewritten case. |
| R3-7 | LOW | Two docs lied after OF-7c. `DistributedQueryMeta::unverified_answerers` (fe-runtime) and `AnswererVerdict::Unverified` (fe-sync) still said "their rows were kept", and plan §2.6 said the same. OF-7c excludes those rows (fail closed). | All three corrected. The plan line is marked superseded by OF-7c. | n/a (docs) |

The callout create-echo race had one gap in coverage, not in behavior. `bind_created_callout`
already returns early for an unknown or empty `callout:` id. The echo test now pins that:
`callout:c99` and `callout:` write nothing and record nothing.

## Foci checked and holding (dispositions with evidence)

- **No W5/W6 hunk silently reverted by R1/R2: NOT-REAL.** Every file R1 or R2 touched
  (fe-sync `docs_engine/sharding/sync_thread/distributed_query/AGENTS.md`; fe-database
  `lib/reconcile/role_grants/replicated_row/elevated_query/AGENTS.md`; fe-api
  `upload.rs/AGENTS.md`) was diffed line by line. Every removed line belongs to a named fix:
  the OF-8 gossip entry point and persisted overflow (`note_advertised_root`, `bind_learned`,
  `MAX_LEARNED_VERSES`), the OF-9 `note_petal_row` signature, the R1-2 eviction order, and
  R2's cap helper refactor (`statement_boundary_refusal`) and `publish_paced`. W6-8's fe-sync
  AGENTS section (§relay self-hosting) survives intact. W6-2's 32-statement cap is preserved
  verbatim inside `statement_boundary_refusal`.
- **f32→f64 egress: holds.** No narrowing cast remains on a BI egress position:
  `export.rs:574-627`, `entity_store_bridge.rs:51-58`, `codec.rs` WKB, and `provider.rs`
  `Float64Array` for `pos_*`. The remaining `as f32` sites (`rest.rs`, `format.rs`, `gpx.rs`,
  `mcp/tools.rs`) are REST/MCP node DTOs and ingress (A8), not BI egress. CSV writes
  `f64::to_string` (export.rs:666-668). Ingress widening (`EntityStore::widen_position`, `as f64`)
  matches the DB's own `position[i] as f64` binds (`crud.rs:431-433`), so analytics and export
  agree bit for bit. The ENU inverse reads the file's own `geo` metadata (`lonlat_from_meta`:
  `crs` absent means lon/lat, identity), so lon/lat WKB output is byte-identical to before.
- **Visibility resolver: holds.** No recursion (`lattice_hidden_reason` → `own_override` +
  `ancestor_or_group_reason`, one level). Solo is step 1, before the per-node override. The epoch is
  bumped on a new alias (now only on a real change, R3-5). A per-stamp override wins: `own_override`
  runs before step 2.5, under the marker id and under the alias.
- **Callout create-echo race: holds.** An unknown id → `take_pending` `None` → early return, no
  panic (pinned above). A partial bag fails `parse_callout_bag`'s strictest reader and renders
  nothing. On the creator side, the echo records geometry before the four `SetNodeProperty`
  writes, so a partial reload never erases it.
- **Relay test: holds apart from R3-4.** Drop runs on unwind: test binaries always build
  `panic=unwind`, whatever the profile says. Port parsing: `<rt> port <name> 3340/tcp` prints
  `127.0.0.1:<port>` on both docker and podman, and `rsplit(':')` also covers the `3340/tcp -> …`
  form. `/healthz` exists in iroh-relay 1.3.0 (`server.rs:740`, 200 + JSON), and
  `IROH_RELAY_ACCESS_TOKEN` overrides `access.shared_token` (`main.rs:234`) as the token test assumes.
- **RP-2 note, `reconcile.rs` as a direct role-cache writer: NOT-REAL (harmless).**
  `reconcile` (`fe-database/src/lib.rs:554`) runs before `role_grants::startup` (`:589`), whose
  `repartition_all` → `write_cache` (`role_grants.rs:560`) diff-writes the cache from the statement
  store and deletes every row outside the projection. The only rewrite (`'local-node'` →
  local DID) is therefore superseded at the same startup. Pre-manifest verses never have a
  projection, and for those the rewrite is the intended legacy fix.
- **Settlement stop condition: holds.** An empty holder set means `holders_done` is vacuously true, so the query
  settles once the hosts are done. When every host is deferred, `settles_shard` excludes deferred answers, so an aggregate never
  early-settles on them, and the shard lands in `incomplete_shards` (covered as well when its rows contributed, which
  is the documented "data contributed" meaning of `covered_shards`; tried and reverted, see below).
  Empty target shards early-settle an aggregate, but `merge_aggregates` reads only per-target-shard
  rows, so the outcome is the same either way (the condition predates the campaign, `4d3a1a6:953`).
- **RP-7 filter vs RP-6 holders: holds.** The responder answers iff
  `local ∈ hosts ∪ admitted holders` from its own view (`responder_covers_shard`), the same set
  the requester's plan awaits (`expected_hosts ∪ expected_holders`). Asymmetric views degrade
  honestly: a self-unadmitted holder stays silent and is reported in `missing_holders`, and
  an answerer the requester cannot judge is excluded and reported in `unverified_answerers`.
- **Harness scenario 18 wiring: fixed (R3-2).** It is registered in `main.rs:115-118` and `scenarios/mod.rs`,
  18 entries in total.
- **OF-10a..c (third-order MEDs on R1): covered elsewhere.** They are queued to post-sweep R6 by
  the orchestrator (`conductor/accountability-log.md` §OF-10), and the prompts are fixed for this run.
  Not re-litigated here.
- **RP-4b pin grammar vs UI-authored pins: NOT-REAL.** `tileset_to_terrain_json` keys the pin by
  `hexon_id`, which it also puts in `tileset_hexon_uris` (`terrain_map/mod.rs:185,197`), so
  `validate_publisher_pins`' binding check passes. Ed25519 did:keys are always 56 chars (the 0xed01
  prefix fixes the base58 length at 47 digits), so the grammar check cannot reject a real sealed publisher.

**Tried and reverted (recorded so R4/R5 do not redo it):** making `incomplete_shards`
exclusive of `covered_shards`. `DistributedQueryMeta::covered_shards` is documented as "data
contributed" and `incomplete_shards` as "may be missing rows". A deferred formal host's
non-empty rows (OF-7b) truthfully satisfy both, so the overlap is intended, not a contradiction.
