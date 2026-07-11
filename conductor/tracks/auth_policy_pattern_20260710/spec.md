---
type: Track Spec
title: Policy Engine — Unified Authorization for Layered Tokens and Entry Points
tags: [spike, spec-only, auth_policy_pattern_20260710]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Specification: Policy Engine

**Track ID:** `auth_policy_pattern_20260710`
**Type:** Spec / design (no implementation this round)
**Status:** Draft
**Goal alignment:** 3D P2P analytics engine on the hexon format, with an extension storage/query API and Rhai/WASM scripting — this track is the authorization backbone that lets the extension API (`analytics_extension_api_20260710`) and every other entry point share one deny-by-default model.

## Overview

FractalEngine has accumulated at least seven independent authorization
surfaces over Waves 1-3, each re-implementing some flavor of "who is allowed
to do this." This spec surveys them (read-only, verified 2026-07-10) and
proposes a single `Policy` abstraction that every entry point calls into,
instead of hand-rolling its own role comparison.

## Survey of existing authorization layers (verified 2026-07-10)

| Layer | Location | Shape | Notes |
|---|---|---|---|
| Session/handshake | `fe-auth/src/{handshake,session,cache,revocation}.rs` | Session cache + revocation, JWT-issued | Governs whether a peer connection is authenticated at all |
| Identity | `fe-identity/src/{jwt,did_key,api_token}.rs` | `did:key` + JWT `sub` claim; `ApiClaims` for API tokens | Identity layer only — does not itself decide authorization |
| DB-layer RBAC | `fe-database/src/{rbac,role_level,role_manager}.rs` | `RoleLevel` enum (`Owner>Manager>Editor>Viewer>None`) in `role_level.rs`, **but** `rbac.rs::require_write_role` uses a separate hardcoded `const WRITE_ROLES: &[&str] = &["owner", "manager", "editor"]` string list | Two different representations of "role" in the same crate — `RoleLevel` (typed, ordered) vs raw string list (`rbac.rs`) — itself evidence of the scattering this track addresses |
| API tokens | `fe-database/src/api_token_store.rs`, `fe-identity/src/api_token.rs` | Scoped API tokens, separate from session JWTs | A third token type with its own scope-check logic |
| API gateway entry point | `fe-api/src/auth.rs::{require_role, require_scope, require_role_and_scope}` | Standalone functions taking `&ApiClaims` and a `min_role: &str` | Re-implements role comparison against `ApiClaims` rather than reusing `RoleLevel`'s `Ord` |
| Hexon registry handlers | `fe-hexon/**` | **No role/scope/RBAC references found anywhere in the crate** | Confirmed gap — Phase 8.4 review already flagged "RBAC not enforced in fe-hexon"; this survey re-confirms it as of 2026-07-10 |
| Plugin capabilities | `fe-plugin/src/capability.rs::{CapabilityManifest, CapabilityToken}` | Named capability strings (`"storage.read"`, `"query.select"`, etc.), fail-closed (`has_capability`) | The **best-designed** of the seven — already deny-by-default, already the pattern `analytics_extension_api_20260710`'s `HostEnv` reuses. The policy engine should generalize this shape, not replace it. |
| UI tab gating | `fe-webview/src/petal_portal.rs::{TabVisibilityFilter, VisibleTabs}` | Role-gated set of visible `BrowserTab` variants | UI-layer enforcement, redundant with (and trusting) whatever gated access to the underlying data |

Seven surfaces, at least four distinct "role" representations (`RoleLevel`
enum, raw role-name strings, `ApiClaims` fields, named capability strings),
and one confirmed enforcement gap (fe-hexon). This is the concrete case for
a single evaluation point.

## Target shape

### The `Policy` abstraction

```rust
trait Policy {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision;
}
```

- `AuthContext` — unifies whatever the caller already has: `did:key` from a
  session JWT, an `ApiClaims` token, or a plugin's `CapabilityToken`. Not a
  new identity system — a thin enum/struct wrapping the existing three.
- `Action` — a verb (`Read`, `Write`, `Query`, `Install`, `Publish`, ...).
- `Scope` — reuses the existing hierarchical scope string convention
  already established in `AGENTS.md`/MEMORY.md
  (`VERSE#v-FRACTAL#f-PETAL#p`), not a new addressing scheme.
- `Decision` — `Allow` or `Deny(reason)`. No implicit allow; absence of a
  matching policy is `Deny`.

### Composable policy objects

Each existing layer becomes one small `Policy` implementation composed
together (e.g. `AnyOf`/`AllOf` combinators), rather than one monolithic
rule set:

