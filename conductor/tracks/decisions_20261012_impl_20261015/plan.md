---
type: plan
track: decisions_20261012_impl_20261015
title: "Implementation Plan: the 2026-10-12 decisions (campaign W6)"
created: 2026-10-15
resource: ./spec.md
computed_at_commit: 389b6c7
security_critical: true
depends_on: [role_propagation_20261013]
---

# Implementation plan: decisions_20261012_impl_20261015 (W6)

> **Citations grounded @`389b6c7`** (2026-10-15, while W5 slice RP-1 was in flight in the working
> tree). W6 runs **after** W5 lands. W5 edits `fe-database/src/lib.rs`, `fe-runtime/src/messages.rs`,
> `fe-test-harness/src/peer.rs`, `fe-database/src/handlers/{elevated_query,device_payload,replicated_row}.rs`,
> `fe-ui/src/plugin.rs` and the fe-database/fe-sync AGENTS docs, so slice **W6-0 re-grounds every
> cite below before any code slice runs**. Symbol names are stable; line numbers are not.
>
> **Standing rules** (RUNBOOK §3 working rules, `conductor/RUNBOOK.md:286-298`; critic AC-48/AC-52):
> - Slices run strictly in sequence. Builds never overlap. `TaskStop` the previous workflow id
>   before a relaunch, and check that no cargo is running first (AC-53).
> - **The gate, every code slice:**
>   - `cargo check -j4 --workspace --tests`;
>   - `cargo +1.99.0 clippy -j4 --workspace --all-targets --keep-going -- -D warnings`;
>   - `cargo +1.99.0 fmt --all -- --check`;
>   - the feature matrix: `check` plus `clippy --all-targets -D warnings` for `fe-query --features parquet`,
>     `fe-format --no-default-features`, `fe-terrain --features fetch,render` and `fe-hexon --all-features`.
>
>   Run every step in the foreground. Never return while a cargo process you started is still
>   alive. On exit `0xc0000142` with no other builder running, retry that one step once. No
>   generic `overrides` bypass. If `gate_green` is false: no commit, halt.
> - **AC-52.** A review slice that returns `BLOCKED`, or that leaves a CRITICAL or HIGH finding
>   unfixed, halts the run exactly as a red gate does.
> - **No tests run until the campaign final sweep.** Each executor writes its tests now, names
>   them exactly as listed here, and reports them for the manifest.
> - Reviewers fix their own findings. Prune `target/debug/incremental` and the PDBs at every
>   safe point.
> - **Never revert a W5 hunk.** In a shared file (§1 table), edit only the named regions. If a
>   W5 change blocks a W6 edit, halt and report; do not work around it.

---

## 0. Goal and the item → slice map

**Goal.** Build the 2026-10-12 contracts that are still open (spec §Decisions), correct the
OSS-1.3 ledger contradiction, and close IROH-1X's last acceptance criterion against a local
iroh-relay 1.3 container. None of this may weaken W5's authority model.

| Item | Ledger row | Slice | Size |
|---|---|---|---|
| Re-ground + W5 composition check | — | W6-0 | S |
| FEAPI-BUILDER-CRS | `doneness-ledger.md:220` | W6-1 | S |
| FEAPI-ELEV 32-statement cap | `:187` | W6-2 | S |
| EGR-F32 | `:219` | W6-3 | M |
| EGR-WKB | `:218` | W6-4 | S |
| GPX-HIDE-CASCADE | `:240` | W6-5 | M |
| HS-P6-CALLOUT | `:216` | W6-6 | M |
| TOKEN-CLIENT-CLAIM (core) | `:239` | W6-7a | M |
| TOKEN-CLIENT-CLAIM (GUI) | `:239` | W6-7b | S |
| IROH-1X relay-assisted AC | `:146` | W6-8 | M |
| SECURITY review-and-fix | — | W6-9 | M |
| CORRECTNESS review-and-fix | — | W6-10 | M |
| Docs, ledger, manifest (incl. OSS-1.3 correction) | `:29,:71,:114,:119,:157` | W6-11 | S |

**Why this order.**
- Pure, isolated contracts go first (CRS, cap), because they are cheap to gate and each
  shrinks the review surface.
- The two egress slices come next, and they are sequential because they share
  `codec.rs`/`export.rs`. F32 widens the type that the WKB frame then reorders.
- Then the fe-ui pair. W6-5 and W6-6 share `db_results/properties.rs`.
- The auth slice comes after those, so it lands on the most settled tree and runs right before
  the security review that must attack it.
- The relay slice comes last among the code slices, because it adds the only new CI step and
  the only external runtime dependency.

---

## 1. W5 interaction (role propagation runs first)

### 1.1 The composition rule (TOKEN-CLIENT-CLAIM × AC-46)

W5 RP-4 step 8 (`role_propagation_20261013/plan.md:945-957`) defines the effective API role for
replicated writes as `min(token max_role, role_at(local_did, scope))`. One helper applies it,
shared by `lifecycle_auth_context` (`fe-database/src/lib.rs:244-266` @483b538) and every
replicated-write door. `local_did` is the **node**. The node's `sub` claim and the node's
replicated authorship are the same identity (`fe-database/src/lib.rs:543` derives
`local_did` from the node keypair, and `fe-identity/src/api_token.rs:84` sets
`sub: keypair.to_did_key()`).

W6 rules. W6-7a implements them; W6-9 attacks them.

1. **`client` is never an authorization subject.**
   - `fe-api/src/endpoint.rs:49-53` (`caller_auth`) keeps `did: claims.sub.clone()`.
   - `fe-api/src/auth.rs:58-64` (`api_token_context`) keeps `subject: claims.sub.clone()`.
   - `fe-database/src/lib.rs:225-234` (`caller_auth_to_context`) and W5's cap helper are not
     edited by W6.
   - Test: `a_client_claim_naming_a_manager_did_gains_no_role` (threat TC-1).
2. **`client` never changes `max_role`, and never bypasses the cap.** Mint keeps its existing
   `requested_level > caller_role` refusal (`fe-database/src/handlers/api_token.rs:63-71`).
   The client is only a field written into claims the door has already authorized.
3. **Check order on an upload, after W5 + W6:**
   1. `verify_api_token`, including the new `client` grammar check.
   2. The `jti` revocation cache (`fe-api/src/auth.rs:34-39`).
   3. `require_role_and_scope` at the anchor petal.
   4. W5's AC-46 cap, if the door is a replicated write.
   5. Reach and quota, keyed on `owner_key(claims) = client.unwrap_or(sub)`.
4. **Verify, do not assume, that the upload doors are capped.** Asset rows replicate through
   device-payload emission (`fe-database/src/handlers/device_payload.rs:395-406`), and W5's
   named door list (`InsertReadings`, `InsertDevicePayloads`, `RegisterDeviceFormat`,
   `SetPetalTerrain`, `SetPetalRenderDistance`) does not name `CreateAsset`.
   - W6-0 records whether W5's final helper covers `CreateAsset` and the
     `InsertDevicePayloads` asset rows.
   - If it does not, W6-9 adds `CreateAsset` to that helper's call sites. W6 calls W5's
     helper; it never re-implements it (threat TC-7).

### 1.2 Files both W5 and W6 edit (sequential — W6 only after W5's final commit)

| File | W5 slices | W6 region |
|---|---|---|
| `fe-database/src/lib.rs` | RP-2, RP-3, RP-4, RP-6 | `DbCommand::MintApiToken` arm (`:982-+` @483b538) — W6-7a |
| `fe-runtime/src/messages.rs` | RP-3 (`ReplicatedRowOutcome::Deferred`), RP-6 | `MintApiToken` (`:569-574`), `ApiTokenMinted` (`:1123-1130`), `ApiTokenInfo` (`:1415-1425`) — W6-7a |
| `fe-test-harness/src/peer.rs` | RP-3, RP-6, RP-8 | harness `MintApiToken` arm (`:585-640`) — W6-7a |
| `fe-database/src/handlers/elevated_query.rs` | RP-2 (role tables pinned out of `ELEVATED_WRITABLE_TABLES`) | new `MAX_ELEVATED_STATEMENTS` + check in `elevated_query_reply` (`:48`) — W6-2 |
| `fe-database/src/handlers/device_payload.rs` | RP-0 (`15ae606`) | outbound asset row strip at `:395-406` — W6-7a |
| `fe-database/tests/device_payload_test.rs` | RP-0, RP-4 | one new test — W6-7a |
| `fe-ui/src/plugin.rs` | RP-4b (`tileset_to_terrain_json`) | `InspectorFormState` gains `api_token_client_buf` (`:252-255`, `:298-301`) — W6-7b |
| `fe-database/src/AGENTS.md` | RP-2, RP-4, RP-10 | §handlers elevated paragraph — W6-2 |
| `fe-sync/src/AGENTS.md` | RP-5, RP-7, RP-10 | §relay-health (`:570-`) — W6-8 |
| `conductor/final-sweep-manifest.md` | RP-8 (harness 17→18) | W6 section append — W6-11 |

W6 adds **no** harness scenario, so the harness count stays at W5's 18.

---

## 2. Current state per item (verified @`389b6c7`)

### 2.1 TOKEN-CLIENT-CLAIM
- **Claims.** `ApiClaims { sub, token_type, scope, max_role, iat, exp, jti }`
  (`fe-identity/src/api_token.rs:17-33`), a plain derive with no `deny_unknown_fields`. So an
  added `#[serde(default, skip_serializing_if = "Option::is_none")] client: Option<String>`
  leaves legacy tokens decoding and re-encoding unchanged.
- **Mint.**
  - `mint_api_token` (`:40-54`) and `mint_api_token_at` (`:65-95`); `sub` is set at `:84`.
  - Verify is `verify_api_token` (`:100-112`), with an EdDSA-pinned `Validation`, an empty
    `required_spec_claims` and `validate_exp` (`:104-106`).
  - No signature has a client parameter.
- **DB door.** `mint_api_token_handler` (`fe-database/src/handlers/api_token.rs:32-112`):
  - role and scope validation `:41-52`;
  - Manager+ at scope `:55-62`;
  - no escalation `:63-71`;
  - per-sub cap `:73-81` (`MAX_ACTIVE_TOKENS_PER_SUB = 50`, `fe-database/src/api_token_store.rs:12`);
  - mint `:86`;
  - bookkeeping row `:98-107`.
- **Bookkeeping table.** `api_token` (`api_token_store.rs:15-29`) has no `client` column.
  `store_api_token` strips a null `label` before insert (`:50-55`). That is the
  null-into-`option<string>` lesson from W4's correctness review, `d098b65`.
- **Command and result.**
  - `DbCommand::MintApiToken { scope, max_role, ttl_hours, label }`
    (`fe-runtime/src/messages.rs:569-574`).
  - `DbResult::ApiTokenMinted { token, jti, scope, max_role, expires_at, label }` (`:1123-1130`).
  - `ApiTokenInfo` (`:1415-1425`).
  - Dispatch is at `fe-database/src/lib.rs:873-882`.
- **Mint callers.**
  - GUI: `fe-ui/src/panels/inspector.rs:712-718` and `fe-ui/src/dialogs/entity_settings.rs:572-577`.
  - Harness DB loop: `fe-test-harness/src/peer.rs:585-640`, which re-implements the mint by
    calling `fe_identity::api_token::mint_api_token` directly.
  - Harness scenario `fe-test-harness/src/scenarios/api_token_flow.rs:21,123,198,218,255,303`.
  - CLI `fe-identity/examples/mint_api_token.rs:47`.
  - Test helper `ApiHarness::mint_token` (`fe-test-harness/src/api.rs:273-282`).
  - Integration tests `fe-test-harness/tests/integration.rs:236-375`.
- **There is no REST or MCP mint route.**
- **Ownership choke point.** `UploadOwner::new` (`fe-api/src/upload.rs:154-160`) sets
  `owner_did: claims.sub.clone()`. It is called from every upload door:
  - REST `fe-api/src/upload.rs:640`;
  - device ingest `fe-api/src/device.rs:275`;
  - MCP `fe-api/src/mcp/tools.rs:378` and `:788`.
