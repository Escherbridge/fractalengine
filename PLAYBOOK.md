# FractalEngine Implementation Playbook

> Copy-paste prompts for oh-my-claudecode plugins. Write-first, validate-last.
> No `cargo build`, `cargo test`, `cargo clippy`, or `cargo tarpaulin` until Wave 6.
> Developer reviews and commits manually after each sprint.

## Progress

| Sprint | Track | Plugin | Status |
|--------|-------|--------|--------|
| 1 | Track 01 — Seed Runtime | `/ultrawork` | ✅ Done |
| 2A | Track 02 — Root Identity | `/ultrawork` (worktree) | ✅ Done |
| 2B | Track 03 — Petal Soil scaffold | `/ultrawork` (worktree) | ✅ Done |
| 2C | Track 04 — Mycelium scaffold | `/ultrawork` (worktree) | ✅ Done |
| Post-Sprint 2 | Merge 2A/2B/2C → main | manual | ✅ Done |
| 3A | Track 03 — Petal Soil (full) | `/ultrawork` (worktree) | ✅ Done |
| 3B | Track 04 — Mycelium Network (full) | `/ultrawork` (worktree) | ✅ Done |
| 3C | Track 05 — Bloom Renderer scaffold | `/ultrawork` (worktree) | ✅ Done |
| Post-Sprint 3 | Merge 3A/3B/3C → main | manual | ✅ Done |
| 4A | Track 05 — Bloom Renderer (full) | `/ultrawork` (worktree) | ✅ Done |
| 4B | Track 06 — Petal Gate (full) | `/ultrawork` (worktree) | ✅ Done |
| 4C | Track 07 — Canopy View scaffold | `/ultrawork` (worktree) | ✅ Done |
| 4D | Track 08 — Fractal Mesh scaffold | `/ultrawork` (worktree) | ✅ Done |
| Post-Sprint 4 | Merge 4A/4B/4C/4D → main | manual | ✅ Done |
| 5A | Track 07 — Canopy View (full) | `/ultrawork` (worktree) | ✅ Done |
| 5B | Track 08 — Fractal Mesh (full) | `/ultrawork` (worktree) | ✅ Done |
| 5C | Track 09 — Gardener Console | `/ultrawork` (worktree) | ✅ Done |
| 5D | Track 10 — Thorns & Shields | `/ultrawork` (worktree) | ✅ Done |
| Post-Sprint 5 | Merge all → main | manual | ✅ Done |
| Wave 6.1 | Build Fix | `/ultrawork` | ✅ Done |
| Wave 6.2 | Lint Pass | `/ultrawork` | ✅ Done |
| Wave 6.3 | Per-Crate Tests | `/ralph` | ✅ Done |
| Wave 6.4 | Integration Tests | `/ultraqa` | ✅ Done |
| Wave 6.5 | Coverage + Quality Gate | `/ultraqa` | ⬜ Pending |

---

## Plugin Strategy

| Plugin | When to Use |
|--------|------------|
| `/ultrawork` | Primary workhorse — parallel file writing across multiple agents |
| `/ralph` | Persistence loop — keep fixing until tests pass |
| `/ultraqa` | QA cycling — test, diagnose, fix, repeat until all pass |
| `/conductor:implement` | Alternative to ultrawork when you want conductor's task tracking |
| `/oh-my-claudecode:architect` | Read-only architectural review when a track fails repeatedly |

---

## Rules for All Prompts

Add this suffix to EVERY prompt:

```
RULES:
- Write code only. Do NOT run: cargo build, cargo test, cargo check, cargo clippy,
  cargo fmt, cargo tarpaulin, or any command that invokes rustc.
- Do NOT run any command that takes more than 2 seconds.
- Do NOT create git commits.
- Do NOT add doc comments (///) or inline comments (//) unless the logic is
  genuinely non-obvious (complex algorithm, safety invariant, etc.).
- Do NOT add error handling beyond what the spec requires.
- Mark each completed plan task: change `- [ ]` to `- [~]` (tilde = written, not validated).
- If a task says "Run tests", "Verify compilation", or "cargo ...": SKIP it. Write:
  // DEFERRED TO WAVE 6 VALIDATION
- If uncertain about a cross-crate type, use the expected type name and add:
  // CROSS-CRATE: verify this type exists after Wave 1 merge
- Update PLAYBOOK.md progress table: change ⬜ Pending to ✅ Done when all phases complete.
```

---

## SPRINT 1 — Foundation (Track 01: Seed Runtime)

**Plugin:** `/ultrawork`
**Branch:** main (sequential — no worktree needed)

```
Implement Track 01: Seed Runtime for FractalEngine — a Rust Cargo workspace establishing
the three-thread topology (Bevy main thread, libp2p/iroh network thread, SurrealDB database
thread) with typed crossbeam channel bridges between them.

Read the full plan at: conductor/tracks/seed_runtime_20260321/plan.md
Read the spec at: conductor/tracks/seed_runtime_20260321/spec.md

IMPLEMENT ALL 6 PHASES:

Phase 1 — Cargo Workspace and Crate Skeleton:
- Create Cargo.toml at workspace root:
  [workspace]
  members = [
    "fractalengine",
    "fe-runtime",
    "fe-network",
    "fe-database",
    "fe-identity",    # stub — implemented Sprint 2A
    "fe-renderer",    # stub — implemented Sprint 3C/4A
    "fe-auth",        # stub — implemented Sprint 4B
    "fe-webview",     # stub — implemented Sprint 4C/5A
    "fe-sync",        # stub — implemented Sprint 4D/5B
    "fe-ui",          # stub — implemented Sprint 5C
  ]
  resolver = "2"

  [workspace.dependencies]
  bevy = { version = "0.18", features = ["dynamic_linking"] }
  bevy_egui = "0.39"
  crossbeam = "0.8"
  tokio = { version = "1", features = ["full"] }
  surrealdb = { version = "3.0", features = ["kv-surrealkv"] }
  libp2p = { version = "0.56", features = ["kad", "mdns", "noise", "yamux", "quic"] }
  iroh-blobs = "0.30"
  iroh-gossip = "0.30"
  iroh-docs = "0.30"
  ed25519-dalek = { version = "2.2", features = ["rand_core"] }
  jsonwebtoken = "10.3"
  keyring = "3"
  blake3 = "1"
  tracing = "0.1"
  tracing-subscriber = { version = "0.3", features = ["env-filter"] }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  anyhow = "1"
  thiserror = "1"
  ulid = "1"

- Create fractalengine/Cargo.toml (binary crate):
  [package] name = "fractalengine" version = "0.1.0"
  [dependencies] fe-runtime = { path = "../fe-runtime" }
  bevy, crossbeam, tracing, tracing-subscriber from workspace

- Create fe-runtime/Cargo.toml: bevy, crossbeam, tracing from workspace
- Create fe-network/Cargo.toml: tokio, tracing from workspace
- Create fe-database/Cargo.toml: surrealdb, tokio, tracing from workspace
  [env] SURREAL_SYNC_DATA = "true"

- Create stub lib.rs for ALL crates not implemented this sprint
  (fe-identity, fe-renderer, fe-auth, fe-webview, fe-sync, fe-ui):
  //! Stub crate — implemented in later sprints.
  #![allow(dead_code)]

- Create rustfmt.toml: max_width = 100
- Create .clippy.toml: msrv = "1.82" (or current stable)

Phase 2 — Typed Channel Message Definitions:
- In fe-runtime/src/messages.rs, define:
  #[derive(Debug, Clone)] pub enum NetworkCommand { Ping, Shutdown }
  #[derive(Debug, Clone)] pub enum NetworkEvent { Pong, Started, Stopped }
  #[derive(Debug, Clone)] pub enum DbCommand { Ping, Shutdown }
  #[derive(Debug, Clone)] pub enum DbResult { Pong, Started, Stopped, Error(String) }

- In fe-runtime/src/channels.rs, define:
  pub struct ChannelHandles {
    pub net_cmd_tx: crossbeam::channel::Sender<NetworkCommand>,
    pub net_cmd_rx: crossbeam::channel::Receiver<NetworkCommand>,
    pub net_evt_tx: crossbeam::channel::Sender<NetworkEvent>,
    pub net_evt_rx: crossbeam::channel::Receiver<NetworkEvent>,
    pub db_cmd_tx: crossbeam::channel::Sender<DbCommand>,
    pub db_cmd_rx: crossbeam::channel::Receiver<DbCommand>,
    pub db_res_tx: crossbeam::channel::Sender<DbResult>,
    pub db_res_rx: crossbeam::channel::Receiver<DbResult>,
  }
  impl ChannelHandles { pub fn new() -> Self { all bounded(256) } }

- Write unit tests in fe-runtime/src/messages.rs:
  #[cfg(test)] tests for Debug/Clone on all enums, channel send/recv roundtrip

Phase 3 — Database Thread (T3):
- In fe-database/src/lib.rs, implement:
  pub fn spawn_db_thread(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
  ) -> std::thread::JoinHandle<()>

  Implementation:
  - assert!(tokio::runtime::Handle::try_current().is_err(), "No ambient Tokio allowed on DB thread spawn");
  - std::thread::spawn(move || {
      let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
      rt.block_on(async {
        // init SurrealDB with SurrealKV
        let db = surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db").await
          .expect("SurrealDB init");
        tx.send(DbResult::Started).ok();
        loop { match rx.recv() {
          Ok(DbCommand::Ping) => { tx.send(DbResult::Pong).ok(); }
          Ok(DbCommand::Shutdown) | Err(_) => break,
        }}
        tracing::info!("DB thread shutting down");
      });
    })

- Write integration test: spawn_db_thread, send Ping, assert Pong within 5ms

Phase 4 — Network Thread (T2):
- In fe-network/src/lib.rs, implement:
  pub fn spawn_network_thread(
    rx: crossbeam::channel::Receiver<NetworkCommand>,
    tx: crossbeam::channel::Sender<NetworkEvent>,
  ) -> std::thread::JoinHandle<()>

  Implementation mirrors DB thread pattern:
  - assert no ambient Tokio before creating runtime
  - tx.send(NetworkEvent::Started)
  - loop: NetworkCommand::Ping → NetworkEvent::Pong, Shutdown → break
  - Add: // TODO Sprint 3B: initialise libp2p Swarm and iroh endpoint here

- Write integration test: send Ping, assert Pong within 2ms

Phase 5 — Bevy App Integration (T1):
- In fe-runtime/src/app.rs, implement:
  pub fn build_app(handles: ChannelHandles) -> bevy::app::App

  - Create NetworkCommandSender(crossbeam::Sender<NetworkCommand>) Bevy Resource
  - Create DbCommandSender(crossbeam::Sender<DbCommand>) Bevy Resource
  - Create NetworkEventReceiver(Arc<Mutex<crossbeam::Receiver<NetworkEvent>>>) Bevy Resource
  - Create DbResultReceiver(Arc<Mutex<crossbeam::Receiver<DbResult>>>) Bevy Resource
  - Add drain_network_events system to Update schedule:
    non-blocking try_recv() loop → emit NetworkEvent via EventWriter<NetworkEvent>
    Never blocks. Never sleeps.
  - Add drain_db_results system to Update schedule:
    non-blocking try_recv() loop → emit DbResult via EventWriter<DbResult>
  - Add FrameTimeDiagnosticsPlugin

Phase 6 — Integration Tests and Frame Budget Validation:
- tests/integration_test.rs:
  - Thread isolation: assert Handle::try_current().is_err() on main thread
  - Network round-trip: Ping → Pong < 2ms
  - DB round-trip: Ping → Pong < 5ms
  - Clean shutdown: both threads exit within 1s on Shutdown
  - Frame budget test stub (comment: // DEFERRED TO WAVE 6 — requires headless Bevy runner)

RULES:
- Write code only. Do NOT run: cargo build, cargo test, cargo check, cargo clippy,
  cargo fmt, cargo tarpaulin, or any command that invokes rustc.
- Do NOT run any command that takes more than 2 seconds.
- Do NOT create git commits.
- Do NOT add doc comments or inline comments unless the logic is non-obvious.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- If a task says "Run tests" or "cargo ...": skip it, write // DEFERRED TO WAVE 6
- Update PLAYBOOK.md progress table: change Sprint 1 ⬜ Pending to ✅ Done when complete.
```

