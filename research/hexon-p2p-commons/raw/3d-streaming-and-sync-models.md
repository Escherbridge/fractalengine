# RAW: 3D Content Streaming + Sync Authority Models

Fetch date: 2026-07-11
Intent: Understand what real 3D streaming relies on (range requests, hierarchical LOD)
and how live-world sync-authority models compare to our CRDT/op-log.

## Resonite / FrooxEngine — HOST-AUTHORITATIVE, version-based optimistic concurrency
Source: https://wiki.resonite.com/index.php?title=Data_model_synchronization

- Architecture is **host-authoritative**, NOT peer-to-peer. A client's update is sent to the
  **host**, who forwards it to all other users in the session. The host is the single ordering point.
- Conflict resolution is **version-based (optimistic concurrency)**, NOT last-write-wins:
  "If the host receives updates from more than one user for the same data with the SAME version
  number, the FIRST one is kept as the authoritative version and the later updates are DISCARDED."
- Losers get a **rollback**: "the host will re-sync the authoritative value ... (effectively
  rolling back the 'optimistic' local change the client made)."
- Sync is **delta-based / event-driven**; "the data model state DOES NOT include dynamic events,
  impulses, or other time-based behaviors; only data."
- Explicit tradeoff: "optimistic concurrency works best when most data writes are NOT contended
  (at most one user updating a value at a time)."
- No atomic transactions built in — manual handling required.

CONTRAST with hexon/iroh-docs LWW: Resonite chose version-vector optimistic concurrency +
central host rollback over LWW precisely because a live 3D session has contended writes and
needs a coherent authoritative value NOW. Our model (CRDT LWW, no host) converges eventually
but cannot roll back a peer or guarantee a single authoritative value at an instant. This is the
CAP tension made concrete: Resonite picks C (per-session, via host) sacrificing P; hexon picks
A+P sacrificing C. => For a fully P2P commons, live contended editing of the SAME node by the
SAME instant is the weakest spot; per-node ownership / soft-locks are the realistic mitigation.

## Substrata (Glare Technologies) — CLIENT/SERVER, C++, per-world server
Sources: https://substrata.info/about_substrata , https://github.com/glaretechnologies/substrata

- Native C++ client + **server**; custom Glare engine; networked physics + Lua scripting.
- Renders one world of **>12,000 UGC objects at ~200 fps**.
- Model is **run-your-own-server** federation (like Mastodon), NOT peer-mesh. "Users are welcome
  to run their own server." Interop is promised via a **published network protocol + 3D mesh
  format spec** so third parties can write clients/servers/bots.
- TAKEAWAY: the credible "open metaverse" projects (Substrata, Hubs, Mastodon-style) all land on
  **federation of per-world servers with a documented protocol**, not pure P2P. The server is the
  availability + ordering anchor for a world; federation happens BETWEEN servers. This is exactly
  the role fractalengine's **relay + hexon registry** containers can play — an honest federation
  seam rather than pretending zero infrastructure.

## Cesium 3D Tiles — the streaming properties real 3D relies on
Sources:
- https://github.com/CesiumGS/3d-tiles/blob/main/specification/ImplicitTiling/README.adoc
- https://cesium.com/blog/2021/11/10/introducing-3d-tiles-next/
- https://docs.ogc.org/cs/22-025r4/22-025r4.html (3D Tiles 1.1, OGC ratified 2023)

- OGC community standard (introduced 2015, OGC standard 2019, **1.1 ratified 2023**).
- Dataset → **hierarchical spatial tiles streamed on demand**; client loads ONLY tiles visible at
  current camera position + zoom. **LOD hierarchy**: zoom in → progressively higher-res tiles.
- **Implicit tiling** (3DTILES_implicit_tiling): quadtree/octree indexed by **Morton code**;
  "enables **random access to any tile or range of tiles**", k-NN and **range queries**. Only the
  **root bounding volume** stored → tileset.json stays tiny.
- **Subtrees** = fixed-size sections storing availability bitstreams, "partitioned to bound the
  size of each availability buffer for **optimal network transfer and caching**."
- What this relies on: **predictable URL/addressing scheme** (Morton index → tile URL), **HTTP
  range requests / random access**, low-latency hierarchical fetch, and **CDN caching** of
  immutable-ish tiles. Root-first, coarse-to-fine descent.

## TENSION: content-addressed P2P fetch vs. tile streaming (analysis, not a fetch)
- 3D Tiles wants: (a) predictable index→URL mapping so the client can COMPUTE the next tile URL
  from camera pose (Morton code), and (b) HTTP range requests to pull a sub-range of a big blob
  (e.g. a glTF buffer) without downloading the whole thing.
- Content addressing (blake3 hash per blob) gives: verifiable, dedup'd, immutable blobs — but the
  hash is NOT computable from spatial position. You must first fetch a **manifest** (index→hash map)
  before you can request a tile. That is one extra hop of latency on cold start.
- Range-into-blob: iroh-blobs uses a **BLAKE3 verified-streaming (bao)** tree, so you CAN request
  and verify a byte RANGE of a blob without the whole thing — this preserves the "range request"
  property that tile streaming needs, over content-addressed transport. GOOD FIT. (HIGH confidence
  that BLAKE3/bao supports verified range streaming; iroh-blobs exposes it.)
- Hash-per-tile vs range-into-blob is the real design fork:
  * hash-per-tile: each tile is its own content-addressed blob → perfect dedup, natural P2P fetch
    from any peer, but a HashSeq/manifest per tileset and many small blobs (overhead per blob).
  * range-into-blob: one big blob per subtree, range-request sub-tiles → fewer blobs, matches
    3D-Tiles subtree design, but coarser dedup and the whole subtree shares one hash.
- CONCLUSION: content addressing is compatible with hierarchical tile streaming IF you (1) keep a
  small manifest mapping Morton/tile index → blake3 hash (the tileset.json analog), and (2) use
  BLAKE3 verified range streaming for large per-subtree blobs. The cold-start manifest hop is the
  only inherent extra latency vs. HTTP-URL tiling. LOD coarse-to-fine hides it (fetch root manifest
  once, then descend).
