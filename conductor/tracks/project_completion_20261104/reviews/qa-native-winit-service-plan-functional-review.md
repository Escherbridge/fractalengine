---
type: Review
title: Strict winit service plan functional review
date: 2026-11-04
---

# Strict winit service profile: independent functional/design plan review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**APPROVE the proposed strict service-profile design for source authoring, with the implementation/acceptance requirements below.** This approves the exact plan in `.omc/research/qa44-visible-window-source-trace.md`, SHA-256 `7fb4b79a1cdd734a7336159936e76b7025d7f9203724828554a8676c564bf23b`; it does not approve implementation or identify QA44's actual unexpected HWND.

Only this report was authored. Inspection used read-only repository/installed primary-source/hash data. No code edit, compiler/test/parser, application/helper, OS/process/native/API/DB/network action or Git mutation occurred. Other agents' work and root's live application were untouched.

## Why this is a useful bounded compatibility correction

Independently inspected the locked winit 0.30.13 `platform_impl/windows/event_loop.rs:904–962`. The thread event target uses the exact proposed class, zero origin/size, null parent and the four proposed extended-style bits. It then explicitly sets ordinary style to `WS_VISIBLE | WS_POPUP`; the adjacent primary-source explanation says the layered window is not displayed to the user. The installed file hash matches the plan's `f99c748e189ea8ce4bde08ec186ee27306756ce93d5235371e198c07e582c52d`. Bevy's actual name-to-class forwarding at `bevy_winit-0.18.1/src/winit_windows.rs:244–248` also supports retaining the separate `fractalengine` root class.

The present worker rejects every visible owned non-root class at `worker.cpp:246`. Therefore a normal source-reachable framework service can prevent observation even with the correct primary window. A zero-area visual-service profile is a meaningful compatibility path. It does not justify admitting a visible Portal, dialog, generic tool window, arbitrary same-class window or another event-loop service.

QA44's fixed refusal establishes only that the current worker saw an unexpected visible owned window. Its actual class was not observed. Successful source support for winit must not be represented as proof that this was QA44's HWND, or broadened if the real process subsequently fails a different predicate.

## Required implementation shape

The proposed profile is sufficiently narrow: exact bounded class; same retained process incarnation and original root thread; ownerless top-level relationship; exact zero window/client rectangles including origin; exact normalized 32-bit styles; no native descendants; at most one service; and the existing aggregate eight-owned-window bound. Malformed same-class services refuse even when hidden. No UIA lookup, action, title/user-data read, message, hide/focus/style mutation or framework reconfiguration is needed.

Use one small retained window-set value and one shared validation path. Discovery should collect the unique primary and optional service, then validate their relationship after the original root thread is known. Later census is validation of that original set, never selection of a replacement. Initial service absence must remain absence throughout the worker; initial presence must retain that exact HWND/thread and complete signature. Missing/unknown reads and partial/over-bound census results refuse. Existing hidden-window policy remains subject to later full census if visibility changes.

The watcher still begins before discovery. Its create/destroy hooks and heartbeat are necessary but do not cover all style/rectangle changes, so repeated complete set validation is required. Preserve the acknowledged sampled-enumeration/handle-reuse limitations; neither the public class nor matching HWND alone establishes unforgeable birth identity.

I traced current use boundaries in `worker.cpp:603–695`: initial discovery, provider activation loop at 627, shared boundary at 650–652, observe completion, target selection/preflight, both sides of durable intent, final pre-Invoke and post-return reporting. The plan correctly requires replacing every relevant window check with original-set validation and adding the otherwise missing final observe validation immediately before positive output at 659–660. The final pre-Invoke check must follow all intervening target/property work; only changing the existing early `boundary()` call is insufficient if later reads leave a gap in the stated contract. Preserve process/watcher/deadline checks around census and recheck the original primary afterward, within the original absolute five-second deadline.

Retain the same original UIA root/marked element/target/InvokePattern. No second `ElementFromHandle`, selector rediscovery, target replacement, native-set adoption, deadline restart or host-proof renewal is authorized. Existing immutable one-second host proof and source/context/epoch callbacks remain authoritative. Failure after intent stays spent; Invoke return remains submitted/outcome unknown.

## Output and evidence requirements

A fixed positive observe-only code for the verified service path is reasonable and avoids publishing names/handles. If added, it must be wired consistently through the worker's final exit-success predicate (`worker.cpp:734` currently recognizes only `observed`), controller allowlist/result interpretation and existing evidence journey. It must never become an invocation success/consumption acknowledgement. Preserve legacy codes and strict output shape. A code added only to the allowlist would leave valid service observations exiting 2 and would not implement the planned usable path.

Prefer fixed refusal categories and this one optional positive distinction over a generic window-dump interface. An optional diagnostic-only build may classify known public classes while still refusing; it grants no profile admission.

Acceptance must include a real input-off owned app/helper with exact staged hashes and incarnation, independently confirmed primary/closed-Portal conditions, repeated successful ordinary observation, then separately controlled one-visible-Petal invocation with unchanged authority/outcome semantics. A service-present positive result should identify the compiled predicate path, not certify arbitrary same-class safety. Test the actual production classification/retained-set seams for malformed geometry/styles/relationships/threads/children, duplicates and changed/appearing/disappearing service; retain visible Portal/default-class refusal, census bounds, watcher gaps, process changes, expiry and post-intent spent outcomes. Controlled facts are not executed native coverage, and this plan does not authorize arbitrary live window mutation or new generic test commands.

No plan-level functional/design blocker remains. Full independent source review must check the final call graph, refusal/output plumbing and exact identity/timing preservation before integration. Compilation and actual native acceptance remain separate gates; if the strict real profile refuses, capture the fixed category and investigate rather than adding speculative style masks or visible-window exemptions.
