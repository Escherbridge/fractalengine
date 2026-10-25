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

- `RoleFloorPolicy(RoleLevel)` — action-agnostic "role ≥ floor". Exists
  because fe-api's `require_role_level(Owner)` has no verb in
  `RoleLevelPolicy::standard()` (Owner is unmapped there). A floor of
  `RoleLevel::None` admits everyone — the historical `require_role("typo")`
  admit-all semantics, documented at `fe-api/src/auth.rs`, kept on purpose.
- `TokenScopePolicy` — action-agnostic API-token scope gate: allows iff the
  subject is `AuthContext::ApiToken` and one of its `scopes` contains the
  resource (`scope_contains`). Did/Anonymous/Capability subjects deny with
  "token-scope: subject carries no token scope"; `Scope::global()` (`*`) is
  never covered (deny-by-default). **Production caller:**
  `fe-api/src/auth.rs::require_scope` (and `require_role_and_scope`), the
  gate under every REST/WS/MCP scope check; proven by
  `fe-api/tests/api_integration.rs::foreign_verse_token_is_denied_by_token_scope_policy`.
- `OwnershipPolicy::new(owner_did)` — owner-only gate. Ownership is
  pre-fetched resource data carried in the policy instance (§design
  constraints: no I/O); allows iff `Did { did }` or `ApiToken { subject }`
  equals the owner. **Production caller:**
  `fe-hexon/src/authz.rs::HexonRegistry::publish_as` (republish of a
  `crate_id` by a different signed `publisher_did` denied) and `uninstall_as`
  on the `"global"` published record; reached from fe-api `publish_crate`.

`scope_contains` (keyword-boundary prefix containment, `VERSE#v1` never
covers `VERSE#v10`) lives in `types.rs` with `Scope::contains`; fe-database
re-exports it (`fe_database::scope_contains`) so existing call sites are
unchanged.

Deferred (not this slice): causal-DAG membership ops (blocked on per-op
signing, decisions §D5-1).

## §scope-parser (`scope.rs`)

