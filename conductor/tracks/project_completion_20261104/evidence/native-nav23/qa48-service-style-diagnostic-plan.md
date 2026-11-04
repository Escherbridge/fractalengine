---
type: Plan
title: Bounded service style diagnostic plan
date: 2026-11-04
---

# QA48: bounded service style diagnostic plan

Source-plan only, 2026-11-04; independent review required before production edits.
QA48's `qa/passes/48/owner-native-observe-component.jsonl` reports
`window_service_style_refused`, helper exit2, no Invoke. This identifies the failed
comparison, not its value or cause. Do not infer a particular extra bit.

## Proposed closed result extension

Keep `fe-native-worker-result-v1` and its exact existing base keys. Permit exactly
one optional additional key, `service_style_u32`, a JSON integer in
0..4294967295 inclusive. Reject bool, float, string, negative, overflow, null,
unknown keys and duplicate keys. No nested/free-form diagnostic payload or new CLI
option. The field identifies a public style bitset, never a window identity.

Emit it only for parsed **observe** requests whose terminal code is exactly
`window_service_style_refused`, with worker attempt_spent=false,
invoke_entered=false, method_returned=false and unchanged HRESULT zero. Controller
also requires the actual operation be observe and confirmed helper exit2. Never
emit/accept it for invoke mode, success, another refusal or malformed requests.
Its absence remains valid, including that refusal from an older sealed worker.
Controller's final ledger-derived attempt_spent may reflect an earlier invocation;
this diagnostic must not change that independent ownership result.

## Capture and failure propagation

`worker.cpp:245-255` already verifies service class/process/thread, ownerless
top-level identity, visibility and successful rectangle reads, then reads style
and extended style once. Name those existing normalized DWORD values locally and
compare the same values in the same strict order. On the style mismatch only,
throw a small local typed refusal carrying that one DWORD and the fixed existing
code. A dedicated catch before generic Refusal stores the optional value only
when the parsed request was observe. Use an explicit parsed-observe flag defaulting
false; do not infer mode from an unparsed request or spent-state replacement.
Generic Refusal catches, including post-Invoke handling, preserve their behavior.
Do not extend generic protocol Refusal into an arbitrary data channel.

This carries a failed-read sample to output without a second native call, globals,
sticky diagnostics or later rediscovery. `inspect_windows:271-316` already checks
the complete <=8 census, exactly one primary, <=1 service, original primary
validity and equal root/service thread before service validation. The diagnostic
does not assert remaining extended-style/rectangle/child predicates passed; the
rectangles were read but comparison stops at style. Later retained-set checks may
also fail here; the value is only that terminal mismatch sample, not a timeline.

## Minimal source scope and compatibility

- `worker.cpp`: local typed refusal/optional receipt field, existing style-read
  locals and conditional numeric emit. Preserve strict signature constants,
  refusal priority, output bound, watchdog, exit2 and all invocation/proof paths.
- `controller.py:20,166-179,236-256`: exact base-or-base-plus-one key validation;
  explicit actual-mode and helper-exit checks before returning the diagnostic.
  Keep bounded unique-key decoding and every existing output validation.
- `scenario.py::exercise`: extend the existing offline journey with production
  parser cases; no new registered test or native coverage claim.
- Native directory `AGENTS.md`: document field, mode, stage compatibility and limits.

No stage-schema/request/ledger/ownership/bootstrap changes. `build.ps1:35-45`
already seals changed worker source and EXE hashes; controller `helper_stage:64-77`
still requires pinned exact stage metadata/binary. New controller accepts old
sealed workers without the field; old strict controllers safely reject new
extended output. No published stage mutation, downgrade, alternate helper or
capability inferred from field presence. Output/request byte caps stay unchanged.

## Primary references and independent acceptance

Installed SDK22621 `um/WinUser.h:1907,2790-2805,9848-9852` defines GWL_STYLE,
named style bits and GetWindowLongPtrW's LONG_PTR return; `shared/minwindef.h:156`
defines DWORD. Preserve the existing DWORD normalization of this x64 return.
Locked winit0.30.13 `src/platform_impl/windows/event_loop.rs:939-961` requests
WS_OVERLAPPED then sets WS_VISIBLE|WS_POPUP. These sources explain the intended
0x90000000 comparison; they do not establish QA48's actual bitset.

Review requires exact sole-origin/mode/exit gating, unchanged strict predicates
and no extra native reads, strings, handles, titles, stderr, other-window values
or reusable selectors. Existing journey covers min/max, representative mismatch,
invalid types/ranges/keys, duplicate key, wrong mode/code/flags/exit, and legacy
absence. Root alone builds/runs later. Fresh evidence must retain the failed
code/exit and actual integer; interpret named/unknown bits against the SDK before
any separately reviewed policy change. No positive service/UIA/Invoke credit.

Only this plan was written; no production edits, implementation execution,
compiler/parser/tests, apps/OS/native/API/DB/network or Git mutation occurred.
