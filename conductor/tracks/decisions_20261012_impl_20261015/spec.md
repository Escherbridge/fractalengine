---
type: Track Spec
title: Implement the 2026-10-12 decisions (campaign W6)
track: decisions_20261012_impl_20261015
tags: [security, auth, egress, fe-api, fe-identity, fe-query, fe-ui, fe-sync, iroh, decisions_20261012_impl_20261015]
created: 2026-10-15
status: planned
resource: ./metadata.json
plan: ./plan.md
depends_on: [role_propagation_20261013]
ledger_rows: [TOKEN-CLIENT-CLAIM, GPX-HIDE-CASCADE, HS-P6-CALLOUT, EGR-WKB, EGR-F32, FEAPI-BUILDER-CRS, FEAPI-ELEV-TIMEOUT (32-statement-cap residual), OSS-1.3 (ledger correction only), IROH-1X (relay-assisted AC residual)]
decisions: ../../RUNBOOK.md#3-decisions
---

# Implement the 2026-10-12 decisions (W6)

## Why

The 2026-10-12 grill (RUNBOOK §3, `conductor/RUNBOOK.md:255-284`) ratified a batch of small,
cross-cutting product contracts. The 2026-10-13 grill (R3-R7, `RUNBOOK.md:172-253`) confirmed
them and settled the relay-host question for now. None of them is built yet, except
PETAL-RENDER-DISTANCE (done in `b9a85f3`, excluded here) and OSS-1.3 (done in `da3e6a1`; only
the ledger is wrong, see §OSS-1.3 below). RUNBOOK §7 step 5 (`RUNBOOK.md:456-459`) schedules
them after role propagation (W5) and before the campaign final sweep.

This track runs **after W5** (`conductor/tracks/role_propagation_20261013/`). W5 rewrites the
replicated-row admission gate and adds the AC-46 API-role cap
(`role_propagation_20261013/plan.md:945-957`). TOKEN-CLIENT-CLAIM touches the same token path.
W6 lands on top of W5 and never undoes it (§Interaction with W5).

## Decisions being implemented (ratified text, quoted)

| # | Ledger row | Ratified text | Source |
|---|---|---|---|
| 1 | TOKEN-CLIENT-CLAIM | "add a `client` claim at mint; asset `owner_did`/quota key on it; `sub` stays the issuing node DID." Plus: "Legacy API tokens without a `client` claim are treated as node-level ownership · to reverse: force re-mint (cheap now — no consumers)." | `RUNBOOK.md:268-269`, §4 `:396-397`; ledger `doneness-ledger.md:239` |
| 2 | GPX-HIDE-CASCADE | "Hiding a GPX track hides its stamps (cascade on `owning_path_id`); a per-stamp override wins." | `RUNBOOK.md:276`; ledger `:240` |
| 3 | HS-P6-CALLOUT | "D-11: dimension callouts persist as addressable nodes (`node_kind=dimension_callout`)." | `RUNBOOK.md:275`; ledger `:216`; D-11 resolution `outstanding_decisions_20260715/spec.md:43` |
| 4 | EGR-WKB | "WKB petal-local frame = east,north,up `(x, -z, y)`" + §4 "WKB east-north-up applies only to petal-local outputs; `coords=latlon` output unchanged". | `RUNBOOK.md:278-279`, §4 `:398-399`; ledger `:218` |
| 5 | EGR-F32 | "f64 positions end to end in egress". D-54 amended: "the arrow 54.x / f32 rotation-scale / EPSG:4326-placeholder details stand except where superseded". | `RUNBOOK.md:279`; `outstanding_decisions_20260715/spec.md:31`; ledger `:219` |
| 6 | FEAPI-BUILDER-CRS | "fresh tilesets stamp `crs=EPSG:4326`". | `RUNBOOK.md:279`; ledger `:220` |
| 7 | FEAPI-ELEV-TIMEOUT (residual) | "elevated: DB-side TIMEOUT on every statement + 32-statement cap". The 5 s TIMEOUT landed in `562f551`; the cap did not. | `RUNBOOK.md:280`; ledger `:187` |
| 8 | OSS-1.3 | "allow all 7 outlier licences in deny.toml and turn `cargo deny check licenses` on in CI". **Already done** — ledger correction only. | `RUNBOOK.md:273-274`; ledger `:29` (stale) vs `:157` (correct) |
| 9 | IROH-1X (residual) | Relay-assisted acceptance against a self-hosted iroh-relay 1.3. R6: "Relay host (D-23): undecided — the relay-assisted acceptance test runs against a LOCAL iroh-relay 1.3 container; `FE_SYNC_RELAY` stays a settings placeholder until a host exists." | `RUNBOOK.md:234-235`, `:270-272`; ledger `:146`; `iroh_1_0_upgrade_20260711/plan.md:123-126` |

