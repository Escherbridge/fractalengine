# Specification: Portal URL Persistence + Inspector Settings

## Overview

Expand the inspector panel from a simple transform/URL editor into a full hierarchical configuration panel with persistent portal URLs, a tab system (Info/Settings/Access), support for inspecting every level of the entity hierarchy (Node, Petal, Fractal, Verse), and a per-entity role/access configuration UI.

## Background

The current inspector panel (`InspectorFormState`) displays transform fields and URL fields for the selected node. URLs can be edited and "saved" via `UiAction::SaveUrl`, which updates the in-memory `VerseManager` and sends `DbCommand::UpdateNodeUrl` to SurrealDB. However, the full round-trip (load saved URLs on node selection, display them in the inspector, persist across sessions) has gaps. Additionally, the inspector only works for nodes -- there is no way to inspect or configure petals, fractals, or verses. The RBAC system (roles, permissions, peer access) has no UI representation yet.

This track builds out the inspector into the primary configuration surface for the entire entity hierarchy, preparing the foundation for full RBAC management in the UI.

## Cross-Track Dependencies

This track depends on the **Shared Peer Infrastructure** track (`shared_peer_infra_20260419`) for:
- `NodeIdentity` resource — required for admin determination (Phase 4)
- `PeerRegistry` resource — provides composite peer data for the Access tab (Phase 4)
- `RoleManager` resource — handles hierarchical role resolution, assignment, invites (Phase 4)
- `LocalUserRole` resource — replaces the dead `is_admin: bool` field, uses hierarchical resolution (Phase 4)
- `RoleLevel` enum — role hierarchy with `Owner > Manager > Editor > Viewer > None` (Phase 4)
- Peer presence system — provides online/offline status for Access tab entries (Phase 4)
- Canonical `did:key` format — all peer identifiers use consistent format
- Scope string utilities (`build_scope`, `parse_scope`) — for hierarchy-aware role queries (Phase 4)

Phases 1-3 of this track are **independent** of the shared infrastructure and can proceed immediately. Phase 4 (Access tab / Peer Management) requires shared infrastructure to be complete.

## Functional Requirements

### FR-1: Portal URL Persistence (Round-Trip)

**Description:** When a node is selected, its saved portal URL must be loaded from SurrealDB and displayed in the inspector's URL field. When the user edits and saves, the URL is persisted to the database and survives application restarts.

**Acceptance Criteria:**
- On node selection, `InspectorFormState.external_url` is populated from the database (via `VerseManager` or a direct DB query).
- Saving a URL via `UiAction::SaveUrl` persists it to SurrealDB via `DbCommand::UpdateNodeUrl`.
- After restarting the application and selecting the same node, the previously saved URL appears in the inspector.
- Empty URL strings are stored as `None` (not as empty strings) in the database.
- URL validation (via `is_url_allowed`) is enforced before persistence -- blocked URLs are rejected with user-visible feedback.

**Priority:** P0

### FR-2: Inspector Tab System

**Description:** Add a horizontal tab bar to the inspector panel with three tabs: Info, Settings, Access. The Info tab contains the current transform fields and URL fields (existing functionality). Settings and Access tabs are initially placeholder panels with descriptive text.

**Acceptance Criteria:**
- A horizontal tab bar appears at the top of the inspector panel when an entity is selected.
- Three tabs are displayed: "Info", "Settings", "Access".
- Clicking a tab switches the inspector content below the tab bar.
- The Info tab shows all current inspector content (transform, URLs).
- The Settings tab shows a placeholder message: "Node settings will appear here."
- The Access tab shows a placeholder message: "Access control will appear here."
- Tab selection persists while the same entity is selected; resets to Info on entity change.
- Tab state is stored in `InspectorFormState` (or a new `InspectorTabState` resource).

**Priority:** P1

### FR-3: Inspectable Hierarchy Levels

**Description:** Extend the inspector to work with petals, fractals, and verses -- not just nodes. The sidebar hierarchy (verse > fractal > petal > node) should allow selecting any level, and the inspector should show appropriate content for that level.

**Acceptance Criteria:**

**Petal Inspection:**
- Selecting a petal in the sidebar opens the inspector with petal-specific content.
- Info tab: petal name, petal ID, node count, creation date.
- Settings tab: placeholder for future petal configuration.
- Access tab: petal-level peer management with explicit + inherited roles (see FR-4).

