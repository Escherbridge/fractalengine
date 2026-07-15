---
type: Track Plan
title: Policy Engine — implementation plan (minimal slice)
tags: [auth_policy_pattern_20260710, implementation]
timestamp: 2026-07-15T00:00:00Z
resource: ./metadata.json
---

# Plan: Policy Engine — minimal implementation slice

Promotion of the spec (see `spec.md`, incl. 2026-07-11 §D1 amendment) to a
first implementation slice. Causal-DAG membership stays out of this slice
(blocked on per-op signing, decisions §D5-1).

## Phase 1 — fe-policy crate (engine core)

- [x] Task 1.1: Create `fe-policy` crate; add workspace member to root `Cargo.toml`.
- [x] Task 1.2: Types — `AuthContext` (Anonymous/Did/ApiToken/Capability), `Action`, `Scope`, `Decision` (`types.rs`).
- [x] Task 1.3: `Policy` trait + `PolicyEngine` with DENY-BY-DEFAULT (empty set denies — proven by tests) and tracing decision log (allow=debug, deny=warn) (`engine.rs`).
- [x] Task 1.4: `AnyOf`/`AllOf` combinators + `PermissiveMigrationPolicy` (`combinators.rs`).
- [x] Task 1.5: `RoleLevelPolicy` (standard verb map), `PublicReadPolicy`, `CapabilityPolicy` (`policies.rs`).
- [x] Task 1.6: Move canonical `RoleLevel` into fe-policy (`role_level.rs`); fe-database re-exports.

## Phase 2 — adapters

- [x] Task 2.1: fe-database `rbac.rs` — delete `WRITE_ROLES`, delegate `require_write_role` to the engine; public signatures stable; pure `evaluate_write` + tests.
- [x] Task 2.2: fe-hexon Phase 8.4 gap — `authz.rs`: `install_as`/`uninstall_as` (Editor+), public `list_installed_as`/`search_local_as`; tests: allowed role passes, insufficient denied, absent auth denied, anonymous read public.
- [x] Task 2.3: fe-sync §D1 write gate — `write_policy.rs` `PolicyHandle` (Resource); `handle_write_row_entry` gated; default `permissive_migration()` (warn-logs would-be denies) until role plumbing lands.
- [x] Task 2.4: fe-plugin — `host_env.rs::require()` delegates to `CapabilityPolicy` engine via `CapabilityToken::to_auth_context()`.
- [x] Task 2.5: fe-webview — `TabVisibilityFilter::can_view_config()` reads engine `Manage` decision instead of local role comparison.

## Phase 3 — remaining adapters (follow-ups, NOT this slice)

- [ ] Task 3.1: fe-api `auth.rs` (`require_role`/`require_scope`/`require_role_and_scope`) → engine calls (fe-api owned by another agent this round).
- [ ] Task 3.2: fe-hexon-registry HTTP service routes through the `*_as` gated registry variants.
- [ ] Task 3.3: Plumb peer roles to the sync thread; flip `PolicyHandle` to `strict()`.
- [ ] Task 3.4: `TokenScopePolicy` (ApiClaims scope matching) + `OwnershipPolicy` (publisher DID).
- [ ] Task 3.5: Causal-DAG membership ops + strong-removal resolver — BLOCKED on per-op ed25519 signing (`hexon_delta_format_20260710`, decisions §D5-1).
