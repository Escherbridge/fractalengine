---
type: track-spec
---

# Track: Pear Runtime P2P Layer SPIKE — Research: JS-Native P2P for FractalEngine

**Created:** 2026-06-30
**Status:** Draft (SPIKE / research)
**Priority:** P2
**Depends on:** none
**Blocks:** none (this is a SPIKE, research only)

---

## SPIKE Purpose

This is a **time-boxed research/spike track** to evaluate leveraging **Pear Runtime** (pears.com, by Holepunch) for FractalEngine's P2P layer.

**Core tension**: FractalEngine's existing P2P ("mycelium") is **Rust-native** (libp2p DHT + iroh transport). Pear is **JS-native** — it runs in the JavaScript context.

This spike determines whether Pear should augment or replace the existing mycelium layer.

---

## Pear Runtime Overview

Pear Runtime (pears.com, Holepunch) provides infrastructure-free P2P on the Hypercore stack:

| Component | Technology |
|-----------|------------|
| Data storage | Hypercore (append-only logs) |
| Discovery | Hyperswarm (DHT) |
| Key-value DB | Hyperbee |
| File system | Hyperdrive |
| Multi-writer | Autobase |

**Key point**: Pear runs **natively in JavaScript** (browser or Node.js). It's not a Rust library.

---

## The Connection to Track 2

Track 2 (Tauri IPC/Asset Bridge) designed a **shared node structure** that bridges Tauri↔Bevy. This structure is the "seam" where Pear will plug in:

```
┌─────────────────────────────────────────────────────────────┐
│                    Tauri WebView (JS)                       │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Pear Runtime                                        │   │
│  │    - Hypercore / Hyperswarm                          │   │
│  │    - Peer discovery                                  │   │
│  │    - Data sync                                       │   │
│  └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            ▼                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Shared Node Structure (from Track 2)               │   │
│  │    - Node data                                       │   │
│  │    - WebViewInteraction events                      │   │
│  └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            │ invoke()                        │
│                            ▼                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  #[tauri::command] handlers                         │   │
│  │    - notify_interaction()                           │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Bevy (Rust)                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  NodeManager / VerseManager                         │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

**Why this matters**: The IPC bridge from track 2 is already designed to accept external events. Pear just needs to emit the right interaction types.

---

## Current P2P Architecture (Mycelium)

```
fe-network / fe-sync:
├── libp2p DHT (peer discovery)
├── iroh transport (data sync)
└── iroh-docs (document sync)
```

This is **Rust-native**, runs in the same process as Bevy.

---

## Options Analysis

### Option A: Pear-in-Webview (Augment)

Run Pear inside the Tauri webview (JS context):
- Pear provides P2P for web-based features
- Existing mycelium layer continues for Rust-side P2P
- Bridge via the shared node structure (track 2)

**Pros**:
- Leverages the IPC bridge already designed
- Pear's JS-native nature is natural in webview
- Can coexist with mycelium

**Cons**:
- Two P2P stacks to maintain
- Data model differences (Hypercore vs. iroh)
- NAT traversal overlap between hyperswarm and libp2p

### Option B: Replace Mycelium

Replace libp2p/iroh entirely with Pear:
- All P2P goes through Pear (in webview)
- Rust side just receives events via IPC

**Pros**:
- Single P2P stack
- Hypercore provides strong consistency

**Cons**:
- Large migration
- Lose iroh's specific features (docs, gossip)
- Pear is newer / less proven than iroh

### Option C: Interop/Compat

Keep both but design interoperability:
- Shared identity format
- Data migration tools
- Gateway between Hypercore and iroh namespaces

**Pros**:
- Flexibility
- Gradual migration

**Cons**:
- Complex
- May never fully interoperate

---

## Goals (SPIKE)

1. Research Pear Runtime: APIs, capabilities, limitations
2. Understand the hypercore stack (Hyperswarm, Hyperbee, Hyperdrive)
3. Design how Pear would integrate with the IPC bridge from track 2
4. Produce comparison + recommendation

---

## Non-Goals (SPIKE)

- Full production implementation (not the goal)
- Replacing mycelium immediately (decision deferred)
- Modifying the shared node structure (already designed in track 2)

---

## Research Areas

### Pear Runtime APIs

Document:
- How to initialize Pear/Hyperswarm
- Peer discovery API
- Data sync API (hypercore read/write)
- Identity management

### Identity Reconciliation

Current: FractalEngine uses Ed25519 keys (fe-identity)

Pear: Uses Hypercore keys (different format)

**Question**: Can we share identity, or do we need a mapping?

### Data Model Mapping

Current mycelium data:
- Verse (namespace_id → iroh-docs replica)
- Fractal (logical grouping)
- Petal (active scene)
- Node (scene entity with transform, URL)

Pear/Hypercore:
- Hypercore (append-only log)
- Hyperdrive (file system abstraction)
- Autobase (multi-writer)

**Question**: How do we map the hierarchy to hypercore structure?

### NAT Traversal

Current: libp2p + iroh handle NAT

Pear: Hyperswarm uses different NAT traversal

**Question**: Does Pear work in the same network scenarios as mycelium?

---

## Functional Requirements (SPIKE)

### FR-1: Pear Integration Design

Design how Pear plugs into the IPC bridge:

```typescript
// In Tauri webview (JavaScript)

