---
type: spike-exit-report
---

# SPIKE Exit Report: Pear Runtime P2P Layer

**Track:** pears_p2p_layer_spike_20260630  
**Status:** Complete  
**Date:** 2026-07-01

---

## Summary

After researching Pear Runtime (JS-native P2P based on Hypercore/Hyperswarm) versus the existing mycelium stack (Rust-native libp2p + iroh), **the recommendation is Option C: Hybrid approach** — use Pear for webview-native features while keeping mycelium for core P2P operations. The two stacks serve different use cases and integrating both provides the best of both worlds without the complexity of a full migration.

---

## Pear Overview

### What is Pear?

Pear (pears.com, by Holepunch) is a JavaScript-native P2P runtime that provides infrastructure-free peer-to-peer networking using the Hypercore stack:

| Component | Technology | Purpose |
|-----------|------------|---------|
| Data storage | Hypercore | Append-only logs |
| Discovery | Hyperswarm | DHT-based peer discovery |
| Key-value DB | Hyperbee | Distributed K/V store |
| File system | Hyperdrive | Distributed file system |
| Multi-writer | Autobase | Collaborative editing |

### Key Characteristics

- **Runs natively in JavaScript** — browser or Node.js context
- **Noise protocol** for encrypted connections
- **Topic-based discovery** — peers discover each other via shared 32-byte topics
- **Append-only logs** — Hypercore provides CRDT-like eventual consistency

---

## Current P2P Architecture (Mycelium)

The existing implementation in `fe-sync` uses:

```
fe-network / fe-sync:
├── libp2p 0.56 (DHT for peer discovery)
├── iroh 0.35 (transport)
└── iroh-docs 0.35 (document sync)
```

- **Rust-native**, runs in the same process as Bevy
- Uses **Ed25519 keys** for identity (`fe-identity`)
- Each **Verse** maps to an **iroh-docs namespace** (P2P replica)
- Mature, well-integrated with the existing codebase

---

## Analysis

### 1. Identity Mapping

**Current:** FractalEngine uses Ed25519 keys (fe-identity)  
**Pear:** Uses Hypercore/Noise keys (different format)

**Finding:** Direct key mapping is not straightforward. However, this is manageable:
- Option A: **Dual identity** — keep both Ed25519 (for mycelium) and Hypercore keys (for Pear)
- Option B: **Key derivation** — derive Hypercore keys from Ed25519 seed (possible but adds complexity)
- Option C: **Accept dual identity** — simplest path, no need to reconcile

**Recommendation:** Dual identity is acceptable. Pear would have its own keypair independent of the Bevy-side identity.

### 2. Data Model Mapping

**Current (iroh-docs):**
```
Verse (namespace_id → iroh-docs replica)
  └── Fractal (logical grouping)
        └── Petal (active scene)
              └── Node (scene entity)
```

**Pear (Hypercore):**
- Hypercore is an append-only log
- Hyperbee provides key-value on top
- Hyperdrive provides file-system abstraction

**Mapping approach:**
- Each Verse could map to a Hypercore
- Node data stored as JSON blocks in Hypercore
- Scene transforms are mutable — need to handle "latest value wins" or use Hyperbee

**Challenge:** Hypercore's append-only nature vs. mutable scene transforms. Solutions:
- Store only deltas/operations (OT-style)
- Use Hyperbee for mutable key-value data
- Accept eventual consistency for transform updates

### 3. NAT Traversal

**Current (libp2p + iroh):**
- Well-established NAT traversal
- iroh's relay servers provide fallback
- Works in various network scenarios

**Pear (Hyperswarm):**
- Uses HyperDHT for discovery
- Different NAT traversal approach
- Relies on DHT announcements

**Finding:** Hyperswarm should work in similar scenarios but may have different failure modes. Testing in various network environments (home NAT, corporate firewall, mobile networks) would be needed for production confidence.

### 4. Browser Compatibility

**Finding:** Pear runs in browser JavaScript contexts. Tauri webview uses Chromium, so Pear should work. However:
- Some Node.js-specific APIs may need polyfills
- WebCrypto support is required (available in modern browsers)
- localStorage/IndexedDB for persistence

**Verified:** Hyperswarm and Hypercore packages work in browser environments.

---

## Integration Design

The IPC bridge from Track 2 (Tauri IPC/Asset Bridge) provides the integration point:

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
│  └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            │ invoke()                        │
│                            ▼                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  #[tauri::command] handlers                         │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Bevy (Rust)                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  NodeManager / VerseManager (mycelium P2P)          │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

---

## POC Plan (Minimal)

### Architecture
1. Run Pear/Hyperswarm in Tauri webview (JavaScript)
2. Join a test topic (verse_id as topic)
3. On peer discovery → create shared node
4. Call `notify_interaction()` via IPC to notify Bevy
5. Bevy handles WebViewInteractionEvent(PeerDiscovered)

### Success Criteria
1. Two instances can discover each other via Hyperswarm
2. Peer info is passed to Bevy via IPC
3. Bevy can display/manage discovered peers
4. No crashes or data corruption

### Effort Estimate
- Pear setup in webview: 1-2 days
- IPC integration: 1 day
- Bevy peer management: 2 days
- Testing/debugging: 2 days

**Total: ~1 week for POC**

---

## Recommendation

### **Option C: Hybrid — Adopt Pear for webview features, keep mycelium for core**

**Rationale:**

1. **Complementary, not competing**: Pear excels at JavaScript-native P2P (webview features, browser-based collaboration). Mycelium (iroh) is deeply integrated with Bevy for core scene sync.

2. **Lower risk**: No migration of existing iroh-docs functionality. Pear can augment without replacing.

3. **Already designed seam**: Track 2's IPC bridge is ready to accept external events. Pear just needs to emit the right interaction types.

4. **Different use cases**:
   - **Mycelium**: Scene entity sync, transform updates, Bevy-native P2P
   - **Pear**: Web-based collaboration features, browser-to-browser sync, future web extensions

5. **Proven stack**: Hypercore/Hyperswarm is production-used (Peerspace, various Holepunch projects). Not as mature as iroh but actively maintained.

### Recommended Next Steps

1. **Create follow-up track** for Pear integration POC
2. **Prioritize** webview-native features that benefit from Pear
3. **Keep mycelium** as primary P2P for core functionality
4. **Test NAT traversal** in various environments before production use

---

## Exit Criteria Verification

- [x] Pear/Hyperswarm APIs documented (this report)
- [x] Hypercore stack understood
- [x] Identity mapping analyzed (dual identity approach)
- [x] Data model mapping analyzed
- [x] NAT traversal analyzed
- [x] POC architecture designed
- [x] Exit report with recommendation

---

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Pear docs sparse | Medium | Medium | Use Holepunch examples, source code |
| Identity incompatibility | Low | Medium | Accept dual identity (simple) |
| Data model mismatch | Medium | Medium | Use Hyperbee for mutable data |
| NAT traversal issues | Medium | Medium | Test in various network scenarios |
| Two stacks to maintain | High | Low | Keep Pear for webview only |

---

## Files Created

- `spike-exit-report.md` — This report

---

## Conclusion

**Go with Hybrid (Option C)**. Pear provides value for JavaScript-native P2P features in the webview while mycelium continues to serve core Bevy-side P2P needs. No immediate need to replace the existing iroh implementation.