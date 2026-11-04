---
type: Review
title: Reviewed native build evidence and current Conductor state
date: 2026-11-04
status: approved
---

# Completion build evidence and Conductor reconciliation review

Reviewer: `/root/native_nav23_functional_review`, 2026-11-04.

**APPROVED for publication of the exact corrected Conductor documents and eight new build-evidence files identified below.** The successful `925d7cba` stage is supported by its retained log, exact published manifest, matching application/helper binaries and native source recipe hash. The failed attempts remain failed, without a published stage. No runtime, native interaction, visual, final-sweep, CI or release acceptance follows from this build evidence.

This was bookkeeping/evidence review only. Read-only source/Git/file inspection and byte/hash/JSON processing were used. No implementation parser, test, compiler, application/helper, process/OS/native/API/DB/network action or Git mutation was performed. Only this report was authored. Root owns the live QA42 application and all lifecycle actions; its live state was not inspected or changed by this reviewer. The preexisting `conductor/.conductor_session_log` is expressly excluded.

## Corrected document scope

The pending diff is limited to current source/build/review status, the new build evidence and the live-launch bookkeeping. Earlier QA evidence, open acceptance items, final sweep and delivery gates remain distinct. `plan.md` marks NAV23 partial `[~]`, retaining actual native/lifecycle acceptance as pending. The metadata reports 28 final native paths and 19 residual-layout paths, matching the separately approved exact integration of 47 source paths in `8968d918`.

Two stale statements found in the first read were corrected by root before this final review: checkpoint's old 24-file helper count now says 28, and metadata's remaining-work field no longer lists compilation as unperformed. The current owned-app clause also names the completed QA42 launch rather than calling preparation in progress. These corrections were independently re-read; no documentary correction remains blocking.

| Corrected document | Bytes | SHA-256 |
| --- | ---: | --- |
| `checkpoint.md` | 9088 | `cd46ab9b92d0c6107cb1ec6fb0e7822e22dba88cf2d8e8a75212a47bc3cba4d5` |
| `metadata.json` | 5942 | `cfb2cb2491cb07c8f5fc0fc8fd958334d7eb85fc1ba6f7cbbd61a2d233b8cc92` |
| `plan.md` | 11461 | `86a1ab895aa6e56d69b250d8090ee6b494b321ef54810fed982c6010771f6448` |

Paths above are under `conductor/tracks/project_completion_20261104/`. The Markdown documents retain their required OKF frontmatter. No historical packet or review body is rewritten to imply current-build acceptance.

## Raw evidence identity

All eight new paths are under that track's `evidence/build-attempts/`:

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `86c53c14-failed-native-compile.json` | 1257 | `01bd39ef3ba78f019c0a2062e01baf784520793d0669517d8088808c82f57150` |
| `86c53c14-failed-native-compile.log` | 775 | `52e0b4cb79746f84a692e0a57baf19c53dd431dfbb758c5f82c273ba5c06ba87` |
| `86c53c14-sdk-header-diagnostic.log` | 24772 | `e8ab3f3e71e7786d17079dfd6e71eb2138f3324e62f6e0b509c936201555fb68` |
| `4add551c-failed-native-ulong.json` | 614 | `ec8b5ae7d90fc0f580d7a30e51ab28432016f93d7380b33ffdbd535e367c85a8` |
| `4add551c-failed-native-ulong.log` | 949 | `72ebbaac76d62cd47129920e3cfe14eb41aabecf2395166b2ffae24d6bf57735` |
| `925d7cba-successful-native-stage.json` | 1021 | `ec8753fe1d9ca503da66755a2040e781367d67634bd22c0fc5afaddd64a7c3a2` |
| `925d7cba-successful-native-stage.log` | 1860 | `a6b065795385ae879ee2cc0bb7e552b97b2bea0082480916fb0ce6cbad583f17` |
| `925d7cba-stage-manifest.json` | 1166 | `964cfb6e2800478d8eaad9f71fa1b2c071a346c53f4c77b1513cd485c00aeea9` |

Each receipt's declared payload length/hash matches the actual file, where declared. Every copied log is byte-exact against its retained original, without text re-encoding:

- `86c53c14-failed-native-compile.log` equals `.omc/research/completion-native-layout-v3-stage-retry.log`.
- The SDK diagnostic equals `.omc/research/native-first-compiler-diagnostics.log`.
- `4add551c-failed-native-ulong.log` equals `.omc/research/completion-native-layout-v3-stage-final-retry.log`.
- The successful `925d7cba` log equals `.omc/research/completion-native-layout-v3-stage-ulong-retry.log`. Its original UTF-16 LE BOM is preserved.
- The copied stage manifest is raw-byte identical to `.qa/stage/925d7cba-20261104150814356/stage.json`.

The chronological retry filenames do not change which source attempt their contents establish. No failed log is substituted for a successful stage or normalized into a cleaner transcript.

## Successful build claim and its provenance limit

