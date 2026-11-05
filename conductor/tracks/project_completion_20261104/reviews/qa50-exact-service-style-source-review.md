---
type: Review
title: Observed exact service style source review
date: 2026-11-04
---

# QA50 exact service style: independent source review

**APPROVED AT SOURCE** for the exact two-file slice against `71cd5f8e9f05bd39dbb1a61c17a95ca054dda5fe`, implementing approved plan `59d5ba166d3e7acc677bcc9f0ec420f106daed2840686420fa627c06a989fc91`. No source blocker was found. Root may commit/restage and collect fresh evidence. This provides no build, service admission, downstream UIA/Invoke or actual causality acceptance.

Manifest **1204 bytes**, SHA-256 **`2672f2bab76d9c21017ceb40b3cd0c36d5b7f229de6389619ea32a9aa38ed955`**; full diff **3005 bytes**, SHA-256 **`d6569a17f091ac1bcc3f0154946b437b2a047e823803b74503c0b5fefbfb0c4f`**. Both frozen raw files match declared lengths/hashes and current working bytes. Baseline source contents still match current HEAD. Author report matches requested SHA-256 `f55ae90dfb283469a9823326f1187c5ff61aacfdeebc6fa89595e52e7c9c2723`.

| File under `tools/qa/native_observer/` | Bytes | Reviewed SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 45651 | `719558c5a52d57ad16ce065ee11cad920c7de0b3bd251283655fedf384f1ba21` |
| `AGENTS.md` | 29880 | `b7e509200941fe10363e37ca4c103df8bf837af0bca7acaab329e20ec948c59d` |

The sole C++ expression adds `WS_CLIPSIBLINGS` to the expected full style. Independently reversing that one occurrence reproduces the entire baseline worker byte for byte, SHA-256 `8c7f7bffbcc011aa15c7fc2bfb77090b4067d6af32e1222aa4b5c999d31c27bc`. Its direct inequality remains: only exact `0x94000000` passes this comparison. Old `0x90000000`, any required-bit removal or extra bit fails. There is no ignored-bit mask, alternative value, tolerance or new read/action. This conclusion is source arithmetic, not executed truth-table coverage.

Every other worker byte is unchanged: exact exstyle, zero origin/size rectangles, class/PID/root-thread/owner/parent/child constraints, counts, original optional retained set, watcher/rechecks, failed-value diagnostic gates, immutable proof/deadline, original COM/target, intent/spent ownership, Invoke and outcome rules. Controller, request/stage/ledger schemas, pinning and success/exit semantics have no delta. Concurrent camera Rust authoring and evidence publication are excluded.

Directory documentation accurately requires the full observed value, identifies the SDK painting/clipping bit, and preserves unknown precise added-bit cause, non-universal OS behavior and sampled trust limits. QA50's failed style sample remains historical failure; it proves none of the later exstyle/rectangle/child or provider predicates. No old stage bytes are changed or relabeled. Another component refusal must remain refused and receive its own investigation rather than speculative relaxation.

Only this review was authored. Read-only source/Git/hash inspection; no source/index/history mutation, implementation/parser/compiler/formatter/tests/build, apps/helpers, OS/process/native/input/capture, API/DB/network action. Root owns exact commit/build, meaningful controlled negatives and fresh process/controller evidence. Positive service/UIA/navigation/Invoke, capture, lifecycle, FULL campaign, final sweep, CI and release gates remain separate.
