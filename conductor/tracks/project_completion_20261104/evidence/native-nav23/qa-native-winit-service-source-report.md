---
type: Review
title: Strict native service source author report
date: 2026-11-04
---

# Strict winit visual service: source freeze for independent review

Author `/root/legacy_helper_io_fix`, 2026-11-04, source-only.
This is implementation authorship, not source approval, compilation or native
acceptance. QA44's actual unexpected class and service-profile match remain unproved.

## Authority, foundation and exact scope

Authored in MAIN against inspected foundation
`a77cabbee465a690749eba684d8299e51c716b82`, whose service-source context is the
same reviewed9d1e061d baseline. Parent explicitly assigned ONLY
tools/qa/native_observer/{worker.cpp,controller.py,AGENTS.md}.
Parallel UI/DB/qa_debug, evidence, Conductor and .gitattributes edits were preserved.
No header/build/runtime/DB/UI/host/native-proof/ledger/lifecycle source changed.

Read the complete approved source plan and both independent reviews before edits:

| Document | SHA-256 |
| --- | --- |
| qa44-visible-window-source-trace.md | `7fb4b79a1cdd734a7336159936e76b7025d7f9203724828554a8676c564bf23b` |
| qa-native-winit-service-plan-functional-review.md | `bc5f23682d79429d041e89d7a1f4935ed560ac9a6aa0a44ebe99572967ae7d34` |
| qa-native-winit-service-plan-security-review.md | `c81160eff82f19aac2b072785d6596c144127d68948c13c58c99ecb20b2be99e` |

Both reviews approve design for authoring, requiring complete source review and
actual acceptance later. Shared UX/directory guidance applies; no product UI or
operator action channel is added.

## Frozen files and patch

| File under tools/qa/native_observer | Before bytes / SHA-256 | After bytes / SHA-256 |
| --- | --- | --- |
| worker.cpp | 40988 / `dab1b189cf55a87491b3f17af07d81160353e5debfbf5cd37d1bb35f915dd043` | 44445 / `da27fe8405ec5e5bd8b78b4f002fc53fc62fde4d79e97d776cf3e549e8a982ee` |
| controller.py | 15924 / `cad569ed06b08c4baa72d68ce54fb43cc8abb0f2df4c172c83b72c12ba0eca89` | 16097 / `0d4c87f67f19d7a21d2b9136b54ab2fd66b3751372812ce225f43f3463e17ec0` |
| AGENTS.md | 23114 / `18bca6742417cf05c67579db8309ddeaae95374628854e0ea164b65cf1f99322` | 26705 / `ab56fc920dcb95174cf894028ceb3e1a5b9e8eea56f81788b3b54b33590e8549` |

Frozen packet `.omc/research/qa-native-winit-service-source-v1/` contains all three
raw files and:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| source.diff | 16860 | `bd09b6991afe4808592c4a167e011f3b1f065f6756ecc4ee90063601ad553092` |
| manifest.json | 1054 | `4892a4f9306c1452d1302bc3303662d68f0e48866cd05c32ad95e1439bd55bbe` |

Scoped diff:155 additions/29 deletions, including52 directory-document lines and
two controller allowlist lines. No new source path, registered test declaration,
runtime mode or positive observation code. Patches normalize text for review;
exact raw hashes/snapshot govern source identity.

## Production predicate and facts

worker.cpp:234 `winit_service_signature` is the actual pure geometry/style seam.
It requires all four coordinates of BOTH window and client RECT exactly zero,
including origin. Normalized DWORD style equals WS_VISIBLE|WS_POPUP0x90000000;
extended style equals exactly NOACTIVATE|TRANSPARENT|LAYERED|TOOLWINDOW0x080800a0.
No undocumented flag mask, WS_CLIPSIBLINGS, size tolerance or offscreen allowance.
Source/SDK basis remains the locked winit0.30.13 constructor and SDK22621 constants
identified in the approved plan; strict runtime disagreement refuses.

`service_current` at242 requires valid HWND, nonzero same-root thread and retained
FE PID, ownerless top-level/no parent, exact bounded public class and visible state.
Both GetWindowRect and GetClientRect must succeed before the pure signature check.
It then applies the actual unchanged128-child refusal discipline shared through
`no_native_children`. Same-class hidden/malformed services are not treated as
ordinary ignorable hidden windows. The service receives no UIA/provider lookup,
action, title/user-data read, message or mutation.

`inspect_windows` at261 performs one complete census with eight fixed entries.
EVERY owned HWND consumes a slot, including service and ordinary hidden windows.
Failed thread lookup, failed/truncated bounded class read, callback/refusal or
incomplete EnumWindows stays unavailable. Unknown visible classes, including
Portal/dialog/default windows, still refuse. Ordinary unknown hidden windows keep
their prior policy but every later census revisits visibility.

The collection records only typed root/service/hidden kind plus native handle and
thread, never emitted. After enumeration it requires exactly one original-class
fractalengine primary and at most one service, then validates their thread/profile
relationship independent of enumeration order. All original primary checks remain:
valid same-PID root, owner/tool/child exclusion, visibility/minimize/class/cloak/
Default desktop and no native children. Root is rechecked after service facts.

