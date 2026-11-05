---
type: Plan
title: Observed exact service style revision plan
date: 2026-11-04
---

# QA50: exact observed service style revision plan

Source/SDK plan only, 2026-11-04; independent policy review before code.
Scope: the existing exact winit service, not a general visible-window exception.

## Observed value and primary interpretation

`qa/passes/50/owner-native-observe-style-value.jsonl` (626 bytes, SHA-256
`da3a033737406105a2e6aad571a75e83eb35cf934be79a4a62586f526f4bcc2f`)
reports style uint32 **2483027968 = 0x94000000**, code
window_service_style_refused, exit2, raw attempt_spent/Invoke/return=false,
HRESULT zero and no capture. XOR against required0x90000000 is exactly0x04000000.
Installed SDK22621 `um/WinUser.h:2790,2793,2795` defines WS_POPUP=0x80000000,
WS_VISIBLE=0x10000000 and **WS_CLIPSIBLINGS=0x04000000**. Header SHA-256:
`a0ef5ea3f5f2587aa6b84fb6c845b3ce866a0f09fa0a5898ed1b153072a09c0b`.

Microsoft's [Window Styles](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-styles)
documents WS_CLIPSIBLINGS as excluding overlapping sibling child-window regions
from a child window's paint region. It describes painting/clipping behavior;
it supplies no authority, target identity or positive interactivity guarantee.
The existing ownerless/no-child/zero-area restrictions remain independently required.

Locked winit0.30.13 `src/platform_impl/windows/event_loop.rs:939-961` creates its
zero-area service with WS_OVERLAPPED then requests GWL_STYLE=WS_VISIBLE|WS_POPUP.
`windows/mod.rs:170-173` directly calls SetWindowLongPtrW on x64. Its service
callback `event_loop.rs:2475-2547` contains no style-changing branch/bit addition;
unhandled messages go to DefWindowProcW. The source recipe and getter value differ.
The worker's existing DWORD cast normalizes the pointer-sized getter result to
32 bits; this is distinct from any OS modification of the requested style.

The consulted primary [CreateWindowExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createwindowexw)
and [SetWindowLongPtrW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowlongptrw)
pages do **not establish the particular added-bit cause**. The latter documents
cached window data, which also prevents treating a request/getter comparison as
an atomic history. No claim that all Win32 versions always add/preserve this bit.
This proposal is an explicit observed-profile correction, not a proven OS-internal
normalization rule. Raw official responses are preserved in
`.omc/research/qa50-microsoft-window-style-primary.json` and decoded `.txt`.

## Proposed minimum change

Only `tools/qa/native_observer/worker.cpp` and its directory AGENTS.md:
replace the pure classifier's expected_style expression with
`WS_VISIBLE | WS_POPUP | WS_CLIPSIBLINGS`, requiring **exact0x94000000**.
Keep direct full-value equality. Missing WS_CLIPSIBLINGS (including old0x90000000)
refuses; every additional or removed bit refuses. No allowed-extra-bit mask,
optional-bit alternative, tolerance, capability or compatibility fallback.
Older stages retain their original strict predicate; restaging is required.

Keep exact exstyle0x080800a0, zero origin/size outer+client rectangles, <=8 census,
<=1 service, exact public class, same-process/root-thread identity, ownerless
top-level/no-parent/no-native-child constraints, exactly one valid primary, retained
optional presence/HWND/thread, watcher and full-set rechecks unchanged. Keep
refusal order, optional bounded diagnostic, stage/controller/ledger/proofs/COM,
original one-second permission, final Invoke gates and outcome rules unchanged.

## Independent review and remaining acceptance

Review the actual one-constant delta and documentation, with strict truth-table
negatives:0x90000000, removed VISIBLE/POPUP, CHILD/DISABLED/caption/other added bits,
wrong exstyle, nonzero rectangles, child/owner/foreign thread, extra service/root
and changed retained presence still refuse. No test declaration/harness is proposed;
these are review criteria and controlled negative acceptance cases where the
existing owned fixture supports them, not executed assertions.

Root must build the independently reviewed source and observe a fresh exact
process/stage through the existing controller. QA50's earlier style refusal proves
neither later exstyle/zero-rectangle/child acceptance nor UIA/provider/navigation/
Invoke delivery. A subsequent component failure remains a refusal requiring its
own evidence. Public class/style identity remains sampled and forgeable by an
equally privileged process; the full reviewed trust/proof model still applies.

Only plan/research captures written; no production edit, test/compiler/parser,
helper/app/OS/native/API/DB action or Git mutation occurred. Microsoft Learn reads
were expressly authorized source research, not application/network acceptance.