**Fractal Inspection:**
- Selecting a fractal shows fractal-specific inspector content.
- Info tab: fractal name, fractal ID, petal list/count.
- Settings tab: placeholder.
- Access tab: fractal-level peer management with explicit + inherited roles (see FR-4).

**Verse Inspection:**
- Selecting a verse shows verse-specific inspector content.
- Info tab: verse name, verse ID, fractal list/count, connected peer count.
- Settings tab: placeholder.
- Access tab: verse-level member management, invite generation, default access config (see FR-4).

**General:**
- The inspector title/header indicates the type of entity being inspected (e.g., "Node: MyModel", "Petal: MainWorld").
- Selection at a higher hierarchy level does not deselect the current node (if any) -- the inspector shows the most recently selected entity.
- Each hierarchy level uses the same three-tab layout (Info/Settings/Access).

**Priority:** P2

### FR-4: Peer Management UI (Access Tab)

**Description:** The Access tab is a dedicated peer management surface at every level of the entity hierarchy. Unlike a simple peer list, each hierarchy level has scope-appropriate management controls. Role resolution follows the hierarchical cascade — the UI shows both explicit and inherited roles so operators understand the effective permissions.

This follows the GitHub pattern: org-level member management, team-level access, repo-level collaborators — but applied to the verse/fractal/petal hierarchy.

**Acceptance Criteria:**

**Verse-Level Access Tab:**
- Lists all verse members with their verse-level role and online/offline status.
- Verse owner sees full management controls:
  - Invite generation (produces a `VERSE#<id>` scoped invite link).
  - Role assignment dropdown for each member (manager, editor, viewer).
  - Revoke access button (removes member from verse entirely).
  - Default access setting: toggle between "New members can view everything" (viewer) and "New members need explicit grants" (none).
- Non-owner members see the member list as read-only.
- The verse owner's role is displayed as "owner" with no dropdown (cannot be changed).

**Fractal-Level Access Tab:**
- Lists all peers with access to this fractal (explicit + inherited from verse).
- Shows two columns: "Explicit Role" (set at this fractal) and "Effective Role" (resolved via cascade).
- Inherited roles shown dimmed with "(inherited from verse)" label.
- Fractal managers and verse owner see management controls:
  - Invite generation (produces a `VERSE#<id>-FRACTAL#<id>` scoped invite link).
  - Role assignment dropdown: manager, editor, viewer, none.
  - Setting `none` explicitly blocks inherited verse access for that peer at this fractal.
- The verse owner's entry always shows "owner" and cannot be overridden.

**Petal-Level Access Tab:**
- Lists all peers with access to this petal (explicit + inherited from fractal + verse).
- Shows "Explicit Role" and "Effective Role" columns.
- Inherited roles shown dimmed with "(inherited from fractal)" or "(inherited from verse)" label.
- Petal managers, fractal managers, and verse owner see management controls:
  - Invite generation (produces a `VERSE#<id>-FRACTAL#<id>-PETAL#<id>` scoped invite link).
  - Role assignment dropdown: manager, editor, viewer, none.
  - Setting `none` explicitly blocks inherited access for that peer at this petal.
- The verse owner's entry always shows "owner" and cannot be overridden.

**Invite Link UI (All Levels):**
- "Generate Invite" button opens a dialog with:
  - Role to grant (dropdown: the roles assignable at this scope).
  - Expiry duration (dropdown: 1 hour, 24 hours, 7 days, 30 days, never).
  - Generated link displayed with a "Copy" button.
- The invite encodes the scope via Resource Descriptor format (e.g., `VERSE#abc-FRACTAL#def`).
- Only users with manager or owner role at the current scope can generate invites.

**Permission Presets (All Levels):**
- Quick-set buttons for common configurations:
  - "Public" — all peers get viewer access at this scope.
  - "Members Only" — only explicitly assigned peers have access (sets `none` for unassigned).
  - "Admin Only" — only managers and owner have access.
- Presets modify role assignments at the current scope and send appropriate DB commands via `RoleManager`.

**Role Audit Trail:**
- Role changes are recorded in the op_log following the existing `OpType::AssignRole` pattern.
- The Access tab shows a small "History" link that displays recent role changes for this scope.

**Priority:** P3

## Non-Functional Requirements

### NFR-1: Performance

