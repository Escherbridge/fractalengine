# Mobile Architecture Strategy

> **Status:** v2 planning document (mobile is a v2 non-goal per product.md)
> **Last updated:** 2026-05-08

## Executive Summary

FractalEngine's mobile strategy is a **thin-client relay architecture**: the mobile app is a lightweight REST/WS client that connects to a FractalEngine relay server, which handles all P2P networking, database operations, and data synchronization. This avoids shipping SurrealDB, libp2p, and iroh on mobile devices, keeping the mobile binary under 30 MB.

## Architecture Overview

```
+---------------------------------------------+
|  Mobile App (thin-client)                    |
|  - HTTP/WS client (talks to relay)          |
|  - Local credential storage (platform API)  |
|  - Optional: local entity cache             |
|  - Native UI (Swift/Kotlin/cross-platform)  |
|  - WebView for Petal Portal overlays        |
+---------------------------------------------+
              | REST/WS (TLS)
              v
+---------------------------------------------+
|  FractalEngine Relay (fe-relay binary)       |
|  - API gateway (axum): REST + WS + MCP      |
|  - SurrealDB (embedded, full query engine)  |
|  - libp2p (DHT peer discovery)              |
|  - iroh (P2P sync, blob transfer, gossip)   |
|  - RBAC enforcement at DB layer             |
|  - JWT session management                   |
+---------------------------------------------+
              | P2P (QUIC)
              v
        Other FractalEngine Nodes
```

## API Surface (What Mobile Consumes)

