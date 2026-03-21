# FractalEngine: Systems Integration Analysis
*Generated: 2026-03-21 | Analyst: Scientist Agent (SYSTEMS persona) | Status: Greenfield Architecture Review*

---

## [OBJECTIVE]

Map the complete technical interaction graph of FractalEngine components, identify failure modes at every integration boundary, define thread ownership, and surface the top architectural risks with concrete solutions.

## [DATA] Analysis Scope

| Metric                               | Value          |
| ------------------------------------ | -------------- |
| Components analyzed                  | 7              |
| Integration points mapped            | 6              |
| Critical path integration points     | 3              |
| Total failure modes catalogued       | 28             |
| CRITICAL architectural risks         | 2              |
| HIGH architectural risks             | 2              |
| Frame budget (60fps)                 | 16.67ms        |
| Steady-state frame usage (estimated) | 12.7ms         |
| Frame headroom                       | 3.97ms (23.8%) |

---

## Component Integration Diagram

```

┌─────────────────────────────────────────────────────────────────────────────────┐
│                        FRACTALENGINE COMPONENT INTEGRATION GRAPH                │
│                               (Thread Topology View)                            │
└─────────────────────────────────────────────────────────────────────────────────┘

  OS MAIN THREAD (T1)
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  ┌─────────────────────────────┐    ┌──────────────────────────────────┐ │
  │  │      BEVY ECS SCHEDULER     │    │   wry WebView (event loop inject) │ │
  │  │  ┌───────────┐ ┌─────────┐ │    │  ┌────────────┐ ┌──────────────┐ │ │
  │  │  │  Systems  │ │ Render  │ │    │  │ WebView    │ │ JS Bridge    │ │ │
  │  │  │  (60fps)  │ │ Graph   │ │    │  │ Handle     │ │ (IPC)        │ │ │
  │  │  └─────┬─────┘ └────┬────┘ │    │  └──────┬─────┘ └──────┬───────┘ │ │
  │  │        │             │      │    │         │               │         │ │
  │  │  ┌─────▼─────────────▼────┐ │    │  ┌──────▼───────────────▼──────┐ │ │
  │  │  │  wgpu Device / Queue  │ │    │  │   Browser Process (WebKit)  │ │ │
  │  │  │  (GPU submission)     │ │    │  │   [separate OS process]     │ │ │
  │  │  └───────────────────────┘ │    │  └─────────────────────────────┘ │ │
  │  └──────────┬──────────────────┘    └──────────────────────────────────┘ │
  │             │  Resource reads                                              │
  │  ┌──────────▼──────────────────────────────────────────────────────────┐ │
  │  │              BEVY RESOURCES (shared in-ECS state)                   │ │
  │  │  Resource<P2PEventQueue> │ Resource<DbQueryQueue> │                  │ │
  │  │  Resource<SessionCache>  │ Resource<DbResultQueue>│                  │ │
  │  │  Resource<AssetServer>   │ Resource<SurrealDbHandle>                 │ │
  │  └──────────┬───────────────┬───────────────────────┬────────────────┘ │
  └─────────────┼───────────────┼───────────────────────┼──────────────────┘
                │               │                       │
     mpsc channel    async task bridge           Handle<Gltf>
     (thread-safe)   (bevy_tasks pool)           (Handle clone)
                │               │                       │
                ▼               ▼                       ▼
  ┌─────────────────┐  ┌────────────────────┐  ┌─────────────────────────┐
  │  NETWORK THREAD  │  │   DB THREAD (T3)   │  │  IO THREAD POOL (T4-TN) │
  │     (T2)         │  │                    │  │                         │
  │  ┌────────────┐  │  │  ┌──────────────┐  │  │  ┌───────────────────┐  │
  │  │ libp2p     │  │  │  │ SurrealDB    │  │  │  │  bevy_gltf        │  │
  │  │ Swarm      │  │  │  │ (embedded)   │  │  │  │  AssetLoader      │  │
  │  │ ┌────────┐ │  │  │  │ ┌──────────┐ │  │  │  │  (disk I/O +      │  │
  │  │ │Kademlia│ │  │  │  │ │ Tokio    │ │  │  │  │   parse)          │  │
  │  │ │DHT     │ │  │  │  │ │ (internal│ │  │  │  └─────────┬─────────┘  │
  │  │ └────────┘ │  │  │  │ │ runtime) │ │  │  │            │            │
  │  │ ┌────────┐ │  │  │  │ └──────────┘ │  │  │  ┌─────────▼─────────┐  │
  │  │ │Gossip  │ │  │  │  └──────────────┘  │  │  │  wgpu Buffer      │  │
  │  │ │Sub     │ │  │  │                    │  │  │  Upload (staging)  │  │
  │  │ └────────┘ │  │  │                    │  │  └───────────────────┘  │
  │  │ Tokio      │  │  │                    │  │                         │
  │  │ (dedicated)│  │  │                    │  │                         │
  │  └────────────┘  │  └────────────────────┘  └─────────────────────────┘
  └──────────────────┘
                │                       │
                └───────────────────────┘
                  GossipSub -> DB sync
                  (via Bevy mpsc bridge)

  DATA FLOW LEGEND:
  ───  synchronous call (same thread)
  ──►  channel send (cross-thread, async)
  ═══  shared Bevy Resource (ECS-owned, T1 only)
  ···  OS IPC / subprocess boundary

```

