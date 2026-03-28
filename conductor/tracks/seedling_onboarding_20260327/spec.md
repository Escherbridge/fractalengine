# Specification: Seedling Onboarding -- Local/Peer Instance Bootstrap + Entity CRUD

## Overview

This track delivers the first-launch onboarding experience, local account/profile management, full CRUD operations for the entity hierarchy (Verses, Fractals, Petals, Rooms, Models/Nodes), and initial peer connection setup. It bridges the existing Wave 1 infrastructure (identity, database schema, RBAC, networking) with the Wave 2 interactive features by giving operators the UI and data layer needed to create, read, update, and delete their own content — and to connect with other nodes.

## Background

Wave 1 established:
- **Root Identity**: ed25519 keypair generation, OS keychain storage, `did:key` derivation, JWT minting (`fe-identity`)
- **Petal Soil**: SurrealDB schema for petals, rooms, models, roles, op-log, verses, fractals, nodes, assets (`fe-database/src/schema.rs`)
- **Petal Gate**: RBAC enforcement at DB layer (`require_write_role`), session cache, auth handshake (`fe-auth`)
- **Gardener Console**: egui-based admin UI shell with sidebar, inspector, toolbar, dashboard (`fe-ui`)
- **Mycelium Network**: libp2p DHT + iroh data transport (`fe-network`)
- **Seed data**: `seed_default_data()` creates a Genesis Verse/Fractal/Petal/Rooms on first launch

What is missing:
- No first-launch onboarding UX (welcome screen, node naming, guided Petal creation)
- No user-facing CRUD for Verses, Fractals, Petals, Rooms, or Nodes beyond the hardcoded seed
- No profile/account management (display name, node metadata editing)
- No peer connection management UI
- `DbCommand` only supports `Ping`, `Seed`, `Shutdown` — no CRUD commands flow through the channel
- Sidebar shows counts but no interactive list items (cannot click a petal to manage it)

## Functional Requirements

### FR-1: First-Launch Detection & Welcome Screen
**Description:** On application start, detect whether this is a first launch (no keypair in keychain, no verse in DB). If so, present a full-screen welcome overlay in egui guiding the operator through initial setup.
**Acceptance Criteria:**
- AC-1.1: If `fe_identity::keychain::load_keypair("local-node")` fails, the app enters onboarding mode.
- AC-1.2: Welcome screen displays the FractalEngine name, a brief description, and a "Get Started" button.
- AC-1.3: Welcome screen blocks all other UI (toolbar, sidebar, inspector) until onboarding completes.
**Priority:** P0

### FR-2: Node Identity Setup Step
**Description:** After the welcome screen, guide the operator to name their node and confirm keypair generation.
**Acceptance Criteria:**
- AC-2.1: Operator enters a display name for their node (1-64 chars, non-empty after trim).
- AC-2.2: The keypair is generated (if not already present) and stored in OS keychain via `fe_identity::keychain::store_keypair`.
- AC-2.3: The `did:key` is derived and displayed to the operator (read-only, copyable).
- AC-2.4: A `node_profile` table record is created in SurrealDB with `node_id`, `display_name`, `did_key`, `created_at`.
**Priority:** P0

### FR-3: Initial Petal Creation Wizard
**Description:** After node setup, guide the operator to create their first Petal (3D world). This reuses and extends the existing `PetalWizardState` in `fe-ui`.
**Acceptance Criteria:**
- AC-3.1: Wizard collects: Petal name (required, 1-128 chars), description (optional), visibility (public/private/unlisted dropdown), initial tags.
- AC-3.2: On confirm, a Verse, Fractal, and Petal are created via `DbCommand` (not hardcoded seed).
- AC-3.3: A default "Lobby" room is created inside the new Petal.
- AC-3.4: The operator is assigned the `owner` role for the new Petal.
- AC-3.5: Onboarding completes and the main Gardener Console UI becomes active with the new Petal selected.
**Priority:** P0

