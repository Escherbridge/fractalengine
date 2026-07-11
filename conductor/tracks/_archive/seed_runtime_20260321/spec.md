# Track Spec: Seed Runtime

## Overview

Seed Runtime establishes the foundational concurrency architecture of FractalEngine. Every subsequent track depends on the thread topology defined here. The goal is to prove — with a running prototype and integration tests — that Bevy's ECS render loop, a libp2p/iroh network thread, and a SurrealDB database thread can coexist in a single process without any thread blocking the Bevy render loop's 16.67ms frame budget.

This track produces no user-visible features. It produces the skeleton that all features are built on.

---

## Problem Statement

FractalEngine requires three distinct async runtimes in one process:

1. **Bevy ECS** — uses its own `smol`-based `AsyncComputeTaskPool`. Cannot use Tokio directly.
2. **libp2p + iroh** — hard-depend on Tokio. Must run in a dedicated OS thread with an isolated Tokio runtime.
3. **SurrealDB embedded** — internally uses Tokio. Must run in a second dedicated OS thread with its own isolated Tokio runtime.

If any of these runtimes interact on the same thread, or if `block_on()` is called from a Bevy system, the process either panics ("Cannot start a runtime from within a runtime") or stalls the render loop with frame drops.

The solution is strict OS-thread isolation with typed `mpsc` channel bridges between threads.

---

## Architecture

### Thread Topology

```
T1: OS Main Thread
  ├── Bevy App (winit event loop + ECS scheduler @ 60fps)
  ├── wry WebView (injected into winit event loop — Track 7)
  └── Channel drain systems (non-blocking try_recv each frame)

T2: Network Thread (dedicated OS thread)
  ├── Tokio runtime (multi-thread)
  ├── libp2p Swarm stub (Kademlia DHT + Gossipsub — Track 4)
  └── iroh endpoint stub (iroh-blobs + iroh-gossip + iroh-docs — Track 4)

T3: Database Thread (dedicated OS thread)
  ├── Tokio runtime (multi-thread)
  └── SurrealDB embedded with SurrealKV backend

T4-TN: IO Thread Pool
  └── bevy_tasks AsyncComputeTaskPool (GLTF loading — Track 5)
```

### Channel Bridges

```
T1 (Bevy)  ──NetworkCommand──►  T2 (Network)
T2 (Network) ──NetworkEvent──►  T1 (Bevy)

T1 (Bevy)  ──DbCommand──►  T3 (Database)
T3 (Database) ──DbResult──►  T1 (Bevy)
```

All channels are `crossbeam::bounded(N)` or `std::sync::mpsc`. No `Arc<Mutex<T>>` as a communication primitive.

---

## Deliverables

1. **Cargo workspace** with `fractalengine` binary crate and library crates:
   - `fe-runtime` — thread spawning, channel definitions, app startup
   - `fe-network` — network thread stub (Tokio runtime + placeholder Swarm)
   - `fe-database` — database thread stub (Tokio runtime + SurrealDB connection)

2. **Typed message enums** in `fe-runtime`:
   - `NetworkCommand` — commands from Bevy → network thread
   - `NetworkEvent` — events from network thread → Bevy
   - `DbCommand` — commands from Bevy → database thread
   - `DbResult` — results from database thread → Bevy

3. **Bevy channel drain systems**:
   - `drain_network_events` system — non-blocking `try_recv()` each frame, emits `NetworkEvent` into Bevy's `EventWriter`
   - `drain_db_results` system — non-blocking `try_recv()` each frame, emits `DbResult` into Bevy's `EventWriter`

4. **SurrealDB connection** in T3:
   - Embedded `SurrealKV` backend
   - `SURREAL_SYNC_DATA=true` enforced
   - Responds to `DbCommand::Ping` with `DbResult::Pong`

5. **Network thread stub** in T2:
   - Isolated Tokio runtime
   - Responds to `NetworkCommand::Ping` with `NetworkEvent::Pong`
   - Placeholder for libp2p Swarm (added in Track 4)

6. **Integration test**: Round-trip message latency from Bevy system → T2 → Bevy system is measured and asserted under 2ms at 60fps steady state.

7. **Frame budget validation**: A Bevy system measures frame time each tick. The test asserts that no frame exceeds 20ms (16.67ms budget + 20% headroom) during a 5-second run with all three threads active and synthetic channel traffic.

---

## Acceptance Criteria

- [ ] `cargo build` succeeds with zero warnings (`-D warnings`)
- [ ] `cargo test` passes all unit and integration tests
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] Three threads are alive and channels are bidirectionally functional within 2 seconds of app launch
- [ ] `NetworkCommand::Ping` → `NetworkEvent::Pong` round-trip completes in under 2ms (measured in integration test)
- [ ] `DbCommand::Ping` → `DbResult::Pong` round-trip completes in under 5ms (SurrealDB query latency)
- [ ] No frame exceeds 20ms during 5-second synthetic load test
- [ ] SurrealDB starts with `SurrealKV` backend and `SURREAL_SYNC_DATA=true` (verified by checking SurrealDB startup log)
- [ ] No `unwrap()` or `expect()` in production code paths
- [ ] No `block_on()` calls inside any Bevy system

---

## Out of Scope

- libp2p Kademlia DHT (Track 4)
- iroh endpoint (Track 4)
- SurrealDB schema (Track 3)
- RBAC (Track 3)
- Any user-visible UI (Track 9)
- WebView (Track 7)
- Identity/keypair (Track 2)

---

## Key Risks

| Risk | Mitigation |
|---|---|
| SurrealDB's internal Tokio runtime conflicts with libp2p's Tokio runtime if they share a thread | Strictly isolated OS threads — T2 and T3 each have their own Tokio `Runtime::new()` call; verified by asserting `Handle::try_current()` returns `Err` on the main thread |
| Channel backpressure causes frame stalls | All channels are bounded; `try_recv()` never blocks; overflow is dropped and logged |
| SurrealKV instability under write load | Track 1 only uses `DbCommand::Ping` — no writes. SurrealKV stability under write load is validated in Track 3. |
