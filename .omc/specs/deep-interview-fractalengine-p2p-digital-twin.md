# Deep Interview Spec: FractalEngine — P2P 3D Digital Twin Ecosystem

## Metadata
- Interview ID: fractalengine-2026-03-21
- Rounds: 8
- Final Ambiguity Score: 18%
- Type: greenfield
- Generated: 2026-03-21
- Threshold: 20%
- Status: PASSED

## Clarity Breakdown
| Dimension | Score | Weight | Weighted |
|-----------|-------|--------|----------|
| Goal Clarity | 0.85 | 0.40 | 0.340 |
| Constraint Clarity | 0.82 | 0.30 | 0.246 |
| Success Criteria | 0.78 | 0.30 | 0.234 |
| **Total Clarity** | | | **0.820** |
| **Ambiguity** | | | **18%** |

---

## Goal

Build **FractalEngine**: a decentralized, peer-to-peer 3D digital twin platform where any operator can run a **Node** (native Bevy desktop app), host multiple **Petals** (3D worlds/spaces), populate them with uploaded 3D Models, and grant other peers role-based access — all without a central server. Each 3D Model can have an embedded in-world browser with two tabs: a public-facing external URL (configurable by the Petal admin) and an internal config panel. Roles are flexible (admin-defined custom tiers between `public` and `admin`), scoped per Petal and overridable per Model.

The full network of connected Nodes is called the **Fractal**. The platform is domain-agnostic — any use case (virtual offices, industrial twins, showrooms, social spaces) can be built on it.

---

## Entity Hierarchy (The Fractal Naming System)

```
Fractal  (the entire P2P network)
  └── Node  (one peer / operator running the Bevy desktop app)
        └── Petal  (a 3D world / space hosted by the Node)
              └── Room  (a zone / area within a Petal)
                    └── Model  (a 3D object placed in a Room)
                          └── BrowserInteraction  (tabbed embedded browser on the Model)
                                ├── Tab: External URL  (admin-set, role-gated)
                                └── Tab: Config Panel  (per-setting role mapping)
```

---

## Core Technology Stack

| Component | Technology | Rationale |
|-----------|-----------|-----------|
| 3D Engine | Bevy (Rust) | ECS architecture, native performance, cross-platform |
| Database | SurrealDB (embedded) | Multi-model, graph-capable, runs in-process |
| Auth | Keypair + signed JWT sessions | Pragmatic decentralization; DID-compliant wrapper deferred |
| P2P Discovery | DHT / gossip / libp2p (Rust) | Truly decentralized; no bootstrap server required for data |
| Embedded Browser | wry or egui-based webview | Renders URLs inside Bevy 3D scene |
| 3D Model Format | GLTF/GLB (primary) | Industry standard, Bevy native support |
| Identity (future) | did:fractal (custom DID method) | P2P DID docs via SurrealDB gossip; v2+ |

---

## Constraints

- **Runtime**: Native desktop app (Linux, Windows, macOS via Bevy cross-platform)
- **No central server**: The Fractal is fully P2P; Nodes discover each other via DHT/libp2p
- **Data ownership**: Each Node stores its own Petal data in embedded SurrealDB; visiting peers cache replicas locally; a single operator can run multiple Nodes (multiple Petals)
- **Auth model (v1)**: Each Node has a keypair; issues signed session tokens (JWT) to visiting peers; admin assigns roles to known peer public keys; DID wrapper is v2
- **Role system**: `public` (hardcoded floor, any peer, view-only) → `<custom roles>` (admin-named, permission-assigned) → `admin` (hardcoded ceiling, Node host); scoped per Petal, overridable per Model
- **Browser interaction**: In-world embedded browser with 2 tabs per Model — (1) External URL tab (any URL, role-gated), (2) Config tab (model properties + per-setting role mapping); admin sees the full config screen
- **3D model uploads**: Users upload GLTF/GLB models to their own Node; assets distributed via content-addressed caching across peers

