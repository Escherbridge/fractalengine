---
type: Implementation Plan
title: "Implementation Plan: iroh 0.35 → 1.x"
tags: [iroh_1_0_upgrade_20260711]
timestamp: 2026-10-11T00:00:00Z
resource: ./spec.md
---

# Implementation Plan: iroh 0.35 → 1.x

## Overview

Ported 2026-10-11 as three serial slices: (a) manifests plus the type-only crates,
(b) fe-sync core plus SYNC-RELAYWATCH, (c) workspace fallout plus docs. The slices did
not build or test themselves; the orchestrator runs one serial test sweep at the end.
Everything below is **uncommitted** in the working tree on `main` (base `4b30d17`).

Pinned stack: iroh **1.3** · iroh-docs **0.101** · iroh-gossip **0.101** · iroh-blobs
**0.103.1** (0.103.0 is yanked) · noq-proto **1.3** (replaces iroh-quinn-proto 0.13) ·
`redb_v2` = redb **2.6.4** (used only for the migration). quic-rpc and iroh-io were removed.
Workspace MSRV went from 1.83 to **1.91**, which iroh 1.3 and its sibling crates require.
Local 1.94 and CI stable 1.99 both satisfy it. Research is under `.omc/research/iroh1-*-2026-10-11.md`.

## Decisions (2026-10-11)

- **Flag-day rollout.** 0.35 and 1.x are wire-incompatible: the relay protocol, QUIC
  multipath, noq, and gossip ALPN `/iroh-gossip/0`→`/1` all changed. Every peer and relay
  node upgrades together. iroh v1 promises wire compatibility across all its minor versions.
- **Endpoint preset `presets::Minimal`**, not `N0`. This keeps 0.35's behavior: no
  pkarr/DNS publishing to n0.
- **The store is migrated, not reset.** A pre-open redb v2→v3 file-format upgrade runs
  first; iroh-docs' `redb-v2-migration` then rewrites the tuple tables. The original
  directory is copied to a backup that is never deleted.
- **Relay health starts `Unknown` at bind.** A 1.x bind contacts no relay, so health only
  becomes `Healthy` when the home-relay watcher sees a connection.
- **Bootstrap JSON** is now `EndpointAddr`, and the legacy 0.35 `NodeAddr` shape is still
  parsed.

## Phase 0: 0.35 store fixture (written before any version bump)

- [x] Task: Commit a 0.35-written `data/p2p` fixture plus `expected.json`.
      — Evidence: `fe-sync/tests/fixtures/p2p_store_v035/` (docs.redb, blobs/blobs.db,
      blobs/data/f6afa29c….data holding a 40 KiB row, default-author). It was written by a
      real iroh 0.35 build through the standalone non-member generator
      `fe-sync/tests/fixtures/p2p_store_v035_gen/` (SecretKey [7;32], NamespaceSecret
      [23;32], with a `Doc::del` tombstone). A `.gitignore` negation keeps `blobs/data/`
      trackable. The fixture is ~7.1 MB, mostly preallocated redb pages; whether to commit
      it, use LFS, or leave it out is open (see below).

## Phase 1: Manifests + type-only crates (slice a)

- [x] Task: Version bumps, MSRV 1.91, noq-proto lockstep, drop quic-rpc/iroh-io/
      iroh-quinn-proto, add `redb_v2`.
      — Evidence: root `Cargo.toml` (workspace deps + `rust-version`), `fe-sync/Cargo.toml`,
      `Cargo.lock`, `.clippy.toml` `msrv = "1.91"`, plus the BUILDING.md/CONTRIBUTING.md/
      README.md toolchain lines. fe-sim `NodeId`/`NodeAddr` → `EndpointId`/`EndpointAddr`
      (type-only, `fe-sim/src/net.rs`). fe-identity and fe-database changed comments only.
      fe-network has no iroh dependency.

## Phase 2: fe-sync core (slice b)

- [x] Task: Port endpoint, relay config, docs stack, replicator, sync thread, gossip and
      distributed query to the 1.x APIs, with the invariants preserved.
      — Evidence: `fe-sync/src/{endpoint,relay_config,docs_engine,replicator,sync_thread,
      distributed_query,virtual_transport,messages,status}.rs`, plus the API map in
      `fe-sync/src/AGENTS.md` §iroh-1.x. BBR is kept as `noq_proto::congestion::Bbr3Config`
      (§congestion-control; test `endpoint::tests::bbr_transport_config_constructs`).
      Address book: `SyncEndpoint::add_peer_addr` (MemoryLookup) still runs before the
      gossip join (§gossip-bootstrap). Gossip subscribe is now async but is still awaited
      before the doc open. The F23 `is_direct()` gate input is unchanged. The F20 identity
      alignment is unchanged (`author_import` + `author_set_default` of the endpoint key).
      ALPN test: `docs_engine::tests::alpn_constants_match_iroh_1x_wire_protocols`.
      Bootstrap parsing is covered by `sync_thread::tests::parse_bootstrap_peer_*` (new,
      legacy and bare-id shapes).
