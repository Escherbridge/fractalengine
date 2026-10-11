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

- [x] Task 3.1: fe-api `auth.rs` (`require_role`/`require_scope`/`require_role_and_scope`) → engine calls (fe-api owned by another agent this round). — AUDIT 2026-10-10: PARTIAL — hexon routes already authorize through the engine (`fe-api/src/hexon.rs:158-184` `require_hexon_petal_access` → `fe_hexon::authz::authorize`, landed 916a3ae; fe-api depends on fe-policy), but `fe-api/src/auth.rs:55-90` `require_role`/`require_role_level`/`require_scope`/`require_role_and_scope` are still standalone role/scope comparisons with no `fe_policy` call. Still open. **DONE 2026-10-11 (commit pending, AP-3.1):** `require_role_level` → `RoleFloorPolicy`, `require_scope` → `TokenScopePolicy`, `require_role_and_scope` → one `AllOf` through a `PolicyEngine`; `api_token_context` is the single subject builder (hexon reuses it); ws.rs Editor gates route through `require_role_level`. Tests: `fe-api/src/auth.rs` `helper_table_evaluates_through_policies`; fe-api integration suite green (gate for the verify scripts).
- [x] Task 3.2: fe-hexon-registry HTTP service routes through the `*_as` gated registry variants. — AUDIT 2026-10-10: DONE BY SUPERSESSION — the engine's hexon HTTP API (`fe-api/src/hexon.rs:318` `uninstall_as`, `:364`/`:560` `search_local_in_petal_as`, `:399` `list_installed_as`, `:429` `get_installed_as`) routes through the gated variants since 916a3ae; the standalone `fe-hexon-registry` service holds no `HexonRegistry` and is a shared-bearer-token dev fixture host by design (`fe-hexon-registry/AGENTS.md` §auth model).
- [x] Task 3.3: Plumb peer roles to the sync thread; flip `PolicyHandle` to `strict()`. — AUDIT 2026-10-10: DONE BY SUPERSESSION (F3) — peer admission moved to the DB thread instead of the sync thread: `fe-database/src/handlers/replicated_row.rs:261` `admit_inbound_row` resolves the author's role from local tables and requires Editor+ via the fe-policy write gate (commits 20c3a96, 5f0fcdc); `fe-sync/src/AGENTS.md` §write-policy records `PolicyHandle` as having no production caller.
- [x] Task 3.4: `TokenScopePolicy` (ApiClaims scope matching) + `OwnershipPolicy` (publisher DID). — AUDIT 2026-10-10: OPEN — no Rust definition of either type exists; `fe-policy/src/policies.rs` has only RoleLevelPolicy/PublicReadPolicy/CapabilityPolicy; `fe-policy/AGENTS.md:56` still lists them as deferred. **DONE 2026-10-11 (commit pending, AP-3.4):** `TokenScopePolicy` + `OwnershipPolicy` (+ `RoleFloorPolicy`) in `fe-policy/src/policies.rs`, `scope_contains` moved to fe-policy. Production callers: `fe-api/src/auth.rs::require_scope` (proved by `fe-api/tests/api_integration.rs::foreign_verse_token_is_denied_by_token_scope_policy`) and `fe-hexon/src/authz.rs::publish_as`/`uninstall_as` (proved by `republish_by_a_different_signer_is_denied`, `only_the_publisher_may_unpublish_the_global_record`).
- [ ] Task 3.5: Causal-DAG membership ops + strong-removal resolver — BLOCKED on per-op ed25519 signing (`hexon_delta_format_20260710`, decisions §D5-1). — AUDIT 2026-10-10: STALE — superseded by `canonical_data_log_20260808`: signed operations, capability chains, epochs and persistent revocation are SPEC-1/2/3 (D-CL3, D-CL18; `fe-canonical-log/src/{signing.rs,capability/,author_key/}`, `fe-database/src/canon_log/`), and D-CL24(5) rules legacy op_log rows permanently unsigned, so the hexon_delta_format signing route will never unblock this item. Re-home any residual membership work under that program.
