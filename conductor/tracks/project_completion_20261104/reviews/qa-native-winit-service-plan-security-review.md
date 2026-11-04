---
type: Review
title: Strict winit service plan security review
date: 2026-11-04
---

# Winit service-window plan: independent security/design review

**APPROVED AT DESIGN SCOPE** for the strict service profile and retained-set plan in `.omc/research/qa44-visible-window-source-trace.md`, SHA-256 `7fb4b79a1cdd734a7336159936e76b7025d7f9203724828554a8676c564bf23b`. No blocking security/design defect was found in that complete proposal. This permits authoring the narrowly specified change for independent source review. It does not approve an implementation, relax the current worker, establish QA44's actual unexpected window identity, or grant native observation/Invoke/capture acceptance.

Only this review was authored. Read-only repository, installed primary package/SDK and hash inspection; no source/Git mutation, compiler/parser/formatter/tests/build, app/helper, OS/process/native/input/capture, API/DB/network action. Root's QA44 lifecycle and publication work were untouched. Report source baseline is `9d1e061d53d91db7680819609c8049129c708eee`; source review applies to that implementation context even if evidence bookkeeping commits advance HEAD.

## Primary source and diagnosis boundary

Locked `winit 0.30.13` constructs its separate event target during Windows EventLoop construction (`platform_impl/windows/event_loop.rs:187-217`). Its `create_event_target_window` at 910-961 registers exact public class `Winit Thread Event Target`, creates an ownerless zero-origin/zero-size top-level window, sets the four stated extended-style bits, then explicitly writes `WS_VISIBLE | WS_POPUP`. The adjacent primary-source rationale says this visibility supports resize WM_PAINT without displayed layered content. The retained event-loop thread ID and subsequent Bevy `event_loop.create_window` path support the proposed same-thread check. The ordinary FE primary class override remains `fractalengine`; this trace does not justify changing that root class.

The installed primary hashes independently match the proposal:

| Primary file | SHA-256 |
| --- | --- |
| `winit-0.30.13/src/platform_impl/windows/event_loop.rs` | `f99c748e189ea8ce4bde08ec186ee27306756ce93d5235371e198c07e582c52d` |
| `bevy_winit-0.18.1/src/winit_windows.rs` | `e25dd4e5cea949cf005dbf492cfeba3516474c63f0f2f270b0ab26b04ca57c24` |
| `wry-0.54.4/src/webview2/mod.rs` | `2551394dccec7f0f45be580e011a6ca82ad5e2dc8585749decf79b958fcfd38b` |

Installed SDK 22621 `WinUser.h:2790,2793,2840,2843,2869,2890` confirms style sums `0x90000000` and `0x080800a0`. The plan correctly normalizes the defined 32-bit style representation rather than treating pointer-width sign extension as extra flags. It permits no undocumented extra-bit mask or `WS_CLIPSIBLINGS` allowance. Exact runtime mismatch must refuse until separately evidenced/reviewed; source intent alone is not proof of actual observed styles.

The Portal alternative remains material: `fe-webview/src/backends/win32_popup.rs` creates a visible owned `FE_Portal` popup for initialization, and `tauri.rs` hides it only after successful child webview construction. An actually visible Portal or other unsupported surface still refuses. The reported QA44 categorical result proves a visible non-app-class FE-owned window was encountered; neither this source trace nor this review determines which class/HWND caused it. No actual window census was performed here. Do not publish a resolved live root cause from this plan.

## Essential admission constraints

Approval is for the entire predicate, not a class exemption: exact bounded public class; valid original FE process/birth context; ownerless top-level with no parent; nonzero thread equal to the original root's thread; successful exact `(0,0,0,0)` window/client rectangles; exact normalized styles; no native descendants under the documented bounded sampled census; at most one service; and all existing root checks. Same-class malformed windows refuse even when hidden. Unsupported visible windows, duplicates, failed reads or inconsistent relationships stay refused.

The service still counts toward the original eight-owned-window limit. Initial discovery remains one bounded census, admitting exactly one original `fractalengine` root and zero or one exact service. It must not select a replacement root or treat a service as an actionable/native child/provider target. Merely matching a public class establishes no authority. The profile describes a zero-area visual housekeeping surface; it does not claim the target receives no WM_INPUT/WM_PAINT or other event-loop messages.

