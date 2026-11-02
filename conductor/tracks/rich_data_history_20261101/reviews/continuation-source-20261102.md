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


## Pass16 independent evidence and combined source approval

The independent verifier supports all19 focused derived pass16 checks, measured
330.75px compact body scrolling with fixed y154/h24 tabs, scoped token controls,
fixture receipts and exact cleanup records. All12 startup line hashes were
recomputed. Eleven WARN lines and the duplicate logger ERROR remain explicit;
no clean-log or GPU-validation claim. The served corrupt body was not retained
separately, so its member-difference claim relies on the inspected capture oracle
and receipt. Original failed attempts and coverage limits remain visible.

Independent design review found a duplicate Nav signature/call in the Path/terrain
merge. Root8d2df989 removes exactly those two lines; the verifier independently
approved that correction. Loggerd9440e39/main4aaa0a27 passed separate source review.

Portal670b8808/main6386868b passed both security/read-model and design rechecks:
its Viewer-fenced current reads hydrate the exact cache independently of form
ownership, preserve newer drafts and reject replaced/rebuilt targets. All reviewed
Path/Portal/WEB seams on maind7fadfa2 passed combined security review. The offline
resolver updated only three dependency edges, with no version upgrades; root
05285dd7 commits them and six whitespace-only changed lines. Stage build is now
running; no compilation or live acceptance is implied by these source verdicts.

## Pass17 independent evidence and next source batch

The independent verifier approved the focused pass17 evidence. All three canonical
hashes recompute; versions6 through10 retain the entire old history prefix, and
Cancel/Remove/Undo/Redo/final Undo match their intended full bags and raw rows.
The nine transform axes and native token controls meet the measured target sizes.
Property Edit/Cancel succeeded after actual226px horizontal scrolling. Stage and
restart manifests match; all eight saved cleanup identities are absent.

All13 stdout warning-line hashes match, with zero ERROR lines and two explicit
URL transaction refusals. Duplicate logger initialization is absent. Screenshots
show three red stamped cubes and the descriptor stores fixed_count3, but selection
remains the Path; atomic stamp Apply is not claimed. No JWT was found in saved
JSON/JSONL. FULL coverage, convergence and integrated-test flags remain false.

Separate security review approves Portal ff540ddb, terminal reset throughfca37755,
durable auth through964b000e and native fixture surface throughe3056d4b for source
integration. Design review approves Path UI be7a0518/17cd7b4e and catalog2aa15dbe
only at its audited d7fadfa2 snapshot. All require integrated build/live execution.

Coverage re-review approves fe16e792/dd69e818/43cb0821: immutable pins deny write
and delete sharing, active stdout alone permits its writer, and bounded evidence
hashing and strict parsing consume the same byte buffer. The synthetic packet has
not run and supplies no application coverage. Terminal-close evidence after root
exit remains a separate mandatory implementation and acceptance gap.

## Integrated source batch after pass17

Main6875a256 integrates the Portal canonical snapshot/explicit metadata fix;
be1afedd/7037576b integrate Path label/target/help ownership. Reset service,
proof and desktop supervision are8a43f89e/f992cfeb/20b74543. Durable auth is
06630fab/6ce74b78/7c20f75a. Native Portal evidence is
c677bedb/a0b2814f/8c48c308/c78f9c8c. Source conflicts were directory-guide
appends; both contracts were retained. Offline lock resolutionff274672 adds four
existing-version dependency edges and removes the known unused mutable binding.

Independent combined-source review found the Portal, Path, reset and native
registration seams intact, but requires one further auth correction: repeat the
DB-less unavailable-feed predicate after the final awaited cache read, before
the fresh expiry check. That correction is authoring in an isolated lane and is
mandatory before the next build/acceptance claim.

Coverage464731b8/218f407b/93ffdc1d is independently source-approved. Catalog
ca91bd44 retains its d7fadfa2 audit provenance and unexecuted status; later source
requires another reconciliation. Stamp selection and terminal-close evidence
are still authoring. No tests or new-stage runtime acceptance are asserted.

Independent review approved the narrow final-cache correction539bb064, now
main07852fad. Both cache reads share the JTI/unavailable-feed predicate; fresh
expiry remains last. The authored lock-interleaving journey remains unexecuted.
The same reviewer approved checked stamp DB2ebabc40, nowb1ff72df, including raw
source/position, actual permission, history, descriptor, sampler index, Petal,
target identity/absence and creation-schema transaction fences. GUI producer and
selection binding approval is still required. One exact duplicate incoming guide
section was removed during its inspected append-only merge.

The configured production compile in qa18-compile2.log reached the binary but
failed on missing direct serde/ulid dependencies. Main462f9a70 declares those
existing workspace packages and their two lock edges; it also corrects conditional
confirmed-clear credential copy and excludes submitted invite_string from tracing.
Those narrow root changes await separate review. The preliminary compile used the
wrong profile and was interrupted; no success or test evidence is inferred from it.

