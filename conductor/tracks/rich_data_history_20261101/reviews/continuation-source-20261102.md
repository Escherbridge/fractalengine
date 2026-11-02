---
type: Review Record
date: 2026-11-02
status: source-reviewed-runtime-gates-pending
---

# Continuation source reviews

These records summarize independent source reviews. They are not test execution
or live acceptance, and they do not supersede the campaign's final verification.

| Slice | Frozen source | Independent verdict | Remaining gate |
| --- | --- | --- | --- |
| Flexible schema revision migration | 56f6b43 | harness_security_review: valid pinned parser types, outer-before-element overwrite, nested fields retained; no bare array/any FLEXIBLE remains. | Same-root startup succeeded in pass11; final schema round trips and suite remain required. |
| Scoped SQL | 10fa755, 775b122, 0c11899, 62769c6 | harness_security_review: source-ready after closing computed TABLE/RECORD cast and versioned model-call escapes. | Integration, actual guarded transaction/result index, compatibility and negative cases. |
| Feedback/status | cb34c7c | harness_security_review: source-ready; existing queue, public identity and role authority preserved. | Compile and live hover/dismiss/conditions/contrast. |
| Groups/durable settings | a57940d, 77fc2ac, bc1032f, fc160ba | harness_advocate: source-ready after correcting cross-field finalization and focus-only retry. | Compile and native drag/key/text/readback journeys. |
| Saved region reports | 1c3b168, 2d02934, 5945188 | harness_advocate: ownership issue closed; requested one-line type-inference correction applied in 5945188. | Compile and actual-region/context report reachability. |
| QA driver namespace and snapshots | de2b70a, 5581381 | harness_security_review: qualified namespace valid; shallow defaults limited to display_name/asset_id/tombstone preserve the typed snapshot contract. | Pass13 all38 functional checks succeeded; warning reconciliation and restart/distributed issue remain explicit. |
| Checked Undo and interaction stack | aaaaf6e, 7f6210b, 77feeae | harness_security_review and interaction_design_review: source-ready after three corrections: separate version endpoints with verified predecessor rebasing, current-only recovery projection/cache, and live navigation at enqueue/drain. | Integrated through60e5772; compilation and existing real journeys/live behavior pending. |
| Distributed catalog restart repair | 9fb8848, 01baece | harness_security_review: source-ready, bounded stored Manager authority/read fences, allowlisted wire rows, committed Petal publication and catalog admission ordering. | Integrated5ed9e6c/c6fda5f; prove old populated Petal recovers without re-ingestion, then fresh scenario/restart. |
| Selected-object terrain diagnostics | 1ee27ec | harness_advocate: source-ready; existing Owner/input/scope boundary, selected-object bounds and five pristine probes only, epoch explicitly unverified. | Integrated3461b8b; runtime grounded origin and sample/bounds proof pending. |
| Combined integration | main60e5772 versus ea8c8e8 | harness_security_review: no combined blocker; reviewed final Undo/ScopedQuery/distributed files match, ParentMutation helper/index and Viewer/Editor/Manager floors preserved, report Petal pin and optional public identity wiring intact. | Build and live acceptance pending. Reported CRLF hygiene is normalized separately; no runtime claim. |

## Scoped SQL compatibility boundary

The shared API guard now refuses whole-word TABLE and RECORD, including their
field/string uses, as a dedicated security correction. Ordinary API comparisons
remain supported. The desktop dialect additionally refuses every unquoted less-
than character, including less-than comparisons, along with its documented
regex/division/generator/dynamic-call restrictions. Parser statement counts are
not treated as proof of authorization. Future dialect expansion is D-QA-010 in
the campaign decision ledger.

## Review discipline

All authors used isolated worktrees without Cargo, app control or live input.
Root owns integration, compilation and the QA process. Review-found fixes must
retain their evidence and receive a follow-up verdict; an authored journey alone
does not close a runtime acceptance item.

## Optional Undo structure follow-up

The independent design review found the Command/Memento boundary appropriate.
Before adding another command kind, consider an explicit PendingPhase enum for
path-read/commit/reconciliation states and extract confirmation validation,
cursor rebasing and retention into named helpers. These are maintainability
recommendations, not unresolved correctness findings or user decisions.
