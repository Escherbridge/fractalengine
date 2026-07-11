# Track: Realtime API Gateway — MCP + REST + WebSocket for External Access

**Created:** 2026-04-27
**Status:** Draft
**Priority:** P1
**Depends on:** Wave 1 complete (Root Identity, Petal Soil, Petal Gate, Fractal Mesh)
**Blocks:** IoT Integration, AI Agent Framework, External SDK

---

## Problem Statement

FractalEngine is a desktop-only Bevy application with zero external API surface. The only network interfaces are iroh P2P (for peer sync) and libp2p (for discovery). There is no way for:

- AI agents to query or manipulate the 3D scene
- IoT devices to stream sensor data into node transforms
- External tools to perform CRUD on the verse/fractal/petal/node hierarchy
- Web dashboards to observe real-time scene changes

This track adds a **unified API gateway** (`fe-api` crate) that exposes MCP tools, REST endpoints, and WebSocket streams on a single port, authenticated via scoped API tokens and gated by the existing RBAC hierarchy.

---

## Goals

1. External clients can authenticate and perform hierarchy CRUD via REST
2. AI agents can discover and invoke MCP tools to manipulate the scene
3. Real-time transform updates stream to/from WebSocket subscribers
4. API tokens are issued from the node inspector UI, scoped to any hierarchy level
5. Architecture supports future IoT (MQTT) and WebTransport upgrades

## Non-Goals (this track)

- MQTT broker integration (deferred to IoT track)
- WebTransport upgrade path (deferred)
- Multi-node MCP mesh (deferred)
- Client SDKs (TypeScript, Python)

---

## Architecture Overview

### New Crate: `fe-api`

A new workspace member that depends on `fe-runtime` (messages), `fe-identity` (auth), `fe-database` (types, role_level, scope). It contains:

- **axum router** serving REST, MCP, and WebSocket on one TCP listener
- **rmcp MCP server** mounted at `/mcp` using streamable HTTP transport
- **WebSocket handler** at `/ws` with auth handshake and subscription management
- **Auth middleware** verifying `ApiClaims` tokens and resolving RBAC roles

### Threading Model

The API server runs on a **dedicated OS thread** with a `tokio::runtime::Builder::new_multi_thread()` runtime (2-4 workers). It communicates with Bevy via:

- `crossbeam::channel::bounded<ApiCommand>` — commands from API to Bevy
- `tokio::sync::broadcast<TransformUpdate>` — transform fan-out (data plane)

Bevy remains the central coordinator. API commands flow through Bevy to the DB thread — no direct API-to-DB channel.

### Auth Model

Two token types coexist:

| Token | Type | Max TTL | Scope | Purpose |
|-------|------|---------|-------|---------|
| `FractalClaims` | Session | 300s | petal_id | P2P handshake, webview |
| `ApiClaims` | API | 30 days | scope string | REST, MCP, WebSocket |

`ApiClaims` includes: `sub` (DID), `token_type: "api"`, `scope` (hierarchical string), `max_role`, `jti` (for revocation), `iat`, `exp`.

---

## Phases

### Phase 1: API Foundation (P1-core)

**New files:**
- `fe-api/Cargo.toml` — depends on axum, tokio, rmcp, serde_json, fe-runtime, fe-identity, fe-database
- `fe-api/src/lib.rs` — `spawn_api_thread(config, channels) -> JoinHandle`
- `fe-api/src/server.rs` — axum Router with `/api/v1/...`, `/mcp`, `/ws`
- `fe-api/src/auth.rs` — `ApiClaims` type, verification middleware, scope enforcement
- `fe-api/src/bridge.rs` — `ApiCommand` enum with oneshot reply channels
- `fe-api/src/types.rs` — DTO types (never expose DbCommand/DbResult directly)

**Modified files:**
- `Cargo.toml` — add `fe-api` to workspace members
- `fe-identity/src/jwt.rs` — add `ApiClaims`, `mint_api_token()`, `verify_api_token()`
- `fe-runtime/src/messages.rs` — add `ApiCommand`, `TransformUpdate`, `DbCommand::MintApiToken`
- `fractalengine/src/main.rs` — create API channels, spawn API thread

**Deliverables:**
- [ ] `ApiClaims` JWT with scope field, configurable TTL (up to 30 days)
- [ ] `spawn_api_thread()` with multi-thread tokio runtime
- [ ] `ApiCommand` enum with `oneshot::Sender<DbResult>` reply channels
- [ ] Bevy `drain_api_commands` system that forwards to DB channel
- [ ] `PendingApiRequests` resource for correlation tracking

### Phase 2: REST + MCP Endpoints (P2-endpoints)

**New files:**
- `fe-api/src/rest.rs` — REST endpoint handlers
- `fe-api/src/mcp.rs` — MCP tool definitions via `#[tool]` macro

**Deliverables:**
- [ ] REST: `GET /api/v1/hierarchy` — returns full verse tree
- [ ] REST: `POST /api/v1/verses`, `POST /api/v1/nodes`, etc. — hierarchy CRUD
- [ ] REST: `PATCH /api/v1/nodes/{id}/transform` — update transform
- [ ] REST: `GET /api/v1/nodes/{id}/transform` — read transform
- [ ] MCP: `create_verse`, `create_fractal`, `create_petal`, `create_node` tools
- [ ] MCP: `update_transform`, `get_hierarchy`, `search_nodes` tools
- [ ] MCP: `assign_role`, `revoke_role` tools (Manager+)
- [ ] MCP: `fractal://` resource URIs for hierarchy browsing
- [ ] Auth middleware: scope check + role check on every request
- [ ] DTO layer: API types decoupled from internal DbCommand/DbResult

