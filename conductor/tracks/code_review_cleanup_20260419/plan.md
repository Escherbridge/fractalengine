# Implementation Plan: Code Review Cleanup

## Overview

This track addresses issues found during code review, organized by severity. There are four phases: Phase 1 handles the critical SSRF fix, Phase 2 covers high-priority dead code and doc cleanup, Phase 3 addresses medium-priority fixes, and Phase 4 handles low-priority polish. Each phase ends with a verification checkpoint.

---

## Phase 1: Critical Security Fix

Goal: Close the SSRF vulnerability in the wry navigation handler.

Tasks:

- [ ] Task 1.1: Write navigation handler security tests (TDD Red)
  - Create tests in `fe-webview/src/backends/wry.rs` (or a companion test module) that assert:
    - A navigation to `https://example.com` is allowed (returns `true`).
    - A navigation to `http://192.168.1.1` is blocked (returns `false`).
    - A navigation to `http://localhost:8080` is blocked.
    - A navigation to `ftp://example.com` is blocked (non-http scheme).
    - A `BackendEvent::Error` is emitted when navigation is blocked.
  - Since the handler is a closure inside `WryBackend::create()`, extract the validation logic into a testable helper function: `fn should_allow_navigation(url: &str) -> bool`.
  - Confirm tests fail (handler currently allows everything).

- [ ] Task 1.2: Implement navigation handler validation (TDD Green)
  - Extract a `fn should_allow_navigation(raw_url: &str) -> bool` function in `wry.rs` (or in `security.rs` as a convenience wrapper) that parses the URL and calls `security::is_url_allowed()`.
  - Modify the `navigation_handler` closure in `WryBackend::create()` to:
    1. Call `should_allow_navigation(&url)`.
    2. If blocked: push `BackendEvent::Error` to `nav_events`, log via `tracing::warn!`, return `false`.
    3. If allowed: push `BackendEvent::UrlChanged` as before, return `true`.
  - Clone the `events` Rc into the closure for error reporting.
  - Run tests, confirm green.

- [ ] Task 1.3: Refactor and verify (TDD Refactor)
  - Review the closure for clarity. Consider whether the helper should live in `security.rs` alongside `is_url_allowed`.
  - Ensure `cargo clippy -- -D warnings` passes.
  - Ensure `cargo fmt --check` passes.

- [ ] Verification: Phase 1 Manual Verification [checkpoint marker]
  - Run `cargo test -p fe-webview`.
  - Launch the application, open a portal to a known safe URL (e.g., `https://example.com`).
  - Attempt to navigate to a blocked URL by entering `http://localhost` in the portal -- confirm the page does not load.
  - Check logs for the security warning message.

---

## Phase 2: High-Priority Cleanup

Goal: Remove dead code, update stale docs, fix system ordering.

Tasks:

- [ ] Task 2.1: Delete dead guard/flush code from petal_portal.rs (TDD: verify no references, then delete)
  - Search the entire crate for references to `tab_switch_guard_system`, `flush_browser_commands_system`, and `PendingBrowserCommands`.
  - Confirm no code calls or registers these items (the comment at line 375 confirms removal from plugin build).
  - Delete the `PendingBrowserCommands` struct, its `Default` impl, `tab_switch_guard_system` function, and `flush_browser_commands_system` function.
  - Run `cargo check -p fe-webview` to confirm compilation.
  - Run `cargo test -p fe-webview` to confirm all tests pass.

- [ ] Task 2.2: Audit and update AGENTS.md
  - Read `AGENTS.md` and search for references to removed types: `InspectorState`, `PortalPanelState`, individual dialog state resources.
  - Replace with current types: `UiManager`, `ActiveDialog`, `PortalState`, `UiAction`, `UiSet`, `InspectorFormState`.
  - Ensure the document accurately describes the current architecture.

- [ ] Task 2.3: Audit and update docs/architecture.md
  - Read `docs/architecture.md` and search for stale type references.
  - Update all references to match current codebase state.
  - Add `UiSet` ordering description if system scheduling is documented.

- [ ] Task 2.4: Audit and update docs/dataflows.md
  - Read `docs/dataflows.md` and search for stale type references.
  - Update data flow diagrams/descriptions to reflect the `UiAction` queue pattern.
  - Replace any `PortalPanelState` flow with `UiManager.portal: PortalState` flow.

- [ ] Task 2.5: Add VerseManager systems to UiSet ordering
  - Read `fe-ui/src/verse_manager.rs` and identify systems that access `UiManager` or `InspectorFormState`.
  - Add `.before(UiSet::ProcessActions)` (or appropriate ordering) to those systems in the plugin `build()`.
  - Write a test or compile-time check that the ordering is respected (e.g., a system-ordering smoke test).
  - Run `cargo test -p fe-ui` to confirm no regressions.

