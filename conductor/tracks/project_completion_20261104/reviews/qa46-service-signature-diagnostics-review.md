---
type: Review
title: Exact native service signature diagnostic source review
date: 2026-11-04
---

# QA46 signature diagnostics: independent exact source review

**APPROVED AT SOURCE** for the three-path diagnostic-only delta against `be37b2dc260b06b42307a9cab3a49f08202c108c`. No correctness/security blocker was found. Root may commit/build and collect a new matching observation; this review does not execute or relabel QA46 and grants no positive service/native acceptance.

Frozen manifest SHA-256 **`e9449cbb822358b973d144ba885c271039f760d3fcc723a8e7ccafdb6867aac8`** (1057 bytes); complete source diff SHA-256 **`5ab012de1f7156e339e7d0a7f012523334d44a0aa98378d3739c65ac7f19131c`** (4972 bytes). All three raw snapshot files match the manifest and current working bytes. Author report and frozen report copy are exact, SHA-256 `8ca68640ea3127204189836c6bf839d544c3f3f2e7ac7aaf78a7579c73b8c1ce`.

| File under `tools/qa/native_observer/` | Bytes | Reviewed SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 44677 | `1df716827ceb97df0a0803f66d8691be6f6b9ea6d98f6abc08abbc2e2c0941a6` |
| `controller.py` | 16226 | `17adfce21cfc4e0a891042c8a5ec2143900d364192f63d4c4f575d738717414c` |
| `AGENTS.md` | 27355 | `7b389bdece379b0b082f407ec16ba43cbea5c144f80320e9b54402766f65547c` |

`worker.cpp:234-257` changes the pure boolean signature to a static reason pointer, null iff the same complete predicate passes. It checks exact normal style, exact extended style, zero outer rectangle and zero client rectangle in the original conjunction order. Each zero check still tests all four coordinates. Expected masks, DWORD normalization and exact comparisons are unchanged; no mask relaxation, tolerance or new native read exists. Both style arguments are still read at the same function-call expression, with the same C++ argument-evaluation rules as before; the diagnostic classification itself introduces no read sequencing change.

The only production caller throws the fixed reason when nonnull and reaches the unchanged native-child check only when all original signature predicates pass. Both rectangle-read success checks and preceding process/thread/class/owner/parent/visibility admission remain untouched. The original retained window set, census cap, watcher, all operation boundaries, proof/deadline, COM objects/target, durable intent, Invoke and outcome handling have no delta. Search confirms the renamed helper has exactly its definition and one production call, with no stale caller.

Four controller strings are additive: `window_service_style_refused`, `window_service_exstyle_refused`, `window_service_outer_rect_refused`, `window_service_client_rect_refused`. Legacy `window_service_signature_refused` remains. All reasons are static literals; no flags, coordinates, titles/classes, handles, counts, fields or exception text are exposed. Existing output shape, `observed` success/exit, no-token/no-HTTP native boundary, attempt-spent and unknown-outcome semantics remain unchanged. The directory guide accurately describes first-failure categories and withholds retrospective diagnosis.

Scoped whitespace check is clean apart from prospective Git LF/CRLF notices. Only this review was authored; source/Git/hash inspection was read-only. No source/index/history mutation, implementation/parser/compiler/formatter/tests/build, app/helper, OS/process/native/input/capture, API/DB/network action occurred. Concurrent asset-author publisher work, root evidence and session log were preserved and excluded.

The prior combined refusal proves none of the four particular components or raw values. A future fixed code identifies a failed component only; it does not justify admitting extra bits, relaxing geometry or claiming a service-profile match. Actual build, matching fresh observation, malformed-fact coverage and positive provider/Invoke/lifecycle/capture/campaign/CI/release gates remain root-owned pending work.