The relay already exposes all APIs needed for a mobile client:

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/v1/health` | GET | Health check |
| `/ready` | GET | Readiness probe |
| `/api/v1/hierarchy` | GET | Full verse/fractal/petal/node tree |
| `/api/v1/verses` | POST | Create verse |
| `/api/v1/fractals` | POST | Create fractal |
| `/api/v1/petals` | POST | Create petal |
| `/api/v1/nodes` | POST | Create node |
| `/api/v1/nodes/:id/transform` | GET/PATCH | Read/update transform |
| `/api/v1/nodes/:id/properties` | GET/PATCH | Read/update properties |
| `/api/v1/nodes/:id/properties/:key` | DELETE | Delete property |
| `/api/v1/petals/:id/export` | GET | Export petal as .fractal |
| `/api/v1/petals/:id/import` | POST | Import petal from .fractal |
| `/api/v1/assets/:hash` | GET | Download asset by BLAKE3 hash |
| `/api/v1/query` | POST | SELECT queries (viewer+) |
| `/api/v1/query/elevated` | POST | Mutation queries (manager+) |
| `/ws` | WS | Scene streaming + transform updates |
| `/mcp` | POST | MCP tool interface |

**Authentication flow:**
1. Mobile generates ed25519 keypair on first launch, stores in platform keychain
2. Mobile presents public key to relay
3. Relay issues JWT session (`sub: did:key:z6Mk<multibase_pub>`)
4. All subsequent requests include JWT in `Authorization: Bearer <token>` header

## What the Mobile Client Needs

| Component | Purpose | Suggested Implementation |
|-----------|---------|------------------------|
| HTTP client | REST API calls | `reqwest` (Rust) or platform-native |
| WebSocket client | Scene streaming, transform updates | `tokio-tungstenite` or platform-native |
| ed25519 keypair | Identity, JWT auth | `ed25519-dalek` (lightweight, ~1 MB) |
| Platform keychain | Secure key storage | Android Keystore / iOS Keychain |
| WebView | Petal Portal browser overlays | Platform-native (Android WebView / WKWebView) |
| Local cache (optional) | Offline entity snapshots | SQLite or `redb` (not SurrealDB) |

**Estimated mobile binary size:** 15-30 MB

## What Mobile Does NOT Need

These components are handled by the relay and should NOT be linked into the mobile binary:

| Component | Size Contribution | Reason to Exclude |
|-----------|-------------------|-------------------|
| SurrealDB | ~35-50 MB | Full SQL engine + RocksDB — relay handles all queries |
| libp2p | ~15-25 MB | DHT peer discovery — relay handles discovery |
| iroh (blobs, gossip, docs) | ~25-35 MB | P2P sync — relay handles replication |
| Bevy | ~5-150 MB | ECS + rendering — mobile uses native UI |
| tokio (full) | ~15-25 MB | Can use lighter async runtime or platform async |

## Platform Considerations

### Android

- **APK size limit:** 150 MB (Play Store)
- **AAB size limit:** 200 MB (Play Store, recommended)
- **User expectation:** 50-100 MB
- **Keychain:** Android Keystore API (hardware-backed on most devices)
- **WebView:** Android WebView (Chromium-based, pre-installed)
- **Rust integration:** via JNI (`jni` crate) or Kotlin/Native interop
- **NDK targets:** `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`

### iOS

- **App Store limit:** 4 GB (generous)
- **User expectation:** 50-200 MB
- **Keychain:** iOS Keychain Services (hardware-backed Secure Enclave)
- **WebView:** WKWebView (mandatory, Safari-based)
- **Rust integration:** via C FFI + Swift bridging headers, or `uniffi`
- **Targets:** `aarch64-apple-ios`, `aarch64-apple-ios-sim`

### Cross-Platform Frameworks

If a single codebase is preferred over native Swift/Kotlin:

| Framework | Language | Bevy-free | Notes |
|-----------|----------|-----------|-------|
| Tauri Mobile | Rust + Web | Yes | Uses platform WebView, small binary |
| Flutter | Dart + Rust FFI | Yes | Mature mobile UI, Rust via `flutter_rust_bridge` |
| React Native | JS + Rust FFI | Yes | Large ecosystem, Rust via Turbo Modules |
| Kotlin Multiplatform | Kotlin | Yes | Native feel, Rust via JNI/cinterop |

## Security

- All relay communication over TLS (HTTPS/WSS)
- JWT tokens with 300s lifetime, re-validated against relay
- Private keys never leave the platform keychain
- RBAC enforced server-side at the SurrealDB layer — mobile trusts the relay's authorization decisions
- Signed revocations propagated via relay's gossip → relay pushes to mobile via WS

## Credential Storage (SecretStore Trait)

The `fe-identity` crate already has a pluggable `SecretStore` trait ([fe-identity/src/keychain.rs](../fe-identity/src/keychain.rs)):

```rust
pub trait SecretStore: Send + Sync {
    fn store_secret(&self, key: &str, value: &[u8]) -> Result<()>;
    fn load_secret(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn delete_secret(&self, key: &str) -> Result<()>;
}
```

Current implementations:
- `OsKeystoreBackend` — OS keychain (desktop, uses `keyring` crate)
- `EnvBackend` — Environment variables (relay/headless)

**Mobile implementations needed:**
- `AndroidKeystoreBackend` — wraps Android Keystore via JNI
- `IosKeychainBackend` — wraps iOS Keychain via C FFI

## Implementation Path

### Prerequisites
- Relay binary hardened and deployed (Headless Relay track)
- API gateway stable (Realtime API Gateway track)
- Release CI producing relay Docker images (Release CI track)

### Phase 1: REST Client SDK
- Extract API types from `fe-api/src/types.rs` into a shared `fe-api-types` crate
- Create `fe-mobile-sdk` with HTTP client, WS client, JWT auth
- No platform-specific code yet — test against relay from desktop

### Phase 2: Platform Integration
- Implement `AndroidKeystoreBackend` and `IosKeychainBackend`
- Set up cross-compilation for Android NDK and iOS targets
- Create minimal native app shells (Kotlin + Swift) with Rust FFI

### Phase 3: Native UI
- Build mobile-optimized UI for hierarchy browsing, node inspection
- Integrate platform WebView for Petal Portal overlays
- Implement local entity cache for offline browsing

### Phase 4: Distribution
- Android: Play Store AAB with native library
- iOS: App Store with embedded framework
- Test on physical devices across OS versions

## Dependencies

This mobile work depends on these tracks being complete:
- **Headless Relay** (in progress) — relay binary as the mobile backend
- **Realtime API Gateway** (pending) — stable API surface for mobile to consume
- **Release CI** (pending) — automated relay deployment

## Open Questions

1. **3D rendering on mobile?** If mobile needs 3D viewport (not just REST data), consider Bevy's experimental mobile support or a WebGL viewer served by the relay.
2. **Offline-first vs online-only?** Pure thin-client (online-only) is simpler. Offline-first requires local database (SQLite/redb) and sync protocol.
3. **Push notifications?** Relay could push scene changes via FCM (Android) / APNs (iOS) instead of persistent WS connection.
