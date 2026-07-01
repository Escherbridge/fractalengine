---
type: track-plan
---

# Implementation Plan: Pear Runtime P2P Layer SPIKE

## Overview

**SPIKE / RESEARCH** — Time-boxed investigation of Pear Runtime for FractalEngine P2P.

Four-phase approach:
1. Research Pear Runtime
2. Analyze architecture fit
3. Design POC plan
4. Produce exit report

**Time limit**: 2 weeks (SPIKE)

---

## Phase 1: Pear Runtime Research

**Goal:** Understand Pear APIs, capabilities, and how they work.

---

### Task 1.1 — Research Pear/Hyperswarm APIs [ ]

Document:
- How to initialize Hyperswarm
- Peer discovery API (`swarm.join()`, `swarm.on('connection')`)
- Data replication API (Hypercore)
- Identity management

**Verification:** API documentation exists.

**Files:** Research document

---

### Task 1.2 — Research Hypercore stack [ ]

Document:
- Hypercore (append-only log)
- Hyperbee (key-value store)
- Hyperdrive (file system)
- Autobase (multi-writer)

**Verification:** Component documentation exists.

**Files:** Research document

---

### Task 1.3 — Check Pear examples [ ]

Find and analyze working Pear examples:
- Holepitch examples
- Pear documentation site
- Community projects using Pear

**Verification:** At least one working example analyzed.

**Files:** Example analysis

---

### Task 1.4 — Verify Pear in Tauri webview [ ]

Confirm Pear can run in a browser/JavaScript context (Tauri webview is Chromium):
- Check for Node.js-specific APIs
- Verify browser compatibility
- Note any limitations

**Verification:** Browser compatibility documented.

**Files:** Research document

---

### Phase 1 Checkpoint

- Pear/Hyperswarm APIs documented
- Hypercore stack understood
- Examples analyzed
- Browser compatibility verified

---

## Phase 2: Architecture Analysis

**Goal:** Analyze how Pear fits (or doesn't) with FractalEngine.

---

### Task 2.1 — Analyze identity mapping [ ]

Current: FractalEngine uses Ed25519 keys (fe-identity)

Pear: Uses Hypercore keys

**Question**: Can we map Ed25519 → Hypercore key format?

Design:
- Option A: Dual identity (keep both)
- Option B: Key mapping function
- Option C: Hypercore-only identity

**Verification:** Identity approach documented.

**Files:** Architecture analysis

---

### Task 2.2 — Analyze data model mapping [ ]

Map current hierarchy to Hypercore:

```
Current:                    Hypercore:
├── Verse                   ├── hypercore[verse_id]
│   ├── Fractal             │   ├── hypercore[fractal_id]
│   │   ├── Petal           │   │   ├── hypercore[petal_id]
│   │   │   └── Node        │   │   │   └── hypercore[node_id]
```

Design:
- How to map Verse/Fractal/Petal to hypercores
- How to handle the append-only nature vs. mutable transforms

**Verification:** Data model mapping documented.

**Files:** Architecture analysis

---

### Task 2.3 — Analyze NAT traversal [ ]

Compare:
- Current: libp2p + iroh NAT handling
- Pear: Hyperswarm NAT handling

Document:
- Network scenarios (home NAT, corporate firewall, etc.)
- Which works better where
- Any gaps

**Verification:** NAT analysis documented.

**Files:** Architecture analysis

---

### Phase 2 Checkpoint

- Identity approach designed
- Data model mapping designed
- NAT traversal analyzed

---

## Phase 3: Proof-of-Concept Plan

**Goal:** Design a minimal POC to validate Pear integration.

---

### Task 3.1 — Design POC architecture [ ]

```
┌─────────────────────────────────────────────────────────────┐
│                    Tauri WebView                            │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Pear/Hyperswarm POC                                │   │
│  │    1. Initialize swarm                              │   │
│  │    2. Join topic (verse_id)                         │   │
│  │    3. On peer: create shared node                   │   │
│  │    4. notify_interaction() via IPC                  │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼ invoke()
┌─────────────────────────────────────────────────────────────┐
│                    Bevy (Rust)                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Handle: WebViewInteractionEvent(PeerDiscovered)    │   │
│  │    → Add peer to VerseManager                       │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Architecture diagram exists.

**Files:** POC design document

---

### Task 3.2 — Define POC success criteria [ ]

Success means:
1. Two instances can discover each other via Hyperswarm
2. Peer info is passed to Bevy via IPC
3. Bevy can display/manage discovered peers
4. No crashes or data corruption

**Verification:** Criteria documented.

**Files:** POC design document

---

### Task 3.3 — Estimate effort [ ]

Rough estimate:
- Pear setup in webview: 1-2 days
- IPC integration: 1 day
- Bevy peer management: 2 days
- Testing/debugging: 2 days

Total: ~1 week for POC

**Verification:** Estimate exists.

**Files:** POC design document

---

### Phase 3 Checkpoint

- POC architecture designed
- Success criteria defined
- Effort estimated

---

## Phase 4: Exit Report + Recommendation

**Goal:** Produce research summary and recommendation.

---

### Task 4.1 — Create exit report [ ]

```markdown
# SPIKE Exit Report: Pear Runtime P2P Layer

## Summary
[One paragraph summary of findings]

## Pear Overview
- What Pear provides
- How it differs from current mycelium

## Integration Analysis
### Identity
[How identity maps, or doesn't]

### Data Model
[How hierarchy maps to Hypercore, or doesn't]

### NAT Traversal
[Comparison of network handling]

## Recommendation

### Option A: Adopt Pear
Proceed with Pear integration for P2P. [Rationale]

### Option B: Stick with Mycelium
Continue with libp2p + iroh. [Rationale]

### Option C: Hybrid
Use Pear for webview features, mycelium for core. [Rationale]

## Next Steps
[If adopting, what tracks need to be created]
```

**Verification:** Report exists and is complete.

**Files:** `spike-exit-report.md`

---

### Task 4.2 — Update track dependencies [ ]

If recommendation affects other tracks:
- Track 2 (IPC bridge) already designed for this
- May need new track for Pear integration

**Verification:** Tracks updated if needed.

**Files:** Tracks metadata (if changed)

---

### Phase 4 Checkpoint

- Exit report complete
- Recommendation clear
- Tracks updated if needed

---

## Summary

| Phase | Delivers | Verification |
|-------|----------|--------------|
| 1 | Pear research | APIs documented |
| 2 | Architecture analysis | Integration design exists |
| 3 | POC plan | Architecture + success criteria |
| 4 | Exit report | Recommendation |

## SPIKE Criteria

- [ ] Pear/Hyperswarm APIs documented
- [ ] Hypercore stack understood
- [ ] Identity mapping designed
- [ ] Data model mapping designed
- [ ] NAT traversal analyzed
- [ ] POC architecture designed
- [ ] Exit report with recommendation

## Time Box

**2 weeks** from start to exit report. If not complete, default to NO-GO (stick with mycelium).

## Notes

This spike relates to tracks 1 & 2:
- Track 2's shared node structure is the "seam" for Pear
- If Pear is adopted, the IPC bridge is already ready
- No changes needed to tracks 1-3 for this spike