Independent review approved462f9a70's two existing-version dependency edges,
conditional reset labels and invite tracing exclusion. Configured production
compile3 then succeeded in2m27s without warnings through checked stamp DBb1ff72df.
No test suite or new-stage live acceptance was run. Catalog followup3b9e2aca,
nowmain2c713add, passed separate source review at audit07852fad:142families,
1300IDs, all1183prior retained,1286mandatory,49MCP/103REST. Final fingerprint
remains null and execution false; stamp/terminal-close reconciliation is pending.

Source investigation found QA-0050: the current scoped-invite Join path returns
VerseJoined without importing a Verse, role or namespace. Independent exploration
also finds no cold secretless read-capability import/peer bootstrap on the ordinary
open path. This is mandatory collaboration functionality work, with no runtime
exploit or successful join claimed. Source transport/authority planning and actual
two-owned-app acceptance remain required before FULL coverage.

## QA18/19 integration checkpoint

Mainb32195fa integrates terminal-close supervision. Checked stamp selection is
b9271afe/e7e69c19, with84ca20af registering its reset resources and extending the
existing App journey. Catalog5f374efc/26e0a2fd records143families/1375IDs,
1361mandatory at audited84ca20af; fingerprint remains null and execution false.

The sealed84ca20af production stage built in240.5seconds including helpers and
sealing. It emitted one unused test-only StampRenderSource export warning. QA18
and19 ran this exact stage against the retained world. Derived evidence review is
pending; neither run is FULL, convergence, or integrated-test acceptance.

Separate source reviewers approved the following frozen stacks before integration:

- Mapf028/e9 is main0bccfdb9/f825c309. Complete native Petal row bytes, including
  NONE/geometry/metadata, are captured and checked by promotion and Apply. Existing
  regression geometry uses pinned-engine-compatible fixed literal SQL.
- Creatione7eb/0df/9fb/935 is main5f1f5fcd/7fa38240/d310fe9c/7c25b859.
  Initial creation is one scoped atomic baseline/operation transaction; committed
  TRS/properties/version use manager materialization. Native completion is bound to
  exact captured scope, dialog and nonce. Full queues never block the UI; unknown
  completion cannot silently authorize another import.
- Shared startup-nav5689 and receipt hydrationb306 are main77d71312/9a9ebd13.
  Explicit navigation/Back suppress startup auto-selection without inventing an
  intent transition during read-only hydration suppression.
- Formsd2/cac is mainca28d1fe/edebf4e8. Shared text-input minimums cover remaining
  native forms; scoped header geometry restores Context style before user content,
  retains intended body style, and handles no-body restoration.
- Nativec569 is main712c0e20. Only exact admitted page-one-to-two navigation may
  retain intermediate old Source for one generation. Actual page-two Source plus
  fresh fixed DOM observation remains required; all unrelated invalidations abort.

Root resolved additive directory-guide conflicts while preserving both contracts.
The GLTF existing journey merge retains both bounded queue admission and scaled
geometry checks. Maine562696b updates that merged fixture to the typed request
owner/time API and makes the test-only stamp export conditional. Combined-source
review of these integration seams is pending; no tests have run.

QA18 exposed duplicate unscoped native creation facts (QA-0051), and source followup
found initial nonidentity transform loss (QA-0052). QA19's second-link terminal
failure is QA-0053. Integrated source fixes await a fresh-stage replay; old retained
history and failed attempts remain unchanged. P2P QA-0050 is still open in its
isolated checkout; no paired-app or credential transfer execution is claimed.

Independent combined review approved main source throughe562696b, including typed
request ownership, complete creation/materialization, both startup latch branches,
scoped UI style restoration, native-row map fences and expected native navigation.
It approved the merged owner/time fixture and conditional test export. One trailing
documentation blank line was removed. No compilation or test execution is implied.

Independent evidence review approved QA18/19's bounded claims after correcting only
the derived Point2 heading's UTF-8 encoding. All131 input hashes,129/68 JSON records,
all retained log-line hashes and five PNG hashes/dimensions reproduce. QA18 has
11WARN/0ERROR; QA19 has11WARN/1ERROR. Full node/history save-clear equality and local
Model URL/TRS readbacks reproduce. Native page-one receipt and15 actual client path
refusals reproduce. Exact cleanup is supported by retained records, with no fresh
process inspection. Open defects, unavailable replay, failed stamp promotion,
terminal second-link failure, and all FULL/test/convergence limits remain explicit.

P2P re-review closes startup/transient-cursor lease issues but finds a remaining
MED: Cancel/timeout after Join replaces an already-current Verse replica can close
it while normal lifecycle sees unchanged cursor and never reopens. Isolated repair
is assigned; the invite stack remains unintegrated and functionally incomplete.
