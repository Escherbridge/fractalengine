# Specification: User Profile Manager

## Overview

Add a user identity and profile management system to FractalEngine. This includes displaying the current user's profile in the UI, editing profile fields (display name, avatar), managing cryptographic identities (create, switch, import, export), and syncing profile data to peers via the P2P layer.

## Background

FractalEngine uses ed25519 keypairs for peer identity. On first launch, a keypair is generated and stored in the OS keychain. The public key serves as the peer's identity (`did:key:z6Mk...`). Currently there is no UI to view, edit, or manage this identity. Users cannot set a display name, switch between identities, or see their own profile information anywhere in the application. The peer list in future inspector Access tabs (Track: Inspector Settings) will need display names and profile data to show meaningful peer entries instead of raw public keys.

This track builds the profile management layer that other features depend on for human-readable peer identification.

## Cross-Track Dependencies

This track depends on the **Shared Peer Infrastructure** track (`shared_peer_infra_20260419`) for:
- `PeerRegistry` resource — Profile Manager writes display names and avatar data into PeerRegistry entries (Phase 4)
- `NodeIdentity` resource — provides the local user's DID for profile identification (Phase 1+)
- Canonical `did:key` format — all peer identifiers use consistent format
- Peer presence system — online/offline status is provided by PeerRegistry, not duplicated here

Phases 1-3 of this track are **independent** of the shared infrastructure and can proceed immediately. Phase 4 (P2P sync + PeerProfileCache) requires shared infrastructure to be complete.

## Functional Requirements

### FR-1: Profile Display

**Description:** Show the current user's profile in a persistent, always-visible location in the UI (top of the left sidebar or a top toolbar section).

**Acceptance Criteria:**
- A profile section is visible at all times when the application is running.
- The profile section displays:
  - Display name (default: "Anonymous" or first 8 chars of the public key).
  - Identity key (truncated to first 8 and last 4 characters with ellipsis, e.g., `z6MkpTHq...xY4f`).
  - Current role for the active petal (if a petal is selected), e.g., "admin", "viewer".
- An online/offline status indicator (green dot = connected to at least one peer, gray dot = no peers).
- Clicking the profile section opens a profile panel/popover (see FR-2).

**Priority:** P0

### FR-2: Profile Editing

**Description:** Allow the user to edit their display name and select an avatar/icon.

**Acceptance Criteria:**

**Display Name:**
- A text input field for the display name.
- Maximum length: 32 characters.
- Allowed characters: alphanumeric, spaces, hyphens, underscores.
- Changes are applied immediately to the local profile.
- Changes are persisted to the local database.

**Avatar/Icon:**
- A selection grid of built-in icons/avatars (8-16 options).
- Selecting an icon updates the profile immediately.
- Custom image upload is out of scope for v1 -- built-in icons only.
- The selected icon is stored as an enum variant or index, not as image data.

**Save/Cancel:**
- An explicit "Save" button persists changes to the database.
- A "Cancel" button or clicking away discards unsaved edits.
- Visual indicator when there are unsaved changes.

**Priority:** P1

### FR-3: Identity Management

**Description:** Allow the user to manage their cryptographic identities: view the full key, copy it, export it for backup, import an existing identity, create new identities, and switch between them.

**Acceptance Criteria:**

**Identity Display:**
- Full public key displayed in a read-only field with a "Copy" button.
- Copy-to-clipboard puts the full `did:key:z6Mk...` string on the system clipboard.
- Visual confirmation (brief "Copied!" tooltip or color flash) after copying.

**Identity Export:**
- An "Export" button writes the identity (public key + encrypted private key) to a file.
- Export uses a password-protected format (e.g., encrypted JSON or similar).
- The user is prompted for a password before export.
- The file is saved via the OS file picker dialog.

**Identity Import:**
- An "Import" button opens the OS file picker to select an exported identity file.
- The user is prompted for the decryption password.
- On successful import, the identity is added to the local identity list.
- On failure (wrong password, corrupt file), an error message is displayed.

**Identity Switching:**
- A dropdown or list of available identities.
- Selecting a different identity switches the active keypair.
- Switching identity requires confirmation: "This will disconnect from all peers. Continue?"
- After switching, the application reconnects with the new identity.

**Identity Creation:**
- A "Create New Identity" button generates a new ed25519 keypair.
- The new keypair is stored in the OS keychain alongside existing identities.
- The user can optionally set a display name immediately.

**Priority:** P2

### FR-4: Integration

**Description:** Connect the profile system to persistence and the P2P layer so profiles are visible to other peers.

**Acceptance Criteria:**

**Local Persistence:**
- Profile data (display name, avatar selection, identity list) is stored in the local SurrealDB instance.
- Profile data loads on application startup.
- Profile data survives application restarts.

**P2P Sync:**
- When connecting to a peer, the local profile (display name, avatar, public key) is sent as part of the handshake or as a gossip message.
- Received peer profiles are cached locally.
- Profile updates are propagated to connected peers via iroh-gossip within 5 seconds.
- Stale peer profiles are marked stale after 7 days (shown dimmed in UI) and hard evicted after 30 days. Display names are persisted in SurrealDB `profiles` table to survive cache eviction. (Serve-stale pattern from DNS RFC 8767)
- Profile data is exchanged via gossip only (not handshake). Accept ~1 second delay for name resolution on new connections.
- Rate limiting: maximum 1 `ProfileAnnounce` per peer per 10 seconds.
- `ProfileAnnounce` messages are treated as untrusted input — validate display name length/charset, enforce monotonic timestamps per public key, verify signature before caching.