The `VERSE#v[-FRACTAL#f[-PETAL#p]]` string parser (`build_scope`,
`parse_scope`, `parent_scope`, `scope_level`, `ScopeParts`, `ScopeError`,
`ScopeLevel`) moved here verbatim from `fe-database/src/scope.rs` (W5b,
OF-3). RP-1 had left a second, private copy of `parent_scope` (plus a
`verse_id_of` helper) inside `role_grants.rs` rather than touch
`fe-database`, documented at the time as an intentional duplicate — but two
parsers of the same wire-level scope string sitting on an authority path is
exactly a parser-differential risk: the role model and the DB plane could
silently disagree on which subtree a scope names (e.g. one accepting a
malformed segment the other rejects). This slice collapses that back to
**one parser**: fe-policy owns the implementation and its unit tests
(`scope.rs`'s own `#[cfg(test)] mod tests`); `fe-database/src/scope.rs` is
now a re-export plus a differential test that drives both public entry
points (`fe_database::scope::*` and `fe_policy::scope::*`) over the same
fixture set (well-formed, malformed, interim `VERSE#<hex>` authority keys,
deep chains) and asserts identical results — the regression guard against
either side re-diverging. `role_grants.rs`'s `scope_chain`/`validate_statement`
now call `crate::scope::{parent_scope, parse_scope}` instead of the removed
private copies; `validate_statement` is stricter as a result (it now fully
parses the scope via `parse_scope` instead of the old `verse_id_of`'s
loose prefix scan, so a scope with a malformed inner segment is rejected as
`RejectReason::MalformedScope` instead of silently taking only its leading
verse id).

## §node-lifecycle (`node_lifecycle.rs`)

Track `node_lifecycle_addressing_20260725` FR-1. Node delete/cascade and stamp
promotion are mutations, so they authorize at **Editor+** (`MIN_DELETE_ROLE`)
through the existing `RoleLevelPolicy::standard()` `Write` gate — no new
`Action` variant, no bespoke role math. A Viewer/Anonymous subject is denied by
construction (deny-by-default). `authorize_node_edit` (spatial-builder Step 1)
extends the same gate to rename/duplicate: one shared Editor+ write check for
both verbs (`RenameNode` / `DuplicateNode` arms), so the threshold cannot
drift per-command.

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

## §role-grants (`role_grants.rs`)

Pure per-grantor role-statement model
(`conductor/tracks/role_propagation_20261013` RP-1; plan §2.2–§2.4).
**Algorithm: least fixpoint via Kleene iteration** on a monotone operator
over a finite lattice (Knaster–Tarski) — `materialize` iterates Manager
statements from `mgr ≡ ⊥`, raising a cell only while its grantor's
`authority` (itself derived from the current `mgr`/owner state) is at least
Manager, until a pass changes nothing, bounded at `|statements|+1` passes
with a `tracing::warn!` bug guard if the bound is hit (plan §5 risk 1).

- **Owner statements are fixed; Manager statements only raise.** A grant
  pins a cell exactly; a revoke blocks it (the peer then inherits from the
  enclosing scope). `role_at`'s max-fold over the whole scope chain is what
  makes "Managers only raise" hold automatically: a weaker Manager-sourced
  value anywhere in the chain can never pull the result below a stronger
  one elsewhere in the chain (D-RP-2, T20). `cache_rows()` mirrors that into
  the simple nearest-row-wins `role` table by only writing a row when the
  cell's `mgr` value actually exceeds what inheriting from the parent would
  give — the hot-path `resolve_role` stays untouched and still returns
  exactly `role_at`.
- **Owner pins cut the authority walk.** `role_at` stops at the FIRST
  owner-pinned cell it reaches walking outward from the queried scope,
  discarding nothing folded in from narrower cells but ignoring everything
  wider. This is what lets the owner carve a petal out of a verse Manager's
  authority (D-RP-2, T21) — without the cut, the wider grant would still
  get folded in via the max.
- **`GrantedRole` structurally excludes `Owner`** (T3: nobody can grant
  Owner) and deliberately does not reuse `RoleLevel::from`'s unknown-string
  mapping: `validate_statement` parses the wire `role` string through
  `GrantedRole::parse`, which denies `"owner"` and any unrecognized string
  outright instead of silently landing on `None`.
- **`validate_statement`'s owner-dependent check defers, not denies,
  pre-manifest.** With `owner_did: None` it skips the target-is-owner check
  and returns `Ok`; the full check reruns when a held statement is promoted
  once the owner is known (§2.4). Everything else it checks is a
  **permanent** deny (`RejectReason`): key/payload/author agreement (T1),
  the scope parsing to this replica's verse (T6), and the role string
  (T3/unknown).
- **`admissible`** is the one placement predicate both the DB and sync
  planes use: true for the owner, else `authority(grantor, scope) ≥
  Manager` under the CURRENT materialization. A statement that fails this
  is Deferred (R7), never denied — `fe-database::role_grants` (RP-2) owns
  the durable store that holds it; this module only computes whether a
  statement is admissible, never where to park it.
- **`has_any_role`** exists only for the RP-3 enrolment membership floor
  (AC-44c): a petal-scoped member resolves to `RoleLevel::None` at the
  verse scope (R7 subtree semantics give them no wider authority), so the
  floor cannot use `role_at` at the verse scope alone.
- **`diff` is per cell, never best-across-scopes (refine R3, RP-1's open
  note).** `risen` = every DID whose `role_at` rose at ANY `(scope, did)`
  cell either snapshot mentions (pins, blocks, Manager grants); `fallen`
  likewise; an owner change counts the new owner risen and the old one
  fallen; a DID may be in both. The sync plane seeds its §2.5 re-read on
  empowerment from `risen` (fe-sync `sharding.rs::rematerialize`), so a
  best-role comparison silently dropped a Manager at fractal A newly
  empowered at fractal B, and never queued a freshly pinned owner whose
  pre-manifest statements had been discarded. Mirrors the DB plane's
  `changed_between` (fe-database `role_grants.rs`).
- **The scope-chain parser is `crate::scope::{parent_scope, parse_scope}`**
  (see §scope-parser) — unified there in W5b/OF-3; this module no longer
  carries its own copy.
- **`latest_per_grantor`** is last-writer-wins by `hlc` within a
  `(scope, peer_did, grantor_did)` key — a single author, so HLC skew can
  only reorder that author's own statements (T7) — tie-broken on the
  lexicographic order of the canonical payload bytes (a local
  `format!`-based encoding, not a hash; AC-51 — `fe-policy/Cargo.toml`
  carries no hashing/property-test dependency).