- **Quota.** `check_upload_quota` (`fe-api/src/upload.rs:356-380`) runs
  `SELECT math::sum(size_bytes) … WHERE owner_did = $owner` (`:364`). It is called at `:403`
  and `:447`. The default is 2 GiB (`:40-43`).
- **Reach.**
  - `asset_reach` (`fe-api/src/assets.rs:60-126`) checks anchor first (`:79-87`), then
    references (`:89-112`).
  - For an unplaced owned asset it returns `Foreign` (`:115-125`).
  - The comment at `:115-117` says "`owner_did` alone grants nothing — every token this node
    mints carries the node DID as `sub`". The doc at `fe-api/AGENTS.md:355-359` foresaw "a
    future per-caller subject claim".
- **Outbound leak.**
  - Device-payload emission replicates `SELECT *` asset rows (`fe-database/src/handlers/device_payload.rs:659-692`
    builds `row_json`; `:395-406` emits it). `owner_did` therefore goes onto the wire.
  - Inbound, peers strip it (`fe-database/src/handlers/replicated_row.rs:771-777`).
- **`ApiClaims` struct literals that a new field breaks:**
  - `fe-api/src/assets.rs:607`, `fe-api/src/auth.rs:122`, `fe-api/src/endpoint.rs:541`,
    `fe-api/src/rest.rs:2432`;
  - `fe-api/tests/analytics_scope_test.rs:27`, `fe-api/tests/distributed_query_test.rs:491`,
    `fe-api/tests/export_share_test.rs:48`, `fe-api/tests/gis_test.rs:41`,
    `fe-api/tests/iot_ingest_test.rs:42`, `fe-api/tests/mcp_integration.rs:715`;
  - `fe-identity/src/api_token.rs:83`.
- **did:key parse.** `public_key_from_did_key` is at `fe-identity/src/did_key.rs:39`.
- **Existing ownership tests** in `fe-api/tests/asset_ownership_test.rs`:
  - `unplaced_upload_is_readable_from_its_anchor_petal_only` (`:148`);
  - `owned_asset_placed_elsewhere_stays_readable_from_its_anchor_petal` (`:198`);
  - `legacy_unowned_asset_keeps_reference_only_reach` (`:220`);
  - `upload_past_the_owner_quota_is_413_and_writes_nothing` (`:252`);
  - `refused_uploads_write_nothing` (`:287`).

### 2.2 GPX-HIDE-CASCADE
- **One resolver.**
  - `effective_visibility` (`fe-ui/src/visibility/resolve.rs:56-62`) delegates to
    `node_hidden_reason` (`:67-113`).
  - Precedence: solo (`:73-79`) > own override (`:82-91`) > ancestor chain (`:94-99`) >
    groups (`:102-111`).
  - `HiddenReason` is at `:38-51`. `toggle_node_override` (`:200-207`) and `node_eye_tooltip`
    (`:249`) match it exhaustively.
- **Consumers.**
  - `pickable` (`fe-ui/src/visibility/mod.rs:374-376`) is THE picker rule.
  - `compute_sync_decision` (`:544-586`) is the pure apply decision. It is called by
    `sync_node_visibility` (`:610-675`), which evaluates each entity with its own petal
    ancestry.
  - Sidebar: `fe-ui/src/panels/sidebar.rs:564-567`.
- **Stamps.**
  - Every stamp entity carries `SpawnedNodeMarker { node_id: "{track}::stamp::{i}" }`
    (`fe-ui/src/verse_manager/path_asset_materialize.rs:220-222`, `:567`), plus
    `PathAssetInstance { source_track_id }` (`fe-ui/src/verse_manager/spawn.rs:134-137`,
    `:146-179`).
  - `parse_stamp_marker_id` (`path_asset_materialize.rs:226-`) recovers the track.
  - Promoted stamps also have a node row whose `properties` hold `path_id` and
    `owning_path_id` set to the same track id (`fe-database/src/handlers/crud.rs:1589-1590`).
    `hydrate_promoted_stamp` (`fe-ui/src/actions/asset.rs:376-393`) maps
    `(track, index) → promoted node id`.
- **Today's gap.** `fe-ui/src/AGENTS.md:1028-1030` says "hiding a track does not hide its
  stamps (no inheritance invented — open question)". Stamp overrides are session-only
  (`fe-ui/src/visibility/mod.rs:353-358`).
- **Tests to extend:**
  - `pickable_refuses_every_effectively_hidden_cause` (`fe-ui/src/visibility/mod.rs:1166`),
    which is table-driven;
  - `the_ground_stamp_tier_skips_hidden_stamps_and_other_petals`
    (`fe-ui/src/node_manager/context_pick.rs:779`), also table-driven.

### 2.3 HS-P6-CALLOUT
- **Measure tool.**
  - `MeasureState { mode, anchors: Vec<[f32;3]>, closed }` (`fe-ui/src/node_manager/measure.rs:54-59`).
  - Events `Place/Undo/Close/Clear/SetMode` (`:75-81`).
  - `readout(state, world_scale)` (`:145`) and `measure_world_scale` (`:180`).
  - The state is cleared on tool exit or petal switch (`handle_measure_interaction`, `:224-243`).
  - Drawn by `draw_measure_overlay` (`:260`).
  - Options UI: `render_measure_options` (`fe-ui/src/panels/tool_options.rs:424-490`).
  - Nothing persists today (`fe-ui/src/node_manager/AGENTS.md:128-129`).
- **node_kind.**
  - It is free-form inside the `FLEXIBLE` `node.properties` (`fe-database/src/schema.rs:219`).
  - There is no enum and no ASSERT.
  - The inbound `node` row is applied generically (`replicated_row.rs:1034-1053`).
- **Template (earthwork).**
  - `EARTHWORK_NODE_KIND` (`fe-ui/src/actions/terrain_proposal.rs:414`).
  - Correlation prefix `"earthwork:"` (`:416`).
  - `earthwork_node_properties` (`:503-512`) and `create_region_node` (`:519-541`).
  - `earthwork_region_id_from_correlation` (`:490-494`) and `hydrate_earthwork_region` (`:547-`).
  - Created-echo bind: `handle_node_created` (`fe-ui/src/verse_manager/db_results/nodes.rs:52-110`).
  - Reload: `handle_node_properties_loaded` (`fe-ui/src/verse_manager/db_results/properties.rs:35-64`).
  - DB arms `CreateNode` (`fe-database/src/lib.rs:568`) and `SetNodeProperty` (`:943`).
  - Delete: `handle_node_deleted` (`fe-ui/src/verse_manager/db_results/nodes.rs:189`).