## Immutable retained set and no adoption

`WindowSet` stores original root/service HWNDs and thread IDs. Operation retains a
const WindowSet; initial null service/thread0 is also immutable. Its const current
method at315 reruns the complete predicate and compares all four original fields.
New, missing, replaced or duplicate service/root/thread refuses; the current
snapshot is never assigned into the retained set or used to repair it. It is a
read-only identity comparison, not provider/target rediscovery. Original root and
thread are checked again after that census.

Known-class matching grants no role, source or host permission. The helper is a
zero-area visual event-loop service, not input-free: winit intentionally routes
WM_PAINT/WM_INPUT. Nothing hides, destroys, activates or reconfigures it. The
original one-second host proof and typed callback remain the only corresponding
navigation authority; no native service fact refreshes them.

## Exact production call graph and review conditions

One shared operation boundary, worker.cpp:679-683, brackets WindowSet.current
with the existing watcher/process/original deadline checks. The watcher still
starts before initial census at675; its CREATE/DESTROY/barrier/cap/heartbeat code
is untouched. Repeated native facts are required because those hooks cannot
establish that style/geometry/visibility never changed.

| Actual route | Retained-set coverage |
| --- | --- |
| Initial census and before COM/provider activation | inspect_windows at677, boundary684 |
| Every provider activation wait iteration | boundary700 |
| Accepted original root/source and observe validation | boundary723,727 |
| Final positive observe, after latest source/hash/property work | boundary729 immediately before emit730 |
| Before and after initial typed target acquisition | boundary734,738 |
| Original target/safety walks and current source in preflight | boundary743,748 |
| Tail of preflight AFTER all target/property reads | boundary751, then only local return |
| Before durable intent | preflight754 |
| After durable intent and final pre-Invoke | preflight756 ends in boundary751; no intervening native/property work before original Invoke758 |
| Post-return inspection/reporting | boundary before AND after target/source reads at762 |

This explicitly closes the review's final-observe and final-pre-Invoke gaps rather
than only replacing the old early root check. All work stays under the original
Deadline object; it is never rebound/restarted here. No local census grants a new
host deadline, native alias or source epoch. Any post-intent failure remains spent.

The sole ElementFromHandle remains at693 for the ORIGINAL root; the sole
GetCurrentPatternAs remains at522 in initial target selection, and the sole
original pattern Invoke at758. Same-thread retained COM objects, private demand,
original target/signature/source checks and ledger ordering remain. Post-return
S_OK remains submitted/outcome unknown; a failed inspection yields the existing
submitted_state_changed behavior, never consumption/render acknowledgement.

## Output compatibility and limits

Kept existing observed code as parent preferred. Worker success-exit predicate
at806 is unchanged and still recognizes observed or entered Invoke. Controller
success/outcome interpretation and strict output shape are unchanged; only six
fixed refusal strings are added:

- window_service_identity_refused
- window_service_geometry_refused
- window_service_signature_refused
- window_service_children_refused
- window_services_multiple
- window_set_changed

Legacy codes remain. No raw class/title/handle/count/name, service fact or new
positive field is published. Therefore **observed does not independently prove
whether service was present or absent**. Withhold a live service-profile-match
claim from that code alone. QA44 remains a failed generic-visible-window attempt
whose actual class is unproved; the reviewed source candidate is not a live result.

Enumeration, native fact reads and watcher delivery remain sampled/non-atomic,
not lossless HWND birth or interstitial-state proof. Same-privilege metadata/window
tampering is not solved. Unknown reads/progress/lifetimes refuse. Capture stays
unavailable; no Owner HTTP floor, role grant, generic input/action or exception is
introduced.

## Verification and remaining acceptance

Source/installed-primary-source/Git/byte/hash inspection and source/artifact authoring
only. Scoped diff --check and trailing-whitespace inspection are clean. No compiler,
parser/formatter/tests, helper/app/process/OS/native/API, database/network or Git
mutation occurred. No passing controlled fact or runtime case is claimed.

Both full independent source reviews must inspect the complete three-file diff,
actual fact extraction/predicate and every boundary. The production pure signature,
typed census and original WindowSet comparison are the seams for root's meaningful
controlled malformed-fact acceptance; no mirrored test logic or new test command
was authored outside assigned ownership. All geometry/origin/style/read/thread/
relationship/children/duplicate/census-bound/changed-set/watcher/expiry/post-intent
cases from the approved plan remain unexecuted acceptance requirements.

Root alone builds the exact reviewed source, checks fresh staged hashes and owned
input-off process/incarnation, independently inspects primary/closed-Portal state,
and runs ordinary observation plus separately authorized retained one-visible-Petal
invocation. Positive provider/navigation, Viewer/Editor, service presence, lifecycle/
restart/pruning/durability, capture, FULL campaign, integrated sweep, CI and release
are not supplied by this source freeze. Strict runtime signature failures must be
investigated, never loosened with speculative masks or arbitrary window exemptions.