---

## Thread Ownership Table

| Thread                   | Owner                     | Runtime                            | Components                                                                  | Latency Budget        |
| ------------------------ | ------------------------- | ---------------------------------- | --------------------------------------------------------------------------- | --------------------- |
| T1: OS Main Thread       | Bevy App / winit          | Bevy Scheduler                     | ECS World, RenderGraph, wgpu Device, wry WebView (via event loop injection) | 16.67ms/frame (60fps) |
| T2: Async Network Thread | libp2p Swarm              | Tokio (dedicated runtime)          | Kademlia DHT, GossipSub, Noise, Yamux, P2P connections                      | 100ms (network ops)   |
| T3: DB Thread            | SurrealDB embedded        | Tokio (internal SurrealDB runtime) | Petal data, Sessions, Roles, Asset metadata                                 | 50ms (query ops)      |
| T4-TN: IO Thread Pool    | Bevy AsyncComputeTaskPool | Bevy task executor (smol-based)    | GLTF disk I/O, texture decode, mesh upload staging                          | 2000ms (asset load)   |
| T-wry (Linux only)       | wry WebView subprocess    | OS WebKit/Blink process            | Browser rendering (separate process on Linux via WebKitGTK)                 | 100ms (browser ops)   |

> **Key rule**: No component may block another thread's event loop. All cross-thread communication is via channels or shared Resources guarded by Mutex.

---

## Integration Points: Detailed Analysis

### IP-01: Bevy <-> libp2p (Tokio Boundary)
*Critical path: No | Latency budget: 16.67ms*

**Components**: BevyECS <-> libp2p

**Mechanism**: std::sync::mpsc (or crossbeam) channels; libp2p thread sends events into BevyChannel Resource; Bevy systems drain channel each frame

**Data Flow**: libp2p -> [SwarmEvent via mpsc::Sender] -> Bevy Resource<P2PEventQueue> -> Bevy System reads and processes

**Failure Modes**:
- Channel backpressure: libp2p floods events faster than Bevy drains at 60fps -> queue unbounded growth
- Tokio runtime panic in libp2p thread -> Bevy continues unaware; P2P silently dead
- Bevy system panics -> libp2p thread unaffected but channel fill stall
- Clock skew between Tokio and Bevy scheduler: events arrive out-of-order relative to ECS frame

### IP-02: Bevy <-> SurrealDB (Async Bridge)
*Critical path: YES | Latency budget: 50ms*

**Components**: BevyECS <-> SurrealDB

**Mechanism**: Two viable patterns: (A) bevy_tasks::AsyncComputeTaskPool::spawn_local with block_on; (B) Tokio handle stored in Resource, call block_in_place from Bevy system

**Data Flow**: Bevy System -> AsyncComputeTaskPool::spawn -> SurrealDB query (async) -> Task<T> polled -> result via bevy event or Resource update

**Failure Modes**:
- Pattern A: block_on inside Bevy system blocks the ECS scheduler thread for query duration -> frame stall if query > 16ms
- Pattern B: block_in_place requires Tokio context; calling from Bevy's non-Tokio thread panics
- SurrealDB embedded Tokio runtime conflict: if libp2p also has a Tokio runtime, two runtimes must not nest (cannot call block_on inside async context)
- Transaction isolation: concurrent Bevy systems issuing queries without transaction coordination -> dirty reads on role/session state
- SurrealDB startup failure -> Bevy App::run() returns error; no graceful degradation path in v1

