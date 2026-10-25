---
type: Mechanical Gate
track: role_propagation_20261013
phase: refine R4 (workspace clippy 1.99 + fmt + feature matrix)
reviewed_range: 4d3a1a6..HEAD (through R3 aa6d1b5)
date: 2026-10-25
verdict: FIXED
---

# Refine R4: workspace clippy (rustc 1.99.0) + fmt + feature matrix — fix and commit

**Verdict: FIXED.** Every finding below was fixed. No CRITICAL or HIGH finding; nothing left open.
This slice is the mechanical gate: `cargo +1.99.0 clippy --workspace --all-targets --keep-going -- -D warnings`
(the exact CI toolchain — see RUNBOOK "CI @stable toolchain drift"), `cargo +1.99.0 fmt --all` (+ `--check`),
and the four-combination feature matrix. No tests were run (R5 owns the sweep); this slice owns clippy/fmt.

## Gate evidence

- `cargo +1.99.0 clippy -j4 --workspace --all-targets --keep-going -- -D warnings` (env
  `CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728`): **exit 0**, zero warnings, after 4 fix rounds (below).
- `cargo +1.99.0 fmt --all -- --check`: **exit 0** after `cargo +1.99.0 fmt --all`.
- Feature matrix, `check` + `clippy -- -D warnings` each, all **exit 0**:
  - `-p fe-query --features parquet`
  - `-p fe-format --no-default-features`
  - `-p fe-terrain --features fetch,render`
  - `-p fe-hexon --all-features`
- `.github/workflows/build-artifacts.yml` (edited in the campaign diff): parsed with `yaml.safe_load` — **OK**.
- `scripts/bi-egress-verify.ps1` (edited in the campaign diff): parsed with
  `[System.Management.Automation.Language.Parser]::ParseFile`, 0 parse errors — **OK**.

## Findings (all fixed)

1. **`clippy::too_many_arguments` (8/7), `fe-sync/src/distributed_query.rs:1463`
   `respond_to_compute_request`.** Bundled the 8 parameters into a new `ComputeResponseArgs` struct
   (transport, sender, verse_id, request_id, spec, shards, timeout_ms, row_cap); the single call site
   (`distributed_query.rs:1445`) now builds the struct instead of passing 8 positional args. Pure
   refactor, no behavior change — the function body is unchanged after destructuring `args`.

2. **`E0433` unresolved crate `fe_identity`, `fe-ui/src/dialogs/entity_settings.rs:586` and
   `fe-ui/src/panels/inspector.rs:722` (lib build, not test build).** `fe-identity` was declared only
   under `[dev-dependencies]` in `fe-ui/Cargo.toml`, but W6-7b's TOKEN-CLIENT-CLAIM GUI code
   (`fe_identity::validate_client_did`) calls it from production UI code, not from a test. This was a
   **real latent build break** introduced by the campaign — `cargo check -p fe-ui` (no `--tests`) was
   failing before this fix, which R1/R2/R3's narrower per-crate gates on fe-sync/fe-database/fe-api never
   exercised. Fix: moved `fe-identity = { path = "../fe-identity" }` from `[dev-dependencies]` to
   `[dependencies]` in `fe-ui/Cargo.toml`, with a one-line rationale comment. `tempfile` stays dev-only.

3. **`clippy::redundant_guards`, `fe-test-harness/src/scenarios/role_propagation.rs:425`.** Applied
   clippy's own suggested fix verbatim: `DbResult::IotReadingsInserted { written, .. } if written == 1`
   → `DbResult::IotReadingsInserted { written: 1, .. }`. Semantically identical pattern match, no
   behavior change.

4. **`clippy::manual_find`, `fe-sync/tests/relay_assisted_test.rs:55` `container_runtime`.** Rewrote
   the `for rt in ["docker","podman"] { if ... { return Some(rt) } } None` loop as
   `["docker","podman"].into_iter().find(|rt| ...)`. Same short-circuit-on-first-success semantics,
   same `Option<&'static str>` return type.

5. **`clippy::large_enum_variant`, `fe-ui/src/dialogs/mod.rs:97` `ActiveDialog::EntitySettings`
   (424 bytes vs. 201 for the next-largest variant).** Disposition: **allowed, not boxed** — added
   `#[allow(clippy::large_enum_variant)]` with a rationale comment, mirroring the existing precedent at
   `fe-api/src/canonical_ws/handler.rs:107` (`PinnedSessionStatus`). `ActiveDialog` is a single
   long-lived Bevy-resource value (exactly one per app, "at most one [dialog] at a time" per its own
   doc comment), changed only on dialog open/close/tab-switch, and matched by reference (`ResMut`) at
   every one of its ~15 call sites across 5 files. Boxing `EntitySettings`'s ~20 fields into a separate
   struct would touch every one of those call sites to save ~220 bytes on a value that is never copied
   in a hot path — the exact trade the lint exists to warn against, not satisfy. NOT-REAL as a
   correctness concern; satisfied via the codebase's own established `allow` convention for this lint.

6. **`dead_code`, `fe-ui/src/verse_manager/db_results/tokens.rs:162` `mint_command` (lib build).**
   Root-caused rather than suppressed: `mint_command`'s own doc comment says it is "the exact, tested
   shape both GUI mint surfaces... build inline at their 'Generate API Token' send sites — keep those in
   sync with this function if the shape ever changes" — but neither call site actually called it; both
   duplicated its blank-client→`None` / else-trimmed logic inline, so the function was only reachable
   from its own unit test. Fixed by wiring both GUI call sites
   (`fe-ui/src/dialogs/entity_settings.rs` and `fe-ui/src/panels/inspector.rs`, both "Generate API
   Token" click handlers) to call `mint_command` instead of duplicating its body, which also closes the
   drift risk the doc comment warned about. Required raising `mod tokens` (in
   `fe-ui/src/verse_manager/db_results/mod.rs`) and `mod db_results` (in `fe-ui/src/verse_manager/mod.rs`)
   from private to `pub(crate)` so the function is reachable from `dialogs::` and `panels::`.

## Test disposition (NO DEFERRALS — covered-elsewhere, with proof)

Finding 6 is the only fix with an observable behavior change (the two GUI send-sites now route through
one function instead of two independently-maintained copies of the same branch). Its exact
trim/`None`-vs-`Some` branching logic is already covered by a **pre-existing, mutation-sensitive flow
test** that was not added by this slice:
`fe-ui/src/verse_manager/db_results/tokens.rs:225`
`a_blank_client_field_sends_none_and_a_filled_one_sends_it_trimmed` — asserts `client_did == None` for a
blank/whitespace buffer and `client_did == Some("...")` (trimmed) for a filled one, directly on
`mint_command`. Inverting either branch in `mint_command` turns that test red. Because both call sites
now call this exact function (no local logic of their own left to diverge), the existing test's coverage
transfers to both GUI surfaces without duplicating it. No new test added for this fix — writing one would
re-assert the same branch the existing test already pins (tautological per the project's own flow-tests
rule). Findings 1, 3, 4 are refactors with no behavior change (same output for same input, verified by
reading the diff, not by a new test). Finding 2 is a `Cargo.toml` dependency-section move with no Rust
logic to test. Finding 5 is a lint `allow`, no behavior change.

## Scope note

Per the R4 brief, this slice ran clippy and fmt (which it owns) but **no tests** — R5 runs the full sweep.
All fixes above compile clean under `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` and
`cargo +1.99.0 fmt --all -- --check`, both re-verified after every fix round.