---

## SPRINT 2 — Identity + Scaffolds (Tracks 02, 03 scaffold, 04 scaffold)

> Run 2A, 2B, 2C in **parallel worktrees** — launch all 3 in one message using the Agent tool.
> **Dependency gate:** Sprint 1 merged to main.

### Prompt 2A — Root Identity (Track 02, full implementation)

**Plugin:** `/ultrawork` in a git worktree on branch `track/root-identity`

```
Implement Track 02: Root Identity for FractalEngine.
Branch from main. Work in fe-identity crate (replace the stub lib.rs from Sprint 1).

Read the plan at: conductor/tracks/root_identity_20260321/plan.md

EXISTING CODE:
- fe-identity/src/lib.rs — stub (//! Stub crate) from Sprint 1
- fe-runtime/src/messages.rs — NetworkCommand, NetworkEvent, DbCommand, DbResult enums
- Cargo.toml workspace — fe-identity already declared as workspace member
  with ed25519-dalek 2.2, jsonwebtoken 10.3, keyring 3, serde, anyhow in workspace deps

IMPLEMENT ALL PHASES:

Phase 1 — Keypair Generation:
- fe-identity/src/keypair.rs:
  pub struct NodeKeypair { signing_key: ed25519_dalek::SigningKey }
  impl NodeKeypair {
    pub fn generate() -> Self  // uses OsRng
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, anyhow::Error>
    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey
    pub fn sign(&self, message: &[u8]) -> ed25519_dalek::Signature
    pub fn to_did_key(&self) -> String  // delegated to did_key module
  }
  // NEVER expose signing_key field publicly — private by design

Phase 2 — OS Keychain Storage:
- fe-identity/src/keychain.rs:
  const SERVICE: &str = "fractalengine";
  pub fn store_keypair(node_id: &str, keypair: &NodeKeypair) -> anyhow::Result<()>
    // keyring::Entry::new(SERVICE, node_id)?.set_password(&hex_encoded_key)
  pub fn load_keypair(node_id: &str) -> anyhow::Result<NodeKeypair>
  pub fn delete_keypair(node_id: &str) -> anyhow::Result<()>

Phase 3 — did:key Derivation:
- fe-identity/src/did_key.rs:
  pub fn pub_key_to_did_key(pub_key: &ed25519_dalek::VerifyingKey) -> String
    // multicodec prefix for ed25519: 0xed01
    // encode: multibase::encode(Base::Base58Btc, [0xed, 0x01, ...pub_key_bytes...])
    // result: "did:key:z6Mk<multibase_string>"

Phase 4 — JWT Mint and Verify:
- fe-identity/src/jwt.rs:
  #[derive(serde::Serialize, serde::Deserialize)]
  pub struct FractalClaims {
    pub sub: String,   // did:key string
    pub petal_id: String,
    pub role_id: String,
    pub iat: u64,
    pub exp: u64,      // iat + max 300 seconds
  }
  pub fn mint_session_token(
    keypair: &NodeKeypair, petal_id: &str, role_id: &str, ttl_secs: u64
  ) -> anyhow::Result<String>
    // jsonwebtoken::encode with EdDSA algorithm
    // enforce: ttl_secs <= 300, else return Err
  pub fn verify_session_token(
    token: &str, issuer_pub_key: &ed25519_dalek::VerifyingKey
  ) -> anyhow::Result<FractalClaims>
    // always use verify_strict() — NEVER verify()

Phase 5 — Bevy Resource:
- fe-identity/src/resource.rs:
  #[derive(bevy::prelude::Resource)]
  pub struct NodeIdentity { keypair: NodeKeypair, did_key: String }
  impl NodeIdentity {
    pub fn new(keypair: NodeKeypair) -> Self
    pub fn did_key(&self) -> &str
    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey
    pub fn sign(&self, msg: &[u8]) -> ed25519_dalek::Signature
    // No pub keypair field. SigningKey never leaves this struct.
  }

Phase 6 — Unit Tests:
- fe-identity/src/keypair.rs #[cfg(test)]:
  test_generate_and_sign: sign a message, verify with verifying_key
  test_tampered_message_rejected: sign "hello", verify "world" → must be Err
- fe-identity/src/did_key.rs #[cfg(test)]:
  test_did_key_format: result starts with "did:key:z6Mk"
- fe-identity/src/jwt.rs #[cfg(test)]:
  test_mint_and_verify: mint token, verify returns correct claims
  test_expired_token: mint with ttl=1, sleep 2s (in test), verify returns Err
  test_ttl_limit_enforced: mint with ttl=301 returns Err
  test_tampered_token: mint token, flip one char, verify returns Err

RULES:
- Write code only. Do NOT run: cargo build, cargo test, cargo check, cargo clippy,
  cargo fmt, cargo tarpaulin, or any command that invokes rustc.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- If a task says "Run tests" or "cargo ...": skip it, write // DEFERRED TO WAVE 6
- Update PLAYBOOK.md progress table: change Sprint 2A ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 2B — Petal Soil Scaffold (Track 03, types + schema constants only)

**Plugin:** `/ultrawork` in a git worktree on branch `track/petal-soil-scaffold`

```
Scaffold Track 03: Petal Soil for FractalEngine.
Branch from main. Work in fe-database crate.
SCAFFOLD ONLY — define types and schema string constants. No SurrealDB query calls.

Read the plan at: conductor/tracks/petal_soil_20260321/plan.md

EXISTING CODE:
- fe-database/src/lib.rs — has spawn_db_thread() from Sprint 1 (keep it, add modules)
- Cargo.toml — surrealdb 3.0 (kv-surrealkv), tokio, serde, anyhow, ulid in workspace

CREATE fe-database/src/schema.rs:
  // SurrealQL schema definitions as &str constants
  pub const DEFINE_PETAL: &str = "
    DEFINE TABLE petal SCHEMAFULL PERMISSIONS
      FOR select WHERE $auth.role = 'public' OR $auth.petal_id = id
      FOR create, update, delete WHERE $auth.role = 'admin';
    DEFINE FIELD petal_id ON petal TYPE string;
    DEFINE FIELD name ON petal TYPE string;
    DEFINE FIELD node_id ON petal TYPE string;
    DEFINE FIELD created_at ON petal TYPE datetime;
  ";
  pub const DEFINE_ROOM: &str = "/* DEFINE TABLE room SCHEMAFULL ... */";
  pub const DEFINE_MODEL: &str = "/* DEFINE TABLE model SCHEMAFULL ... */";
  pub const DEFINE_ROLE: &str = "/* DEFINE TABLE role SCHEMAFULL ... */";
  pub const DEFINE_OP_LOG: &str = "
    DEFINE TABLE op_log SCHEMAFULL;
    DEFINE FIELD lamport_clock ON op_log TYPE int;
    DEFINE FIELD node_id ON op_log TYPE string;
    DEFINE FIELD op_type ON op_log TYPE string;
    DEFINE FIELD payload ON op_log TYPE object;
    DEFINE FIELD sig ON op_log TYPE bytes;
  ";

CREATE fe-database/src/types.rs:
  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub struct PetalId(pub ulid::Ulid);

  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub struct NodeId(pub String);  // did:key string

  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub struct RoleId(pub String);

  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub enum OpType {
    CreatePetal, CreateRoom, PlaceModel, AssignRole, RevokeSession, DeletePetal,
  }

  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub struct OpLogEntry {
    pub lamport_clock: u64,
    pub node_id: NodeId,
    pub op_type: OpType,
    pub payload: serde_json::Value,
    pub sig: [u8; 64],
  }

CREATE fe-database/src/rbac.rs:
  // Stub function signatures — implemented Sprint 3A
  pub async fn apply_schema(db: &surrealdb::Surreal<surrealdb::engine::local::Db>) -> anyhow::Result<()> {
    todo!("Sprint 3A: run all DEFINE_* constants against db")
  }
  pub async fn get_role(db: &surrealdb::Surreal<surrealdb::engine::local::Db>, node_id: &str, petal_id: &str) -> anyhow::Result<RoleId> {
    todo!("Sprint 3A")
  }

