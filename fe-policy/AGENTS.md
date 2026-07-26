# fe-policy — module rationale

Implementation of `conductor/tracks/auth_policy_pattern_20260710` (minimal
slice). One deny-by-default `Policy::evaluate(subject, action, resource)`
seam replacing the eight ad-hoc authorization surfaces surveyed in the spec.

## §design constraints

- **No I/O, no Bevy, serde-light.** Policies evaluate pre-fetched plain data
  (`AuthContext` carries the role the caller already looked up). The DB call
  to fetch a role happens *before* `evaluate`, never inside it — this is the
  spec's "unit-testable without I/O" acceptance criterion, and mirrors how
  `fe-plugin::CapabilityToken` is minted once then checked synchronously.
- **Deny-by-default everywhere.** An empty `PolicyEngine`, empty `AnyOf`,
  empty `AllOf`, and an unmapped `RoleLevelPolicy` action all deny. Tests in
  `engine.rs`/`combinators.rs`/`policies.rs` prove each.
- **Every decision is logged** (`engine::log_decision`): allow at `debug`,
  deny at `warn` (security event, per general.md Failure Transparency).

## §role-level

`RoleLevel` moved here verbatim from `fe-database/src/role_level.rs` so the
engine could depend on it without `fe-database` ⇄ `fe-policy` circularity
(fe-database now depends on fe-policy and re-exports the type, keeping
`fe_database::RoleLevel` paths alive). Ordering semantics unchanged:
Owner > Manager > Editor > Viewer > None; `"public"` parses as Viewer.

## §types

`AuthContext` is a thin enum over the three existing identity shapes
(did:key/JWT, API token, plugin capability token) plus `Anonymous` — the
spec's open question 1 resolved as: anonymous is a first-class variant whose
`role()` is `RoleLevel::None`. `Scope` wraps the established hierarchical
scope string convention; it is not a new addressing scheme.

## §engine / §combinators

`PolicyEngine` is an any-allows set with the decision log attached; it also
implements `Policy` so engines nest. `AnyOf`/`AllOf` are the composition
primitives. `PermissiveMigrationPolicy` exists solely for gates whose inputs
are not plumbed yet (fe-sync write path today): it warn-logs every would-be
denial and allows, so the gate ships before enforcement flips.

## §policies

- `RoleLevelPolicy` — action → minimum `RoleLevel` map; `standard()` is the
  canonical verb map (Read/Query=Viewer+, Write/Install=Editor+,
  Publish/Manage=Manager+). Replaces `rbac.rs::WRITE_ROLES`.
- `PublicReadPolicy` — the only place "anonymous read is public" is stated;
  compose with `RoleLevelPolicy` via the engine for public-discovery
  surfaces (hexon registry list/search).
- `CapabilityPolicy` — thin adapter over `CapabilityToken` grants carried in
  `AuthContext::Capability`; does not replace fe-plugin's types (spec: they
  are already the right shape).

Deferred (not this slice): `TokenScopePolicy`, `OwnershipPolicy`, the
fe-api adapter, and causal-DAG membership ops (blocked on per-op signing,
decisions §D5-1).

## §node-lifecycle (`node_lifecycle.rs`)

Track `node_lifecycle_addressing_20260725` FR-1. Node delete/cascade and stamp
promotion are mutations, so they authorize at **Editor+** (`MIN_DELETE_ROLE`)
through the existing `RoleLevelPolicy::standard()` `Write` gate — no new
`Action` variant, no bespoke role math. A Viewer/Anonymous subject is denied by
construction (deny-by-default).

**Where they are invoked.** The DB dispatch loop (`fe-database/src/lib.rs`)
calls `authorize_node_delete` / `authorize_instance_promotion` on the
`TombstoneNode` / `CascadeTombstoneNode` / `PromoteInstance` arms *before* any
mutation. Each command carries a `fe_runtime::messages::CallerAuth` (the caller's
already-resolved role at the scope — UI local user or API-token subject); the
loop maps it to an `AuthContext` (`caller_auth_to_context`), resolves the node's
scope, and rejects sub-Editor callers with a `DbResult::Error` (no row touched).
The rejection path is exercised end-to-end in
`fe-database` `runtime_lifecycle_tests` (Anonymous + Viewer → denied, node
survives). N-5: authz lives here, never in the UI; the UI receives only
pre-authorized deletes.
