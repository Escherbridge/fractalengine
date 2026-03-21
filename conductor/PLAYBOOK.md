# FractalEngine Implementation Playbook

> **This file has been superseded.**
> The authoritative playbook with copy-paste prompts is at: [PLAYBOOK.md](../PLAYBOOK.md)
>
> The root PLAYBOOK.md follows the same sprint/prompt-cookbook style as plantgeo,
> with self-contained copy-paste prompts for each track, progress table, and plugin strategy.

---

<!-- ARCHIVED STRATEGY DOCUMENT BELOW — kept for reference only -->

**Strategy:** Write-first, validate-last. All 10 tracks are written in parallel waves.
No `cargo build`, `cargo test`, `cargo clippy`, or `cargo tarpaulin` until Wave 6.

---

## Dependency Map

```
Wave 1:  [T1 Seed Runtime]                                           ← foundation
Wave 2:  [T2 Root Identity] [T3 scaffold] [T4 scaffold]              ← identity + scaffolds
Wave 3:  [T3 Petal Soil] [T4 Mycelium] [T5 scaffold]                 ← data + network
Wave 4:  [T5 Bloom Renderer] [T6 Petal Gate] [T7 scaffold] [T8 scaffold]
Wave 5:  [T7 Canopy View] [T8 Fractal Mesh] [T9 Console] [T10 Shields]
Wave 6:  VALIDATION — build → lint → test → coverage → finalize
```

---

## Write-Only Guard

> Include this preamble in EVERY `/conductor:implement` and agent invocation:

```
CRITICAL WRITE-ONLY CONSTRAINTS:
1. Do NOT run: cargo build, cargo test, cargo check, cargo clippy, cargo fmt,
   cargo tarpaulin, or any command that invokes rustc.
2. Do NOT run any command that takes more than 2 seconds.
3. You MAY run: git commands, file reads, mkdir, touch, ls.
4. Write ALL source files (.rs), ALL test files, ALL Cargo.toml files.
5. Write tests as if TDD — but do not execute them.
6. If a task says "Run tests" or "Verify compilation", SKIP it. Write a comment:
   // DEFERRED TO VALIDATION PHASE
7. Mark completed tasks as [~] in plan.md, not [x].
8. Commit frequently with proper conductor commit message format.
9. If uncertain about a cross-crate type, use the expected name and add:
   // CROSS-CRATE: verify this type exists after merge
```

---

## Phase 0 — Generate Missing Track Plans

Tracks 2–10 need `plan.md` before agents can implement them. Run in 3 sub-batches.

### Sub-batch 0a (4 parallel agents)

```bash
# In Claude — send all 4 at once in one message
```

**Agent 0a-1 — Track 2 plan:**
```
/conductor:new-track
Title: Root Identity — Ed25519 Keypair, OS Keychain, JWT + did:key
Track ID: root_identity_20260321
Depends on: seed_runtime_20260321
Scope: Create fe-identity crate. Ed25519 keypair gen (ed25519-dalek 2.2, verify_strict only),
OS keychain store/load (keyring crate), JWT mint/verify (jsonwebtoken 10.3,
sub=did:key:z6Mk<multibase>, 300s max lifetime), did:key derivation from public key.
All crypto functions have unit tests for success and tampered-input rejection.
```

**Agent 0a-2 — Track 3 plan:**
```
/conductor:new-track
Title: Petal Soil — SurrealDB Schema, RBAC Permissions, Op-Log
Track ID: petal_soil_20260321
Depends on: seed_runtime_20260321, root_identity_20260321
Scope: fe-database schema. DEFINE TABLE with record-level RBAC (public/custom/admin).
Op-log: every mutation writes { lamport_clock, node_id, op_type, payload, sig }.
SurrealKV backend. SURREAL_SYNC_DATA=true. Time-travel queries via VERSION clause.
```