## Non-Goals (v1)

- No mobile or browser-based client (WASM Bevy deferred)
- No full W3C DID method implementation (DID wrapper is v2+)
- No global asset marketplace (assets are per-Node)
- No voice/video communication between peers
- No physics simulation or real-time collaborative editing (concurrent edits)
- No built-in payment or token system

---

## Acceptance Criteria

- [ ] A Node can start, generate a keypair, and publish its presence to the P2P network
- [ ] A Peer can connect to another Node and enter one of its Petals (3D world loads in Bevy)
- [ ] An operator can upload a GLTF/GLB model to their Node via the in-app UI
- [ ] The uploaded model appears in a Room within a Petal, correctly rendered in 3D
- [ ] Approaching/clicking a Model opens an embedded browser with two tabs (URL tab + Config tab)
- [ ] The URL tab loads the admin-configured external URL inside the Bevy scene
- [ ] The Config tab shows model properties; admin sees per-setting role mapping controls
- [ ] The Petal admin can define custom roles (names + permissions) for their Petal
- [ ] A `public` peer (no auth) sees the Petal but cannot access role-gated content
- [ ] A peer with a valid signed session token (authenticated) receives their assigned custom role
- [ ] Role-gated tabs/interactions are hidden or disabled for peers without sufficient role
- [ ] An operator can run multiple Nodes (multiple Petals) simultaneously
- [ ] A visiting peer's Node caches the visited Petal's data locally for offline access
- [ ] The Node host (`admin`) can revoke a peer's session and role assignment

---

## Assumptions Exposed & Resolved

| Assumption | Challenge | Resolution |
|------------|-----------|------------|
| "Digital twin" = industrial replica | What is the PRIMARY domain? | General platform — domain is whatever each Node operator decides |
| One Node per peer | Can operators run multiple instances? | Yes — one operator can spin up multiple Nodes (Petals) |
| Full DID implementation in v1 | What's the simplest decentralized auth? | Keypair + signed JWTs in v1; DID wrapper deferred to v2 |
| Fixed RBAC tiers | How many role levels? | Flexible: admin defines custom named roles between `public` and `admin` |
| Browser shows one thing | What does the embedded browser show? | Tabbed: External URL tab + Config tab; role determines access to each |
| Peers only visit one world | Data storage model? | Peers cache replicas of visited Petals; operator can host multiple Nodes |

---

## Technical Context

**Greenfield project.** Key integration challenges to resolve per track:

1. **Bevy + embedded browser**: Rendering a live browser (wry/webview) inside a Bevy ECS scene requires careful window/texture management. This is the highest technical risk item.
2. **SurrealDB embedded in Bevy**: SurrealDB can run embedded (in-process) via `surrealdb` crate. Bevy system can hold a `Resource<SurrealDB>` handle.
3. **P2P discovery**: `libp2p` (Rust) provides Kademlia DHT, gossip-sub, noise encryption, and yamux multiplexing — all compatible with Bevy's async runtime (Tokio).
4. **GLTF loading**: Bevy natively supports GLTF/GLB via `bevy_gltf`. Dynamic asset loading at runtime (from user upload) requires `AssetServer::load` with a path or bytes.
5. **Signed JWT sessions**: Use `jsonwebtoken` crate; Node's keypair (ed25519 via `ed25519-dalek`) signs tokens.

---

## Ontology (Key Entities)

| Entity | Type | Fields | Relationships |
|--------|------|--------|---------------|
| Fractal | network | id, peer_count, genesis_block | contains many Nodes |
| Node | core domain | id, keypair_pub, owner_did, petals[] | part of Fractal, hosts Petals |
| Petal | core domain | id, node_id, name, rooms[], roles[] | belongs to Node, contains Rooms |
| Room | supporting | id, petal_id, name, models[], access_role | belongs to Petal, contains Models |
| Model | core domain | id, room_id, gltf_url, position, rotation, scale, interaction | placed in Room, has BrowserInteraction |
| BrowserInteraction | supporting | model_id, external_url, min_role_url, min_role_config | attached to Model |
| Role | core domain | id, petal_id, name, permissions[], is_public, is_admin | scoped to Petal |
| Session | supporting | peer_pub_key, petal_id, role_id, jwt_token, expires_at | granted by Node to visiting Peer |
| Peer | external | pub_key, did_str (v2), cached_petals[] | connects to Nodes, holds Sessions |

