---
type: Track Plan
title: Hexon as Path Asset — Remaining W5 Plan (FR-5, FR-6)
tags: [plan, hexon, petal-hexon, multi-asset, hexon_path_asset_20260713]
timestamp: 2026-07-15T00:00:00Z
resource: ./metadata.json
---

# Plan: Hexon as Path Asset — remaining scope

FR-1..FR-4 shipped and reconciled to as-built (see `spec.md` §As-built notes,
2026-07-15). The loader/P2P residual is handed off to
`hexon_p2p_bucket_20260710`. Only the two W5 phases below remain.

Before starting either phase, confirm W5 is still next-up against the
2026-07-14 roadmap (BI-egress primary; this is P2 platform work) and
reconcile FR-6 with `hexon_p2p_bucket_20260710`'s FR-1/FR-2
(directory-asset manifest) so the two tracks don't invent competing formats.

## Phase 1: FR-5 — PetalHexon bake (XL)

Serialize a petal's node write-ops (`node_log`/`op_log`, already append-only
and HLC-stamped in `fe-database`) into a `fe-hexon` package; reloading the
package rehydrates the petal's nodes. No `PetalHexon` symbol exists anywhere
in the workspace today — this is net-new.

- [ ] Define `PetalHexon` type + package layout (op-log entries as hexon
      entries; decide entry granularity and versioning) in `fe-hexon`
- [ ] Bake: read a petal's `node_log`/`op_log` from `fe-database` and write a
      signed `fe-hexon` package (reuse Phase 6.5 manifest signing)
- [ ] Reload: install the package and rehydrate nodes into a petal
- [ ] Tests incl. READ-BACK: bake → reload → node set matches source petal

**Acceptance criteria**
- A petal with ≥2 nodes (incl. one carrying a `path_asset` descriptor) bakes
  to a package and rehydrates into an empty petal with identical node
  properties (READ-BACK verified).
- Reload is idempotent: re-installing the same package does not duplicate
  nodes (define HLC/op-id dedup semantics and test them).
- Package manifest is ed25519-signed and verified with `verify_strict` on
  load; tampered package is rejected.
- RBAC: bake requires Editor+ on the petal scope via fe-policy / existing
  helpers — no new ad-hoc role checks (Phase 8.4 flagged fe-hexon RBAC gaps;
  do not widen them).

## Phase 2: FR-6 — Multi-asset-per-node via `application/x-fe-directory` (L)

`node.asset_id` is single-asset; the `application/x-fe-directory` seam in
`fe-api/src/assets.rs:213` returns 501 NOT_IMPLEMENTED. Implement the seam so
a hexon/node can carry model data alongside other data.

- [ ] Directory-asset manifest format (align with `hexon_p2p_bucket_20260710`
      FR-2 sketch: content-addressed JSON manifest listing
      `{relative_path, content_hash, mime}` per file)
- [ ] Replace the 501 stub: `GET` on a directory asset returns the manifest;
      member files fetchable by their own `content_hash` via the existing
      asset endpoint
- [ ] Ingestion path for a directory asset (RBAC-gated, per existing
      mutating-handler pattern)
- [ ] Tests incl. READ-BACK: ingest directory → manifest + members
      retrievable; existing single-GLB behavior unchanged

**Acceptance criteria**
- A directory asset containing a `.glb` + one non-GLB file round-trips
  ingest → storage → API fetch; the `.glb` member is loadable by the stamp
  path (`asset_path` can point at a directory member).
- `application/x-fe-directory` GET no longer returns 501; unknown content
  types still fail gracefully (no panic, thiserror + `?`).
- All existing single-asset tests stay green (regression gate).

## Explicitly NOT in this plan

- `fe-renderer::load_to_bevy` / P2P blob fetch → `hexon_p2p_bucket_20260710`.
- `hexon_ref` stamp-from-hexon descriptor field → spec.md Open Question 1;
  decide during/after Phase 2.