**Agent 0a-3 — Track 4 plan:**
```
/conductor:new-track
Title: Mycelium Network — libp2p DHT + iroh Data Transport
Track ID: mycelium_network_20260321
Depends on: seed_runtime_20260321, root_identity_20260321
Scope: Extend fe-network. libp2p 0.56 Kademlia DHT + mDNS + Noise + Yamux + QUIC.
iroh-blobs (BLAKE3-verified asset transfer), iroh-gossip (signed payloads, HyParView),
iroh-docs (CRDT world-state sync). All gossip messages carry ed25519 signature.
```

**Agent 0a-4 — Track 5 plan:**
```
/conductor:new-track
Title: Bloom Renderer — GLTF Asset Pipeline, Content Addressing, Dead-Reckoning
Track ID: bloom_renderer_20260321
Depends on: petal_soil_20260321, mycelium_network_20260321
Scope: fe-renderer crate. AssetIngester trait (GltfIngester v1, SplatIngester stub v2).
BLAKE3 content addressing. 256MB size limit. iroh-blobs integration.
DeadReckoningPredictor Bevy component (send updates on threshold only).
```

### Sub-batch 0b (3 parallel agents) — after 0a completes

**Agent 0b-1 — Track 6:**
```
/conductor:new-track
Title: Petal Gate — Auth Handshake, Session Cache, Role Enforcement
Track ID: petal_gate_20260321
Depends on: root_identity_20260321, petal_soil_20260321, mycelium_network_20260321
Scope: fe-auth crate. Session handshake (connect → JWT issue → role assign → verify → revoke).
SessionCache 60s TTL with SurrealDB re-validation. RBAC enforcement at DB layer only.
Signed revocations broadcast via iroh-gossip within 5s. Test: tampered messages, expiry, revocation.
```

**Agent 0b-2 — Track 7:**
```
/conductor:new-track
Title: Canopy View — wry WebView Overlay + BrowserInteraction Tabs
Track ID: canopy_view_20260321
Depends on: seed_runtime_20260321, bloom_renderer_20260321, petal_gate_20260321
Scope: fe-webview crate. Custom wry 0.54 Bevy plugin (NOT bevy_wry — it's abandoned).
Overlay window positioned via Camera::world_to_viewport() + EventLoopProxy.
Typed IPC (BrowserCommand enum). URL denylist (localhost, 127.0.0.1, all RFC 1918).
Non-dismissible trust bar. REQUIRED: docs/webview-threat-model.md before any code.
```

**Agent 0b-3 — Track 8:**
```
/conductor:new-track
Title: Fractal Mesh — Multi-Node Sync, Petal Replication, Offline Cache
Track ID: fractal_mesh_20260321
Depends on: petal_soil_20260321, mycelium_network_20260321, bloom_renderer_20260321
Scope: fe-sync crate. iroh-docs range reconciliation on peer connect (delta only).
Per-Petal SurrealDB namespace. LRU asset cache (2GB default, 7-day eviction).
Offline mode: load from local replica without network. Lamport clocks on all mutations.
```

### Sub-batch 0c (2 parallel agents) — after 0b completes

**Agent 0c-1 — Track 9:**
```
/conductor:new-track
Title: Gardener Console — Node Operator Admin UI
Track ID: gardener_console_20260321
Depends on: petal_soil_20260321, bloom_renderer_20260321, petal_gate_20260321, canopy_view_20260321
Scope: fe-ui crate. bevy_egui 0.39 admin dashboard. Node status, peer list, RBAC management,
asset browser (BLAKE3 IDs), role chip HUD, session monitor. Minimal & silent UI per guidelines.
```

