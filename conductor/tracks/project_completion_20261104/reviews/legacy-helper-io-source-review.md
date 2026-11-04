---
type: Review
date: 2026-11-04
title: Independent legacy helper stdin ownership source review
source_artifact_sha256: 00d3ab787c5de4bb2ec180060067c5235e40e883a761d5a8bffc4932bbca6ade
---

# Legacy helper stdin ownership: independent source review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the narrow source correction.** No blocking finding in the reviewed
two-file diff. This is not runtime acceptance or proof that the retained launch
failure has only this cause.

## Exact reviewed source

Isolated checkout `.omc/worktrees/qa-legacy-adoption-fixture`, committed baseline
`5a90a07a9c8e4d57a77e71dd3710f969fa2d6f90`. The only dirty tracked files observed
were `tools/qa/_legacy_runner.cs` and `tools/qa/AGENTS.md`. The correction was
uncommitted at review. `git diff --check` for those files returned 0.

SHA256 of the captured `git diff --no-ext-diff --no-color --` for those paths:
`25d8123c8ea4fa8dc7f498b70cf5cc12c2c73c1b6c24219482ba111e9096ce36`.

Raw working-file SHA256:

| File | SHA256 |
| --- | --- |
| `tools/qa/_legacy_runner.cs` | `c6bbfc1deefaf4cdcfa9717ca089583a16c421ec70f5c602849450a81d4a064c` |
| `tools/qa/AGENTS.md` | `e4db9e570da6435d6ee14d3b0050cdd89155850bb6fa13e7338f567d4abc02ce` |

## Findings and reasoning

The previous `Write(Stream, byte[])` closed `StandardInput.BaseStream` directly.
Its owning `StreamWriter` remained open, and the successful `Execute` cleanup
later closed that writer. A writer flush against an already disposed base stream
can throw and be mapped by `Complete` to `helper_io_refused`. The retained launch
log records that static refusal, while the inherited preparation evidence records
generated files. That supports the source diagnosis but does not identify the
original exception conclusively.

The correction passes `process.StandardInput` into `Write(StreamWriter, byte[])`.
It keeps the same raw-byte write and flush, then closes the owning writer to
deliver EOF. Later cleanup closes an already disposed writer, rather than an open
writer over a manually disposed base stream. Repeated disposal is the intended
idempotent ownership path. No new string re-encoding, command argument, helper
mode, output acceptance or identity/authority path is introduced. The terse
implementation and directory-level ownership explanation fit the existing style.

The operation still starts both output collectors before writing and waits for
output, stderr, input completion and the owned helper exit before successful
cleanup/release. The 65,536-byte input limit, 4,194,304-byte stdout limit, zero-byte
stderr limit, strict UTF-8 output decoding, 35-second absolute operation deadline
and five-second cleanup wait are unchanged. Writer closure runs within the
existing task/deadline ownership, so a blocked close does not create a new caller
wait outside that bound.

SafeHandle add-reference and release conditions are unchanged. Success releases
only after the existing exit/stream cleanup path; failure enters the existing
termination path, and unconfirmed cleanup retains references and reports
`helper_lifetime_unknown`. Existing timeout-versus-completion concurrency and
conservative retention are not redesigned or newly certified by this diff.
Offline store reads, resume, adoption, replay and direct raw-helper bypass remain
outside this correction.

## Remaining acceptance

The parent must preserve the original partial fixture root, integrate this exact
reviewed correction, and use a fresh exclusive fixture root for a bounded actual
Windows retry under the reviewed launcher. Preserve its result and cleanup
evidence. Do not report successful fixture preparation, app startup, adoption or
restart until those paths actually execute. A successful narrow helper retry
does not waive the unsupported store-confinement and require-existing restart
work.

This lane used only read-only source, Git status/diff/hash inspection, the writer
report and the retained failure log, followed by this review artifact. No
compiler, Cargo, tests, app/fixture, OS process inspection, API/DB/store access,
network request, production-source edit or Git mutation was performed.