- [ ] Verification: Phase 2 Manual Verification [checkpoint marker]
  - Run `cargo test` (all crates).
  - Grep the entire repo for `InspectorState` (excluding `InspectorFormState`), `PortalPanelState`, `tab_switch_guard_system`, `flush_browser_commands_system`, `PendingBrowserCommands` -- confirm zero hits in code and docs.
  - Run the application and verify no Bevy system-ordering ambiguity warnings in the console.

---

## Phase 3: Medium-Priority Fixes

Goal: Fix persistence bugs, optimize per-frame allocations, improve UI interactions.

Tasks:

- [ ] Task 3.1: Fix node_options_save_url DB persistence (TDD: Write test, implement, refactor)
  - Write a test that verifies `DbCommand::UpdateNodeUrl` is sent when the Node Options Save button triggers the save path in `dialogs.rs`.
  - Implement: add `db_sender.0.send(DbCommand::UpdateNodeUrl { ... })` to the save handler in `dialogs.rs`, matching the pattern already used in `process_ui_actions` for `UiAction::SaveUrl`.
  - Add warning log if the channel is closed.
  - Run tests.

- [ ] Task 3.2: Cache portal URL hostname (TDD: Write test, implement, refactor)
  - Write a test: when `PortalState` transitions to `Open { current_url }`, a cached hostname is derived and accessible.
  - Implement: add a `cached_hostname: String` field to `PortalState::Open` (or a separate small cache in `UiManager`). Update it whenever `current_url` changes.
  - Update the portal status bar rendering to read the cached hostname instead of parsing every frame.
  - Run tests.

- [ ] Task 3.3: Fix context menu close detection (TDD: Write test, implement, refactor)
  - Identify the hardcoded size in the context menu close-detection code in `dialogs.rs` or the panel rendering.
  - Write a test: clicking at coordinates inside the rendered menu rect does not trigger close; clicking outside does.
  - Implement: capture the `egui::Response` or `Rect` from the context menu rendering and use it for the click-outside check.
  - Run tests.

- [ ] Task 3.4: Resolve sidebar toggle button (implement decision)
  - If decision is "remove": delete the toggle button code and any associated state.
  - If decision is "make functional": implement manual override of auto-collapse, add a code comment explaining the behavior.
  - Default action (if no product owner decision): remove the button and add a `// TODO: re-add manual sidebar toggle when needed` comment.
  - Run tests.

- [ ] Task 3.5: Remove unused SidebarState.tag_filter_buf
  - Grep for `tag_filter_buf` across the codebase.
  - If no reads exist (only the field declaration and default init), remove the field.
  - Update `Default` impl accordingly.
  - Run `cargo check -p fe-ui` and `cargo test -p fe-ui`.

- [ ] Verification: Phase 3 Manual Verification [checkpoint marker]
  - Run `cargo test` (all crates).
  - Launch the application:
    - Set a URL in Node Options, click Save, restart, confirm URL persists.
    - Open a portal and verify the hostname in the status bar is correct.
    - Right-click to open context menu, click inside -- confirm it stays open. Click outside -- confirm it closes.
    - Verify sidebar behavior is clean (no orphan toggle button or it works correctly).

---

## Phase 4: Low-Priority Polish

Goal: Logging consistency and minor code quality.

Tasks:

- [ ] Task 4.1: Replace eprintln! with bevy::log in fe-webview (TDD: Write test, implement, refactor)
  - Audit all `eprintln!` calls in `fe-webview/src/plugin.rs` and `fe-webview/src/backends/wry.rs`.
  - Replace with appropriate `bevy::log` or `tracing` macros:
    - Errors: `bevy::log::error!`
    - Warnings/blocks: `bevy::log::warn!`
    - Lifecycle events: `bevy::log::info!` or `bevy::log::debug!`
  - For `eprintln!` calls in `init_backend` that fire before LogPlugin: use `tracing::info!` (available if any subscriber is installed) or keep as `eprintln!` with a comment explaining why.
  - Run `cargo clippy -- -D warnings` and `cargo test -p fe-webview`.

- [ ] Task 4.2: Remove redundant .clone() calls
  - Check `node_manager.rs:457` -- determine if the cloned value is used after the clone. If not, remove `.clone()` and move instead.
  - Check `dialogs.rs:378` -- same analysis.
  - Run `cargo check -p fe-ui` and `cargo test -p fe-ui`.

- [ ] Verification: Phase 4 Final Verification [checkpoint marker]
  - Run `cargo test` (all crates).
  - Run `cargo clippy -- -D warnings` (all crates).
  - Run `cargo fmt --check`.
  - Grep for `eprintln!` in `fe-webview/src/` -- confirm zero hits (or only documented exceptions in early init).
  - Confirm no new warnings in build output.