### IP-03: Bevy/wgpu <-> wry WebView (Render Integration)
*Critical path: YES | Latency budget: 100ms*

**Components**: BevyECS <-> wry_webview

**Mechanism**: Two architectural options: (A) Side-by-side windows: wry WebView in separate OS window overlaid; (B) Offscreen render: wry renders to offscreen bitmap, captured as wgpu Texture, composited in Bevy's render graph

**Data Flow**: Option A: Bevy positions wry window to match 3D object screen coords each frame; Option B: wry -> [SharedMemory or dma-buf on Linux] -> wgpu Texture upload -> Bevy material

**Failure Modes**:
- Option A (overlay): Z-order fights with OS compositor; window decoration bleeds; breaks on multi-monitor; fails if wgpu uses exclusive fullscreen
- Option B (offscreen): wry offscreen API (WebViewBuilder::with_visible(false)) is experimental on some platforms; texture upload latency adds ~16ms minimum
- macOS PLATFORM CONSTRAINT: wry MUST run on the NS main thread; Bevy's winit integration also wants the main thread -> hard contention
- WebView process crash (renderer subprocess) -> no IPC recovery path in v1
- wgpu/wry sharing a GL/Metal/DX12 context is not supported; they use separate GPU contexts -> no zero-copy path
- Input event routing: wry captures OS mouse/keyboard input; Bevy's input system receives same events -> double-handling without explicit passthrough logic

### IP-04: GLTF Loader <-> Bevy ECS (Asset Streaming)
*Critical path: No | Latency budget: 2000ms*

**Components**: GLTF_Loader <-> BevyECS

**Mechanism**: Bevy AssetServer; Handle<Gltf> returned immediately; LoadState polled each frame; AssetEvent<Gltf>::Created fires when complete

**Data Flow**: UI triggers AssetServer::load(path) -> IO thread reads disk -> Gltf parsed -> Assets<Gltf> updated -> AssetEvent fired -> Bevy system spawns ECS entities

**Failure Modes**:
- Large GLB (>500MB) loaded into RAM blocks IO thread for seconds; no streaming/LOD in v1
- Malformed GLTF from untrusted peers: bevy_gltf panics on certain malformed inputs (historical CVEs in gltf-rs)
- Missing textures referenced by GLTF: AssetServer returns LoadState::Failed silently unless explicitly checked
- P2P content-addressed cache miss: model requested before peer has finished transferring -> load fails, no retry logic in v1
- Memory fragmentation: multiple large GLTF unload/reload cycles without explicit mesh drop -> wgpu buffer leaks

### IP-05: libp2p <-> SurrealDB (Gossip Sync)
*Critical path: No | Latency budget: 500ms*

**Components**: libp2p <-> SurrealDB

**Mechanism**: libp2p gossip-sub broadcasts delta updates; Bevy system receives via P2PEventQueue, issues SurrealDB upsert for received Petal state

**Data Flow**: Remote Node -> [GossipSub message] -> libp2p thread -> mpsc channel -> Bevy system -> SurrealDB upsert (async bridge)

**Failure Modes**:
- No conflict resolution: two peers update same Petal state simultaneously -> last-write-wins with no CRDT; data loss
- Gossip amplification: N nodes each re-broadcast -> O(N^2) messages with naive gossip-sub config; no message dedup TTL in v1
- SurrealDB write during sync overwhelms embedded DB on slow hardware
- Malicious gossip: forged Petal updates from unauthenticated peers; no signature verification on gossip payloads in v1

### IP-06: Auth/JWT <-> RBAC <-> Bevy Systems
*Critical path: YES | Latency budget: 1ms*

**Components**: Auth_JWT <-> RBAC

**Mechanism**: JWT verified at session creation; Role cached in Bevy Resource<SessionCache>; Bevy systems query Resource per frame for render gating

**Data Flow**: Peer connects -> JWT presented -> ed25519 verify (sync) -> SurrealDB role lookup (async) -> Role stored in Resource<SessionCache> -> all subsequent frame checks are O(1) HashMap lookup

