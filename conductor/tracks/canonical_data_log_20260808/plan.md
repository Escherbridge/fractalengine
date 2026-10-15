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

## Workstream G — implementation (tracked separately)

Workstream S's owner approval (2026-08-08/09) unlocked Workstream G, the gated
implementation of SPEC-1..8 in `fe-canonical-log` + `fe-database`. Its own wave
plan, slice briefs and per-wave status live outside this file — this plan never
duplicated them and does not start now:

- `workstream-g-plan.json` — the original wave partition (waves 1-4).
- `campaign-plan-w4-20261013.md` — Wave 4's campaign plan, which supersedes
  `workstream-g-plan.json` waves[3] (that brief had gone stale by 2026-10-13;
  see its own §2 "Current-code reality").
- `metadata.json` tasks `G-WAVE-1`..`G-WAVE-4`, `G-ERRATA-*`, `SPEC-9-durability`,
  `SPEC-10-observation` — the machine-truth status for each wave/phase.
- `conductor/doneness-ledger.md` rows `CDL-*` and `conductor/decisions/canonical-data-log-20260808.md`
  — the work-queue and decision register for the whole program.

**Status as of 2026-10-14:** Waves 1-3 complete-verified-committed; Wave 4
(`G-WAVE-4`, ledger `CDL-W4`) and the production `DeviceEnrolmentView`/
`IssuerAuthorityView` implementors (ledger `CDL-R2`) are DONE-pending-sweep —
code-complete and reviewed, tests written but not yet run. Network rollout,
relay seeding and inbound P2P on the canonical path remain owner-gated (NO-GO)
per this plan's own `network_enablement: prohibited`.

**Status as of 2026-10-15:** `CDL-SPEC10` (SPEC-10 observation plane + errata
G3/G6, `campaign-plan-spec10-20261013.md`) is also DONE-pending-sweep. Both
phases landed: Phase A (A1 draft, A2 `6b456e4`, A3 `65f39ec`, A3b `01e9668`,
A4 `f6e7af2`, A5 `f0b736c`, A6 `3788a6b`) and Phase B (B1 `2515ab9`, B2
`f7f9610`, B2b `f4df2a3`, B3 `446fa3c`), closed by three review-and-fix passes
— Phase A security FIXED (`e3ad8e8`), Phase B security FIXED (`1c5f8ef`),
correctness FIXED (`d098b65`) — and this B4 docs close-out. D-CL30..34 are
implemented as specified; the SPEC-10 Status line stays `PRE-APPROVED
2026-10-13 (owner pre-approved the draft's defaults; post-review pending)` —
owner post-review of the draft's defaults is a separate, non-blocking step.
TESTS ARE WRITTEN, NOT RUN for either CDL-W4 or CDL-SPEC10: the single
campaign-end workspace sweep this plan's Execution rules call for still
covers both. Phase B runs only under `MigrationMode::DualEmitShadow`/
`ShadowRebuild`; the live default stays `legacy_only` (SPEC-8 §3 rule 1), so
the hot tables are unaffected until the owner flips the mode. Network
rollout, relay seeding, inbound P2P, PLUG-HOST decoder plugins, and the
`canonical_authoritative_local` cutover remain owner-gated (NO-GO), unchanged.

## Execution rules

- `HARD-2` through `HARD-4` have one owner because they share `ws.rs`.
- Do not run concurrent Cargo builds. Run one serial integrated sweep after all
  hardening changes in the wave are applied.
- Record a blocker immediately in `metadata.json`; do not silently make a new
  product decision. Add it to the decision record as `PENDING-RATIFICATION`.
