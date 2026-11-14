---
type: Evidence
title: "Crate Consolidation R2: consumer map for FR-1, FR-2, FR-3"
tags: [crate_consolidation_r2_20260718]
resource: ../plan.md
---

# Consumer map (Phase 1 evidence)

Method: scoped greps of `Cargo.toml` files only (workspace root, `fe-*/`, `fractalengine/`,
`tools/fe-ext/`, `extensions/iot-bridge/`, `docker/`). No cargo, no builds. Use-site counts
are carried from the scoper's 2026-11-13 pass and were not re-grepped in this round.

## FR-3: fe-query (G-3)

Dependency edges, re-grepped at edit time:

| Consumer | Line | Features |
|----------|------|----------|
| `fe-api` | `fe-api/Cargo.toml:18` | `datafusion` |
| `fe-database` | `fe-database/Cargo.toml:15` | `parquet` |

`fe-query` itself depends only on `fe-sdk`, `fe-format` (default-features = false) and optional
`datafusion`, `arrow`, `parquet`, `fe-entity-store` (`fe-query/Cargo.toml` `[dependencies]`).
It does not depend on `fe-api` or `fe-database`, so no cycle exists today.

`fe-database` use sites (re-grepped 2026-11-14 with `git grep -n 'fe_query' -- fe-database/src`, excluding the
`fe-database/src/AGENTS.md` prose): **19 code occurrences in 16 files**.

- `fe-database/src/api_token_store.rs:6`
- `fe-database/src/canon_log/observation/hydrator.rs:25`, `:1018` (test module)
- `fe-database/src/canon_log/observation/mod.rs:9`
- `fe-database/src/canon_log/observation/sealer.rs:39`
- `fe-database/src/handlers/device_payload.rs:5`
- `fe-database/src/handlers/elevated_query.rs:28`
- `fe-database/src/handlers/field_def.rs:10`
- `fe-database/src/handlers/invite.rs:8`
- `fe-database/src/handlers/iot_reading.rs:3`
- `fe-database/src/handlers/node_log.rs:1`
- `fe-database/src/handlers/scoped_query.rs:6`
- `fe-database/src/handlers/seed.rs:3`
- `fe-database/src/handlers/ts_partial.rs:24`, `:27`
- `fe-database/src/query_helpers.rs:1`, `:5`
- `fe-database/src/rbac.rs:6`
- `fe-database/src/role_manager.rs:1`

The scoper carried 7 of these sites; the re-grep found 19.

Reading: moving `fe-query` under `fe-api` makes `fe-database` depend on `fe-api`, which is a
layering inversion (the storage crate would import the HTTP crate). Option (a) needs
`fe-database`'s query-builder and columnar pieces moved to a lower crate first. Option (b)
leaves two homes for the query code. Neither is mechanical. Outcome: DEFER (see decision record); the 19-site re-grep strengthens it, and the merge would invert layering rather than fold a sparse crate.

## FR-1: fe-plugin-test (G-1)

Dependency edges, re-grepped at edit time:

| Consumer | Line | Kind |
|----------|------|------|
| `fe-plugin` | `fe-plugin/Cargo.toml:63` | dev-dependency |
| `tools/fe-ext` | `tools/fe-ext/Cargo.toml:23` | dependency |
| `extensions/iot-bridge` | `extensions/iot-bridge/Cargo.toml:19` | dependency (used by `tests/bridge_loop.rs`) |

`fe-plugin` uses it at `fe-plugin/src/rhai/sandbox.rs:96`, `fe-plugin/src/wasm/mod.rs:363` and
`:456` (carried from the scoper). No workspace crate depends on it outside those three.
Re-verified 2026-11-14: exactly three consumers, all with the edges in the table above (`fe-plugin/Cargo.toml:63`,
`tools/fe-ext/Cargo.toml:23`, `extensions/iot-bridge/Cargo.toml:19`).

Shape: `fe-plugin-test/Cargo.toml` has its own published-crate shape. Direct dependencies:
`fe-sdk` (path, 0.1.0), `rhai` 1.19, `serde` (derive), `serde_json`, `ulid` (workspace). Per
oss_release plan 4.2 it is published alongside `fe-sdk`.

Reading: folding it into `fe-plugin` (which carries `wasmtime`, Win32/unix libc targets and the
Bevy-free but large Wasm closure) would make every OSS plugin author pull the wasm stack to get the
test harness. Outcome: DEFER (see decision record).

## FR-2: fe-hexon-registry (G-2)

Dependency edges, re-grepped at edit time:

| Consumer | Line | Kind |
|----------|------|------|
| `fe-hexon` | `fe-hexon/Cargo.toml:45` | dev-dependency |
| `fractalengine` | `fractalengine/Cargo.toml:32` | dev-dependency (in-process registry for flow tests, see comment at lines 28-31) |
| docker image | `docker/Dockerfile.hexon-registry:6`, `:11`, `:16` | `cargo build --release -p fe-hexon-registry`, runs the bin |
| compose | `docker/compose.dev.yml:17-20` | carried from scoper; not re-grepped |

`fe-hexon-registry` direct dependencies (`fe-hexon-registry/Cargo.toml` `[dependencies]`):
`axum`, `tokio` (fs), `tokio-util` (io), `tower-http`, `serde`, `serde_json`, `anyhow`, `tracing`,
`tracing-subscriber`, `zip` (deflate only), `semver` 1, `async-trait`, and `fe-format`
(`default-features = false`). None of these is a Bevy or wasmtime edge directly.

Reading: the image builds only the registry bin, so it avoids the engine closure. `fe-hexon`
keeps the registry only as a dev-dependency, so folding would put a server bin inside a library
crate. Registry work is part of the extension-platform store track (`conductor/test-batches.md`
R11-STORE, lines 417-437). Outcome: DEFER (see decision record).

## Dependency sizes: not measured

`cargo tree` is not run in this round (no cargo of any kind). The closure sizes for G-1 and G-2 are
therefore not measured. The statements above list direct dependencies only. A measured closure
(`cargo tree -p fe-hexon-registry`, `cargo tree -p fe-plugin-test`) is a precondition for re-opening
either gate.