**Failure Modes**:
- JWT expiry check requires system clock; NTP drift between peers -> valid token rejected or expired token accepted
- Role cache stale: admin revokes role in SurrealDB but cache not invalidated until next session refresh -> grace window of unchecked access
- No token rotation: long-lived JWT tokens with compromised ed25519 key -> full Petal compromise
- RBAC check bypass: Bevy system panic after role check but before action guard -> action executes without authorization

---

## Technical Deep Dives

### Q: How does Tokio (libp2p) coexist with Bevy's scheduler?

Bevy's scheduler is NOT Tokio-based. It uses its own parallel executor (derived from 
the `async-executor` crate / `bevy_tasks` with `smol` or blocking thread pools). 
Bevy systems run as synchronous functions dispatched across a thread pool — they are 
NOT Tokio futures.

libp2p requires a Tokio runtime. The correct coexistence pattern:

1. At startup, BEFORE App::run(), spawn a dedicated OS thread:
   ```rust
   std::thread::spawn(|| {
       let rt = tokio::runtime::Builder::new_multi_thread()
           .enable_all()
           .build()
           .unwrap();
       rt.block_on(run_libp2p_swarm(tx_to_bevy, rx_from_bevy));
   });
   ```

2. The libp2p thread holds a `mpsc::Sender<P2PEvent>` pointing into Bevy.
   Bevy holds a `Resource<Receiver<P2PEvent>>` (wrapped in Arc<Mutex<>>).

3. Each Bevy frame, a dedicated system drains the channel:
   ```rust
   fn drain_p2p_events(queue: Res<P2PEventQueue>, mut events: EventWriter<P2PEvent>) {
       while let Ok(event) = queue.rx.try_recv() {
           events.send(event);
       }
   }
   ```

4. Commands TO libp2p go the other direction: Bevy system writes to 
   `mpsc::Sender<P2PCommand>` held as a Resource; libp2p thread select!()-polls it.

KEY CONSTRAINT: Never call tokio::runtime::Handle::block_on() from within a Bevy 
system that is itself running on a thread that already has a Tokio context (e.g., 
if bevy_tasks ever uses Tokio — it does NOT by default, but verify).

### Q: How does wry render inside wgpu (or alongside it)?

wry and wgpu use SEPARATE GPU contexts. There is NO shared texture/surface path 
in production use. The two viable architectures are:

OPTION A — OVERLAY (RECOMMENDED FOR V1):
wry creates a child window (or transparent overlay window) positioned and sized 
to match the screen-space bounding box of the 3D Model's interaction surface, 
computed each frame from Bevy's camera projection.

Mechanism:
- Bevy system computes model's screen AABB each frame via Camera::world_to_viewport()
- Sends WindowPositionCommand to wry (via event loop user event: EventLoopProxy::send_event())
- wry updates its window position/size
- No GPU texture sharing needed; OS compositor handles layering

Problems: Z-order artifacts, breaks in VR/fullscreen, mouse events need routing.

OPTION B — OFFSCREEN RENDER (FUTURE/ADVANCED):
wry supports WebViewBuilder::with_visible(false) on some platforms. 
The rendered bitmap can be captured via IPC (shared memory on Linux, 
CoreGraphics on macOS). The bitmap is then uploaded as a wgpu Texture each frame 
and composited as a Bevy material on the Model's surface mesh.

Mechanism:
1. wry renders offscreen -> platform-specific capture (WebKitGTK IOSurface / 
   WkWebView snapshot API / OffscreenCanvas on Linux)
2. Raw RGBA bytes -> Arc<[u8]> -> wgpu::Queue::write_texture()
3. Texture bound to Bevy StandardMaterial -> rendered on Model's mesh

Problems: High latency (16-100ms capture round-trip), experimental API, 
not supported on all platforms in v1 timeframe.

CONCLUSION: V1 should use Option A (overlay). Option B is V2 after platform 
coverage is validated. The spec's "inside Bevy 3D scene" is best approximated 
by Option A with careful screen-space positioning; true in-scene compositing 
requires Option B.

### Q: How do SurrealDB async queries integrate with Bevy systems?

SurrealDB embedded mode (surrealdb::engine::local::Db) exposes an async API. 
Bevy systems are synchronous. The integration requires careful bridging:

