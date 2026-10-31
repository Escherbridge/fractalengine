---
type: Product Definition
title: FractalEngine — Spatial Analytics Engine on a P2P 3D Twin Substrate
tags: [product, analytics, bi-egress, gis, p2p, digital-twin]
timestamp: 2026-10-31T00:00:00Z
resource: ./roadmap.md
---

# FractalEngine

## What FractalEngine is (2026-07): a spatial analytics engine

FractalEngine is a self-hosted **spatial analytics / reporting engine**. You
build or import 3D/GIS spatial worlds (Petals), attach IoT and entity data to
them, and export live queries into your existing BI stack: copy a SQL string,
an API URL, or a DuckDB connection snippet out of the GIS panel and paste it
into PowerBI, a spreadsheet, or a notebook. FractalEngine is the spatial
backend; your BI tool stays the reporting frontend.

The P2P 3D world-building layer described under
[Foundational concept](#foundational-concept-2026-03-historical) is the
substrate this runs on — the foundation, not the headline. Strategic
direction: see [roadmap.md](./roadmap.md) (2026-07-14 repositioning;
2026-10-31 open-core alignment). Current stack details: see
[tech-stack.md](./tech-stack.md).

**Open core (2026-10-31):** this repository is the Apache-2.0 open core.
Everything a self-hoster needs lives here, and hosted services are built on its
public seams elsewhere. The line and the seams are in [open-core.md](./open-core.md).

## Target Users

### Primary: Analysts & operations teams doing spatial reporting
Teams that already live in PowerBI, spreadsheets, or notebooks and need a
spatial backend they control:
- Facilities / infrastructure reporting over a 3D twin of a real site (map
  tiles at real-world scale, paths, sensors, assets)
- IoT dashboards where readings are queryable as spatial rows seconds after
  ingestion
- GIS-flavored BI: distance/within predicates, GeoJSON and GeoParquet egress,
  CRS-correct coordinates

### Secondary: World-builders, indie developers & hackers
Technically capable builders who want a self-hosted 3D world without cloud
dependencies. They build Petals, import maps, script the engine via the plugin
system (Rhai/WASM) and MCP tools, and contribute to the Fractal network.

### Tertiary: Creative professionals
3D artists, architects, and designers using Petals as interactive portfolio or
showroom spaces (the original concept's audience — still supported, no longer
the design driver).

---

## Core Value Proposition

| Value | Description |
|---|---|
| **BI egress, not BI lock-in** | Copy a query URL / SQL string / DuckDB snippet from the GIS panel into the tools you already use. FractalEngine never tries to be your dashboard. |
| **Self-sovereign** | The operator owns all data. No third-party cloud holds world state, assets, or query results. |
| **Single binary** | One Rust binary embeds the 3D engine, database, query engine, HTTP API, and P2P networking. A headless relay binary serves the same data GPU-less. |
| **Spatially correct** | Map-authoritative real-world scale; petal-local meters in the store, lat/lon at the API edge; exports are CRS-stamped. |
| **Federated, not centralized** | Peers connect P2P. Any operator can host any number of Petals; maps distribute as content-addressed packages. |
| **Role-precise access** | Every read and write is gated by a deny-by-default policy engine over a fixed role hierarchy. |

---

## Entity Hierarchy

The shipped hierarchy (scope strings `VERSE#v-FRACTAL#f-PETAL#p`):

```
Verse    — top-level tenancy/network scope
  └── Fractal  — a federation within a Verse
        └── Petal  — a 3D world/space (owns terrain, map scale, GIS origin)
              └── Node  — an entity placed in a Petal (3D object, path,
                          sensor, primitive) with typed properties
```

RBAC resolves hierarchically down this chain: a role granted at Verse scope
applies to every Fractal/Petal beneath it unless a narrower scope overrides
it. Note: **Node is a scene entity inside a Petal, not a peer** — a machine
running the app is a *peer*. (The historical concept used Node-as-peer plus
Room/Model tiers; those were never built — see below.)

---

## Status: what works today (re-audited 2026-10-31)

Three read-only audits re-checked this list against the code on 2026-10-31.
"Wired" means the capability is reachable from a shipped binary. The test
sweep is 4008 passed / 0 failed, but hands-on checking in the app is still
rare (see "Not yet" below).

**Shipped and wired**
- Native Bevy desktop app (`fractalengine`) + headless relay (`fe-relay`)
- Petal terrain from map tiles / imported maps, with map-authoritative
  real-world scale and a scale-bar ruler; terrain proposals with a cut/fill
  delta view and a volume report
- GPX import, path/pen editing, road tools, path asset stamping
- Entity store with an HLC-stamped op-log, and the canonical signed append log
  (`fe-canonical-log`) on the production write path
- BI egress: `/api/v1/query` single-SELECT SQL with cost/row/timeout guards,
  GeoJSON GIS endpoints, GeoParquet/CSV export, ed25519-signed share URLs and a
  Copy-for-BI panel. Live-verified against DuckDB (`scripts/bi-egress-verify.ps1`).
- IoT ingestion: `POST /api/v1/petals/{id}/iot/readings` into queryable spatial rows
- MCP server: 31 tools always advertised; `screenshot` on GUI hosts; `sim_*`
  only on sim-lab hosts
- Real iroh-docs replication (iroh 1.3); content-addressed asset and tileset
  streaming; hexon map packages publish/import with fe-policy checks on
  publish/install
- Deny-by-default RBAC (`fe-policy`) with per-grantor role propagation

**Built but not wired**
- Rhai + WASM plugin system (`fe-plugin` / `fe-sdk`). No binary loads it.
  → [plugin_host_revival_20261031](./tracks/plugin_host_revival_20261031/)
- `fe-network` libp2p thread: spawned, but it carries no traffic (all real
  P2P runs through `fe-sync`)
- `LicenseType::Paid` / `encrypted_key` / `free_entries` in the hexon manifest:
  the fields are defined, but nothing reads them
  → [hexon_entitlements_20261031](./tracks/hexon_entitlements_20261031/)
- GraphQL (`fe-query/src/graphql`) is a placeholder

**Not yet**
- No `v*` release has ever been cut. Binaries are unsigned and have no
  installer. → [release_ci_20260429](./tracks/release_ci_20260429/)
- SSO (spec only); legacy op-log entries carry placeholder signatures
  (`SECURITY.md`)
- Hands-on checking in the app is thin. The one session on 2026-10-25 found 6
  bugs, all since fixed.
- Map foundry + hosted registry/marketplace live outside this repository (see
  [open-core.md](./open-core.md))

---

## Foundational concept (2026-03, historical)

The original vision — kept because the roadmap **supplements** it, it does not
erase it: FractalEngine as a decentralized, peer-to-peer 3D digital twin
platform. Any operator runs a peer (native Bevy desktop app), hosts multiple
Petals, populates them with uploaded GLTF/GLB models, and grants other peers
role-based access — all without a central server. 3D objects can carry an
embedded in-world browser portal (a Portal URL configured by the Petal owner)
turning any object into an interactive interface. Businesses own their 3D
collaborative infrastructure: no cloud vendor, no subscription, no data
lock-in — virtual offices, industrial twins, showrooms, or social spaces.

Parts of that concept that were **never built and are not current
vocabulary**: the `Room` and `Model` entity tiers, `BrowserInteraction` as an
entity, Node-as-peer, and admin-defined custom roles. The portal/webview,
P2P distribution, and self-sovereignty pillars all shipped and carry the
analytics identity today.

---

## RBAC

- Fixed role hierarchy: **Owner > Manager > Editor > Viewer > None**, assigned
  per scope (Verse / Fractal / Petal) and resolved hierarchically.
- Enforcement is a deny-by-default policy engine (`fe-policy`) consulted on
  every write path: database writes, map-package install, and the sync write
  gate all route through it. Authorization never lives in Bevy systems or UI
  code.
- Signed revocations propagate via P2P gossip.

---

## Non-Goals (v1)

- **Not a dashboard/BI tool** — egress hands data to PowerBI / spreadsheets /
  DuckDB; FractalEngine does not grow charting or report layouts
- **Not a general-purpose OLAP warehouse** — the query surface is guarded
  single-SELECT spatial egress, not arbitrary analytical workloads
- **Not a hosted SaaS** — the engine is a self-hosted single binary, and
  operators run their own peers and relays. Hosted/managed offerings are built
  on this repo's public seams, outside it ([open-core.md](./open-core.md))
- **No marketplace or service operations in this repo** — this repo owns the
  open format, the reference single-tenant registry, registry protocol v1 and
  client-side entitlement *verification*. Multi-tenant hosting, curation and
  entitlement issuance live outside it (D-12, realised 2026-10-31 in
  [open-core.md](./open-core.md))
- No mobile or WASM browser client (deferred)
- No full W3C DID resolver or VC wallet integration (deferred)
- No built-in voice/video between peers
- No FBX/OBJ format support — GLTF/GLB only (convert via Blender)
- No payment processing or token system — the engine only *verifies* signed
  entitlement tokens (S-ENTITLEMENT); it never handles payment

---

## Success Metrics

**Analytics (primary)**
- A user can copy a query URL / SQL string from the GIS panel and get rows in
  PowerBI or a spreadsheet without reading documentation
- Exported coordinates are metrically correct: CRS-stamped, map-authoritative
  scale, lat/lon at the edge
- An IoT reading is queryable as a spatial row within seconds of ingestion
- Query results are filtered per requester role by the policy engine

**Platform (substrate)**
- A peer can start and connect to another peer within 30 seconds on a LAN
- A Petal with 10 GLTF models loads in under 5 seconds for a visiting peer
- A signed revocation reaches all connected peers within 5 seconds
- An operator completes upload model → place in Petal → set role → invite peer
  without reading documentation

---

## Competitive Frame

**Primary frame (2026-07): spatial analytics.** The gap FractalEngine fills
is *PowerBI-class reporting with a real spatial/3D backend you self-host*.
GIS plugins for BI tools flatten the world to 2D layers; GIS servers
(ArcGIS/Cesium class) don't hand you a copy-paste SQL/DuckDB egress. The
aspirational comparison set is industrial spatial-analytics tooling like
**AVEVA PI System** and **Neara** — asset-centric, metrically correct,
query-first — delivered as a self-hosted single binary with P2P data
distribution and live 3D editing in the same tool.

**Historical frame: virtual-world platforms.** The original comparison set
(Gather.town, Spatial.io, Vircadia — centralized vs. self-hosted, embedded
browser, P2P) still describes the substrate, but it is no longer the market
FractalEngine is positioned in.