**Agent 0c-2 — Track 10:**
```
/conductor:new-track
Title: Thorns and Shields — Security Hardening + Pre-Launch Documents
Track ID: thorns_shields_20260321
Depends on: mycelium_network_20260321, bloom_renderer_20260321, petal_gate_20260321, canopy_view_20260321
Scope: Security hardening pass. WebView threat model doc (if not from T7). Dependency audit
(cargo-audit). Fuzzing harness scaffolds (crypto, JWT parse). Security checklist doc.
Audit all gossip validation paths. Ensure no unwrap()/expect() in production paths.
Test harnesses: tampered sigs, expired JWTs, RFC 1918 bypass, oversized assets.
```

---

## Wave 1 — Foundation (1 agent, sequential)

**Dependency gate:** None (first wave)
**Branch:** main

### Pre-step: Pre-declare ALL workspace members in Cargo.toml

Before Track 1 begins implementing, the Cargo.toml must pre-declare all 10 crates to eliminate
merge conflicts in later waves. Add this to Track 1 instructions.

### Invocation

```
/conductor:implement seed_runtime_20260321
```

**With these additional instructions:**

```
WRITE-ONLY MODE. Additional requirements beyond the plan:

1. WORKSPACE SETUP — In the workspace Cargo.toml, pre-declare ALL workspace members
   (even if they don't exist yet — cargo allows this):
   [workspace]
   members = [
     "fractalengine",
     "fe-runtime",
     "fe-network",        # Wave 2
     "fe-database",       # Wave 2/3
     "fe-identity",       # Wave 2 — create empty stub now
     "fe-renderer",       # Wave 3/4
     "fe-auth",           # Wave 4
     "fe-webview",        # Wave 4/5
     "fe-sync",           # Wave 4/5
     "fe-ui",             # Wave 5
   ]
   Add [workspace.dependencies] with ALL crates pinned to versions in tech-stack.md.

2. Create stub lib.rs files for fe-identity, fe-renderer, fe-auth, fe-webview, fe-sync, fe-ui
   now (each just: //! Stub — implemented in later waves) so the workspace compiles as a unit.

3. WRITE ALL CODE for all 6 phases without running cargo. Mark tasks [~] not [x].

4. Commit per phase with message format: feat(fe-runtime): <description>
```

---

## Wave 2 — Identity + Scaffolds (3 parallel worktrees)

**Dependency gate:** Wave 1 merged to main
**Branches:** track/root-identity, track/petal-soil-scaffold, track/mycelium-scaffold

### Launch all 3 in one message using Agent tool with `isolation: "worktree"`

**Agent 2-1 (Track 2, full implementation):**

```
/conductor:implement root_identity_20260321

WRITE-ONLY MODE.
Create fe-identity crate (replace the stub from Wave 1 with real implementation).
Implement: ed25519 keypair generation and loading, OS keychain store/load via keyring crate,
JWT mint: build_session_token(peer_pub_key, petal_id, role_id, ttl) -> Result<String>,
JWT verify: verify_session_token(token, issuer_pub_key) -> Result<Claims>,
did:key derivation: pub_key_to_did_key(pub_key: &VerifyingKey) -> String.
NodeIdentity Bevy Resource wrapping the signing key handle.
NEVER expose raw SigningKey outside NodeIdentity.
Write all unit tests (success + failure cases, tampered input rejection).
Mark tasks [~]. Do NOT run cargo.
```

**Agent 2-2 (Track 3, scaffold only):**

```
SCAFFOLD ONLY — Track 3 Petal Soil. Branch from main.
In fe-database/src/: Create module files: schema.rs, rbac.rs, op_log.rs, queries.rs.
Define Rust types (fully specified, no todo bodies):
- struct PetalId(ulid::Ulid)
- struct NodeId(String) // did:key string
- struct RoleId(String)
- struct OpLogEntry { lamport_clock: u64, node_id: NodeId, op_type: OpType, payload: serde_json::Value, sig: [u8; 64] }
- enum OpType { CreatePetal, CreateRoom, PlaceModel, AssignRole, RevokeSession, DeletePetal }
Define SurrealQL schema constants as Rust &str (DEFINE TABLE petal SCHEMAFULL ...).
Write function signatures with todo!() for all query functions.
Do NOT implement SurrealDB calls. Do NOT run cargo.
Commit: feat(fe-database): Add Petal Soil scaffold types and schema constants
```