CREATE fe-database/src/op_log.rs:
  pub async fn write_op_log(db: &surrealdb::Surreal<surrealdb::engine::local::Db>, entry: OpLogEntry) -> anyhow::Result<()> {
    todo!("Sprint 3A")
  }
  pub async fn query_petal_at_time(db: &surrealdb::Surreal<surrealdb::engine::local::Db>, petal_id: &PetalId, timestamp: chrono::DateTime<chrono::Utc>) -> anyhow::Result<serde_json::Value> {
    todo!("Sprint 3A: use SurrealKV VERSION clause")
  }

CREATE fe-database/src/queries.rs:
  // Stub query functions
  pub async fn create_petal(db: &surrealdb::Surreal<surrealdb::engine::local::Db>, name: &str, node_id: &NodeId) -> anyhow::Result<PetalId> { todo!() }
  pub async fn create_room(db: &surrealdb::Surreal<surrealdb::engine::local::Db>, petal_id: &PetalId, name: &str) -> anyhow::Result<()> { todo!() }
  pub async fn place_model(db: &surrealdb::Surreal<surrealdb::engine::local::Db>, petal_id: &PetalId, asset_id: &str, transform: serde_json::Value) -> anyhow::Result<()> { todo!() }

Update fe-database/src/lib.rs: add mod schema; mod types; mod rbac; mod op_log; mod queries;
pub use types::*;

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- todo!() bodies are EXPECTED for scaffold — do not implement them.
- Update PLAYBOOK.md progress table: change Sprint 2B ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 2C — Mycelium Network Scaffold (Track 04, types + module structure only)

**Plugin:** `/ultrawork` in a git worktree on branch `track/mycelium-scaffold`

```
Scaffold Track 04: Mycelium Network for FractalEngine.
Branch from main. Work in fe-network crate.
SCAFFOLD ONLY — define types and module structure. No libp2p or iroh calls.

Read the plan at: conductor/tracks/mycelium_network_20260321/plan.md

EXISTING CODE:
- fe-network/src/lib.rs — has spawn_network_thread() from Sprint 1 (keep it, add modules)
- Cargo.toml — add to fe-network/Cargo.toml:
  libp2p = { workspace = true }
  iroh-blobs = { workspace = true }
  iroh-gossip = { workspace = true }
  iroh-docs = { workspace = true }
  serde = { workspace = true }
  blake3 = { workspace = true }
  url = "2"
  ipnet = "2"

CREATE fe-network/src/types.rs:
  // GossipMessage ALWAYS carries a signature — unsigned messages are not representable
  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub struct GossipMessage<T: serde::Serialize> {
    pub payload: T,
    pub sig: [u8; 64],
    pub pub_key: [u8; 32],
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
  pub struct AssetId(pub [u8; 32]);  // BLAKE3 hash

  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  pub struct PetalTopic(pub String);  // iroh-gossip topic = petal_id string

  #[derive(Debug, Clone)]
  pub struct IrohConfig {
    pub relay_url: Option<url::Url>,
    pub max_concurrent_transfers: usize,
  }
  impl Default for IrohConfig {
    fn default() -> Self { Self { relay_url: None, max_concurrent_transfers: 8 } }
  }

CREATE fe-network/src/swarm.rs:
  // libp2p Swarm stub — full implementation Sprint 3B
  pub struct SwarmHandle; // placeholder
  pub async fn build_swarm(local_key: libp2p::identity::Keypair) -> anyhow::Result<SwarmHandle> {
    todo!("Sprint 3B: Kademlia DHT + mDNS + Noise + Yamux + QUIC")
  }

CREATE fe-network/src/discovery.rs:
  pub async fn start_discovery(swarm: &mut super::swarm::SwarmHandle) -> anyhow::Result<()> {
    todo!("Sprint 3B: bootstrap Kademlia, start mDNS")
  }

CREATE fe-network/src/gossip.rs:
  pub async fn subscribe_petal(petal_id: &str) -> anyhow::Result<()> {
    todo!("Sprint 3B: iroh-gossip HyParView topic subscription")
  }
  pub async fn broadcast<T: serde::Serialize>(msg: super::types::GossipMessage<T>) -> anyhow::Result<()> {
    // CRITICAL: verify signature BEFORE any other logic on INBOUND messages
    todo!("Sprint 3B")
  }

CREATE fe-network/src/iroh_blobs.rs:
  pub async fn register_asset(hash: super::types::AssetId, path: std::path::PathBuf) -> anyhow::Result<()> {
    todo!("Sprint 3B: iroh-blobs provide")
  }
  pub async fn fetch_asset(hash: super::types::AssetId) -> anyhow::Result<Vec<u8>> {
    todo!("Sprint 3B: iroh-blobs get")
  }

CREATE fe-network/src/iroh_docs.rs:
  pub async fn sync_petal_state(petal_id: &str, peer_id: &str) -> anyhow::Result<Vec<u8>> {
    todo!("Sprint 3B: iroh-docs range reconciliation, delta only")
  }

Update fe-network/src/lib.rs: add mod types; mod swarm; mod discovery; mod gossip; mod iroh_blobs; mod iroh_docs;
pub use types::*;

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- todo!() bodies are EXPECTED — do not implement them.
- Update PLAYBOOK.md progress table: change Sprint 2C ⬜ Pending to ✅ Done when complete.
```

### Post-Sprint 2 — Merge

```bash
git checkout main
git merge track/root-identity
git merge track/petal-soil-scaffold   # resolve Cargo.toml conflicts: keep superset
git merge track/mycelium-scaffold     # resolve Cargo.toml conflicts: keep superset
```

---

## SPRINT 3 — Data + Network + Renderer Scaffold

> Run 3A, 3B, 3C in **parallel worktrees**.
> **Dependency gate:** Sprint 2 merged to main.

### Prompt 3A — Petal Soil Full Implementation (Track 03)

**Plugin:** `/ultrawork` in a git worktree on branch `track/petal-soil-impl`

```
Implement Track 03: Petal Soil for FractalEngine — fill in all todo!() stubs.
Branch from main after Sprint 2 merge. Work in fe-database crate.

Read the plan at: conductor/tracks/petal_soil_20260321/plan.md

EXISTING CODE (from Sprint 2B scaffold):
- fe-database/src/schema.rs — DEFINE_PETAL, DEFINE_ROOM, DEFINE_MODEL, DEFINE_ROLE, DEFINE_OP_LOG constants
- fe-database/src/types.rs — PetalId, NodeId, RoleId, OpType, OpLogEntry
- fe-database/src/rbac.rs — apply_schema(), get_role() stubs
- fe-database/src/op_log.rs — write_op_log(), query_petal_at_time() stubs
- fe-database/src/queries.rs — create_petal(), create_room(), place_model() stubs

IMPLEMENT:

fe-database/src/rbac.rs — implement apply_schema():
  db.query(schema::DEFINE_PETAL).await?;
  db.query(schema::DEFINE_ROOM).await?;
  db.query(schema::DEFINE_MODEL).await?;
  db.query(schema::DEFINE_ROLE).await?;
  db.query(schema::DEFINE_OP_LOG).await?;
  Ok(())

fe-database/src/rbac.rs — implement get_role():
  let result: Option<serde_json::Value> = db
    .query("SELECT role FROM role WHERE node_id = $node_id AND petal_id = $petal_id")
    .bind(("node_id", node_id)).bind(("petal_id", petal_id))
    .await?.take(0)?;
  Ok(result.map(|v| RoleId(v["role"].as_str().unwrap_or("public").to_string()))
    .unwrap_or_else(|| RoleId("public".to_string())))

fe-database/src/op_log.rs — implement write_op_log():
  // write BEFORE applying the actual change — this is the invariant
  db.create::<Vec<serde_json::Value>>("op_log").content(&entry).await?;
  Ok(())

fe-database/src/op_log.rs — implement query_petal_at_time():
  // SurrealKV VERSION clause for time-travel
  let ts = timestamp.timestamp();
  db.query(format!("SELECT * FROM petal:{} VERSION d'{}'", petal_id.0, timestamp.to_rfc3339()))
    .await?.take(0)
    .map_err(Into::into)

fe-database/src/queries.rs — implement create_petal(), create_room(), place_model():
  // Each mutation: write_op_log() FIRST, then apply the change

fe-database/src/lib.rs — add DbHandle Bevy Resource:
  #[derive(bevy::prelude::Resource, Clone)]
  pub struct DbHandle(pub std::sync::Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>);

Update spawn_db_thread() to call apply_schema() on startup.

WRITE INTEGRATION TESTS in fe-database/tests/rbac_test.rs:
  test_public_cannot_write: connect as public role, attempt create_petal → Err
  test_custom_role_scoped: assign custom role, verify they can access assigned petal
  test_admin_full_access: verify admin can CRUD all tables
  test_op_log_written_before_change: intercept op_log write order

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- Update PLAYBOOK.md progress table: change Sprint 3A ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 3B — Mycelium Network Full Implementation (Track 04)

**Plugin:** `/ultrawork` in a git worktree on branch `track/mycelium-network-impl`

```
Implement Track 04: Mycelium Network for FractalEngine — fill in all todo!() stubs.
Branch from main after Sprint 2 merge. Work in fe-network crate.

Read the plan at: conductor/tracks/mycelium_network_20260321/plan.md

EXISTING CODE (from Sprint 2C scaffold):
- fe-network/src/types.rs — GossipMessage<T>, AssetId, PetalTopic, IrohConfig
- fe-network/src/swarm.rs, discovery.rs, gossip.rs, iroh_blobs.rs, iroh_docs.rs — all todo!() stubs
- fe-network/src/lib.rs — spawn_network_thread() stub from Sprint 1

