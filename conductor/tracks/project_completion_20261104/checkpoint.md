---
type: ProgressCheckpoint
date: 2026-11-04
status: in_progress
title: Completion baseline and next actions
---

# Current state (2026-11-04, after QA36)

- Goal service is active; the attached objective was read and matches goal-service.txt.
- Main source is fa955563. Combined production stage `.qa/stage/fa955563-20261104084453622` succeeded in 327.5s. It emitted one unused-mut warning at layout_policy.rs:207; this is not the final checks sweep or a warning-free build.
- QA36 disposable terrain run `.qa/run/20261104-084851-621` is stopped. All eight recorded root/descendant process identities were checked absent. Both retained campaign stores remain untouched.
- QA36 packet has independent focused evidence approval ([review](./reviews/qa36-full-evidence-review.md)): [manifest](../../../qa/passes/36/packet-manifest.json), SHA256 `2c28abd2f725696bffd655dc1727d0a1114077cb6f6ac1b1c9b2f24a96ea6473`, 154 payloads / 14 actual screenshots. No full-pass credit.
- Independently reviewed focused results: preferred widths survive scale clamps; actual divider drags preserve required camera.pose fields and selected node transform; rectangle configuration leaves terrain unchanged before arming; partial 5402/16384 coverage withholds cut/fill; complete 16384/16384 genuine zero saves the pair as one atomic batch at versions5/6. Full history bytes remain identical at version6 across navigation commands and final observation. The after-up state still had Petal A active: no actual departure/return is established by QA36.
- Preserve the invalid first camera comparison alongside its corrected camera.pose-based proof. Preserve the navigation input timeout, wrong-route refusal and initial relative stop-path refusal. The one runtime ERROR is the retained input_timed_out HTTP500 at16:08:40; later applied state was observed without replay.
- Visual gaps remain: two measured 10px divider targets and low contrast on the active-Petal breadcrumb. Browse-state opaque headers improved, but this does not close all chrome. Divider v3 is frozen in isolation for review; its design reviewer identified Results side-content overlap requiring correction.
- QA-0070 remains open. Grant-continuity plan v2 SHA256 `64f1b3cb57eb0213d22bf4e2a24f8c64362278c86758efc4ad28f59d52543114` has independent functional/security design approval. Implementation must preserve bounded enqueue/deadline ownership and prove composed transaction/isolation behavior. A fresh isolated worktree is prepared; no source or runtime acceptance is claimed.
- Native navigation foundation is being authored in isolation; it enables no positive action profile yet. The reviewed helper and trustworthy capture remain pending.
- Finding register:61 entries,14 open/reproduction (2S1,7S2,5S3); source-fixed/live-pending and focused states retain their existing limited meanings.
- Next: finish QA36 publication bookkeeping; execute separate fresh analytic nonzero volume/material/intersection run; correct/review divider v3 and implement/review QA-0070/native interaction. Then continue the complete plan. Final integrated sweep, two full passes, exact-SHA CI and all release gates remain pending.

# Historical acceptance baseline

This is the durable current checkpoint for the user-adopted completion goal.
The older [campaign progress ledger](../../qa-chain-progress-20261101.md) retains
chronological history; its early status paragraphs are not current-state claims.

- Integrated source: `a1a8691bf4e7f9ceeeec6b7d618ae6a8e12df366` before this documentation slice.
- Matching stage: `.qa/stage/a1a8691b-20261103063616971`, successful production build in 444.9s with zero warnings; log `.omc/research/qa35-audit-stage.log`. Stage source_dirty=true is recorded; this is not a clean-checkout claim or test-suite result.
- At the initial baseline, last recorded owned apps were stopped. Continued execution and the later exact owned stop are recorded below. Retained campaign Owner and recipient stores are preserved.
- Latest committed focused packet: [pass33](../../../qa/passes/33/), commit `ee6773db`. Focused passes do not count as full coverage/convergence.
- Final integrated format/lint/test sweep, final-SHA CI and tagged release are pending.
- Goal service: existing original goal is unfinished and reports `usageLimited`. A requested replacement was refused because an unfinished goal exists. This track records the expanded user instruction without falsely completing or bypassing that goal.