The successful log records four completed Rust dev-profile builds, actual native compiler source output `worker.cpp`, then `Staged 925d7cba` at the exact published path with 12.7 seconds total. It contains zero Rust compiler warning/error diagnostics and zero MSVC warning/error diagnostics. PowerShell wraps one ordinary Cargo `Finished` stderr line in a `NativeCommandError` record; that cosmetic host wrapper is retained and is not misclassified as a compiler error or a reason to hide output.

The current reviewed build recipe invokes actual `cl.exe` compilation and linking, tests its exit status and output existence, hashes the native image and returns the helper manifest before stage publication. The successful stage therefore establishes the completed compile/link path, rather than only an SDK parse or Rust-only build. This review does not execute the recipe or claim an independently captured command trace for every compiler argument.

The root-authored retained receipt records **session 40403, terminal exit code 0**. Root additionally confirmed that its actual `write_stdin` tool response returned `exit_code: 0`; that conversation tool response has not been separately archived as a raw terminal-response file. The success log and existing stage independently corroborate successful publication, but do not themselves encode the orchestration session ID/terminal result. This is the explicit provenance limit for that particular receipt field, not an independent re-execution of session 40403.

The actual published manifest names full source commit `925d7cbaddcdbd9996fc32d10078326cae0aaaa3`, build timestamp `2026-11-04T22:08:14.6497603Z`, native x64 SDK `10.0.22621.0` and the supported native contract. Its `source_dirty: true` is preserved. Read-only Git shows no pending production source/build-input change; the tracked dirt consists of the reviewed Conductor documents and excluded session log. This is a successful build from those recorded inputs, not a claim of a clean Git worktree.

Independently hashed the actual published files:

| Staged file | Bytes | SHA-256 |
| --- | ---: | --- |
| `fractalengine.exe` | 369651712 | `3825c318ab967c62890c1f20724b10421ffeb5609118967daaf8ed3ee65e42ea` |
| `native_observer.exe` | 304128 | `633dd7e67f2dc49bbf91a76aac4d8454f6d9ec7cf9ad61edd357173af95ca04f` |
| `seal_hexon.exe` | 39797760 | `22efc1d8e82545d3f06ddbb40721294cb0383a030c8d8dea9a6165f5b3c42971` |
| `legacy_adoption_fixture.exe` | 1495040 | `a9b6f8edd19ad8afe4ad7315bab4f7dd2a0c945b675dde5158382e1a5d841b67` |

All four hashes equal the corresponding manifest values. Independently hashing the current `build.ps1`, `worker.cpp`, `worker.manifest` and `worker_protocol.hpp` in the declared ordered recipe yields native source hash `5993b74e1fc2728cb636722d6152580d443eb0e7af1ef17d3c67f46bec1164cd`, exactly the manifest value. This is artifact/source identity evidence, not executable-behavior acceptance.

## Failed attempts and commit sequence

Read-only commit inspection confirms the ordered follow-ups: `1b477696` discovers the installed Visual Studio edition; `86c53c14` preserves hardened lookup while preparing its compiler environment; `4add551c` includes COM prerequisites and exposes native diagnostics; `925d7cba` uses the declared BCrypt ULONG bound. The last header correction has its own independent source approval and was not re-authored or broadened in this review.

The existing session 71477 receipt/log establishes failed toolchain lookup with no native compiler invocation. Session 73746's normal log shows native build failure; its separately retained actual compiler diagnostic shows SDK `UIAutomationCore.h` missing interface/type declarations. The receipt correctly discloses that the diagnostic used an incorrect output directory, and does not turn that diagnostic into a completed stage. Session 99364 shows actual `worker.cpp` compilation followed by C2065 for undeclared `MAXULONG` and native build failure.

At read-only inspection, the actual recorded partial stage directories for `1b477696`, `86c53c14` and `4add551c` contain neither a published `stage.json` nor a `native_observer.exe`. The incorrectly named diagnostic output directory does not exist. Thus no failed attempt is credited with a successful native stage. Rust completion during those attempts does not establish successful combined publication.

## Live launch bookkeeping and publication boundary

The corrected checkpoint/metadata identify root-owned QA42 run `.qa/run/20261104-150850-588`, PID 110832, and distinguish completed preparation from session 18216's open inherited-output handle. The retained `.omc/research/qa42-launch.log` records that exact stage, run/PID and completed 15.2-second launch. At inspection it is 1730 bytes, SHA-256 `e68abf51022db423b4b26f016213b34f999af84ad62d1762cbdf9b80cb7e3451`. Root's assertion that the app remains live is lifecycle-owner information; this reviewer made no OS/process query and does not infer that an open output handle means launch is incomplete or no app is running.

The pending documents keep actual helper/provider/invocation/lifecycle, paired restart, historical stage pruning, matching visual acceptance, final integrated sweep, exact-SHA CI and release delivery open. Approval covers the exact three corrected documents and eight evidence files above, plus an exact-body OKF copy of this review if root publishes it. It excludes the session log, evolving QA42 packet, live run, unrelated source and any blanket directory staging. Recheck staged bytes at publication; a later live-state documentary update should be reviewed against its own evidence rather than silently receiving runtime acceptance from this report.
