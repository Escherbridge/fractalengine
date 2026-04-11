# FractalEngine P2P Mycelium Test Harness

Standalone functional test harness for the P2P Mycelium features in
[fractalengine](../fractalengine/). Runs headlessly (no Bevy/GPU required)
and simulates multiple peers on one machine.

## What it tests

| Scenario | Peers | Description |
|---|---|---|
| Blob Store Roundtrip | 1 | Import GLB, verify blob store write, BLAKE3 hash, blob:// URL |
| Legacy Migration | 1 | Base64 decode + blob store write produces correct hash |
| Invite Flow | 2 | Alice creates verse, generates invite, Bob joins via invite |
| Verse Sync Infrastructure | 1 | OpenVerseReplica + WriteRowEntry command pipeline |
| Two-Peer Blob Exchange | 2 | Alice imports GLB, blob copied to Bob, content-addressable verify |
| Two-Peer Verse Join | 2 | Full lifecycle: create verse/fractal, invite, join, independent writes |
| Two-Peer Sync Pipeline | 2 | Both peers open replicas, exchange row blobs, close cleanly |

## Prerequisites

- Rust toolchain (stable, 1.80+)
- The `fractalengine` workspace must be checked out at `../fractalengine/`
  (sibling directory)

## Running

### All scenarios via binary

```sh
cargo run
```

Set `RUST_LOG` for verbose output:

```sh
RUST_LOG=debug cargo run
```

### Integration tests

```sh
cargo test
```

### Single scenario (via test filter)

```sh
cargo test test_blob_store_roundtrip
cargo test test_invite_token_roundtrip
```

## Architecture

Each `TestPeer` instance owns:
- An **in-memory SurrealDB** (`kv-mem` engine) -- no file locks, fully isolated
- A **FsBlobStore** in a unique temp directory
- A **sync thread** with its own iroh endpoint
- A unique **ed25519 keypair** for identity and invite signing

The test harness does NOT depend on Bevy's render pipeline. It tests the
database, blob store, sync, and identity layers directly.

## Directory layout

```
src/
  main.rs              # CLI runner: executes all scenarios
  peer.rs              # TestPeer struct: isolated fractalengine instance
  fixtures/mod.rs      # Minimal GLB generator and test data
  scenarios/
    mod.rs
    blob_roundtrip.rs            # Scenario 1: single-peer blob store
    migration.rs                 # Scenario 2: legacy base64 migration
    invite_flow.rs               # Scenario 3: two-peer invite lifecycle
    verse_sync.rs                # Scenario 4: sync command pipeline
    two_peer_blob_exchange.rs    # Scenario 5: cross-peer blob portability
    two_peer_verse_join.rs       # Scenario 6: collaborative verse with independent writes
    two_peer_sync_pipeline.rs    # Scenario 7: full two-peer sync pipeline
tests/
  integration.rs       # cargo test integration tests (10 tests)
```

## How two-peer testing works

Each `TestPeer` gets its own temp directory, in-memory DB, blob store, sync thread, and
keypair. Since there's no real network between them (iroh endpoints bind to different
ports but don't discover each other), blobs are "exchanged" by reading bytes from one
peer's blob store and writing them to the other's. This simulates the network fetch that
the sync thread would perform in production.

This approach lets you test the full data pipeline (create -> serialize -> hash -> store ->
transfer -> verify) without needing two machines or network configuration.
