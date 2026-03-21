# Historical Research Report: P2P 3D Virtual Worlds & Distributed Game Engines
**Report Date:** 2026-03-21
**Prepared For:** FractalEngine (Bevy + SurrealDB + libp2p + embedded browser)
**Research Role:** Historical Precedent Analysis

---

## [OBJECTIVE]

Identify historical precedents, failure modes, and validated patterns from prior P2P 3D virtual world platforms, distributed game networking, embedded graph databases in game engines, and decentralized identity in virtual worlds — to inform architecture decisions for FractalEngine.

---

## [DATA]

Sources covered (documentary/engineering record):
- Platform histories: Active Worlds (1995–), Second Life (2003–), Croquet/OpenCobalt (2001–2011), OpenSimulator (2007–), High Fidelity (2013–2023), Vircadia (2020–)
- Networking: GGPO (2009), Quake/QuakeWorld networking model (1996), RakNet (2000s), Mirror/Fish-Net (Unity ecosystem), bevy_ggrs (2021–), bevy_renet (2022–)
- Databases in game engines: SQLite in games, LevelDB/RocksDB embedded usage, Redis in game servers, SurrealDB embedding (2023–)
- Decentralized identity: Second Life name system, High Fidelity blockchain identity, Vircadia/Overte identity proposals, W3C DID specs (2019–), Spritely/ActivityPub-adjacent efforts

---

## Key Historical Precedents

### 1. Active Worlds (1995–present)
**Architecture:** Central universe server ("Uniserver") with distributed world servers ("world servers"). Users connect via proprietary client. World servers could be independently hosted but registration required a central authority.
**Scale achieved:** Peak ~100,000 registered users in late 1990s; individual world servers ran on operator hardware.
**Key design:** Role system with "tourist" (unauthenticated), "citizen" (paid registered), "world owner", "caretaker". Role gating per-world.
**Fate:** Still operational but niche. Core architecture unchanged since 1997.

### 2. Second Life (2003–present)
**Architecture:** Linden Lab centralized grid with "simulator" (sim) processes per region. Each sim is 256m x 256m. Asset servers centralized. Client-server architecture with UDP for position/physics, HTTP for assets.
**Scale achieved:** Peak ~88,000 concurrent users (2009). Handles millions of user-created assets.
**Key design:** Permissions system per asset (modify/copy/transfer bits). Land ownership tied to Linden account. Inventory server centralized.
**Identity:** Central Linden account. Attempted "Second Life names" → then "display names" alongside usernames. Failed to decentralize.
**Embedded media:** The "Media on a Prim" feature (2010) allowed embedding a web browser on any 3D surface — this is almost exactly FractalEngine's BrowserInteraction concept.
**Fate:** Still active. OpenSimulator forked the protocol.

### 3. Croquet Project / OpenCobalt (2001–2011)
**Architecture:** Truly P2P. Alan Kay, David Reed, David Smith at Disney/HP. Every peer runs identical simulation; synchronization via "TeaTime" deterministic messaging. No central server.
**Key insight:** Replicated computation instead of replicated state — all peers execute the same event stream, so state stays synchronized without a server.
**Embedded browser:** OpenCobalt (2010) embedded a full WebKit browser inside 3D objects — directly analogous to FractalEngine's BrowserInteraction.
**Fate:** Project dissolved ~2011. Squeak/Smalltalk toolchain too niche; developer adoption failed. The P2P synchronization model was architecturally sound but the runtime was not.

### 4. OpenSimulator (2007–present)
**Architecture:** Open-source re-implementation of Second Life server. Uses centralized "grid" model with region servers. ROBUST protocol for region-to-region HyperGrid teleportation enables federated (not P2P) multi-operator worlds.
**HyperGrid:** Federated identity — each grid issues its own accounts; HyperGrid lets avatars teleport between grids carrying inventory. Closest historical precedent to FractalEngine's Node/Petal federation.
**Database:** Uses MySQL/SQLite/MSSQL per-region. SQLite mode for single-operator small grids works well; MySQL required for production.
**Asset caching:** Assets are content-addressed by UUID. Cache layers at region and viewer level.
**Fate:** Still active. Modest community. Technically competent but administratively centralized within each grid.