WRONG APPROACHES:
- block_on() inside a Bevy system: blocks the ECS thread, causes frame drops
- Spawning a raw thread per query: no structured lifecycle, leaks
- Using tokio::spawn() from Bevy: Bevy does not have a Tokio runtime by default

CORRECT APPROACH — Async Task Bridge Pattern:

```rust
// Resource holding the DB handle (Arc for thread-safety)
#[derive(Resource)]
struct DbHandle(Arc<SurrealDbClient>);

// Resource holding pending task results
#[derive(Resource)]  
struct DbResults(Vec<QueryResult>);

// Bevy system: enqueue a query (non-blocking)
fn query_roles_system(
    db: Res<DbHandle>,
    mut commands: Commands,
    task_pool: Res<AsyncComputeTaskPool>,
) {
    let db = db.0.clone();
    let task = task_pool.spawn(async move {
        db.query("SELECT * FROM role WHERE petal_id = $id")
          .bind(("id", petal_id))
          .await
    });
    commands.spawn(DbQueryTask(task));
}

// Bevy system: poll task completion (non-blocking, next frame)
fn poll_db_tasks(
    mut tasks: Query<(Entity, &mut DbQueryTask)>,
    mut results: ResMut<DbResults>,
    mut commands: Commands,
) {
    for (entity, mut task) in tasks.iter_mut() {
        if let Some(result) = block_on(future::poll_once(&mut task.0)) {
            results.0.push(result);
            commands.entity(entity).despawn();
        }
    }
}
```

NOTE: bevy_tasks::AsyncComputeTaskPool uses a smol-based executor, NOT Tokio.
SurrealDB's embedded Tokio runtime is compatible because it runs on a separate 
OS thread — the task pool threads are not inside any Tokio context.

If SurrealDB's async calls internally require a Tokio context on the calling thread,
the solution is to proxy all queries through the SurrealDB's own thread via a 
oneshot channel:
  Bevy system -> mpsc::Sender<DbCommand> -> SurrealDB thread (Tokio) -> oneshot reply

### Q: Memory model for large GLTF assets shared between peers

OWNERSHIP MODEL:
Bevy's AssetServer owns all loaded GLTF assets via Assets<Gltf> resource.
Assets are reference-counted via Handle<Gltf>. When all Handles are dropped, 
the asset is deallocated (GPU buffers freed, CPU data freed).

SHARING BETWEEN PEERS (P2P Content Distribution):
Content-addressed storage: each GLB is identified by SHA-256 hash of its bytes.
This hash is gossiped alongside Model metadata (position, rotation, etc.).

Transfer path:
1. Remote peer gossips Model with gltf_hash + gltf_size_bytes
2. Receiving Node checks local cache (SurrealDB asset_cache table by hash)
3. Cache miss: request bytes via libp2p request-response protocol from host Node
4. Bytes written to local disk cache (OS temp dir / dedicated cache dir)
5. AssetServer::load(cached_path) -> Handle<Gltf> -> ECS entity spawned

MEMORY PRESSURE POINTS:
- CPU: GLB parse allocates ~2-3x file size in RAM during parse (gltf-rs intermediate)
- GPU: wgpu allocates vertex buffers + texture atlases; typical 50MB GLB -> ~150MB VRAM
- Disk: local cache grows unbounded in v1 without eviction

SHARING WITHIN A NODE (multiple peers viewing same Petal):
All peers viewing the same Petal on one Node see the SAME loaded GLTF asset 
(same Handle<Gltf> pointing to same Assets<Gltf> entry). No duplication.
GLTF is loaded once per Node, not once per connected peer.

LARGE ASSET MITIGATION:
- Pre-transfer: gossip payload includes gltf_size_bytes; receiving Node rejects 
  if > MAX_ASSET_SIZE (configurable, default 256MB)
- During transfer: libp2p request-response chunks bytes in ~64KB frames
- Post-load: LRU eviction of cached assets not accessed in > 7 days (v2)
- In v1: warn operator if VRAM headroom < 500MB before loading

---

## Critical Path Analysis

### Path: Peer Connect -> Role Resolution -> First Render
*Total: 67.77ms | Bottleneck: SurrealDB async bridge (T3->T1 context switch + query) | SLA: < 200ms (acceptable for connect flow)*