IMPLEMENT fe-network/src/swarm.rs:
  pub struct SwarmHandle {
    swarm: libp2p::Swarm<FractalBehaviour>,
  }
  #[libp2p::swarm::NetworkBehaviour]
  struct FractalBehaviour {
    kademlia: libp2p::kad::Behaviour<libp2p::kad::store::MemoryStore>,
    mdns: libp2p::mdns::tokio::Behaviour,
  }
  pub async fn build_swarm(local_key: libp2p::identity::Keypair) -> anyhow::Result<SwarmHandle> {
    let peer_id = libp2p::PeerId::from(&local_key.public());
    let transport = libp2p::quic::tokio::Transport::new(libp2p::quic::Config::new(&local_key))
      .map(|(peer, conn), _| (peer, libp2p::core::muxing::StreamMuxerBox::new(conn)))
      .boxed();
    // configure Kademlia + mDNS behaviour
    // build Swarm with Noise + Yamux
  }

IMPLEMENT fe-network/src/gossip.rs:
  // INBOUND: ALWAYS verify signature before any logic. Reject and log on invalid sig.
  pub fn verify_gossip_message<T: serde::de::DeserializeOwned>(
    msg: &GossipMessage<T>, expected_pub_key: Option<&[u8; 32]>
  ) -> anyhow::Result<()> {
    let pub_key = ed25519_dalek::VerifyingKey::from_bytes(&msg.pub_key)?;
    let payload_bytes = serde_json::to_vec(&msg.payload)?;
    let sig = ed25519_dalek::Signature::from_bytes(&msg.sig);
    pub_key.verify_strict(&payload_bytes, &sig).map_err(Into::into)
  }

IMPLEMENT fe-network/src/iroh_blobs.rs — iroh-blobs provide/get for BLAKE3-verified assets

IMPLEMENT fe-network/src/iroh_docs.rs — iroh-docs range reconciliation, delta sync only

Update spawn_network_thread() in lib.rs:
  - Build the SwarmHandle and iroh endpoint inside the Tokio runtime
  - Replace the stub loop with real event processing
  - NetworkCommand::Shutdown still breaks the loop cleanly

WRITE TESTS in fe-network/tests/ using in-process channel mocks:
  test_gossip_valid_message: sign a message, verify passes
  test_gossip_tampered_message: flip a byte, verify rejects with Err
  test_asset_roundtrip: register_asset then fetch_asset returns same bytes

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- Update PLAYBOOK.md progress table: change Sprint 3B ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 3C — Bloom Renderer Scaffold (Track 05, types only)

**Plugin:** `/ultrawork` in a git worktree on branch `track/bloom-renderer-scaffold`

```
Scaffold Track 05: Bloom Renderer for FractalEngine.
Branch from main after Sprint 2 merge. Replace stub fe-renderer crate from Sprint 1.
SCAFFOLD ONLY — define trait, types, and function signatures with todo!() bodies.

Read the plan at: conductor/tracks/bloom_renderer_20260321/plan.md

Create fe-renderer/Cargo.toml:
  [package] name = "fe-renderer" version = "0.1.0"
  [dependencies]
  bevy = { workspace = true }
  blake3 = { workspace = true }
  serde = { workspace = true }
  anyhow = { workspace = true }
  fe-database = { path = "../fe-database" }   # for DbHandle and AssetId
  fe-network = { path = "../fe-network" }     # for AssetId (shared type)

Create fe-renderer/src/lib.rs:
  pub mod ingester;
  pub mod addressing;
  pub mod dead_reckoning;
  pub mod loader;

Create fe-renderer/src/ingester.rs:
  use fe_network::AssetId;
  pub trait AssetIngester: Send + Sync {
    fn ingest(&self, bytes: &[u8]) -> anyhow::Result<AssetId>;
  }
  pub struct GltfIngester;
  impl AssetIngester for GltfIngester {
    fn ingest(&self, _bytes: &[u8]) -> anyhow::Result<AssetId> {
      todo!("Sprint 4A: validate GLB magic, check size, compute BLAKE3, register in DB")
    }
  }
  pub struct SplatIngester; // v2 — Gaussian splatting, not in scope until post-v1
  impl AssetIngester for SplatIngester {
    fn ingest(&self, _bytes: &[u8]) -> anyhow::Result<AssetId> {
      todo!("v2 target: Gaussian splatting ingestion")
    }
  }
  pub const MAX_ASSET_SIZE_BYTES: usize = 256 * 1024 * 1024; // 256 MB

Create fe-renderer/src/addressing.rs:
  use fe_network::AssetId;
  pub fn content_address(bytes: &[u8]) -> AssetId {
    todo!("Sprint 4A: blake3::hash(bytes), wrap in AssetId")
  }
  pub fn validate_glb_magic(bytes: &[u8]) -> bool {
    todo!("Sprint 4A: check bytes[0..4] == [0x67, 0x6C, 0x54, 0x46] (glTF)")
  }

Create fe-renderer/src/dead_reckoning.rs:
  #[derive(bevy::prelude::Component)]
  pub struct DeadReckoningPredictor {
    pub velocity: bevy::math::Vec3,
    pub last_network_pos: bevy::math::Vec3,
    pub last_network_time: f64,
    pub threshold: f32,  // send update when |actual - predicted| > threshold
  }
  impl Default for DeadReckoningPredictor {
    fn default() -> Self { Self { velocity: bevy::math::Vec3::ZERO, last_network_pos: bevy::math::Vec3::ZERO, last_network_time: 0.0, threshold: 0.1 } }
  }
  pub fn dead_reckoning_system(
    _query: bevy::prelude::Query<(&mut bevy::prelude::Transform, &mut DeadReckoningPredictor)>,
    _time: bevy::prelude::Res<bevy::prelude::Time>,
  ) {
    todo!("Sprint 4A: predict position each frame, emit NetworkCommand::EntityUpdate on threshold cross")
  }

Create fe-renderer/src/loader.rs:
  use fe_network::AssetId;
  pub fn load_to_bevy(
    _asset_id: AssetId,
    _asset_server: &bevy::prelude::AssetServer,
  ) -> bevy::prelude::Handle<bevy::gltf::Gltf> {
    todo!("Sprint 4A: resolve local cache path by hash, load; if missing request via iroh-blobs")
  }

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- todo!() bodies are EXPECTED — do not implement them.
- Update PLAYBOOK.md progress table: change Sprint 3C ⬜ Pending to ✅ Done when complete.
```

### Post-Sprint 3 — Merge

```bash
git checkout main
git merge track/petal-soil-impl
git merge track/mycelium-network-impl
git merge track/bloom-renderer-scaffold
```

---

## SPRINT 4 — Renderer + Auth + Late Scaffolds

> Run 4A, 4B, 4C, 4D in **parallel worktrees**.
> **Dependency gate:** Sprint 3 merged to main.

### Prompt 4A — Bloom Renderer Full Implementation (Track 05)

**Plugin:** `/ultrawork` in a git worktree on branch `track/bloom-renderer-impl`

```
Implement Track 05: Bloom Renderer for FractalEngine — fill in all todo!() stubs.
Branch from main after Sprint 3 merge. Work in fe-renderer crate.

Read the plan at: conductor/tracks/bloom_renderer_20260321/plan.md

EXISTING CODE (Sprint 3C scaffold):
- fe-renderer/src/ingester.rs — GltfIngester::ingest() and SplatIngester stubs
- fe-renderer/src/addressing.rs — content_address(), validate_glb_magic() stubs
- fe-renderer/src/dead_reckoning.rs — DeadReckoningPredictor component, system stub
- fe-renderer/src/loader.rs — load_to_bevy() stub

IMPLEMENT fe-renderer/src/addressing.rs:
  pub fn content_address(bytes: &[u8]) -> AssetId {
    let hash = blake3::hash(bytes);
    AssetId(*hash.as_bytes())
  }
  pub fn validate_glb_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == [0x67, 0x6C, 0x54, 0x46]
  }

IMPLEMENT fe-renderer/src/ingester.rs — GltfIngester::ingest():
  fn ingest(&self, bytes: &[u8]) -> anyhow::Result<AssetId> {
    if bytes.len() > MAX_ASSET_SIZE_BYTES { anyhow::bail!("Asset exceeds 256MB limit") }
    if !validate_glb_magic(bytes) { anyhow::bail!("Not a valid GLB file") }
    let asset_id = content_address(bytes);
    // write to local cache: std::fs::write(cache_path(asset_id), bytes)?;
    Ok(asset_id)
  }
  // helper: fn cache_path(asset_id: AssetId) -> PathBuf
  //   dirs::cache_dir() / "fractalengine" / "assets" / hex(asset_id.0)

IMPLEMENT fe-renderer/src/loader.rs:
  pub fn load_to_bevy(asset_id: AssetId, asset_server: &AssetServer) -> Handle<bevy::gltf::Gltf> {
    let path = cache_path(asset_id);
    if path.exists() {
      asset_server.load(path)
    } else {
      // request via iroh-blobs (async — spawn bevy task, return placeholder handle for now)
      // CROSS-CRATE: fe_network::iroh_blobs::fetch_asset(asset_id)
      asset_server.load("assets/placeholder.glb")
    }
  }

IMPLEMENT fe-renderer/src/dead_reckoning.rs — system:
  pub fn dead_reckoning_system(
    mut query: Query<(&mut Transform, &mut DeadReckoningPredictor)>,
    time: Res<Time>,
    net_cmd_tx: Option<Res<NetworkCommandSender>>,  // CROSS-CRATE: from fe-runtime
  ) {
    for (mut transform, mut predictor) in query.iter_mut() {
      let dt = time.elapsed_secs_f64() - predictor.last_network_time;
      let predicted = predictor.last_network_pos + predictor.velocity * dt as f32;
      transform.translation = predicted;
      let deviation = (transform.translation - predicted).length();
      if deviation > predictor.threshold {
        if let Some(tx) = &net_cmd_tx {
          // tx.0.try_send(NetworkCommand::EntityUpdate(...)).ok();
          // CROSS-CRATE: NetworkCommand::EntityUpdate added in Sprint 3B
        }
      }
    }
  }

WRITE TESTS in fe-renderer/tests/:
  test_glb_magic_valid: pass valid 4-byte GLB header → true
  test_glb_magic_invalid: pass [0,0,0,0] → false
  test_size_limit: ingest bytes.len() > 256MB → Err
  test_content_address_deterministic: same bytes → same AssetId twice
  test_content_address_unique: different bytes → different AssetId

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- Update PLAYBOOK.md progress table: change Sprint 4A ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 4B — Petal Gate Full Implementation (Track 06)

**Plugin:** `/ultrawork` in a git worktree on branch `track/petal-gate-impl`

```
Implement Track 06: Petal Gate for FractalEngine — auth handshake, session cache, role enforcement.
Branch from main after Sprint 3 merge. Create fe-auth crate (replace stub).

