---
type: Workflow
title: FractalEngine Project Workflow
tags: [workflow, conductor, tracks, testing, commits]
timestamp: 2026-07-17T00:00:00Z
resource: ./tracks.md
---

# Project Workflow

## Guiding Principles

1. **The track is the source of truth:** all work is tracked in
   `tracks/<id>/` — the track's `metadata.json` (machine state) + `plan.md`
   (human plan). There is no single global plan file.
2. **The tech stack is deliberate:** changes to the tech stack are documented
   in [`tech-stack.md`](./tech-stack.md) _before_ implementation.
3. **New behavior ships with tests:** every feature or fix lands with tests
   that pin its behavior. No numeric coverage gate.
4. **One sweep at the end:** apply a batch of changes first, verify once —
   not test-fix-test loops per change.
5. **User experience first:** every decision prioritizes user experience;
   terminology follows [`product-guidelines.md`](./product-guidelines.md).
6. **Non-interactive & CI-aware:** prefer non-interactive commands; use
   `CI=true` for watch-mode tools to ensure single execution.

## Track workflow

Standing rule: **feature → track at start, retro + archive at completion.**

1. **Create the track.** A new feature or bug batch gets a folder
   `tracks/<id>/` (id = `<slug>_<YYYYMMDD>`) containing `spec.md`, `plan.md`,
   and `metadata.json` — created via `conductor-okf:new-track`.
2. **metadata.json is the machine source of truth.** Status canon:
   `pending` | `in_progress` | `spec_only` | `done` | `superseded`.
   Dependencies live in `depends_on` / `blocks`. Each track carries an
   `alignment` field: its verdict + priority against
   [`roadmap.md`](./roadmap.md).
3. **tracks.md is the live board**, ordered by the roadmap go-forward slate.
   Board lines summarize; the track folder holds the detail.
4. **All conductor markdown carries OKF YAML frontmatter** (required field:
   `type`). Add it lazily when touching a file that lacks it.
5. **Completion = retro + archive.** Write the retro (in the track folder or
   a consolidated batch retro), move the folder to `tracks/_archive/<id>/`,
   set `archived: true` + `archived_at` in `metadata.json`, and collapse the
   board to one line per archive batch.
6. **Decisions needing user sign-off go to the ratification register**
   (`tracks/outstanding_decisions_20260715/spec.md`). Register entries are
   **never treated as settled until the user ratifies them** — do not state
   an unratified decision as fact in any doc or commit message.

## Verification & quality gates

The practiced gate is a **single integrated sweep at the end of a change
batch** — not per-task loops:

```bash
cargo test --workspace
cargo clippy -- -D warnings
cargo fmt --check
```

- Apply all fixes in the batch first; run the sweep once at the very end.
- Exception: changes that touch the test harness itself may run their own
  file inline once to confirm the harness still works.
- New behavior ships with tests; there is no numeric coverage gate and no
  tarpaulin step.
- **In-app verification is user-gated:** when a track needs manual in-app
  checks, mark it done-pending-user-verify, list the manual steps in the
  track folder, and continue — do not pause the session waiting for
  confirmation.

Per-change checklist (verified by the sweep + review, not re-run per fix):

- [ ] All tests pass (`cargo test --workspace`)
- [ ] `cargo fmt --check` and `cargo clippy -- -D warnings` pass
- [ ] Terse one-line doc comments on public items; rationale in the
      directory-level `AGENTS.md`, not inline comment blocks
- [ ] No `unwrap()` or `expect()` in production code paths
- [ ] All gossip messages carry ed25519 signatures (enforced by type system)
- [ ] No `block_on()` calls inside Bevy systems
- [ ] Authorization routed through `fe-policy` — never checked in Bevy
      systems or UI code
- [ ] Security-relevant events logged via `tracing`
- [ ] No hardcoded secrets or private key material in code

## Commits

- **One commit per coherent batch** (feature slice or fix wave), conventional
  message — e.g. `fix(ux): 2026-07-16 UX-testing batch — path interaction,
  stamp persistence, ...`.
- Verification evidence lives in the track folder (`retro.md` /
  `metadata.json`), **not** git notes.
- Conductor bookkeeping (track creation, board updates, archives) commits
  under the `conductor` type/scope.

### Message format

```
<type>(<scope>): <description>
```

### Types

- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `style`: Formatting only
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `test`: Adding or updating tests
- `chore`: Maintenance tasks
- `conductor`: Conductor track/board updates

### Scopes

Any workspace crate name from the root `Cargo.toml` members list (22 crates):

`fractalengine`, `fractalengine-relay`, `fe-runtime`, `fe-network`,
`fe-database`, `fe-identity`, `fe-renderer`, `fe-webview`, `fe-sync`,
`fe-ui`, `fe-test-harness`, `fe-api`, `fe-format`, `fe-entity-store`,
`fe-query`, `fe-terrain`, `fe-hexon`, `fe-hexon-registry`, `fe-plugin`,
`fe-plugin-test`, `fe-sdk`, `fe-policy`

— plus the non-crate scopes `ci`, `ux`, `docs`, `conductor`.

### Examples

```bash
git commit -m "feat(fe-network): Add libp2p Kademlia DHT peer discovery"
git commit -m "feat(fe-api): Add signed share URLs for query egress"
git commit -m "fix(fe-webview): Block RFC 1918 addresses in navigation handler"
git commit -m "conductor(track): Archive analytics_egress batch with retro"
```

## Development Commands

### Setup

```bash
rustup toolchain install stable
rustup component add rustfmt clippy
cargo build
```

### Daily Development

```bash
# Run with logging
RUST_LOG=debug cargo run

# End-of-batch sweep (run ONCE per change batch)
cargo test --workspace
cargo clippy -- -D warnings
cargo fmt --check
```

## Testing Requirements

### Unit Testing

- Every module has corresponding tests in `#[cfg(test)]` blocks.
- Use `tokio::test` for async functions in network/database/sync crates.
- Mock external peers using in-process channels; never require a live network
  for unit tests.
- Test both success and failure cases for all auth, crypto, and RBAC paths.

### Integration Testing

- Integration tests in `tests/` at the crate root; cross-crate flows in
  `fe-test-harness`.
- Test the session flow end to end (JWT issue → role assign → verify →
  revoke via `fe_database::session_cache`).
- Test RBAC enforcement through `fe-policy` (None/Viewer cannot write, Editor
  can within scope, Owner has full access).
- Test WebView IPC command dispatch for all typed command variants.

### Security Testing

- Every signature verification function has a test that passes a tampered
  message and asserts rejection.
- WebView URL denylist has tests for localhost, 127.0.0.1, and each RFC 1918
  range.
- JWT expiry and revocation propagation have integration tests with
  synthetic clock advancement.

## Definition of Done

A track (or batch within it) is complete when:

1. All code implemented to the track's spec
2. New behavior covered by tests
3. The end-of-batch sweep is green (`cargo test --workspace`, clippy
   `-D warnings`, `fmt --check`)
4. Terse doc comments present; rationale captured in the directory
   `AGENTS.md` where non-obvious
5. Security rules verified (signatures, fe-policy placement, no unsafe in
   auth paths)
6. `metadata.json` status updated; retro written at archive time
7. Changes committed as a coherent batch with proper message format
8. Any decision needing user sign-off is filed in the ratification register,
   not assumed