- **The REST DTO doc** lists the kinds (`fe-api/src/endpoint.rs:150`, "`stamp` /
  `earthwork_region`").
- **Track tasks open:** `conductor/tracks/hexon_scale_orchestration_20260712/plan.md:120` and `:124`.

### 2.4 EGR-WKB and EGR-F32
- **DB.** `node.position` is `geometry<point>` `[x, z]`, plus `elevation: float` (y). Both are f64
  (`fe-database/src/schema.rs:203-220`).
- **Downcasts to f32 on the egress path:**
  - `EntitySnapshot.position: [f32;3]` and `ReadingSnapshot.anchor_position: Option<[f32;3]>`
    (`fe-entity-store/src/lib.rs:14-26`, `:36-48`);
  - `row_to_snapshot` (`fe-api/src/export.rs:563-602`), which casts at `:575` and `:577`;
  - `reading_row_to_snapshot` (`:608-630`), which casts at `:616` and `:618`;
  - the hot-cache bridge (`fe-api/src/entity_store_bridge.rs:44-70`);
  - the analytics provider's `pos_x/pos_y/pos_z` `Float32` (`fe-query/src/columnar/provider.rs:59-61`, `:97-99`, `:115-117`).
- **WKB.** `point_z_to_wkb([f32;3])` (`fe-query/src/columnar/geoparquet/codec.rs:251-260`)
  writes the components verbatim, already as doubles. Its inverse is `wkb_to_point_z`
  (`:262-288`).
- **Frame flag already present.**
  - `GeoParquetMeta.lonlat_crs84` (`fe-query/src/columnar/geoparquet/mod.rs:34-38`) is set in
    `parquet_response` from `out.coords == Coords::LatLon` (`fe-api/src/export.rs:357-362`).
  - The `geo` JSON omits `crs` for lon/lat and keeps `crs: null` for local.
  - So the writer and the reader already know the frame. The ENU change needs **no new flag**.
- **Coords.** `Coords { Local, LatLon }` (`fe-api/src/export.rs:34-37`) and `parse_coords`
  (`:67-76`).
- **CSV headers.** `x_m,y_m,z_m` and `lon,lat,ele_m` (`:641`, `:643`).
- **Tests.**
  - `round_trip_preserves_rows_and_geo_metadata` (`fe-query/src/columnar/geoparquet/mod.rs:258`);
  - `bytes_writer_matches_file_writer_round_trip` (`:321`);
  - `crs_key_absent_only_for_lonlat_output` (`:413`);
  - the `1e-2` tolerances in `export_csv_local_vs_latlon_landmine`
    (`fe-api/tests/export_share_test.rs:339`, `:400`) and
    `export_readings_latlon_converts_anchor_position` (`:528`, `:564`).
  - **No test asserts axis semantics anywhere.**
- **e2e.** `scripts/bi-egress-verify.ps1:377-380` only counts non-null geometry. There is no
  `ST_X/ST_Y/ST_Z` check, although the ledger row assumes one.
- **Doc.** The precision note is at `fe-api/AGENTS.md:920-921`. The known limitation is at
  `conductor/tracks/_archive/bi_verification_20261009/spec.md:62-69`.
- **Ingress stays f32** (assumption A8): `CreateNode.position: [f32;3]`
  (`fe-runtime/src/messages.rs:431`) and `UpdateNodeTransform` (`:495`).

### 2.5 FEAPI-BUILDER-CRS
- **Three literals override `..Default::default()`'s `Some("EPSG:4326")` with `crs: None`:**
  - `fe-terrain/src/tiles/builder.rs:273` in `package_hexon` (`:222`);
  - `:379` (`chunk_meta`) in `package_tiles_chunked` (`:309`);
  - `:444` (`outer_meta`) in the same function.
- **TilesetMeta.**
  - The field is `#[serde(default = "default_crs", skip_serializing_if = "Option::is_none")] crs`
    (`fe-format/src/manifest.rs:72-75`).
  - `default_crs` is at `:81-84`, `crs_or_default` at `:86-91`, and the hand-written
    `Default` at `:99-119`.
- **Readers.** `crs_or_default()`'s only caller is `resolve_petal_crs`
  (`fe-api/src/crs.rs:48-85`, `:74`).
- **Builder test.** The only one is `package_tiles_chunked_produces_a_container_that_verifies_sealed`
  (`builder.rs:483`), and it does not assert crs.
- **The smart part.** A `None` serializes as an *absent* key, and an absent key deserializes
  back to `Some("EPSG:4326")`. So a struct-level assertion passes before AND after the fix.
  The test must read the archive's raw meta JSON.

### 2.6 FEAPI-ELEV 32-statement cap
- **Route.** `execute_elevated_query` (`fe-api/src/rest.rs:1229`). A validation failure
  returns `Json(ApiResponse::error(e))`, which is HTTP 200 with `ok:false` (`:1260-1266`).
  Every elevated refusal uses that envelope.
- **Guard.** `validate_elevated_sql` (`fe-api/src/query_guard.rs:397-470`):
  - it rejects comments (`:408`);
  - it splits (`:452`) with `split_elevated_statements` (`:476-517`), which tracks strings,
    escaped identifiers and bracket depth, and drops empty statements;
  - it suffixes ` TIMEOUT 5s` on DML-led statements (`:453-465`; `STATEMENT_TIMEOUT_SECS`
    `:104`; `ELEVATED_DML_VERBS` `:371`).
- **DB.** `elevated_query_reply` (`fe-database/src/handlers/elevated_query.rs:48`) joins
  statements with `";\n"` (`:76`, `:132`). The request struct is at `:31-37`. Dispatch is at
  `fe-database/src/lib.rs:1419`.
- **No statement-count cap exists anywhere.**
- **Tests** in `fe-api/tests/elevated_query_test.rs`:
  - `elevated_query_is_refused_unless_the_operator_opted_in` (`:111`);
  - `elevated_query_requires_owner_even_when_enabled` (`:136`);
  - `owner_write_is_op_logged_replicated_and_broadcast` (`:166`);
  - `failing_elevated_script_rolls_back_writes_and_log` (`:265`);
  - `long_scan_hits_the_db_side_timeout` (`:303`);
  - `elevated_works_without_db_reader` (`:358`);
  - `owner_cannot_rewrite_privilege_rows_through_elevated_sql` (`:386`).
- **Doc.** `fe-api/AGENTS.md:708-722` (TIMEOUT) and `:750-761` (residuals).

### 2.7 IROH-1X relay-assisted acceptance
- **Config.**
  - `RelayConfig { Default (n0), Disabled, Custom(Vec<String>) }` (`fe-sync/src/relay_config.rs:14-26`).
  - `FE_SYNC_RELAY` (`:50`), `resolve` (`:66-80`), `parse` (`:89-117`), `from_env` (`:122-127`)
    and `to_relay_mode` (`:130-152`).
  - `RelayHealth::on_relay_status` (`:233`) and `RelayStatusSummary::from_status` (`:264`).
  - The config is `Debug`-logged at `fe-sync/src/endpoint.rs:103` and `fe-sync/src/sync_thread.rs:459`.
- **Endpoint.**
  - `SyncEndpoint::new(secret, &RelayConfig)` (`fe-sync/src/endpoint.rs:86-118`) uses
    `presets::Minimal` (no DNS/pkarr discovery), BBR3 and the in-memory address book.
  - `node_addr` (`:126-139`) waits on `online()` when a relay is configured.
  - `home_relay_status` is at `:149`.
- **Production allowed-relay derivation.** `fe-sync/src/sync_thread.rs:620-628` computes
  `relay_config.to_relay_mode().relay_map().urls()` and passes it to
  `IrohBlobTransport::with_allowed_relays`.
- **Transport.** `sanitize_provider_addr` (`fe-sync/src/blob_transport.rs:35-50`) keeps a
  relay URL only if it is allowed. `with_allowed_relays` is at `:93` and `provider_ref` at `:104`.
- **Template test.** `iroh_transport_round_trips_a_hash_seq_on_loopback`
  (`fe-sync/tests/tileset_transfer_test.rs:518`), with `loopback_endpoint` at `:502-516`.
- **Skip-or-fail precedent.** `skip_unless_required` (`fe-sync/src/p2p_store_migration.rs:175-181`).
  The CI step runs it with the sweep's `-p` set plus `-p fe-sync`, and greps the test name
  (`.github/workflows/build-artifacts.yml:431-442`).
- **iroh 1.3 facts** (read from `~/.cargo/registry/.../iroh-1.3.0`, `iroh-relay-1.3.0`):
  - `Endpoint::Builder::clear_ip_transports` (`src/endpoint.rs:510`);
  - `Connection::paths()` → `PathInfo::is_relay()/is_ip()` (`src/endpoint/connection.rs:1184`;
    `socket/remote_map/remote_state/path_watcher.rs:475-482`);
  - `RelayMap::with_auth_token` (`iroh-relay src/relay_map.rs:154`), sent as
    `Authorization: Bearer` on native targets (`:258-268`).
- **iroh-relay server facts.**
  - `--dev` serves HTTP on `[::]:3340` with QUIC address discovery off (`src/main.rs:33-34`,
    `:588-595`, `cfg_defaults::enable_quic_addr_discovery() = false` `:375-377`).
  - It has a `/healthz` route (`src/server.rs:740`).
  - Access modes include `shared_token`, whose `IROH_RELAY_ACCESS_TOKEN` env override replaces
    the list (`src/main.rs:176-197`, `:231-240`). The override only applies when the config
    file selects `shared_token`.
- **Image.** `n0computer/iroh-relay:v1.3.0`, index digest
  `sha256:7a51e3826531dba5eee73d8b48a8f27284684d98c4d0da271d99127cc43b9d57` (amd64 + arm64).
  Entrypoint `["/iroh-relay"]`; exposes `80/tcp, 443/tcp, 3478/udp, 9090/tcp`. Captured in
  `.omc/research/iroh-relay-dockerhub-tags-2026-10-15.json` and
  `.omc/research/iroh-relay-image-config-2026-10-15.json`.
- **Runtimes.** Podman 5.7.1 is installed on the dev box (`C:\Program Files\RedHat\Podman\podman`);
  `docker` is not on PATH. GitHub `ubuntu-latest` ships docker.
- **Open task.** `conductor/tracks/iroh_1_0_upgrade_20260711/plan.md:123-126` asks for "two
  peers … whose direct IP paths are unusable".

### 2.8 OSS-1.3
Done at `da3e6a1`; full evidence in spec §OSS-1.3. Only bookkeeping remains (W6-11).

---

## 3. Slices (strictly sequential, one executor each)

### W6-0: re-ground + W5 composition check · S · haiku
**Owned files:** this track's `plan.md` (§1–§2 citations only) and `metadata.json`
(`partitions.computed_at_commit`).
**Depends:** W5 closed (RP-10 committed and W5's final sweep-manifest section appended).
**Steps.**
1. `git rev-parse --short HEAD`. For every `path:line` in §1–§3, re-grep the symbol and
   rewrite the line number.
2. Record W5's final AC-46 helper: its name, its file:line, and the list of doors that call it.
3. Mark explicitly whether `CreateAsset` and the asset rows of `InsertDevicePayloads` are
   capped.
4. Record whether W5 changed `caller_auth`, `api_token_context`, `ApiClaims` or `UploadOwner`.
   Expected: none. Any change halts W6 for an orchestrator decision.
5. Set `partitions.computed_at_commit` and `confidence: "verified"` once every `owns` path
   exists, or is marked `future`, and no path sits in two slices outside `shared_writes`.

**No cargo.** Docs only, and no gate beyond `git diff --stat` touching only these two files.

### W6-1: FEAPI-BUILDER-CRS · S · sonnet
**Owned files:**
- `fe-terrain/src/tiles/builder.rs`;
- `fe-format/src/manifest.rs`;
- `fe-format/src/AGENTS.md` (§scale).

**Depends:** W6-0.

**Change.**
1. Add `pub const DEFAULT_TILESET_CRS: &str = "EPSG:4326";` in `fe-format/src/manifest.rs`.
   `default_crs()` (`:81-84`) and `crs_or_default()` (`:89`) use it.
2. In `builder.rs`, extract one helper,
   `fn fresh_tileset_meta(&self, bounds: [f64; 4], tile_count: u32) -> TilesetMeta`, built as
   `TilesetMeta { …, ..Default::default() }` with **no** `crs` override (house rule,
   `fe-format/src/AGENTS.md` §scale).
3. The three sites (`:265-275`, `:371-381`, `:436-446`) call the helper. One helper means one
   test covers all three, which makes it mutation-safe.
4. Delete the "preserve pre-Default behavior exactly" comments.
5. Leave untouched every *deliberately legacy* `crs: None` fixture:
   - `fe-terrain/src/tiles/composite.rs:754`, `hexon_source.rs:770`, `store.rs:545`;
   - `fe-terrain/tests/phase7_4a_qa.rs:48,289,420,474`;
   - `fe-api/tests/hexon_tileset_test.rs:76`;
   - `fe-test-harness/src/scenarios/two_peer_tileset_stream.rs:48`;
   - `fractalengine/src/terrain_bridge.rs:1929`, `fractalengine-relay/src/main.rs:926`.

   These are test or synthetic fixtures. The `crs: None` hits at `fe-api/src/rest.rs:1299`
   and `:1447` are `QueryResultDto.crs`, a different struct.

**Flow tests to write.** In `builder.rs`'s `mod tests`:
`a_freshly_packaged_tileset_records_epsg_4326_in_its_raw_archive_meta`.
1. Package a multi-chunk container with `package_tiles_chunked` (same fixture shape as `:483`).
2. Open it with `HexonArchiveIndex`.
3. Read the outer and the per-chunk tileset meta members as **raw bytes**. Parse them as
   `serde_json::Value` and assert `value["crs"] == "EPSG:4326"` for each.

Mutation check: reintroduce `crs: None`, and the key is absent, so the test goes red.
**Declined:** a `resolve_petal_crs` test. `crs_or_default` already maps `None` to EPSG:4326,
so that test could never fail (a tautology), even though the ledger acceptance asks for it.
The docs slice records that in the ledger row.

**Acceptance:** the raw-meta test exists. `rg 'crs: None' fe-terrain/src/tiles/builder.rs` is
empty.

### W6-2: FEAPI-ELEV 32-statement cap · S · sonnet
**Owned files:**
- `fe-api/src/query_guard.rs`;
- `fe-database/src/handlers/elevated_query.rs` (shared with W5 RP-2 — append only);
- `fe-api/tests/elevated_query_test.rs`;
- `fe-api/AGENTS.md` (§elevated-query `:664-796`);
- `fe-database/src/AGENTS.md` (§handlers elevated paragraph only).

**Depends:** W6-1.

**Change.**
1. Add `pub const MAX_ELEVATED_STATEMENTS: usize = 32;` in
   `fe-database/src/handlers/elevated_query.rs` (fe-api already depends on fe-database).
2. **DB (the authority).** At the top of `elevated_query_reply` (`:48`), refuse when
   `request.statements.len() > MAX_ELEVATED_STATEMENTS`, before any snapshot, query or op-log
   work. The reply is the correlated failure the handler already uses, with the text
   `elevated script has {n} statements; the limit is 32`.
3. **API (fast fail, same envelope).** In `validate_elevated_sql`, right after
   `split_elevated_statements` (`:452`), return `Err(format!(…same text…))` when the count is
   over the cap. Count the split result **before** the TIMEOUT map, so empty statements never
   count.
4. Docs: one paragraph in fe-api §elevated-query, stating that the cap counts top-level
   statements only and that a `FOR`/`IF` block is one. The fe-database §handlers line gets the
   same.

**Flow tests to write.**
- `elevated_statement_cap_counts_top_level_statements`, in `fe-api/src/query_guard.rs`
  `mod tests`. It is table-driven over `validate_elevated_sql`. Cases:
  - 32 × `SELECT * FROM node` is accepted;
  - 33 is refused, with the text naming 32;
  - 33 `;` inside one string literal is accepted (one statement);
  - 40 `;` inside `{ }` braces of one statement are accepted;
  - a trailing `;;;` is not counted.
- `an_elevated_script_over_the_statement_cap_is_refused_and_writes_nothing`, in
  `fe-api/tests/elevated_query_test.rs`. It uses the `elevated(...)` helper (`:72`) as the
  Owner with opt-in on. It sends 33 `CREATE node …` statements and asserts:
  - `body["ok"] == false`;
  - the error names the cap;
  - `elevated_op_log_rows(&h)` (`:102`) is empty;
  - no new node row exists.
- `the_db_thread_refuses_an_elevated_request_over_the_statement_cap`, in
  `fe-database/src/handlers/elevated_query.rs` `mod tests` (next to `:576-658`). It builds an
  `ElevatedRequest` with 33 statements directly and asserts the failure reply and that
  nothing was written. Kept, not collapsed: the DB thread is a public seam with its own
  contract. The decision says the cap is enforced DB-side, and the API check is only a fast
  path.

**Acceptance:** the three tests exist, and the constant is the single source used by both
layers.

### W6-3: EGR-F32 — f64 positions end to end in BI egress · M · sonnet
**Owned files:**
- `fe-entity-store/src/lib.rs`, `fe-entity-store/AGENTS.md`;
- `fe-api/src/export.rs`, `fe-api/src/share.rs`, `fe-api/src/entity_store_bridge.rs`;
- `fe-query/src/columnar/geoparquet/codec.rs`, `fe-query/src/columnar/geoparquet/mod.rs`;
- `fe-query/src/columnar/provider.rs`, `fe-query/src/columnar/context.rs` (test helper);
- `fe-api/tests/export_share_test.rs`, `fe-api/tests/analytics_scope_test.rs`;
- `fe-api/AGENTS.md` (§geoparquet precision note `:920-921`);
- `fe-query/src/AGENTS.md` (§geoparquet `:98-136`).

**Depends:** W6-2.

**Change.**
1. In `fe-entity-store/src/lib.rs`, `EntitySnapshot.position` becomes `[f64; 3]` (`:17`) and
   `ReadingSnapshot.anchor_position` becomes `Option<[f64; 3]>` (`:47`). `rotation` and
   `scale` stay `[f32; 3]` (D-54).
2. `fe-api/src/export.rs`: drop the `as f32` casts in `row_to_snapshot` (`:575`, `:577`) and
   `reading_row_to_snapshot` (`:616`, `:618`).
3. `fe-api/src/entity_store_bridge.rs:44-70`: the position fields become f64.
4. `codec.rs`:
   - `point_z_to_wkb` takes `[f64; 3]`;
   - `wkb_to_point_z` returns `[f64; 3]`;
   - the CSV writers in `export.rs:635-712` format f64 with shortest round-trip (`{}` on
     f64), never via f32.
5. `provider.rs`: `pos_x/pos_y/pos_z` become `DataType::Float64` and `Float64Array`
   (`:59-61`, `:97-99`, `:115-117`). The `rot_*` and `scale_*` columns stay `Float32`.
6. Fix every compile fallout in the owned files: the `context.rs:128-132` helper and the
   `geoparquet/mod.rs` round-trip tests.

**Flow tests to write** (all seed the DB with true-f64 values through the existing
`seed_node`/`seed_petal` helpers in `export_share_test.rs`, simulating a replicated, elevated
or imported row):
- `parquet_and_csv_exports_carry_f64_positions_bit_exact`, in `fe-api/tests/export_share_test.rs`.
  - Seed a node at x = `123456.78901234567`, elevation = `0.000123456789012`,
    z = `-98765.4321098765`.
  - Export with `coords=local`, as both Parquet and CSV.
  - Assert the decoded geometry and the parsed CSV cells `==` the seeded f64 values, using
    `to_bits` equality. Under W6-4 the WKB components are in ENU order, so write the
    assertion against the frame helper W6-4 introduces. In W6-3 itself, assert the raw x,y,z
    order, and W6-4 updates this one assertion.
- `export_readings_anchor_position_is_f64_bit_exact`: the same shape, for the readings anchor.
- `analytics_nodes_table_serves_f64_positions`, in `fe-api/tests/analytics_scope_test.rs`. It
  seeds a snapshot with an f64 position, runs `SELECT pos_x FROM nodes`, and asserts exact
  equality. The column type is now `Float64`.
- **Tighten in place** (and name them in the manifest as modified):
  `export_csv_local_vs_latlon_landmine` (`:400`) and
  `export_readings_latlon_converts_anchor_position` (`:564`). Replace `< 1e-2` and `< 1e-4`
  with equality against `Projection::local_to_wgs84` computed in the test in f64. Mutation
  check: reintroduce `as f32`, and they go red.

**Acceptance:** no `as f32` remains on a position in `fe-api/src/export.rs`, and the four tests
above exist.

### W6-4: EGR-WKB — east, north, up for petal-local geometry · S · sonnet
**Owned files:**
- `fe-query/src/columnar/geoparquet/codec.rs`, `fe-query/src/columnar/geoparquet/mod.rs`;
- `fe-api/src/export.rs`, `fe-api/tests/export_share_test.rs`;
- `scripts/bi-egress-verify.ps1`;
- `fe-query/src/AGENTS.md`, `fe-api/AGENTS.md` (§export, §geoparquet), `docs/bi-egress.md`.

**Depends:** W6-3.

**Change.**
1. Add one pure frame function in `codec.rs`:
   `fn engine_to_wkb(position: [f64; 3], lonlat: bool) -> [f64; 3]`.
   - Lon/lat: identity, because the snapshot already holds `[lon, lat, ele]` from
     `export.rs:573-574`.
   - Local: `[x, -z, y]`.
   - Its inverse is `wkb_to_engine`.

   `snapshots_to_batch` and `readings_to_batch` pass `meta.lonlat_crs84`. The flag already
   exists (`mod.rs:34-38`) and is set at `export.rs:357-362`. **No new parameter crosses the
   crate boundary.**
2. The reader (`read_nodes_parquet` and `read_readings_parquet`, `mod.rs:158-197`) decides
   the inverse from the file's own `geo` metadata: `crs` absent means lon/lat, so identity;
   `crs: null` means local, so ENU→engine. A written file round-trips to engine positions,
   and the existing round-trip tests keep their meaning.
3. Add an informational `"fe:axes": "east,north,up"` to `geo_metadata_json` for local output
   only (assumption A7). CSV and analytics stay in the engine frame.
4. `scripts/bi-egress-verify.ps1`:
   - Add a node with an f32-representable position, e.g. `@(1234.5625, 12.25, -987.75)`.
     REST ingress is `[f32;3]`, so use exact binary fractions.
   - After the share URLs are minted, run
     `INSTALL spatial; LOAD spatial; SELECT ST_X(ST_GeomFromWKB(position)) AS e, ST_Y(…) AS n, ST_Z(…) AS u FROM read_parquet('$nodeShareUrl') WHERE node_id = '<id>'`.
   - `Assert-Equal` `e = 1234.5625`, `n = 987.75`, `u = 12.25`.
   - Add the same check for one readings anchor.
   - A failed `INSTALL spatial` (offline) fails the script loudly. It is never a skip.

   This proves the frame end to end through a real GIS reader. f64 precision is pinned by
   the Rust tests of W6-3.

**Flow tests to write.**
- `wkb_axis_order_follows_the_output_frame`, in `fe-query/src/columnar/geoparquet/mod.rs`
  `mod tests`. It is table-driven over a write then raw-WKB decode. Cases:
  - local engine `(1, 2, 3)` → WKB `(1, -3, 2)`;
  - lon/lat `(-122.3, 47.6, 12.0)` → identity;
  - the local file read back through `read_nodes_parquet` returns `(1, 2, 3)`;
  - `fe:axes` is present only on local output.
- `parquet_export_geometry_is_east_north_up_locally_and_lon_lat_ele_for_latlon`, in
  `fe-api/tests/export_share_test.rs`. It seeds a node at engine `(10, 2, -5)` and asserts:
  - the local Parquet geometry decodes to `(10, 5, 2)`;
  - the latlon geometry equals `Projection::local_to_wgs84` in `(lon, lat, ele)` order;
  - the CSV local header is still `x_m,y_m,z_m` (A7 regression).
- The script assertions above. They run in the final sweep's BI-egress e2e.

**Gate extra:** a parse-only check of the script, with no execution:
`pwsh -NoProfile -Command "[void][System.Management.Automation.Language.Parser]::ParseFile('scripts/bi-egress-verify.ps1',[ref]$null,[ref]$e); if($e){exit 1}"`.

**Acceptance:** the two tests and the script assertions exist. The latlon output is
byte-identical to before W6-4 for the same snapshot.

### W6-5: GPX-HIDE-CASCADE · M · sonnet
**Owned files:**
- `fe-ui/src/visibility/resolve.rs`, `fe-ui/src/visibility/mod.rs`, `fe-ui/src/visibility/cache.rs`;
- `fe-ui/src/verse_manager/db_results/properties.rs` (shared with W6-6 — W6-5 first);
- `fe-ui/src/node_manager/context_pick.rs` (test rows only);
- `fe-ui/src/AGENTS.md` (§visibility `:1018-1037`).

**Depends:** W6-4.

**Change.**
1. **Resolver.** In `node_hidden_reason` (`resolve.rs:67-113`), add a step between the own
   override (`:82-91`) and the ancestor chain (`:94-99`):
   - If `node_id` names a stamp, compute the owning track id: parse the marker id
     `{track}::stamp::{i}` (the same parse as `parse_stamp_marker_id`;
     `crate::verse_manager::parse_stamp_marker_id` is a pure fn), else look it up in the
     promoted alias map (step 3).
   - Evaluate the **track's** hidden reason with the same ancestry, with solo excluded and
     the owning-path step disabled. That is one level only, so recursion is impossible.
   - If the track's reason is `OverrideHide` or `Group(_)`, return the new
     `HiddenReason::OwningPath(track_id)`.

   The stamp's own Show/Hide was already decided at `:82-91`. That is exactly "a per-stamp
   override wins". Doing it inside the resolver means every consumer gets the cascade for
   free: `pickable`, `compute_sync_decision`, the sidebar and the ground-stamp tier. That is
   the "one resolver" rule (`fe-ui/src/AGENTS.md:1020-1023`).
2. Extend the exhaustive matches:
   - `toggle_node_override` (`:200-207`): `OwningPath` → `Show`, like `Ancestor`;
   - `node_eye_tooltip` (`:249`): "hidden with its path '<track>'".
3. **Promoted stamps (A11).** `VisibilityState` (`mod.rs:69-92`) gains a session map
   `promoted_stamp_alias: HashMap<String /*marker id*/, String /*promoted node id*/>`.
   - It is filled where `hydrate_promoted_stamp` runs, in `db_results/properties.rs:61`.
     That caller has both the node id and the bag's `path_id`/`instance_index`, so it builds
     the marker id with `stamp_marker_id`.
   - In step 2 of the resolver, a stamp's own override is
     `node_overrides[marker]`, else `node_overrides[alias]`. So a persisted `view.state`
     hydrated for the promoted row (`cache.rs:77-90`) governs the drawn stamp.
   - Any alias insert bumps `epoch`, so the apply pass re-runs.
4. Update `fe-ui/src/AGENTS.md` §visibility. Replace "hiding a track does not hide its stamps
   (no inheritance invented — open question)" with the ratified rule and A10's two
   non-cascades: the `gis.track.visible` style flag, and solo.

**Flow tests to write.**
- `a_stamp_follows_its_owning_tracks_visibility_unless_its_own_override_wins`, in
  `resolve.rs` `mod tests`. Table rows:
  - track Hide → stamp `OwningPath`;
  - track Hide + stamp Show → visible;
  - track in a hidden group → stamp hidden;
  - track visible + stamp Hide → `OverrideHide`;
  - hidden petal → `Ancestor` (unchanged);
  - solo with the stamp a member → visible;
  - solo with the track a member but the stamp not → `Solo` (A10);
  - a non-stamp node with a `::stamp::`-free id → unaffected;
  - track override Show → stamp visible even with the track's group hidden.
- `pickable_refuses_every_effectively_hidden_cause` (`mod.rs:1166`): add a row, "owning track
  hidden → its stamp is not pickable".
- `compute_sync_decision_hides_a_hidden_tracks_stamps_and_spares_a_shown_one`, in `mod.rs`
  `mod tests`. Entities: the track ribbon and three stamps of that track, one of which has a
  Show override. Hiding the track puts the ribbon and two stamps in `to_hide`. The shown
  stamp is in neither set.
- `a_promoted_stamps_persisted_view_state_beats_its_tracks_hide`, in `mod.rs` `mod tests`.
  Register the alias, hydrate `view.state = "show"` for the promoted id, hide the track →
  the marker is visible.
- `the_ground_stamp_tier_skips_hidden_stamps_and_other_petals` (`context_pick.rs:779`): add a
  row, "hidden track → its stamps are skipped".

**Acceptance:** the five tests exist, and the AGENTS "open question" line is gone.

### W6-6: HS-P6-CALLOUT · M · sonnet
**Owned files:**
- `fe-ui/src/actions/dimension_callout.rs` (new), `fe-ui/src/actions/mod.rs`, `fe-ui/src/actions/AGENTS.md`;
- `fe-ui/src/node_manager/measure.rs`, `fe-ui/src/node_manager/mod.rs`, `fe-ui/src/node_manager/AGENTS.md`;
- `fe-ui/src/panels/tool_options.rs`;
- `fe-ui/src/verse_manager/db_results/mod.rs` (shared with W6-7a/b), `fe-ui/src/verse_manager/db_results/nodes.rs`,
  `fe-ui/src/verse_manager/db_results/properties.rs` (after W6-5);
- `fe-api/src/endpoint.rs` (the doc comment `:150` only — shared with W6-7a);
- `fe-database/tests/entity_property_test.rs`.

**Depends:** W6-5.

**Change.** The template is the earthwork pattern, §2.3.
1. `dimension_callout.rs`:
   - Constants: `DIMENSION_CALLOUT_NODE_KIND = "dimension_callout"`,
     `CALLOUT_CORRELATION_PREFIX = "callout:"`, `MAX_CALLOUT_ANCHORS = 256`.
   - The resource `DimensionCalloutMap`, holding `pending: HashMap<callout_id, CalloutGeometry>`
     and `nodes: HashMap<node_id, CalloutGeometry>`.
   - Pure `callout_from_measure(&MeasureState) -> Option<CalloutGeometry>`. It returns `Some`
     only for Tape with exactly 2 anchors, or Area that is closed with at least 3 anchors.
   - Pure `callout_properties(&CalloutGeometry) -> Vec<(&str, Value)>`, writing
     `node_kind`, `dimension.mode` (`"tape"`/`"area"`), `dimension.anchors` (array of
     `[f64;3]`) and `dimension.closed`.
   - `hydrate_dimension_callout(node_id, &Value, &mut DimensionCalloutMap)`, the strictest
     reader. It accepts a bag only when it is the right kind, has every key, a known mode, an
     anchor count that fits the mode and ≤ 256, and all-finite numbers. Anything else is
     ignored, so it renders nothing.
2. `tool_options.rs` `render_measure_options` (`:424-490`): a "Save as callout" button,
   enabled iff `callout_from_measure(..).is_some()`. On click:
   - mint a ULID;
   - record `pending[ulid]`;
   - send `DbCommand::CreateNode { petal_id: active, name: "Dimension <readout>", position: <tape midpoint | area centroid>, correlation_id: Some("callout:<ulid>") }`.
3. `db_results/nodes.rs` `handle_node_created` (`:52-110`): beside the earthwork branch
   (`:80-108`), a `callout:` correlation moves `pending → nodes[node_id]` and sends one
   `SetNodeProperty` per bag key. `handle_node_deleted` (`:189`) removes `nodes[node_id]`.
4. `db_results/properties.rs` `handle_node_properties_loaded` (`:35-64`): call
   `hydrate_dimension_callout` next to `hydrate_earthwork_region` (`:64`).
5. Add the map to the `caches` SystemParam bundle in `db_results/mod.rs`, which keeps the
   16-param ceiling (see the comment there).
6. `node_manager/mod.rs` (`:262-300`): register `draw_dimension_callouts`. It runs always,
   not mode-gated, outside the chain.
   - It draws only callouts in the active petal whose node id passes
     `crate::visibility::effective_visibility(id, active_ancestry(&nav), &vis)`. Gizmos are
     not entities, so `sync_node_visibility` cannot hide them; the draw system must ask the
     resolver.
   - The draw itself is a pure `visible_callouts(&map, &nav, &vis) -> Vec<&CalloutGeometry>`
     plus gizmo lines.
   - The label is `readout(&MeasureState::from(geometry), measure_world_scale(..))`. The
     readout is recomputed per frame, never stored (A12).
7. `fe-api/src/endpoint.rs:150`: add `dimension_callout` to the documented kinds.
8. Docs:
   - `fe-ui/src/node_manager/AGENTS.md:128-129` "Not here … blocked on D-11" becomes the
     callout section.
   - `fe-ui/src/actions/AGENTS.md` gets a §dimension-callout entry.

**Flow tests to write.**
- `promote_builds_a_callout_bag_only_from_a_complete_measurement`, in `dimension_callout.rs`.
  Table: Tape with 2 → `Some`; Tape with 1 → `None`; Area open → `None`; Area closed with 3
  → `Some`; the bag keys and values are exact.
- `hydrate_dimension_callout_accepts_only_a_complete_finite_bounded_bag`. Table: complete →
  inserted; a missing key, a NaN, 257 anchors, a Tape with 3 anchors, or the wrong
  `node_kind` → ignored.
- `a_callout_correlation_echo_writes_the_bag_and_nothing_else_does`. Through
  `handle_node_created` with a crossbeam test channel as the DB sender (the process edge):
  - a `callout:` echo emits exactly the bag's `SetNodeProperty` commands;
  - a plain echo and an `earthwork:` echo emit no callout keys.
- `visible_callouts_skip_hidden_and_foreign_petal_callouts`.
- `a_deleted_callout_node_is_dropped_from_the_map`.
- `a_dimension_callout_node_round_trips_through_the_property_store`, in
  `fe-database/tests/entity_property_test.rs`. `CreateNode` → four `SetNodeProperty` → reload
  the node's properties → the bag is equal, including f64 anchors. Then tombstone the node →
  it is absent from a live hierarchy load.

**Acceptance:**
- The tests exist.
- `hexon_scale_orchestration_20260712/plan.md:120` is ticked by W6-11.
- The `:124` verification stays user-gated for the in-app check, and W6-11 notes that.

### W6-7a: TOKEN-CLIENT-CLAIM, core (identity, DB, API, harness) · M · sonnet (escalate to opus if the `asset_reach` rewrite outgrows §1.1)
**Owned files:**
- **fe-identity:** `src/api_token.rs`, `src/lib.rs` (re-exports `:11`), `examples/mint_api_token.rs`,
  `src/AGENTS.md` (§api-token).
- **fe-database:** `src/handlers/api_token.rs`, `src/api_token_store.rs`;
  `src/lib.rs` (the `MintApiToken` arm only); `src/handlers/device_payload.rs` (the outbound
  strip only); `tests/device_payload_test.rs`.
- **fe-runtime:** `src/messages.rs` (the 3 token items only).
- **fe-api:**
  - sources: `src/upload.rs`, `src/assets.rs`; `src/auth.rs`, `src/endpoint.rs` and
    `src/rest.rs` for the test literals only;
  - tests: `tests/asset_ownership_test.rs`, `tests/analytics_scope_test.rs`,
    `tests/distributed_query_test.rs`, `tests/export_share_test.rs`, `tests/gis_test.rs`,
    `tests/iot_ingest_test.rs`, `tests/mcp_integration.rs` (the `ApiClaims` literals only);
  - docs: `AGENTS.md` (§assets `:300-375`, §asset-ingest `:1520-`).
- **fe-test-harness:** `src/api.rs`, `src/peer.rs` (the mint arm only),
  `src/scenarios/api_token_flow.rs`.
- **fe-ui** (compile-only `client_did: None`): `src/panels/inspector.rs:713`,
  `src/dialogs/entity_settings.rs:572`.

**Depends:** W6-6.

**Change.**
1. **fe-identity.**
   - Add `ApiClaims.client: Option<String>` with
     `#[serde(default, skip_serializing_if = "Option::is_none")]`.
   - Add `pub fn validate_client_did(did: &str) -> anyhow::Result<()>`, which uses
     `public_key_from_did_key` and caps the length at 128.
   - Add `pub fn new_client_did() -> String`, from a fresh ed25519 key, keeping only the
     public DID.
   - Add `pub fn mint_client_api_token(keypair, scope, max_role, ttl_secs, jti, client_did: &str)`,
     which validates and then sets `client: Some`.
   - Existing `mint_api_token*` keep their signatures and mint legacy tokens (`client: None`).
     That keeps the ~20 test call sites untouched.
   - `verify_api_token` (`:100-112`) rejects a token whose `client` is present but invalid.
   - The CLI example gains `--client <did:key>` and `--new-client` (A3), and prints the
     client to stderr.
2. **fe-runtime.**
   - `MintApiToken` gains `client_did: Option<String>`.
   - `ApiTokenMinted` gains `client: String`.
   - `ApiTokenInfo` gains `client: Option<String>`.
3. **fe-database.**
   - `mint_api_token_handler` gains `client_did: Option<&str>`. It resolves the client with
     `validate_client_did` (`Some`) or `new_client_did` (`None`), mints with
     `mint_client_api_token`, and records `client` in the bookkeeping row.
   - Add `DEFINE FIELD IF NOT EXISTS client ON api_token TYPE option<string>`, idempotent and
     additive (`api_token_store.rs:15-29`).
   - `store_api_token` strips a null `client`, the same way as `label` (`:50-55`).
   - `records_to_info` maps it.
   - The dispatch arm (`lib.rs:873-882`) threads `client_did` through and echoes `client`.
4. **fe-api.**
   - `pub(crate) fn owner_key(claims) -> &str` returns `client.as_deref().unwrap_or(&sub)`.
     `UploadOwner::new` (`upload.rs:154-160`) uses it. Every door inherits it: REST `:640`,
     device `device.rs:275`, MCP `tools.rs:378` and `:788`.
   - The quota query (`:364`) is unchanged in shape and keyed on that owner.
   - `asset_reach` (`assets.rs:60-126`): the anchor rule (`:79-87`) only counts rows where
     `owner_did` is absent, equals `claims.sub` (node-level), or equals `owner_key(claims)`
     (A5). The reference scan is unchanged. The `:115-117` comment is rewritten.
   - **Do not touch `caller_auth` or `api_token_context`** (§1.1).
5. **Outbound strip (A6).** In `device_payload.rs`, the emission at `:395-406` serializes
   `asset.row_json` without `owner_did`. Remove the key in `fetch_assets` (`:659-692`) or at
   emission.
6. **Harness.**
   - `ApiHarness::mint_client_token(scope, role, client)` sits beside `mint_token`
     (`api.rs:273`).
   - The harness DB arm (`peer.rs:585-640`) applies the same `Some→validate / None→new`
     rule through the fe-identity helpers. Those are the one shared rule; the DB handler is
     `pub(crate)` and not reachable from the harness.
   - `api_token_flow.rs` constructors add `client_did: None`, and one leg asserts the minted
     token's `client` is a valid `did:key`.
7. Add `client: None` to each `ApiClaims` literal listed in §2.1.

**Flow tests to write.**
- In `fe-identity/src/api_token.rs` `mod tests`:
  - `a_client_token_round_trips_and_a_legacy_token_has_no_client`;
  - `a_token_with_a_malformed_client_is_rejected`. Table: not a DID, an x25519 `did:key`, an
    oversized string, an empty string.
- In `fe-database/src/handlers/api_token.rs` `mod tests` (in-memory DB):
  - `minting_records_the_requested_client_and_a_blank_request_gets_a_fresh_one`. Two blank
    mints get distinct clients. A named client round-trips through the JWT and through
    `ListApiTokens`.
  - `a_malformed_client_did_is_refused_at_mint_and_nothing_is_stored`.
- In `fe-api/tests/asset_ownership_test.rs` (`ApiHarness::spawn_with_upload_quota`):
  - `two_clients_of_one_node_have_separate_quota_buckets`. Client A fills its quota and gets
    413; client B uploads the same size → 201.
  - `a_legacy_token_uploads_into_the_node_level_bucket`. The row's `owner_did == sub`; a
    second legacy token shares the bucket.
  - `unplaced_upload_reach_follows_the_owner_key`. Table:
    - client A reads its own unplaced upload → 200;
    - client B in the same petal → 404, the uniform denial;
    - client B `place_asset` over MCP → refused with the unknown-id text;
    - a legacy token on a node-level upload → 200;
    - a legacy token on client A's upload → 404;
    - after A places it, B reads it by reference → 200.
  - `a_client_claim_naming_a_manager_did_gains_no_role`. Mint an editor-max token whose
    `client` is a verse Manager's DID, then try a Manager-only route (for example a
    visibility-group registry write, or an MCP Manager tool) → refused.
- In `fe-database/tests/device_payload_test.rs`:
  `an_outbound_asset_row_never_carries_the_uploader_client`. Ingest a payload whose asset
  has an `owner_did`; capture the replication emission; the asset row JSON has no
  `owner_did`.
- Harness `api_token_flow` (modified). It runs in the harness sweep.

**Acceptance:**
- The tests exist.
- `rg 'claims\.client' fe-api/src` hits only `owner_key`.
- `rg 'client' fe-api/src/auth.rs fe-api/src/endpoint.rs` hits only the test literals.

### W6-7b: TOKEN-CLIENT-CLAIM, GUI mint surfaces · S · sonnet
**Owned files:**
- `fe-ui/src/plugin.rs` (`InspectorFormState` `:252-255`, `:298-301` only);
- `fe-ui/src/dialogs/mod.rs` (`EntitySettings` fields `:183-186`);
- `fe-ui/src/actions/mod.rs` (`:1345-1348`);
- `fe-ui/src/viewport.rs` (`:311-314`, `:558`);
- `fe-ui/src/panels/inspector.rs`, `fe-ui/src/dialogs/entity_settings.rs`;
- `fe-ui/src/verse_manager/db_results/mod.rs` (`ApiTokenMinted` arm `:263-277`);
- `fe-ui/src/verse_manager/db_results/tokens.rs`.

**Depends:** W6-7a.

**Change.**
1. Add an optional "Client DID (blank = new client)" text field to both mint surfaces. Send
   it as `client_did` (`None` when blank, trimmed).
2. Show the minted `client` with a copy button in the generated-token panel.
3. Add a client column to the token list.
4. Validate the field client-side with `fe_identity::validate_client_did`, which disables
   Generate on invalid input. The DB door re-validates.

**Flow tests to write.**
`handle_api_token_minted_surfaces_the_client_for_both_mint_surfaces`, in `tokens.rs`. Pure:
`ApiTokenMinted` with a client → both the inspector and the EntitySettings dialog state hold
the token and the client.

**Acceptance:** the test exists. A blank field sends `None`, which the code asserts by
construction in the send-site helper `mint_command(scope, role, ttl, label, client_buf)`. That
helper is unit-tested in the same file:
`a_blank_client_field_sends_none_and_a_filled_one_sends_it_trimmed`.

### W6-8: IROH-1X relay-assisted acceptance against a local iroh-relay 1.3 container · M · sonnet (opus if container management gets hairy)
**Owned files:**
- `fe-sync/src/relay_config.rs`, `fe-sync/src/endpoint.rs`;
- `fe-sync/tests/relay_assisted_test.rs` (new);
- `fe-sync/Cargo.toml` (dev-dependencies only, if an RNG is needed for the test token);
- `.github/workflows/build-artifacts.yml` (the Linux job, one new step);
- `fe-sync/src/AGENTS.md` (§relay-health `:570-`);
- `fractalengine-relay/README.md` (relay secrets `:38-41`);
- `BUILDING.md` (the `FE_SECRET_*` table `:140-`).

**Depends:** W6-7b. **`fe-sync/src/sync_thread.rs` is NOT owned.** The production token path
enters through `SyncEndpoint::new`, so the call at `sync_thread.rs:511` needs no edit.

**Change.**
1. **8a, test infrastructure.** All of it lives in `fe-sync/tests/relay_assisted_test.rs`.
   - **Runtime choice.**
     - `FE_CONTAINER_RUNTIME` (`docker`|`podman`), else the first of `docker`, `podman` for
       which `<rt> info` exits 0.
     - With none: `skip_unless_required("no usable container runtime (docker/podman info failed)")`,
       which `eprintln!`s, or panics under `FE_REQUIRE_RELAY_CONTAINER=1`. This is the exact
       contract of `p2p_store_migration.rs:175-181`.
   - **Container start.**
     - `<rt> run -d --rm --name fe-relay-test-<pid>-<nanos> -p 127.0.0.1::3340 <IMAGE> --dev`
       (open legs). `<IMAGE>` is the pinned
       `n0computer/iroh-relay:v1.3.0@sha256:7a51e3826531dba5eee73d8b48a8f27284684d98c4d0da271d99127cc43b9d57`.
     - Read the mapped port with `<rt> port <name> 3340/tcp`.
     - Poll `GET /healthz` through a raw `std::net::TcpStream`, for up to 60 s.
     - Print `iroh-relay container <name> started on 127.0.0.1:<port>`.
   - **RAII guard.** Its `Drop` runs `<rt> rm -f <that exact name>` and prints the removal.
     It is never pattern-based, per the user's cleanup rules. Test profiles unwind on panic,
     so `Drop` runs on failure.
   - **Pull and readiness failures** follow the same skip-or-fail contract.
2. **8b, relay access token** (A14).
   - In `relay_config.rs`, add a `RelayAccessToken(String)` newtype with a redacting `Debug`
     (`RelayAccessToken(***)`), no `Serialize`, and no `Display`.
   - `RelayAccessToken::from_env()` reads `FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN` through
     `fe_identity::secret_store::EnvBackend`, with service `fractalengine` and account
     `sync_relay_token`.
   - Add `RelayConfig::to_relay_mode_with_token(Option<&RelayAccessToken>)`.
     - It applies `RelayMap::with_auth_token` only for `Custom`.
     - It refuses a token on any `http://` URL whose host is not loopback
       (`RelayConfigError::InsecureTokenTransport`).
     - `Default` and `Disabled` never carry the token.
   - Add `SyncEndpoint::with_relay_token(secret, &RelayConfig, Option<RelayAccessToken>)`.
     The existing `SyncEndpoint::new` (`endpoint.rs:86`) delegates to it with
     `RelayAccessToken::from_env()`, so production gets the token with **no** sync_thread
     edit. The endpoint stores `relay_token_configured: bool` for status only.
   - The token never enters `RelayConfig`, so the existing `?relay_config` logs
     (`endpoint.rs:103`, `sync_thread.rs:459`) cannot leak it.
   - Token legs of the test:
     - Write a 3-line TOML to a tempdir: `http_bind_addr = "0.0.0.0:3340"`,
       `enable_metrics = false`, and `access.shared_token = ["<random decoy>"]`.
     - Mount it read-only and add `--config-path /relay.toml`.
     - Pass the real per-run random token as `IROH_RELAY_ACCESS_TOKEN` through the child
       `Command`'s **environment**, using `-e IROH_RELAY_ACCESS_TOKEN` without a value, so
       the token is not on argv. It never goes in a file.
3. **8c, CI.** Add a Linux-job step after "Store-migration test" (`build-artifacts.yml:431-442`):

   ```yaml
   - name: Relay-assisted acceptance (iroh-relay 1.3 container required)
     timeout-minutes: 30
     shell: bash
     env:
       FE_REQUIRE_RELAY_CONTAINER: "1"
       FE_CONTAINER_RUNTIME: docker
     run: |
       cargo test --release -p fe-ui -p fe-terrain -p fractalengine -p fe-sync --test relay_assisted_test \
         -- --color never 2>&1 | tee "$RUNNER_TEMP/relay-assisted-test.log"
       for t in relay_assisted_tileset_fetch_through_a_self_hosted_iroh_relay \
                a_relay_only_dialer_reaches_its_peer_solely_through_the_relay \
                a_provider_naming_an_untrusted_relay_is_never_dialed \
                a_token_gated_relay_admits_the_configured_token_and_refuses_others; do
         grep -q "$t \.\.\. ok" "$RUNNER_TEMP/relay-assisted-test.log"
       done
   ```

   The `-p` set mirrors the migration step: cargo unifies features over the selected
   packages, so the release build is reused. The grep loop makes a zero-match filter or a
   skip go red. Windows CI cannot run Linux containers, so it does not run the step and the
   test self-skips.
4. **8d, docs.**
   - `fe-sync/src/AGENTS.md` §relay-health gets a "Self-hosting iroh-relay 1.x" subsection.
     It covers the pinned image, `--dev` being for local tests only, production TLS/ACME,
     `access.shared_token` plus the FE_SECRET token, and the token rules above.
   - `fractalengine-relay/README.md` and `BUILDING.md` get the new `FE_SECRET` row.
   - D-23 stays as-is (A1).

**Flow tests to write.** All are in `fe-sync/tests/relay_assisted_test.rs` and modelled on
`iroh_transport_round_trips_a_hash_seq_on_loopback`
(`fe-sync/tests/tileset_transfer_test.rs:518`).
- `relay_assisted_tileset_fetch_through_a_self_hosted_iroh_relay`.
  1. Alice and Bob are production `SyncEndpoint::new(secret, &RelayConfig::parse(url))` with
     `DocsStack::spawn`.
  2. Assert both `node_addr()` carry `TransportAddr::Relay(url)`, and that
     `RelayHealth::on_relay_status(home_relay_status() → RelayStatusSummary::from_status)`
     is healthy.
  3. Alice publishes a 3-bundle tileset.
  4. Bob's `IrohBlobTransport::from_stack(..).with_allowed_relays(cfg.to_relay_mode()?.relay_map().urls())`
     (the production derivation, `sync_thread.rs:620-628`) fetches with a provider address
     **stripped of every IP address**. The HashSeq and the bytes are verified.
  5. A direct `bob.inner().connect(relay_only_addr, <iroh-blobs ALPN>)` shows
     `paths().iter().any(PathInfo::is_relay)`.
- `a_relay_only_dialer_reaches_its_peer_solely_through_the_relay`.
  - The dialer's endpoint is
    `iroh::Endpoint::builder(presets::Minimal).relay_mode(cfg.to_relay_mode()?).clear_ip_transports()`.
    This is test scaffolding that mirrors `SyncEndpoint::new`, minus IP.
  - The fetch completes, and every path `is_relay()`. That is iroh_1_0's "direct IP paths
    unusable" criterion.
- `a_provider_naming_an_untrusted_relay_is_never_dialed`. Bob's transport has an empty
  allowed set, so the sanitized address has no transports and the fetch fails fast, before
  `CONNECT_TIMEOUT` (`blob_transport.rs:24`). That is the S8b guard against a real relay.
- `a_token_gated_relay_admits_the_configured_token_and_refuses_others`. In a token-mode
  container:
  - the endpoint with the right token goes healthy;
  - wrong-token and no-token endpoints never connect a home relay within the bounded wait
    (`HOME_RELAY_WAIT`, `endpoint.rs:29`).
- In `relay_config.rs` `mod tests`:
  - `relay_tokens_attach_only_to_custom_relays_over_tls_or_loopback`. Table: Default →
    none; Disabled → none; Custom https → attached; Custom `http://127.0.0.1` → attached;
    Custom `http://relay.example` → `InsecureTokenTransport`.
  - `a_relay_access_token_never_appears_in_debug_output`.

**Gate extra:** the YAML parse-only check
`python -c "import yaml,sys; yaml.safe_load(open('.github/workflows/build-artifacts.yml'))"`.
No container is started at gate time, because tests run only at the final sweep.

**Acceptance:**
- The tests and the CI step exist.
- `FE_REQUIRE_RELAY_CONTAINER=1` turns every skip into a failure.
- No relay default changed: `rg 'RelayConfig::Default' fe-sync/src/relay_config.rs` shows
  the arm untouched.

### W6-9: SECURITY review-and-fix · M · opus (fresh context; refute, do not confirm)
**Owned files:** `conductor/tracks/decisions_20261012_impl_20261015/review-w6-9.md` (future),
holding a one-line verdict and the findings list, plus fix-only edits to any file of W6-1..W6-8.
**Depends:** W6-8.

**Steps.**
1. Run `/security-review` scoped to the W6 diff (`git diff <W5-final>..HEAD`). Try to break
   every threat in §4, and add any the diff suggests.
2. Fix each finding directly, adding a failing-first test where the guard is new.
3. Record the verdict in `review-w6-9.md`. The orchestrator mirrors it into RUNBOOK §2's
   review ledger.
4. **BLOCKED, or any unfixed CRITICAL/HIGH, halts the run (AC-52).**

### W6-10: CORRECTNESS review-and-fix · M · opus (`/code-review high`)
**Owned files:** `review-w6-10.md` (future), plus fix-only edits to any file of W6-1..W6-9.
**Depends:** W6-9.

**Focus.**
1. f32→f64 fallout. Any remaining `as f32` on a position in egress, any
   `Float32Array::from` on position columns, and CSV formatting.
2. The ENU inverse. Does the reader use the file's own `geo.crs`, and is lon/lat output
   byte-identical?
3. The visibility resolver. No infinite recursion, solo precedence intact, epoch bumps on
   alias insert.
4. The callout create-echo race. An echo for an unknown `callout:` id must not panic. A
   partial bag after a crash renders nothing.
5. `api_token` schema idempotence, the null-strip of `client`, and `ApiTokenInfo` consumers.
6. The CI step's `-p` set reusing the release build, and the grep covering all 4 names.
7. The relay test's `Drop` cleanup on panic, port parsing on podman versus docker output, and
   `/healthz` readiness.
8. That no W5 hunk was reverted. Diff every §1.2 shared file against W5's final commit and
   confirm that only W6's named regions changed.

Same AC-52 halt rule.

### W6-11: docs, ledger and manifest · S · sonnet
Sonnet, not haiku: it records the W6-9 and W6-10 verdicts and SHAs across 6+ files. That
follows the AC-54 deviation precedent for RP-10.

**Owned files:**
- `conductor/doneness-ledger.md`, `conductor/tracks.md`, `conductor/final-sweep-manifest.md`;
- `conductor/tracks/oss_release_20260717/plan.md`;
- `conductor/tracks/hexon_scale_orchestration_20260712/plan.md`;
- `conductor/tracks/iroh_1_0_upgrade_20260711/plan.md`;
- `conductor/tracks/_archive/bi_verification_20261009/spec.md` (Known limitations `:62-69`);
- `conductor/tracks/_archive/analytics_egress_20260714/plan.md` (only if it restates the f32 or axis limit);
- this track's `plan.md` and `metadata.json` (status and progress).

**Depends:** W6-10.

**Steps.**
1. **Ledger rows to DONE-pending-sweep,** each with slice SHAs, its test names and "TESTS
   WRITTEN, NOT RUN":
   - TOKEN-CLIENT-CLAIM (`:239`): also correct its acceptance text. "quota lookups key on
     (owner_did, client_did)" becomes "owner_did = client (legacy: node DID); quota sums by
     it". Its source "RUNBOOK §3 R4" becomes "2026-10-12 grill".
   - GPX-HIDE-CASCADE (`:240`): its acceptance "via view.state" becomes "the stamp's own
     override (session marker, or the promoted row's persisted view.state)".
   - HS-P6-CALLOUT (`:216`).
   - EGR-WKB (`:218`): note the e2e is DuckDB spatial in the script, and that the frame is
     WKB-only (A7).
   - EGR-F32 (`:219`): note ingress stays f32 (A8).
   - FEAPI-BUILDER-CRS (`:220`): fix the drifted cites `:266/:376` to `:273/:379/:444`, and
     record why the `resolve_petal_crs` test was declined.
   - FEAPI-ELEV-TIMEOUT (`:187`): the cap is done.
   - IROH-1X (`:146`): the relay-assisted AC is done against a LOCAL container. D-23's host
     stays open.
2. **OSS-1.3 correction.**
   - Rewrite the `:29` paragraph as "OSS-1.3 is DONE (`da3e6a1`); the 2026-10-13 refresh
     read the pre-`da3e6a1` tree", citing CI run `38498925963`'s lint line and its
     warn-only posture (A15).
   - Fix the stale `:71` oss_release cell, which says "still open: 1.3".
   - Fix `:114` and `:119`.
   - Re-cite `:157`: `deny.toml:8-28`, `:32-41`, and `build-artifacts.yml:128-134`.
   - Tick `oss_release_20260717/plan.md:24` (task 1.3) with that evidence.
   - Fix `tracks.md:311` ("Open: cargo-deny CI step (OSS-1.3)").
3. Remove the stale "Two 2026-10-12 decisions have no ledger row yet" paragraph (`:31`). Both
   rows now exist (`:239`, `:240`), and the third (PETAL-RENDER-DISTANCE) is DONE. Recompute
   the class counts (`:94-102`) by a literal row count.
4. **Track plans.**
   - Tick `hexon_scale_orchestration_20260712/plan.md:120`. `:124` stays open for the
     user-gated in-app check.
   - Tick `iroh_1_0_upgrade_20260711/plan.md:123`, local-container AC, host still D-23.
   - Amend `bi_verification_20261009/spec.md:62-69` to "fixed by W6, see EGR-WKB/EGR-F32".
5. **Append the §7 block** below to `conductor/final-sweep-manifest.md` as section
   "W6 decisions_20261012_impl (`<base>..<tip>`)", with no line truncated. Add the
   per-slice must-cover lists the executors reported.
6. Set this track's `metadata.json` `status` to `implemented_pending_sweep`, and update
   `progress`.

---

## 4. Threat list (W6-9 must try each one; each fixed guard gets a test that fails without it)

| # | Threat | Guard | Slice | Test |
|---|---|---|---|---|
| TC-1 | A minter names a Manager's DID as `client`, and some door resolves roles by `client`, so the token escalates | `client` is never an `AuthContext`/`CallerAuth` subject (§1.1) | W6-7a | `a_client_claim_naming_a_manager_did_gains_no_role` |
| TC-2 | Client B reads, or MCP-places, client A's unplaced upload in a shared petal | `asset_reach` owner clause (A5) | W6-7a | `unplaced_upload_reach_follows_the_owner_key` |
| TC-3 | A legacy token reads a client's unplaced upload, or a client cannot read node-level uploads (regression) | node-level = `sub`, shared; client-level is private | W6-7a | same table |
| TC-4 | Quota evasion: a malformed or absent `client` forms a new bucket, or the quota is keyed on the wrong field | `owner_key` single choke point; `client` grammar checked at mint AND verify; per-sub token cap 50 (`api_token_store.rs:12`) bounds client churn per node | W6-7a | `two_clients_of_one_node_have_separate_quota_buckets`, `a_token_with_a_malformed_client_is_rejected` |
| TC-5 | A malformed `client` (not ed25519 `did:key`, oversized, control characters) reaches `owner_did` | `validate_client_did` at mint and verify | W6-7a | `a_malformed_client_did_is_refused_at_mint_and_nothing_is_stored` |
| TC-6 | The client DID leaks to every verse peer through replicated asset rows | outbound strip (A6) | W6-7a | `an_outbound_asset_row_never_carries_the_uploader_client` |
| TC-7 | A stale token (minted before the node's remote demotion) keeps uploading or ingesting assets that peers defer | W5's AC-46 helper must cover `CreateAsset` and `InsertDevicePayloads`; W6-0 records its coverage; W6-9 adds the call if missing (never a second helper) | W5/W6-9 | W5's `an_api_token_minted_before_the_nodes_demotion_cannot_write_after_it`, extended to an upload if W6-9 adds the door |
| TC-8 | Revocation bypass: a client token is not revocable, or revoke ownership is keyed on `client` | revoke stays by `jti` + `sub` (`api_token_store.rs:67-78`) | W6-7a | covered by the existing harness `api_token_flow` revoke leg (W6-9 verifies it with a client token) |
| TC-9 | An alg or claim confusion with the added field: an unsigned or `none` alg token, or a session token carrying `client` | EdDSA pinned (`api_token.rs:104`), `token_type == "api"` (`:108-110`) unchanged | W6-7a | existing `session_token_rejected_as_api`, `tampered_token_rejected` (`api_token.rs:175`, `:184`) |
| EL-1 | Cap bypass through splitter/parser disagreement: the API counts fewer statements than SurrealDB runs | comments are already rejected (`query_guard.rs:408`); the DB-side cap counts the same split; W6-9 fuzzes unbalanced brackets and escapes and records a verdict (an unbalanced script fails to parse, so nothing runs) | W6-2/W6-9 | `elevated_statement_cap_counts_top_level_statements` (+ any row W6-9 adds) |
| EL-2 | Block amplification: one `FOR` with N inner DML statements counts as 1 statement and carries no inner TIMEOUT | **verdict-only.** A documented residual (`fe-api/AGENTS.md:750-761`): Owner-only, opt-in endpoint, 7 s API backstop. W6-9 records whether to ban `FOR` in elevated (a follow-up row) | W6-9 | — (verdict row) |
| EL-3 | The DB handler trusts the API's count | DB-side `MAX_ELEVATED_STATEMENTS` check before any work | W6-2 | `the_db_thread_refuses_an_elevated_request_over_the_statement_cap` |
| RL-1 | The relay token leaks through `Debug`/logs/settings.ron/argv/files | newtype with redacting `Debug`, never in `RelayConfig` (logged at `endpoint.rs:103`, `sync_thread.rs:459`); container gets it by inherited env, file holds a decoy | W6-8 | `a_relay_access_token_never_appears_in_debug_output` |
| RL-2 | The operator token is sent to n0's public relays | token only on `Custom` maps | W6-8 | `relay_tokens_attach_only_to_custom_relays_over_tls_or_loopback` |
| RL-3 | The bearer token is sent in cleartext over remote `http://` | `InsecureTokenTransport` unless the host is loopback | W6-8 | same table |
| RL-4 | A wire-supplied provider names an attacker relay (SSRF-ish dial) | `sanitize_provider_addr` + allowed set from the node's own relay map | W6-8 | `a_provider_naming_an_untrusted_relay_is_never_dialed` |
| RL-5 | Test hygiene: the dev relay is exposed on the LAN, a container is leaked, or an unpinned image is pulled | publish on `127.0.0.1` only; unique name and exact-name `rm -f` in `Drop`; digest-pinned image; `--rm` | W6-8 | W6-9 reads the test (verdict) |
| RL-6 | Open-relay posture in production docs (anyone can relay through the operator's bandwidth) | docs recommend `shared_token` or an allowlist and TLS; `--dev` is marked test-only | W6-8 | — (doc verdict) |
| GV-1 | A stamp hidden by its track stays clickable (D-12 regression) | the cascade lives in the resolver that `pickable` uses | W6-5 | `pickable_refuses_every_effectively_hidden_cause` (new row), ground-stamp tier row |
| CO-1 | A peer-authored `dimension_callout` bag with NaN, huge or mismatched anchors crashes or stalls the draw loop | strictest-reader `hydrate_dimension_callout` (finite, ≤256, mode-consistent) | W6-6 | `hydrate_dimension_callout_accepts_only_a_complete_finite_bounded_bag` |
| X-1 | A W6 edit silently weakens W5 (for example a reverted `lifecycle_auth_context` hunk or a widened `caller_auth`) | W6-0 baseline; W6-10 diff check on every §1.2 file | W6-10 | verdict |

---

## 5. Correctness review focus (W6-10)

See W6-10's step list. It also runs the C6-style greps:
- `rg 'as f32' fe-api/src/export.rs fe-api/src/entity_store_bridge.rs` — no position hits;
- `rg 'crs: None' fe-terrain/src/tiles/builder.rs` — empty;
- `rg 'claims\.sub' fe-api/src/upload.rs fe-api/src/assets.rs` — only the node-level
  comparison in `asset_reach`, plus `owner_key`'s fallback;
- `rg 'MAX_ELEVATED_STATEMENTS'` — one definition and two uses.

---

## 6. Docs (W6-11, plus each slice's own AGENTS section)

Directory docs are written by the slice that owns the code:

| Doc | Slice |
|---|---|
| `fe-format/src/AGENTS.md` §scale | W6-1 |
| `fe-api/AGENTS.md` §elevated-query | W6-2 |
| `fe-database/src/AGENTS.md` elevated line | W6-2 |
| `fe-api/AGENTS.md` §geoparquet | W6-3/W6-4 |
| `fe-query/src/AGENTS.md` §geoparquet | W6-3/W6-4 |
| `docs/bi-egress.md` | W6-4 |
| `fe-entity-store/AGENTS.md` | W6-3 |
| `fe-ui/src/AGENTS.md` §visibility | W6-5 |
| `fe-ui/src/node_manager/AGENTS.md` | W6-6 |
| `fe-ui/src/actions/AGENTS.md` | W6-6 |
| `fe-identity/src/AGENTS.md` §api-token | W6-7a |
| `fe-api/AGENTS.md` §assets/§asset-ingest | W6-7a |
| `fe-sync/src/AGENTS.md` §relay-health | W6-8 |
| `fractalengine-relay/README.md` | W6-8 |
| `BUILDING.md` | W6-8 |

W6-11 does the conductor bookkeeping only. RUNBOOK §2/§3/§4/§8 stay orchestrator-owned: the
orchestrator copies spec §Assumptions A1–A15 into RUNBOOK §4 at W6 launch.

---

## 7. Final-sweep manifest additions (W6-11 appends verbatim; listed here, NOT yet in the manifest)

```markdown
## W6 decisions_20261012_impl (`<W5-final>..<W6-tip>`)

**Core-suite rows (add to the table):**
| **Relay-assisted acceptance (IROH-1X)** | Windows: `podman machine start` if stopped, then `$env:FE_REQUIRE_RELAY_CONTAINER='1'; $env:FE_CONTAINER_RUNTIME='podman'; cargo test -p fe-sync --test relay_assisted_test -- --nocapture` | All 4 tests `... ok` BY NAME (`relay_assisted_tileset_fetch_through_a_self_hosted_iroh_relay`, `a_relay_only_dialer_reaches_its_peer_solely_through_the_relay`, `a_provider_naming_an_untrusted_relay_is_never_dialed`, `a_token_gated_relay_admits_the_configured_token_and_refuses_others`); output shows `iroh-relay container fe-relay-test-… started` and its removal; ZERO `skipping:` lines; afterwards `podman ps -a --filter name=fe-relay-test-` lists nothing this run created (remove only the exact names the run printed) |
| **BI egress e2e (W6 additions)** | `scripts/bi-egress-verify.ps1` (network needed for DuckDB `INSTALL spatial`) | New asserts pass: node parquet `ST_X/ST_Y/ST_Z` = east/north/up (`1234.5625`, `987.75`, `12.25`) + one readings-anchor ENU assert; a failed `INSTALL spatial` is a FAIL, not a skip |
| **CI relay step (post-push)** | Watch the Linux job step "Relay-assisted acceptance (iroh-relay 1.3 container required)" on the pushed tip | Step green; its 4-name grep loop passed |

**W6-1 (FEAPI-BUILDER-CRS) — must cover:**
- fe-terrain `tiles::builder::tests::a_freshly_packaged_tileset_records_epsg_4326_in_its_raw_archive_meta`
- fe-terrain `tiles::builder::tests::package_tiles_chunked_produces_a_container_that_verifies_sealed` (helper refactor)
- fe-format manifest tests (`legacy_tileset_meta_deserializes_with_defaults` + `DEFAULT_TILESET_CRS` users)
- `cargo test -p fe-terrain --features fetch,render` (builder sits behind it)

**W6-2 (FEAPI-ELEV cap) — must cover:**
- fe-api `query_guard::tests::elevated_statement_cap_counts_top_level_statements`
- fe-api `--test elevated_query_test` (ALL, incl. new `an_elevated_script_over_the_statement_cap_is_refused_and_writes_nothing` and existing `long_scan_hits_the_db_side_timeout`)
- fe-database `handlers::elevated_query::tests::the_db_thread_refuses_an_elevated_request_over_the_statement_cap` + existing `read_only_script_with_let_and_return_answers`, `a_write_over_a_table_past_the_diff_cap_is_refused_before_running`

**W6-3 (EGR-F32) — must cover:**
- fe-api `--test export_share_test` (ALL; new `parquet_and_csv_exports_carry_f64_positions_bit_exact`, `export_readings_anchor_position_is_f64_bit_exact`; TIGHTENED `export_csv_local_vs_latlon_landmine`, `export_readings_latlon_converts_anchor_position`)
- fe-api `--test analytics_scope_test` (ALL; new `analytics_nodes_table_serves_f64_positions`)
- fe-query `--features parquet` `columnar::geoparquet` + `columnar::provider` + `columnar::context` tests
- fe-api lib `entity_store_bridge` tests

**W6-4 (EGR-WKB) — must cover:**
- fe-query `--features parquet` `columnar::geoparquet::tests::wkb_axis_order_follows_the_output_frame`, `round_trip_preserves_rows_and_geo_metadata`, `bytes_writer_matches_file_writer_round_trip`, `crs_key_absent_only_for_lonlat_output`
- fe-api `export_share_test::parquet_export_geometry_is_east_north_up_locally_and_lon_lat_ele_for_latlon`
- `scripts/bi-egress-verify.ps1` (row above)

**W6-5 (GPX-HIDE-CASCADE) — must cover:**
- fe-ui `visibility::resolve::tests::a_stamp_follows_its_owning_tracks_visibility_unless_its_own_override_wins`
- fe-ui `visibility::tests::pickable_refuses_every_effectively_hidden_cause`, `compute_sync_decision_hides_a_hidden_tracks_stamps_and_spares_a_shown_one`, `a_promoted_stamps_persisted_view_state_beats_its_tracks_hide`
- fe-ui `node_manager::context_pick::tests::the_ground_stamp_tier_skips_hidden_stamps_and_other_petals`
- all of `fe-ui` `visibility::` (resolver change touches every consumer)

**W6-6 (HS-P6-CALLOUT) — must cover:**
- fe-ui `actions::dimension_callout::tests::{promote_builds_a_callout_bag_only_from_a_complete_measurement, hydrate_dimension_callout_accepts_only_a_complete_finite_bounded_bag, a_callout_correlation_echo_writes_the_bag_and_nothing_else_does, visible_callouts_skip_hidden_and_foreign_petal_callouts, a_deleted_callout_node_is_dropped_from_the_map}`
- fe-database `--test entity_property_test` (ALL; new `a_dimension_callout_node_round_trips_through_the_property_store`)
- fe-ui `node_manager::measure` tests (`reducer_table`, `readout_rows_carry_units`) and `actions::terrain_proposal` earthwork-echo tests (shared `handle_node_created`)
- USER_GATED (not sweep): in-app promote → reload → callout re-renders (hexon_scale plan `:124`)

**W6-7a/b (TOKEN-CLIENT-CLAIM) — must cover:**
- fe-identity `api_token::tests::{a_client_token_round_trips_and_a_legacy_token_has_no_client, a_token_with_a_malformed_client_is_rejected}` + existing `session_token_rejected_as_api`, `tampered_token_rejected`
- fe-database `handlers::api_token::tests::{minting_records_the_requested_client_and_a_blank_request_gets_a_fresh_one, a_malformed_client_did_is_refused_at_mint_and_nothing_is_stored}`
- fe-database `--test device_payload_test` (ALL; new `an_outbound_asset_row_never_carries_the_uploader_client`)
- fe-api `--test asset_ownership_test` (ALL; new `two_clients_of_one_node_have_separate_quota_buckets`, `a_legacy_token_uploads_into_the_node_level_bucket`, `unplaced_upload_reach_follows_the_owner_key`, `a_client_claim_naming_a_manager_did_gains_no_role`; existing 6 unchanged in meaning)
- fe-api `--test mcp_integration`, `--test iot_ingest_test`, `--test device_ingest_test`, `--test gis_test`, `--test distributed_query_test` (claims literals + every upload door now via `owner_key`)
- fe-ui `verse_manager::db_results::tokens::tests::{handle_api_token_minted_surfaces_the_client_for_both_mint_surfaces, a_blank_client_field_sends_none_and_a_filled_one_sends_it_trimmed}`
- harness `api_token_flow` scenario (in the 18-scenario ×2 harness run)
- e2e `hexon-install-verify.ps1` / `hexon-serve-verify.ps1` / `bi-egress-verify.ps1` still green with LEGACY CLI tokens (A3 — legacy path regression)

**W6-8 (IROH-1X) — must cover:**
- the relay-assisted row above
- fe-sync lib `relay_config::tests::{relay_tokens_attach_only_to_custom_relays_over_tls_or_loopback, a_relay_access_token_never_appears_in_debug_output}` + existing `endpoint::tests::*`
- `scripts/relay-health-verify.ps1` (already a core row) still passes (no default change)

**W6-9 / W6-10 must-cover:** append the reviewers' `test_sweep_must_cover` lists verbatim at W6 close.
```

---

## 8. Decisions and owner questions defaulted

The defaults (A1–A15) are in spec §Assumptions, with their reversal costs. The owner questions
this track **defaulted rather than asked** are listed below. Following the lesson that a slice
depends on its question being defaulted first, each was settled before the slice that needs
it:

| Question | Default | Reversal | Slice that depends on it |
|---|---|---|---|
| D-23: which relay is the default | Unchanged (n0 v1 fleet + warning); container recipe documented; no host named | cheap once a host exists | W6-8 |
| What is a `client`, and what does a blank mint do | ed25519 `did:key`; blank → fresh client; CLI stays legacy unless flagged | cheap | W6-7a |
| Does per-client ownership change who can READ unplaced uploads | yes: private per client; node-level stays shared; placed = reference reach | cheap | W6-7a |
| Is the relay credential in scope | yes: env-only token, Custom-only, TLS/loopback-only | cheap | W6-8 |
| Is the ENU frame WKB-only | yes; CSV/analytics keep the engine frame, plus a `fe:axes` hint | cheap | W6-4 |
| Which surfaces get f64 | BI egress only; REST DTO/GPX/.hexon/ingress/rot-scale stay f32 | moderate | W6-3 |
| How the cap counts | top-level statements, empty ones excluded, blocks = 1, both layers | cheap | W6-2 |
| What hides a track for the cascade | lattice only (override Hide / hidden group); not the style flag, not solo | cheap | W6-5 |
| Callout content | Tape + Area; anchors only; readout recomputed; earthwork create pattern | cheap | W6-6 |

---

## 9. Risks

- **W5 lands with a different shape** (for example the AC-46 helper moved, or `CallerAuth`
  changed). W6-0 halts on any change to `caller_auth`, `api_token_context`, `ApiClaims` or
  `UploadOwner`.
- **The DuckDB spatial extension needs network at sweep time.** It is loud by design (A13
  analogue). If the sweep box is offline, the BI e2e row fails rather than skips, and the
  sweep reports it.
- **Podman on Windows.** A stopped `podman machine` makes `podman info` fail, so the test
  skips locally unless `FE_REQUIRE_RELAY_CONTAINER=1`. The manifest row starts the machine
  first. Mounting the token-mode TOML from `%TEMP%` relies on podman-machine's default `C:\Users`
  mount. If that is unavailable, the token leg fails loudly under the flag; it does not
  silently skip.
- **IPv6 inside the container.** `--dev` binds `[::]:3340`. The token leg's TOML pins
  `0.0.0.0:3340`. If an open leg cannot reach a v6-only listener through the v4 publish, the
  fix is to always mount the TOML (W6-8 decides at implementation).
- **The callout create is non-atomic** (earthwork pattern). A crash between `CreateNode` and the
  property writes leaves a plain node that renders nothing. It is deletable from the sidebar.
  Accepted (A12).
- **f64 widening touches every `EntitySnapshot` user.** `fe-query` provider/context and
  fe-api's bridge, export and share. The compile gate catches all of them. The analytics
  `pos_*` type change is a schema change for SQL consumers, which is acceptable because there
  are no consumers (2026-10-12 grill: "breaking is cheap").
- **Reach tightening (A5) and token rotation.** A rotated client cannot read its predecessor's
  *unplaced* uploads; placed ones are unaffected. Use the GUI's "Client DID" field, or the
  CLI's `--client`, to keep continuity. Documented in fe-api §assets.

---

## 10. Deferral audit (what W6 consciously does not do)

| Not done | Why | Trigger to revisit |
|---|---|---|
| Making the self-hosted relay the default (D-23 posture) | Owner question: no host exists (R6) | The owner names a host/domain |
| f64 REST DTOs, GPX/.hexon export, f64 ingress | Outside "in egress" (A8) | A survey-grade consumer, or an import precision complaint |
| Banning `FOR`/block amplification in elevated | Not ratified; Owner-only opt-in endpoint | W6-9's EL-2 verdict says otherwise |
| Persisted overrides for never-promoted stamps | No node row exists to persist to | Stamps get rows by default (PATH-ASSET-W5) |
| `gis.track.visible` → stamp cascade, solo → stamps | Outside the lattice / not ratified (A10) | User asks in-app |
| In-app verification of the callout and the cascade | User-gated (RUNBOOK §4) | The user's UX pass |