Read the plan at: conductor/tracks/petal_gate_20260321/plan.md

Create fe-auth/Cargo.toml:
  [package] name = "fe-auth" version = "0.1.0"
  [dependencies]
  fe-identity = { path = "../fe-identity" }
  fe-database = { path = "../fe-database" }
  fe-network = { path = "../fe-network" }
  bevy = { workspace = true }
  ed25519-dalek = { workspace = true }
  anyhow = { workspace = true }
  serde = { workspace = true }
  tokio = { workspace = true }

CREATE fe-auth/src/lib.rs:
  pub mod session;
  pub mod cache;
  pub mod handshake;
  pub mod revocation;

CREATE fe-auth/src/cache.rs:
  use std::collections::HashMap;
  use std::time::Instant;
  pub const SESSION_TTL_SECS: u64 = 60;
  pub struct SessionCache {
    entries: HashMap<[u8; 32], (fe_database::RoleId, Instant)>,
  }
  impl SessionCache {
    pub fn new() -> Self { Self { entries: HashMap::new() } }
    pub fn get(&self, pub_key: &[u8; 32]) -> Option<&fe_database::RoleId> {
      self.entries.get(pub_key)
        .filter(|(_, t)| t.elapsed().as_secs() < SESSION_TTL_SECS)
        .map(|(role, _)| role)
    }
    pub fn insert(&mut self, pub_key: [u8; 32], role: fe_database::RoleId) {
      self.entries.insert(pub_key, (role, Instant::now()));
    }
    pub fn revoke(&mut self, pub_key: &[u8; 32]) {
      self.entries.remove(pub_key);
    }
    pub fn prune_expired(&mut self) {
      self.entries.retain(|_, (_, t)| t.elapsed().as_secs() < SESSION_TTL_SECS);
    }
  }

CREATE fe-auth/src/handshake.rs:
  // connect() → issue JWT → assign role → return token
  pub async fn connect(
    peer_pub_key: ed25519_dalek::VerifyingKey,
    petal_id: &fe_database::PetalId,
    node_identity: &fe_identity::NodeIdentity,
    db_handle: &fe_database::DbHandle,
    cache: &mut fe_auth::cache::SessionCache,
  ) -> anyhow::Result<String> {
    let role = fe_database::rbac::get_role(&db_handle.0, &peer_pub_key.to_bytes().map(|b| format!("{:02x}", b)).concat(), &petal_id.0.to_string()).await
      .unwrap_or_else(|_| fe_database::RoleId("public".to_string()));
    let token = fe_identity::jwt::mint_session_token(
      node_identity, &petal_id.0.to_string(), &role.0, 300
    )?;
    let key_bytes: [u8; 32] = peer_pub_key.to_bytes();
    cache.insert(key_bytes, role);
    Ok(token)
  }

CREATE fe-auth/src/revocation.rs:
  // CRITICAL ORDER: write to DB first, broadcast second, flush cache last
  pub async fn revoke_session(
    peer_pub_key_bytes: [u8; 32],
    petal_id: &fe_database::PetalId,
    db_handle: &fe_database::DbHandle,
    cache: &mut fe_auth::cache::SessionCache,
    // net_gossip_tx: crossbeam channel to network thread
  ) -> anyhow::Result<()> {
    // 1. Write revocation to SurrealDB op_log
    let entry = fe_database::OpLogEntry { /* revocation entry */ };
    fe_database::op_log::write_op_log(&db_handle.0, entry).await?;
    // 2. Broadcast signed revocation via iroh-gossip
    // CROSS-CRATE: send NetworkCommand::BroadcastRevocation to network thread
    // 3. Flush cache entry
    cache.revoke(&peer_pub_key_bytes);
    Ok(())
  }

WRITE TESTS in fe-auth/tests/:
  test_session_cache_ttl: insert entry, immediately get → Some; sleep 61s → None
    (use mock Instant, not real sleep in unit tests)
  test_connect_public_fallback: peer with no DB entry gets public role
  test_revocation_order: verify DB write happens before cache flush (use operation log)
  test_expired_jwt_rejected: verify fe_identity::jwt::verify_session_token rejects expired tokens
  test_tampered_jwt_rejected: flip one char in token, verify returns Err

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- Update PLAYBOOK.md progress table: change Sprint 4B ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 4C — Canopy View Scaffold (Track 07)

**Plugin:** `/ultrawork` in a git worktree on branch `track/canopy-view-scaffold`

```
Scaffold Track 07: Canopy View for FractalEngine.
Branch from main after Sprint 3 merge. Create fe-webview crate (replace stub).
SCAFFOLD ONLY — define types, trait, denylist constants, module structure. No wry calls.

Read the plan at: conductor/tracks/canopy_view_20260321/plan.md

IMPORTANT: Before any WebView code is written in Sprint 5A, a threat model document is required.
This scaffold does NOT include wry implementation — only types and security constants.

Create fe-webview/Cargo.toml:
  [package] name = "fe-webview" version = "0.1.0"
  [dependencies]
  bevy = { workspace = true }
  wry = "0.54"
  url = "2"
  ipnet = "2"
  serde = { workspace = true }
  anyhow = { workspace = true }
  fe-database = { path = "../fe-database" }

Create fe-webview/src/lib.rs: pub mod plugin; pub mod ipc; pub mod security; pub mod overlay;

CREATE fe-webview/src/ipc.rs:
  // Versioned typed IPC — no raw eval, no JS string injection
  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  #[serde(tag = "cmd", rename_all = "snake_case")]
  pub enum BrowserCommand {
    Navigate { url: url::Url },
    Close,
    GetUrl,
    SwitchTab { tab: BrowserTab },
  }
  #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
  #[serde(tag = "evt", rename_all = "snake_case")]
  pub enum BrowserEvent {
    UrlChanged { url: url::Url },
    LoadComplete,
    Error { message: String },
    TabChanged { tab: BrowserTab },
  }
  #[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
  pub enum BrowserTab { ExternalUrl, Config }

CREATE fe-webview/src/security.rs:
  use std::net::IpAddr;
  use ipnet::IpNet;
  // Block ALL localhost and RFC 1918 addresses — no exceptions
  pub const BLOCKED_HOSTS: &[&str] = &[
    "localhost", "127.0.0.1", "::1", "0.0.0.0",
  ];
  pub const BLOCKED_RANGES: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",   // link-local
    "fc00::/7",         // IPv6 ULA
  ];
  pub fn is_url_allowed(url: &url::Url) -> bool {
    if let Some(host) = url.host_str() {
      if BLOCKED_HOSTS.contains(&host) { return false; }
      if let Ok(ip) = host.parse::<IpAddr>() {
        for range in BLOCKED_RANGES {
          if let Ok(net) = range.parse::<IpNet>() {
            if net.contains(&ip) { return false; }
          }
        }
      }
    }
    true
  }

CREATE fe-webview/src/overlay.rs:
  #[derive(Debug, Clone)]
  pub struct WebViewConfig {
    pub petal_id: fe_database::PetalId,
    pub initial_url: Option<url::Url>,
    pub role: fe_database::RoleId,
  }
  pub trait BrowserSurface: Send + Sync {
    fn show(&mut self, url: url::Url) -> anyhow::Result<()>;
    fn hide(&mut self) -> anyhow::Result<()>;
    fn navigate(&mut self, url: url::Url) -> anyhow::Result<()>;
    fn position(&mut self, x: f32, y: f32, width: f32, height: f32) -> anyhow::Result<()>;
  }

CREATE fe-webview/src/plugin.rs:
  pub struct WebViewPlugin; // placeholder
  impl bevy::prelude::Plugin for WebViewPlugin {
    fn build(&self, _app: &mut bevy::prelude::App) {
      todo!("Sprint 5A: implement wry 0.54 overlay integration")
    }
  }

WRITE TESTS in fe-webview/tests/security_test.rs:
  test_localhost_blocked: is_url_allowed("http://localhost/") == false
  test_127_blocked: is_url_allowed("http://127.0.0.1/") == false
  test_10_x_x_x_blocked: is_url_allowed("http://10.0.0.1/") == false
  test_172_16_blocked: is_url_allowed("http://172.16.0.1/") == false
  test_192_168_blocked: is_url_allowed("http://192.168.1.1/") == false
  test_public_allowed: is_url_allowed("https://example.com") == true
  test_ipv6_ula_blocked: is_url_allowed("http://[fc00::1]/") == false

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- todo!() bodies are EXPECTED — do not implement wry.
- Update PLAYBOOK.md progress table: change Sprint 4C ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 4D — Fractal Mesh Scaffold (Track 08)

**Plugin:** `/ultrawork` in a git worktree on branch `track/fractal-mesh-scaffold`

```
Scaffold Track 08: Fractal Mesh for FractalEngine.
Branch from main after Sprint 3 merge. Create fe-sync crate (replace stub).
SCAFFOLD ONLY — define types, traits, module structure. No iroh-docs implementation.

Read the plan at: conductor/tracks/fractal_mesh_20260321/plan.md

Create fe-sync/Cargo.toml:
  [package] name = "fe-sync" version = "0.1.0"
  [dependencies]
  bevy = { workspace = true }
  fe-database = { path = "../fe-database" }
  fe-network = { path = "../fe-network" }
  serde = { workspace = true }
  anyhow = { workspace = true }
  tokio = { workspace = true }

Create fe-sync/src/lib.rs: pub mod replication; pub mod cache; pub mod offline; pub mod reconciliation;