The implementation must check cross-window thread/profile relationships after facts are collected, independent of EnumWindows visit order, while retaining the same root and optional service identities. Missing facts remain unavailable. No raw class/title/handle/count, user data or exception details may enter output. No hide/focus/activate/message injection, reconfiguration or framework-window destruction is authorized by this approval.

## Retained lifetime and boundary coverage

Retain the original root HWND/thread and optional original service HWND/thread/profile. Initial absence is permitted but immutable for the worker: later appearance, disappearance, replacement, duplicate or changed profile refuses; there is no discovery refresh or signature repair. The watcher still starts before initial discovery. Its process-scoped create/destroy refusal, barriers, event cap and heartbeat remain, but do not prove that an existing window's style, size, visibility or child state stayed unchanged.

The proposal therefore correctly requires a repeated complete bounded census and full profile validation at every existing native-window boundary, retaining exact root/service identities, with watcher/process/deadline checks around it and root revalidation afterward. Coverage must include initial provider waits, ordinary observe completion, target acquisition/preflight, both sides of durable intent, final pre-Invoke and post-return reporting. The current observe tail ends with watcher/process/deadline checks only; the implementation must explicitly add the retained-set check there rather than merely replacing the existing `window_current` call sites.

All work remains within the original five-second worker deadline and original one-second private host target proof. No source/window/profile check renews a proof, reselects COM elements, changes native aliases, bypasses actual typed callback guards, or provides role/grant authority. Original COM target/pattern and same-worker ownership remain. Any refusal after reservation/intent leaves the durable attempt spent, and late/unknown outcomes cannot be replayed. Post-Invoke inspection cannot create GUI/DB/navigation acknowledgement.

Repeated enumeration is sampled and non-atomic. The proposal explicitly retains interstitial change/HWND reuse and same-privilege tampering limitations; this approval does not promote it to a lossless lifetime or security-origin proof. Unknown census/progress/lifetime remains unavailable. Existing Owner HTTP gates and native profile floors remain unchanged.

## Diagnostics and implementation review requirements

The optional first diagnostic step may compare the existing buffer to fixed known public classes and emit fixed categories while still refusing; no raw data or positive exemption is needed. Source proof of a normal framework cause is sufficient for this bounded design approval, but actual QA44 class identity remains unproved without independent live evidence.

If authoring chooses the optional positive `observed_winit_service_v1` code, its exact semantics need independent source review across **worker `wmain` success/exit classification**, controller code allowlist and outcome handling, lifecycle receipts and CLI output. The current worker recognizes exact `observed` in its success-exit condition. Adding a controller string alone is not a complete protocol change. Alternatively retain `observed` and explicitly withhold any claim that output independently distinguishes service presence from absence. Neither choice grants action acknowledgement or capture credit.

The eventual source review must inspect every production call site and actual fact-extraction/classification seams, not only a test mirror. Controlled malformed fact coverage must retain failed reads, exact style/geometry/thread/relationship constraints, duplicates, late service changes and census overflow. Any live mutation fixture requires its own reviewed/root-owned scope; this plan authorizes no arbitrary OS manipulation or generic helper action channel.

## Remaining evidence gates

Before acceptance, root must build exact reviewed source, retain staged app/helper hashes and fresh input-off process/run ownership, establish ordinary observer behavior, and independently inspect the primary/closed-Portal state. If a positive profile-specific result is used, it proves the compiled predicate matched, not arbitrary same-class safety or downstream consumption. Real normal observation and separately authorized one-visible-Petal invocation still require original root/UIA/private-host proof and outcome-unknown semantics.

The proposed refusal matrix is appropriate but remains unexecuted. It must cover nonzero/moved/bad geometry, bad/unreadable class/styles, foreign process/thread, owner/parent/children, duplicates, unsupported visible Portal/dialog/default windows, the eight-window cap, retained-set changes, watcher/context/proof expiry and post-intent refusal without replay. No source-authored table earns runtime coverage. Viewer/Editor/provider, restart/pruning/lifecycle, capture, full campaign, CI and tagged-release acceptance retain their existing pending gates.