### FR-4: Node Profile Management
**Description:** Allow the operator to view and edit their node profile after onboarding.
**Acceptance Criteria:**
- AC-4.1: A "Node Settings" panel is accessible from the sidebar or toolbar.
- AC-4.2: Operator can edit: display name, node description (optional, 0-512 chars).
- AC-4.3: `did:key` is displayed read-only.
- AC-4.4: Changes persist to the `node_profile` table in SurrealDB.
- AC-4.5: Changes are written to the op-log with a new `UpdateNodeProfile` op type.
**Priority:** P1

### FR-5: Verse CRUD
**Description:** Full create, read, update, delete operations for Verses.
**Acceptance Criteria:**
- AC-5.1: **Create**: Operator can create a new Verse with a name (1-128 chars). A `verse_member` record is created with the operator's `did:key` as the founding member.
- AC-5.2: **Read/List**: All Verses the operator belongs to are listed in the sidebar. Selecting a Verse sets `NavigationState::active_verse_id`.
- AC-5.3: **Update**: Operator can rename a Verse they created.
- AC-5.4: **Delete**: Operator can delete a Verse they created (only if it has no Fractals). Confirmation dialog required.
- AC-5.5: All mutations write to the op-log with appropriate `OpType` variants.
**Priority:** P1

### FR-6: Fractal CRUD
**Description:** Full CRUD for Fractals within a Verse.
**Acceptance Criteria:**
- AC-6.1: **Create**: Operator creates a Fractal within the active Verse, providing name and optional description.
- AC-6.2: **Read/List**: Fractals in the active Verse are listed under the "Fractal" sidebar section. Selecting one sets `NavigationState::active_fractal_id`.
- AC-6.3: **Update**: Operator can edit Fractal name and description.
- AC-6.4: **Delete**: Operator can delete a Fractal (only if it has no Petals). Confirmation dialog required.
- AC-6.5: All mutations write to the op-log.
**Priority:** P1

### FR-7: Petal CRUD
**Description:** Full CRUD for Petals within a Fractal, extending the existing `create_petal` query.
**Acceptance Criteria:**
- AC-7.1: **Create**: Operator creates a Petal providing name, description, visibility, tags. Linked to the active Fractal. A default "Lobby" Room is auto-created. Owner role is auto-assigned.
- AC-7.2: **Read/List**: Petals in the active Fractal are listed in the sidebar "Petals" section with real data (not just counts). Clicking selects and sets `NavigationState::active_petal_id`.
- AC-7.3: **Update**: Operator can edit name, description, visibility, and tags via the existing `SpaceManager::update_petal_metadata`. Name updates require a new query (not currently supported).
- AC-7.4: **Delete**: Operator can delete a Petal and its child Rooms/Nodes. Confirmation dialog with cascade warning. Writes `DeletePetal` op-log entry.
- AC-7.5: Petal list supports the existing search/filter from `SearchQuery`.
**Priority:** P0

### FR-8: Room CRUD
**Description:** Full CRUD for Rooms within a Petal, extending the existing `create_room` query.
**Acceptance Criteria:**
- AC-8.1: **Create**: Operator creates a Room within the active Petal, providing name, optional description, optional bounds, optional spawn point.
- AC-8.2: **Read/List**: Rooms in the selected Petal are listed (collapsible under the Petal in sidebar or in the inspector).
- AC-8.3: **Update**: Operator can edit Room metadata via `SpaceManager::update_room_metadata`.
- AC-8.4: **Delete**: Operator can delete a Room (moves contained Nodes to "unassigned" or deletes them). Confirmation required.
- AC-8.5: RBAC enforced: only `owner`/`editor` roles can mutate.
**Priority:** P1