| Step                                 | Thread | Est. Time |
| ------------------------------------ | ------ | --------- |
| Peer TCP connect via libp2p          | T2     | 20ms      |
| JWT presented + ed25519 verify       | T1     | 1ms       |
| SurrealDB role lookup (async bridge) | T3->T1 | 30ms      |
| SessionCache write to Resource       | T1     | 0.1ms     |
| First Bevy frame with correct RBAC   | T1     | 16.67ms   |

### Path: Model Click -> WebView Visible
*Total: 255.6ms | Bottleneck: External URL network fetch (uncontrollable) | SLA: < 500ms to first paint (acceptable; matches browser norms)*

| Step                                    | Thread          | Est. Time |
| --------------------------------------- | --------------- | --------- |
| Bevy raycast hit detection              | T1              | 0.5ms     |
| RBAC check (SessionCache lookup)        | T1              | 0.1ms     |
| wry WebView show/navigate call          | T1 (event loop) | 5ms       |
| Browser DNS + HTTP fetch (external URL) | T-wry           | 200ms     |
| First render of browser content         | T-wry           | 50ms      |

### Path: GLTF Upload -> In-World Visible
*Total: 2951ms | Bottleneck: Remote peer P2P transfer (network speed dependent) | SLA: Local: < 1s; Remote peers: < 30s (P2P transfer bound)*

| Step                                    | Thread | Est. Time |
| --------------------------------------- | ------ | --------- |
| File picker -> bytes read from disk     | T4     | 500ms     |
| bevy_gltf parse + validate              | T4     | 300ms     |
| wgpu mesh/texture upload (GPU staging)  | T1     | 50ms      |
| Bevy ECS entity spawn + transform set   | T1     | 1ms       |
| Gossip broadcast to peers               | T2     | 100ms     |
| Remote peer GLTF fetch via P2P transfer | T2     | 2000ms    |

### Path: Frame Render (Steady State, 60fps)
*Total: 12.7ms | Bottleneck: Render graph (wgpu GPU submission) | SLA: < 16.67ms (60fps budget); 4ms headroom*

| Step                                   | Thread | Est. Time |
| -------------------------------------- | ------ | --------- |
| Bevy system update (ECS tick)          | T1     | 2ms       |
| RBAC guard checks (all visible models) | T1     | 0.5ms     |
| Render graph execution (wgpu)          | T1     | 8ms       |
| wgpu present (swapchain)               | T1     | 2ms       |
| P2P event drain from channel           | T1     | 0.2ms     |

---

## [FINDING] Key Quantitative Results

**[FINDING 1]** Steady-state render frame uses 12.7ms of the 16.67ms budget, leaving 3.97ms (23.8%) headroom.
[STAT:effect_size] Headroom ratio = 23.8%
[STAT:n] Composed from 5 steady-state pipeline stages

**[FINDING 2]** Peer connect-to-first-render latency is dominated by SurrealDB async bridge (44% of 68ms total).
[STAT:effect_size] DB bridge contributes 44% of connect latency
[STAT:n] 5 sequential steps in connect critical path

**[FINDING 3]** 28 discrete failure modes identified across 6 integration points; 3 integration points are on the render-critical path.
[STAT:n] n=28 failure modes, n=6 integration points
[STAT:effect_size] 50% of IPs are render-critical (3 of 6)

---

## Top 5 Architectural Risks with Solutions

### Risk 1: [CRITICAL] Two Tokio Runtimes (libp2p + SurrealDB embedded) Cannot Nest
*Affected integration points: IP-01, IP-02*

**Description**: SurrealDB embedded mode internally creates a Tokio runtime. libp2p requires its own Tokio runtime. Rust panics if you call block_on inside an existing async context (cannot nest runtimes). If both use the same thread or either calls into the other's runtime, the process panics.

**Solution**: Isolate runtimes on separate OS threads. Use tokio::runtime::Builder::new_multi_thread() for libp2p on Thread-2. Let SurrealDB use its internal runtime on Thread-3. Bevy on Thread-1 (main). Cross-thread communication ONLY via std::sync::mpsc or tokio::sync::mpsc with thread-safe senders. Never block_on inside either async context.

### Risk 2: [CRITICAL] wry + Bevy Main Thread Ownership Conflict (macOS/Windows)
*Affected integration points: IP-03*