**Inspector Integration:**
- The Access tab (from Track: Inspector Settings) reads peer display names from `PeerRegistry`, which Profile Manager populates via `PeerProfileCache`. Profile Manager writes display names into PeerRegistry; the Access tab does NOT query PeerProfileCache directly.

**Priority:** P3

## Non-Functional Requirements

### NFR-1: Security

- Private key material is NEVER displayed in the UI. Only the public key is shown.
- Identity export encrypts the private key with the user's password using a strong KDF (e.g., Argon2).
- The OS keychain is the only runtime storage for private keys. No plaintext keys on disk.
- Identity switching clears all session state (JWT tokens, cached peer sessions).

### NFR-2: Performance

- Profile display updates must not cause per-frame allocations. Cache display strings.
- P2P profile sync messages must be small (< 512 bytes per profile).
- Identity switching and reconnection should complete within 5 seconds on LAN.

### NFR-3: Code Coverage

- >80% test coverage for profile state management, validation, and serialization.
- Identity management (create, export, import) must have unit tests including error paths (wrong password, corrupt file, duplicate identity).

### NFR-4: Privacy

- Profile data (display name, avatar) is user-controlled. No telemetry or external transmission beyond connected peers.
- The user can choose to remain "Anonymous" with no display name.

## User Stories

### US-1: New User Seeing Their Identity

**As** a first-time user, **I want** to see my identity information in the UI, **so that** I know which keypair I'm operating under and can share my identity with peers.

**Given** I launch FractalEngine for the first time
**When** the application finishes loading
**Then** I see "Anonymous" (or my truncated key) in the profile section at the top of the sidebar

### US-2: Operator Setting a Display Name

**As** a Node operator, **I want** to set a display name for my identity, **so that** other peers see a human-readable name instead of a cryptographic key.

**Given** I click on my profile section
**When** I type "Alice" in the display name field and click Save
**Then** my profile section shows "Alice" and connected peers see "Alice" in their peer lists

### US-3: Operator Backing Up Their Identity

**As** a Node operator, **I want** to export my identity to a file, **so that** I can restore it on another machine or after a system reinstall.

**Given** I open the identity management panel
**When** I click "Export", enter a password, and choose a save location
**Then** an encrypted identity file is written to disk that I can import later

### US-4: Operator Importing an Identity

**As** a Node operator, **I want** to import a previously exported identity, **so that** I can use the same identity across multiple devices.

**Given** I click "Import" and select an exported identity file
**When** I enter the correct password
**Then** the identity is added to my identity list and I can switch to it

### US-5: Peer Seeing Profile in Access Tab

**As** a peer visiting a petal, **I want** to see other peers' display names in the Access tab, **so that** I know who else is present and what roles they have.

**Given** I open the Access tab for a petal
**When** I see the peer list
**Then** each peer shows their display name (or truncated key) and role

## Technical Considerations

- **Keychain integration:** The `keyring` crate is already used for the primary keypair. Multi-identity support requires storing multiple keys, potentially with different service/username combinations in the keychain.
- **Profile DB schema:** A `profiles` table in SurrealDB: `{ id: did_key, display_name: option<string>, avatar: option<u8>, created_at: datetime }`.
- **P2P profile messages:** Define a `ProfileAnnounce` gossip message type. Include: public key, display name, avatar index, timestamp, signature.
- **Clipboard:** Use the `arboard` crate for cross-platform clipboard access (already common in egui apps).
- **File picker:** Use `rfd` (Rust File Dialog) for OS-native file picker dialogs.
- **Identity export format:** JSON with encrypted private key: `{ "version": 1, "public_key": "...", "encrypted_private_key": "...", "kdf": "argon2id", "salt": "...", "nonce": "..." }`. Encryption via `aes-gcm` + `argon2`.
- **Display name validation:** Regex or char-level filter. Reject control characters, limit to 32 chars.
- **PeerRegistry integration:** Profile Manager's `PeerProfileCache` feeds display name and avatar data into the `PeerRegistry` resource (from Shared Peer Infrastructure track). The cache and registry are separate: `PeerProfileCache` stores the full gossip-received profile, `PeerRegistry` stores only the display fields needed for UI composition. Self-asserted data (profiles via gossip) stays separate from other-asserted data (roles via RBAC DB).
- **Identity switching impact:** Switching identity requires full sync thread teardown, iroh endpoint rebuild, and session clearing. Roles are orphaned on the old key. Treat as "destructive reset" for v1 with a clear user warning dialog. This is documented as a HIGH priority finding in the cross-track alignment report.

## Out of Scope

- Custom avatar image upload (v1 uses built-in icons only).
- Avatar rendering in the 3D world (avatar is UI-only).
- Social features (friend lists, direct messaging).
- Multi-device identity sync (manual export/import only).
- Biometric authentication for identity switching.

## Open Questions

1. **Profile location in UI:** Cross-track alignment analysis recommends top toolbar (not sidebar) because the sidebar auto-collapses when the inspector opens. Top toolbar ensures profile is always visible.
2. **Identity storage:** Should multiple identities be stored in the OS keychain (one entry per identity) or in an encrypted local file alongside the DB? Keychain is more secure but some OS keychains have entry limits.
3. **Avatar set:** What built-in avatars should be included? Geometric shapes, nature icons (matching fractal/botanical theme), or abstract patterns?
4. **Profile visibility control:** Should users be able to set their profile as "private" (hidden from non-admin peers)? Current assumption: profiles are always visible to connected peers.
