---
type: Review
status: source_approved
---

# Native toolchain Installer PATH: independent source review

**APPROVED for the exact two-path correction against HEAD `1b477696a8d5a14594cab1f9679c3a40da6e0364`.** Temporarily adding the already verified official Installer directory to the build process PATH addresses the inspected VsDevCmd lookup while preserving the current-directory search hardening flag. The previous process PATH is restored in `finally`. No concrete PowerShell, quoting, native/Owner-boundary or source-preservation blocker was found. Root may commit and retry the production stage; no successful native build or application acceptance is asserted here.

Only this report was authored. Read-only source/Git/local official batch-file/evidence/hash inspection; no source/index/history mutation, parser/compiler/formatter/test, toolchain invocation, app/helper, OS/native/process/input/capture, API/DB/network action. Root's retained terminal build evidence is not an execution performed by this reviewer. Current Conductor status documents and preexisting session log are explicitly outside this source approval.

## Exact delta and failure anchors

Scoped binary diff: **2338 bytes**, SHA-256 **`2c965c2a03e22cc48c92ba1ee4e9cbcbea90be1e2b4ba58a9a116b7d5fd5e4c8`**.

| Path | Before bytes / SHA-256 | Reviewed working bytes / SHA-256 |
| --- | --- | --- |
| `tools/qa/native_observer/build.ps1` | 2920 / `d37fd8c9b956d636458e53985c1a005cd532b12c9f96ce2390967ae0b4da2c0f` | 3101 / `cca88042fecf04212689c993f58f29bf03e1022dc33c180b1be8d1ee11118e23` |
| `tools/qa/native_observer/AGENTS.md` | 21446 / `de5e43eff858b2965106c12c95870bd1d3ef85fcecb7d68d974610a02ff10a55` | 21723 / `5d223ad5ac2f29f7adc73bdf3817c2556cbf097c43b1c6df8bfb7c1c50b15fdf` |

Retained failed-stage evidence:

- `conductor/tracks/project_completion_20261104/evidence/build-attempts/1b477696-failed-native-toolchain.log`: **1743 bytes**, SHA **`acff568dff79b960a877768cf70df9ba454d5e8a781fd3b31451fd4fe8e9e9f1`**, independently verified.
- Corresponding JSON receipt: **519 bytes**, SHA **`8c661a783d655e9f1171a9a403e47283472dd28c9239b5e0a6c0b5f47210e15f`**.

The receipt records session 71477 terminal exit 1, Rust application compiled, native compiler not invoked, and no published stage manifest. Raw log shows completed Rust builds followed by unqualified `vswhere.exe` lookup failure and `Native helper build failed`. This is a failed overall production stage, not native compilation or app/native acceptance. The partial stage remains identified by the receipt; this review did not operate or modify it.

## Inspected cause and correction

Read the installed official Community `Common7/Tools/VsDevCmd.bat` around lines 165–189. It locates the Installer directory, pushes that directory, then calls **unqualified `vswhere.exe`** inside its `for /F` command. Merely changing the working directory does not provide executable lookup when the existing `NoDefaultCurrentDirectoryInExePath` hardening excludes it and Installer is absent from PATH. Root reports that exact environment preflight; no environment probe was performed in this review.

The correction derives `Split-Path -Parent $vswhere` from the fixed absolute official discovery executable path already existence-checked and invoked earlier. It prepends that trusted directory to the captured existing **process** PATH only around the same `cmd /d /s /c` build invocation. It does not write user/machine settings or alter `NoDefaultCurrentDirectoryInExePath`.

PowerShell captures the old PATH string before entering the try block and restores that same value in `finally`, including normal command failure and launch exceptions. The child inherits the temporary PATH; vcvars' own child environment changes do not need to be copied back. Neither the assignment nor restoration invokes another native command, so the existing `$LASTEXITCODE` gate still observes the build command's exit status. No new shell text is assembled from PATH.

The build command, quoted vcvars/source/object/output/manifest paths, metacharacter checks, fixed VS2022 C++ discovery filters, SDK22621 and x64 arguments, static compiler/link recipe, asInvoker/no-UIAccess/PMv2 manifest, immutable-stage refusal and returned stage schema are unchanged. No caller toolchain parameter or download is added. The recipe still hashes build.ps1 itself, so this correction is reflected in future sealed source hashes.

Native worker/controller/ledger/CLI/host source is untouched. There is no new Owner HTTP request, changed native floor, input prerequisite, capture mode, proof renewal or invocation retry. The additive directory guide accurately states the temporary environment boundary and remaining build limits.

Scoped read-only `git diff --check` passes, with ordinary prospective newline-policy notices. No source edit was needed in this review.

## Scope and next gate

Approval covers only **build.ps1 and its native_observer/AGENTS.md documentation** at the hashes above. Root owns any separate bookkeeping approval for the retained failure receipts/current Conductor updates; this report does not approve those concurrent documents by implication. Exclude the session log. An exact-body OKF copy of this source review is a separately permitted documentation addition.

The next production-stage retry must independently establish vcvars setup, actual C++ compilation/linking, embedded manifest and full stage publication. Parser/journey/lifecycle/native Viewer/Editor/capture and final campaign acceptance remain their separate unfinished gates. The failed attempt remains preserved regardless of retry result.
