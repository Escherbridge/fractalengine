---
type: Review
status: source_approved
---

# Native helper toolchain discovery: independent source review

**APPROVED for the exact two-path portability correction against HEAD `8968d9188421c981bc6081aa9359c22730e43f84`.** It replaces the absent fixed BuildTools edition path with installed VS2022 C++ discovery through the fixed official Installer `vswhere.exe`. No concrete PowerShell, argument-quoting, native wire, authorization or source-preservation blocker was found. Approval is source-only; no parser, build, linker, helper or application success is asserted.

Only this report was authored. Read-only Git/source/captured-help/hash inspection; no source/index/history mutation, parser/compiler/formatter/test, tool invocation, app/process/native/OS API, HTTP/DB/network action. The parent's prior vswhere preflight is retained evidence, not execution performed by this reviewer. Index was empty. The preexisting `conductor/.conductor_session_log` is outside scope.

## Exact correction

The scoped binary diff is **2635 bytes**, SHA-256 **`3a7c531e25c72c8731b7c1042d95c385b92e2eacc608c10736d1477204022313`**. Working tracked changes consist of these two source/doc paths plus the excluded session log.

| Path | Before bytes / SHA-256 | Reviewed working bytes / SHA-256 |
| --- | --- | --- |
| `tools/qa/native_observer/build.ps1` | 2405 / `501f1496b017fc927c471574f00f877abafebc7cba5b9b8e63eca2dc0f542692` | 2920 / `d37fd8c9b956d636458e53985c1a005cd532b12c9f96ce2390967ae0b4da2c0f` |
| `tools/qa/native_observer/AGENTS.md` | 21259 / `22ff52e6010ab62b541aace83bb894ae478e813af3e6e784c1bc8539eafde85b` | 21446 / `de5e43eff858b2965106c12c95870bd1d3ef85fcecb7d68d974610a02ff10a55` |

Read the whole current build script and exact documentation delta. No worker, controller, ledger, CLI, lifecycle, app or host file changes in this correction.

## Discovery and PowerShell semantics

Discovery executes only the fixed literal official Installer path `C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe`, requiring its existence. It asks for the newest installed complete release in version range `[17.0,18.0)`, across products, requiring `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`, and requests only `installationPath`.

The captured official local help `.omc/research/native-vswhere-help.log` is **7438 bytes**, SHA **`2ad08a308cdae5adf8ad2a15ebe52a9afc9e3ec322a6f36e9ed0c30221e357fc`**. It confirms these flags, `-latest` selection, the all-products `*` value, version-range semantics and property-only value output without a logo. This source review did not invoke vswhere or browse externally.

PowerShell's `@(...)` captures discovery output as an array even for zero/one result. The pipeline removes blank output only. Native command invocation sets `$LASTEXITCODE`; `Where-Object` does not replace that native exit code. The script refuses a nonzero exit or a count other than one. A missing executable, launch failure, no eligible installation or unexpected extra output cannot silently select an arbitrary compiler. The quoted `'*'` and version range are single literal native arguments, not a PowerShell provider search or shell-built query.

The sole path is trimmed, then `Join-Path` appends the fixed `VC\Auxiliary\Build\vcvarsall.bat` child. This is a discovered installed toolchain, not a caller-supplied toolchain parameter or download. The parent reported Community as the actual installed match; this review approves discovery logic, not that preflight as a compilation result.

## Shell quoting and retained build contract

The discovered vcvars path must pass the existing literal file-existence check. The SDK22621 header remains a fixed literal required file. Before shell composition, the output directory, source-script directory and derived vcvars path still reject quote, percent, exclamation, caret, ampersand, pipe, angle-bracket and CR/LF metacharacters. These checks precede the existing quoted `call "<vcvars>" x64 10.0.22621.0` command.

The literal vcvars child and quoted path keep ordinary spaces/parentheses in standard Visual Studio installation paths intact. No discovery output is appended as command text. Existing quoted source/object/output/manifest paths, compiler/linker arguments, failure checks and fixed x64/SDK22621 selection remain unchanged. vcvars supplies the compiler environment as before; this correction does not add a caller-selected `cl.exe` or arbitrary helper path.

Published immutable stages still refuse mutation. The stage manifest schema, helper filename, architecture, SDK string and wire version are unchanged. The source recipe still hashes `build.ps1` itself plus worker/header/manifest, so this script correction naturally changes the recorded recipe hash rather than silently reusing the old recipe identity. The reviewed runtime entry, ordinary/no-HTTP native boundary, one-attempt ledger, permissions, Owner HTTP floor and capture refusal are untouched.

The directory guide accurately describes edition discovery and retains those fixed/no-download limits. Read-only scoped `git diff --check` passes, with ordinary prospective LF/CRLF notices only.

## Scope and remaining execution

Root may explicitly stage and commit **only the two reviewed paths**, excluding the session log. An exact-body OKF copy of this review is a separately allowed documentation addition. Any changed discovery/build code after these anchors requires corresponding review.

The real PowerShell invocation, vcvars environment setup, exact SDK/compiler selection, C++ compilation/link/embedded manifest, source/EXE stage seals and integrated existing/native lifecycle checks remain root-owned execution gates. This review does not count the first production build or any native acceptance as complete.
