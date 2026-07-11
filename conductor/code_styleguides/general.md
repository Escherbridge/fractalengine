---
type: Code Styleguide
title: General Code Style Principles
tags: [enforcement, 2026-07-10]
timestamp: 2026-07-10T00:00:00Z
---

# General Code Style Principles

This document outlines general coding principles that apply across all modules in FractalEngine.

## File Size — Small, Domain-Specific Files

- **Soft cap: ~300 lines per file.** A file that grows past this is doing too
  many things; split it by domain (one file per resource cluster, per dialog
  variant, per chained system — not by arbitrary line count).
- **God-files get decomposed, not tolerated.** If you're about to add the
  next feature to an already-large file, that is the signal to split first,
  add second. See `conductor/tracks/feui_decomposition_20260710/` for the
  worked example (fe-ui's `panels.rs`/`plugin.rs`/`dialogs.rs`/
  `verse_manager.rs`/`node_manager.rs`, all 800-2000 lines, being split by
  domain).
- **Reviewer check:** if a diff adds >50 lines to a file already over 300
  lines, flag it — the addition likely belongs in a new sibling module
  instead.

## Naming and Algorithmic Quality

- **Self-explaining names over comments.** A well-named function/variable
  needs no comment to explain what it does; a comment should only explain
  *why*, and even that belongs in the directory's `AGENTS.md` (see
  Documentation below), not inline.
- **Algorithmic excellence over cleverness.** Prefer the straightforward,
  correct-complexity solution over a clever one-liner. "Clever" that
  obscures cost is a bug waiting to happen — see
  `conductor/tracks/code_review_20260430_performance_hotpaths/` (an O(n³)
  tree walk that looked simple but wasn't cheap) as the standing
  counter-example to avoid repeating elsewhere.

## Readability

- Code should be easy to read and understand by humans.
- Avoid overly clever or obscure constructs.
- Prefer explicit over implicit — make intent visible in the code itself.

## Consistency

- Follow existing patterns in the codebase.
- Maintain consistent formatting, naming, and structure.
- When in doubt, match the surrounding code.

## Simplicity

- Prefer simple solutions over complex ones.
- Break down complex problems into smaller, manageable parts.
- The right amount of abstraction is the minimum needed for the current task.

## Maintainability

- Write code that is easy to modify and extend.
- Minimize dependencies and coupling between modules.
- Prefer composition over inheritance.

## Documentation

- **Doc comments are terse, one line, and say *what*.** (Supersedes any
  older "doc comments explain WHY" guidance in this repo — that guidance is
  now wrong.) Every public API surface still needs a doc comment, but keep
  it to one line.
- **Design rationale, the "why", and cross-cutting notes live in a
  directory-level `AGENTS.md`** next to the source, not in a multi-paragraph
  `///`/`//!` block. See `fe-database/src/AGENTS.md` for the canonical
  example (`§geometry-inserts` — a real incident writeup, one line pointer
  from the code, full story in the doc).
- **Reviewer check / greppable pattern:** a new `//!` or `///` block that
  spans more than 1-2 lines in a diff should be redirected into the
  directory's `AGENTS.md` with a one-line pointer left in the code (e.g.
  `// see fe-database/src/AGENTS.md §geometry-inserts`).
- Keep documentation up-to-date with code changes.

## Data Access — Typed Writes for Schema-Typed Tables

- **Schema-typed columns (e.g. SurrealDB `geometry<point>`, `geometry<polygon>`)
  require hand-written, explicitly-cast queries** (`<geometry<point>> [$x, $z]`,
  `.check()`ed) — never a generic query-builder's `InsertBuilder`/`UpdateBuilder`,
  which cannot emit the cast and will silently fail the schema check.
- **Generic query builders (`fe-query::QueryBuilder`/`InsertBuilder`/etc.)
  are for the read/analytics lane only** — non-geometry tables, ad-hoc
  filters, extension-facing query APIs. They are not a substitute for
  hand-written writes on schema-typed tables.
- **The incident:** a 2026-05 refactor (commit `059f381`) routed geometry
  inserts through `InsertBuilder`, which rendered a plain `CREATE {table}
  CONTENT $p0` with no cast — every node/petal creation silently failed for
  weeks, compounded by `exec_query` not calling `.check()` so the failure
  never surfaced. Full incident writeup and the fix: `fe-database/src/AGENTS.md`
  `§geometry-inserts`.
- **Reviewer check / greppable pattern:** `InsertBuilder` (or `UpdateBuilder`)
  appearing anywhere under `fe-database/src/handlers/` for a geometry-typed
  table is a regression of this exact incident — flag it.

## Extension-Facing APIs — Fail-Closed Capability Gating

- Any API surface an extension (Rhai script or WASM component) can call
  must be **deny-by-default**: an absent capability grant means the call is
  rejected, not silently no-op'd and not implicitly allowed.
- Follow the existing pattern in `fe-plugin/src/capability.rs`/`host_env.rs`
  (`CapabilityManifest`, `CapabilityToken::has_capability`,
  `HostApiError::NotAvailable`/`CapabilityDenied`) rather than inventing a
  new gating mechanism per extension surface.
- **Reviewer check:** a new extension-facing function that reads/writes
  engine state without a `require(token, CAPABILITY)`-style check at the top
  is a fail-open bug.

