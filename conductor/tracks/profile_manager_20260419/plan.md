# Implementation Plan: User Profile Manager

## Overview

Four phases, progressing from read-only display to full identity management and P2P integration. Phase 1 adds a profile display widget. Phase 2 adds profile editing. Phase 3 implements identity management (create, switch, import, export). Phase 4 integrates profiles with persistence and the P2P layer.

---

## Phase 1: Profile Display

Goal: Show the current user's identity in a persistent UI element.

Tasks:

- [ ] Task 1.1: Define UserProfile data structures (TDD Red)
  - Write tests for:
    - `UserProfile { display_name: Option<String>, public_key: String, avatar_index: u8 }` struct.
    - `UserProfile::display_name_or_default()` returns the name or "Anonymous".
    - `UserProfile::truncated_key()` returns first 8 + "..." + last 4 characters.
    - `ProfileState` resource holding the active `UserProfile` and connection status.
  - Confirm tests fail (types don't exist yet).

- [ ] Task 1.2: Implement UserProfile and ProfileState (TDD Green)
  - Create `fe-ui/src/profile.rs` module.
  - Implement `UserProfile` struct with `display_name_or_default()` and `truncated_key()` methods.
  - Implement `ProfileState` resource: `{ active_profile: UserProfile, is_connected: bool }`.
  - Register `ProfileState` as a resource in `GardenerConsolePlugin::build()`.
  - Run tests, confirm green.

- [ ] Task 1.3: Populate ProfileState from identity system (TDD Red -> Green)
  - Write a test: on startup, `ProfileState.active_profile.public_key` is populated from the identity system (`fe-identity` crate or the keyring).
  - Implement a startup system that reads the active keypair's public key and creates the initial `UserProfile`.
  - If no identity exists yet (first launch), use a placeholder key and "Anonymous" name.
  - Run tests.

- [ ] Task 1.4: Render profile widget in sidebar (TDD: implement + visual test)
  - Add a `render_profile_section()` function that draws the profile at the top of the left sidebar panel.
  - Display: avatar icon (placeholder colored circle for now), display name, truncated key, role chip, online/offline dot.
  - Wire the online/offline indicator to `ProfileState.is_connected`.
  - Wire the role to `InspectorFormState.is_admin` (or the active petal's role).

- [ ] Task 1.5: Cache display strings to avoid per-frame allocation
  - Compute `display_name_or_default()` and `truncated_key()` once on profile change, store results in `ProfileState`.
  - Ensure the rendering function reads cached strings, not computing them each frame.
  - Run tests.

- [ ] Verification: Phase 1 Manual Verification [checkpoint marker]
  - Run `cargo test`.
  - Launch the application.
  - Verify the profile section appears at the top of the sidebar.
  - Verify it shows "Anonymous" (or truncated key) and an offline indicator.
  - If connected to a peer, verify the indicator turns green.

---

## Phase 2: Profile Editing

Goal: Allow users to set a display name and select a built-in avatar.

Tasks:

- [ ] Task 2.1: Define display name validation (TDD Red)
  - Write tests:
    - Valid names: "Alice", "Bob-123", "my_name" pass validation.
    - Invalid names: empty string, >32 chars, control characters fail.
    - Edge cases: exactly 32 chars passes, 33 chars fails.
  - Confirm tests fail.

- [ ] Task 2.2: Implement display name validation (TDD Green)
  - Implement `fn validate_display_name(name: &str) -> Result<(), ValidationError>`.
  - Allowed characters: alphanumeric, spaces, hyphens, underscores.
  - Max length: 32.
  - Run tests, confirm green.

- [ ] Task 2.3: Define avatar selection (TDD Red -> Green)
  - Write tests: `AvatarIcon` enum or `avatar_index: u8` with 8-16 valid values.
  - Implement the type and a `fn is_valid_avatar(index: u8) -> bool` helper.
  - Run tests.

- [ ] Task 2.4: Build profile edit panel (TDD: implement + visual test)
  - Create `render_profile_edit_panel()` function.
  - Display: text input for display name (pre-filled with current), avatar grid (selectable icons), Save button, Cancel button.
  - Save button validates the display name, updates `ProfileState`, and queues a `UiAction::SaveProfile` (new variant).
  - Cancel button closes the panel without changes.
  - Show validation errors inline (red text under the name field).

- [ ] Task 2.5: Add UiAction::SaveProfile processing (TDD Red -> Green)
  - Write tests: `UiAction::SaveProfile` updates `ProfileState` and sends `DbCommand::UpdateProfile` to the database channel.
  - Add `SaveProfile` variant to `UiAction` enum.
  - Process it in `process_ui_actions`.
  - Run tests.

- [ ] Task 2.6: Add unsaved changes indicator
  - Track whether the edit form has been modified from the saved state.
  - Show a small dot or asterisk next to "Save" when there are unsaved changes.
  - Warn before closing the panel with unsaved changes.
  - Run tests.

- [ ] Verification: Phase 2 Manual Verification [checkpoint marker]
  - Run `cargo test`.
  - Launch the application, click the profile section.
  - Type a display name, select an avatar, click Save.
  - Verify the sidebar profile section updates immediately.
  - Close and reopen the edit panel -- verify saved values are shown.
  - Try an invalid name (>32 chars) -- verify validation error appears.
  - Click Cancel with unsaved changes -- verify changes are discarded.

---

## Phase 3: Identity Management

Goal: Support creating, switching, exporting, and importing cryptographic identities.

Tasks:

- [ ] Task 3.1: Define IdentityEntry and IdentityList (TDD Red)
  - Write tests for:
    - `IdentityEntry { public_key: String, display_name: Option<String>, created_at: DateTime, is_active: bool }`.
    - `IdentityList { entries: Vec<IdentityEntry> }` with methods: `active()`, `add()`, `set_active()`.
    - Setting active clears the previous active flag.
  - Confirm tests fail.

- [ ] Task 3.2: Implement IdentityEntry and IdentityList (TDD Green)
  - Implement the structs and methods.
  - Populate `IdentityList` from the keychain on startup (initially contains one entry -- the existing keypair).
  - Run tests, confirm green.

- [ ] Task 3.3: Implement copy-to-clipboard (TDD Red -> Green)
  - Add `arboard` crate dependency.
  - Write a test: calling `copy_to_clipboard(key)` does not panic.
  - Implement the copy function with visual feedback state (a `Copied` flag that auto-clears after 2 seconds).
  - Render the full public key with a "Copy" button in the identity management UI.
  - Run tests.

- [ ] Task 3.4: Implement identity creation (TDD Red -> Green)
  - Write tests: `create_identity()` generates a valid ed25519 keypair and adds it to `IdentityList`.
  - Implement: generate keypair via `ed25519-dalek`, store in OS keychain via `keyring`, add to `IdentityList`.
  - Run tests.

- [ ] Task 3.5: Implement identity switching (TDD Red -> Green)
  - Write tests:
    - Switching identity changes the active entry in `IdentityList`.
    - Switching clears session state (mock JWT cache clear).
  - Implement: switch active identity, clear JWT session cache, trigger P2P reconnect (send event/command).
  - Add confirmation dialog: "This will disconnect from all peers. Continue?"
  - Show a destructive-action warning: "This will disconnect from all peers, clear session state, and orphan any roles assigned to the current identity. This cannot be undone."
  - Switching triggers full sync thread teardown and iroh endpoint rebuild.
  - Run tests.

- [ ] Task 3.6: Implement identity export (TDD Red -> Green)
  - Add `aes-gcm`, `argon2`, and `rfd` crate dependencies (if not already present).
  - Write tests:
    - Export produces valid JSON with the expected schema.
    - Encrypted private key can be decrypted with the correct password.
    - Wrong password fails decryption.
  - Implement export:
    1. Prompt for password via UI dialog.
    2. Derive key via Argon2id from password + random salt.
    3. Encrypt private key bytes via AES-256-GCM.
    4. Write JSON `{ version, public_key, encrypted_private_key, kdf, salt, nonce }` to file via `rfd` save dialog.
  - Run tests.

- [ ] Task 3.7: Implement identity import (TDD Red -> Green)
  - Write tests:
    - Import with correct password adds the identity to `IdentityList`.
    - Import with wrong password shows error, does not add identity.
    - Import of corrupt file shows error.
    - Import of duplicate identity (already in list) shows warning.
  - Implement import:
    1. Open file via `rfd` file picker.
    2. Parse JSON, prompt for password.
    3. Decrypt private key, verify it matches the public key.
    4. Store in keychain, add to `IdentityList`.
  - Run tests.

- [ ] Task 3.8: Build identity management UI panel
  - Create `render_identity_management_panel()` function.
  - Display:
    - Current identity with full key and Copy button.
    - List of available identities (selectable).
    - "Create New" button.
    - "Export" and "Import" buttons.
    - "Switch" button (grayed out if already active).
  - Wire all buttons to their respective handlers.

- [ ] Verification: Phase 3 Manual Verification [checkpoint marker]
  - Run `cargo test`.
  - Launch the application, open identity management.
  - Copy the public key -- verify it's on the clipboard.
  - Create a new identity -- verify it appears in the list.
  - Export the identity -- verify a file is created.
  - Import the exported identity on a fresh instance -- verify it works.
  - Switch identities -- verify the application reconnects.

---

## Phase 4: Integration (Persistence + P2P Sync)

Goal: Persist profiles to SurrealDB and broadcast them to connected peers.

Tasks:

- [ ] Task 4.1: Define profile DB schema (TDD Red -> Green)
  - Write tests for profile serialization/deserialization to/from SurrealDB format.
  - Define the `profiles` table schema: `{ id: did_key, display_name: option<string>, avatar_index: u8, created_at: datetime }`.
  - Implement `DbCommand::SaveProfile` and `DbCommand::LoadProfile` in `fe-runtime`.
  - Run tests.

- [ ] Task 4.2: Implement profile persistence (TDD Red -> Green)
  - Write tests: saving a profile and loading it back produces identical data.
  - Implement the database handler for `SaveProfile` and `LoadProfile`.
  - Load profile on startup (populate `ProfileState` from DB).
  - Save profile on `UiAction::SaveProfile` processing.
  - Run tests.

- [ ] Task 4.3: Define ProfileAnnounce gossip message (TDD Red -> Green)
  - Write tests for:
    - `ProfileAnnounce { public_key, display_name, avatar_index, timestamp, signature }` serialization.
    - Signature verification: valid signature accepted, tampered message rejected.
    - Message size < 512 bytes for typical profiles.
    - Monotonic timestamp enforcement: reject ProfileAnnounce with timestamp <= last seen timestamp for that public key.
    - Input validation: display name length <= 32, allowed charset only, no control characters.
    - Rate limiting: reject if less than 10 seconds since last ProfileAnnounce from same peer.
  - Implement the message type and signing/verification.
  - Run tests.

- [ ] Task 4.4: Implement profile broadcast on change (TDD Red -> Green)
  - Write tests: when `ProfileState` changes, a `ProfileAnnounce` is broadcast via iroh-gossip.
  - Implement: after `UiAction::SaveProfile` is processed, send `ProfileAnnounce` via the P2P layer.
  - Run tests.

- [ ] Task 4.5: Implement profile reception and caching (TDD Red -> Green)
  - Write tests:
    - Receiving a valid `ProfileAnnounce` updates the local `PeerProfileCache` and writes display name into `PeerRegistry`.
    - Receiving a message with invalid signature is rejected.
    - Profiles older than 7 days are marked stale (shown dimmed in UI) but NOT evicted.
    - Profiles older than 30 days are hard evicted from cache.
    - Display names are persisted in SurrealDB `profiles` table (survive cache eviction).
    - Stale profile fallback: if cache is evicted but DB has a persisted name, use the DB name with a stale indicator.
  - Implement a system that processes incoming `ProfileAnnounce` messages and updates a `PeerProfileCache` resource.
  - Run tests.

- [ ] Task 4.6: Wire PeerProfileCache into PeerRegistry
  - When PeerProfileCache receives or updates a profile, write the display name and avatar into the corresponding PeerRegistry entry.
  - The Access tab (Inspector Settings track) reads from PeerRegistry — it does NOT query PeerProfileCache directly.
  - Fall back to truncated key if no profile exists in PeerRegistry for a peer.
  - Run tests.

- [ ] Task 4.7: Refactor and polish
  - Ensure profile loading on startup is non-blocking (uses async DB query via channel).
  - Add error handling for keychain failures (e.g., locked keychain on macOS).
  - Run `cargo clippy` and `cargo fmt --check`.

- [ ] Task 4.8: Persist display names to SurrealDB for eviction resilience
  - Write tests: when a ProfileAnnounce is received, the display name is also written to the `profiles` table in SurrealDB.
  - Write tests: on cache miss, the system checks the `profiles` table before falling back to truncated key.
  - Implement the persistence and fallback logic.
  - Run tests.

- [ ] Verification: Phase 4 Final Verification [checkpoint marker]
  - Run `cargo test`.
  - Launch the application, set a display name, restart -- verify it persists.
  - Connect two nodes on LAN. Set a display name on Node A.
  - On Node B, open the Access tab -- verify Node A's display name appears (not just a raw key).
  - Disconnect Node A, wait, reconnect -- verify the profile is still cached on Node B.
  - Verify no private key material is ever logged or displayed in the UI.
  - Verify stale profile handling: disconnect a peer, wait, check profile is shown dimmed but not removed.
  - Verify PeerRegistry contains display names written by PeerProfileCache.
  - Verify rate limiting: rapid profile updates from a peer are throttled.
