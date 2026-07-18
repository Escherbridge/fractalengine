# fe-test-harness/src — harness rationale

Package `fractalengine-test-harness` has two targets: the P2P scenario runner
binary (`main.rs`, §scenarios below) and a library (`lib.rs`) exposing the
API-integration harness (§api-harness).

## §api-harness

`api.rs` — reusable in-process fe-api integration harness, consumed from
`fe-api/tests/` as a dev-dependency (`fractalengine-test-harness = { path =
"../fe-test-harness" }`; the dev-dep cycle with fe-api's normal dep is legal
cargo). Import path for test authors:

```rust
use fractalengine_test_harness::api::ApiHarness;
```

**What `ApiHarness::spawn()` builds** (no network, no Bevy, no DB thread):

- In-memory SurrealDB (`engine::local::Mem`, ns/db `test`/`test`) with the
  full schema (`fe_database::schema::apply_all` + api-token schema).
- The **real router** via `fe_api::server::build_router` over a real
  `ApiState`: `db_reader = Some(db)` (so every direct-read handler works),
  a tempdir-backed `FsBlobStore`, and a live `api_cmd_tx` whose receiver is
  held open but **never serviced** — handlers that require the
  crossbeam→DB-thread round-trip (most writes, `get_hierarchy`, …) will hang
  or 5xx; test against `db_reader`-backed handlers or seed directly.
- Requests go through `tower::ServiceExt::oneshot` (`h.request(req)`), with
  `get`/`post_json` conveniences returning `(StatusCode, lenient JSON)`.
  Raw bodies: `api::body_bytes(resp)`.

**Auth approach**: the harness generates its own `NodeKeypair`, installs its
verifying key in `ApiState`, and `mint_token(scope, role)` mints a real signed
JWT via `fe_identity::api_token::mint_api_token` (1h TTL, fresh jti) — so the
production `auth_middleware` runs unmodified: 401/403 paths are the real ones.
Scopes use the repo grammar (`VERSE#v`, `VERSE#v-FRACTAL#f-PETAL#p`);
`SeededHierarchy::verse_scope()` gives the covering verse scope.

**Seeding**: `seed_verse`/`seed_fractal`/`seed_petal`/`seed_node` (+
`seed_hierarchy` for the full chain) run the same `CREATE ... CONTENT`
statements the fe-database handlers write — including the mandatory
`<geometry<point>>` cast (fe-database/src/AGENTS.md §geometry-inserts) and
omit-when-absent optional fields. The real DbCommand dispatch loop is
quarantined inside fe-database's DB thread (SurrealKV-only, not callable
against Mem), so statement-parity is the strongest "real write path"
available in-process; `h.db` is the same handle as `state.db_reader` for
bespoke seeding/assertions. Smoke test proving the wiring end-to-end:
`fe-api/tests/api_harness_smoke.rs`. Run with
`RUST_MIN_STACK=134217728 cargo test -p fe-api --test <file>` (surrealdb-core
stack gotcha, see project memory).

Standalone binary (package `fractalengine-test-harness`) that functional-tests
the P2P Mycelium stack headlessly: each scenario spawns isolated `TestPeer`s
(own in-memory DB, blob store, sync thread, and identity — no Bevy/GPU) and
drives them via `DbCommand`/`SyncCommand` messages. `main.rs` runs every
scenario in sequence and exits non-zero on any failure; `peer.rs` owns peer
spawning and `wait_for` result matching; `fixtures/` provides minimal asset
bytes (`create_minimal_glb`).

## §scenarios

| Scenario (file) | Purpose | Gotchas |
| --- | --- | --- |
| 1 Blob Store Roundtrip (`blob_roundtrip.rs`) | `ImportGltf` writes the bytes to the blob store, returns a `blob://` asset_path, and the BLAKE3 hash in the path matches the original file bytes (store holds the exact bytes). | — |
| 2 Legacy Base64 Migration (`migration.rs`) | Legacy base64 asset → decode → blob-store write → hash/`blob://` URL construction. | Tests the migration *pattern*, not `migrate_base64_assets_to_blob_store` — that function is private to fe-database, so the scenario verifies the public blob-store API produces the same BLAKE3 hash the migration would. |
| 3 Invite Flow (`invite_flow.rs`) | Alice creates a verse and an invite string; Bob joins and his hierarchy contains the verse with the correct name. | — |
| 4 Verse Sync Infrastructure (`verse_sync.rs`) | `SyncCommand::OpenVerseReplica` + `WriteRowEntry` are accepted without errors. | Infrastructure stub: actual two-peer P2P sync requires `IrohDocsReplicator` fully wired (Phase F+); this only proves the command pipeline and sync thread process commands without panicking. |
| 5 Two-Peer Blob Exchange (`two_peer_blob_exchange.rs`) | A blob written by Alice lands in Bob's store with the same hash, identical bytes, and the same `blob://` URL — content-addressability and portability across peers. | The "network fetch" is a manual byte copy between the two stores; no transport is exercised. |
| 6 Two-Peer Verse Join (`two_peer_verse_join.rs`) | Full lifecycle: Alice's verse/fractal/petal, invite with `include_write_cap=true`, Bob joins and creates his own fractal in the joined verse. | Proves invite-based collaboration over *independent* DB state — nothing is replicated between peers. |
| 7 Two-Peer Sync Pipeline (`two_peer_sync_pipeline.rs`) | Full sync command pipeline for two peers sharing a verse: replica open, row-entry write, and close on both sides. | Bob opens his replica with the namespace_id from Alice's invite; the blob crosses via manual store-to-store copy (simulated fetch), as in scenario 5. |
| API Token Lifecycle (`api_token_flow.rs`) | Mint → list → JWT claim verification → revoke → re-list. | One file, two entry points registered as separate scenarios in `main.rs`: `run` (lifecycle) and `run_edge_cases` (empty scope, excessive TTL, double revoke, wrong JTI). |
