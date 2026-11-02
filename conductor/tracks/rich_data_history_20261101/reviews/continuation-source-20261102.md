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
| Requirements persistence and saved cue | 094de4e, c6163e3, a3b0ff8 | harness_security_review and interaction_design_review: source-ready; actual stored authority, raw-document/scope CAS, atomic operation/readback, exact pending identity and newer drafts retained. Clean and dirty saved copy corrected and rechecked. | Integrated through b34df9f; compile, real saved declaration/reopen and replication journeys pending. Declaration-only product boundary is D-QA-012. |
| Report scroll flow | 3a3251b | interaction_design_review: source-ready; remove inner vertical viewport, retain bounded128 rows and per-node identity in outer report scroll. | Integrated9b73a7e; three-size painted-clip journey authored, next-stage live QA-0038 repair proof pending. |
| Shell recovery/settings/reset safety | 795fca9, 237c38d, 580c274 | harness_security_review: source-ready only with live Reset disabled; per-section Retry, actual disk receipts and immutable offline clear fences inspected. | Integrated through d0d7192. Coordinated API/sync/plugin shutdown and runtime/cache/session invalidation remain open before enabling live Reset. Compile/native and isolated-store journeys pending. |
| Inspector URL/endpoint/token lifecycle | 90c786b, 90562d5, f903a25, d569d40 | harness_security_review: initial stack source-ready. interaction_design_review found same-target report-tab cleanup bypass; private tab/setter plus submission/reply guards close it in d569d40, independently rechecked. | Integrated through8942516. Combined integration review, compile and native mint/clear/endpoint/A8 proof pending. |

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

## Focused evidence review

The independent qa14_evidence_verifier recomputed the pass13 context hash and all
three original fleet state digests, checked read-only traces and exact REST/MCP
rows/coverage metadata, and confirmed two covered shards and a real answerer with
unchanged six raw plus six calibrated readings. This supports targeted cross-build
recovery without re-ingestion; it is not the strict same-stage driver restart gate.

The same verifier checked native consecutive Undo/Redo input traces against each
stored and ECS readback, immutable history prefixes and restored Cube3 properties.
The initial supported baseline explains v4→v6. Terrain bounds and five probes
independently reproduce2.2704029455780983m³, matching the2.270m³ display; the report
epoch remains explicitly unknown. QA-0038 clipping was visually confirmed and
QA-0039 small targets remain open. No full coverage approval was given.

Pass13 warnings seq1327 and1328 were independently reconciled against the two
intentional refusal receipts and bounded owned stdout at matching timestamps:
invalid calibration.gain expected Number, and required maintenance.last_service
absent on Gamma. Before/after state/history/schema remain unchanged. All38 original
receipts passed with no generation/retention gap. Original artifacts, hashes and
coverage_ready=false remain unchanged. This review closes only those warning
classifications, not the campaign's clean full-pass gate.


## Pass15 qualification and pass16 source continuation

Independent pass15 reassessment withdrew only the scratch durable-URL equality
claim: node.webpage_url is unused; actual storage is model.external_url. Original
raw probes remain intact, with explicit correction and reviewed warning/A8 ledger.

Security review approved Path/Data throughc7dbd24a and Portal throughd5443024.
Separate design review then found two MEDs: automatic legacy stamp clamping and
obsolete URL receipt dropping committed manager hydration. Path60cb4c18 closes
clamping with retained input intent, selection/revision widget identity and shared
finite/range validation; design reviewer approved the correction. Portal hydration
is being corrected separately and has not been integrated.

Design review approved Inspector19de3155: selection/Editor guards, responsive native
24-point targets and separate token metadata/action rows. Security approved narrow
fixture discovery8f018100 and low-risk W5 cleanupa5b431ee. No tests were executed.

WEB security review rejectedc5a70903 for provisional Wry allowance plus fallible
native cancellation. Final296d7ab9 removes the dual-handler ordering dependency and
uses one native cancel-first owner with synchronous Stop/Close and terminal backend
failure. Independent review approved the complete source under its fixed trusted
fixture contract; cancellation+Stop+Close all failing remains explicitly unconfirmed.
This is not guaranteed transport prevention under every native-platform failure.
Lock resolution, compile and native journey execution are still required.

Pass16 focused runtime artifacts and a truthful limitations list are in
qa/passes/16/verdict.json. Independent evidence review and combined-source checks
remain pending; this record does not approve full coverage, convergence or release.