### FR-9: Node (3D Object) CRUD
**Description:** Full CRUD for Nodes (interactive 3D objects) within a Petal.
**Acceptance Criteria:**
- AC-9.1: **Create**: Operator places a new Node by selecting an asset and specifying position/rotation/scale, or by using defaults. Uses existing `node` schema.
- AC-9.2: **Read/List**: Nodes in the selected Petal are listed in the sidebar "Nodes" section with display names.
- AC-9.3: **Update**: Operator can edit Node display_name, position, elevation, rotation, scale, interactive flag, and asset_id via the inspector panel.
- AC-9.4: **Delete**: Operator can delete a Node. Confirmation required.
- AC-9.5: RBAC enforced at DB layer.
**Priority:** P1

### FR-10: Peer Connection Setup
**Description:** Allow the operator to connect to other nodes and accept incoming peer connections.
**Acceptance Criteria:**
- AC-10.1: A "Peers" section in the sidebar shows connected peers (count already shown in status bar).
- AC-10.2: Operator can add a peer by entering their `did:key` or multiaddr.
- AC-10.3: Operator can view connected peers' display names and `did:key`.
- AC-10.4: Operator can disconnect/block a peer.
- AC-10.5: Peer connections use the existing libp2p/iroh stack from `fe-network`.
**Priority:** P2

### FR-11: DbCommand Channel Extension
**Description:** Extend the `DbCommand`/`DbResult` enum to carry CRUD operations from Bevy systems to the database thread.
**Acceptance Criteria:**
- AC-11.1: New `DbCommand` variants for each CRUD operation (e.g., `CreateVerse { name }`, `ListVerses`, `UpdatePetal { ... }`, `DeleteRoom { ... }`, etc.).
- AC-11.2: Corresponding `DbResult` variants carry success payloads or error strings.
- AC-11.3: The database thread's event loop dispatches to the appropriate query function.
- AC-11.4: No `block_on()` in Bevy systems — all DB interaction via the channel.
**Priority:** P0

## Non-Functional Requirements

### NFR-1: Performance
- Onboarding flow must complete (keypair gen + DB writes) in under 2 seconds on a mid-range machine.
- Entity list rendering must handle 100+ Petals at 60fps in egui.
- DB queries for list operations must return within 50ms for up to 1000 records.

### NFR-2: Security
- Private key material never displayed in UI; only `did:key` shown.
- All DB mutations gated by RBAC (`require_write_role` or verse-level ownership check).
- Op-log entries for all state changes (audit trail).
- No `unwrap()` or `expect()` in production code paths.

### NFR-3: Data Integrity
- Delete operations are cascading where specified and guarded by confirmation dialogs.
- Verse deletion blocked if Fractals exist; Fractal deletion blocked if Petals exist.
- All IDs are ULIDs for global uniqueness and chronological sorting.

### NFR-4: Accessibility
- All egui inputs have descriptive labels.
- Keyboard navigation for onboarding wizard steps (Tab/Enter).
- Error states clearly communicated with colored text (using existing theme constants).

## User Stories

### US-1: First-Time Operator
**As** a first-time operator,
**I want** a guided setup experience when I launch FractalEngine for the first time,
**So that** I can quickly create my identity and first 3D space without reading documentation.

**Given** the application has never been launched on this machine,
**When** I start FractalEngine,
**Then** I see a welcome screen and am guided through naming my node and creating my first Petal.

### US-2: Returning Operator Managing Petals
**As** a returning operator,
**I want** to create, edit, and delete Petals from the Gardener Console,
**So that** I can manage my 3D spaces without touching the database directly.

**Given** I have completed onboarding and have at least one Petal,
**When** I click "New Petal" in the sidebar,
**Then** I can fill in the name, description, visibility, and tags, and the Petal appears in my sidebar list.

### US-3: Operator Organizing Rooms
**As** an operator managing a Petal,
**I want** to create and organize Rooms within my Petal,
**So that** I can subdivide my 3D space into logical zones.

**Given** I have a Petal selected,
**When** I create a new Room with a name and optional bounds,
**Then** the Room appears in the Room list and I can edit its properties.

### US-4: Operator Placing 3D Objects
**As** an operator,
**I want** to place, configure, and remove 3D objects (Nodes) in my Petals,
**So that** I can populate my spaces with interactive content.