- `RoleLevelPolicy` — wraps `fe-database::role_level::RoleLevel`'s existing
  `Ord` comparison (the one correct typed representation found in the
  survey) — replaces both `rbac.rs`'s string list and `fe-api/src/auth.rs`'s
  `require_role`.
- `TokenScopePolicy` — wraps `ApiClaims`/API-token scope matching.
  `CapabilityPolicy` — wraps `fe-plugin`'s existing `CapabilityToken`
  unchanged (it is already the right shape; this policy is a thin adapter,
  not a reimplementation).
- `OwnershipPolicy` — for resource-ownership checks (e.g. a publisher DID
  matching a hexon's `publisher_did`).

### Single decision point per entry point

Every entry point becomes a thin adapter calling one `Policy::evaluate`:

- API gateway (`fe-api`) — replace `require_role`/`require_scope`/
  `require_role_and_scope` with calls into the engine.
- DB command dispatch (`fe-database`) — replace `rbac.rs::require_write_role`'s
  string-list check with `RoleLevelPolicy` via the engine.
- Hexon registry HTTP handlers (`fe-hexon`) — **add** engine calls where
  none exist today (this is the gap closure).
- Plugin host function registration (`fe-plugin`) — `CapabilityPolicy`
  becomes the engine-native form of what `host_env.rs::require()` already
  does inline; same behavior, one shared implementation.
- UI gating (`fe-webview::TabVisibilityFilter`) — becomes a read of the
  engine's decision for the tabs in question, not its own role comparison.

### Deny-by-default + decision logging

Every `Policy::evaluate` call is logged (subject, action, scope, decision,
reason) via `tracing`, matching the existing `general.md` "Failure
Transparency" rule ("Log all security-relevant events"). No entry point may
short-circuit to `Allow` without going through the engine — this is the
enforceable, greppable rule (see Acceptance Criteria).

## Acceptance Criteria (for the eventual implementation track)

- The fe-hexon registry enforcement gap (Phase 8.4) is closed by wiring
  `fe-hexon`'s HTTP handlers through the policy engine — this is the
  concrete, testable proof the engine works, not just a diagram.
- No entry point performs an ad-hoc role/scope comparison outside the
  engine — greppable: no new occurrences of a hardcoded role-name string
  list (like today's `rbac.rs::WRITE_ROLES`) or a standalone `require_*`
  function outside the policy crate.
- Policy decisions are unit-testable without I/O — `Policy::evaluate` takes
  plain data (`AuthContext`, `Action`, `Scope`) and returns `Decision`; no
  database or network call is required to test a policy's logic (the DB
  call to *fetch* the subject's role happens before `evaluate`, not inside
  it — mirrors how `fe-plugin::CapabilityToken` is already minted once and
  then checked synchronously many times).
- Deny-by-default: a resource/action pair with no matching policy is
  `Deny`, verified by a test with an empty policy set.

## Out of Scope (this spec)

- Implementation — a future track, scoped once this design is reviewed.
- Replacing `fe-plugin`'s `CapabilityManifest`/`CapabilityToken` types
  themselves (they are already correct) — only generalizing the *pattern*
  they represent to the other six layers.
- New identity/credential systems — this reuses `did:key`, JWT, and API
  tokens exactly as they exist today.
- UI redesign of any gated surface.

## Dependencies / Related Tracks

- `analytics_extension_api_20260710` — its `HostEnv` capability-gating
  pattern is the model this engine generalizes; once the engine exists,
  `HostEnv::require()` should become a thin call into it rather than its own
  parallel implementation.
- Fixes the known gap noted in `crate_registry_20260508` (Phase 8.4:
  "RBAC not enforced in fe-hexon").

## Open Questions

1. Does `AuthContext` need to represent "no subject" (anonymous/public
   access) as a first-class variant, or is public access simply "a subject
   whose `RoleLevel` is `None`"? The existing `role_level.rs` `RoleLevel::None`
   suggests the latter — worth confirming before implementation.
2. Where does the engine live — a new `fe-policy` crate, or inside
   `fe-auth`? A new crate avoids `fe-database`/`fe-api`/`fe-hexon`/`fe-plugin`
   all depending on each other transitively just to share the engine; likely
   the right call, but should be confirmed against the dependency graph in
   `AGENTS.md` before implementation starts.
3. Performance: `fe-plugin`'s capability check is already synchronous/in-memory
   (`CapabilityToken::has_capability`). `RoleLevelPolicy`/`TokenScopePolicy`
   need the subject's role fetched from SurrealDB first — should the engine
   itself do async fetching, or strictly require pre-fetched `AuthContext`
   (favored, per the "unit-testable without I/O" acceptance criterion above)?