CREATE fe-sync/src/replication.rs:
  pub struct ReplicationConfig {
    pub max_cache_gb: f32,
    pub eviction_days: u64,
  }
  impl Default for ReplicationConfig {
    fn default() -> Self { Self { max_cache_gb: 2.0, eviction_days: 7 } }
  }
  pub struct PetalReplica {
    pub petal_id: fe_database::PetalId,
    pub last_seen: std::time::Instant,
    pub local_db_namespace: String,
  }
  pub trait ReplicationStore: Send + Sync {
    fn sync(&self, peer_id: &str, petal_id: &fe_database::PetalId) -> anyhow::Result<Vec<u8>>;
  }
  pub struct IrohDocsStore;
  impl ReplicationStore for IrohDocsStore {
    fn sync(&self, _peer_id: &str, _petal_id: &fe_database::PetalId) -> anyhow::Result<Vec<u8>> {
      todo!("Sprint 5B: iroh-docs range reconciliation, delta only")
    }
  }

CREATE fe-sync/src/cache.rs:
  pub struct CacheEntry {
    pub asset_id: fe_network::AssetId,
    pub size_bytes: u64,
    pub last_accessed: std::time::Instant,
  }
  pub struct AssetCache {
    entries: std::collections::HashMap<fe_network::AssetId, CacheEntry>,
    config: super::replication::ReplicationConfig,
  }
  impl AssetCache {
    pub fn new(config: super::replication::ReplicationConfig) -> Self { todo!() }
    pub fn touch(&mut self, asset_id: fe_network::AssetId) -> anyhow::Result<()> { todo!("Sprint 5B") }
    pub fn evict_expired(&mut self) -> anyhow::Result<()> { todo!("Sprint 5B: evict after eviction_days") }
    pub fn total_size_bytes(&self) -> u64 { todo!() }
  }

CREATE fe-sync/src/offline.rs:
  pub fn is_petal_available_offline(petal_id: &fe_database::PetalId) -> bool {
    todo!("Sprint 5B: check local SurrealDB namespace exists")
  }
  pub async fn load_petal_offline(
    petal_id: &fe_database::PetalId,
    db_handle: &fe_database::DbHandle,
  ) -> anyhow::Result<serde_json::Value> {
    todo!("Sprint 5B: query local replica namespace, return stale data with staleness indicator")
  }

CREATE fe-sync/src/reconciliation.rs:
  pub async fn reconcile_petal(
    petal_id: &fe_database::PetalId,
    peer_id: &str,
    store: &dyn super::replication::ReplicationStore,
    db_handle: &fe_database::DbHandle,
  ) -> anyhow::Result<u64> {
    todo!("Sprint 5B: exchange iroh-docs document heads, apply delta, return entries applied count")
  }

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- todo!() bodies are EXPECTED — do not implement them.
- Update PLAYBOOK.md progress table: change Sprint 4D ⬜ Pending to ✅ Done when complete.
```

### Post-Sprint 4 — Merge

```bash
git checkout main
git merge track/bloom-renderer-impl
git merge track/petal-gate-impl
git merge track/canopy-view-scaffold
git merge track/fractal-mesh-scaffold
```

---

## SPRINT 5 — Final Feature Tracks

> Run 5A, 5B, 5C, 5D in **parallel worktrees**.
> **Dependency gate:** Sprint 4 merged to main.

### Prompt 5A — Canopy View Full Implementation (Track 07)

**Plugin:** `/ultrawork` in a git worktree on branch `track/canopy-view-impl`

```
Implement Track 07: Canopy View for FractalEngine — wry 0.54 WebView overlay plugin.
Branch from main after Sprint 4 merge. Work in fe-webview crate.

Read the plan at: conductor/tracks/canopy_view_20260321/plan.md

EXISTING CODE (Sprint 4C scaffold):
- fe-webview/src/ipc.rs — BrowserCommand, BrowserEvent, BrowserTab enums
- fe-webview/src/security.rs — is_url_allowed(), BLOCKED_HOSTS, BLOCKED_RANGES
- fe-webview/src/overlay.rs — WebViewConfig, BrowserSurface trait
- fe-webview/src/plugin.rs — WebViewPlugin stub

FIRST — Write docs/webview-threat-model.md covering:
  # WebView Threat Model
  ## SSRF (Server-Side Request Forgery)
  - Attack: WebView navigates to internal service via URL injection
  - Mitigation: navigation_handler checks ALL urls via is_url_allowed() before loading.
    localhost and all RFC 1918 ranges blocked unconditionally.
  ## XSS via IPC
  - Attack: Malicious page injects JS to call native IPC with crafted payload
  - Mitigation: Only typed BrowserCommand enum accepted. No raw eval. No string injection.
    All IPC messages deserialized strictly via serde.
  ## Credential Harvesting
  - Attack: External URL phishing for FractalEngine credentials
  - Mitigation: Non-dismissible trust bar injected into every WebView page
    showing domain + "External Website" label. Users cannot remove it.
  ## Localhost Bypass via DNS Rebinding
  - Attack: External domain resolves to 127.0.0.1 after DNS TTL expires
  - Mitigation: Block by resolved IP, not by hostname only.
    Post-navigation IP check deferred to v2 (document here as known gap).
  ## Oversized Content
  - Attack: WebView loading multi-GB external resource to exhaust memory
  - Mitigation: Browser-level limit deferred to v2. Document as known gap.

THEN implement fe-webview/src/plugin.rs:
  pub struct WryBrowserSurface {
    webview: wry::WebView,
    config: WebViewConfig,
    current_tab: BrowserTab,
  }
  impl BrowserSurface for WryBrowserSurface {
    fn show(&mut self, url: url::Url) -> anyhow::Result<()> {
      if !security::is_url_allowed(&url) { anyhow::bail!("URL blocked by security policy: {}", url) }
      // inject non-dismissible trust bar CSS/HTML into page
      self.webview.load_url(url.as_str());
      Ok(())
    }
    fn hide(&mut self) -> anyhow::Result<()> { /* hide window */ Ok(()) }
    fn navigate(&mut self, url: url::Url) -> anyhow::Result<()> {
      if !security::is_url_allowed(&url) { anyhow::bail!("URL blocked: {}", url) }
      self.webview.load_url(url.as_str());
      Ok(())
    }
    fn position(&mut self, x: f32, y: f32, width: f32, height: f32) -> anyhow::Result<()> {
      // position wry overlay window using EventLoopProxy
      Ok(())
    }
  }

Implement WebViewPlugin::build():
  - Register BrowserCommand as Bevy Event
  - Register BrowserEvent as Bevy Event
  - Add webview_position_system to PostUpdate:
      query: Model AABBs → Camera::world_to_viewport() → update overlay position
  - Add webview_command_system to Update:
      read EventReader<BrowserCommand>, dispatch to WryBrowserSurface
  - Add WebViewConfig as Bevy Resource

Two-tab interface logic:
  BrowserTab::ExternalUrl — navigate to WebViewConfig::initial_url (role-gated: public can view)
  BrowserTab::Config — admin-only config panel (check role == "admin")
  Tab switching via BrowserCommand::SwitchTab

IPC handler — exhaustive match, NO wildcard arm:
  match cmd {
    BrowserCommand::Navigate { url } => self.navigate(url),
    BrowserCommand::Close => self.hide(),
    BrowserCommand::GetUrl => { /* emit BrowserEvent::UrlChanged */ Ok(()) },
    BrowserCommand::SwitchTab { tab } => { /* switch tabs */ Ok(()) },
  }

Trust bar injection — inject this HTML before page renders:
  const TRUST_BAR_JS: &str = r#"
    const bar = document.createElement('div');
    bar.style.cssText = 'position:fixed;top:0;left:0;right:0;height:24px;background:#1a1a2e;color:#8888cc;font-family:sans-serif;font-size:11px;padding:4px 8px;z-index:2147483647;pointer-events:none;';
    bar.textContent = '🌐 External Website: ' + location.hostname;
    document.body.prepend(bar);
  "#;

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- Update PLAYBOOK.md progress table: change Sprint 5A ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 5B — Fractal Mesh Full Implementation (Track 08)

**Plugin:** `/ultrawork` in a git worktree on branch `track/fractal-mesh-impl`

