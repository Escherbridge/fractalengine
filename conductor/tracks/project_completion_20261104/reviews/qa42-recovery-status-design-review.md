---
type: Review
title: Shared recovery status and diagnostics design review
date: 2026-11-04
---

# QA42 recovery/status independent design review

Reviewer `/root/legacy_helper_io_fix`, 2026-11-04.
**APPROVE at the requested design/readability scope. No concrete blocker found.**
This is a separate pass over the other author's slice, not self-approval or a
replacement for the earlier full security/correctness review. No execution or
claim that the underlying QA42 database guard is fixed.

Verified frozen manifest SHA-256
`176a275203080ce3c8638f099d8e5495ec11da4f507609b30556d7bb641f5ecb` at
`.omc/research/qa42-recovery-diagnostics-status-v1/manifest.json`. All15 raw file
sizes/hashes match. Read the complete changed-file patch, author report, changed
production seams and adjacent capture/route/refresh/maintenance ownership. Applied
the shared UI/UX guidance. Review baseline is a77cabbe; unrelated native/recovery
and root edits were not changed.

## Findings

- **The transaction classifier is a small, appropriately scoped shared module.**
  Transaction::{CurrentRead,Publication} gives fixed operation labels without
  duplicating ordering/redaction logic or exposing a broad logging API. The
  deterministic earliest non-NotExecuted selection, rollback fallback and empty
  fallback remain easy to inspect. Static reason mapping and statement-family
  labels expose useful diagnosis without interpolating unknown error text or
  bindings. CurrentRead's existing prefix/format is retained by its thin wrapper.
  The publication labels sit beside the existing statement additions and carry
  through PreparedSceneRow; no speculative framework or dependency is needed.

- **Publication behavior remains separated from diagnosis.** The actual final
  transaction SQL, bindings and fences are unchanged. emit takes indexed errors
  before dependency/node enqueue and returns the shared sanitized diagnostic on
  failure; successful enqueue ordering remains. The diagnostic does not reinterpret
  a successful local edit as failed/rolled back or waive a failing guard. Earlier
  preparation-time query behavior is outside this narrow final-transaction change;
  it is not a claim that every possible database error is newly classified.

- **Waiting status has one clear rendering seam and one controller owner.** The
  private read_status_controls takes EditorBatchState and returns an optional
  explicit action; render_read_status adapts that to UiManager while render_form
  uses its existing temporary state ownership. Inspector and Options call the same
  renderer on their existing Waiting routes. Terminal error, active read, missing
  capture and accepted-write reconciliation states are readable without another
  selection resource or retry controller. Refresh is named, observed, wrapping and
  sized through the shared target convention. No extra user decision or generic
  recovery subsystem is introduced.

- **The actual refresh/capture/draft boundary is preserved.** Existing refresh
  clears error/capture/interaction/read only when !is_pending; it does not clear
  draft, pending write or its outcome owner. An explicit pure-read retry is issued
  by ordinary maintain, not by rendering. Terminal error prevents automatic retry.
  The capture/key and interaction/transform-route checks still govern edit controls;
  neither new Waiting branch treats visible ECS state as captured storage. The
  complete form restores the same state before enqueuing the optional action.

- **Empty versus foreign terrain snapshots are distinguished at the right seam.**
  selected_terrain_diagnostic now maps an absent/no-Petal snapshot to the same
  unavailable reason before the genuine foreign-Petal stale check. It does not
  force producer collection or change ViewScope, selection, terrain, geometry or
  epoch gates. No empty snapshot becomes a zero-valued measurement or a capture.

## Existing journeys and limits

The authored extensions are meaningful production-seam checks: a real rollback
response exercises shared publication error selection; redaction/fallback cases
cover fixed output; the existing Inspector journey routes an owned terminal read,
finds the real accessible Refresh control, authors its click and expects exactly
one pure read with no queued edit or fabricated capture. The existing grounded
diagnostic journey adds both absent/default snapshot cases while retaining the
foreign-Petal stale case. No new registered test declaration is introduced.

Independently compared normalized production prefixes before the existing test
modules: op_log.rs and vision_bridge.rs are unchanged from baseline; registered
test declarations stay10→10 and23→23 respectively. Their delta remains inside
existing journeys. The Inspector change likewise extends an existing journey;
its production change is the intended Waiting renderer call.

No change request is warranted solely to replace these narrow helpers with a
larger abstraction. Root must still compile and run the integrated journeys,
reproduce the primary publication diagnostic, verify terminal error/explicit
read-only Refresh in both real panels, confirm preserved drafts/unknown-write
ownership and verify legitimate report demand. Narrow/responsive/disabled/AX
behavior remains actual UI acceptance, not source proof.

Only this report was written. Read-only source/Git/hash and text comparison;
no compiler/parser/formatter/tests/journey, implementation execution, application,
OS/native/process/API/database/network action or Git mutation. No packet/source
rewrite. Final CI, campaign and release gates remain unchanged.