**Agent 2-3 (Track 4, scaffold only):**

```
SCAFFOLD ONLY — Track 4 Mycelium Network. Branch from main.
In fe-network/src/: Create module files: swarm.rs, discovery.rs, gossip.rs, iroh_blobs.rs, iroh_docs.rs.
Define Rust types:
- struct GossipMessage<T: Serialize> { payload: T, sig: [u8; 64], pub_key: [u8; 32] }
  // Unsigned messages are not representable by design
- struct AssetId([u8; 32]) // BLAKE3 hash
- struct PetalTopic(String) // iroh-gossip topic = petal_id
- struct IrohConfig { relay_url: Option<Url>, max_concurrent_transfers: usize }
Add to fe-network/Cargo.toml: libp2p 0.56, iroh-blobs, iroh-gossip, iroh-docs.
Write function signatures with todo!() for all network functions.
Do NOT implement networking logic. Do NOT run cargo.
Commit: feat(fe-network): Add Mycelium Network scaffold types and module structure
```

### Merge order after Wave 2

```bash
git checkout main && git merge track/root-identity
git merge track/petal-soil-scaffold   # resolve Cargo.toml conflicts if any
git merge track/mycelium-scaffold     # resolve Cargo.toml conflicts if any
```

---

## Wave 3 — Data + Network + Early Renderer (3 parallel worktrees)

**Dependency gate:** Wave 2 merged to main

**Agent 3-1 (Track 3, full implementation):**

```
/conductor:implement petal_soil_20260321

WRITE-ONLY MODE. Scaffold exists — fill in implementations.
Implement all SurrealDB query functions. Use DbHandle Bevy Resource wrapping Arc<Surreal<Db>>.
RBAC: DEFINE TABLE permissions. public → SELECT only. Admin → full CRUD.
Op-log: every mutation calls write_op_log() before applying the change.
Time-travel: implement query_petal_at_time(petal_id, timestamp) via VERSION clause.
Write integration tests for RBAC enforcement (3 scenarios: public, custom role, admin).
Mark tasks [~]. Do NOT run cargo.
```

**Agent 3-2 (Track 4, full implementation):**

```
/conductor:implement mycelium_network_20260321

WRITE-ONLY MODE. Scaffold exists — fill in implementations.
Implement libp2p Swarm: Kademlia DHT + mDNS + Noise + Yamux + QUIC transport.
Integrate into spawn_network_thread() from Track 1 (replace stub).
Implement iroh-blobs: register_asset(hash, path), fetch_asset(hash) -> bytes.
Implement iroh-gossip: subscribe_petal(petal_id), broadcast(msg: GossipMessage<T>).
ALL inbound gossip messages: verify signature FIRST, reject if invalid, log with peer pub key.
Implement iroh-docs: sync_petal_state(petal_id, peer_id) -> delta.
Write tests (mock peers via in-process channels — never live network).
Mark tasks [~]. Do NOT run cargo.
```

**Agent 3-3 (Track 5, scaffold only):**

```
SCAFFOLD ONLY — Track 5 Bloom Renderer. Branch from main.
Create fe-renderer crate (replace stub from Wave 1).
Define: trait AssetIngester { fn ingest(&self, bytes: &[u8]) -> Result<AssetId>; }
Define: struct GltfIngester; impl AssetIngester for GltfIngester { todo!() }
Define: struct SplatIngester; // v2 stub — body is todo!()
Define: #[derive(Component)] struct DeadReckoningPredictor {
  pub velocity: Vec3, pub last_network_pos: Vec3,
  pub last_network_time: f64, pub threshold: f32
}
Write Cargo.toml with bevy 0.18, blake3, iroh-blobs deps.
Function signatures with todo!() bodies for: validate_glb(), content_address(), load_to_bevy().
Do NOT implement. Do NOT run cargo.
Commit: feat(fe-renderer): Add Bloom Renderer scaffold
```