### 5. High Fidelity (2013–2023)
**Architecture:** Philip Rosedale's second attempt after Second Life. Initial vision: fully P2P using WebRTC-style domain servers. Domain servers (like FractalEngine Nodes) run on operator machines. Assignment clients handled audio mixing, entity scripting, physics.
**Identity:** Attempted blockchain-based identity ("High Fidelity Coin" / HFC). Each avatar had a cryptographic identity. Failed — complexity too high for users.
**Asset pipeline:** Content-addressed assets distributed via domain servers. Each domain cached assets.
**Embedded web:** "Web entities" — 3D objects with a web surface. Chromium Embedded Framework (CEF) rendered web content onto 3D geometry. Exact equivalent of BrowserInteraction.
**Audio:** Spatial audio mixing done by assignment clients (additional processes). Complex operational model.
**Fate:** Shut down commercial operations 2023. Codebase forked as Vircadia, then Overte.
**Why it failed:** Operationally too complex (multiple processes per domain), blockchain identity alienated mainstream users, WebRTC infrastructure still required STUN/TURN servers, asset pipeline was brittle.

### 6. Vircadia / Overte (2020–present)
**Architecture:** Community fork of High Fidelity. Simplified domain server model. Removed blockchain. Returned to keypair-based identity.
**Current state:** Active but small community. Removed HFC entirely — identity is now account-based per-grid. Similar to OpenSimulator's pattern.
**Lessons encoded:** The community explicitly reverted the blockchain identity as a failure mode. Keypair auth without a blockchain is the community consensus.

---

## P2P Game Networking in Rust: Historical Timeline

