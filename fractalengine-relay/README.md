# FractalEngine Relay

Headless relay server for FractalEngine. Runs without GPU, windowing, WebView, or OS keychain — designed for Linux/Docker/ARM server deployment.

Thin clients (web browsers, mobile apps) connect via the HTTP/WebSocket API to interact with the scene graph, manage entities, and stream transforms in real time.

## Quick Start

```bash
cargo build --release -p fractalengine-relay
./target/release/fe-relay
```

The relay listens on `0.0.0.0:8765` by default.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FE_BIND_ADDR` | `0.0.0.0:8765` | Listen address (host:port) |
| `FE_DB_PATH` | `data/fractalengine.db` | SurrealDB storage path |
| `FE_CORS_ORIGINS` | `*` | Comma-separated allowed CORS origins |
| `RUST_LOG` | (none) | Log level filter (`info`, `debug`, `fe_api=debug`, etc.) |

### Secret injection

The relay uses `EnvBackend` for secrets. Keys are mapped to env vars as `FE_SECRET_{SERVICE}_{ACCOUNT}` (uppercased, special chars replaced with `_`).

The node keypair is stored/loaded from `FE_SECRET_FRACTALENGINE_NODE_KEYPAIR`. If unset, the relay generates an ephemeral keypair on startup.

## Health Endpoints

| Endpoint | Auth | Description |
|----------|------|-------------|
| `GET /api/v1/health` | None | Liveness probe — always returns `200 {"status":"ok"}` |
| `GET /ready` | None | Readiness probe — returns `200` when DB is initialized, `503` otherwise |

## API Surface

The relay exposes the same API as the GUI binary:

- **REST** — CRUD for verses, fractals, petals, nodes, transforms
- **WebSocket** (`/ws`) — real-time transform streaming, scene subscriptions
- **MCP** (`POST /mcp`) — machine-callable protocol for AI agents
- **Assets** (`GET /api/v1/assets/:hash`) — content-addressed blob delivery with immutable caching

All authenticated endpoints require a Bearer JWT. Mint tokens via `MintApiToken` DB command or MCP tool.

## Graceful Shutdown

The relay handles `SIGINT` / `SIGTERM` (ctrl+c):

1. Sends `DbCommand::Shutdown` to drain in-flight DB commands
2. SurrealDB flushes to disk
3. Process exits cleanly

## Docker

See [docker/README.md](../docker/README.md) for container deployment.

```bash
docker build -f docker/Dockerfile.relay -t fractalengine-relay .
docker run -p 8765:8765 -v relay-data:/data fractalengine-relay
```

## Architecture

The relay shares all core crates with the GUI binary but excludes GPU/display dependencies:

```
Shared: fe-runtime, fe-database, fe-network, fe-sync, fe-identity, fe-auth, fe-api
GUI only: fe-renderer, fe-ui, fe-webview, bevy_egui, keyring
```

No feature flags on shared crates — the Cargo dependency graph alone controls the split. The relay uses `MinimalPlugins` + `ScheduleRunnerPlugin` instead of `DefaultPlugins`.
