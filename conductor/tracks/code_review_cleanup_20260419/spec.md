# Specification: Code Review Cleanup

## Overview

Address issues identified during code review, ranging from a critical SSRF vulnerability in the wry navigation handler to dead code removal, stale documentation, and minor quality-of-life improvements. This is a chore track with no new features -- only correctness, security, and maintainability fixes.

## Background

After the Gardener Console and Canopy View tracks were completed, a code review surfaced multiple issues across `fe-webview` and `fe-ui`. The most urgent is a Server-Side Request Forgery (SSRF) risk: the wry `navigation_handler` unconditionally returns `true`, allowing in-page links and redirects to reach private network addresses despite the `Navigate` command being gated by `security::is_url_allowed()`. Other issues include dead code left from the guard/flush refactor, stale documentation referencing removed types, missing system ordering, and several minor code quality items.

## Functional Requirements

### FR-1: Navigation Handler URL Validation (CRITICAL)

**Description:** The `navigation_handler` closure in `WryBackend::create()` (wry.rs) must validate every URL through `security::is_url_allowed()` before permitting navigation. Currently it returns `true` for all URLs, meaning an attacker-controlled page loaded in the portal can redirect to `http://192.168.x.x` or `http://localhost:xxxx` and the webview will follow.

**Acceptance Criteria:**
- The navigation handler calls `security::is_url_allowed()` on every navigation request.
- Returns `false` (blocks navigation) for any URL that fails the security check.
- Blocked navigations are logged via `tracing::warn!`.
- A `BackendEvent::Error` is pushed to the event queue when a navigation is blocked so the UI layer can display feedback.
- Existing tests for `is_url_allowed()` still pass; new tests cover the handler integration.

**Priority:** P0

### FR-2: Remove Dead Guard/Flush Code (HIGH)

**Description:** `tab_switch_guard_system`, `flush_browser_commands_system`, and the `PendingBrowserCommands` resource in `petal_portal.rs` are dead code. The comment at line 375-378 confirms they were removed from the plugin build, but the function bodies and resource definition remain. They should be deleted entirely.

**Acceptance Criteria:**
- `tab_switch_guard_system` function is deleted from `petal_portal.rs`.
- `flush_browser_commands_system` function is deleted from `petal_portal.rs`.
- `PendingBrowserCommands` struct and its `Default` impl are deleted.
- No remaining references to these items exist in the crate.
- All existing tests still compile and pass.

**Priority:** P1

### FR-3: Update Stale Documentation (HIGH)

**Description:** `AGENTS.md`, `docs/architecture.md`, and `docs/dataflows.md` reference types and resources that were removed during the UI Manager refactor (e.g., `InspectorState`, separate dialog state resources, `PortalPanelState`). These references must be updated to reflect the current architecture: `UiManager`, `ActiveDialog`, `PortalState`, `UiAction`, `UiSet`.

**Acceptance Criteria:**
- No references to `InspectorState` (the old resource, not `InspectorFormState`) remain in docs.
- No references to removed dialog state resources (individual dialog resources replaced by `ActiveDialog` enum).
- No references to `PortalPanelState` (replaced by `UiManager.portal: PortalState`).
- Current types (`UiManager`, `ActiveDialog`, `PortalState`, `UiAction`, `UiSet`) are documented where previously the old types appeared.
- Docs are factually accurate relative to the current codebase.

**Priority:** P1

### FR-4: Add VerseManager Systems to UiSet Ordering (HIGH)

**Description:** `VerseManager` systems that read or write shared UI state must be ordered relative to `UiSet::ProcessActions` so they do not race with action draining. They should run `.before(UiSet::ProcessActions)` or at an appropriate point in the set chain.

**Acceptance Criteria:**
- VerseManager systems that interact with `UiManager` or `InspectorFormState` are explicitly ordered via `.before(UiSet::ProcessActions)` or equivalent.
- No Bevy ambiguity warnings related to VerseManager systems appear at startup.
- Existing tests pass.

**Priority:** P1

### FR-5: Fix node_options_save_url Persistence (MEDIUM)

**Description:** The "Save" button in the Node Options dialog (`dialogs.rs`) updates the in-memory `VerseManager` node URL but does not send a `DbCommand::UpdateNodeUrl` to persist the change to SurrealDB. The URL is lost on restart.

**Acceptance Criteria:**
- Clicking Save in the Node Options dialog sends `DbCommand::UpdateNodeUrl` to the database channel.
- The URL persists across application restarts.
- If the database channel is closed, a warning is logged.

**Priority:** P2

### FR-6: Cache Portal URL Hostname (MEDIUM)

**Description:** The portal status bar parses the current URL string every frame to extract the hostname for display. This allocates a new `Url` struct and `String` per frame. Cache the hostname when the URL changes.

**Acceptance Criteria:**
- Hostname is computed once when the portal URL changes, not every frame.
- A cached hostname field (or equivalent) avoids per-frame allocation.
- The displayed hostname matches the current URL.

**Priority:** P2

### FR-7: Fix Context Menu Close Detection (MEDIUM)

**Description:** The context menu uses a hardcoded size for click-outside detection rather than measuring the actual rendered rect. This causes the menu to close prematurely or stay open when it shouldn't.