// Initialize Pear
const swarm = await Hyperswarm();

// On peer discovery
swarm.on('connection', (conn, info) => {
  // Handle new peer
  // Convert to WebViewInteraction event
  window.__TAURI__.invoke('notify_interaction', {
    type: 'PeerDiscovered',
    peer_id: info.peerId,
  });
});

// On data sync
const core = new Hypercore((storage) => ...);
core.on('append', () => {
  // New data available
  // Notify Bevy via IPC
});
```

### FR-2: Shared Node Integration

Show how Pear events map to the shared node structure from track 2:

```typescript
// Pear peer event → WebViewInteraction
const interaction = {
  PeerDiscovered: {
    node: sharedNodeFromPeer(peerData),
  },
};

// Send via IPC
await window.__TAURI__.invoke('notify_interaction', { interaction });
```

### FR-3: Minimal POC Plan

Plan a minimal proof-of-concept:
1. Run Pear/Hyperswarm in Tauri webview
2. Discover a test peer
3. Pass peer data to Bevy via IPC
4. Verify Bevy receives the event

---

## Testing Strategy (SPIKE)

- **Research**: Document Pear APIs
- **Design**: Show integration with IPC bridge
- **POC plan**: Concrete steps to validate
- **Decision**: Recommendation based on research

---

## Risks and Mitigations (SPIKE)

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Pear docs are sparse | Medium | Medium | Check pears.com, Holepunch examples |
| Identity incompatibility | High | High | Design mapping or accept dual-identity |
| Data model mismatch | High | Medium | Research Hyperbee/Hyperdrive structure |
| NAT traversal issues | Medium | Medium | Test in various network scenarios |

---

## Design Decision (Deferred)

This spike does NOT make the adoption decision. It produces a recommendation:
- **Adopt Pear** (if viable)
- **Stick with mycelium** (if Pear doesn't fit)
- **Hybrid** (Pear for webview features, mycelium for Rust)

---

## Key Open Questions (Lead)

1. **egui-under-Tauri-host feasibility**: Track 4 spike must answer this before full shell inversion can be considered
2. **Pear-in-webview vs Rust mycelium**: This spike answers: can Pear augment/replace mycelium?
3. **Identity reconciliation**: Can Ed25519 keys map to Hypercore keys?
4. **Data model mapping**: How does the Verse/Fractal/Petal/Node hierarchy map to Hypercore?

---

## SPIKE Exit Criteria

This spike is complete when:
1. Pear Runtime APIs are documented
2. Integration with IPC bridge is designed
3. Identity/data model mapping is analyzed
4. POC plan exists
5. Exit report with recommendation exists
