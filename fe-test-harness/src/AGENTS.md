# fe-test-harness/src — harness rationale

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