## Scope

1. **TOKEN-CLIENT-CLAIM.**
   - `ApiClaims` (`fe-identity/src/api_token.rs:17-33`) gains an optional `client` claim (a
     `did:key`). Every new token minted through the production mint door carries one. If the
     minter names no client, the door generates a fresh one.
   - `sub` stays the node DID.
   - Every upload door attributes `asset.owner_did` to `client`, or to `sub` for a legacy
     token. Those doors are REST, MCP and device ingest, all of which go through
     `UploadOwner::new`, `fe-api/src/upload.rs:154-160`. The per-owner quota
     (`fe-api/src/upload.rs:356-380`) sums by that same key.
   - Unplaced client-owned uploads become readable only by their own client.
   - Outbound replicated asset rows stop carrying `owner_did`.
   - The GUI mint dialogs gain an optional "Client DID" field and show the minted client.
2. **GPX-HIDE-CASCADE.**
   - A stamp is hidden when its owning track is hidden in the visibility lattice, unless the
     stamp's own Show or Hide override says otherwise. The owning track is the `{track}` in
     the stamp marker id `{track}::stamp::{i}`, which equals the promoted row's
     `owning_path_id`.
   - One resolver change (`fe-ui/src/visibility/resolve.rs:67-113`) makes every picker,
     apply pass and sidebar consistent.
3. **HS-P6-CALLOUT.**
   - The Measure tool gains a "Save as callout" action. It creates an addressable node with
     `properties.node_kind = "dimension_callout"`, the measured anchors and the mode.
   - The callout re-renders on reload, honours visibility, and replicates and deletes like
     any node.
4. **EGR-WKB.** For `coords=local`, Parquet geometry (node position and reading anchor)
   becomes WKB Point Z in east, north, up = `(x, -z, y)`. `coords=latlon` is unchanged.
5. **EGR-F32.**
   - Node positions and reading anchor positions stay `f64` from the DB row to every BI
     egress byte: Parquet, CSV, share redemption and the analytics `nodes` table.
   - Rotation and scale stay `Float32` (D-54 stands).
6. **FEAPI-BUILDER-CRS.** `TilesetBuilder` writes `crs: "EPSG:4326"` into every fresh
   archive's meta: the single archive, the outer container and each chunk.
7. **FEAPI-ELEV 32-statement cap.**
   - `/api/v1/query/elevated` refuses any script with more than 32 top-level statements.
   - The check runs in two places: fe-api, which fails fast with the existing `ok:false`
     envelope, and the DB handler, which is the authority.
8. **OSS-1.3.** Correct the ledger and the board. The code and CI are already done (§OSS-1.3
   resolution).
9. **IROH-1X relay-assisted acceptance.**
   - A new fe-sync integration test runs real fe-sync endpoints and transport against a
     **locally managed `n0computer/iroh-relay:v1.3.0` container**, pinned by digest.
   - The test skips with a reason only where no usable container runtime exists. It fails
     instead under `FE_REQUIRE_RELAY_CONTAINER=1`, which mirrors the store fixture's
     `FE_REQUIRE_STORE_FIXTURE` (`fe-sync/src/p2p_store_migration.rs:175-181`).
   - CI's Linux job runs it as required.
   - A relay access token is supported, from the environment only. It is the credential a
     self-hosted relay needs.