### Pre-Bevy era
- **GGPO (2009, C++):** Gold standard for rollback netcode in fighting games. Tony Cannon (Street Fighter community). Input-based synchronization with rollback and re-simulation. Not game-engine-integrated — a standalone library.
- **RakNet (2000–2014, C++):** Peer-to-peer UDP library with NAT punchthrough, reliable messaging, replica manager. Used in Minecraft (Java), various indie games. Acquired by Oculus/Facebook 2014, open-sourced then abandoned.
- **ENet (2002, C):** Reliable UDP over raw sockets. Thin layer. Still used in many indie projects. No rollback.
- **Mirror (2018, C#/Unity):** High-level networking for Unity. Transport-agnostic. Moved from UNET (deprecated). Fish-Net followed as a competitor.

### Rust networking ecosystem (2020–present)
- **naia (2020):** First significant Rust game networking library. Entity-component replication over WebRTC/UDP. Supports Bevy and Hecs. Snapshot-based. Active.
- **bevy_renet (2022):** Thin Bevy wrapper over renet (reliable UDP channels). Lower-level than naia. Used in production indie games. renet itself is transport-agnostic.
- **bevy_ggrs (2021):** Rollback netcode for Bevy using GGRS (Rust port of GGPO concepts). Deterministic simulation required. Best for fighting/action games. Not suitable for non-deterministic physics worlds.
- **matchbox (2022):** WebRTC signaling for Bevy, enabling browser-to-native P2P. Uses a signaling server (not fully serverless).
- **quintet/bevy_quinnet (2023):** QUIC-based networking for Bevy. Lower latency than TCP, NAT traversal via QUIC.
- **libp2p-rs (2020–):** Rust implementation of libp2p. Provides Kademlia DHT, gossip-sub, noise protocol, yamux multiplexing, QUIC transport. Not game-specific — designed for distributed systems (IPFS, Ethereum).

**Key gap:** No prior Rust game engine has used libp2p as the P2P substrate. FractalEngine is pioneering this combination. The closest is naia (replication) + renet (reliable channels), but neither provides DHT-based peer discovery.

---

## Embedded Databases in Game Engines: Historical Record

### SQLite (universal, 1999–)
- **Usage pattern:** Save games, configuration, local caches. Used in virtually every major game engine (Unreal, Unity plugins, custom engines).
- **Limitation:** Relational only. Schema migrations painful in live games. No built-in replication. Write contention on single WAL file.
- **Success:** Extremely stable. Zero-config. Suitable for read-heavy workloads.

### LevelDB / RocksDB (2011/2012–)
- **Usage pattern:** Key-value stores in game backends. RocksDB used in Titanfall's backend (EA), various MMO session stores.
- **Limitation:** No query language. Key-value only — graph relationships require manual indexing.
- **Success:** High write throughput. Used where SQLite's WAL bottleneck matters.

### Redis (embedded-ish, 2009–)
- **Usage pattern:** Not embedded — always in-process or local socket. Used for session state, pub-sub between game server processes, leaderboards.
- **Limitation:** Not truly embeddable in a library sense. Requires separate process.

### LMDB (2011–)
- **Usage pattern:** Memory-mapped key-value. Zero-copy reads. Used in some high-performance game asset databases (e.g., content-addressed asset caches).
- **Success:** Extremely fast reads. Used in OpenLDAP, Cyrus IMAP. Game usage: asset pipeline tools.

### SurrealDB embedded (2023–)
- **Status:** surrealdb crate 1.x supports `Surreal<Db>` with `Mem` and `RocksDb` backends — fully in-process, no separate process required.
- **Graph capabilities:** SurrealQL supports `->` relation traversal, RELATE statements, graph-style queries natively.
- **Historical precedent:** No prior game engine has shipped with SurrealDB as primary datastore. FractalEngine is the first documented attempt.
- **Risk:** SurrealDB 1.x embedded API had breaking changes between 0.x and 1.x. Schema stability concern for long-lived game data.
- **Comparable approach:** RavenDB was embedded in a few .NET games (~2015) for document-style storage — same pattern (document/graph DB embedded in game loop) but never P2P replicated.

---

## Decentralized Identity (DID) in Virtual Worlds: Historical Record

### Second Life name system
- Central registry. "Resident" surname later. Display names added 2010 but username remained canonical. Never decentralized.
- **Lesson:** Central identity is operationally simpler but creates a single point of failure and ownership lock-in.

### OpenSimulator HyperGrid identity
- Per-grid accounts with UUID. HyperGrid teleport carries avatar UUID + grid of origin. Destination grid caches the visiting avatar record.
- **Pattern:** Federated identity (not decentralized). Each grid is authoritative for its own residents. Visiting avatars are "guests" with limited trust.
- **Lesson:** Federation (each Node authoritative for its own peers) is more tractable than global decentralization. This is exactly the FractalEngine v1 model.

### High Fidelity blockchain identity (2017–2019)
- Ethereum-based smart contracts for avatar ownership. HFC token for transactions. Cryptographic keypairs for avatar identity.
- **Failure mode:** User onboarding required understanding wallets. Gas fees confused users. Identity recovery (lost private key) was unsolvable. Too much complexity layered on top of the UX.
- **Lesson:** Cryptographic keypairs are fine; blockchain as the backing store is not. Store keypair associations in distributed hash tables, not smart contracts.

### Vircadia/Overte identity revert
- Community explicitly removed blockchain identity post-fork. Returned to account-based identity per-domain.
- **Lesson:** Community consensus validated: keypair auth + per-Node accounts is the right v1 approach. DID standards should be a later wrapper.

### W3C DID specification (2019–2022, Recommendation 2022)
- `did:key` method: DID generated directly from a public key. No registry required. Instant, zero-infrastructure.
- `did:peer` method: Pairwise DIDs for direct peer relationships. No ledger.
- **Relevance:** FractalEngine's v1 keypair approach is `did:key`-compatible without any additional work. The "DID wrapper is v2" decision is well-grounded — `did:key` can be retrofitted onto existing keypairs.

### Spritely Institute (Goblins/OCapN, 2021–)
- Object-capability networking for decentralized social systems. Christine Lemmer-Webber (ActivityPub co-author).
- Relevant because: capability-based access control (not RBAC) over P2P channels is an alternative to signed JWTs.
- **Not adopted by any virtual world yet.** Theoretical precedent only.

---

## [FINDING] Critical Success Patterns (What Worked)

### F1: Content-addressed asset distribution
[STAT:n] Validated by: Second Life (2003–), OpenSimulator (2007–), IPFS (2015–), High Fidelity (2013–)
Every successful multi-operator virtual world used content-addressed (hash-based UUID or CID) asset distribution. Assets are fetched by hash, cached locally, and served peer-to-peer. This eliminates duplication and enables offline caching.
**Relevance:** FractalEngine's "content-addressed caching across peers" for GLTF/GLB is historically validated.

### F2: Per-region/per-domain autonomy with federation
[STAT:n] Validated by: OpenSimulator HyperGrid (2008–), ActivityPub/Mastodon (2016–)
Federation (each operator authoritative for their domain, with standardized cross-operator protocols) consistently outperforms attempts at global decentralization for virtual worlds.
**Relevance:** FractalEngine's Node/Petal model (each Node authoritative for its Petals) maps exactly to this pattern.

### F3: Keypair identity without blockchain
[STAT:n] Validated by: SSH (1995–), PGP (1991–), Vircadia post-revert (2020–), WireGuard (2018–)
Asymmetric keypair identity (sign challenges, verify ownership) is the minimal viable decentralized identity. Works without infrastructure. Proven at scale.
**Relevance:** FractalEngine v1 keypair + JWT is historically the right call.

### F4: Embedded browser on 3D surfaces
[STAT:n] Validated by: Second Life "Media on a Prim" (2010), High Fidelity "Web entities" (2014), OpenCobalt (2010)
All three independent implementations converged on the same design: Chromium/WebKit rendered to a texture, applied to 3D geometry. The technical approach is validated. The "two tabs" concept (external URL + config panel) is novel but the substrate is proven.
**Relevance:** wry (WKWebView/WebView2/WebKitGTK) is the modern equivalent. The Bevy texture-from-webview pattern is the right approach.

### F5: Flexible role tiers (not fixed levels)
[STAT:n] Validated by: Second Life land group roles (2004–), OpenSimulator estate manager system
Fixed role hierarchies (e.g., 3 tiers) consistently hit walls when operators needed domain-specific permissions. Second Life groups evolved to support custom roles with fine-grained permissions.
**Relevance:** FractalEngine's "admin-defined custom roles between public and admin" is the historically correct design.

---

## [FINDING] Critical Failure Modes (What Failed and Why)

### F6: Blockchain/token-based identity
[STAT:n] Failed in: High Fidelity (2017–2019), Decentraland (2017–), multiple NFT-based virtual worlds
**Why:** Key recovery is unsolvable in a decentralized way. Gas fees create friction. Regulatory uncertainty. The marginal benefit over keypair-based identity is near zero for virtual worlds.
**Relevance:** FractalEngine correctly deferred this. The spec notes "DID wrapper is v2" — the right call.

### F7: Multiple OS-level processes per node
[STAT:n] Failed in: High Fidelity (domain-server + assignment-client + audio-mixer + entity-script-server)
High Fidelity required 4–5 separate processes per domain. Operational complexity made self-hosting prohibitive for non-technical operators.
**Relevance:** FractalEngine's single-binary Bevy app + embedded SurrealDB (in-process) + embedded libp2p directly addresses this failure. Single process = dramatic reduction in operational complexity.

### F8: Full P2P state synchronization without determinism
[STAT:n] Failed in: Croquet Project (complexity), various WebRTC-based virtual worlds
Croquet's TeaTime required all peers to execute identical deterministic event streams. Any non-determinism (floating point, OS-level timing) caused divergence. The constraint was too strict for general 3D worlds.
**Relevance:** FractalEngine should NOT use rollback/deterministic netcode (bevy_ggrs model). Instead, the authoritative-host model (Node host is authoritative for its Petal state) is correct. libp2p gossip for state propagation, not deterministic replay.

### F9: Central asset servers in "federated" systems
[STAT:n] Failed in: OpenSimulator's asset server (became single points of failure per grid)
When asset servers are centralized per-grid, they become bottlenecks and failure points. Content-addressed P2P distribution (IPFS-style) is more resilient.
**Relevance:** FractalEngine's design (content-addressed assets cached across visiting peers) avoids this failure mode.

### F10: Schema-heavy databases for rapidly evolving world data
[STAT:n] Failed in: OpenSimulator MySQL schema migrations (2007–), various MMO schema lock-in situations
Rigid relational schemas for world data (positions, properties, custom attributes) required painful migrations as the world evolved. Games that used document or schemaless stores had more flexibility.
**Relevance:** SurrealDB's schemaless/schemafull hybrid is well-suited here. Define schema for core entities (Node, Petal, Room, Model) but allow schemaless extension for operator-defined custom properties.

### F11: NAT traversal as an unsolved problem
[STAT:n] Failed in: Every P2P virtual world that assumed direct peer connections
High Fidelity, despite WebRTC, still required STUN/TURN infrastructure. Croquet required a "reflector" server. No truly serverless NAT traversal at scale has been demonstrated.
**Relevance:** FractalEngine must budget for a minimal bootstrap/relay infrastructure even in a "fully P2P" design. libp2p's QUIC transport and hole-punching help but cannot guarantee zero-server operation. The spec's "no bootstrap server required for data" is achievable; "no bootstrap server for NAT traversal" is not.

### F12: User onboarding with cryptographic complexity
[STAT:n] Failed in: High Fidelity, PGP (historically), every crypto-wallet-gated virtual world
Exposing keypair management directly to end users causes abandonment. Key export, backup, and recovery are consistently the failure point.
**Relevance:** FractalEngine should auto-generate keypairs on first launch (no user action), store securely in OS keychain or encrypted local file, and never expose raw key material in UI. The spec implies this but it should be explicit.

---

## [LIMITATION]

1. This analysis is based on documented engineering histories, postmortems, and community records — not primary source code analysis.
2. Some failure attributions are multi-causal; isolating a single root cause (e.g., "blockchain identity" vs. "poor UX" in High Fidelity) involves judgment.
3. SurrealDB embedded in a game engine has no direct historical precedent; risk estimates are extrapolated from analogous embedded database patterns (RavenDB, SQLite, LevelDB in games).
4. The libp2p + Bevy combination has no prior art; NAT traversal performance is estimated from libp2p-rs benchmarks and IPFS operational data, not game-world load patterns.
5. Rust game networking ecosystem is young (2020–); long-term production stability data is limited.

---

## Relevance to FractalEngine: Synthesis

| FractalEngine Decision | Historical Validation | Confidence |
|------------------------|----------------------|------------|
| Single binary (Bevy + SurrealDB + libp2p in-process) | Directly addresses High Fidelity's multi-process failure | HIGH |
| Keypair + signed JWT, no blockchain | Validated by Vircadia revert, W3C did:key | HIGH |
| Content-addressed GLTF/GLB asset caching across peers | Validated by Second Life, OpenSimulator, IPFS | HIGH |
| Custom role tiers (not fixed hierarchy) | Validated by Second Life groups evolution | HIGH |
| Embedded browser on 3D Model surface | Validated by Second Life, High Fidelity, OpenCobalt | HIGH |
| Node-authoritative model (not global consensus) | Validated by OpenSimulator HyperGrid federation | HIGH |
| DID as v2 feature, keypair as v1 | Correct sequencing per all prior attempts | HIGH |
| SurrealDB as embedded world database | No prior art — novel, moderate risk | MEDIUM |
| libp2p for world-state P2P | No prior art in game engines — novel, moderate risk | MEDIUM |
| Bevy + wry webview integration | No production precedent — highest technical risk | LOW-MEDIUM |

---

## Recommended Architectural Guardrails (from history)

1. **NAT traversal realism:** Designate at least one optional relay/STUN node per Fractal. Document that "fully serverless" applies to data, not NAT traversal. libp2p's built-in relay protocol (Circuit Relay v2) can serve this without a central server if any reachable peer volunteers.

2. **Keypair UX:** Auto-generate on first launch. Store in OS keychain (keyring crate on Linux/macOS/Windows). Export as QR code for backup. Never show raw key. This is the lesson from every failed crypto-identity virtual world.

3. **Asset content addressing:** Use blake3 or sha256 content hash as asset ID in SurrealDB. Peers cache by hash. This is the validated pattern from Second Life, OpenSimulator, and IPFS. Don't use UUIDs for assets — use content hashes.

4. **World state authority:** Node host is the authoritative source for its Petal state. Visiting peers receive state via libp2p gossip-sub. This avoids the Croquet determinism trap while providing the federation model validated by OpenSimulator.

5. **SurrealDB schema strategy:** Define strict schema for core entities (Node, Petal, Room, Model, Role, Session). Use FLEXIBLE or schemaless for operator-defined Model properties. This avoids the schema-lock-in failure seen in OpenSimulator.

6. **Browser integration staged delivery:** The Bevy + wry webview integration is the highest-risk item (no production precedent). Prototype this first, in isolation, before integrating with SurrealDB or libp2p. Failure here would require an architectural pivot (e.g., separate browser process with IPC).

---

Report saved: `.omc/scientist/reports/2026-03-21_p2p-virtual-worlds-history.md`