- Inspector tab switching must be instantaneous (no re-fetch from DB on tab switch).
- Entity info for the selected entity should be cached in the inspector state and refreshed only on selection change or explicit refresh action.
- No per-frame database queries. All data flows through the existing channel/resource pattern.

### NFR-2: Extensibility

- The tab system must be designed so new tabs can be added without modifying existing tab implementations.
- Each hierarchy level's inspector content should be modular (separate rendering functions per level per tab).

### NFR-3: Code Coverage

- >80% test coverage for new inspector state management code.
- Tab switching logic, hierarchy level detection, and URL persistence round-trip must all have unit tests.

## User Stories

### US-1: Node Operator Configuring Portal URL

**As** a Node operator, **I want** to set a portal URL on a node and have it persist across sessions, **so that** I can configure my digital twin once and have visitors see the correct content.

**Given** I select a node in the 3D viewport
**When** I type a URL in the inspector's URL field and click Save
**Then** the URL is stored in SurrealDB and appears when I reselect the node after restart

### US-2: Operator Exploring Entity Hierarchy

**As** a Node operator, **I want** to click on a petal, fractal, or verse in the sidebar and see its properties in the inspector, **so that** I can understand and configure my entire entity hierarchy from one panel.

**Given** I click on a petal name in the sidebar
**When** the inspector opens
**Then** I see the petal's name, ID, node count, and tabs for Info/Settings/Access

### US-3: Admin Managing Access Control

**As** a Node admin, **I want** to see which peers have access to my petal and change their roles, **so that** I can control who can view and edit my 3D spaces.

**Given** I select a petal and open the Access tab
**When** I see the peer list
**Then** I can change a peer's role from "viewer" to "editor" via a dropdown

## Technical Considerations

- **Inspector state architecture:** The current `InspectorFormState` holds text buffers for the node inspector. For multi-level inspection, introduce an `InspectedEntity` enum (`Node(id)`, `Petal(id)`, `Fractal(id)`, `Verse(id)`) to track what is currently being inspected.
- **Tab rendering:** Use egui's `Ui::selectable_label` or `egui::TopBottomPanel` with horizontal layout for tab buttons. Store active tab in a resource.
- **URL loading on selection:** When `NodeManager.selected` changes, the inspector should query `VerseManager` for the node's stored URL and populate `InspectorFormState.external_url`. This is currently partial -- needs verification and completion.
- **Database interaction:** All DB reads go through the existing `DbCommandSender` channel pattern. Consider a `DbCommand::GetNodeUrl` query or ensure `VerseManager` caches URLs from the initial petal load.
- **Access tab data:** Role assignments are stored in SurrealDB per entity. The Access tab needs a `DbCommand` to fetch peer roles for a given entity scope.
- **RoleManager integration:** The Access tab (Phase 4) uses `RoleManager` for all role operations: `resolve_role` for display, `assign_role` for changes, `get_scope_members` for the peer list, and invite generation. It reads composite peer data (display names, online status) from `PeerRegistry`. The Access tab never queries the role DB directly.
- **Hierarchical scope awareness:** Each hierarchy level's Access tab builds the appropriate scope string via `build_scope()` and passes it to `RoleManager`. The UI shows both explicit and effective (resolved) roles using `resolve_role` per peer.
- **Role audit trail:** Role changes must write to the op_log following the existing pattern in queries.rs (OpType::AssignRole). The scope string is included in the op_log payload so auditors can see which hierarchy level was affected.
- **Invite generation:** Uses `RoleManager::generate_invite` (or companion `InviteManager`) to produce scope-encoded invite links. The invite UI is consistent across all hierarchy levels, differing only in the scope encoded in the link.

## Out of Scope

- Custom role creation UI (the 5 role levels are fixed: owner, manager, editor, viewer, none).
- P2P propagation of role changes (deferred to the Mycelium Live track).
- Real-time collaboration on inspector changes.
- Drag-and-drop reordering of the entity hierarchy.

## Open Questions

1. **Multi-selection:** Should the inspector support inspecting multiple entities simultaneously (e.g., showing shared fields)? Current assumption: single selection only.
2. **Inspector width:** Should the inspector panel width be fixed or resizable? Currently it shares space with the portal panel.
3. **Tab persistence:** Should the active tab be remembered per entity type (e.g., always open Settings for petals) or reset to Info on every selection change?