### Merge order after Wave 3

```bash
git checkout main && git merge track/petal-soil-impl
git merge track/mycelium-network-impl
git merge track/bloom-renderer-scaffold
```

---

## Wave 4 — Renderer + Gate + Late Scaffolds (4 parallel worktrees)

**Dependency gate:** Wave 3 merged to main

**Agent 4-1 (Track 5, full implementation):**

```
/conductor:implement bloom_renderer_20260321

WRITE-ONLY MODE. Scaffold exists.
Implement GltfIngester: validate file is GLB (magic bytes check), reject > MAX_ASSET_SIZE_MB (256MB),
compute BLAKE3 hash, register in SurrealDB asset table via DbHandle.
Implement content_address(bytes: &[u8]) -> AssetId using blake3 crate.
Implement load_to_bevy(asset_id: AssetId, asset_server: &AssetServer) -> Handle<Gltf>:
  load from local cache path by hash; if missing, request via iroh-blobs.
Implement dead_reckoning_system: predict Transform each frame using velocity;
  send NetworkCommand::EntityUpdate only when |actual - predicted| > threshold.
Write tests: invalid file rejection, hash roundtrip, threshold logic.
Mark tasks [~]. Do NOT run cargo.
```

**Agent 4-2 (Track 6, full implementation):**

```
/conductor:implement petal_gate_20260321

WRITE-ONLY MODE. Create fe-auth crate (replace stub).
Implement session handshake:
  connect(peer_pub_key) → issue JWT via fe-identity → assign role via fe-database → return token
Implement SessionCache: HashMap<[u8;32], (RoleId, Instant)> with 60s TTL.
  Cache miss → re-query SurrealDB for role. Cache hit → use cached role if not expired.
Implement revoke_session(peer_pub_key): write to SurrealDB, broadcast signed GossipMessage<Revocation>
  via fe-network, THEN flush SessionCache entry. Order is critical.
Public access: peer with no JWT gets public role (hardcoded, no DB lookup needed).
Write tests: tampered JWT rejection, expiry, revocation propagation, public access.
Mark tasks [~]. Do NOT run cargo.
```

**Agent 4-3 (Track 7, scaffold only):**

```
SCAFFOLD ONLY — Track 7 Canopy View. Branch from main.
Create fe-webview crate (replace stub).
Define: enum BrowserCommand { Navigate(Url), Close, GetUrl } // typed IPC — versioned
Define: enum BrowserEvent { UrlChanged(Url), LoadComplete, Error(String) }
Define: struct WebViewConfig { petal_id: PetalId, initial_url: Option<Url>, role: RoleId }
Define: trait BrowserSurface { fn show(&mut self, url: Url); fn hide(&mut self); } // abstraction
Define URL denylist constants:
  const BLOCKED_HOSTS: &[&str] = &["localhost", "127.0.0.1", "0.0.0.0"];
  const BLOCKED_RANGES: &[IpNet] = &[/* 10/8, 172.16/12, 192.168/16 */];
Write module files: plugin.rs, ipc.rs, security.rs, overlay.rs.
Function signatures with todo!() bodies. Do NOT run cargo.
Commit: feat(fe-webview): Add Canopy View scaffold
```

**Agent 4-4 (Track 8, scaffold only):**

```
SCAFFOLD ONLY — Track 8 Fractal Mesh. Branch from main.
Create fe-sync crate (replace stub).
Define: struct ReplicationConfig { max_cache_gb: f32, eviction_days: u64 }
Define: struct PetalReplica { petal_id: PetalId, last_seen: Instant, local_db_namespace: String }
Define: struct CacheEntry { asset_id: AssetId, size_bytes: u64, last_accessed: Instant }
Define: trait ReplicationStore { fn sync(&self, peer: PeerId) -> Result<Delta>; }
Define: struct IrohDocsStore; impl ReplicationStore for IrohDocsStore { todo!() }
Write module files: replication.rs, cache.rs, offline.rs, reconciliation.rs.
Function signatures with todo!() bodies. Do NOT run cargo.
Commit: feat(fe-sync): Add Fractal Mesh scaffold
```