**Description**: wry (via tao/winit) requires UI components on the OS main thread on macOS (AppKit constraint) and Windows (COM STA). Bevy's App::run() also consumes the main thread via winit event loop. Both frameworks fighting for main thread ownership causes a deadlock or panic at startup.

**Solution**: Architecture: Run Bevy's winit event loop on the main thread. Integrate wry via EventLoopExtUnix / EventLoopBuilder with user events to inject WebView creation onto the main thread event queue. Use WebViewBuilder::with_new_window() (Option A) for overlay positioning, managed entirely within Bevy's winit event loop callback. On macOS, use dispatch_main() bridging. Do NOT run wry on a background thread.

### Risk 3: [HIGH] Frame Budget Violation from Synchronous DB Queries
*Affected integration points: IP-02, IP-06*

**Description**: Any synchronous (blocking) call to SurrealDB from inside a Bevy system will stall the ECS scheduler. At 60fps, each frame has 16.67ms budget. A SurrealDB query taking 20-50ms causes frame drops and render stutter. This is especially acute during Petal load (many queries) and role resolution on peer connect.

**Solution**: Never query SurrealDB from inside a synchronous Bevy system. Pattern: (1) Bevy system enqueues query request into Resource<DbQueryQueue>. (2) Dedicated bevy_tasks::AsyncComputeTaskPool task processes queue asynchronously. (3) Results written back to Resource<DbResultQueue>. (4) Next frame's Bevy system reads results. This adds 1-frame latency on DB results but never stalls the render thread.

### Risk 4: [HIGH] GLTF Memory Pressure from Large Untrusted Peer Assets
*Affected integration points: IP-04*

**Description**: GLTF/GLB models received from P2P peers are loaded into RAM and uploaded to wgpu GPU buffers. A malicious or careless peer can publish a 2GB GLB. Bevy's AssetServer has no size limit guard. Multiple peers sharing the same Petal could collectively exhaust VRAM and system RAM. bevy_gltf also historically panicked on malformed inputs.

**Solution**: Implement a content-addressed asset registry with pre-download size metadata (stored in gossip payload). Enforce a configurable MAX_ASSET_SIZE_MB limit before initiating transfer. Wrap bevy_gltf load in a catch_unwind equivalent (spawn as isolated task). Implement LRU eviction for cached peer assets. Pin max simultaneous GLTF loads to N=2.

### Risk 5: [MEDIUM] Role Cache Staleness Enables Unauthorized Access Window
*Affected integration points: IP-05, IP-06*

**Description**: RBAC role checks use a Bevy Resource<SessionCache> populated at session creation. If an admin revokes a peer's role in SurrealDB, the in-memory cache is not immediately invalidated. The peer retains access until the next session refresh (JWT expiry or explicit re-auth). In a P2P context, the host Node must also propagate revocation to all caching Nodes via gossip — adding further latency.

**Solution**: Implement a max cache TTL (e.g., 60 seconds) with mandatory re-validation against SurrealDB. On admin revoke action: (1) Write to local SurrealDB immediately, (2) Broadcast signed revocation event via gossip-sub with ed25519 signature for authenticity, (3) All nodes receiving the event flush their SessionCache entry for that peer's pub_key. JWT lifetime should be <= cache TTL to prevent token reuse after revocation.

---

## [LIMITATION]

- All latency estimates are architectural approximations derived from known library characteristics, not empirical profiling of running code (greenfield project, no executable exists yet).
- wry offscreen rendering API stability is platform-dependent; Linux/WebKitGTK support is less mature than macOS/WKWebView. Must be empirically validated per OS target.
- SurrealDB embedded Tokio runtime behavior under concurrent Bevy task pool load has not been benchmarked; query latency estimates assume lightly loaded embedded DB.
- P2P transfer speeds (GLTF distribution) depend entirely on network conditions between peers; estimates assume LAN or decent broadband.
- Frame budget estimates assume a mid-range GPU (RTX 3060 class); lower-end hardware will have less wgpu headroom.
- No conflict resolution analysis for concurrent Petal edits (explicitly deferred per spec: no real-time collaborative editing in v1).
- Role cache TTL recommendation (60s) is a starting heuristic; optimal TTL requires empirical measurement of SurrealDB query latency under load.

---

*Report generated by Scientist Agent (SYSTEMS persona) | FractalEngine Architecture Analysis | 2026-03-21*