## Verified focused outcomes

Typed property editing/grid actions and local data restart, history chart/table
and completed-save playback, per-entry icon/color preferences, smaller glyphs,
group lifecycle and exact Undo/Redo, selection count/primary agreement across
tree and right panels, Ctrl/Shift selection, navigation clearing and four repaired
native asset reads have focused evidence. Pass33 additionally preserves scrolled
row positions through selection and keeps twenty complete stored responses
unchanged. These are bounded successes; the final combined matrix remains open.

## Findings inventory

Read from `qa/findings.jsonl` on 2026-11-04: 60 entries, comprising 39
fixed_verified_focused, six fixed_source_live_pending, thirteen open/reproduction
items, one eventual-cleanup/review-pending and one closed. The thirteen include
two S1, six S2 and five S3. Unmerged source-review findings are additional work,
not counted as closed defects or included in this denominator.

| Open IDs | Remaining issue |
| --- | --- |
| QA-0024 (S1) | Brush configuration persisted an unintended world circle; root cause pending. |
| QA-0063 (S1) | Joined Viewer has hierarchy but no ordinary nodes; source repair needs fresh paired proof. |
| QA-0069 (S2) | Missing coverage can persist zero volumes; separate writes churn history. |
| QA-0025, QA-0026 (S2) | Black scene-only capture; capture acknowledgement timeout after persisted edit. |
| QA-0028, QA-0034 (S2) | Restore/navigation acknowledgements time out despite an applied action. |
| QA-0039 (S2) | Small chevron/chrome hit targets. |
| QA-0004, QA-0006, QA-0007, QA-0008, QA-0016 (S3) | Populated status counts, viewport selection indication, Enter submit, Escape navigation and stale empty-Petal Inspector. |

Live proof still required for source fixes QA-0041 (scope IDs), QA-0044
(credential authority after restart/reset), QA-0048 (QA evidence accounting),
QA-0049 (invite tracing), QA-0050 (invite import/readiness) and QA-0056 (stamp
fallback duplication). QA-0021 retains cleanup review closure.

## Unfinished source and failed execution

| Slice | Exact state and next action |
| --- | --- |
| Volume ownership | Isolated `.omc/worktrees/qa-volume-measurement-ownership`, frozen base `bef56e790fdd80ce5688055c820ee11709e0a3f9`, seven uncommitted correction files observed. Not integrated/approved. Preserve them. Three MED blockers: use-time direct projection eligibility, accepted owner retained across terminal reset, and property-only resolved projection preserving new gestures. Low-priority hash cache correction also remains. Reviews: `.omc/research/qa-volume-ownership-{functional,security}-review.md`. |
| Native witness A2 | Isolated `.omc/worktrees/qa-native-a2`, frozen `9902f43105e922fa9097c00904976d50ac5e821f`, not integrated/compiled/approved. Four blockers: completed Guard retained/rechecked; suspend/resume retirement; actual view/unblocked access; accounting for direct painters outside Areas. Functional report `.omc/research/qa-native-a2-witness-functional-review.md`; security agent interrupted before final artifact. Helper, navigation profile and WGC capture remain unfinished. |
| Legacy fixture | Preparation source/helper compiled; first fresh launch failed `helper_io_refused` before app launch. Preserve `.qa/run/20261103-064506-766`, including GLB and legacy-preparing.json; do not replay generate into it. Log `.omc/research/qa35-launch-fresh-owner.log`. Investigate stdin/base-stream closure, then independently review and use a fresh fixture root. Offline store reads remain explicitly unsupported. |
| Live asset audit | Main `a1a8691b` metadata-only endpoint source/integration approved and compiled; no positive live audit/adoption executed. Use actual runtime identity/current Owner and exact UI operation receipt. |
| Require-existing restart | `.omc/research/qa-legacy-live-resume-plan.md` is a proposal, not approved implementation. Prevent reseed via actual DB-owner validation and provisional owned startup; never use a second offline store opener or early public handoff. |