**Acceptance Criteria:**
- Context menu close detection uses the actual rendered `egui::Response` rect.
- Clicking inside the menu does not close it.
- Clicking outside the menu closes it.

**Priority:** P2

### FR-8: Remove or Document Sidebar Toggle Button (MEDIUM)

**Description:** The sidebar toggle button is a visual no-op because the sidebar auto-collapses based on portal/inspector state. Either remove the button or document its intended behavior and make it functional.

**Acceptance Criteria:**
- The toggle button is either removed from the UI or made functional (manual override of auto-collapse).
- If kept, the behavior is documented in a code comment.

**Priority:** P2

### FR-9: Remove Unused SidebarState.tag_filter_buf (MEDIUM)

**Description:** `SidebarState.tag_filter_buf` is declared but never read by any system. Remove it to reduce confusion.

**Acceptance Criteria:**
- `tag_filter_buf` field is removed from `SidebarState`.
- No compilation errors after removal.
- If any code references it, that code is also updated or removed.

**Priority:** P2

### FR-10: Replace eprintln! with bevy::log (LOW)

**Description:** `fe-webview/src/plugin.rs` and `wry.rs` use `eprintln!()` for debug output. These should use `bevy::log::info!`, `bevy::log::warn!`, or `bevy::log::error!` (via `tracing`) for consistent structured logging.

**Acceptance Criteria:**
- All `eprintln!` calls in `fe-webview/src/plugin.rs` and `fe-webview/src/backends/wry.rs` are replaced with appropriate `bevy::log` macros.
- Log levels are appropriate: errors for failures, warn for blocked navigations, info/debug for lifecycle events.

**Priority:** P3

### FR-11: Remove Redundant .clone() Calls (LOW)

**Description:** `node_manager.rs:457` and `dialogs.rs:378` contain `.clone()` calls on values that are already owned or could be moved.

**Acceptance Criteria:**
- Redundant `.clone()` calls are removed.
- Code compiles and tests pass after removal.

**Priority:** P3

## Non-Functional Requirements

### NFR-1: Security

- The navigation handler fix (FR-1) must block all URLs that `is_url_allowed()` rejects, including redirects initiated by JavaScript within the webview.
- No regression in existing security test coverage.

### NFR-2: Code Coverage

- New code must have >80% test coverage as measured by `cargo tarpaulin`.
- Navigation handler validation (FR-1) must have dedicated tests covering: allowed URL passes, blocked private IP is rejected, blocked localhost is rejected, non-http scheme is rejected.

### NFR-3: No Behavioral Regressions

- All existing `cargo test` tests must continue to pass after every change.
- No new `cargo clippy` warnings introduced.

## User Stories

### US-1: Security-Conscious Operator

**As** a Node operator, **I want** the embedded browser to block navigation to private network addresses even if a loaded external page redirects there, **so that** my internal network is not exposed to SSRF attacks through the portal overlay.

**Given** the portal is open showing `https://malicious.example.com`
**When** that page issues a JavaScript redirect to `http://192.168.1.1/admin`
**Then** the navigation is blocked and the webview stays on the current page

### US-2: Developer Maintaining the Codebase

**As** a developer reading the codebase, **I want** dead code and stale documentation to be removed/updated, **so that** I can trust the docs and don't waste time understanding code paths that are never executed.

**Given** I open `petal_portal.rs`
**When** I search for `tab_switch_guard_system`
**Then** the function does not exist (it has been deleted)

### US-3: Operator Saving Node URLs

**As** a Node operator, **I want** URLs I save in the Node Options dialog to persist across restarts, **so that** I don't have to re-enter them every time I launch the application.

**Given** I set a portal URL in the Node Options dialog and click Save
**When** I restart the application and select the same node
**Then** the previously saved URL is displayed in the inspector

## Technical Considerations

- FR-1 requires access to `security::is_url_allowed()` from the navigation handler closure. Since the closure is `move`, the function must be callable without capturing mutable state. `is_url_allowed()` is a pure function taking `&Url`, so this is straightforward.
- FR-1 also requires pushing `BackendEvent::Error` from within the closure. The `events: Rc<RefCell<Vec<BackendEvent>>>` is already cloned into the closure, so a third clone for the security-block path is needed.
- FR-2 is a pure deletion -- no new code, just removal.
- FR-4 requires understanding which VerseManager systems touch `UiManager`. Check `verse_manager.rs` for system parameter lists.
- FR-10 requires care: some `eprintln!` calls happen before the Bevy logger is initialized (during `init_backend`). Those may need to remain as `eprintln!` or use `tracing::info!` directly (which works before Bevy's LogPlugin if a subscriber is set).

## Out of Scope

- Adding new features or UI elements.
- Refactoring the WebView backend trait or architecture.
- Changing the portal overlay positioning system.
- Any changes to the P2P or database layers.

## Open Questions

1. **FR-8 (sidebar toggle):** Should the button be removed entirely, or should it become a manual override that temporarily forces the sidebar open/closed? Decision needed from product owner.
2. **FR-10 (eprintln in init_backend):** The `init_backend` system runs before Bevy's `LogPlugin` may have fully initialized. Should we keep `eprintln!` for the earliest init messages, or rely on `tracing` being available?