## Interaction with W5 (role propagation) — the composition contract

- **The authorization subject stays the node.**
  - `caller_auth` (`fe-api/src/endpoint.rs:49-53`) keeps `did: claims.sub`.
  - `api_token_context` (`fe-api/src/auth.rs:58-64`) keeps `subject: claims.sub`.
  - W5's AC-46 cap computes `min(token max_role, role_at(local_did, scope))`
    (`role_propagation_20261013/plan.md:945-957`). It resolves the role of the node, because
    peers judge only the node's authored rows.
  - **The `client` claim never enters role resolution.** It never appears as an
    `AuthContext`/`CallerAuth` DID, never raises `max_role`, and never bypasses the cap.
    Otherwise a minter could name a Manager's DID as `client` and inherit that Manager's
    role (threat TC-1).
- **Order of checks on an upload (after W6):**
  1. JWT verify, including the `client` grammar.
  2. Revocation by `jti`.
  3. `require_role_and_scope` at the anchor petal.
  4. W5's AC-46 cap, wherever the door is a replicated write.
  5. Reach and quota, keyed on the owner key: `client`, or `sub` for a legacy token.

  `client` only narrows ownership inside what the node may already do.
- **Shared files.** W5 and W6 both edit:
  - `fe-database/src/lib.rs`: W5's RP-2/3/4/6 arms; W6's `MintApiToken` arm `:873-882`.
  - `fe-runtime/src/messages.rs`: W5's `ReplicatedRowOutcome::Deferred`; W6's
    `MintApiToken`/`ApiTokenMinted`/`ApiTokenInfo`.
  - `fe-test-harness/src/peer.rs`: W5's RP-3/6/8; W6's `MintApiToken` arm `:585-640`.
  - `fe-database/src/handlers/elevated_query.rs`: W5's RP-2 pin; W6's statement cap.
  - `fe-database/src/handlers/device_payload.rs`: W5's RP-0; W6's outbound `owner_did` strip.
  - `fe-database/tests/device_payload_test.rs`: W5's RP-0/RP-4; W6's strip test.
  - `fe-ui/src/plugin.rs`: W5's RP-4b; W6's mint-client buffer.
  - The `fe-database/src/AGENTS.md` and `fe-sync/src/AGENTS.md` docs.

  W6 starts from W5's final commit. Slice W6-0 re-grounds every citation, and no W6 slice
  may revert a W5 hunk.

## Out of scope

- PETAL-RENDER-DISTANCE: done (`b9a85f3`, ledger `:241`).
- The second-campaign items (RUNBOOK §4 `:312-316`): D-48 CRUD, MSA-P4, inspector FR-3/FR-4,
  PATH-ASSET-W5, PLUG-HOST and the device decoder plugins.
- **D-23, changing the default relay.** It stays an owner question; see assumption A1. W6
  hardcodes no relay URL.
- Position **ingress** precision. `CreateNode`/`UpdateNodeTransform` carry `[f32; 3]`
  (`fe-runtime/src/messages.rs:431`, `:495`), but the decision is about egress. Assumption
  A8.
- BLOB-HASH-CAPABILITY (ledger `:247`): recorded by W5 as out of scope; unchanged here.
- Persisting a never-promoted stamp's visibility override. Such a stamp has no node row
  (`fe-ui/src/visibility/mod.rs:353-358`), so its override stays session-only, as today.

## Assumptions (unasked — defaulted here, highest reversal cost first; RUNBOOK §4 style)

Each line gives the default and the reversal cost. The orchestrator copies them into RUNBOOK
§4 when W6 launches. The lesson from earlier workflows is that an owner question a slice
depends on is defaulted **before** that slice runs.

- **A8 — f64 scope.** "Egress" means the BI egress surfaces: `export.parquet`, `export.csv`,
  `/shared/{token}` (`fmt=parquet|csv`), and the `/query/analytics` `nodes` table
  (`pos_x/pos_y/pos_z` become `Float64`). These stay `f32`:
  - the REST node DTOs (`fe-api/src/rest.rs:958-959`);
  - GPX export (`fe-api/src/gpx.rs:247-248`);
  - the `.hexon` petal export (`fe-api/src/format.rs:241-242`);
  - position ingress;
  - rotation and scale (D-54).

  Reversal: moderate (widen each DTO; it is a wire change for REST consumers).
