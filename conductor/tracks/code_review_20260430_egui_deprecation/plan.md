# Implementation Plan: Replace Deprecated egui `screen_rect()` API

## Overview

A quick refactor track to eliminate deprecated egui API usage before the next dependency bump. Single phase, no TDD required (no logic change — purely mechanical replacement).

---

## Phase 1: Replace All Call Sites

Goal: Eliminate all `screen_rect()` usage in `fe-ui`.

Tasks:

- [ ] Task 1.1: Replace `panels.rs:738`
  - Change `ctx.screen_rect().width() * 0.8` → `ctx.viewport_rect().width() * 0.8`.
  - Confirm same semantics (full screen width cap for inspector).
  - Run `cargo check -p fe-ui`.

- [ ] Task 1.2: Replace `panels.rs:853`
  - Change `ctx.screen_rect().width() * 0.8` → `ctx.viewport_rect().width() * 0.8`.
  - Run `cargo check -p fe-ui`.

- [ ] Task 1.3: Replace `plugin.rs:629`
  - Change `ectx.screen_rect()` → `ectx.viewport_rect()` (review context; if used for DPI/font scaling, `content_rect()` may be more appropriate — add a code comment explaining the choice).
  - Run `cargo check -p fe-ui`.

- [ ] Task 1.4: Verify clean build
  - Run `cargo clippy -- -D warnings -p fe-ui`.
  - Confirm zero deprecation warnings related to `screen_rect`.

- [ ] Verification: Phase 1 Manual Verification [checkpoint marker]
  - Run `cargo test -p fe-ui` — all tests pass.
  - Launch the application, resize the window, confirm the inspector panel still respects the 80% width cap.
  - Check build output for zero `screen_rect` deprecation warnings.

---

## Completion Criteria

- [ ] All three call sites use `viewport_rect()` or `content_rect()`.
- [ ] `cargo clippy -- -D warnings -p fe-ui` passes.
- [ ] No behavioural regressions in panel sizing.
