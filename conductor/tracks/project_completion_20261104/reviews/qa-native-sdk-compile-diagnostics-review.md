---
type: Review
status: source_approved
---

# Native SDK prerequisite and diagnostics batch: independent source review

**APPROVED for the exact three-path correction against HEAD `86c53c1449f454464923dfc281dea8daa2b23876`.** The one-line OLE prerequisite follows the actual installed SDK include chain, and the compiler stdout/environment changes preserve exit handling and the stage contract. No concrete source blocker was found. Root may commit and retry; successful native compilation/linking or stage publication remains unproved.

Only this review was authored. Read-only source/Git/installed primary SDK/captured-log/hash inspection; no source/index/history mutation, compiler/parser/formatter/test, toolchain invocation, app/helper, native/OS/process/input/capture, API/DB/network action. Root's actual failed builds/diagnostic are retained evidence, not execution performed by this reviewer. Concurrent current Conductor status documents and preexisting session log are excluded.

## Exact source anchors

Scoped three-path binary diff: **2822 bytes**, SHA-256 **`cb64949ab7941712b19bb20a30168f32bef660683e4346e25889dc2e14660eb2`**.

| Reviewed working path | Bytes | SHA-256 |
| --- | ---: | --- |
| `tools/qa/native_observer/build.ps1` | 3292 | `a665fd3fa29cba3cfbc021761b8370c7ff5b7681556afc8f36e502ac27062644` |
| `tools/qa/native_observer/worker_protocol.hpp` | 14597 | `950e403d0c8e314a5b3ceb5de46aa987da0a4484509fd95ff4409481238c7056` |
| `tools/qa/native_observer/AGENTS.md` | 22126 | `f96e17c31156d21e687b8d975d3fe02e02ceb198e50d3a7b99f68de78dfb6d8e` |

Independently verified the header is exactly the prior **14579-byte** header, SHA **`c0682b813f99110a9989a1b0701f2d7993e76c96d4369241ced885722dd6487c`**, with only `#include <ole2.h>` inserted immediately after windows.h. Worker body remains **40808 bytes**, SHA **`cb11508933e81da8a90fb49ba69a91152b7bbd94bc855909d2cbb34b59a39c72`**. Scoped read-only `git diff --check` passes with ordinary newline-policy notices.

## Installed SDK prerequisite

Read the author's `.omc/research/qa-native-sdk-include-fix-report.md`: **2677 bytes**, SHA **`60c55cf80fc4f942a8a06bfeea360e8f427a3cfbcfdcfd45169913f9123b868d`**, and independently inspected the called official installed SDK22621 headers.

`windows.h`'s usual ole2.h inclusion is inside its non-lean block. The worker deliberately defines `WIN32_LEAN_AND_MEAN`, so that prerequisite is skipped. `UIAutomation.h` includes UIAutomationCore before Client, and Core's first forward interface declaration occurs before its later OLE imports. The retained diagnostic begins with actual C4430/C2146 errors at that point.

Explicit ole2.h supplies objbase/combaseapi declarations, including the C++ `interface` definition, plus oleauto declarations used by the existing BSTR/VARIANT/SAFEARRAY helpers before UIAutomation is included. This is the normal primary SDK prerequisite, not a local macro shim or SDK edit. WIN32_LEAN_AND_MEAN and NOMINMAX remain in place. No worker body, wire field, proof/ownership gate, authority path, ABI declaration authored by the project, or linker recipe is otherwise changed. This addresses the demonstrated header cascade; it is not a claim that every later compiler/linker issue has been eliminated.

## Compiler output and temporary environment

The existing stage code collects build.ps1's SuccessStream output to obtain its final manifest object. Previously native compiler stdout could therefore be hidden in that collection. The corrected native pipeline sends each stdout line to `Write-Host`; the final returned helper manifest remains the intended SuccessStream object. Compiler stderr is not merged into the manifest stream, and no output regex is used to decide success.

`ForEach-Object` and `Write-Host` are PowerShell cmdlets; neither replaces the native command's `$LASTEXITCODE`. The existing nonzero-exit/missing-executable failure gate remains after the `finally` block. If command execution or output handling throws, the existing terminating error path still prevents successful manifest return.

Read the installed official VsDevCmd entry, which explicitly generates environment logs when `VSCMD_DEBUG` is nonempty and can print environment state at higher levels. The script now captures the caller's process value, removes it only for the existing child setup/compiler invocation, and restores the captured value in `finally`. Existing temporary Installer PATH setup/restoration is preserved. Neither cleanup assignment invokes another native command or masks its exit code. No user/machine environment setting or `NoDefaultCurrentDirectoryInExePath` change is added; no environment value is printed by this patch.

The fixed VS2022 C++ discovery, literal path/metacharacter checks, SDK22621/x64 arguments, compiler/linker command, asInvoker/no-UIAccess/PMv2 manifest, immutable-stage refusal and returned schema remain unchanged. Build.ps1 and the header are both included in the existing recipe source hash, so future staging records the corrected inputs. Native CLI, Owner HTTP floor, durable slot, retained COM identity, helper lifecycle and capture refusal remain untouched.

## Failed evidence is accurately bounded

The three retained `conductor/tracks/project_completion_20261104/evidence/build-attempts/86c53c14-*` artifacts were read and hashed:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `failed-native-compile.json` | 1257 | `01bd39ef3ba78f019c0a2062e01baf784520793d0669517d8088808c82f57150` |
| `failed-native-compile.log` | 775 | `52e0b4cb79746f84a692e0a57baf19c53dd431dfbb758c5f82c273ba5c06ba87` |
| `sdk-header-diagnostic.log` | 24772 | `e8ab3f3e71e7786d17079dfd6e71eb2138f3324e62f6e0b509c936201555fb68` |

The production receipt records terminal session 73746, exit 1, actual native compiler invocation, hidden parent-stage compiler stdout, partial stage `.qa/stage/86c53c14-20261104144656517` and no published stage manifest. The log retains successful Rust build steps followed by native build failure. The separate 160-line diagnostic contains the SDK header error cascade, including C4430/C2146 and subsequent C2371/C1003; it does not establish a completed worker.

The receipt explicitly discloses that the direct diagnostic used the incorrect unpublished output timestamp `.qa/stage/86c53c14-20261104144658698`. The observed SDK header errors precede object-output creation, so they still support this include prerequisite. They do not prove that the incorrect output target would otherwise work, or that a binary/stage was produced. The receipt's statement about removing two exact temporary SDK environment logs is retained as the parent's cleanup record; this reviewer did not read those environment logs or independently operate/audit their deletion.

Earlier terminal session 71477 remains the separate pre-compiler toolchain-lookup failure. Neither failed attempt is promoted to native readiness, UI operation, parser/test completion or app acceptance.

## Allowed next action and limits

Root may explicitly commit the **three reviewed source/doc paths** and proceed to the actual compiler retry. An exact-body OKF copy of this source review is a separately permitted documentation addition. The failure artifacts and current Conductor reconciliation need their separate publication bookkeeping; they are not implicitly source-approved here. Exclude the session log.

The retry must establish actual native compilation/link/manifest and complete stage publication at its new source fingerprint. All existing journey, real pipe/lifetime/durable ownership, paired restart, Viewer/Editor/native delivery, independent data/rendering and final campaign/CI/release gates remain unfinished until executed and reviewed. No additional speculative source correction is authorized by the old diagnostic cascade alone.
