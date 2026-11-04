---
type: Review
title: Native window discovery categorical refusal diagnostics
date: 2026-11-04
status: approved
---

# Native window refusal diagnostics: independent source review

**APPROVED** for the exact three-path diagnostic delta against HEAD `ff7bc1d4938fdf501a115b5a65a0dd9ed6ce766f`. The six fixed categories refine the existing refusal output without admitting any previously refused window, adding discovery, or changing invocation/authorization gates. No concrete source blocker was found. Root may commit and build; this review provides no compilation or runtime acceptance credit and does not diagnose QA43's particular `window_ambiguous` result.

Only this review was authored. Read-only source/Git/hash inspection; no source/index/history mutation, compiler/parser/formatter/test, build, helper/app, process/OS/native API, input/capture, API/DB/network action. Root retains all QA43 process/evidence ownership. The preexisting session log is excluded.

## Exact scope and hashes

The complete scoped `git diff --binary HEAD` is **4938 bytes**, SHA-256 `50bbf71854bfe602ef501805c00831beca739f502a76ad037227e757687f9156`. It changes only `discover_window`, adds six strings to the controller's exact `WORKER_CODES` set, and documents that behavior in the directory guide. Other pending tracked changes are limited to the preexisting session log. Scoped whitespace checking passes; Git's prospective LF/CRLF notices do not change this reviewed source.

| Working path under `tools/qa/native_observer/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `worker.cpp` | 40988 | `dab1b189cf55a87491b3f17af07d81160353e5debfbf5cd37d1bb35f915dd043` |
| `controller.py` | 15924 | `cad569ed06b08c4baa72d68ce54fb43cc8abb0f2df4c172c83b72c12ba0eca89` |
| `AGENTS.md` | 23114 | `18bca6742417cf05c67579db8309ddeaae95374628854e0ea164b65cf1f99322` |

The corresponding HEAD raw hashes are worker `cb11508933e81da8a90fb49ba69a91152b7bbd94bc855909d2cbb34b59a39c72` (40808 bytes), controller `b9b0a1a85274870362fc4792f8b80718e3236196ed6271f349725d216e6ccd4d` (15767 bytes), guide `f96e17c31156d21e687b8d975d3fe02e02ceb198e50d3a7b99f68de78dfb6d8e` (22126 bytes). Author report `.omc/research/qa-native-window-refusal-diagnostics-report.md`: 4208 bytes, SHA-256 `27b1ba5d3023aa46b4f8e6b5c6ea19634c3d4e9c960a2a569bfd1fcd6f23744e`.

## Discovery predicates and ordering

At `worker.cpp:230-255`, the old admission conjunction was completed enumeration, no callback refusal, exactly one exact-class candidate, then `window_current`. The new code preserves that conjunction: a null refusal pointer represents the old false boolean. The candidate array remains eight entries, and the ninth owned window refuses before class/candidate access. PID filtering, the existing class query, exact class comparison, visibility query, candidate insertion, and callback return points remain in their original order.

The visible non-app-class branch still records refusal and returns TRUE, allowing the same continued enumeration as before. Its removed owner/tool-window queries were redundant: both original branches unconditionally set the same refusal. Removing those queries neither changes admission nor adds an OS discovery operation. Count and class-query failures still stop enumeration immediately. The setter preserves the first callback reason, even if continued enumeration later encounters another refusal.

The branch categories are fixed literals: `window_owned_count_refused`, `window_class_query_refused`, `window_unexpected_visible_refused`, `window_enumeration_refused`, `window_root_missing`, `window_roots_multiple`. Callback refusal is checked before the EnumWindows completion flag, so deliberate callback stops retain their cause. Root-count classifications occur only after otherwise unrefused complete enumeration. No incomplete scan can become a successful or missing-root result. The original `window_current` identity/visibility/owner/cloak/desktop/child checks remain untouched.

At `worker.cpp:608-620`, the watcher is still constructed before this one discovery operation; watcher/process/deadline checks still follow it, and the original one `ElementFromHandle` remains later. No callback, proof, retained COM identity, intent, one-attempt, Invoke, cleanup or receipt-state path was changed.

## Bounded output and compatibility

All six codes are static literals carried through the existing `Refusal`/receipt code field. No title, class text, HWND, count, provider detail or newly collected data enters output. The controller retains its existing exact output-key, code-membership, boolean, watcher-order and invocation-consistency validation. Its only delta accepts these six strings; old `window_ambiguous` remains accepted for older staged workers. Outcome classification, attempt-spent handling, invoke-entered/method-returned semantics and capture availability remain unchanged.

QA43's reported pre-Invoke observation and Owner-state `403 qa_gate_refused` are parent-owned live evidence, not independently executed here. This diagnostic change does not relax Owner HTTP or replace that refusal with native authority. It does not establish which discovery branch caused the earlier generic code.

## Approval boundary

Approval covers only the three exact working files above, plus an optional exact-body OKF publication copy of this review. Root owns source commit/build, matching-stage observation and evidence/Conductor updates. Runtime transport/ownership, native delivery, authorization, rendering, final campaign/CI/release gates remain pending according to their existing records; a categorical refusal alone earns no Invoke or capture credit.
