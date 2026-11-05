---
type: Plan
title: QA53 exact service geometry profile plan
date: 2026-11-04
---

# QA53: exact observed service geometry plan

Investigation/design only; independent policy review before code. Shared UI
guidance, local native AGENTS and frozen/current worker/controller were read.
Current source hashes remain worker17bfd627994e541bcae0f13d0f08dec4761698d539d838b13f9576452b1f539f,
controller462504abcaafee308ca13802c9fdb454bf0b18586f55d1b0110faaf8fa9ee9c8.

## Actual facts, bounded explanation

QA53 `qa/passes/53/owner-native-observe-signature-facts.jsonl`,894 bytes,
SHA-256 `7487e2a56ce79db0976bf681dee84203785a83d1376f74cb89d9a98ea681624a`,
reports style0x94000000, exstyle0x080800a0, outer(0,0,16,16), client(0,0,0,0),
schemafe-native-service-signature-v1 and outer_rect refusal/exit2. No Invoke,
return, capture or positive UIA result. Rectangles are sequential samples with
different coordinate conventions, not an atomic/nonclient-border measurement.
Children and all later admission remain unproved.

Locked winit0.30.13 windows/event_loop.rs198-207 performs DPI-aware setup, when
enabled (the default), before creating the target. At904-961 it registers its class with
thread_event_target_callback, creates WS_OVERLAPPED at0,0 with requested0x0 size,
then directly overwrites GWL_STYLE with WS_VISIBLE|WS_POPUP. windows/mod.rs170-173
calls SetWindowLongPtrW on x64; no resize/SetWindowPos/frame recalculation follows
in that creation routine. The class uses CS_HREDRAW|CS_VREDRAW, no background
brush/menu/cursor and the exact already-required extended styles.

The **service** callback2475-2547 delegates initial messages (including default
WM_GETMINMAXINFO) and otherwise unhandled messages to DefWindowProcW. It has no
custom WM_NCCALCSIZE, WM_DPICHANGED or style/resize handling. Those branches at
1162/2190/2216 and SetWindowPos2409 belong to public_window_callback_inner1116,
not the service callback. The event target's WM_PAINT/raw-input responsibilities
remain; zero client area is not a claim that it cannot receive input/messages.

Installed SDK22621 WinUser.h4873 describes SWP_FRAMECHANGED as triggering
WM_NCCALCSIZE;8985/8998 declare the two rectangle APIs. Its metrics/style constants
alone do not prove a16-pixel extent. The previously captured official
[SetWindowLongPtrW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowlongptrw)
documentation (`.omc/research/qa50-microsoft-window-style-primary.txt:394`) explains
that some changed window data is cached until SetWindowPos. A retained nonclient
frame after creation/style change is therefore a **plausible mechanism**, not an
established cause or a rule for all Win32/winit/DPI configurations. Do not infer
eight pixels on each side, exact monitor DPI, invisible-border metrics or client
screen origin from these facts. Worker PMv2 checks remain, without new DPI reads.
Primary locked event_loop.rs SHA-f99c748e189ea8ce4bde08ec186ee27306756ce93d5235371e198c07e582c52d;
mod.rs SHA-c739237f4903d6be9aaa3e913e50ae01575a689a4678486a3b82e214adf7020d.

## Proposed minimum reviewed policy

Require **one exact observed profile**: outer.left=0,top=0,right=16,bottom=16;
client all-zero; style0x94000000/exstyle0x080800a0. Replace the existing outer-zero
comparison with that full exact rectangle only. No zero-or16 alternative, maximum
size tolerance, DPI-derived range, ignored coordinates or adopted unknown window.
Other environments/changed metrics remain unavailable until separately reviewed.

Preserve exact class/process/root thread, ownerless top-level/no parent, visible
state, <=8 complete census, <=1 service, exactly one original primary, no native
children, immutable optional presence/HWND/thread and full use-boundary rechecks.
Zero client extent remains mandatory, and the service never becomes a UIA/action
target. Public metadata stays forgeable within the existing equal-privilege model;
no authority is inferred from the small outer rectangle. No new native query,
upstream/window mutation, root/profile adoption, proof/deadline/Owner-floor,
watcher/COM/ledger/Invoke/capture change.

## Diagnostic version and implementation scope

Four prospective paths only: native_observer worker.cpp/controller.py/scenario.py/
AGENTS.md. New worker emits **fe-native-service-signature-v2** for the revised
classifier. The fixed eleven-field shape/types/ranges and raw observe/flags/HR0/
confirmed-exit2 gates remain. Controller preserves v1 first-failure semantics
(outer all-zero) for older stages, and dispatches v2 to exact(0,0,16,16); both keep
the same exact style/exstyle/client checks. No reinterpretation of QA53 or v1
history, schema-free fallback or arbitrary versions. Legacy scalar/absence rules
remain. Old controllers reject v2; immutable stage/helper pinning is unchanged.

Review exact classifier/diagnostic synchronization and negatives: zero outer,
15/17 extent, nonzero/negative origin, asymmetric16x17, every nonzero client
coordinate, changed styles, child/owner/foreign thread, extra/changed windows and
expired proofs still refuse. Extend only existing exercise with v1/v2 same-sample
different-first-failure cases, all-pass diagnostic rejection, malformed/version/
range/context cases; no new test registration. Root independently builds and
obtains fresh exact-profile/children/provider evidence. A later refusal remains
a refusal requiring evidence, not automatic constraint removal.

Only this report was written. No implementation/parser/compiler/tests/build,
app/helper/OS/native/API/DB/network action or Git mutation. No positive service,
UIA/Invoke/lifecycle/capture/FULL/CI/release acceptance is supplied.