### Merge order after Wave 4

```bash
git checkout main && git merge track/bloom-renderer-impl
git merge track/petal-gate-impl
git merge track/canopy-view-scaffold
git merge track/fractal-mesh-scaffold
```

---

## Wave 5 — Final Feature Tracks (4 parallel worktrees)

**Dependency gate:** Wave 4 merged to main

**Agent 5-1 (Track 7, full implementation):**

```
/conductor:implement canopy_view_20260321

WRITE-ONLY MODE. Scaffold exists.
FIRST: Write docs/webview-threat-model.md covering:
  SSRF, CSP, localhost/RFC1918 blocking, JS bridge lockdown, credential harvesting,
  external URL trust model, IPC enum exhaustiveness.
THEN implement: wry 0.54 Bevy plugin (struct WebViewPlugin, impl Plugin).
Overlay positioning system: compute Model AABB via Camera::world_to_viewport() each frame,
  send position/size to wry via EventLoopProxy. One WebView instance — repositions on focus change.
Typed IPC: handle all BrowserCommand variants in exhaustive match, no wildcard arm.
URL security: navigation_handler checks all URLs against denylist before loading.
Trust bar: inject non-dismissible HTML element showing domain + "External Website" badge.
Two-tab interface: Tab 1 = external URL (role-gated), Tab 2 = config panel (admin only).
Tests: denylist for every blocked range, IPC roundtrip per BrowserCommand variant.
Mark tasks [~]. Do NOT run cargo.
```

**Agent 5-2 (Track 8, full implementation):**

```
/conductor:implement fractal_mesh_20260321

WRITE-ONLY MODE. Scaffold exists.
Implement IrohDocsStore: on peer connect, exchange iroh-docs document heads for Petal namespace,
  reconcile delta (pull missing entries from peer, push our missing entries).
Implement local SurrealDB replica: each visited Petal gets its own SurrealDB namespace
  (petal_replica_{petal_id}). Replay op-log into replica on first sync.
Implement LRU cache: when cache size exceeds max_cache_gb, evict Petals
  not accessed in eviction_days, delete from both SurrealDB and disk.
Implement offline mode: if peer is unreachable, load from local namespace. Show stale indicator.
Mark tasks [~]. Do NOT run cargo.
```

**Agent 5-3 (Track 9, full implementation):**

```
/conductor:implement gardener_console_20260321

WRITE-ONLY MODE. Create fe-ui crate.
bevy_egui 0.39 admin dashboard plugin. Panels:
  - Node status: thread health (Bevy/network/DB), frame time, peer count, DHT connected
  - Petal list: name, active peer count, asset count, disk usage per Petal
  - Peer manager: connected peers, role chip per peer, session expiry, kick/ban
  - Role editor: create/rename/delete custom roles, assign permissions, assign to peer pub keys
  - Asset browser: BLAKE3 hash, size, filename, download count, delete action
  - Session monitor: JWT expiry times, revocation controls
Role chip HUD: small persistent chip in viewport corner showing current peer's role.
Click to expand compact permissions summary. Never animates.
Mark tasks [~]. Do NOT run cargo.
```

**Agent 5-4 (Track 10, full implementation):**