- **A5 — unplaced-upload reach per client.**
  - An unplaced upload owned by client C is reachable only by C, still within the
    anchor-petal scope.
  - Node-level (legacy) uploads, whose `owner_did` is the node's `sub`, keep today's
    anchor-petal reach for every token in scope.
  - Placed assets keep reference reach.

  This finishes FEAPI-ASSET-OWNER's original "readable by uploader only" acceptance
  (`doneness-ledger.md:184`; `fe-api/AGENTS.md:355-359` foresaw "a future per-caller subject
  claim"). Reversal: cheap (drop the owner clause in `asset_reach`).
- **A3 — client identity shape.**
  - `client` is a `did:key` (ed25519), validated with `public_key_from_did_key`
    (`fe-identity/src/did_key.rs:39`) at mint and again at verify.
  - A blank client at the production mint door (the GUI's `DbCommand::MintApiToken`)
    generates a fresh client: a new ed25519 key, of which only the public DID is kept.
  - The `fe-identity --example mint_api_token` CLI stays legacy (no `client`) unless
    `--client <did:key>` or `--new-client` is passed. That keeps the e2e scripts' cross-token
    reads working, and it is the in-tree exercise of the legacy path.

  Reversal: cheap.
- **A1 — D-23 default relay unchanged.**
  - `RelayConfig::Default` stays iroh's n0 v1 fleet, with the existing one-time warning
    (`fe-sync/src/relay_config.rs:14-26`, `fe-sync/src/endpoint.rs:50-61`).
  - `FE_SYNC_RELAY` and the settings relay remain the operator switch, and W6 hardcodes no
    relay host.
  - The 2026-10-12 D-23 posture ("self-hosted container is the production default") stays
    operational. W6 ships the tested container recipe and the docs; making it the default
    waits for a named host.

  Reversal: cheap, once a host exists (set `FE_SYNC_RELAY`, or change the `Default` arm).
- **A7 — ENU applies to WKB geometry only.**
  - CSV `x_m,y_m,z_m` (`fe-api/src/export.rs:641`) and analytics `pos_x/pos_y/pos_z`
    (`fe-query/src/columnar/provider.rs:59-61`) keep the engine frame (x, y=up, z), as the
    decision names WKB only.
  - Parquet `geo` metadata gains an informational `fe:axes = "east,north,up"` key on local
    output.

  Reversal: cheap (rename and reorder the CSV columns).
- **A9 — elevated cap counting.**
  - The cap counts top-level statements as `split_elevated_statements`
    (`fe-api/src/query_guard.rs:476`) splits them. Empty statements are not counted.
  - One constant, `MAX_ELEVATED_STATEMENTS = 32`, lives in
    `fe-database/src/handlers/elevated_query.rs`. It is enforced by fe-api (fast fail) and by
    the DB handler (the authority).
  - A `FOR`/`IF` block counts as one statement. The security slice records a verdict on
    block amplification (threat EL-2).

  Reversal: cheap.
- **A10 — GPX cascade sources.**
  - The cascade input is the owning track's **lattice** hidden state: its own override Hide,
    or a hidden Group containing it. The track's ancestor chain already hides the stamp
    directly.
  - The ribbon style flag `gis.track.visible` (`fe-ui/src/actions/path.rs:259`) does NOT
    cascade. It is a render style outside the lattice
    (`fe-ui/src/visibility/mod.rs:537`).
  - Solo membership is not extended to a soloed track's stamps.

  Reversal: cheap (feed style-hidden track ids into the cascade input).
- **A12 — callout shape.**
  - Tape and Area mode.
  - The property bag stores the anchors (f64 JSON), the mode and `closed`. It never stores a
    readout; the readout is recomputed at render with the petal's current world scale
    (`fe-ui/src/node_manager/measure.rs:145`, `:180`), so it follows map-scale changes.
  - Creation follows the earthwork pattern: a correlated `CreateNode`, then
    `SetNodeProperty` writes (`fe-ui/src/actions/terrain_proposal.rs:503-541`). That is not
    atomic, so a partial bag renders nothing (the strictest reader).
  - At most 256 anchors, all finite.

  Reversal: cheap.
- **A14 — relay access token.**
  - Read only from `FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN`, the fe-identity `EnvBackend`
    naming (`fe-identity/src/secret_store/mod.rs:96-130`). Never from settings or files.
  - It is attached as `Authorization: Bearer` only to **Custom** relays
    (`RelayMap::with_auth_token`, `iroh-relay 1.3.0 src/relay_map.rs:154`), and only over
    `https://` or to a loopback `http://` host.
  - It is never stored in `RelayConfig`, which is `Debug`-logged at
    `fe-sync/src/endpoint.rs:103` and `fe-sync/src/sync_thread.rs:459`.

  Reversal: cheap (drop step 8b).
- **A13 — relay test infrastructure.**
  - The test manages its own container through the `docker` or `podman` CLI
    (`FE_CONTAINER_RUNTIME` override). The image is
    `n0computer/iroh-relay:v1.3.0@sha256:7a51e3826531dba5eee73d8b48a8f27284684d98c4d0da271d99127cc43b9d57`
    (multi-arch index, verified on Docker Hub 2026-10-15; entrypoint `/iroh-relay`).
  - The relay runs in `--dev` mode: plain HTTP, QUIC address discovery off, published on
    `127.0.0.1` only.
  - A "usable runtime" means `<rt> info` succeeds.
  - CI Linux runs the test with `FE_REQUIRE_RELAY_CONTAINER=1`. Windows CI skips it, because
    its runners cannot run Linux containers.

  Reversal: cheap.
- **A6 — outbound asset rows drop `owner_did`.** The device-payload emission sends
  `SELECT *` asset rows (`fe-database/src/handlers/device_payload.rs:659-692`, `:404`). Peers
  already discard `owner_did` on inbound rows (`fe-database/src/handlers/replicated_row.rs:771-777`),
  so the client DID never needs to leave the node. Reversal: cheap.
- **A11 — promoted-stamp persisted override.**
  - A promoted stamp's `view.state` on its node row counts as that stamp's own override.
    `hydrate_promoted_stamp` (`fe-ui/src/actions/asset.rs:376-393`) already holds both ids.
  - The resolver looks up the marker id first, then the promoted alias.

  Reversal: cheap.
- **A15 — OSS-1.3 is warn-only by design.** The licences check runs in CI, but Lint is
  warn-only (R7, `RUNBOOK.md:251-253`). A future licence violation is a `::warning::` plus a
  summary row, not a red run. That is the ratified posture, not a gap.

## Acceptance

- **TOKEN-CLIENT-CLAIM.**
  - A token minted through `DbCommand::MintApiToken` carries the requested `client`, or a
    fresh `did:key` when none was requested.
  - Two clients of one node have separate quota buckets.
  - A legacy token (no `client`) still verifies and uploads into the node-level bucket.
  - Client B cannot read or place client A's unplaced upload. A node-level upload stays
    shared within its scope.
  - A `client` naming a Manager's DID gains no role.
  - Outbound asset rows carry no `owner_did`.
  - All of the above are flow tests (plan W6-7a).
- **GPX-HIDE-CASCADE.**
  - Hiding a track hides every one of its stamps, in the apply pass and in every picker.
  - A stamp with its own Show override stays visible, and one with Hide stays hidden.
  - Table tests (plan W6-5).
- **HS-P6-CALLOUT.**
  - A completed Tape or Area measurement promotes to a `node_kind=dimension_callout` node.
    Its bag round-trips through the property store and re-renders after reload.
  - A hidden callout is not drawn.
  - Deleting the node drops the callout.
  - Plan W6-6. The hexon_scale Phase 6 callout task (`hexon_scale_orchestration_20260712/plan.md:120`)
    is ticked.
- **EGR-WKB.**
  - Local Parquet geometry decodes to `(x, -z, y)`, and latlon to `(lon, lat, ele)`.
  - The reader round-trips.
  - `scripts/bi-egress-verify.ps1` asserts `ST_X/ST_Y/ST_Z` = east/north/up through DuckDB
    spatial.
- **EGR-F32.**
  - An f64-precise position seeded in the DB comes back bit-exact from Parquet WKB, the CSV
    text and analytics `pos_*`.
  - The two latlon tolerance tests (`fe-api/tests/export_share_test.rs:400`, `:564`, both
    `1e-2`) are tightened to exact equality against the same f64 projection.
- **FEAPI-BUILDER-CRS.**
  - A freshly packaged tileset's raw meta JSON contains `"crs":"EPSG:4326"`: the single
    archive, the outer container and every chunk.
  - Asserted on the **raw JSON**. The deserialized struct cannot tell the difference,
    because serde fills `default_crs()` for an absent key (`fe-format/src/manifest.rs:72-84`).
- **FEAPI-ELEV cap.**
  - A script of 33 statements is refused with the `ok:false` envelope and writes nothing
    (empty op-log).
  - 32 is accepted.
  - The DB handler refuses 33 on its own.
- **OSS-1.3.** The ledger, `tracks.md` and the oss_release plan task 1.3 say DONE, citing
  `da3e6a1` and CI run `38498925963`'s lint log line "advisories ok, bans ok, licenses ok,
  sources ok".
- **IROH-1X.**
  - Against the local iroh-relay 1.3 container:
    - two production `SyncEndpoint`s report the container as their home relay (relay health
      Healthy);
    - a tileset HashSeq fetch completes when the dialer knows only the provider's relay URL;
    - a dialer with **no IP transports** completes the fetch, so the relay alone carried it;
    - a provider address naming an untrusted relay is never dialed;
    - a token-gated relay admits the configured token and refuses others.
  - CI Linux runs all of these as required.
  - `iroh_1_0_upgrade_20260711/plan.md:123` is ticked.
- **The gate, every slice.** The same gate as W5 (`role_propagation_20261013/spec.md:152-158`):
  - `cargo check -j4 --workspace --tests`;
  - `cargo +1.99.0 clippy -j4 --workspace --all-targets --keep-going -- -D warnings`;
  - `cargo +1.99.0 fmt --all -- --check`;
  - the feature matrix: `fe-query --features parquet`, `fe-format --no-default-features`,
    `fe-terrain --features fetch,render` and `fe-hexon --all-features`, each run as `check`
    plus `clippy --all-targets -D warnings`.

  If `gate_green` is false, there is no commit and the run halts. A review slice that
  returns `BLOCKED`, or that leaves a CRITICAL or HIGH unfixed, also halts the run (AC-52).
  No tests run until the campaign final sweep.

## OSS-1.3 resolution (the ledger contradicts itself — resolved by reading HEAD)

**DONE.** `doneness-ledger.md:29`, `:71`, `:114` and `:119` are stale. The row at `:157` is
right in substance, though two of its citations have drifted. Evidence at `389b6c7`:

- `deny.toml:1-6` (header) and `:8-28` (allow-list). The list holds the 7 outlier licences:
  five allowed globally, plus OFL-1.1 and Ubuntu-font-1.0 scoped as
  `epaint_default_fonts` exceptions at `:41`. The BUSL-1.1 `surrealdb*` exceptions are at
  `:32-40`.
- `.github/workflows/build-artifacts.yml:128-134` runs
  `cargo deny --color never --all-features check advisories bans sources licenses`.
  `git blame` puts `:128` and `:133` at `da3e6a1`, whose message says "cargo-deny licenses on
  in CI".
- CI run `38498925963`, the first fully green run, on `53a4c4c`. Its Lint log reads
  `2026-10-14T02:06:22.7678868Z advisories ok, bans ok, licenses ok, sources ok`.

The ledger's 2026-10-13 refresh read the pre-`da3e6a1` tree. That is why it cited `:88-91`,
`command: check advisories bans sources`.
