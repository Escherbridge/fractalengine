---
type: Review
title: Strict native service security source review
date: 2026-11-04
---

# Strict winit service v1: independent security/correctness source review

**APPROVED AT SOURCE** for the complete three-path frozen slice `.omc/research/qa-native-winit-service-source-v1/`, baseline `a77cabbee465a690749eba684d8299e51c716b82`. No concrete source blocker was found. The implementation follows the approved strict signature/retained-set plan without a generic visible-window exemption, new authority or action path. Root may stage/commit/build after the other independent review; compiled/live acceptance remains required.

Manifest **1054 bytes**, SHA-256 **`4892a4f9306c1452d1302bc3303662d68f0e48866cd05c32ad95e1439bd55bbe`**; full diff **16860 bytes**, SHA-256 **`bd09b6991afe4808592c4a167e011f3b1f065f6756ecc4ee90063601ad553092`**. All frozen files match their byte counts/hashes and actual working bytes. Their HEAD baseline contents equal a77 after newline normalization; intervening publication has not changed these source bases. Author report and exact frozen copy match SHA-256 `f4ac2b0bdc04e1940459b3e06ed97f2a8c9b96a15f06230d5d65db7eed1dce79`. Approved design identity remains `7fb4b79a1cdd734a7336159936e76b7025d7f9203724828554a8676c564bf23b`.

| Exact file under `tools/qa/native_observer/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 44445 | `da27fe8405ec5e5bd8b78b4f002fc53fc62fde4d79e97d776cf3e549e8a982ee` |
| `controller.py` | 16097 | `0d4c87f67f19d7a21d2b9136b54ab2fd66b3751372812ce225f43f3463e17ec0` |
| `AGENTS.md` | 26705 | `ab56fc920dcb95174cf894028ceb3e1a5b9e8eea56f81788b3b54b33590e8549` |

Only this review was authored. Read-only frozen/source/Git/installed primary SDK/hash inspection; no source/index/history mutation, implementation/parser/compiler/formatter/tests/build, helper/app, OS/process/native/input/capture, API/DB/network action. Concurrent recovery/UI/DB authors and root evidence/lifecycle work were preserved. This approval covers no other pending path.

## Actual admission and retained identity

`worker.cpp:234-255` is the production signature/fact path. Both native rectangle reads must succeed; all four coordinates of each must equal zero, including origin. Normalized DWORD styles must equal exactly `0x90000000` and `0x080800a0`; no additional-bit mask, tolerance or offscreen allowance exists. A failed style read returning zero cannot match either nonzero required signature. Valid HWND, exact bounded class, visible state, original FE PID, nonzero same-root thread, ownerless top-level/no parent and no native descendants are all required. A same-class hidden/malformed service goes through this predicate and refuses rather than falling through as an ordinary hidden window.

`inspect_windows` at 261-312 retains the eight-entry aggregate cap: every FE-owned top-level window, including service and ordinary hidden windows, consumes a slot before insertion. Ninth entry, failed thread/class read, class truncation, callback refusal or incomplete enumeration refuses. It requires exactly one primary and at most one service, then checks their relationship after collection, independent of visit order. Unsupported visible Portal/dialog/default/other classes still refuse. Ordinary hidden windows retain their prior policy and are revisited by subsequent census. An unreadable foreign-window thread now conservatively refuses the census; it introduces no admission bypass.

Original primary visibility/minimize/PID/class/owner/tool/child/cloak/Default-desktop checks remain intact. `no_native_children` extracts the same root child-refusal body and applies it to service too; the documented sampled EnumChildWindows limitation remains. The service never receives UIA lookup, target selection, focus/message/window mutation or action. Fixed public class metadata grants no role/source authority.

`WindowSet::current` at 315-323 validates a fresh full census, then compares root/service HWNDs and thread IDs against the original `const WindowSet`. Initial absence is immutable too. Replacement/disappearance/appearance, duplicates, changed profile or wrong relationship refuses; no new result is assigned into retained state. Rechecking a temporary candidate's native facts before comparison does not adopt it or perform UIA/action work. Original root identity/thread is checked again afterward.

## Real production boundaries and authority preservation

The watcher is constructed before initial census. Its existing process-scoped create/destroy refusal, cap/barrier/heartbeat and cleanup are unchanged. The shared boundary brackets retained-set validation with watcher/process/original Deadline checks. It runs before COM/provider activation and each provider wait, around target acquisition/preflight and both sides of durable intent.

The two formerly important tail gaps are explicitly covered: final observe runs `boundary()` after latest source/hash/property work and immediately before emit; final preflight runs it after all original-target/property reads and returns locally, then the post-intent preflight is followed only by local receipt bookkeeping and the original pattern Invoke. Post-return checks bracket original target/source reads as well. Thus adding the full census did not leave only the old early boundary sites in place.

Original single `ElementFromHandle`, retained outer/marked COM runtime/ancestry, selected original target/pattern and one Invoke are preserved. No provider or target replacement/retry exists. Rolling root pass/tick validation follows the original approved same-source contract; this delta does not renew the host's immutable one-second target proof, aliases, role/scope/epoch predicates or typed callback authority. The absolute five-second worker Deadline is reused, not rebound. Extra native checks may consume that budget and must fail unavailable rather than extend it.

Durable controller reservation and worker intent ownership are unchanged. Refusal/timeout after intent remains spent; uncertain operation is not replayed. Post-Invoke S_OK still reports submission/outcome unknown; a changed post-state uses existing submitted-state-changed semantics and is not a consumption, database or render acknowledgement. Same-worker COM, worker cleanup/retention, executable/process pins, wire bounds and Owner HTTP/native floors have no delta.

## Output, limitations and execution gates

Controller changes are exactly six additive fixed refusal codes; legacy codes and strict shape/outcome checks remain. Worker `observed` and its `wmain` success-exit classification are unchanged, avoiding a partial positive-code protocol change. No service presence, handle, raw class/title/name/count or provider exception is emitted. Consequently **`observed` alone cannot distinguish service present from absent** and cannot prove QA44 matched this profile.

The installed locked winit constructor and SDK constants reviewed for the design justify this exact proposed framework profile. Actual QA44 unexpected class/profile remains unobserved; strict runtime mismatch must be investigated, never solved by speculative extra style masks. Enumeration, native reads and watcher events remain sampled/non-atomic, not lossless HWND birth/interstitial-state proof. Same-privilege tampering is not solved. The documentation accurately preserves these limits and calls for separate acceptance.

No new test/command/mode was authored, and controlled malformed-fact or actual native checks were not executed in this slice. The production signature and retained-set seams must receive meaningful coverage for failed/moved/nonzero rectangles; bad/missing/extra style/class reads; PID/thread/owner/parent/children; duplicates/cap overflow; retained presence/signature changes; visible Portal/unknown surfaces; watcher/proof expiry; and post-intent refusal without replay. Root owns compiled fresh disposable input-off observation, exact staged hashes, independent primary/closed-Portal inspection and separately authorized one-visible-Petal invocation. Source approval alone grants no service-presence, positive UIA/provider/navigation, Viewer/Editor, restart/pruning/lifecycle/durability, capture, FULL, final sweep, CI or release acceptance.
