# Initial Concept

FractalEngine is a decentralized, peer-to-peer 3D digital twin platform where any operator can run a Node (native Bevy desktop app), host multiple Petals (3D worlds/spaces), populate them with uploaded 3D Models (GLTF/GLB), and grant other peers role-based access — all without a central server. Each 3D Model can have an embedded in-world browser with two tabs: a public-facing external URL (configurable by the Petal admin) and an internal config panel. Roles are flexible (admin-defined custom tiers between `public` and `admin`), scoped per Petal and overridable per Model. The full network of connected Nodes is called the Fractal. The platform is domain-agnostic — virtual offices, industrial digital twins, showrooms, or social spaces can all be built on it. Naming convention follows fractal/botanical metaphors: Fractal (network) → Node (peer) → Petal (world/space) → Room (zone) → Model (3D object).

---

# Product Guide

## Vision

FractalEngine empowers small businesses and teams to own their 3D collaborative infrastructure — no cloud vendor, no monthly subscription, no data lock-in. A business deploys a Node on any machine they control, builds 3D Petals representing their spaces (showroom, office, factory floor, portfolio), and invites peers to interact within those spaces under precise role-based access control. The Fractal is the collective network of all such Nodes, federated without a central authority.

## Target Users

### Primary: Small Businesses & Teams
Organizations that need a self-hosted 3D collaborative environment for internal use or client-facing purposes. Use cases include:
- Virtual product showrooms with interactive product pages (browser-triggered on 3D models)
- Digital twins of physical office or factory spaces for remote team coordination
- Private 3D meeting/presentation rooms with role-controlled access for clients vs. staff
- Interactive training environments with embedded instructional web content

### Secondary: Indie Developers & Hackers
Technically capable builders who want to self-host a decentralized 3D world without cloud dependencies. They extend the platform, build custom Petals, and contribute to the Fractal network.

### Tertiary: Creative Professionals
3D artists, architects, and designers who use Petals as interactive portfolio or showroom spaces, embedding links to their work via the browser interaction system.

---

## Core Value Proposition

| Value | Description |
|---|---|
| **Self-sovereign** | The Node operator owns all data. No third-party cloud holds world state or assets. |
| **Zero infrastructure dependency** | A single binary runs the entire stack — 3D engine, database, P2P networking, and browser overlay. |
| **Federated, not centralized** | Nodes connect peer-to-peer. There is no master server. Any Node can host any number of Petals. |
| **Role-precise access** | Every interaction — entering a Petal, viewing a browser tab, editing a Model — is gated by admin-defined roles. |
| **Interactive 3D content** | 3D Models are not just visual — they carry embedded browser interfaces linking to any web content or internal config. |

---

## Entity Hierarchy (The Fractal Naming System)

```
Fractal  — the entire P2P network of all connected Nodes
  └── Node  — one peer/operator running the Bevy desktop app (the Node host is the admin)
        └── Petal  — a 3D world/space hosted by the Node
              └── Room  — a zone or area within a Petal
                    └── Model  — a 3D object (GLTF/GLB) placed in a Room
                          └── BrowserInteraction  — tabbed embedded browser on the Model
                                ├── Tab 1: External URL  (admin-configured, role-gated)
                                └── Tab 2: Config Panel  (model properties + per-setting role mapping)
```

---

## Key Features

### 1. Self-Hosted Node
- Single Rust binary embedding Bevy 3D engine, SurrealDB, and P2P networking
- Generates an ed25519 keypair on first launch stored in the OS keychain
- The Node operator is automatically the `admin` for all their Petals
- Multiple Petals can be hosted per Node

### 2. 3D World Building (Petals & Rooms)
- Operators create Petals (worlds) and subdivide them into Rooms (zones)
- GLTF/GLB model uploads populate Rooms with positioned, scaled 3D objects
- Content-addressed asset distribution: assets shared peer-to-peer by BLAKE3 hash
- Dead-reckoning on moving entities reduces bandwidth by 60–90%

### 3. Embedded Browser Interaction
- Approaching or clicking a Model opens an overlay browser window
- Two tabs per Model:
  - **External URL tab**: loads any admin-configured URL (product page, web app, video)
  - **Config tab**: shows Model properties and per-setting role mapping (admin full control)
- Role determines which tabs are visible and editable

### 4. Decentralized RBAC
- Three hardcoded tiers: `public` (floor) → custom named roles → `admin` (ceiling)
- Admins define custom roles with named permissions scoped per Petal
- Role assignments stored in SurrealDB with record-level enforcement
- Roles overridable at the Room or Model level
- Signed revocations propagate via P2P gossip within seconds

### 5. Peer Authentication
- Visiting peers present their ed25519 public key
- Host Node issues signed JWT sessions (`sub: did:key:<pub>`)
- Session cache with 60s TTL and mandatory re-validation
- Public (unauthenticated) access available for visitor-facing Petals
- DID-compliant identity (W3C did:key) — wallet integration deferred to v2

### 6. Petal Caching & Offline Access
- Visiting peers cache Petal data (SurrealDB + assets) locally
- Offline mode loads previously-visited Petals without network
- Operators can define per-Petal persistence tier: EPHEMERAL / CACHED / REPLICATED

---

## Non-Goals (v1)

- No mobile or WASM browser client (deferred to v2)
- No full W3C DID resolver or VC wallet integration (v2)
- No built-in voice/video between peers
- No global asset marketplace
- No FBX/OBJ format support — GLTF/GLB only (operators use Blender to convert)
- No concurrent real-time collaborative editing
- No payment or token system

---

## Success Metrics (v1)

- A Node can start and connect to another Node within 30 seconds on a LAN
- A Petal with 10 GLTF models loads in under 5 seconds for a visiting peer
- Role-based access correctly gates browser tabs for public vs. authenticated peers
- A signed revocation reaches all connected peers within 5 seconds
- An operator can complete the full flow (upload model → place in Room → set role → invite peer) without reading documentation
- A previously-visited Petal loads from local cache when the host Node is offline

---

## Competitive Differentiation

| Platform | Centralized | Self-Hosted | 3D | RBAC | Embedded Browser | P2P |
|---|---|---|---|---|---|---|
| Gather.town | Yes | No | No | Basic | No | No |
| Spatial.io | Yes | No | Yes | Basic | No | No |
| Vircadia | No | Yes | Yes | Yes | Yes (CEF) | Partial |
| **FractalEngine** | **No** | **Yes** | **Yes** | **Flexible** | **Yes (wry)** | **Yes** |

FractalEngine's differentiator: a single binary, no infrastructure dependency, with the most flexible RBAC in the space and a browser-overlay interaction model that turns any 3D object into an interactive interface.