## Ontology Convergence

| Round | Entity Count | New | Changed | Stable | Stability Ratio |
|-------|-------------|-----|---------|--------|----------------|
| 1 | 5 | 5 | - | - | N/A |
| 2 | 6 | 1 | 0 | 5 | N/A |
| 3 | 7 | 1 | 0 | 6 | 86% |
| 4 | 8 | 1 | 0 | 7 | 88% |
| 5 | 6 | 0 | 0 | 6 | 100% (consolidated) |
| 6 | 7 | 1 | 0 | 6 | 86% |
| 7 | 8 | 1 | 0 | 7 | 88% |
| 8 | 9 | 1 | 0 | 8 | 94% |

---

## Interview Transcript

<details>
<summary>Full Q&A (8 rounds)</summary>

### Round 1
**Q:** The term 'digital twin' traditionally means a digital replica of a physical object or system. But your description sounds closer to a social/collaborative 3D virtual world. What is the PRIMARY real-world domain or use case this ecosystem is built for?
**A:** General Platform / All of the Above — a horizontal platform where any domain can be built by peers.
**Ambiguity:** 73% (Goal: 0.45, Constraints: 0.20, Criteria: 0.10)

### Round 2
**Q:** When you say 'peer-to-peer', what does that mean technically? How do nodes connect?
**A:** Each peer runs a Bevy node (native desktop app). Nodes discover each other via DHT/gossip/libp2p. Truly decentralized.
**Ambiguity:** 65% (Goal: 0.50, Constraints: 0.40, Criteria: 0.10)

### Round 3
**Q:** What is the FIRST thing that should work in a successful v1 demo?
**A:** All three in sequence: upload model → build world → set permissions → connect as peer → trigger browser interaction.
**Ambiguity:** 52% (Goal: 0.55, Constraints: 0.40, Criteria: 0.45)

### Round 4 (Contrarian Mode)
**Q:** Does each peer store a FULL copy of all world data they've visited, or only their own?
**A:** Peers cache and replicate visited worlds; people can spin up multiples of their own worlds.
**Ambiguity:** 46% (Goal: 0.60, Constraints: 0.55, Criteria: 0.45)

### Round 5
**Q:** Map the naming convention (fractals, petals, nodes) to concrete entities.
**A:** Fractal=Network, Node=Peer, Petal=World/Space. Fractal→Node→Petal→Room→Model.
**Ambiguity:** 40% (Goal: 0.75, Constraints: 0.55, Criteria: 0.45)

### Round 6 (Simplifier Mode)
**Q:** For DID-based auth: how does a visited Node know who you are? Simplest decentralized version?
**A:** Simpler: signed JWTs per session. Each Node has keypair, issues signed session tokens. DID wrapper later.
**Ambiguity:** 36% (Goal: 0.75, Constraints: 0.68, Criteria: 0.45)

### Round 7
**Q:** A visitor triggers a 3D Model's interaction — what does the embedded browser show?
**A:** Both tabs: External URL tab + Config tab. Role determines access. Admins get a per-setting role mapping screen.
**Ambiguity:** 26% (Goal: 0.82, Constraints: 0.70, Criteria: 0.68)

### Round 8
**Q:** What are the role levels between 'public' and 'admin', and what scope do they apply at?
**A:** Flexible tiers — admin creates custom named roles with assigned permissions, scoped per Petal, applicable down to per-Model.
**Ambiguity:** 18% (Goal: 0.85, Constraints: 0.82, Criteria: 0.78)

</details>
