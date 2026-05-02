# Specification: Replace Deprecated egui `screen_rect()` API

## Problem

The codebase uses `egui::Context::screen_rect()`, which was deprecated in recent egui versions. The deprecation message states:

> `screen_rect` has been split into `viewport_rect()` and `content_rect()`. You likely should use `content_rect()`.

This will break compilation when egui is next bumped.

## Affected Locations

| File | Line | Current Code |
|------|------|-------------|
| `fe-ui/src/panels.rs` | 738 | `ctx.screen_rect().width() * 0.8` |
| `fe-ui/src/panels.rs` | 853 | `ctx.screen_rect().width() * 0.8` |
| `fe-ui/src/plugin.rs` | 629 | `ectx.screen_rect()` |

## Context Analysis

- **`panels.rs:738` & `:853`**: Used to cap inspector panel width at 80% of screen width. These are `right_inspector` and likely a dialog panel. The intent is "available screen area" — `viewport_rect()` is the correct replacement since it gives the full available viewport.
- **`plugin.rs:629`**: Used in `setup_egui_fonts` or similar startup system to compute font scaling based on screen size. Likely wants `content_rect()` for DPI-aware sizing, but `viewport_rect()` is safer as a drop-in replacement.

## Acceptance Criteria

1. All three call sites compile without deprecation warnings.
2. No behavioural change in panel sizing or font scaling.
3. `cargo clippy -- -D warnings` passes for `fe-ui`.

## Scope

- `fe-ui/src/panels.rs`
- `fe-ui/src/plugin.rs`
- No other crates affected.
