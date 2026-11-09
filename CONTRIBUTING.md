# Contributing to FractalEngine

Thanks for your interest in contributing. This document covers the
practical workflow; for build details see [BUILDING.md](BUILDING.md).

## Quickstart

1. Install Rust 1.91+ (stable) — full prerequisites per platform are in
   [BUILDING.md](BUILDING.md).
2. Clone and build:

   ```bash
   git clone https://github.com/Escherbridge/fractalengine.git
   cd fractalengine
   cargo build
   ```

3. Run the GUI with `cargo run -p fractalengine`, the headless relay with
   `cargo run -p fractalengine-relay`.

## Before You Commit

Format, lint and type-check the crates you touched:

```bash
cargo fmt
cargo check -p <crate> [-p <crate> ...]   # the crates you touched
```

Clippy is not run (user decision 2026-11-09): it is not part of any gate, sweep or CI job.

Then run only the test batches that cover your change. Each batch in
[conductor/test-batches.md](conductor/test-batches.md) is a short list of plain
`cargo test -p <crate> ...` commands: run them directly on any OS, or
use the PowerShell runner, which runs one command at a time and deletes that
command's test executables afterwards:

```powershell
tools\qa\test_batches.ps1 -Batches R1,R8
```

Do not run a bare `cargo test` or `cargo test --workspace`: it links every
test binary at once and can fill the disk. See "Test execution policy" in
[AGENTS.md](AGENTS.md).

### Test policy: one sweep at the end

When a change involves multiple fixes or touches several crates, apply all
of the changes first and then run the `fmt` / test-batch sweep
**once** at the end — do not iterate test-fix-test per individual change.
One integrated sweep surfaces the full picture and keeps iteration cheap.
(Exception: if you touch test-harness infrastructure itself, running just
that harness inline once is fine.)

## Project Workflow

Feature and bug work is tracked in the [`conductor/`](conductor/) directory
— specs, plans, and track status live there (see
[`conductor/workflow.md`](conductor/workflow.md)). For non-trivial changes,
open an issue first so the work can be scoped; maintainers create a
conductor track per feature.

## Code Comment Convention

- Source code carries **terse one-line doc comments** — the "what".
- Design rationale, module notes, and the "why" belong in the
  directory-level `AGENTS.md` next to the source; leave a one-line pointer
  in code if needed.
- Avoid multi-paragraph inline comment blocks.

## Licensing of Contributions

FractalEngine is licensed under Apache-2.0
([LICENSE-APACHE](LICENSE-APACHE); ratified 2026-07-17, decision D-69).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be licensed as above, without any additional terms or
conditions.
