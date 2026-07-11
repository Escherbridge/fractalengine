# RAW: P2P / Distributed 3D Worlds — Prior Art & Postmortems

Fetch date: 2026-07-11
Intent: Extract what centralized or killed each comparable distributed 3D world system.

## Croquet / Multisynq (replicated computation model)
Sources:
- https://grokipedia.com/page/Croquet_Project
- https://github.com/multisynq/m4u-package
- https://www.npmjs.com/package/@croquet/croquet

Key facts:
- Model treats **computation itself, not data, as the unit of replication**. Identical
  deterministic "clockwork compute" runs on every client. When simulation is deterministic
  and inputs are the only thing synced, **network traffic can be exactly zero** while thousands
  of entities move in lockstep on all screens.
- Central primitive: a **universal timebase** — a shared simulated pseudo-time that sequences
  all operations deterministically and prevents race conditions. Replicas advance in lockstep;
  claimed latencies "tens of milliseconds."
- HARD CONSTRAINT: the Croquet Model must be **100% deterministic and completely self-contained**.
  It may interact with the outside world ONLY via subscriptions to input events published by a view.
  Any nondeterminism (Date.now, Math.random uncontrolled, floating-point divergence, external I/O)
  breaks bit-identical replication.
- CRITICAL ARCHITECTURAL NOTE (from Croquet docs / known design): Croquet is NOT pure P2P —
  it uses a **"reflector"** server that orders and timestamps input messages and rebroadcasts them.
  The reflector holds NO application state (it is stateless w.r.t. the model), but it is a
  central sequencer. This is the classic "deterministic lockstep needs a total order" tax:
  you can eliminate state replication but you still need SOMEONE to establish message order.
  Multisynq is the commercialization; it runs reflector infrastructure as a service.

IMPLICATION for hexon commons: deterministic-replication buys huge bandwidth savings but
requires (a) a total order source (a sequencer/reflector — a centralization seam) and
(b) fully deterministic world logic. Our CRDT/op-log model trades the sequencer away for
eventual consistency; we cannot get Croquet's zero-traffic lockstep without a sequencer.

## Third Room (Matrix-based 3D world) — PAUSED / team laid off
Sources:
- https://mastodon.matrix.org/@thirdroom/110787604385806102 (announcement)
- https://github.com/matrix-org/thirdroom/blob/main/README.md
- https://matrix.org/blog/2023/06/07/introducing-third-room-tp-2-the-creator-update/

Why it stopped (from Matrix.org Mastodon announcement, summarized in search):
- **The Third Room team was laid off; project is on pause.**
- "Funding the team to work full-time on Third Room was increasingly challenging — the
  macroeconomic environment did not lend itself to moonshot R&D projects, and Matrix as a whole
  continued to suffer from insufficient financial support."
- Remains open-source (Apache 2.0) but active development halted. => FAILURE MODE = ECONOMICS,
  not a technical dead-end. A federated-commons moonshot could not sustain full-time funding.

Technical architecture (README):
- Engine "Manifold": multithreaded, **bitECS** (ECS) + **Three.js** (WebGL2), glTF as base format.
- Lock-free scene graph via **SharedArrayBuffers + Atomics**; rendering on dedicated WebWorker,
  OffscreenCanvas.
- Physics: **Rapier.js** (WASM) on the game thread. User scripts via WASM.
- Networking: **P2P over WebRTC DataChannels**, signaling via **Matrix (MSC3401)**. Working on an
  **SFU (selective forwarding unit) + sfu-to-sfu protocol** to raise capacity by cutting bandwidth.
- Limitations acknowledged: no WebXR yet; browser API constraints; game sim must complete within
  monitor refresh (double-buffering).
- NOTE: even a "decentralised on Matrix" design reached for an **SFU** — a server-side relay —
  to scale player counts. Pure WebRTC mesh does not scale past small groups (n^2 connections).
  This is the same centralization gravity Third Room's own roadmap conceded.

## Solipsis (Inria / France Télécom) — INACTIVE since ~2009
Sources:
- https://en.wikipedia.org/wiki/Solipsis
- https://inria.hal.science/inria-00337057v1/document (Frey, Royan et al., "Solipsis: A
  Decentralized Architecture for Virtual Environments", MMVE 2008)
- https://typeset.io/papers/solipsis-... (Keller & Simon 2003, 120 citations)

Key facts:
- Fully P2P massively-multiparticipant world; **no server, no IP multicast**; aims for unlimited
  participants. Designed by Joaquin Keller & Gwendal Simon (France Télécom R&D).
- Overlay: **Raynet, an n-dimensional Voronoi-based overlay network**. Each node links to all nodes
  within its **Area of Interest (AOI)**; relationships depend on **virtual proximity** not IP.
- "Emissive and receptive fields" determine how avatar state / media streams are established.
- Stable release **1.09 (Feb 18 2009)**; now categorized as an **inactive MMO**. No published
  postmortem — a NEGATIVE-SPACE finding: the academic P2P-NVE lineage (Solipsis, VAST) produced
  strong interest-management theory but **no lasting deployed commons**. The systems died of
  neglect / lack of adoption, not a documented technical wall.

## Mozilla Hubs — SHUT DOWN May 31 2024, handed to community (Hubs Foundation)
Sources:
- https://support.mozilla.org/en-US/kb/end-support-mozilla-hubs
- https://ryanschultz.com/2024/02/26/mozilla-ceases-support-for-mozilla-hubs...
- https://www.uploadvr.com/mozilla-hubs-shutdown/

Key facts:
- Feb 13 2024: Mozilla org-wide restructuring; Hubs cut. Support ended **May 31 2024**.
- Reason: **organizational restructuring / cost**, NOT a technical failure. Same economics story.
- Codebase → **Hubs Foundation**; "Hubs Community Edition" for self-hosting existed already.
  Mozilla shipped a **data-export tool** (glTF scenes, avatars, media) so users could migrate.
- LESSON: even a well-funded (Mozilla) hosted 3D-world platform is a cost center the sponsor can
  drop. The **open-source + self-host + data-export** combination is what let the commons survive
  the sponsor's exit. This is the strongest argument FOR the local-first / data-ownership premise:
  survivability under sponsor death depends on users already holding their bytes + an export path.