```
/conductor:implement thorns_shields_20260321

WRITE-ONLY MODE.
1. docs/webview-threat-model.md — write if Track 7 agent didn't (check for existence first)
2. docs/security-checklist.md — operator pre-launch checklist
3. scripts/audit.sh — runs cargo audit, reports vulnerable deps
4. Fuzzing harness stubs: fuzz/targets/jwt_parse.rs, fuzz/targets/ed25519_verify.rs
5. Security audit: grep all .rs files for unwrap()/expect() outside #[cfg(test)].
   Write report at docs/unwrap-audit.md listing every violation with file:line.
6. Audit gossip inbound paths: verify every path calls msg.verify() before any logic.
7. Write test harnesses:
   - tampered ed25519 signature → rejected
   - expired JWT → rejected
   - RFC 1918 URL in WebView → blocked
   - Asset > 256MB → rejected at ingest
   - Role cache after revocation → invalidated within TTL
Mark tasks [~]. Do NOT run cargo.
```

### Merge order after Wave 5

```bash
git checkout main && git merge track/canopy-view-impl  # T9, T10 depend on T7 types
git merge track/fractal-mesh-impl
git merge track/gardener-console-impl
git merge track/thorns-shields-impl
```

---

## Wave 6 — Validation (Sequential Stages)

**Dependency gate:** All waves merged to main.

### Stage 6.1 — First Compilation

```bash
cargo build 2>&1 | tee /tmp/fractal-build.log
```

Expect compilation errors from cross-crate type mismatches. Use `ultrawork` to fix:

```
/oh-my-claudecode:ultrawork
Fix all compilation errors in FractalEngine workspace.
Log is at /tmp/fractal-build.log.
Fix errors in dependency order:
  fe-runtime → fe-identity → fe-database → fe-network → fe-renderer →
  fe-auth → fe-webview → fe-sync → fe-ui → fractalengine binary
For each fix: commit with 'fix(<crate>): Resolve <description>'.
Goal: cargo build succeeds with zero errors.
Do NOT fix clippy warnings yet — only compilation errors.
```

### Stage 6.2 — Lint Pass

```
/oh-my-claudecode:ultrawork
Fix all formatting and lint issues in FractalEngine.
Run in order:
  1. cargo fmt --all                     ← format everything
  2. cargo clippy -- -D warnings         ← fix all warnings
Re-run until both pass clean.
Commit: 'style(workspace): Fix cargo fmt and clippy warnings'
```

### Stage 6.3 — Per-Crate Test Execution

```
/oh-my-claudecode:ralph
Execute and fix tests for FractalEngine, crate by crate in dependency order.
For each crate: run tests, diagnose failures, fix, re-run.
Order:
  cargo test -p fe-runtime
  cargo test -p fe-identity
  cargo test -p fe-database
  cargo test -p fe-network
  cargo test -p fe-renderer
  cargo test -p fe-auth
  cargo test -p fe-webview
  cargo test -p fe-sync
  cargo test -p fe-ui
  cargo test -p fractalengine
Circuit breaker: if a crate's tests fail after 3 fix attempts, flag for architectural
  review and move to next crate. Do not get stuck.
Commit per crate: 'fix(<crate>): Fix test failures — <description>'
```

### Stage 6.4 — Integration Test Suite

```
/oh-my-claudecode:ultraqa
Full integration test suite for FractalEngine.
Run: cargo test --workspace 2>&1
Required assertions:
  ✓ Thread isolation: no ambient Tokio on Bevy thread (try_current() returns Err)
  ✓ Network round-trip: NetworkCommand::Ping → NetworkEvent::Pong < 2ms
  ✓ DB round-trip: DbCommand::Ping → DbResult::Pong < 5ms
  ✓ Frame budget: no frame > 20ms under synthetic load (300 frames)
  ✓ Clean shutdown: threads exit within 1s on Shutdown command
  ✓ RBAC: public cannot write, custom role scoped, admin full access
  ✓ Crypto: tampered message rejected, verify_strict used throughout
  ✓ WebView denylist: all RFC 1918 ranges blocked, localhost blocked
  ✓ JWT: expiry enforced, revocation propagates within TTL
Report all passing/failing tests with failure root causes.
Fix failures. Re-run until all pass.
```