## Authorization — Central Policy Engine, Not Ad-Hoc Role Checks

- Authorization decisions go through the central policy engine (see
  `conductor/tracks/auth_policy_pattern_20260710/spec.md` for the design —
  spec-only as of 2026-07-10, implementation to follow). Entry points are
  thin adapters over one `Policy::evaluate(subject, action, resource)` call,
  not independent role/scope comparisons.
- **Deny-by-default.** No matching policy means deny, not allow.
- **Do not hand-roll a new role check.** If you're about to write
  `if role >= X` or a hardcoded list of allowed role strings in a new entry
  point, that logic belongs in (or behind) the policy engine — see the
  survey in `auth_policy_pattern_20260710/spec.md` for why this matters:
  FractalEngine already has at least four different ad-hoc representations
  of "role" scattered across `fe-database::rbac`/`role_level`, `fe-api::auth`,
  and one crate (`fe-hexon`) with no role check at all.
- **Reviewer check / greppable pattern:** a new hardcoded `const *_ROLES:
  &[&str]` list, or a new standalone `require_*(...)` function outside the
  policy engine, is exactly the pattern this rule exists to stop repeating
  (`fe-database/src/rbac.rs::WRITE_ROLES` is the existing example to not
  copy elsewhere).

## Safety & Security (Priority Rules for FractalEngine)

These rules are non-negotiable and take precedence over all other style guidance.

### Network Safety
- **No unsigned messages accepted.** All gossip payloads must carry an ed25519 signature. Reject unsigned or unverifiable messages at the ingest boundary — never pass them into application logic.
- **No localhost or RFC 1918 URLs in WebView.** Block `127.0.0.1`, `localhost`, `10.x.x.x`, `172.16.x.x–172.31.x.x`, and `192.168.x.x` unconditionally in the WebView navigation handler.
- **No raw eval.** The JS-to-Rust IPC bridge is a typed command enum. No `eval()`, `Function()`, or dynamic code execution across the WebView boundary.
- **Rate-limit all peer inputs.** Every inbound channel from a peer has a configurable cap. Drop the oldest messages on overflow — never block the render loop waiting for a slow peer.

### RBAC Transparency
- **RBAC is enforced at the database layer only.** Bevy systems must never implement permission checks. If a system receives data from SurrealDB, it is already authorised. Do not add runtime permission checks in ECS systems.
- **Every role assignment and revocation is logged.** Write to the op-log before applying the change. The log entry must exist even if the subsequent write fails.
- **Revocations propagate immediately.** A revoked session must be broadcast via iroh-gossip before the local SessionCache is updated. Order: sign revocation → broadcast → flush cache.

### Cryptographic Transparency
- **Use `verify_strict()` not `verify()`** from ed25519-dalek. Always.
- **Never expose raw private key material** in logs, error messages, or UI surfaces.
- **JWT `sub` field must always be `did:key:<multibase_pub>`.** A JWT without a DID-compatible subject is invalid.

### Failure Transparency
- **Log all security-relevant events** to the Node's internal log: peer connect/disconnect, JWT issue/verify/reject, role assign/revoke, revocation broadcast, WebView navigation blocked.
- **Never silently swallow errors** in security-critical paths. If a signature verification fails, log it with the peer's public key and the reason. Do not just return `false`.

## Test Policy — Fixes First, One Sweep at the End

- For any multi-fix workflow (review triage, refactor sweeps, bug-fix
  passes): **apply all fixes first, then run the full test/lint/typecheck
  sweep once at the end.** Do not re-run tests after each individual fix, and
  do not iterate test → fix → test → fix in tight loops. Exception: when a
  fix touches the test harness itself (test setup, mock infrastructure),
  running just that file inline once to confirm the harness still works is
  reasonable.
- **Handler success must mean persisted state.** A DB/API handler returning
  `Ok` is not sufficient evidence the write happened — write a read-back
  test that queries the row/value back from the actual store, don't trust
  the handler's own return value as the assertion. This is not a hypothetical:
  see `fe-database/src/AGENTS.md` `§geometry-inserts` — a handler returned
  `Ok` for weeks while every write silently failed, because nothing read the
  data back to check. `fe-database/tests/db_test.rs` now does this for the
  geometry insert paths specifically; extend the same pattern to any new
  handler that persists state.

## Enforcement Checklist (Reviewer Quick-Reference)

One line per rule, with the greppable pattern that flags a likely violation:

| Rule | Greppable pattern / check |
|---|---|
| File soft cap ~300 lines | `wc -l` on changed files; >300 (or +50 to an already-large file) → flag |
| Terse one-line doc comments | Multi-line `//!`/`///` block added in a diff → redirect to directory `AGENTS.md` |
| Typed writes for schema-typed tables | `InsertBuilder`/`UpdateBuilder` appearing under `fe-database/src/handlers/` for a geometry column → flag (see `fe-database/src/AGENTS.md` `§geometry-inserts`) |
| Fail-closed extension APIs | New extension-facing fn with no `require(token, CAPABILITY)` check → flag |
| Central policy engine | New hardcoded `const *_ROLES: &[&str]` or standalone `require_*(...)` fn outside the policy engine → flag |
| Handler success = persisted state | New handler that writes state with no corresponding read-back test → flag |
| Test sweep timing | Tests re-run after every individual fix instead of once at the end of a multi-fix pass → flag in review process, not code |