### Phase 3: WebSocket Streaming (P3-realtime)

**New files:**
- `fe-api/src/ws.rs` — WebSocket handler, subscription manager, auth handshake

**Modified files:**
- `fe-ui/src/node_manager.rs` — publish to transform broadcast channel on drag commit
- `fe-runtime/src/app.rs` — add `TransformBroadcastSender` Bevy resource

**Deliverables:**
- [ ] WebSocket auth handshake: `auth_required` → `auth` → `auth_ok`
- [ ] Subscribe/unsubscribe by petal_id with optional throttle_ms
- [ ] Transform broadcast channel (`tokio::broadcast<TransformUpdate>`)
- [ ] Per-subscription scope check via `resolve_role`
- [ ] Inbound transform writes via WebSocket (Editor+ role)
- [ ] Heartbeat/ping with configurable timeout
- [ ] Backpressure handling: drop oldest on `Lagged`

### Phase 4: Token UI + Rate Limiting (P4-hardening)

**Modified files:**
- `fe-ui/src/panels.rs` — add "API Tokens" tab to inspector
- `fe-database/src/lib.rs` — handle `DbCommand::MintApiToken`, `RevokeApiToken`

**Deliverables:**
- [ ] Node inspector UI: generate API token (select scope, role, TTL)
- [ ] Node inspector UI: list and revoke active tokens
- [ ] `api_token` SurrealDB table for revocation tracking
- [ ] Per-identity token bucket rate limiter in API middleware
- [ ] Separate channel for high-frequency transforms (not shared with DB commands)
- [ ] Request trace ID propagation across thread boundaries

---

## SurrealDB Schema Additions

```sql
DEFINE TABLE api_token SCHEMAFULL;
DEFINE FIELD jti ON api_token TYPE string;
DEFINE FIELD scope ON api_token TYPE string;
DEFINE FIELD max_role ON api_token TYPE string;
DEFINE FIELD created_at ON api_token TYPE datetime;
DEFINE FIELD expires_at ON api_token TYPE datetime;
DEFINE FIELD revoked ON api_token TYPE bool DEFAULT false;
DEFINE FIELD label ON api_token TYPE option<string>;
DEFINE INDEX idx_api_token_jti ON api_token FIELDS jti UNIQUE;

DEFINE TABLE device_binding SCHEMAFULL;
DEFINE FIELD device_id ON device_binding TYPE string;
DEFINE FIELD node_id ON device_binding TYPE string;
DEFINE FIELD scope ON device_binding TYPE string;
DEFINE FIELD created_at ON device_binding TYPE datetime;
DEFINE INDEX idx_device_binding_device ON device_binding FIELDS device_id UNIQUE;
```

---

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Server placement | Dedicated OS thread | Matches DB/sync thread pattern; no Bevy frame coupling |
| Tokio runtime | multi_thread (2-4 workers) | axum needs concurrent HTTP; DB/sync use current_thread |
| API-to-DB path | Through Bevy coordinator | Preserves single-writer invariant; prevents race conditions |
| Correlation | oneshot::Sender in ApiCommand | Non-blocking typed response routing; no shared result queue |
| Transform streaming | tokio::broadcast, bypass DB | Sub-frame latency; DB writes at decimated rate (10 Hz) |
| MCP transport | Streamable HTTP | Current spec (2025-03-26); shares axum router |
| Auth | Separate ApiClaims type | Preserves FractalClaims 300s security; adds scope field |
| Conflict model | Server-authoritative LWW | Matches single-DB-writer; no CRDT complexity |
| MQTT | Deferred (Phase 3 of IoT track) | WebSocket covers initial needs; avoid 3-protocol burden |

---

## Risk Mitigations

| Risk | Mitigation |
|------|------------|
| Channel saturation (IoT floods) | Separate transform broadcast channel; per-client rate limits |
| Cross-verse token misuse | ApiClaims includes `scope` field; middleware validates scope ancestry |
| Stale transforms after offline | Include Bevy tick in every message; clients detect gaps |
| MCP spec changes | Abstract behind trait; swap implementations |
| DB write amplification | Coalescing buffer: transforms persist at 10 Hz, not per-frame |
| WebSocket zombie connections | 30s ping timeout; cleanup on disconnect |

---

## Testing Strategy

- **Unit tests:** ApiClaims mint/verify, scope parsing, DTO conversions
- **Integration tests:** API thread ↔ DB thread round-trip via TestPeer
- **WebSocket tests:** Connect, auth, subscribe, receive transform, disconnect
- **MCP tests:** Tool invocation via rmcp test client
- **Load tests:** 100 concurrent WebSocket clients, 60 Hz transforms
- **Security tests:** Cross-verse token rejection, expired token rejection, role escalation prevention

---

## Dependencies (Cargo)

```toml
[dependencies]
axum = { version = "0.8", features = ["ws"] }
tokio = { version = "1", features = ["full"] }
rmcp = { version = "1.3", features = ["server", "transport-streamable-http", "macros"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
crossbeam = "0.8"
tracing = "0.1"
fe-runtime = { path = "../fe-runtime" }
fe-identity = { path = "../fe-identity" }
fe-database = { path = "../fe-database" }
```

---

## Success Criteria

1. An AI agent (Claude via MCP) can create a verse, add nodes, and read the hierarchy
2. A WebSocket client can subscribe to a petal and receive transform updates at 60 Hz
3. API tokens scoped to a verse cannot access other verses
4. Rate limiting prevents a single client from degrading UI responsiveness
5. The API server adds <5ms latency to transform propagation
