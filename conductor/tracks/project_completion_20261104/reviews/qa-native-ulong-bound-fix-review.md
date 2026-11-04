---
type: Review
title: Native BCrypt ULONG bound source review
date: 2026-11-04
status: approved
---

# Native ULONG bound correction: independent source review

**APPROVED** for the exact two-edit correction in `tools/qa/native_observer/worker_protocol.hpp` against HEAD `4add551c28479c3de61ee6fe7a251ab381283580`. No concrete source blocker was found. Root may commit this header correction and perform the actual compiler retry; compilation, linking, stage publication and native operation remain unproved by this review.

Only this review was authored. Inspection was limited to source, read-only Git, installed SDK headers, the author's report, retained compiler diagnostics and byte/hash processing. No reviewed code, compiler/parser/formatter/test, build, app/helper, process/native/OS API, input/capture, DB/network or Git mutation was executed. Other authors' source and root's evidence/Conductor work were preserved.

## Exact scope and identities

The header changes are exactly: add `#include <limits>` after `<initializer_list>`; replace `MAXULONG` with `std::numeric_limits<ULONG>::max()` in the existing `sha256` input-length guard. Reversing these two edits in memory reproduces the HEAD header byte for byte. No worker body, build recipe, wire/proof, ownership or authorization code changes belong to this delta.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| HEAD header | 14597 | `950e403d0c8e314a5b3ceb5de46aa987da0a4484509fd95ff4409481238c7056` |
| Reviewed working header | 14640 | `79d41e129a3ba84e42a48857616d399331cd0071778deee98aefe482b8006aed` |
| Scoped `git diff --binary HEAD -- tools/qa/native_observer/worker_protocol.hpp` | 1180 | `c5cca6cd630e34d1f905c9b328b6b99532b1b2521757e62616f3bae044c7df0f` |
| Author `.omc/research/qa-native-ulong-bound-fix-report.md` | 2121 | `086ee901c12c069044ef7ae16c976fa10427fde5bffa94eb803501add4ebf036` |

Read-only status and the source-extension diff inventory show this header as the only pending production source change. Concurrent checkpoint/metadata/plan edits, preexisting `conductor/.conductor_session_log`, and untracked failed-build evidence are excluded. The header's scoped whitespace check passes; Git's prospective LF-to-CRLF notice is not a source error and no newline rewrite was performed.

## Bound and SDK semantics

The installed primary SDK source under `C:/Program Files (x86)/Windows Kits/10/Include/10.0.22621.0/` declares `ULONG` as `unsigned long` in `shared/minwindef.h:51`. The actual `BCryptHash` declaration in `shared/bcrypt.h:1419-1426` accepts `ULONG` for `cbInput` and `cbOutput`. The standard library bound therefore expresses the precise representable maximum of the destination type already used by `static_cast<ULONG>(data.size())`, without depending on an unavailable macro.

At header lines 131-132 the left operand of `&&` still executes before the `BCryptHash` call and its input-length conversion. An oversized `string_view` length consequently refuses with the unchanged `hash_unavailable` failure instead of reaching a narrowing conversion or the hash API. Valid lengths retain the same API call, result check and 32-byte hash output. Existing RAII ownership still closes the algorithm handle on success or failure. `NOMINMAX` remains defined before the Windows headers, so the Windows function-like `max` macro does not interfere with the new qualified standard-library call. The OLE prerequisite and all link directives remain unchanged.

The retained `.omc/research/completion-native-layout-v3-stage-final-retry.log:7` records actual C2065 for `MAXULONG` at the old header line 131. It supports this concrete replacement; it does not prove that the next build or any native journey succeeds. No additional correction is inferred from the earlier diagnostic history.

## Approval boundary

Approval covers the exact header bytes above and an optional exact-body OKF publication copy of this review. Root retains ownership of staging/commit, actual compiler retry, failed-build artifact publication and Conductor reconciliation. Preserve the session log exclusion. All runtime, native transport/lifetime/ownership, paired restart, authorization, rendering, campaign, CI and release acceptance gates retain their existing status until separately executed and reviewed.