### Stage 6.5 — Coverage + Final Quality Gate

```
/oh-my-claudecode:ultraqa
Final quality gate. Execute ALL of the following:
  1. cargo fmt --check                                          → must pass
  2. cargo clippy -- -D warnings                               → must pass
  3. cargo test --workspace                                     → must pass
  4. cargo tarpaulin --workspace --out Html --output-dir coverage/   → report coverage per crate
  5. grep -rn 'unwrap()\|expect(' src/ --include='*.rs' | grep -v '#\[cfg(test)\]'  → must be empty
  6. grep -rn 'block_on' crates/ --include='*.rs'              → must be empty (no block_on in Bevy)
  7. grep -rn 'pub fn\|pub async fn' crates/ --include='*.rs' | grep -v '///'  → doc coverage check
Report: pass/fail for each gate. For any failure: file:line and fix required.
```

### Stage 6.6 — Plan Finalization

```
/conductor:implement --all-tracks
Update all plan.md files:
  - Change every [~] task to [x] with the commit SHA where code was written
  - Run the Phase Completion Verification protocol for each completed phase
  - Create checkpoint commits for each phase
  - Update conductor/tracks.md: change [ ] to [x] for each completed track
```

---

## Merge Conflict Protocol

The primary conflict surface is `Cargo.toml` (workspace root). Mitigated by Wave 1
pre-declaring all workspace members.

If conflicts do occur during merges:

```bash
# For Cargo.toml conflicts:
# Always keep the superset of [workspace.members] and [workspace.dependencies]
# Use the later wave's pinned versions (they have correct versions)

# For plan.md conflicts (multiple agents editing):
# Keep both sets of [~] tasks — the validation phase will reconcile

# For src/ conflicts (two agents implementing the same crate area):
# Read both versions, synthesize the better implementation
# The later wave's implementation is usually more complete
```

---

## Quick Reference — OMC Tools Used

| Wave | Tool | Purpose |
|------|------|---------|
| 0 | `/conductor:new-track` | Generate plan.md for tracks 2–10 |
| 1 | `/conductor:implement` | Sequential Track 1 implementation |
| 2–5 | `Agent tool (isolation: worktree)` | Parallel isolated track implementation |
| 2–5 | `/conductor:implement` (in agent) | Structured task execution per plan |
| 6.1 | `/oh-my-claudecode:ultrawork` | Parallel compilation error fixing |
| 6.2 | `/oh-my-claudecode:ultrawork` | Parallel lint/format fixing |
| 6.3 | `/oh-my-claudecode:ralph` | Persistent per-crate test fixing loop |
| 6.4 | `/oh-my-claudecode:ultraqa` | QA cycling — test until all pass |
| 6.5 | `/oh-my-claudecode:ultraqa` | Final quality gate coverage sweep |
| 6.6 | `/conductor:implement` | Plan finalization and checkpointing |
| Monitor | `/conductor:status` | Check progress at any point |
| Context | `/conductor:context-loader` | Load context when starting a new session |
| Review | `/conductor:code-reviewer` | Per-track code review before Wave 6 merge |

---

## Recovery Protocol

If any agent produces unusable output or a track's tests fail 3+ times:

```
# Architectural review (read-only)
/oh-my-claudecode:architect
Track <N> has failed repeatedly. Analyze the interface between <upstream> and <downstream>.
Read: conductor/tracks/<track_id>/plan.md, conductor/tech-stack.md, and the affected crate sources.
Propose interface changes. Do NOT implement — produce a recommendation document.

# Re-run the specific track
# (use a fresh worktree from main post-Wave-N-merge)
Agent tool with isolation: "worktree", repeat the track's invocation instructions.
```

---

## Status Check Command

At any point to see overall progress:

```
/conductor:status
```

---

*Playbook version: 2026-03-21 | Based on deep interview spec + 8-persona research synthesis*
