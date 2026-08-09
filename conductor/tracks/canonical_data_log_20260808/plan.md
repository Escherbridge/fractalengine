---
type: track-plan
title: Canonical Fractal Data Log plan
timestamp: 2026-08-08T00:00:00Z
status: active
resource: ./metadata.json
---

# Plan

## Workstream S — owner-gated specifications

| Task | Dependency | Acceptance |
|---|---|---|
| SPEC-1 Operation envelope and canonical encoding | none | Defines the envelope, deterministic binary bytes, quantized scalars, domain-separated signing, `(author, HLC)` uniqueness, and committed byte-exact golden vectors. |
| SPEC-2 Author key and identity lifecycle | SPEC-1 | Defines key rotation, compromise, recovery, historic-op impact, and `fe-identity` integration. |
| SPEC-3 Capability and revocation model | SPEC-1 | Defines append/fetch/decrypt/materialize/preview/seed authority per object class, delegation, epochs, persistent revocation, and blinded topic derivation. |
| SPEC-4 Log-first materialization contract | SPEC-1 | Defines admitted op → verified append → materializer → Surreal, deterministic replay, invalid-op errors, and checkpoint materializer identity. |
| SPEC-5 Branch, checkpoint, and retention state machine | SPEC-4 | Defines tracking/paused/detached, checkpoint signing, leases, storage obligations, tombstone retention, and bounded quarantine. |
| SPEC-6 Segment/shard manifest and relay trust | SPEC-1 + SPEC-3 | Defines BLAKE3 segments, HashSeq proofs, receipt re-hash, and relay-as-seeder-only constraints. |
| SPEC-7 Commit and preview wire protocols | SPEC-3 + SPEC-4 | Defines durable cursors, acks, resume, replay/snapshot recovery, and a distinct lossy preview protocol. |
| SPEC-8 Migration plan | all prior specs | Defines flag-gated dual emit from one write seam and rebuild-vs-live validation before cutover. |

## Workstream H — permitted local hardening

| Task | Ownership | Acceptance |
|---|---|---|
| HARD-1 Single database write boundary | `fe-database` | Route handlers through one log-first-strict seam; remove divergent orderings and direct `write_op_log` calls outside it. |
| HARD-2 WS transform scope enforcement | `fe-api/src/ws.rs` | Resolve node ancestry, enforce scope containment, and emit a real petal ID. |
| HARD-3 Scene fanout attribution | `fe-api/src/ws.rs` and event vocabulary | Attribute every relevant scene delta to a petal and remove broadcast-to-all fallback. |
| HARD-4 WS lag hygiene | `fe-api/src/ws.rs` | On lag, push a fresh snapshot for subscribed petals. |
| HARD-5 Wire dormant authorization gates | `fe-sync`, `fe-hexon`, `fe-api` | Use built auth gates and deny P2P writes by default without enabling network transport. |
| HARD-6 Desktop analytics wiring | `fractalengine`, `fe-api` | Share `Arc<EntityStore>` with the API thread so local analytics has a source. |

## Execution rules

- `HARD-2` through `HARD-4` have one owner because they share `ws.rs`.
- Do not run concurrent Cargo builds. Run one serial integrated sweep after all
  hardening changes in the wave are applied.
- Record a blocker immediately in `metadata.json`; do not silently make a new
  product decision. Add it to the decision record as `PENDING-RATIFICATION`.
