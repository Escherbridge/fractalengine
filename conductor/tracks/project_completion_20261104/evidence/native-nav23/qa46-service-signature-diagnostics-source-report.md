---
type: SourceReport
title: Native service signature diagnostic author report
date: 2026-11-04
---

# QA46 strict service signature: diagnostic-only source freeze

Author `/root/legacy_helper_io_fix`, source-only, 2026-11-04.
MAIN foundation `be37b2dc260b06b42307a9cab3a49f08202c108c`.
Only the three assigned native_observer source/doc files changed. Root's live
QA45/46, asset-author changes, Conductor/evidence and all other paths are preserved.

## Actual result and unchanged gate

Read root's existing `qa/passes/46/owner-native-observe-service-stage.jsonl`:
window_service_signature_refused, helper_exit_code2, attempt_spent=false,
invoke_entered=false, method_returned=false and no capture. This indicates the
preceding exact winit-class/process/thread/relationship/visibility checks passed
and BOTH rectangle reads succeeded. The combined style/exstyle/outer/client
signature failed; the particular component and raw values are not established.
There is no positive service-profile, UIA/provider/navigation or action result.

This change splits ONLY that existing conjunction into first-failure fixed codes:

1. window_service_style_refused — the same exact DWORD style comparison fails.
2. window_service_exstyle_refused — the same exact extended-style comparison fails.
3. window_service_outer_rect_refused — any outer rectangle coordinate is nonzero.
4. window_service_client_rect_refused — any client rectangle coordinate is nonzero.

The original comparison order is preserved. Expected style0x90000000 and extended
style0x080800a0, all zero-coordinate predicates, same-process/class/thread/owner/
child gates, census, retained WindowSet, watcher and every use boundary remain
unchanged. There is no tolerance, mask, ignored flag or additional native read.
The exact same GetWindowLongPtr/RECT facts enter the production pure function.

`winit_service_signature` becomes `winit_service_signature_refusal`, returning
one static reason or nullptr iff every original predicate passes. service_current
throws that reason when nonnull and continues to the unchanged child check only
when the original conjunction would pass. No authority, proof, deadline, target,
Invoke/ledger ownership or success semantics changes. Output shape is unchanged;
no raw flags, coordinates, names, titles, class strings, handles or counts appear.
Controller adds only four fixed allowlist strings and keeps the old combined code
for older staged workers. Native observe remains no-token/no-HTTP.

## Exact files and packet

All file paths below are under tools/qa/native_observer/.

| File | Before bytes / SHA-256 | After bytes / SHA-256 |
| --- | --- | --- |
| worker.cpp | 44445 / `da27fe8405ec5e5bd8b78b4f002fc53fc62fde4d79e97d776cf3e549e8a982ee` | 44677 / `1df716827ceb97df0a0803f66d8691be6f6b9ea6d98f6abc08abbc2e2c0941a6` |
| controller.py | 16097 / `0d4c87f67f19d7a21d2b9136b54ab2fd66b3751372812ce225f43f3463e17ec0` | 16226 / `17adfce21cfc4e0a891042c8a5ec2143900d364192f63d4c4f575d738717414c` |
| AGENTS.md | 26705 / `ab56fc920dcb95174cf894028ceb3e1a5b9e8eea56f81788b3b54b33590e8549` | 27355 / `7b389bdece379b0b082f407ec16ba43cbea5c144f80320e9b54402766f65547c` |

Frozen packet `.omc/research/qa46-service-signature-diagnostics-source-v1/`
contains all three raw files plus:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| source.diff | 4972 | `5ab012de1f7156e339e7d0a7f012523334d44a0aa98378d3739c65ac7f19131c` |
| manifest.json | 1057 | `e9449cbb822358b973d144ba885c271039f760d3fcc723a8e7ccafdb6867aac8` |

Scoped diff21 additions/6 deletions: worker9+/5-, controller2+/0-, directory
guide10+/1-. No new source path, test declaration or runtime mode. Raw hashes and
snapshot govern identity; normalized source.diff is the exact scoped review aid.

## Verification boundary

Read-only source/Git/evidence/hash inspection and source/artifact authoring only.
Scoped diff --check and trailing whitespace inspection are clean. No compiler,
parser/formatter/tests, helper/app/process/OS/native/API, database/network or Git
mutation occurred. An initial patch-context mismatch applied no changes; the
actual diff was inspected before the successful narrow patch.

Root's exact-delta review, build and new owned observation remain required. New
fixed codes can locate the failing component, not identify a raw bit/value or
justify relaxing the profile. Earlier QA46 is not relabeled. All positive native,
authority/delivery, capture, campaign, CI and release acceptance remains pending.
