---
type: Review
title: Strict native service functional source review
date: 2026-11-04
---

# Strict winit service source: independent functional/design review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**APPROVED for the exact frozen three-file source slice.** The implementation follows the strict reviewed service profile, retains the original root/service identity set and closes the final observe/pre-Invoke boundary gaps. No concrete functional/design blocker was found. Compilation and actual ordinary/native acceptance remain required; QA44's actual unexpected class and a real service-profile match remain unproved.

Only this report was authored. Read-only source/Git/hash inspection; no implementation parser, compiler/tests, application/helper, OS/process/native/API/DB/network action or Git mutation occurred. This reviews another author's native slice, independently of my separate recovery/status authorship. Concurrent source, root evidence and live application work were untouched.

## Exact reviewed source

Baseline: `a77cabbee465a690749eba684d8299e51c716b82`. All three working paths match `.omc/research/qa-native-winit-service-source-v1/` raw files and manifest lengths/SHA values exactly. Baseline Git blob sizes/hashes independently match the author's before anchors.

| Path under `tools/qa/native_observer/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 44445 | `da27fe8405ec5e5bd8b78b4f002fc53fc62fde4d79e97d776cf3e549e8a982ee` |
| `controller.py` | 16097 | `0d4c87f67f19d7a21d2b9136b54ab2fd66b3751372812ce225f43f3463e17ec0` |
| `AGENTS.md` | 26705 | `ab56fc920dcb95174cf894028ceb3e1a5b9e8eea56f81788b3b54b33590e8549` |

Manifest: 1,054 bytes, SHA `4892a4f9306c1452d1302bc3303662d68f0e48866cd05c32ad95e1439bd55bbe`. Complete patch: 16,860 bytes, SHA `bd09b6991afe4808592c4a167e011f3b1f065f6756ecc4ee90063601ad553092`. Author report: 10,637 bytes, SHA `f4ac2b0bdc04e1940459b3e06ed97f2a8c9b96a15f06230d5d65db7eed1dce79`.

Reviewed the complete delta, actual predicate/census/retained-set bodies, operation call graph, controller protocol handling and documentation, retaining the independently reviewed unchanged foundation. Applied the exact strict plan SHA `7fb4b79a1cdd734a7336159936e76b7025d7f9203724828554a8676c564bf23b`, both plan-review conditions and shared UX guidance.

## Predicate and retained identity

`worker.cpp:234–252` implements the actual profile, not a name-only exemption. Both window/client RECTs must have all four coordinates zero. Styles are normalized to DWORD and compared exactly against `WS_VISIBLE|WS_POPUP` and the four locked extended bits. Failed geometry reads refuse. Valid same-PID identity, exact class, nonzero original-root thread, ownerless top-level/no parent, visible state and no native children are required. Hidden malformed same-class services also refuse. The child-check extraction at 210–218 preserves the original root's existing bounded sampled refusal discipline and its acknowledged enumeration limitations.

The census at 261–312 counts every owned HWND toward eight before classification and stores only fixed entries. Count overflow throws before entry access/iteration. Failed thread/class reads, truncated class, incomplete enumeration, unexpected visible windows and duplicate roots/services refuse. The unique root and optional service are resolved before checking their cross-window thread relationship, so EnumWindows order cannot change admission. Visible Portal/dialog/default windows remain refused. Other hidden windows retain the prior policy and are revisited by each census.

`const WindowSet windows` at 677 retains original root/service HWNDs and thread IDs, including initial service absence. `WindowSet::current` at 315–321 revalidates a complete census and compares all four facts to those originals, then rechecks the original primary/thread. It never assigns the observed census into retained state, adopts a later service or selects another primary. Exact signature constants are rechecked each time. This is sampled original-set validation, not unforgeable HWND birth identity or an atomic guarantee about interstitial changes.

The small pure signature, shared child check, typed census and const retained set give the change readable boundaries without a generic discovery framework or caller-supplied exemption. No UIA traversal/action, title/user-data read, message, focus/hide/style mutation or framework reconfiguration is applied to the service.

## Actual boundary coverage

The watcher still starts before initial census. The shared boundary at 679–683 brackets complete set validation with the existing original deadline, process and watcher checks; it does not rely on create/destroy quiescence to prove style/rectangle stability.

| Route | Source coverage |
| --- | --- |
| Initial discovery / pre-provider | Census 677, boundary 684 |
| Every provider activation wait | Boundary 700 |
| Accepted root / observe validation | Boundaries 723 and 727 |
| Final positive observation | Boundary 729 after source/property/hash work, immediately before emit |
| Initial typed target acquisition | Boundaries 734 and 738 |
| Full target/source preflight | Boundaries 743, 748 and final tail 751 |
| Durable intent / final Invoke | Preflight 754; arm 755; preflight 756 ends at 751, followed only by local receipt flag and original Invoke 758 |
| Post-return reporting | Boundary before and after target/source reads at 762 |

Thus the final pre-Invoke set check actually follows the intervening target/property work. The original COM root, marked root, target and InvokePattern remain retained. There is still one `ElementFromHandle` at 693, one initial pattern acquisition at 522 and one original pattern Invoke at 758. Existing fresh-root/source comparison is unchanged; no new host-proof renewal, target rediscovery, worker deadline rebind or typed callback allowance is introduced. Post-intent refusal stays spent; S_OK stays submitted/outcome unknown. Post-return validation failure retains the existing state-changed/unknown semantics.

## Compatibility and preserved foundation

The controller changes only its fixed refusal allowlist, adding the six actual service/set codes. Legacy codes, strict output shape and outcome interpretation remain. The positive `observed` code and worker success-exit predicate at 806 are unchanged, avoiding the incomplete positive-code plumbing identified in plan review. **Observed alone cannot establish whether the optional service was present or absent.** The directory documentation states that limit accurately.

Read-only comparison confirms no change from the baseline to `worker_protocol.hpp`, `windows.py`, `history.py`, `ledger.py`, bootstrap/worker cleanup, pair reseal, launch, pair, clean, restart or stop. The prior EOF repair, aggregate producer/cleanup budgets, fresh paired incarnation/issuer/bootstrap reseal, confirmed historical-stage handling, durable attempt ownership and unknown outcomes therefore remain intact. The worker diff also leaves watcher, retained proof/target validation, ledger and wire bodies unchanged apart from the reviewed boundary calls. No Owner HTTP, role, scope, input or capture path is relaxed.

## Remaining gates

Approve source integration of these exact three raw paths after the separate security verdict and staged-byte verification. This is not runtime proof of service identity, framework compatibility or successful invocation. The real normal source candidate is justified by locked winit; a strict runtime signature mismatch must be investigated rather than masked with undocumented bits or another visible-window exception.

Root must build exact source and retain staged hashes/input-off incarnation provenance, establish actual primary/closed-Portal state, and run repeat ordinary observation followed by separately controlled one-visible-Petal invocation with independent application-state checks. Preserve the approved negative matrix: malformed/read-failed geometry/style/class/thread/relationships/children, duplicate or changing service, eight-window bound, unsupported visible surfaces, initial absence then appearance, watcher/process/proof expiry and post-intent spent behavior. Production seams are present, but no controlled fact or native case was executed by this author or reviewer. No new generic action/test bypass is approved.

Actual Viewer/Editor/provider, restart/pruning/lifecycle/durability, native capture, FULL campaign, final integrated sweep, exact-SHA CI and tagged-release gates retain their existing pending status.