- [x] Task: Migrate 0.35 stores (`fe-sync/src/p2p_store_migration.rs`, §store-migration).
      — Evidence: marker, backup copy and loud failure (→ offline/mock). Tests:
      `legacy_v035_fixture_migrates_and_reads_back`,
      `fresh_dir_gets_marker_and_second_run_is_a_no_op`,
      `unrecognized_marker_fails_loudly`.

## Phase 3: SYNC-RELAYWATCH (rides the port)

- [x] Task: Replace the `home_relay()` TODO with a continuous watcher.
      — Evidence: `sync_thread.rs` adds a `select!` branch on
      `SyncEndpoint::home_relay_status().stream()`. It is skipped under Disabled, virtual or
      offline. `apply_relay_status` → `fold_relay_health` emits `RelayHealthChanged` only on
      change. The mapping `RelayHealth::on_relay_status` is in `relay_config.rs`. Tests:
      `relay_config::tests::on_relay_status_transition_table` and
      `sync_thread::tests::relay_watch_drives_health_into_sync_status` (Unknown → Healthy →
      Degraded → Healthy reaches `SyncStatus` through `drain_sync_events`; an unchanged
      snapshot emits nothing).

## Phase 4: Workspace fallout + docs (slice c)

- [x] Task: Make the whole workspace compile and pass clippy, and update docs.
      — Evidence: the only non-fe-sync source changes are lints the MSRV bump unlocked:
      `fe-terrain/src/terrain_plugin.rs` and `fractalengine/src/gpx_bridge.rs`
      (`chunks_exact(3)` → `as_chunks::<3>()`), and `fe-api/tests/mcp_integration.rs`
      (`% 2 == 0` → `is_multiple_of(2)`). Comment renames went into fe-test-harness
      (`peer.rs`, `two_peer_replica_sync.rs`). fe-api, fe-ui, fractalengine and
      fractalengine-relay needed no API changes. Docs updated: `fractalengine-relay/README.md`
      (EndpointAddr bootstrap format with a legacy example, n0 v1 relays rate-limited,
      self-host `iroh-relay`, flag day, store migration), root `README.md` and `AGENTS.md`,
      `SECURITY.md`, `conductor/{tech-stack,roadmap,tracks,RUNBOOK,doneness-ledger}.md`,
      `docs/diagrams/10-crate-dependencies.md`, and `fe-sim`/`fe-test-harness` AGENTS.md.
      `deny.toml` lost six ignores (hickory-proto 0.25 / rustls-webpki 0.102 via iroh 0.35)
      because cargo-deny now reports them "advisory-not-detected".
      Gates run 2026-10-11: `cargo check -j4 --workspace --tests` (1.94) EXIT 0, no
      warnings. `cargo +1.99.0 clippy -j4 --workspace --all-targets --keep-going -- -D
      warnings` EXIT 0. `cargo +1.99.0 fmt --all -- --check` clean. `cargo deny --workspace
      check advisories bans sources` EXIT 0; its only warning is a yanked `spin`, which
      predates the port (via bevy_platform/heapless).

## Open (acceptance not yet met)

- [ ] Task: Orchestrator test sweep. It must include `cargo test -p fe-sync` (the fixture
      migration test is the real proof that 0.35 stores open on 1.x), `-p fe-sim` and
      `-p fe-identity`, plus the fe-test-harness real-loopback scenarios
      (`two_peer_replica_sync`, `two_peer_tombstone_sync`, `distributed_query`,
      `two_peer_shard_fabric`, `api_db_sync_cross_thread`). Watch for an FsStore drop on
      the current-thread runtime and for the async-subscribe ordering against the
      Join-drop race.
- [ ] Task: Spec acceptance criterion: relay-assisted connection verified against a 1.x
      relay. This needs an `iroh-relay` 1.3 server (feature `server`, dev mode or TLS),
      for example as an optional `docker/compose.dev.yml` service, and two peers with
      `FE_SYNC_RELAY=http(s)://<relay>` whose direct IP paths are unusable. Not built yet.
- [ ] Task: Flag-day rollout, then commit/push (user-gated). Upgrade every deployed peer
      and relay together before 2026-12-31. Decide fixture packaging (commit / LFS / omit).
      Let the macOS CI run show whether the `socket2` shim (`Cargo.toml`, `TODO(iroh-1.x)`)
      can be dropped.
