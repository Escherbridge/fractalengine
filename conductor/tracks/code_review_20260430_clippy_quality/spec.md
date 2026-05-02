# Specification: Clippy Warnings, Code Quality, and Polish

## Problem

A collection of medium- and low-severity code quality issues found across `fe-ui` and related crates. These don't affect correctness but hurt readability, maintainability, and compile times.

## Issues Covered

### Medium Severity

| # | Issue | Location | Description |
|---|-------|----------|-------------|
| 6 | Unnecessary mutable query | `node_manager.rs` | `compute_gimbal_center_inline` takes `&Query<(&mut Transform, &GlobalTransform)>` but only reads `GlobalTransform` |
| 7 | Small epsilon guard | `node_manager.rs` | `segment_dist_2d` uses `1e-6` epsilon — can cause numerical instability for near-zero segments |
| 8 | `format!()` allocations on selection | `node_manager.rs` | `sync_manager_to_inspector` allocates 9 strings via `format!()` on every selection change |
| 10 | Brittle asset path concat | `verse_manager.rs` | `spawn_node_entity` unconditionally appends `#Scene0` to asset paths |
| 11 | Large enum variant | `plugin.rs` | `ActiveDialog::EntitySettings` is ~500+ bytes; variant size difference triggers clippy warning |
| 12 | 14-parameter function | `panels.rs` | `gardener_console` has 14 params, approaching Bevy's 16-param system limit |

### Low Severity

| # | Issue | Location | Description |
|---|-------|----------|-------------|
| 14 | Dead field | `plugin.rs:585` | `peer_registry` in `P2pDialogParams` is never read |
| 15 | Clippy warnings | `fe-webview`, `fe-database` | Redundant closures, unused imports, `clone()` on `Copy` types |
| 16 | Inconsistent logging | Multiple | Some modules use `tracing::`, others `bevy::log::`, others `.ok()` |
| 17 | Missing `Debug` | `node_manager.rs` | `NodeSelection` and `AxisDrag` lack `#[derive(Debug)]` |

## Acceptance Criteria

1. All clippy warnings in `fe-ui`, `fe-webview`, `fe-database` resolved or explicitly allowed.
2. `cargo clippy -- -D warnings` passes for all three crates.
3. No dead code warnings.
4. Consistent logging approach documented (at minimum, no mixing within a single module).

## Scope

- `fe-ui/src/node_manager.rs`
- `fe-ui/src/verse_manager.rs`
- `fe-ui/src/panels.rs`
- `fe-ui/src/plugin.rs`
- `fe-webview/src/`
- `fe-database/src/`