```
Implement Track 08: Fractal Mesh for FractalEngine — fill in all todo!() stubs.
Branch from main after Sprint 4 merge. Work in fe-sync crate.

Read the plan at: conductor/tracks/fractal_mesh_20260321/plan.md

EXISTING CODE (Sprint 4D scaffold):
- fe-sync/src/replication.rs — IrohDocsStore, ReplicationConfig, PetalReplica, ReplicationStore trait
- fe-sync/src/cache.rs — CacheEntry, AssetCache stubs
- fe-sync/src/offline.rs — is_petal_available_offline(), load_petal_offline() stubs
- fe-sync/src/reconciliation.rs — reconcile_petal() stub

IMPLEMENT fe-sync/src/replication.rs — IrohDocsStore::sync():
  // On peer connect: exchange iroh-docs document heads for this Petal namespace
  // Pull missing entries from peer, push our missing entries (delta only)
  // Each Petal gets its own iroh-docs namespace: "petal_{petal_id}"
  fn sync(&self, peer_id: &str, petal_id: &fe_database::PetalId) -> anyhow::Result<Vec<u8>> {
    // CROSS-CRATE: call fe_network::iroh_docs::sync_petal_state(petal_id_str, peer_id)
    todo!("implement delta sync via fe_network::iroh_docs::sync_petal_state")
  }

IMPLEMENT fe-sync/src/cache.rs — AssetCache:
  impl AssetCache {
    pub fn new(config: ReplicationConfig) -> Self {
      Self { entries: HashMap::new(), config }
    }
    pub fn touch(&mut self, asset_id: AssetId) -> anyhow::Result<()> {
      if let Some(entry) = self.entries.get_mut(&asset_id) {
        entry.last_accessed = std::time::Instant::now();
      }
      Ok(())
    }
    pub fn evict_expired(&mut self) -> anyhow::Result<()> {
      let threshold = std::time::Duration::from_secs(self.config.eviction_days * 86400);
      self.entries.retain(|_, e| e.last_accessed.elapsed() < threshold);
      // For evicted entries: delete asset file from disk cache, delete from SurrealDB asset table
      Ok(())
    }
    pub fn total_size_bytes(&self) -> u64 {
      self.entries.values().map(|e| e.size_bytes).sum()
    }
    pub fn should_evict(&self) -> bool {
      let max_bytes = (self.config.max_cache_gb * 1024.0 * 1024.0 * 1024.0) as u64;
      self.total_size_bytes() > max_bytes
    }
  }

IMPLEMENT fe-sync/src/offline.rs:
  pub fn is_petal_available_offline(petal_id: &PetalId) -> bool {
    let namespace_path = local_namespace_path(petal_id);
    namespace_path.exists()
  }
  fn local_namespace_path(petal_id: &PetalId) -> std::path::PathBuf {
    dirs::data_local_dir()
      .unwrap_or_else(|| std::path::PathBuf::from("."))
      .join("fractalengine")
      .join("replicas")
      .join(petal_id.0.to_string())
  }
  pub async fn load_petal_offline(petal_id: &PetalId, db_handle: &DbHandle) -> anyhow::Result<serde_json::Value> {
    // Query local replica namespace
    let namespace = format!("petal_replica_{}", petal_id.0);
    let result = db_handle.0.use_ns("fractalengine").use_db(&namespace).await;
    // return data + metadata indicating it's stale
    Ok(serde_json::json!({ "data": {}, "stale": true, "petal_id": petal_id.0.to_string() }))
  }

IMPLEMENT fe-sync/src/reconciliation.rs:
  pub async fn reconcile_petal(petal_id: &PetalId, peer_id: &str, store: &dyn ReplicationStore, db_handle: &DbHandle) -> anyhow::Result<u64> {
    let delta_bytes = store.sync(peer_id, petal_id)?;
    // Parse delta bytes as op-log entries, replay into local SurrealDB namespace
    // Return count of entries applied
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&delta_bytes).unwrap_or_default();
    Ok(entries.len() as u64)
  }

WRITE TESTS in fe-sync/tests/:
  test_cache_eviction_by_age: add entry with old last_accessed, call evict_expired → removed
  test_cache_size_threshold: fill cache beyond max_cache_gb, assert should_evict() == true
  test_offline_detection: create namespace dir → is_petal_available_offline returns true
  test_reconcile_empty_delta: empty delta → returns 0 entries applied

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- Update PLAYBOOK.md progress table: change Sprint 5B ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 5C — Gardener Console (Track 09)

**Plugin:** `/ultrawork` in a git worktree on branch `track/gardener-console-impl`

```
Implement Track 09: Gardener Console for FractalEngine — bevy_egui 0.39 admin dashboard.
Branch from main after Sprint 4 merge. Create fe-ui crate (replace stub).

Read the plan at: conductor/tracks/gardener_console_20260321/plan.md

Create fe-ui/Cargo.toml:
  [package] name = "fe-ui" version = "0.1.0"
  [dependencies]
  bevy = { workspace = true }
  bevy_egui = { version = "0.39", workspace = true }
  fe-database = { path = "../fe-database" }
  fe-runtime = { path = "../fe-runtime" }
  serde = { workspace = true }

Create fe-ui/src/lib.rs:
  pub mod plugin;
  pub mod panels;
  pub mod role_chip;

CREATE fe-ui/src/plugin.rs:
  pub struct GardenerConsolePlugin;
  impl bevy::prelude::Plugin for GardenerConsolePlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
      app.add_plugins(bevy_egui::EguiPlugin);
      app.add_systems(bevy::prelude::Update, (
        panels::node_status_panel,
        panels::petal_list_panel,
        panels::peer_manager_panel,
        panels::role_editor_panel,
        panels::asset_browser_panel,
        panels::session_monitor_panel,
        role_chip::role_chip_hud,
      ));
    }
  }

CREATE fe-ui/src/panels.rs:
  use bevy_egui::egui;

  pub fn node_status_panel(mut ctx: bevy_egui::EguiContexts) {
    egui::Window::new("Node Status").show(ctx.ctx_mut(), |ui| {
      ui.label("Thread Status");
      ui.horizontal(|ui| { ui.label("Bevy:"); ui.colored_label(egui::Color32::GREEN, "●"); ui.label("Running"); });
      ui.horizontal(|ui| { ui.label("Network:"); ui.colored_label(egui::Color32::GREEN, "●"); ui.label("Running"); });
      ui.horizontal(|ui| { ui.label("Database:"); ui.colored_label(egui::Color32::GREEN, "●"); ui.label("Running"); });
      ui.separator();
      ui.label("Peers connected: —");
      ui.label("Frame time: —");
    });
  }

  pub fn petal_list_panel(mut ctx: bevy_egui::EguiContexts) {
    egui::Window::new("Petals").show(ctx.ctx_mut(), |ui| {
      egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label("No petals loaded");
        // TODO: iterate over DbResult petal list from channel
      });
    });
  }

  pub fn peer_manager_panel(mut ctx: bevy_egui::EguiContexts) {
    egui::Window::new("Peer Manager").show(ctx.ctx_mut(), |ui| {
      ui.label("Connected peers: —");
      // Role chip per peer, session expiry, kick action
    });
  }

  pub fn role_editor_panel(mut ctx: bevy_egui::EguiContexts) {
    egui::Window::new("Role Editor").show(ctx.ctx_mut(), |ui| {
      ui.label("Custom Roles");
      if ui.button("+ New Role").clicked() { /* send DbCommand to create role */ }
      // list existing roles with rename/delete
    });
  }

  pub fn asset_browser_panel(mut ctx: bevy_egui::EguiContexts) {
    egui::Window::new("Asset Browser").show(ctx.ctx_mut(), |ui| {
      ui.label("Assets (BLAKE3-addressed)");
      egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label("No assets");
        // show BLAKE3 hash, size, filename, delete action
      });
    });
  }

  pub fn session_monitor_panel(mut ctx: bevy_egui::EguiContexts) {
    egui::Window::new("Sessions").show(ctx.ctx_mut(), |ui| {
      ui.label("Active sessions");
      // JWT expiry countdown, revoke button per session
    });
  }

CREATE fe-ui/src/role_chip.rs:
  pub fn role_chip_hud(mut ctx: bevy_egui::EguiContexts) {
    // Small persistent chip in bottom-right viewport corner
    // Shows current peer's role. Click to expand compact permissions summary.
    // NEVER animates — functional minimalism.
    bevy_egui::egui::Area::new("role_chip".into())
      .anchor(bevy_egui::egui::Align2::RIGHT_BOTTOM, bevy_egui::egui::vec2(-12.0, -12.0))
      .show(ctx.ctx_mut(), |ui| {
        ui.add(bevy_egui::egui::Button::new("admin") // TODO: read from NodeIdentity resource
          .fill(bevy_egui::egui::Color32::from_rgb(30, 30, 50)));
      });
  }

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- All panels are functional stubs with correct structure — data binding wired in Wave 6.
- Update PLAYBOOK.md progress table: change Sprint 5C ⬜ Pending to ✅ Done when complete.
```

---

### Prompt 5D — Thorns and Shields (Track 10)

**Plugin:** `/ultrawork` in a git worktree on branch `track/thorns-shields-impl`

```
Implement Track 10: Thorns and Shields for FractalEngine — security hardening + pre-launch docs.
Branch from main after Sprint 4 merge.

Read the plan at: conductor/tracks/thorns_shields_20260321/plan.md

FIRST — check if docs/webview-threat-model.md exists (Sprint 5A may have written it).
If it does NOT exist, write it now (same content as specified in Sprint 5A prompt).

CREATE docs/security-checklist.md:
  # FractalEngine Pre-Launch Security Checklist

  ## Operator Checklist (Run before opening to public peers)
  - [ ] Rotate the Node's ed25519 keypair (if previously used in testing)
  - [ ] Verify SURREAL_SYNC_DATA=true is set in the environment
  - [ ] Confirm no default admin passwords exist (ed25519 key IS the admin credential)
  - [ ] Test WebView denylist: navigate to http://127.0.0.1 → must be blocked
  - [ ] Verify iroh relay URL is configured (or mDNS-only for LAN-only mode)
  - [ ] Set JWT lifetime to ≤300s in production config
  - [ ] Confirm session TTL cache is flushed on node restart
  - [ ] Audit custom roles: no role should have permissions beyond its intended scope
  - [ ] Run: cargo audit — zero high-severity vulnerabilities before launch

  ## Developer Checklist (Run before each release)
  - [ ] cargo audit — zero high/critical CVEs
  - [ ] cargo clippy -- -D warnings — clean
  - [ ] grep -rn 'unwrap()\|expect(' src/ --include='*.rs' | grep -v '#\[cfg(test)\]' — empty
  - [ ] grep -rn 'block_on' crates/ --include='*.rs' — empty (no Tokio blocking in Bevy)
  - [ ] All gossip inbound paths call verify() before any logic
  - [ ] docs/webview-threat-model.md up to date

CREATE scripts/audit.sh:
  #!/usr/bin/env bash
  set -e
  echo "=== cargo audit ==="
  cargo audit
  echo "=== unwrap/expect audit ==="
  HITS=$(grep -rn 'unwrap()\|\.expect(' --include='*.rs' \
    fe-runtime fe-identity fe-database fe-network fe-renderer fe-auth fe-webview fe-sync fe-ui fractalengine \
    | grep -v '#\[cfg(test)\]' | grep -v '// SAFETY' | wc -l)
  echo "Production unwrap/expect occurrences: $HITS"
  if [ "$HITS" -gt "0" ]; then echo "FAIL: unwrap/expect found in production code"; exit 1; fi
  echo "=== block_on audit ==="
  if grep -rn 'block_on' --include='*.rs' fe-runtime fractalengine fe-ui; then
    echo "FAIL: block_on found in Bevy crates"; exit 1
  fi
  echo "All checks passed."

CREATE docs/unwrap-audit.md:
  # Unwrap Audit Report
  Generated: 2026-03-21 (pre-implementation — update after Wave 6)
  Status: PENDING — run scripts/audit.sh after Wave 6 compilation

  ## Known Intentional panics (ALLOWED — with justification)
  - fe-database/src/lib.rs: .expect("SurrealDB init") — process cannot continue without DB

  ## All others must be eliminated before launch.