**Given** I have a Petal with at least one Room,
**When** I create a Node by selecting an asset and position,
**Then** the Node appears in the viewport and in the Nodes list, and I can edit its properties via the inspector.

### US-5: Operator Connecting Peers
**As** an operator,
**I want** to connect my node to other operators' nodes,
**So that** I can share my Petals and visit theirs.

**Given** I know another operator's `did:key` or network address,
**When** I add them as a peer in the Peers section,
**Then** our nodes establish a P2P connection and I can see their public Petals.

## Technical Considerations

### Database Schema Additions
- New `node_profile` table: `{ node_id: string, display_name: string, description: option<string>, did_key: string, avatar_asset_id: option<string>, created_at: string, updated_at: string }`
- New `OpType` variants: `CreateVerse`, `UpdateVerse`, `DeleteVerse`, `CreateFractal`, `UpdateFractal`, `DeleteFractal`, `UpdateNodeProfile`, `CreateNode`, `UpdateNode`, `DeleteNode`, `DeleteRoom`

### DbCommand Channel Design
The existing `DbCommand` enum must be extended significantly. Consider using a boxed inner enum or a request/response ID pattern to avoid a massive flat enum. Each command variant should carry all parameters needed by the corresponding query function.

### Onboarding State Machine
Use a Bevy `States` enum (`OnboardingState`) with variants: `Welcome`, `NodeSetup`, `PetalCreation`, `Complete`. The Gardener Console UI system checks this state and renders the appropriate screen.

### Existing Code Reuse
- `PetalWizardState` in `fe-ui/src/atlas/petal_wizard.rs` — extend for onboarding wizard
- `SpaceManager` in `fe-database/src/space_manager.rs` — already has update/search queries for petals, rooms, models
- `NavigationState` in `fe-ui/src/plugin.rs` — already tracks active verse/fractal/petal
- `DashboardState` in `fe-ui/src/atlas/dashboard.rs` — already tracks counts

### Thread Safety
All CRUD operations must flow through the `DbCommand`/`DbResult` crossbeam channel. Bevy systems send commands and poll results — never calling async DB functions directly. This preserves the T1/T3 thread isolation mandated by `tech-stack.md`.

## Dependencies

| Dependency | Track | Status | What We Need |
|---|---|---|---|
| Ed25519 keypair + keychain | Root Identity | Complete | `NodeKeypair`, `store_keypair`, `load_keypair` |
| SurrealDB schema + RBAC | Petal Soil | Complete | All table schemas, `require_write_role` |
| Auth handshake + sessions | Petal Gate | Complete | JWT minting, session cache |
| Admin UI shell | Gardener Console | Complete | egui panels, sidebar, inspector, dashboard |
| libp2p + iroh transport | Mycelium Network | Complete | Peer discovery, data sync primitives |
| Security hardening | Thorns & Shields | Pending | Op-log signature enforcement (can proceed without, use placeholder sigs) |

## Out of Scope

- Avatar image upload and rendering (deferred — `avatar_asset_id` field is a placeholder)
- Verse member invitation flow for remote peers (deferred to a dedicated "Verse Invitations" track)
- Real-time collaborative editing of entities
- Undo/redo for CRUD operations
- Bulk import/export of entities
- Mobile or WASM client
- Voice/video between peers

## Open Questions

1. **Cascade behavior on Petal delete**: Should deleting a Petal cascade-delete all Rooms, Nodes, and Assets? Or should assets be reference-counted and retained if used elsewhere? *Proposed: cascade Rooms/Nodes, retain Assets.*
2. **Node profile vs. Verse identity**: Should the operator have a single global profile, or per-Verse display names? *Proposed: single global `node_profile`, with per-Verse overrides deferred to v2.*
3. **Onboarding skip**: Should there be a "Skip" option that creates defaults automatically (like the current `seed_default_data`)? *Proposed: yes, as a "Quick Start" button alongside the guided flow.*
