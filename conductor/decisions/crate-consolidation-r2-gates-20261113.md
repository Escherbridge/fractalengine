---
type: Decision Record
title: Crate Consolidation Round 2 - gate outcomes G-1, G-2, G-3
timestamp: 2026-11-13T00:00:00Z
status: DEFER (default outcomes, pending the user's answer)
tags: [decision-record, crates, workspace, consolidation, crate_consolidation_r2_20260718]
resource: ../tracks/crate_consolidation_r2_20260718/plan.md
---

# Crate Consolidation R2: gate outcomes (2026-11-13)

Evidence: `conductor/tracks/crate_consolidation_r2_20260718/evidence/consumer-map.md`.
Predecessor audit: `conductor/decisions/crate-consolidation-20260717.md`.

These three outcomes are the scoper's defaults. They are DEFER until the user answers. If the user
answers ACCEPT for G-1 or G-2, the entry changes to ACCEPT and REL-11 or REL-12 is unlocked in a
later round.

## G-1: fe-plugin-test into fe-plugin (test-utils feature)

Outcome: **DEFER** (default).

Counter-evidence:

- `fe-plugin-test` has its own published shape (`fe-plugin-test/Cargo.toml`; direct deps `fe-sdk`,
  `rhai`, `serde`, `serde_json`, `ulid`). It is published with `fe-sdk` under oss_release plan 4.2.
- Consumers are `fe-plugin` (dev-dependency, `fe-plugin/Cargo.toml:63`), `tools/fe-ext`
  (`tools/fe-ext/Cargo.toml:23`) and `extensions/iot-bridge` (`extensions/iot-bridge/Cargo.toml:19`).
- Folding it into `fe-plugin` puts the plugin host's wasm stack behind the harness dependency that
  OSS plugin authors install.

Reversal trigger: re-open when the OSS plugin-author closure, measured with
`cargo tree -p fe-plugin-test`, is acceptable inside `fe-plugin`.

## G-2: fe-hexon-registry into fe-hexon (feature-gated bin)

Outcome: **DEFER** (default).

Counter-evidence:

- `fe-hexon-registry` is built alone by `docker/Dockerfile.hexon-registry:6` and runs as the store
  service. Its direct deps are server-side (`axum`, `tokio`, `tower-http`, `zip`, `semver`) plus
  `fe-format` with `default-features = false`, so the image avoids the engine closure.
- `fe-hexon` and `fractalengine` use it only as a dev-dependency (`fe-hexon/Cargo.toml:45`,
  `fractalengine/Cargo.toml:32`).
- Format-level merge belongs to `hexon_unification_20260716`; this track must not fork that workstream.

Reversal trigger: re-open when the hexon extraction out of this tree is decided, or when the registry image can
be built from `fe-hexon` without pulling the library's dependency set into the store container.

## G-3: fe-query into fe-api

Outcome: **DEFER** (default).

Counter-evidence:

- `fe-query` is consumed by `fe-api` (`fe-api/Cargo.toml:18`, feature `datafusion`) and by
  `fe-database` (`fe-database/Cargo.toml:15`, feature `parquet`).
- `fe-database` has at least seven use sites (carried from the scoper, see consumer-map.md). A merge
  into `fe-api` would make the storage crate depend on the HTTP crate. That is a layering inversion.
- Option (a) moves `fe-database`'s query-builder and columnar pieces to a lower crate first. Option (b)
  keeps two homes for the query code. Neither is a verbatim move, so neither fits the track's
  structure-only rule.

Reversal trigger: re-open when `fe-database` stops using `fe-query` builders.

## Scope

- No code, `Cargo.toml` or lock changed by this record. REL-11 and REL-12 stay NOT PLANNED.
- Dependency sizes were not measured (no `cargo tree` this round). The reversal triggers name the
  measurement that must exist before re-opening.