## Immediate order and coordination

1. Completion scope committed55cead54 after independent approval; [review](./reviews/conductor-scope-review.md).
2. Repair/review the fixture helper I/O path while preserving the failed root.
3. Finish volume ownership corrections and independent reviews; retain accepted unknown saves until the actual DB lifetime ends.
4. Correct/review native witness and implement the minimum reviewed normal recipient interaction needed for paired tests.
5. Build only approved integrated source, then execute volume, peer-node/asset, adoption and permission/lifecycle acceptance with immutable evidence.
6. Follow the remaining [plan](./plan.md) through full QA/UX, final checks and release.

Only one owner runs Cargo/apps. Authors use isolated checkouts and reviewers stay
independent. The preexisting `conductor/.conductor_session_log` is not part of this
slice. Keep runtime session notes consistent, but this Conductor checkpoint is
the durable current scope. No product answer is required for the next routine
fixes; record any genuine future fork in the decision ledger.


## Continued execution and visual steering (2026-11-04)

- Completion scope committed55cead54. Reviewed stdin ownership correction committed98572fdb; exact source review is retained in reviews/legacy-helper-io-source-review.md.
- A fresh legacy preparation and authenticated app startup succeeded in30.5s using the matching Rust stage and current corrected C# launcher, with no stale-stage override. New disposable root .qa/run/20261104-070405-130, OwnerPID83716. Preparation only: no adoption submitted and no unsupported offline reader used.
- Native state and a 1280x720 screenshot were captured and the image was actually inspected. It reproduces the user's vertical one-character Petal label in the hierarchy card. VIS-001 remains open; no visual-fix claim is made. See evidence/hierarchy-card-wrapping-before.png and helper-fresh-state.json.
- Exact recorded root/descendant stop completed exit0; log evidence/helper-fresh-stop.log. No new OS census or universal descendant-absence claim is supplied.
- Preserve the original failed .qa/run/20261103-064506-766. Successful preparation does not establish legacy adoption, live audit, restart or peer rendering.
- User will replace/start the goal service manually; ready-to-paste objective is goal-service.txt. Existing service status is not changed by this documentation.

Next execution begins with VIS-001 and the remaining P1 correctness/automation
blockers, followed by the complete queue. Screenshot inspection and fixing visible
glitches are explicit acceptance requirements throughout.


The user has now replaced/started the goal. The new attached objective was read
and matches goal-service.txt; the service reports active. The earlier usage-limit
and replacement-refusal entries describe the preserved baseline, not current
service status. Continue the expanded objective from this checkpoint.


VIS-001 source correction is independently approved and integrated from isolated
1d3297de: explicit vertical card layout, actual frame margins, measured breadcrumb
header and bounded text. Review: reviews/hierarchy-card-source-review.md. A fresh
production build and actual screenshot/action matrix are still pending; VIS-001
remains open. The new visual-checklist.md records the required inspection states.


The corrected native A2 witness (9902 plus5a51daa1) has independent functional,
security and exact integration-preservation approval. The integrated index keeps
26/27 full source blobs exact; the host documentation retains all main content
plus the exact new suffix. Four source blockers are addressed. Compilation,
actual native delivery/publication/lifecycle, helper/navigation/selected-Inspector
profiles and trustworthy capture are still pending. No WGC success is claimed.
Both focused visual runs are stopped; all16 recorded identities were checked
absent. QA34/35 packets are frozen for independent evidence review.


Independent QA34/35 evidence review approves the focused small-count VIS-001
result: readable1petal/3nodes at actual1280/1024/1600 widths and1.25/2 scales,
card navigation and Options/Close, Create/Cancel with exact unchanged hierarchy.
All16 recorded identities are absent. Preserve ambiguous-locator pre-input refusal
and Browse-All404/lost-scope gap with no replay. All-Verses cards, long names,
large counts and extreme widths remain unverified. Layout checks reported zero
measured violations but incomplete text coverage; actual images keep VIS-002
header contrast and VIS-003 session panel-width loss open. VIS-003 is confirmed by
source investigation and has an independently reviewed implementation design.