CREATE fuzz/targets/jwt_parse.rs:
  #![no_main]
  use libfuzzer_sys::fuzz_target;
  fuzz_target!(|data: &[u8]| {
    if let Ok(token) = std::str::from_utf8(data) {
      // Verify that parsing never panics — only returns Ok or Err
      let fake_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]);
      if let Ok(key) = fake_key {
        let _ = fe_identity::jwt::verify_session_token(token, &key);
      }
    }
  });

CREATE fuzz/targets/ed25519_verify.rs:
  #![no_main]
  use libfuzzer_sys::fuzz_target;
  fuzz_target!(|data: &[u8]| {
    if data.len() >= 96 {
      let pub_key_bytes: [u8; 32] = data[0..32].try_into().unwrap();
      let sig_bytes: [u8; 64] = data[32..96].try_into().unwrap();
      let message = &data[96..];
      if let Ok(pub_key) = ed25519_dalek::VerifyingKey::from_bytes(&pub_key_bytes) {
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        // Must not panic — only Ok or Err
        let _ = pub_key.verify_strict(message, &sig);
      }
    }
  });

CREATE fuzz/Cargo.toml:
  [package] name = "fractalengine-fuzz" version = "0.0.1"
  [dependencies]
  libfuzzer-sys = "0.4"
  fe-identity = { path = "../fe-identity" }
  ed25519-dalek = { workspace = true }
  [[bin]] name = "jwt_parse" path = "targets/jwt_parse.rs"
  [[bin]] name = "ed25519_verify" path = "targets/ed25519_verify.rs"

WRITE TESTS in tests/security_harness.rs (at workspace root):
  test_tampered_ed25519_signature: sign message, flip sig byte 0, verify_strict → Err
  test_expired_jwt: mint ttl=0 (or mock time), verify → Err
  test_rfc1918_url_blocked: all RFC 1918 ranges blocked via fe_webview::security::is_url_allowed
  test_oversized_asset_rejected: pass 256MB+1 bytes to GltfIngester::ingest → Err
  test_role_cache_invalidated_after_revoke: add to SessionCache, revoke, get → None

RULES:
- Write code only. Do NOT run any cargo/rustc commands.
- Do NOT create git commits.
- Mark each completed plan task: change `- [ ]` to `- [~]`.
- Update PLAYBOOK.md progress table: change Sprint 5D ⬜ Pending to ✅ Done when complete.
```

### Post-Sprint 5 — Merge

```bash
git checkout main
git merge track/canopy-view-impl
git merge track/fractal-mesh-impl
git merge track/gardener-console-impl
git merge track/thorns-shields-impl
```

---

## WAVE 6 — Validation (Run sequentially)

> **Dependency gate:** All sprints merged to main. Now cargo commands are ALLOWED.

### Wave 6.1 — First Compilation Fix

**Plugin:** `/ultrawork`

```
Fix all compilation errors in the FractalEngine Cargo workspace.

Run: cargo build 2>&1 | tee /tmp/fractal-build.log

Fix compilation errors in dependency order (stop at each before moving to the next):
  1. fe-runtime        — channel types, Bevy resources
  2. fe-identity       — ed25519, JWT, keyring
  3. fe-database       — SurrealDB, types, RBAC
  4. fe-network        — libp2p, iroh-blobs, iroh-gossip, iroh-docs
  5. fe-renderer       — bevy_gltf, blake3, AssetIngester
  6. fe-auth           — depends on fe-identity, fe-database, fe-network
  7. fe-webview        — wry, security, IPC
  8. fe-sync           — iroh-docs replication, cache
  9. fe-ui             — bevy_egui panels
  10. fractalengine    — binary crate wiring all crates together

For each error: read the error, find the root cause, fix with minimal change.
Common issues to expect:
  - CROSS-CRATE type mismatches (types referenced before implementation)
  - Missing trait implementations (Debug, Clone, Send, Sync)
  - Lifetime issues in async functions
  - iroh/libp2p API changes from expected versions

Goal: cargo build succeeds with zero errors.
Do NOT fix clippy warnings yet — only compilation errors.
Commit each crate fix: fix(<crate>): Resolve <description>
```

---

### Wave 6.2 — Lint Pass

**Plugin:** `/ultrawork`

```
Fix all formatting and clippy warnings in FractalEngine workspace.

Run in order:
  1. cargo fmt --all                          ← format all crates
  2. cargo clippy --workspace -- -D warnings  ← fix ALL warnings (treated as errors)

Re-run both until they pass clean.

Common clippy issues to expect:
  - unused imports from scaffold stubs
  - missing Default implementations
  - inefficient clone patterns
  - todo!() in non-test code (address with proper implementation or document)

Commit: style(workspace): Fix cargo fmt and clippy warnings across all crates
```

---

### Wave 6.3 — Per-Crate Test Execution

**Plugin:** `/ralph`

```
Execute and fix tests for FractalEngine workspace, crate by crate in dependency order.

For each crate: run tests, diagnose failures, fix implementation, re-run. Circuit breaker:
if a crate still fails after 3 fix attempts, flag it with a comment and move to the next crate.

Execution order:
  cargo test -p fe-runtime       # channel types, drain systems
  cargo test -p fe-identity      # keypair gen, JWT, did:key
  cargo test -p fe-database      # RBAC, op-log, schema
  cargo test -p fe-network       # gossip sig verify, asset roundtrip
  cargo test -p fe-renderer      # GLB validation, BLAKE3 hash, size limit
  cargo test -p fe-auth          # session cache TTL, revocation, JWT verification
  cargo test -p fe-webview       # denylist (all RFC 1918 ranges), IPC enums
  cargo test -p fe-sync          # cache eviction, offline detection, reconciliation
  cargo test -p fe-ui            # egui panels compile (no interaction tests at unit level)
  cargo test -p fractalengine    # binary integration: thread startup, Ping/Pong roundtrip

Required passing assertions (from Track 01 spec):
  ✓ fe-runtime: Handle::try_current() returns Err on main thread
  ✓ fe-runtime: NetworkCommand::Ping → NetworkEvent::Pong within 2ms
  ✓ fe-runtime: DbCommand::Ping → DbResult::Pong within 5ms
  ✓ fe-identity: tampered message rejected by verify_strict
  ✓ fe-database: public role cannot write
  ✓ fe-network: unsigned/tampered gossip messages rejected
  ✓ fe-webview: all RFC 1918 ranges + localhost blocked
  ✓ fe-auth: revocation invalidates SessionCache before function returns

Commit per crate fix: fix(<crate>): Fix test failures — <description>
```

---

### Wave 6.4 — Integration Tests

**Plugin:** `/ultraqa`

```
Run full workspace integration test suite and fix all failures.

Run: cargo test --workspace 2>&1

Required assertions (from specs):
  ✓ Thread isolation: no ambient Tokio runtime on Bevy thread
  ✓ Network round-trip: NetworkCommand::Ping → NetworkEvent::Pong < 2ms
  ✓ DB round-trip: DbCommand::Ping → DbResult::Pong < 5ms
  ✓ Frame budget: no frame > 20ms under 300-frame synthetic load
  ✓ Clean shutdown: both threads exit within 1s on Shutdown
  ✓ RBAC: public cannot write, custom role scoped, admin full CRUD
  ✓ Crypto: tampered signatures rejected, verify_strict used everywhere
  ✓ WebView: RFC 1918 + localhost blocked by security layer
  ✓ JWT: 300s max lifetime enforced, revocation propagates within TTL
  ✓ Asset ingestion: >256MB rejected, non-GLB magic rejected
  ✓ BLAKE3: content addressing deterministic and collision-resistant

Report all passing/failing tests with root causes.
Fix failures. Re-run until all pass.
Commit: test(workspace): All integration tests passing
```

---

### Wave 6.5 — Coverage + Final Quality Gate

**Plugin:** `/ultraqa`

```
Run the FractalEngine final quality gate. ALL of the following must pass:

1. cargo fmt --check
   → must exit 0 (no formatting errors)

2. cargo clippy --workspace -- -D warnings
   → must exit 0 (no warnings)

3. cargo test --workspace
   → must exit 0 (all tests pass)

4. cargo tarpaulin --workspace --out Html --output-dir coverage/
   → report coverage per crate; flag any crate below 80%

5. grep -rn 'unwrap()\|\.expect(' fe-runtime/src fe-identity/src fe-database/src \
      fe-network/src fe-renderer/src fe-auth/src fe-webview/src fe-sync/src fe-ui/src \
      fractalengine/src --include='*.rs' | grep -v '#\[cfg(test)\]'
   → must be EMPTY (zero unwrap/expect in production paths)
   → EXCEPTION: .expect("SurrealDB init") in fe-database/src/lib.rs is intentional — document it

6. grep -rn 'block_on' fe-runtime/src fractalengine/src fe-ui/src --include='*.rs'
   → must be EMPTY (no Tokio blocking in Bevy threads)

7. bash scripts/audit.sh
   → must exit 0 (cargo audit clean, no high CVEs)

For each failure: report file:line, root cause, fix applied.
Final commit: chore(workspace): All quality gates passing — FractalEngine v0.1.0 ready
```

---

## Merge Conflict Protocol

The primary conflict zone is `Cargo.toml` (workspace root). Mitigated by Sprint 1 pre-declaring all workspace members.

If conflicts occur:

```bash
# Cargo.toml: always keep the SUPERSET of [workspace.members] and [workspace.dependencies]
# Use the later sprint's pinned versions (they have the most complete set)

# plan.md conflicts: keep both sets of [~] tasks — Wave 6 will reconcile

# src/ conflicts: read both versions, use the more complete implementation
# (later sprint's implementation wins if it subsumes the earlier one)
```

---

*Playbook version: 2026-03-21 | FractalEngine v0.1.0*
*Stack: Bevy 0.18 · SurrealDB 3.0 · libp2p 0.56 · iroh · wry 0.54 · ed25519-dalek 2.2*