The full volume stack bef56e79 plus3ed94d8b now has functional/security and exact
integration-preservation approval. All43 source contributions retain their gates;
35fullblobs are exact and the eight known overlaps preserve main native/audit/
fixture behavior. Terminal reset now retires native input and volume intent,
while retaining accepted outcomes until the actual successful DB-worker join.
QA-0069 is still open: combined compilation and live volume/source/navigation/
late-receipt/new-gesture/reset acceptance have not yet run.


VIS-002 header contrast source is independently approved and integrated: opaque
existing-theme breadcrumb/title strips with measured layout. Exact reviewed
viewport bytes and the exact inserted directory note preserve all volume/native
main changes. The insertion was applied at its original section rather than
assuming an EOF append. Fresh combined build/screenshots remain pending. VIS-003
preferred panel-width preservation is authored in its isolated checkout under
the independently approved public-divider design.


QA34/35 publication committed3062a9ab after full and bookkeeping reviews: all88
staged packet files match raw bytes and review anchors. The combined515b88fc
production build failed (exit1, no stage/app) on unsupported AccessKit setters;
all three setters now use the pinned Windows provider-backed set_author_id after
independent review. See evidence/build-attempts and native-sdk-author-id-review.
VIS-003 implementation is frozen for two independent source reviews; no preferred
width fix or header contrast live acceptance is claimed yet. The final integrated
sweep, CI and release remain pending.


Current source HEAD5322a889 includes the reviewed three-token AccessKit fix.
Combined build96041 is closed exit1; no app or new stage was launched. VIS-003
source review found a left pending-close cancellation gap; a narrow correction
is in progress before re-review. All source/main work remains local; no final
integrated sweep, CI, tag or release has been run.


VIS-003 v2 now has independent functional/design and exact integration approval.
The pending-close MED is corrected before left prepare, with original dialog/
portal gates retained. All six working files equal approved raw hashes; Git index
content differs only by existing source CRLF-to-LF policy, and the complete patch
matches approved63cc9180...a2b6b90f. Shared adapter/panel defaults and native A2
producer hooks remain. No source-level closure is being treated as live visual
or gesture acceptance. The next action is a combined production build retry.


QA-0070 (S2, source-confirmed MED TERR-S1) records discarded API terrain token
scope/credential continuity before DB use. The supported concurrent same-Verse
Petal move ordering is source evidence, not an executed bypass. Stable-scope
fresh-fixture numeric checks remain approved, but queued authority/provenance
acceptance needs a verified-grant/admitted-ancestry transaction fence. Existing
60 finding records were preserved byte-for-byte; the register now has61 entries
and14 open/reproduction items (2S1,7S2,5S3). This is an implementation fix, not a
deferred product decision. Retained-store restart remains held pending the
require-existing startup contract.


QA36 independent full review approves all154 payload hashes/14 inspected images and
focused volume/width outcomes, with navigation claim corrected: command invocation
and history stability only, no observed departure. QA37 is now live in fresh
disposable run `.qa/run/20261104-095346-026`, PID120552, same fa955563 stage;
18.8s launch. Author observations: actualBack clears petal_id; separateRaise2 and
Lower1 cases report/store200fill and100cut respectively with complete16384 samples.
Inside2x1x2 solidbox reports4.000m3/material; outside control hasnativebounds and
nooverlap. These QA37 results remain active/unfrozen/unreviewed and carry no final
acceptance credit. Existing inputtimeout andlocatorrefusals are preserved.


QA37 exact owned stop completed; all eight recorded identities checked absent at
2026-11-04T17:19:41Z. Raw analytic evidence remains unfrozen/unreviewed. Both fresh
focused apps are stopped. No Rust source change or final check sweep was made
during these live runs.
