---
type: plan
track: role_propagation_20261013
title: "Implementation Plan: role propagation + settlement race + responder filtering"
created: 2026-10-13
resource: ./spec.md
computed_at_commit: 877e801
security_critical: true
---

> **R7 applied 2026-10-13** (RUNBOOK §3; accountability AC-1): the body implements subtree grant
> authority, durable Deferred role statements, and no implicit petal owners.

# Implementation plan: role_propagation_20261013

> **Citations re-grounded @`009b282`** (critic tick 8; re-grep before editing if HEAD has moved
> further — symbol names are stable, line numbers are not). RUNBOOK §7 runs this track after
> PAS-P3+P4, PAS-P5, CDL W4 and CDL SPEC-10.
>
> | Plan cite | Now |
> |---|---|
> | `replicated_row.rs:201-207` role → `other` arm | `:224-230` |
> | A3 gate verse-scope resolve `:308-313` | `:403-416` (device_format Manager+ `:408-412`) |
> | bootstrap window `:297-306` | `:372-401` (AC-34 pre-manifest Deny `:382-393`) |
> | — (new) enrolment branch + floor | `:330-348` (floor `:336-346`) |
> | — (new) R5 confinement | `r5_referential_confinement :428-559`, `confine_petal_to_verse :562-581` |
> | `apply_static_row` MERGE `:349-363,:387-393` | `:591-` (default_access check `:610-630`, MERGE `:685`) |
> | — inbound terrain shape | `is_valid_inbound_petal_terrain :944-975` |
> | tests `:1103/:1133/:1168/:1324/:1698` | `viewer_role_row_is_denied_never_applied :1644`, `unknown_peer_row_is_denied :1674`, `denied_row_converges_on_replay_after_role_lands :1710`, `node_row_before_verse_manifest_applies_bootstrap :1865`, `timeseries_reading_from_viewer_is_denied :2239` |
> | `lib.rs:789-800` Assign/RevokeRole | `:845`, `:851`; governance arms `:793`, `:799` (was `:737-750`) |
> | `lib.rs:1460-1486` ApplyReplicatedRow | `:1581`; ExecuteTsPartial `:1440`; `lifecycle_auth_context :240` |
> | `fe-runtime/src/messages.rs:696-710` | `ReplicatedRowOutcome :969-983` |
> | harness `peer.rs:1041` / no-op `:572-582` | `:1084` / `:579-580` |
> | `canonical_epoch.rs:540` | `load :524`, query `:545`, exact-key lookup `:587-604` |
> | `sync_thread.rs:1102-1118` note_entry | `handle_inbound_row_change :1210`, `note_entry :1223`; `handle_write_row_entry :1668`; `TilesetProviderMap :2187`, `root_for :2290` |
> | `query_guard.rs:36` | `ALLOWED_TABLES :42` |
> | stable | role_manager.rs, crud.rs `:187`, seed.rs `:207`, invite.rs, sharding.rs, distributed_query.rs (`:2659` pinning test), elevated_query.rs `:17`, docs_engine.rs `:120`, scope.rs `:163`, fe-policy (untouched since `877e801`) |
>
> **Standing rules (RUNBOOK §3):**
> - Slices run strictly in sequence; never overlap builds.
> - **The gate (critic tick 8):** `cargo check -j4 --workspace --tests`; `cargo +1.99.0 clippy -j4
>   --workspace --all-targets --keep-going -- -D warnings`; `cargo +1.99.0 fmt --all -- --check`;
>   plus the feature matrix (`check` + `clippy --all-targets -D warnings` for `fe-query --features
>   parquet`, `fe-format --no-default-features`, `fe-terrain --features fetch,render`, `fe-hexon
>   --all-features`). `gate_green=false` → no commit, halt. Never return a structured result while
>   a cargo process you started is still alive — poll the summary for `DONE`. On exit
>   `0xc0000142` with no other builder running, retry that one step once before halting. The
>   gate script must not inherit a generic `overrides` bypass unless a user authorization for it
>   is logged first.
> - Each executor writes its tests now. No test runs until the end of the campaign, which
>   ends with one full test, e2e and harness sweep.
> - Reviewers fix their own findings.
> - Prune `target/debug/incremental` and the PDBs at every safe point.

---

## 0. Goal, acceptance, and the one refinement

**Goal.** Every replica converges on the same effective role set for a verse, with no local
provisioning and without trusting the wire. On top of that:
- close the distributed-query settlement race;
- re-enable responder filtering safely;
- close P2P-REF-CONFINE and the `__shards` Editor+ gate, both of which this track unblocks.

**Acceptance.**
- SYNC-ROLE-PROPAGATION, SYNC-RESPONDER and P2P-REF-CONFINE: see spec §Acceptance. The
  ledger rows are at `conductor/doneness-ledger.md:132,191,197`.
- The `__shards` part of SYNC-SIGNED-PLANE (`doneness-ledger.md:265`) closes: "viewer-authored
  `__shards` rows rejected". That row called it impossible without per-op signing. The real
  blocker was that no role data reached the sync plane (`fe-sync/src/AGENTS.md`
  §transport-admission-control item 5). Doc entries are already author-signed by iroh-docs,
  and `__peers` self-declaration already relies on that (`fe-sync/src/sharding.rs:348-365`).
  Relayed-gossip attribution and signed editor ops stay NO_GO.

**Refinement of the §4 assumption ("LWW by HLC per (verse, peer) row").** Implemented
literally, it fails in three ways:

1. **Concurrent revocation does not converge.** The owner revokes Manager M at HLC 100. M
   concurrently grants X at HLC 101. A peer that receives the grant first admits it (M is
   still a Manager); a peer that receives the revoke first denies it. The two peers diverge
   permanently.
2. **Skew abuse.** In cross-author last-writer-wins, any Manager can stamp a far-future HLC
   and win every contest, including against the owner.
3. **Mutual revocation has no unique answer.** M1 and M2 each revoke the other, and there are
   two stable outcomes.

**The refined model** keeps every stated property of the decision.

- **Storage.** Statements are stored per grantor: one doc key per `(scope, peer, grantor)`.
  Each grantor's own statements resolve last-writer-wins by HLC. That is a single author, so
  skew can only reorder that author's own statements.
- **Owner statements override.** A grant sets the role. A revoke blocks the cell, and the
  peer then inherits from the enclosing scope.
- **Manager statements combine by the maximum**, over statements whose grantor holds Manager+
  authority **within their own subtree** (R7: at the statement's scope or an ancestor of it)
  in the least fixpoint. Managers only raise; only the owner lowers.
- **A Manager's revoke retracts only that Manager's own grant.** Only the owner can remove a
  grant that someone else issued.
- **A grantor's grants lapse when the grantor loses authority.** That is a cascade, and it is
  what makes concurrent revocation converge.
- **A statement whose grantor lacks authority is Deferred** (R7). It is held outside the
  admitted set in a durable store that floods cannot evict, and it is re-evaluated whenever the
  grantor's authority changes (§2.4).

The materialization is a monotone operator on a finite lattice, so its least fixpoint exists,
is unique, and does not depend on delivery order. Because held statements are re-evaluated on
every authority change, the converged result equals that least fixpoint over every statement
received (§2.2). §7 records the decisions.

---

## 1. Current-code reality (as of `877e801`)

| Area | Fact | Where |
|---|---|---|
| Inbound role rows | `role` matches the `other` arm and returns `NotApplicable`; it is never applied | `fe-database/src/handlers/replicated_row.rs:201-207` |
| Outbound role rows | Neither `assign_role_checked` nor `revoke_role_checked` takes a replication sender; no role row is ever published | `fe-database/src/role_manager.rs:74-122`, `127-162`; dispatch at `fe-database/src/lib.rs:789-800` |
| A3 gate | Resolves at the VERSE scope only (`build_scope(verse_id, None, None)`); a petal-scoped Editor is denied remotely | `replicated_row.rs:308-313` |
| Bootstrap window | Admits *everything* until the verse row exists locally | `replicated_row.rs:297-306` |
| Manifest takeover (hole) | `apply_static_row` MERGEs the whole payload, so an admitted Editor can rewrite `created_by` (and `namespace_id`, `created_at`, `ts_*`). Only `default_access` is validated | `replicated_row.rs:349-363`, `387-393` |
| Invitee owner (bug) | `join_verse_by_invite_handler` writes `created_by = invite.creator_node_addr`. That is the **inviter**, encoded as **hex**, not `did:key`. The real owner then never resolves to Owner on the invitee, and the owner's rows fall to `default_access` | `fe-database/src/handlers/invite.rs:167`; invite struct `fe-database/src/invite.rs:18-32` (no owner field) |
| Implicit petal owner | `create_petal_handler` writes an explicit `role: "owner"` row at petal scope for the creator, who may be only an Editor. `seed.rs` does the same | `fe-database/src/handlers/crud.rs:182-190`, `fe-database/src/handlers/seed.rs:202-210` |
| Owner assignable | `assign_role_checked` lets an Owner assign `Owner` (`role > caller_role` is false when they are equal). Revoke has no level check | `role_manager.rs:92`, `127-162` |
| Local grant authority | Already subtree-shaped: the caller is resolved AT THE TARGET SCOPE (`resolve_role(db, caller_did, scope)`, which walks up), so a petal Manager may assign in that petal only. R7 keeps this; §2.2 makes it exact | `role_manager.rs:83-89` |
| Role storage | `role {peer_did, scope, role}`, random record id, no HLC, grantor or tombstone; `assign_role` does a delete then an insert | `fe-database/src/schema.rs:119-127`, `fe-database/src/rbac.rs:106-125` |
| Role resolution | Owner via `created_by`, then the most specific explicit row walking up the scope chain, then `default_access` | `role_manager.rs:44-68` |
| Unknown role strings | `RoleLevel::from` maps unknown strings to `None` and `"public"` to Viewer | `fe-policy/src/role_level.rs:80-90` |
| Governance gates | `SetVerseDefaultAccess` and `SetVerseTimeseriesSettings` are not role-gated locally. Default access is not replicated. Settings re-emit the whole manifest | `lib.rs:737-750`; `role_manager.rs:178`; `fe-database/src/handlers/verse_settings.rs:74` |
| HLC | Local stamping only, with no receive/observe | `fe-database/src/op_log.rs:108-134` |
| Replay chance | Denied rows recover only through the snapshot replay on replica open | `fe-sync/src/AGENTS.md` §inbound-apply "Startup reconciliation"; test `replicated_row.rs:1168` |
| Doc keys | `{table}/{record_id}`, split at the first `/` | `fe-sync/src/replicator.rs:247,731,962,1177` |
| Snapshot | `Query::single_latest_per_key()` returns ONE entry per key across authors. iroh-docs 0.101 also has `Query::key_prefix(..)`, a flat query over all authors (`~/.cargo/registry/.../iroh-docs-0.101.0/src/store.rs:314`) | `replicator.rs:694-712` |
| Mock replicator | Last-writer-wins per key with an author tie-break: one entry per key | `replicator.rs:~315-337` |
| Sync-plane rows | `note_entry` runs BEFORE the own-author filter. `__shards`/`__peers` never reach the DB. `note_verse_row` adopts settings from ANY author's manifest, including ones the DB denies | `fe-sync/src/sync_thread.rs:1102-1118`; `fe-sync/src/sharding.rs:200-212`, `339-375` |
| Ledger publish | Written before the reading, on the outbound path | `sync_thread.rs:~1620-1650` |
| Planner | `expected_hosts` holds ledger hosts only | `fe-sync/src/distributed_query.rs:159`, `213-223` |
| Settlement | Aggregates settle when every shard has a non-empty answer, OR all expected hosts have answered and the local partial resolved. Raw and latest settle on the latter only, so a non-host answer that arrives after the hosts is lost | `distributed_query.rs:945-985` |
| Responder | Answers every requested shard (filter reverted) | `distributed_query.rs:1223-1276`; pinned by `a_non_host_authors_creator_copy_serves_rows_its_denying_host_refused` at `distributed_query.rs:2659` (author-first delivery order) |
| Elevated writes | `ELEVATED_WRITABLE_TABLES` does not include `role` (good; pin it) | `fe-database/src/handlers/elevated_query.rs:17-18` |
| Harness | Its simplified DB loop no-ops `AssignRole`/`RevokeRole`; the `distributed_query` scenario keeps the production default (no roles) | `fe-test-harness/src/peer.rs:572-582`; `fe-test-harness/src/scenarios/distributed_query.rs:77-95` |

---

## 2. Design (shared by every slice)

### 2.1 The role statement (wire and storage)

- **Doc key:** `role/{scope}/{peer_did}/{grantor_did}`. Scopes (`VERSE#v-FRACTAL#f-PETAL#p`)
  and DIDs contain no `/`, so `record_id` parses as exactly three segments.
- **Payload:**
  `{scope, peer_did, grantor_did, role: "manager"|"editor"|"viewer"|"none", revoked: bool, hlc: u64}`.
  Revocation is a **non-empty** payload with `revoked: true`. An empty entry (a `Doc::del`)
  on the `role` table is `NotApplicable`: it carries no HLC, and iroh's `del` is a prefix
  delete.
- **DB table `role_grant` (admitted statements):** one row per `(scope, peer_did, grantor_did)`,
  holding `verse_id`, `hlc`, `revoked`, `role` and `updated_at`, with a UNIQUE index on the
  triple and an index on `(verse_id, grantor_did)`. Last-writer-wins by `hlc` within the
  triple; on equal `hlc`, the lexicographically larger canonical payload hash wins
  (deterministic).
- **DB table `role_statement_pending` (Deferred statements, R7):** the same columns and indexes
  plus `received_at`. It holds the statements whose grantor does not yet have authority
  (§2.4). A key has exactly one live version across the two tables: the latest by the rule
  above.
- **The existing `role` table** becomes a **materialized cache** that only the materializer
  writes (the §2.2 step 4 projection). `resolve_role` and `get_role` are unchanged (hot path
  untouched).

### 2.2 Validity, authority and the fixpoint (the pure fe-policy model, one function shared by both planes)

`validate_statement(replica_verse, owner_did, key_parts, author_did, payload)` checks the
following. Any failure is a **permanent deny**.

- The key's `grantor_did` equals `author_did` (the attested entry author). The payload's
  `scope`, `peer_did` and `grantor_did` equal the key's parts.
- The scope parses, and its verse equals `replica_verse`.
- `role` is in {manager, editor, viewer, none}. `owner` and unknown strings are denied, never
  silently mapped through `RoleLevel::from`.
- `peer_did` is not `owner_did`. The owner is untouchable and never a grant target.
- Statements whose grantor is the owner are owner statements; all others are Manager
  statements.

The two owner-dependent checks need the manifest. Before it lands, a statement that passes the
other checks is held (§2.4), and the full check runs when it is promoted.

**Resolution and authority (R7: any Manager, within their own subtree).** Write `chain(S)` for
S followed by its parents up to the verse (`parent_scope`), and `mgr(S,P)` for the maximum
valid Manager grant on cell `(S,P)` (⊥ if there is none).

- `role_at(P, S)` is `Owner` if P is the owner. Otherwise it walks `chain(S)` upward:
  - at an owner-pinned cell, it folds in the pin and stops (an owner pin is exact, so it cuts
    the walk);
  - at an owner-blocked cell, it skips that cell's Manager grants and continues;
  - at any other cell, it folds in `mgr(S',P)` and continues.

  The result is the maximum folded in, or ⊥ if there is none. Callers that gate content fall
  back to `default_access` on ⊥.
- `authority(G, S) = role_at(G, S)`. With no owner pin on the walk, this is exactly "the
  maximum of G's effective role over S and its ancestors". So:
  - a petal Manager has authority in that petal; a fractal Manager in that fractal and all of
    its petals; a verse Manager, and the owner, everywhere in the verse;
  - authority never reaches upward or sideways: a petal Manager has none at the enclosing
    fractal or verse, or in a sibling petal;
  - nobody can grant Owner (`owner` is structurally denied), and every valid grant has
    `R ≤ Manager ≤ authority`.
- Two refinements stop subtree authority from being turned against the owner, or against a
  peer with a wider scope:
  - **Owner pins cut the walk.** If the owner pins a verse Manager to `viewer` in petal P, that
    Manager has no authority in P. Without the cut, they could appoint a petal Manager in P
    and undo the owner's carve-out.
  - **Managers only raise.** A Manager-issued `none` has no effect. Step 4 never writes a
    Manager-sourced cache row that is not above the peer's inherited role. Without this rule, a
    petal Manager could write an explicit `viewer` row for a verse Manager in their petal, and
    `resolve_role` (most-specific row wins, `role_manager.rs:52-63`) would demote that verse
    Manager there. Only owner statements lower anyone.

`materialize(owner, statements) -> EffectiveRoles` takes the admitted statements (§2.4) and
computes the effective roles in five steps:

1. **Owner statements are fixed.** A grant pins cell `(S,P)` to `R`. A revoke marks the cell
   owner-blocked with no explicit value, so the peer inherits from the enclosing scope.
2. **Iterate to a fixpoint from `mgr ≡ ⊥`** (Kleene iteration). Take a Manager statement
   `(S,P,G,R)` that is not revoked, has `R ≠ none`, and whose cell is neither owner-pinned nor
   owner-blocked. It is valid iff `authority(G,S) ≥ Manager` under the current values. A
   valid statement raises the cell: `mgr(S,P) = max(mgr(S,P), R)`.
3. **Termination.** Stop when a full pass changes nothing. That takes at most |statements| + 1
   passes.
4. **Cache projection.** Write a `role` row for `(S,P)` when either:
   - the cell is owner-pinned (value: the pin); or
   - the cell is not blocked and `mgr(S,P) > role_at(P, parent(S))` (value: `mgr(S,P)`).

   `resolve_role` walks to the most specific of these rows, which returns exactly `role_at`.
   The hot path is unchanged.
5. **Properties.** The fixpoint is unique and does not depend on order.
   - Owner cells never move.
   - Owner pins and blocks are constants, and `role_at` is a maximum over the walk of the
     `mgr` values, so `authority` is monotone in `mgr`.
   - Validity is monotone in `authority`, so the operator is monotone. Iteration from ⊥
     therefore reaches its least fixpoint (Knaster–Tarski, on a finite lattice).
   - A cycle of grants with no root at the owner is never valid.

**Admission (R7): held back as Deferred, never stored inert.** Both planes use one predicate,
`admissible(effective, statement)`: the grantor's `authority` at the statement's scope is at
least Manager. That is always true for the owner.
- An admissible statement is admitted (DB: `role_grant`; sync plane: retained in the fabric).
- Any other statement is **Deferred**. It is held outside the admitted set in a durable store
  (§2.4) and re-evaluated whenever its grantor's authority changes.
- **Why the final role set does not depend on delivery order.** Each plane keeps two
  invariants after every change: every held statement is inadmissible under the current
  materialization `M`, and `M` is the least fixpoint over the admitted set. The DB keeps them
  with the §2.4 partition; the fabric keeps them with the §2.5 re-read on empowerment.
  - Every held statement is inadmissible under `M`, so step 2 finds it invalid. Adding the held
    statements to the input therefore leaves `M` a fixpoint.
  - The admitted set is a subset of everything received, so `M` is at most the least fixpoint
    over everything received.
  - So `M` *is* that least fixpoint. It depends only on the set of statements received, not on
    their order, on restarts, or on other authors' traffic. §2.4 makes the held set durable
    and flood-proof, and that is what makes the argument hold in practice.

### 2.3 Local semantics (parity with the inbound path)

Local assign and revoke apply the same rules:
- The caller's `authority` at the target scope must be at least Manager (subtree, §2.2). Once
  the `role` table is the §2.2 step 4 projection, the existing check
  `resolve_role(db, caller_did, scope)?.can_manage()` (`role_manager.rs:83`) computes exactly
  this, so it stays.
- `R ≠ Owner`, and the target is not the owner.
- A Manager may assign `manager`, `editor` or `viewer`. Assigning `none` is refused with a
  named error: Managers only raise, and a Manager retracts their own grant with a revoke. The
  owner may also assign `none`, which pins the cell.
- A caller without authority is refused synchronously. A local command never defers.
- The statement is stamped with `next_hlc_timestamp()`.
- It is written to `role_grant`; it is admissible, because the caller was just checked. Then
  the verse is re-partitioned and rematerialized (§2.4), and the statement is published.

A Manager's revoke writes that Manager's own `revoked` statement, which retracts only their
own grant. If the target still holds the role through another grantor or by inheritance, the
result says so. The UI may surface that later under D-32 FR-4.

### 2.4 Deferred admission (replaces "deny and hope for a reconnect")

A row that fails only because something has not converged is **Deferred**, with one of these
wait reasons:
- `Manifest` (no verse row yet);
- `Parent{table,id}` (the referenced fractal, petal or node is unknown locally);
- `AuthorRole{did}` (the author resolves below the threshold).

Structural violations stay **Denied**. A deferred row is held in one of two places, depending
on what it is.

#### Role statements: a durable store per plane (decision, AC-1)

The in-memory queue (next subsection) is not safe for role statements, for two reasons:
- it does not survive a restart;
- it drops the oldest entry at 4096 per verse, with a cap of 512 per author. A write-capable
  peer with 8 or more author ids can therefore evict a legitimate held statement. That peer
  then diverges until the replica reopens, which breaks R7's order-independence.

The options:

| Retry source | Survives restart | Survives a flood | Verdict |
|---|---|---|---|
| In-memory `InboundAdmissions` (the old plan) | no | no: a shared cap with drop-oldest | rejected (AC-1) |
| The DB plane re-reads the replica's `role/` prefix on every authority change | yes | yes | rejected for the DB plane, for the three reasons below the table |
| **DB plane: a persisted `role_statement_pending` table** | yes (SurrealKV) | yes: rows are keyed by grantor, the budget is per grantor, and there is no shared cap | **chosen.** Re-evaluation runs synchronously in the DB loop, in the same step as rematerialization. It is testable at the DB dispatch seam, and it does not depend on whether the replica is open |
| **Sync plane: a per-author replica re-read** (§2.5) | yes (the iroh-docs store) | yes: a held statement is read back only when its grantor gains authority | **chosen** for the fabric, which already holds the replica. iroh-docs 0.101 composes `Query::author(id).key_prefix(p)`. The re-read also re-forwards to the DB, which is the DB plane's backstop |

Why a replica re-read does not suit the DB plane:
- The DB thread has no replica handle, so a re-read needs a new DB→sync request type through
  both binaries' bridges.
- The re-read rows come back over the drop-and-count DB channel, so the retry itself can be
  lost.
- It works only while the replica is open, but a local grant can change authority while the
  replica is closed.

Rules for the DB plane:
- **One live version per key.** A key `(scope, peer_did, grantor_did)` lives in either
  `role_grant` or `role_statement_pending`, never both, and its version is always the latest
  by the §2.1 rule.
  - An arriving statement older than the live version is reported `Applied` (idempotent) and
    dropped.
  - A newer one replaces the live version, and placement decides which table it lands in.
- **Partition by current authority.** This runs after every change: an admitted statement
  (inbound or local), the manifest landing (the owner becomes known), the migration, and DB
  startup (which also repairs a crash between a statement write and this step).
  1. Rematerialize the verse. That returns `ChangedDids`, with the direction of each change.
  2. **Demote.** For each DID whose authority fell, move its admitted statements that are no
     longer admissible into pending. Demotion is never refused and never drops a row. It does
     not move `M`, because the fixpoint already ignores those statements.
  3. **Promote.** For each DID whose authority rose, take its pending statements that are now
     admissible. Re-validate each in full; for pre-manifest statements, this is where the
     owner-dependent checks first run, and a failure deletes the row as Denied. Admit the
     survivors, rematerialize, and repeat with the newly raised DIDs.

     Two triggers have no previous `M` to compare against, so they seed the "rose" set
     directly. When the manifest lands, the owner DID counts as risen. At startup, the owner
     and every DID holding Manager+ anywhere in the verse count as risen.

  The loop terminates: promotions only raise `M`, demotions never change it, and each round
  moves at least one row. Every lookup goes through the `(verse_id, grantor_did)` index, so a
  flooder's held rows are never scanned unless that flooder gains authority.

  **Invariant:** afterwards, `role_grant` holds exactly the latest statements admissible under
  `M`, and `role_statement_pending` holds the rest.
- **A flood-proof budget.**
  - `validate_statement` pins `grantor_did` to the attested author, so an author can occupy
    only its own keys.
  - `ROLE_PENDING_PER_GRANTOR` (1024 keys per verse) applies only when an inbound statement
    would create a new pending key. Beyond the budget, that statement is refused and counted
    (`role_pending_refused_count()`).
  - An update to an existing key always applies, and an admissible statement never touches
    the budget.
  - There is **no per-verse or shared cap**, so no author can evict or starve another
    author's held statements.
  - Every held row mirrors an entry the replica already stores, so the replica bounds the
    total size. The store adds no amplification beyond the namespace write capability that
    already exists.
- **Over-budget recovery.** A refused statement is still in the replica. When its grantor gains
  authority, the sync-plane re-read (§2.5) forwards it again, and it is admitted directly.
- **Role statements never use the in-memory queue.**
  - They are never deferred on `Parent`. The materializer works on scope strings, so a
    statement for a petal not yet known locally is placed normally and takes effect when the
    petal row lands, with no retry. A scope that names a fractal or petal where the hierarchy
    does not put it never resolves, so it can never take effect.
  - Before the manifest lands, the owner is unknown and nobody has authority, so every role
    statement is held in the pending store (reason `Manifest`). The manifest landing triggers
    the partition.
- **Guarantee.** The converged role set is a function of the set of statements the DB has
  received. Delivery order, restarts and other authors' floods cannot change it.
  - The bounded sync→DB channel can still drop a row under load. That is the pre-existing
    drop-and-count (`fe-sync/src/AGENTS.md` §inbound-apply).
  - A dropped statement comes back through the §2.5 re-read or the next open's replay. That
    delays the result but never changes it.

#### Content rows: the in-memory `InboundAdmissions` queue

The queue lives in the DB loop:
- It is keyed by `(verse, table, record_id, author)` with the latest bytes, and indexed by wait
  reason.
- Caps: 4096 per verse and 512 per author. Overflow drops the oldest entry, with a warning and
  a counter (`deferred_admission_drop_count()`).

Retries run as a worklist loop (not recursion) after each apply:
- a manifest apply retries the `Manifest` waiters;
- a fractal, petal or node apply retries the `Parent` waiters for that id;
- a materialization change retries the `AuthorRole` waiters for every DID whose effective
  role changed.

A restart or an overflow loses entries, and the snapshot replay on open re-feeds them (A4).
Content rows never feed the role set, so a flood here can delay content but cannot change who
holds which role.

### 2.5 Sync plane

`VerseFabric` learns:
- the owner, from the first manifest's `created_by`, pinned with the same immutability rule;
- role statements, from `role` rows in both directions. Through the same `admissible`
  predicate, it **retains only admitted statements**. It does not keep a held statement in
  memory, because the replica already stores it;
- a `petal → fractal` map, from `petal` rows.

It runs the same `fe_policy` materializer. **Every gate calls `role_at` at the row's deepest
scope.** That is the same function the DB's A3 gate (RP-4 step 6) and grant authority use, so
the two planes judge a petal-scoped Editor or Manager (appointed by a petal Manager) the same
way. The gates:
- `__shards` and `__holders`: Editor+ at the petal scope.
- A manifest's settings: Manager+ at the verse scope, or the owner. A petal or fractal
  Manager's manifest edits are ignored, because verse settings lie above their subtree.
- A compute requester: Viewer+ at the requested petal's scope. `default_access` applies when
  `role_at` is ⊥, and the walk lets verse-level Viewers through.
- The merge filter (§2.6): Editor+ at the shard's petal scope.

**Re-read on empowerment (the fabric's durable, flood-proof retry).**
- When an author's `role_at` rises at any scope, the fabric queues that DID in `reread_queue`.
  A role increase needs a valid grant, so a stranger cannot grow the queue.
- The sync loop drains the queue with
  `replicator.entries_by_author(did, ["role/", "__shards/", "__holders/", "verse/"])` and feeds
  every entry through `handle_inbound_row_change`. The fabric and the DB therefore both see the
  entry again, and both apply it idempotently.
- The read is awaited on the local docs store, never on a channel that the select loop drains.
  That respects the §inbound-apply self-drain rule.
- `entries_by_author` is a provided `VerseReplicator` method. Its default filters
  `snapshot()`, so the mock, virtual and `fe-sim` replicators need no edit.
  - `AnyReplicator` forwards it.
  - `IrohDocsReplicator` overrides it with `Query::author(id).key_prefix(p)`. The author id
    comes from `fe_identity::did_key::public_key_from_did_key`, the inverse of
    `author_did_key` (`replicator.rs:896`).

A gated row that cannot be judged yet is **quarantined** (1024 per verse, drop-oldest with a
counter). It is re-evaluated whenever the role view or the petal map changes.
- The quarantine is only an accelerator. An evicted row comes back through the re-read when
  its author is empowered, so a flood of strangers' rows cannot cost a legitimate Editor their
  `__shards` row.
- A row that was waiting only on the petal map comes back with the next open's replay.
- Retention of an unknown shard stays "retain", the safe direction.

### 2.6 Settlement and filtering

- **Holders.** An author whose local reading lands in a shard it does not host publishes
  `__holders/{petal}/{anchor}/{bucket}/{did}` once, after the ledger row and before the
  reading. The last key segment must equal the entry author, as with `__peers`.
- **Expected answerers** are the hosts plus the admitted holders.
  - A holder is awaited for at most `HOLDER_WAIT_MS` (default 750 ms, clamped to the
    deadline).
  - A holder that stays silent is reported in `meta.missing_holders`. It never blocks the
    query until the full deadline.
- **Host honesty.** `TsPartialRows.deferred_shards` lists the shards for which the responder's
  DB holds deferred readings. A covered-empty answer on such a shard does not settle it, and
  the shard is reported in `meta.incomplete_shards`.
- **Merge.** Rows from an answerer that is neither a host nor an admitted holder, and that is
  below Editor at the shard's petal scope in the requester's role view (`role_at`, §2.5), are
  discarded and listed in
  `meta.rejected_answerers`. If the requester's role view has not converged on that answerer,
  the rows are kept and the answerer is listed in `meta.unverified_answerers`. **Superseded
  by W5s OF-7c:** an unverified answerer's rows are EXCLUDED (fail closed); it is still listed.
- **Responder filter.** A responder answers a shard iff local ∈ hosts ∪ holders(shard), or the
  ledger has no entry for that shard. Raw and latest queries are answered iff the responder
  hosts or holds any shard of the petal, or the petal is unknown. **Hosted-only filtering never
  becomes safe**, because replication lag is unknowable; this rule is documented as permanent.

---

## 3. Slices (strictly sequential, one executor each)

Size key: S ≤ 300 LoC, M ≤ 800, L > 800 (tests included).

### RP-0: oversized-window split path test + typed schema rejection · S · sonnet (critic tick 8, AC-49)

**Owned files:**
- `fe-database/tests/observation_plane_test.rs`
- `fe-database/src/handlers/device_payload.rs` (+ its rejection enum's home; grep `DeviceFormatRejection`)
- `fe-database/tests/device_payload_test.rs`

**Steps:**
1. Write `an_oversized_window_splits_by_anchor_then_hlc_and_a_single_oversized_observation_is_marked_uncovered`
   in `observation_plane_test.rs`, reusing that file's existing fixture (verse, capability chain,
   door ingest in a shadow mode). Grant the sealer an `append/shard` chain whose `max_segment_bytes`
   caveat is small enough that one window of ≥3 anchors returns `Prepared::Oversized`
   (`sealer.rs:385` `SegmentTooLarge`). Assert: the batches cover disjoint member sets; every member
   ends either sealed (`sealed_op_id` set) or `uncovered` with the reason "a single observation
   exceeds the sealed artifact bound"; no member is in two batches; `seal_closed_windows` returns.
   Mutation check: deleting the `Prepared::Oversized if group.len()==1` arm must make this test fail
   or hang (bound it with a timeout).
2. In `register_format_with_replication` (`device_payload.rs:69`), a descriptor `schema` that is
   present but not a JSON object returns a typed rejection (`DeviceFormatRejection::InvalidSchema`,
   mapped like its siblings through fe-runtime/fe-api if the enum crosses crates — grep it). Add one
   row to the existing door rejection table test.

**Gate:** the same workspace-triple + feature-matrix gate as every other slice (plan header). Do
not run tests (they run at the final sweep).

### RP-1: fe-policy pure role-grant model · M · sonnet

**Owned files:**
- `fe-policy/src/role_grants.rs` (new)
- `fe-policy/src/lib.rs` (export)
- `fe-policy/AGENTS.md` (new section §role-grants)

**Steps:**
1. Add `RoleStatement`, `StatementKey::parse(record_id)`, `validate_statement(..) ->
   Result<RoleStatement, RejectReason>` and `materialize(owner, impl IntoIterator<Item=&RoleStatement>)
   -> EffectiveRoles`. `EffectiveRoles` holds the owner pins and blocks plus the `mgr` map,
   and exposes:
   - `role_at(did, scope)` and `authority(did, scope)` (§2.2 resolution);
   - `admissible(&RoleStatement)`, the one placement predicate both planes use (§2.2
     "Admission");
   - `cache_rows()`, the §2.2 step 4 projection;
   - `diff(&previous) -> ChangedDids`, which gives each DID with the direction its authority
     moved;
   - `has_any_role(did)` (critic tick 8, AC-44c): true if the DID holds any effective role at
     any scope of the verse. Backs the RP-3 enrolment membership floor, which must admit a
     petal-scoped member even though they resolve to `None` at verse scope.

   `validate_statement` takes `owner_did: Option<_>`. With `None` (pre-manifest), it runs only
   the owner-independent checks.
2. Add a scope-chain helper that splits `VERSE#..-FRACTAL#..-PETAL#..`. Mirror
   `fe-database/src/scope.rs:163` `parent_scope` exactly, or move the pure parser into
   fe-policy and re-export it from fe-database (preferred: one parser).
3. Add `latest_per_grantor(statements)` (HLC last-writer-wins; tie-break on the lexicographic
   order of the canonical payload **bytes** — no hash dependency, critic tick 8 AC-51, since
   `fe-policy/Cargo.toml` carries only `serde`/`tracing` today).

**Invariants:**
- fe-policy AGENTS.md §design constraints: no I/O, serde-light, deny by default.
- §role-level: the ordering is unchanged.
- `role_grants.rs` carries a one-line pointer to §role-grants and names the algorithm
  ("least fixpoint, Kleene iteration").

**Flow tests** (pure, table-driven or property-based; run at campaign end). **(Critic tick 8,
AC-51.)** "Property test" here means seeded, hand-rolled permutations of a generated fixture set
— no new dependency. If a later slice wants a real property-test crate (e.g. `proptest`), it must
add `fe-policy/Cargo.toml` + `Cargo.lock` as owned files, since neither carries a property-test
dependency today:
- `materialize_is_order_independent`: a property test over random permutations of a generated
  statement set. The set has the owner; 3 Managers at verse, fractal and petal scope; and
  grants, revokes, owner pins and cycles. It asserts identical `EffectiveRoles` across
  permutations.
- `partitioned_admission_reaches_the_fixpoint_over_everything_received` (R7, the §2.2
  argument). This is a property test over random statement sets and arrival orders. It feeds
  statements one at a time through admit-if-admissible, then re-partitions to quiescence. It
  asserts that the result equals `materialize` over the whole set, and that the held set is
  exactly the inadmissible statements. It fails if `admissible` and step 2's validity ever
  disagree.
- `owner_revocation_of_a_manager_voids_their_concurrent_grant_in_every_order` (T4). It
  includes the petal Manager whom the revoked Manager appointed, so the cascade runs down the
  subtree.
- `grant_authority_is_the_grantors_own_subtree` (R7, T19). This is table-driven:
  - a petal Manager's grant takes effect in their petal;
  - the same grant in a sibling petal, or at the enclosing fractal or verse, has no effect;
  - a fractal Manager's grant takes effect in every petal of that fractal and nowhere else;
  - a verse Manager's grant takes effect everywhere in the verse.
- `an_owner_pin_below_manager_carves_a_petal_out_of_a_verse_managers_authority` (T21).
- `a_lower_scope_manager_cannot_demote_a_wider_scoped_peer` (T20). A petal Manager's
  `viewer` statement for a verse Manager writes no cache row, and `role_at` stays Manager. A
  Manager's `none` statement has no effect.
- `a_manager_cannot_grant_owner_and_an_owner_grant_statement_is_rejected` (T3).
- `statements_targeting_the_owner_are_rejected` (T5).
- `a_statement_whose_key_names_another_grantor_is_rejected` (T1).
- `a_cross_verse_scope_is_rejected` (T6).
- `mutual_manager_revocation_has_a_unique_outcome`: each Manager's revoke retracts only their
  own grant, and the owner's grants stand.
- `a_far_future_hlc_from_a_manager_cannot_displace_the_owners_statement` (T7).
- `an_unrooted_grant_cycle_is_never_effective`.
- `a_manager_grant_lapses_when_the_manager_loses_authority_and_returns_when_regranted`.

### RP-2: fe-database local statements, materialized cache, outbound publish, migration · L · opus

**Owned files:**
- `fe-database/src/schema.rs` (the `role_grant` and `role_statement_pending` tables and their
  indexes)
- `fe-database/src/role_grants.rs` (new: statement store, placement, partition, rematerialize)
- `fe-database/src/rbac.rs`
- `fe-database/src/role_manager.rs`
- `fe-database/src/handlers/rbac.rs`
- `fe-database/src/handlers/crud.rs` (remove the petal owner row only, `:182-190`)
- `fe-database/src/handlers/seed.rs` (remove the owner row, `:202-210`)
- `fe-database/src/handlers/verse_settings.rs` (Manager+ gate)
- `fe-database/src/lib.rs` (the `AssignRole`/`RevokeRole`/`SetVerseDefaultAccess`/`SetVerseTimeseriesSettings`
  arms gain `repl_tx` and `blob_store`, plus the startup migration call)
- `fe-database/src/migration/role_grants.rs` (new) and `fe-database/src/migration/mod.rs` (register)
- `fe-database/src/handlers/elevated_query.rs` (pin test only)
- `fe-database/examples/seed_join_verse.rs` (`:140-164`; fix the citation — it writes an
  arbitrary-level row for a remote peer, not an "owner row" — then retire its `--role-*` flags
  or route them through the statement API, critic tick 8 AC-45)
- `fe-database/src/AGENTS.md` (§rbac-policy plus a new §role-propagation)
- `fe-database/src/handlers/admin.rs` (`:58`; the admin `role`-clear path — decide its
  membership over `role_grant`/`role_statement_pending`, critic tick 8 AC-45)
- **(Critic tick 8, AC-45 — C3 superseded, false premise; see plan §8 "C3 superseded by C15")**
  NOT owned: `fe-database/src/canon_log/canonical_epoch.rs` — the interim authority keys
  `VERSE#<hex>[-PETAL#<hex>]` rows directly (deliberately not `build_scope`'s string) and was
  never watching `role`/`role_grant`; deriving the owner there is a SPEC-2 concern, out of RP's
  scope. Owned/checked instead — the seed inventory each row of `role` passes through, which
  must each stay valid or move to the statement API:
  `fe-database/src/handlers/petal_render_distance.rs` (`:220,494`),
  `fe-database/src/handlers/replicated_row.rs` (`:1590,1741`),
  `fe-database/tests/device_payload_test.rs` (`:68`),
  `fe-database/tests/device_enrolment_test.rs` (`:438,551,679`),
  `fe-database/tests/observation_plane_test.rs` (`:277,755`),
  `fe-api/tests/elevated_query_test.rs` (`:393`),
  `fe-database/tests/record_id_read_test.rs` (`:39`),
  `fe-database/tests/canon_log_composed_admission_test.rs` (`:196`),
  `fe-database/tests/canon_log_replay_payload_facts_test.rs` (`:231`),
  `fe-database/tests/migration_equivalence_harness.rs` (`:353`).

**Steps:**
1. Add the schema (§2.1):
   - `role_grant` (verse_id, scope, peer_did, grantor_did, role, revoked, hlc, updated_at);
   - `role_statement_pending` (the same columns plus received_at).

   Each gets UNIQUE (scope, peer_did, grantor_did) and an index on (verse_id, grantor_did).
   Keep the `role` table shape, and add the comment "materialized; written only by
   `role_grants::rematerialize`".
2. Add `fe-database/src/role_grants.rs`:
   - `place_statement(db, verse_id, stmt) -> Placement` returns one of `Admitted`, `Held`,
     `Superseded` or `RefusedOverBudget`, following §2.4's one-live-version and budget rules.
   - `rematerialize(db, verse_id) -> ChangedDids` loads the admitted statements and the owner,
     calls `fe_policy::materialize`, and diff-writes `cache_rows()` for that verse's scopes.
     It runs in one transaction. Mind the §log-first-commit ordering.
   - `repartition(db, verse_id)` runs §2.4's demote/promote worklist on top of
     `rematerialize`.
   - At DB startup, `repartition` runs for every verse. That repairs a crash between a
     statement write and the partition step.
3. **(Critic tick 8, AC-42 — durable re-emit of local role statements.)** Statements publish
   through `replicate_row`, which only reaches an OPEN replica; `fe-sync/src/AGENTS.md`
   §pending-writes caps pending writes at 1024, drop-oldest, process-lifetime only. A local
   grant or revocation issued while the verse's replica is closed (the GUI opens only the
   ACTIVE verse's replica — root `AGENTS.md` §NavigationManager), or lost to a pending-write
   flood, never reaches any peer. **Default:** at DB startup and whenever a verse replica opens,
   the DB re-emits every `role_grant` row whose `grantor_did == local_did` (newest per key; iroh
   deduplicates identical bytes, so a re-emit of an already-seen statement costs nothing on the
   wire). Add threat **T23** "a local grant or revocation is never published" to RP-9's matrix,
   and two tests: `a_revocation_issued_while_the_replica_is_closed_reaches_peers_after_restart`
   and `a_pending_writes_flood_cannot_drop_a_local_revocation`.
4. Rewrite `assign_role_checked` and `revoke_role_checked` per §2.3:
   - the op-log entry still lands first (§log-first-commit);
   - keep the existing authority check, `resolve_role(caller, scope)?.can_manage()`. Over the
     new cache it is exactly subtree authority;
   - reject `Owner`, an owner target, and a Manager-issued `none`, each with a named error;
   - then upsert the statement into `role_grant`, then `repartition` (which can promote held
     statements the caller's grant just empowered);
   - then `replicate_row(repl_tx, blob_store, verse_id, "role", "{scope}/{peer}/{grantor}", bytes)`.
5. **Gate parity:** `set_default_access` and `set_verse_timeseries_settings_handler` require
   local Manager+ **at the verse scope**, via `resolve_role` and fe-policy; a petal or fractal
   Manager is refused. `SetVerseDefaultAccess` now re-emits the manifest, the same way
   settings already do (`verse_settings.rs:74`), so the change propagates deliberately.
6. Stop writing implicit `owner` role rows (`crud.rs:182-190`, `seed.rs:202-210`, the example).
   R7 confirmed this.
7. **Migration (idempotent, runs once at DB startup):**
   - drop every `role` row whose `role = "owner"`. That includes every implicit petal-creator
     row (R7). Role rows have never replicated, so no other peer ever honoured those rows, and
     dropping them loses nothing remote. Locally, the creator falls back to the role they
     resolve to from the enclosing scope;
   - for each remaining explicit row in a verse whose `created_by == local_did`, synthesize an
     owner statement (hlc = `next_hlc_timestamp()`) and emit a `ReplicationEvent`. Pending
     writes retain it until the replica opens (`fe-sync/src/AGENTS.md` §pending-writes);
   - for every other legacy row, log a warning and drop it (no consumers; §7 assumption);
   - then repartition every verse.
8. Add a test pinning that `ELEVATED_WRITABLE_TABLES` never contains `role`, `role_grant` or
   `role_statement_pending`.
9. **(Critic tick 8, AC-45 — replaces the earlier C3 step; interim authority rows must survive
   the migration.)** C3's premise was false: the interim authority
   (`canonical_epoch.rs:486-604`) keys rows `VERSE#<hex>[-PETAL#<hex>]`, deliberately NOT
   `build_scope`'s string, and looks up the exact key. The owner rows at `crud.rs:187`/
   `seed.rs:207` use `VERSE#<ulid>-FRACTAL#...` keys and were never visible to it — so deriving
   the owner inside `DurableAuthorizationView::load` is a SPEC-2 concern, out of RP's scope. The
   migration (step 7) and `rematerialize` (step 2) must touch ONLY rows whose scope
   `parse_scope`s to that verse's ULID — **never** a `VERSE#<hex>` interim row. The migration
   stays marker-guarded and runs once per DB. RP-2 owns the seed inventory listed above
   (owned-files): each either stays valid as a legacy hex-keyed test fixture or moves to the
   statement API. Fix the `seed_join_verse` citation (it writes an arbitrary-level row for a
   remote peer, not an owner row) and decide `admin.rs:58`'s membership for the two new tables,
   rematerializing after any `role` clear it still performs.
10. **(Critic.)** `crud.rs` also carries CDL-W4-1's six `record_bypass_for(..)` first-statement
    lines. The owner-row removal must leave them byte-identical (W4-1 invariant).

**Invariants:**
- fe-database AGENTS.md §log-first-commit (the op log before the materialized state).
- §replication-backpressure: `replicate_row` drops and counts, and never blocks.
- §repo (no explicit JSON nulls).
- §schema-macro.
- §hlc (never observe remote HLCs).
- §rbac-policy: decisions go through fe-policy, and the role is fetched before evaluation.
- `resolve_role` stays untouched.

**Flow tests** (DB-level through the public dispatch seam, in-memory schema DB as in
`replicated_row.rs:675`):
- `owner_assigns_editor_and_the_statement_is_published_and_materialized`: drives
  `assign_role_handler`. Asserts a `role_grant` row, a `role` row (read-back), and exactly one
  `ReplicationEvent{table:"role", record_id:"{S}/{P}/{owner}"}` on a test channel.
- `a_manager_grant_then_owner_revocation_of_the_manager_voids_the_grant_locally`.
- `a_manager_revoking_another_managers_grant_reports_the_role_still_held`.
- `refused_assignments_write_nothing` (table-driven): assigning `Owner`; targeting the owner;
  a Manager assigning `none`. Each returns its named error, and writes no `role_grant` row and
  no `ReplicationEvent`.
- `a_petal_manager_assigns_in_their_petal_and_is_refused_in_a_sibling_petal_and_at_the_verse`
  (R7 local parity, T19). The in-petal grant materializes and publishes; the two refusals
  write nothing.
- `an_editor_creating_a_petal_gets_no_implicit_owner_row` (crud flow, T8).
- `implicit_petal_owner_rows_are_dropped_and_the_creator_keeps_only_their_inherited_role`
  (R7 migration, T8). The input is a seeded legacy DB in which an Editor created a petal.
  After migration, that Editor resolves to Editor in the petal, not Owner.
- `non_manager_cannot_change_default_access_or_ts_settings_locally`. It covers the refused cases
  (a Viewer, an Editor and a petal Manager) and the allowed verse-Manager case, where the
  change replicates and the manifest is re-emitted.
- `legacy_role_rows_migrate_owner_rows_dropped_owner_authored_rows_become_statements`
  (migration flow on a seeded legacy DB).
- `elevated_writable_tables_never_include_role_tables` (all three tables).
- **(critic tick 8, AC-42)** `a_revocation_issued_while_the_replica_is_closed_reaches_peers_after_restart`
  and `a_pending_writes_flood_cannot_drop_a_local_revocation` (step 3's durable re-emit).
- **(critic tick 8, AC-45 — replaces the earlier C3 test
  `the_verse_owner_still_answers_manager_plus_to_the_canonical_interim_authority_after_migration`)**
  `interim_authority_rows_survive_the_role_migration_and_rematerialize_across_a_restart`: seed
  hex-keyed interim-authority rows (as the canon_log test fixtures do), run the role migration and
  a `rematerialize` pass, restart the DB, and assert the hex-keyed rows are byte-identical and
  still resolve through `DurableAuthorizationView::load`.

### RP-3: inbound role statements and deferred admission · L · opus

**Owned files:**
- `fe-database/src/handlers/replicated_row.rs`
- `fe-database/src/handlers/inbound_admissions.rs` (new)
- `fe-database/src/handlers/mod.rs`
- `fe-database/src/lib.rs` (the `ApplyReplicatedRow` arm `:1460-1486` uses `InboundAdmissions`)
- `fe-runtime/src/messages.rs` (`ReplicatedRowOutcome::Deferred`, `:696-710`)
- `fe-test-harness/src/peer.rs` (its `ApplyReplicatedRow` arm `:1041` uses the same
  `InboundAdmissions`; compile-level only)
- every exhaustive match on `ReplicatedRowOutcome` (grep the workspace)
- **(Critic — earlier-phase tests whose `Denied` expectations this slice flips):**
  `fe-database/tests/petal_render_distance_test.rs` (PAS-P3P4 S6),
  `fe-database/tests/device_payload_test.rs` (CDL-SPEC10 A3), and
  `fe-database/tests/device_enrolment_test.rs` (CDL-W4-8; re-point only if
  `apply_replicated_row_handler`'s signature changes).

**Steps:**
1. Add a `"role"` arm in `apply_row_mode`, routed to `apply_role_statement`:
   - parse the key;
   - run `fe_policy::validate_statement`. Before the manifest it runs with `owner_did: None`.
     Any `Err` returns `Denied`;
   - call `role_grants::place_statement` (RP-2) and map the result:
     - `Admitted` → `Applied`, then `repartition`, then retry the content `AuthorRole` waiters
       of the changed DIDs;
     - `Superseded` (older than the live version) → `Applied`, idempotent, with no write;
     - `Held` → `Deferred AuthorRole`, or `Deferred Manifest` before the manifest. The row is
       persisted in `role_statement_pending`;
     - `RefusedOverBudget` → `Deferred AuthorRole`, not persisted. The counter rises, and the
       replica keeps the statement for the §2.5 re-read;
   - Role statements bypass the Editor+ write gate. Their gate is the validator plus
     placement. They never enter `InboundAdmissions`, and they are never deferred on `Parent`
     (§2.4).
   - The manifest-apply path calls `repartition` once the owner is known, which is what
     promotes the pre-manifest holds.
2. Add `InboundAdmissions` (§2.4, content rows) as a pub struct. Its `apply(&mut self, db, ...)` is the ONE
   entry point that both the production loop and the harness loop call. That keeps the
   "statement-parity isn't enough" contract at `replicated_row.rs:36-40`.
3. The A3 gate's insufficient-role branch returns Deferred `AuthorRole` instead of `Denied`.
   `Denied` remains for structural violations.
4. Add `ReplicatedRowOutcome::Deferred`. Fix every exhaustive match (memory: the harness
   exhaustive-match gotcha).
5. **(Critic — preserve the arms earlier phases landed in `replicated_row.rs`.)**
   - CDL-W4-8's `canon_device_enrolment` branch stays **first**: verify, then
     `author_did == principal_did`, independent of role — a signature or self-authorship
     failure stays `Denied`, and the Editor+ gate never applies to this branch. **(Critic tick
     8, AC-44(a)-(c) — supersedes the W4 plan §9 wording "never `Deferred AuthorRole`" while
     keeping its intent.)** Membership = any effective role at any scope of the verse
     (`EffectiveRoles::has_any_role`, RP-1). A floor failure (the enrolling principal holds no
     membership anywhere in the verse) is `Deferred AuthorRole`, retried when the principal's
     role rises — not `Denied` — because a brand-new petal-scoped member must still be able to
     enrol even though they resolve to `None` at verse scope. A pre-manifest enrolment (no owner
     known yet) is `Deferred Manifest`, closing the unfloored pre-manifest residual
     (`canon_log/AGENTS.md:746-748`). Revocations stay unfloored and first.
   - CDL-SPEC10 A3's `device_payload` (union), anchored `asset` (insert-if-absent) and
     `device_format` (Manager+) arms keep their semantics. Only their insufficient-role branch
     turns `Deferred AuthorRole`, like every other table.
   - PAS-P3P4 S6's petal `render_distance`/`terrain` shape checks stay `Denied`, because they are
     structural.

**Invariants:**
- Loop-free by construction: no replication sender in the inbound path
  (`replicated_row.rs:4-6`).
- Roles are resolved from local tables and never from the wire; the role payload is only a
  *statement*, and its effect is computed locally.
- The timeseries union semantics (§replication-mode) are unchanged.
- The self-drain rule: the deferred retry must not await any channel the DB loop drains
  (fe-sync AGENTS.md §inbound-apply "Self-drain capacity rule", by analogy).

**Flow tests:**
- `an_owner_role_statement_replicated_in_lets_the_editors_deferred_node_row_apply_without_replay`.
  This replaces the manual-replay test at `:1168`. The order is the node row (Deferred, not
  persisted), then the owner's statement, after which the node is durable with no second
  delivery.
- `a_not_yet_managers_statement_is_deferred_then_admitted_on_promotion` (R7). This is
  table-driven over where the promotion comes from: an inbound owner statement, or a local
  `AssignRole` by the owner.
  - Before the promotion: M's statement returns `Deferred`, sits in `role_statement_pending`
    (not `role_grant`), and has no effect.
  - After the promotion: the statement is in `role_grant`, its grantee's role materializes,
    and pending is empty, with no second delivery.
- `role_statement_delivery_order_never_changes_the_final_role_set` (R7 reordering
  convergence, T22). It uses six statements:
  1. the owner makes M1 verse Manager;
  2. M1 makes M2 Manager of petal P1;
  3. M2 grants X Editor in P1;
  4. M2 grants Y Editor in sibling petal P2, which is never effective;
  5. the owner revokes M1, which cascades: M2 and X lapse;
  6. the owner makes M2 Manager of P1 directly, so X returns.

  Statements 1 and 5 share a key, so the owner's HLC orders them, and whichever arrives
  second is either applied or superseded. It runs all 720 delivery orders, each into a fresh in-memory DB. Drop to a seeded sample of
  at least 120 orders only if the run exceeds 60 s. It asserts identical `role` tables and
  identical `role_grant`/`role_statement_pending` partitions.
- `held_role_statements_survive_restart` (R7, T22). It uses a persisted SurrealKV DB in a temp
  dir. M's statement is held, then the DB is dropped and reopened. The owner's promotion then
  arrives, and M's statement is admitted with no re-delivery.
- `a_hostile_flood_cannot_evict_or_starve_a_held_role_statement` (R7, T12).
  - M's statement arrives first and is held.
  - Then 8 hostile author DIDs send 1100 structurally valid statements each. That is beyond
    both the old 4096-per-verse cap and each hostile author's own 1024 budget. They are
    interleaved with 5000 deferred content rows, which overflow `InboundAdmissions`.
  - Then the owner's promotion of M arrives.
  - It asserts that M's statement is admitted and that no hostile statement is ever admitted.
  - It asserts that `role_pending_refused_count()` counts only the hostile grantors' overflow.
  - It asserts that `deferred_admission_drop_count() > 0`, which shows the content queue
    overflowed without touching role statements.
- `concurrent_revocation_converges_in_both_delivery_orders` (T4, DB-level). Two fresh DBs get
  the same statements in opposite orders: a grant by the revoked Manager, the petal Manager
  that Manager appointed, and the owner's revocation. The `role` tables are equal, and so are
  the partitions.
- `an_inbound_petal_manager_grant_applies_in_their_petal_and_is_held_in_a_sibling_petal`
  (R7 subtree, inbound).
- `an_owner_statement_for_a_petal_not_yet_known_takes_effect_when_the_petal_lands` (no
  `Parent` deferral for role statements, and no re-delivery).
- `role_statements_before_the_manifest_are_held_and_resolve_when_it_lands`. A pre-manifest
  statement that targets the owner is deleted as Denied at promotion.
- `a_replayed_older_owner_grant_never_undoes_the_newer_revocation` (T2).
- `a_forged_grantor_key_and_a_cross_verse_scope_are_denied` (T1/T6, table-driven).
- `an_empty_entry_on_the_role_table_is_not_applicable`.
- `the_deferred_queue_caps_per_author_and_counts_drops` (content rows).
- The existing `viewer_role_row_is_denied_never_applied` (`:1103`) and
  `timeseries_reading_from_viewer_is_denied` (`:1698`) flip to `Deferred` (never persisted).
  `unknown_peer_row_is_denied` (`:1133`) flips the same way. Rename them accordingly. **(Critic)**
  The same flip applies to the viewer-authored rows in PAS-P3P4 S6's and CDL-SPEC10 A3's test
  files, which this slice now owns.
- **(critic tick 8, AC-44 — renamed from `a_viewer_s_self_signed_enrolment_still_applies_and_is_never_deferred`)**
  `a_viewers_enrolment_never_needs_editor_and_a_pre_membership_enrolment_is_deferred_then_applied`:
  a Viewer's self-signed enrolment applies with no floor and is never deferred (W4-8 stays ahead
  of the A3 role gate); a principal with no membership anywhere in the verse is `Deferred
  AuthorRole` and applies once any role lands for them; a pre-manifest enrolment is `Deferred
  Manifest` and applies once the manifest lands.

### RP-4: owner integrity, P2P-REF-CONFINE, scope-precise admission · L · opus

**Owned files:**
- `fe-database/src/handlers/replicated_row.rs` (`admit_inbound_row`, `apply_static_row`)
- `fe-database/src/invite.rs` (adds `owner_did` to the signed bytes)
- `fe-database/src/handlers/invite.rs`
- `fe-database/src/handlers/inbound_admissions.rs` (the `Parent` reasons)
- `fe-database/src/AGENTS.md` (§handlers `replicated_row` paragraph)
- **(Critic tick 4, AC-21(d).)** `fe-database/src/handlers/petal_terrain.rs` — adds the
  missing Editor+ gate so a local edit can never diverge from what peers will accept.
- **(Critic tick 8, AC-46.)** `fe-database/src/lib.rs` (`lifecycle_auth_context :240`, and the
  replicated-write dispatch arms `InsertReadings`, `InsertDevicePayloads`,
  `RegisterDeviceFormat`, `SetPetalTerrain`, `SetPetalRenderDistance`).

**Steps:**
1. **Immutable identity.** For an inbound verse row on an existing verse, any change to
   `verse_id`, `created_by`, `created_at` or `namespace_id` returns `Denied`.
2. **Governance fields.** A change to `default_access`, `ts_mode`, `ts_replication_factor` or
   `ts_bucket_width_ms` needs the author to be Manager+ **at the verse scope** (else Deferred
   `AuthorRole`). A petal or fractal Manager's change is held like anyone else's, because verse
   governance lies above their subtree. `name`/`view_hidden` follow the Editor+ write gate.
3. **First-manifest rule** (narrowing the bootstrap window).
   - Before a manifest exists, ONLY the verse row is admitted. Every other table is Deferred
     `Manifest` (safe now that deferral retries; the old reason for admitting everything,
     `:239-249`, no longer holds). Role statements are the exception: they are held in the
     durable pending store (RP-3, §2.4), not in the in-memory queue.
   - If an owner pin exists for the verse (step 4), a first manifest whose `created_by`
     differs is **Deferred `Manifest`** (critic tick 8, AC-43(3) — supersedes the earlier
     "Denied" default: still never applied, but retried, consistent with "every other table is
     Deferred Manifest" above).
   - Without a pin it is trust-on-first-use (TOFU), as today; this is a documented residual
     for relays (§6).
   - **(Critic tick 8, AC-44(b).)** A pre-manifest `canon_device_enrolment` row (no owner known
     yet) is also `Deferred Manifest`, not left unfloored forever — it applies once the
     manifest lands and the floor (RP-3 step 5) can be evaluated.
4. **Invite owner pin.**
   - `VerseInvite` gains `owner_did`, inside the signed canonical bytes.
   - `generate_verse_invite_handler` fills it from `verse.created_by`.
   - `join_verse_by_invite_handler` writes `created_by = owner_did` (a `did:key`, replacing
     the hex inviter at `:167`). `invited_by` keeps the inviter.
   - This is a breaking invite-string change, which is fine with no consumers.
5. **Referential confinement** (P2P-REF-CONFINE, `replicated_row.rs:251-260`). Before the
   role check, resolve the row's parent chain against the local store. **Both** the payload's
   reference **and** an existing durable row's current parent must be in the replica's verse.
   - `fractal`: `payload.verse_id == verse`.
   - `petal`: `fractal_id` is required (`Option` in the schema at `schema.rs:77`; absent means
     Denied) and must be a fractal of this verse.
   - `node`: `petal_id` must be a petal of this verse. A tombstone or partial row without a
     `petal_id` uses the durable row's petal; if the node is unknown, the row is Deferred.
   - `iot_reading`: `petal_id` must be in this verse, and `node_id` must be in that petal.
   - **(Critic — tables landed by earlier phases.)** CDL-SPEC10 A3 already denies cross-verse
     references for `asset.petal_id`, `device_payload.{petal_id, node_id, asset_ids[*]}` and
     `device_format.verse_id`. This slice reuses that same parent-chain resolver, so there is one
     confinement core and not two, and changes A3's "unknown locally → Denied" to Deferred
     `Parent`. `canon_device_enrolment` is principal-keyed and verse-agnostic, so it is exempt
     from confinement (CDL-W4 §8 assumption); it stays verify-first.
   - A parent unknown locally means Deferred `Parent`. A parent in another verse means Denied.
   - **(Critic tick 8, AC-43(1)-(4) — `r5_referential_confinement` must be three-valued, not
     boolean.)** `node_is_in_petal`, `asset_anchored_to_petal`, `fetch_format`,
     `asset_has_content_type` and `payload_in_petal` each conflate "known foreign/mismatched"
     with "unknown locally" behind one `Option<String>`/boolean return. Split every helper into
     three outcomes:
     1. **Known-ok** — the parent exists locally and is in this verse;
     2. **Known-foreign-or-mismatched** — the parent exists locally and is NOT in this verse (or
        the row's shape otherwise mismatches it) → **Denied**;
     3. **Unknown** — no local row for the parent yet → **Deferred `Parent{table, id}`**.
   - Add wait reasons and retry triggers for `device_format` (verse, format_id), `asset`
     (asset_id) and `device_payload` (payload_id), in addition to the existing `node` retry
     trigger — so a `device_payload` before its `device_format`, a `device_payload` before its
     `asset`s, and an `iot_reading` before its `device_payload` are deferred and applied, not
     denied (the W4 residuals this closes).
   - State the gate order explicitly: verse id check → enrolment verify (RP-3 step 5) →
     Manifest wait (step 3 above) → confinement (this step) → role gate (step 6 below).
   - Tests: `a_device_payload_before_its_format_is_deferred_then_applied`,
     `a_device_payload_before_its_assets_is_deferred_then_applied`,
     `a_reading_before_its_payload_is_deferred_then_applied`, and
     `a_payload_naming_a_known_foreign_asset_is_denied_not_deferred` (the known-foreign case
     stays Denied, never Deferred).
6. **Scope-precise role resolution.** Resolve the author's role at the row's deepest scope
   (`build_scope(verse, fractal, petal)`). Verse-level Editors still pass because
   `resolve_role` walks up the chain. Over the §2.2 cache, this is `role_at`, the same function
   that grant authority and the sync-plane gates use (§2.5). An Editor appointed by a petal
   Manager therefore passes in that petal and is Deferred elsewhere.
7. **(Critic tick 4, AC-21(d) — the missing local write-path gate.)**
   `set_petal_terrain_handler` (`fe-database/src/handlers/petal_terrain.rs:16`) never calls
   `authorize_edit`, although W1 S6 (PAS-P3P4) made petal terrain replicate and op-log the same
   as `petal_render_distance.rs`. Today a Viewer's local `SetPetalTerrain` writes and replicates
   the row; every peer's own write-path check (`is_valid_inbound_petal_terrain` plus the row's
   role gate, §6 step 6 above) then denies the inbound echo, so the Viewer's local state
   silently diverges from the verse with no error surfaced. Fix: give
   `set_petal_terrain_handler` the same shape as `set_petal_render_distance_handler`
   (`petal_render_distance.rs:44-56`) — thread `local_did: &str` and `auth: &CallerAuth`,
   resolve the petal's scope with `resolve_petal_scope_handler`, and call
   `crate::handlers::visibility_groups::authorize_edit(db, local_did, auth, scope,
   "SetPetalTerrain")` before the strict-shape check and before the op-log entry is built. The
   `DbCommand::SetPetalTerrain` dispatch site picks up the caller's identity/auth the same way
   the render-distance command already does.
8. **(Critic tick 8, AC-46 — a stale API token must not outrun a remote demotion.)** API writes
   act on the token's `max_role` (`lib.rs:223-231,:260`; `fe-api/src/endpoint.rs:49-53`), capped
   only at mint (`handlers/api_token.rs:55-69`) and living up to 30 days. Peers judge the node
   DID's converged role via A3. Once RP introduces remote demotion, a demoted node's API keeps
   writing rows that apply locally and that every peer defers — silent divergence (the existing
   C8 test only covers a Local Viewer; an API-path `SetPetalTerrain` diverges the same way; the
   device/IoT doors have no DB-side role check at all, `lib.rs:1494-1524`). **Default (record in
   RUNBOOK §4):** for replicated writes, the effective API role is
   `min(token max_role, role_at(local_did, scope))`, applied by one helper used by
   `lifecycle_auth_context` (Identified) and every replicated-write door (`InsertReadings`,
   `InsertDevicePayloads`, `RegisterDeviceFormat`, `SetPetalTerrain`, `SetPetalRenderDistance`).
   Test: `an_api_token_minted_before_the_nodes_demotion_cannot_write_after_it`. Reversal: cheap
   (drop the cap).

**Invariants:**
- §geometry-inserts (strip, then cast).
- N-4 tombstone dominance (`merge.rs`).
- The scope-injection guard at `:273-295` stays.
- The F20 `default_access` set stays {viewer, none}.
- Invite signature verification precedes any write.

**Flow tests:**
- `an_editor_cannot_rewrite_created_by_through_a_replicated_manifest` (T9; it would have
  passed before this slice — the hole).
- `an_editor_or_petal_manager_cannot_flip_ts_mode_or_default_access_but_a_verse_manager_can`.
- `an_invitee_records_the_real_owner_and_admits_the_owners_rows_as_owner` (T10). The invite
  is generated by a non-owner Manager, and the owner's later petal row is Applied on the
  invitee.
- `a_tampered_invite_owner_did_fails_signature_verification`.
- `a_pinned_owner_rejects_a_first_manifest_naming_another_owner`.
- `a_node_row_referencing_a_foreign_verses_petal_is_denied` (P2P-REF-CONFINE acceptance).
- `a_tombstone_aimed_at_a_foreign_verses_node_is_denied_and_the_node_survives` (T11).
- `a_node_reparent_from_a_foreign_verse_is_denied`.
- `a_petal_row_whose_fractal_arrives_later_is_deferred_then_applied`.
- `a_petal_scoped_editor_is_admitted_for_that_petal_and_deferred_elsewhere`.
- The existing `node_row_before_verse_manifest_applies_bootstrap` (`:1324`) flips to
  `node_row_before_verse_manifest_is_deferred_then_applied_when_the_manifest_lands`.
- **(critic)** `a_device_payload_whose_petal_arrives_later_is_deferred_then_applied` (A3's
  unknown-parent row turns into Deferred).
- **(critic)** `a_pre_manifest_enrolment_row_is_verified_first_and_a_forged_one_is_denied` (the
  narrowed bootstrap window does not reopen W4 risk 2).
- **Owned-file addition (critic):** `fe-database/tests/device_payload_test.rs` (CDL-SPEC10 A3
  unknown-parent rows).
- **(critic tick 4)** `a_viewers_local_set_petal_terrain_is_refused_and_nothing_is_written`
  (AC-21(d)): a Viewer-scoped caller's local `SetPetalTerrain` returns the `authorize_edit`
  error and the petal row's `terrain` column is unchanged — no op-log entry, no replication
  send.
- **(critic tick 8, AC-43)** `a_device_payload_before_its_format_is_deferred_then_applied`,
  `a_device_payload_before_its_assets_is_deferred_then_applied`,
  `a_reading_before_its_payload_is_deferred_then_applied`, and
  `a_payload_naming_a_known_foreign_asset_is_denied_not_deferred` (three-valued confinement).
- **(critic tick 8, AC-46)** `an_api_token_minted_before_the_nodes_demotion_cannot_write_after_it`:
  a token minted while its node held Manager, after the node is demoted to Viewer, fails its
  next replicated write with the `min(token max_role, role_at)` cap.

### RP-4b: STREAMED-TERRAIN-PIN · M · opus (critic tick 8, AC-47 — promotes §8 "Tick 5" C10 into a scheduled slice)

**Owned files:**
- `fractalengine/src/terrain_bridge.rs` (`drive_streamed_tilesets` assembles the pin chain at
  `~:1312-1321` — the petal-authored source is inserted here, between installed and TOFU)
- `fe-terrain/src/petal_binding.rs` (`terrain_config_from_petal_json`)
- `fe-terrain/src/config.rs` (`TerrainConfig`)
- `fe-ui/src/plugin.rs` (`tileset_to_terrain_json`)
- the fe-api terrain validator (DEC-C22 strictest reader)
- `fe-database/src/handlers/replicated_row.rs` (`is_valid_inbound_petal_terrain`: add `did:key`
  grammar validation for the new publisher-pin field)
- `fe-terrain/src/tiles/streamed.rs` — **no change expected**: W2b (`474ab9b`) already threads
  `pin` through `from_meta_json`/`from_header_doc`.

**Steps:**
1. W2b F1 moved streamed-meta verification off the main thread and pins to a locally installed
   publisher. This slice supplies the remaining pin source: a publisher DID carried in the
   Editor-authored petal terrain row, which needs RP-4's Editor+ gate on
   `set_petal_terrain_handler` (§3 RP-4 step 7) to be trustworthy before a streamed
   (never-installed) tileset can pin to it.
2. **Precedence (critic tick 8, AC-47 default — record in RUNBOOK §4 now):** installed >
   petal-authored > TOFU. A petal-authored pin REPLACES a recorded first-contact TOFU pin;
   TOFU never overrides a petal-authored pin.
3. **(Decision carried from critic tick 6, AC-31, defaulted here.)** The streamed path records
   no TOFU pin of its own today — the W2b reviewer's unfixed LOW notes that with nothing
   installed or known locally, the first header wins and a later different-publisher header in
   the same session is also accepted. **Default: record the first-contact pin** in
   `KnownTilesetPublishers` (symmetric with the installed path's TOFU). Reversal: drop the
   recorded pin and rely only on the petal-authored pin (cheap — delete the write, keep the
   read).
4. Validate the publisher-pin field's `did:key` grammar in `is_valid_inbound_petal_terrain`
   (house rule: strictest reader) — a malformed pin is Denied, not silently ignored.

**Gate:** the workspace triple + feature matrix (header), and this slice additionally runs the
`fe-terrain` `fetch,render` feature combination, since it touches the streamed-tileset path.

**Flow tests:**
- `a_streamed_tileset_signed_by_a_publisher_other_than_the_petal_maps_pin_is_refused` (the named
  acceptance test from critic tick 5).
- `a_second_streamed_publisher_for_the_same_id_is_refused_in_session` (first-contact TOFU pin).
- `a_petal_authored_pin_overrides_an_earlier_first_contact_tofu_pin` (AC-47 precedence).
- `an_inbound_petal_terrain_with_a_malformed_publisher_pin_is_denied` (AC-47, did:key grammar).

**Threat matrix additions (RP-9):** T24 (a streamed publisher other than the petal pin), T25
(first-contact TOFU race — a second publisher's header in the same session).

### RP-4s: DB-plane SECURITY review-and-fix · M · opus (critic tick 8, AC-41; fresh context, over RP-0..RP-4b, before RP-5)

**Owned files:** any file touched by RP-0..RP-4b, with fixes only (a reviewer fixes its own
findings, per the standing rule), plus
`conductor/tracks/role_propagation_20261013/review-rp4s.md` (a one-line verdict and findings
list; a track-local record, not a report file for the parent).

**Why a review here, not only at RP-9.** The plan originally had ONE review (RP-9) after every
build slice, with no correctness pass. W4 (CDL-SPEC10) needed three reviews — secA caught 2
MANDATORY authz findings, secB 2 HIGH functional findings, and the correctness pass a HIGH that
both security passes missed (every unlinked `iot_reading` insert would have failed). RP rewrites
the A3 gate for every replicated table, and tests run only at the campaign-end sweep, so a
functional break introduced here would silently underlie RP-5..RP-8 before anything catches it.

**Steps:** Run `/security-review` scoped to the RP-0..RP-4b diff. It must try to break:
- the A3 gate rewrite for every table (`role`, `device_format`, `device_payload`, `asset`,
  `iot_reading`, `node`, `petal`, `fractal`);
- the three-valued confinement split (AC-43) — that Known-foreign-or-mismatched never leaks
  into Deferred, and Unknown never leaks into Denied;
- the enrolment floor and its Deferred `AuthorRole` / Deferred `Manifest` split (AC-44);
- the AC-42 durable re-emit (that it cannot be used to replay a stale grant past a later
  revocation);
- the AC-46 API-token cap (that it is applied on every replicated-write door, not just the
  ones named);
- the migration's scope confinement (AC-45) — that it cannot be tricked into touching an
  interim `VERSE#<hex>` row.

Fix any finding directly (reviewer fixes its own findings, standing rule). Record a one-line
verdict in `review-rp4s.md` and mirror it into RUNBOOK §2's review ledger.

### RP-5: sync-plane role view and gates · L · sonnet (opus if the quarantine gets hairy)

**Owned files:**
- `fe-sync/src/sharding.rs` (VerseFabric role view, owner pin, petal map, quarantine, gated
  `note_entry`/`note_verse_row`)
- `fe-sync/src/replicator.rs`:
  - snapshot: add an all-authors `Query::key_prefix("role/")` pass for the real doc, and
    make the mock keep per-(key, author) entries for the `role/` prefix;
  - add the provided `VerseReplicator::entries_by_author` method, which the iroh replicator
    overrides (§2.5).
- `fe-sync/src/sync_thread.rs`:
  - feed outbound `role`/`petal` rows to the fabric in `handle_write_row_entry`;
  - `AnyReplicator` forwards `entries_by_author`;
  - the select loop drains `reread_queue`.
- `fe-sync/src/distributed_query.rs` (the requester Viewer+ gate in `handle_gossip_incoming`,
  `:1076`)
- `fe-sync/src/AGENTS.md`:
  - §sharding ledger admission;
  - §transport-admission-control item 5, rewritten as CLOSED with the residual removed;
  - a new §role-view.
- Not edited: `fe-sim/src/net.rs` (`SimVerseReplicator`) inherits the provided method's
  default.
- **(Critic tick 4, AC-21(e).)** `fe-sync/src/docs_engine.rs` — the blob-ALPN request filter.
- **(Critic tick 4, AC-21(e).)** `fe-sync/src/AGENTS.md` §tileset-security "Risk 2" paragraph,
  resolved (its own revisit trigger fires in this slice).

**Steps:**
1. Implement §2.5 using `fe_policy::role_grants`:
   - the fabric retains only admissible statements;
   - every gate calls `role_at` at the row's deepest scope;
   - quarantine caps are 1024 per verse, drop-oldest with a counter.
2. Gate `note_verse_row` on Manager+ at the verse scope (or the owner). Owner-pinned identity is
   never relearned.
3. Snapshot: role statements must be read from ALL authors. Otherwise an Editor writing the
   key `role/S/P/{owner}` with a later iroh timestamp would hide the owner's genuine entry
   from `single_latest_per_key`, a DoS on convergence for fresh peers. The gate drops the
   forgery; the all-authors pass keeps the genuine entry visible.
4. **Re-read on empowerment** (§2.5, the R7 durable retry for the fabric, and the DB plane's
   backstop):
   - add `reread_queue`, filled from the fabric's `ChangedDids` rises;
   - add `entries_by_author` (a provided method whose default filters `snapshot()`;
     `AnyReplicator` forwards it; `IrohDocsReplicator` overrides it with
     `Query::author(id).key_prefix(p)`);
   - the drain feeds `handle_inbound_row_change`.
5. Add a `drop_reason::INSUFFICIENT_ROLE` for a requester below Viewer at the requested
   petal's scope. It fires when `default_access = none` and no grant reaches that petal.
6. **(Critic tick 4, AC-21(e) — the blob-ALPN request filter, deferred at W1 S8b.)**
   `fe-sync/src/AGENTS.md` §tileset-security "Risk 2" records the exact revisit trigger:
   "when RP replicates roles, intercept `RequestMode` with an `EventSender` that admits a
   connection whose `EndpointId` maps to a role in any open verse." This slice is that moment
   — it already builds the sync-plane role view (`role_at` over the fabric) the filter needs.
   Implement it: `BlobsProtocol::new(&store, Some(event_sender))` where the `EventSender`
   resolves the requesting `EndpointId`'s DID (the existing P2P identity mapping) and admits
   the request only if that DID holds Viewer+ in some open verse whose role view currently
   accounts for the requested hash (via the `TilesetProviderMap`/download ledger bookkeeping
   `TilesetTransfer` already maintains, so row-entry content and `two_peer_blob_exchange`
   fetches that carry no tileset context are unaffected, exactly as the risk note requires). A
   requester with no role in any open verse is refused.

**Invariants:**
- The §inbound-apply self-drain rule: quarantine re-evaluation is synchronous and never awaits
  an aggregated-stream send. The re-read awaits only the local docs store.
- `note_entry` stays BEFORE the own-author filter (`sync_thread.rs:1110-1118`).
- Retention of an unknown shard stays "retain" (§sharding).
- §transport-admission-control items 1-4 stay.
- One authority function: the fabric never resolves roles any way except
  `fe_policy::role_grants` `role_at`.

**Flow tests:**
- `a_viewer_authored_shards_row_is_quarantined_and_admitted_once_the_owner_grants_editor`.
- `a_forged_shards_row_from_a_stranger_never_reaches_the_ledger`.
- `a_petal_scoped_editors_shards_row_is_admitted_for_that_petal_only` (R7 subtree, on the
  sync plane). The Editor is appointed by a petal Manager.
- `manifest_settings_are_adopted_only_from_a_verse_manager_or_the_owner` (table-driven).
  Settings from a Viewer or a petal Manager are ignored; settings from a verse Manager are
  adopted.
- `snapshot_replay_sees_the_owners_role_statement_despite_a_later_forged_entry_on_the_same_key`
  (real loopback replicator, two authors).
- `a_held_role_statement_is_reread_from_the_replica_when_its_grantor_is_empowered` (R7).
  M's statement reaches the fabric before the owner's grant and is not retained. The grant
  then arrives. The re-read retains M's statement and re-forwards it on the DB command
  channel.
- `a_quarantine_flood_cannot_lose_a_legit_editors_shards_row` (T12). The legit row is
  quarantined. Then 1024 or more strangers' rows evict it. Then the owner grants Editor, and
  the legit row reaches the ledger through the re-read.
- `requester_gate_resolves_at_the_requested_petal` (table-driven, with
  `default_access = none`):
  - a petal-scoped Viewer is served for that petal and dropped for a sibling petal;
  - a verse Viewer is served everywhere;
  - a stranger is dropped.
- `fabric_and_db_materialize_identical_effective_roles_from_the_same_statements` (cross-plane
  parity on a shared fixture). The fixture includes subtree grants, an owner pin, and a
  Manager `viewer` statement that must write no cache row.
- **(critic tick 4)** `a_verse_stranger_is_refused_the_blob_alpn_and_an_open_verse_viewer_is_served`
  (AC-21(e)): two requesters dial the same provider over the blob ALPN for a tileset root the
  provider is serving — the caller holding Viewer+ in the open verse that root belongs to is
  served; the caller with no role in any open verse is refused; the existing row-entry /
  `two_peer_blob_exchange` fetch inside an open verse is unaffected.

### RP-6: settlement race closed · L · opus

**Owned files:**
- `fe-sync/src/distributed_query.rs` (planner `expected_answerers`, holder wait, settlement,
  merge filtering, meta)
- `fe-sync/src/sharding.rs` (`HOLDERS_TABLE`, `note_holder_row`, `declared_holdings`)
- `fe-sync/src/sync_thread.rs` (publish the `__holders` row on the outbound path, after the
  ledger and before the reading)
- `fe-runtime/src/distributed_query.rs` (`TsPartialRows.deferred_shards`, serde-default; meta
  `incomplete_shards`, `missing_holders`, `rejected_answerers`, `unverified_answerers`, all
  serde-default)
- `fe-database/src/handlers/ts_partial.rs` (fill `deferred_shards` from `InboundAdmissions`)
- `fe-database/src/lib.rs` (`ExecuteTsPartial` arm passes the queue)
- `fe-test-harness/src/peer.rs` (same, compile-level)

**Steps:** implement §2.6. Holders are re-learned after a restart from the snapshot's own rows,
as the ledger already is.

**Invariants:**
- Commutative merges (§distributed-query "Commutative merges").
- Failed ≠ covered-empty (F23).
- The diagnostics echo stays `try_send`.
- The `PendingQueries` duplicate refusal.
- `GOSSIP_ENVELOPE_BUDGET`.
- Ledger-before-row publish ordering.

**Flow tests:**
- `settlement_waits_for_a_declared_holder_even_when_the_host_answers_empty_first`. This is the
  previously-residual order, and it now passes. Both the aggregate and the raw leg run over the
  in-process wire from `:2601`.
- `a_silent_holder_is_reported_missing_after_the_holder_wait_not_the_full_deadline`. It
  asserts on elapsed time, measured against `HOLDER_WAIT_MS`.
- `a_host_with_deferred_rows_does_not_settle_the_shard_on_a_covered_empty`.
- `rows_from_a_sub_editor_unexpected_answerer_are_discarded_and_reported`.
- `a_viewer_cannot_declare_itself_a_holder`.
- `holder_declaration_is_published_once_per_shard_and_relearned_after_restart`.

### RP-7: responder filtering re-enabled, pinning test flipped · S · sonnet

**Owned files:** `fe-sync/src/distributed_query.rs` (`handle_gossip_incoming` and
`respond_to_compute_request`, `:1076-1276`, plus tests), `fe-sync/src/AGENTS.md`
§distributed-query ("Creator copies are load-bearing" rewritten).

**Steps:** implement the §2.6 responder filter. A request whose filter leaves nothing drops
with `drop_reason::NOT_HOSTED` (named, unit-testable).

**(Critic — RUNBOOK §3 sequencing.)** §3 (2026-10-12) reads: *"After it lands: fix the
distributed-query settlement race, then consider re-enabling responder filtering"*, and §7 step 4
reads *"only then reconsider responder filtering"*. Because tests run only at campaign end, RP-7 is
landed **provisionally** and as its own revertible commit. RP-9's critic pass must explicitly
endorse re-enabling, which is the "consider". If any RP-6 settlement test or the RP-8
`distributed_query` scenario is red in the final sweep, RP-7 is reverted on its own and the old
pinning test is restored. It never ships on a red RP-6.

**The test that must flip.** `a_non_host_authors_creator_copy_serves_rows_its_denying_host_refused`
(`:2659`) is rewritten as
`a_declared_holder_serves_its_creator_copy_and_a_non_holder_stays_silent`.
- bob is the author and a declared holder; carol is the host, answering empty with
  `deferred_shards=[shard]`; dave is neither.
- The test delivers **the host first**, the order the old test could not survive.
- It asserts that bob's rows are in the outcome and that dave sends no frame within 300 ms.
- It fails if the filter regresses to hosted-only (bob goes silent) or to no filter (dave
  answers).

Add `the_responder_answers_an_unknown_ledger_shard` (the safe direction).

### RP-8: harness end-to-end · M · sonnet

**Owned files:**
- `fe-test-harness/src/peer.rs` (`AssignRole`/`RevokeRole` call the real `fe-database` handlers
  with a replication sender, replacing the no-op at `:572-582`)
- `fe-test-harness/src/scenarios/role_propagation.rs` (new)
- `fe-test-harness/src/scenarios/mod.rs`
- `fe-test-harness/src/scenarios/distributed_query.rs`
- `fe-test-harness/src/scenarios/invite_flow.rs`
- `fe-test-harness/src/main.rs` (register)
- `fe-test-harness/src/AGENTS.md`
- **(Critic tick 8, AC-48.)** `README.md` and `conductor/final-sweep-manifest.md` — this slice
  adds the harness's 18th scenario (17→18) and both docs must say so before the final sweep
  runs against them.

**Flow scenarios** (real loopback transport):
- **`role_propagation`** (three peers; alice is the owner):
  - alice grants bob Editor. carol, a fresh peer, admits bob's node row and reading with no
    local provisioning. This is the SYNC-ROLE-PROPAGATION acceptance.
  - alice grants bob Manager. bob grants dave Editor, and carol admits dave's row.
  - The petal-scoped leg (R7 subtree, end to end):
    - alice makes a member Manager of petal P1 only;
    - that Manager grants another identity Editor in P1;
    - carol admits that identity's P1 row, and Defers its row in sibling petal P2.
  - alice revokes bob, and dave's next row is Deferred on carol (cascade, T4).
  - a node row from bob in verse A referencing verse B's petal is denied on carol
    (P2P-REF-CONFINE, two-peer).
- **`distributed_query`:**
  - before ingest, alice (the owner) grants bob and carol Editor through `AssignRole`. That is
    the production flow, not local seeding.
  - a new leg 0 asserts that every ledger host's DB holds every row of the shards it hosts,
    polled against the authors' ground truth. This is the ledger's "hosts holding every
    planned shard".
  - leg 1 adds the host-first delivery order. Leg 5a still passes, now through alice's holder
    declaration.
- **`invite_flow`:** bob's verse row has `created_by == alice_did` (a `did:key`), and alice's
  later petal row is Applied on bob.

### RP-9: SECURITY review-and-fix · M · opus (fresh context; a separate pass that refutes rather than confirms)

**Scope (critic tick 8, AC-41):** every file touched by RP-0..RP-8, including RP-4b. This is
the sync-plane + cross-plane security pass; RP-4s already covers the DB-plane slices
(RP-0..RP-4b) in isolation, so this pass's job is cross-plane interaction and anything RP-5..RP-8
introduce.

**Owned files:** any file touched by RP-0..RP-8 (incl. RP-4b), with fixes only (a reviewer fixes
its own findings, per the standing rule), plus
`conductor/tracks/role_propagation_20261013/review-rp9.md` (a one-line verdict and findings
list; this is a track-local record, not a report file for the parent).

Run `/security-review` scoped to the track diff. Run `oh-my-claudecode:critic` on §2.2 and
§2.4. The critic must try to break five things:
- subtree authority;
- the owner-pin cut;
- the raise-only rule;
- the admission argument ("the partitioned materialization equals the least fixpoint over
  everything received");
- the partition invariant.

Load `conductor-okf:code-styleguides`. **Threat matrix:** each row needs a named test that
fails if its guard is removed; the reviewer adds any that are missing.

| # | Threat | Guard | Test (slice) |
|---|---|---|---|
| T1 | Forged role row (grantor ≠ attested author) | key grantor == entry author | RP-1, RP-3 |
| T2 | Replayed role row (old genuine grant after revoke) | per-grantor HLC last-writer-wins, one live version per key across both tables; iroh latest-per-(key, author) | RP-3 |
| T3 | Manager grants Owner, or self-elevation | `role ≠ owner`; `R ≤ Manager ≤ authority` | RP-1, RP-2 |
| T4 | Revocation race (revoked Manager's concurrent grants, and the subtree Managers they appointed) | cascade via the fixpoint, demotion to pending; order-free | RP-1, RP-3, RP-8 |
| T5 | Demoting or revoking the owner | owner derived from `created_by`, never a target | RP-1, RP-2 |
| T6 | Cross-verse role rows | scope's verse == replica verse | RP-1, RP-3 |
| T7 | HLC skew abuse | no cross-author last-writer-wins; no remote HLC observation | RP-1 |
| T8 | Petal creator becomes Owner | implicit owner rows no longer written; migration drops existing ones (R7) | RP-2 |
| T9 | Manifest MERGE rewrites `created_by` | immutable identity fields | RP-4 |
| T10 | Invitee trusts the inviter as owner | signed `owner_did` pin | RP-4, RP-8 |
| T11 | Cross-verse tombstone or re-parent | referential confinement (payload and durable row) | RP-4, RP-8 |
| T12 | Flooding: role statements, deferred content rows, or quarantined `__shards` rows from many author ids | role statements never use the shared in-memory queue; the durable pending store is keyed by attested grantor, with a per-grantor budget and NO shared cap, so there is no cross-author eviction; the fabric retains only admitted statements and re-reads by author; content-queue caps and drop counters (delay only) | RP-3, RP-5 |
| T13 | Viewer-authored `__shards`/`__holders`/manifest settings, or a petal Manager's verse settings | sync role view gates via `role_at` at the deepest scope; settings at the verse scope | RP-5, RP-6 |
| T14 | Forged later entry hides the owner's statement from snapshots | all-authors `role/` snapshot | RP-5 |
| T15 | Unauthorized creator copy injected into query results | merge rejects sub-Editor unexpected answerers (at the shard's petal scope) | RP-6 |
| T16 (critic) | Cross-verse device-payload / asset / device-format rows (tables from CDL-SPEC10 A3) | shared confinement resolver; Manager+ on `device_format` | SPEC10 A3, RP-4 |
| T17 (critic) | Enrolment branch bypassed or deferred by the new gates | W4-8 branch first, verify-before-anything | RP-3, RP-4 |
| T18 (critic tick 8, AC-45 — replaces the earlier "canonical interim authority loses the owner" row, whose premise was false) | Migration or `rematerialize` touches an interim `VERSE#<hex>` authority row | migration and `rematerialize` only touch rows whose scope `parse_scope`s to the verse's ULID; interim rows are structurally out of reach | RP-2 |
| T19 (R7) | A Manager grants above or beside their own subtree (enclosing fractal or verse, sibling petal), or grants Owner | `authority = role_at` at the target scope (maximum over the scope and its ancestors); `owner` structurally denied | RP-1, RP-2, RP-3 |
| T20 (R7) | A lower-scope Manager demotes a wider-scoped peer inside their subtree with an explicit lower grant that shadows inheritance | Managers only raise: Manager `none` has no effect, and there is no non-raising cache row | RP-1, RP-5 parity |
| T21 (R7) | A verse Manager undoes the owner's carve-out by appointing a petal Manager where the owner pinned them below Manager | owner pins cut the authority walk | RP-1 |
| T22 (R7) | A held role statement is lost (restart, overflow, channel drop), so the final role set depends on delivery order | durable `role_statement_pending` + the partition run on every authority change + startup repartition; sync-plane re-read by author | RP-1, RP-3, RP-5 |
| T23 (critic tick 8, AC-42) | A local grant or revocation is never published (replica closed, or a pending-write flood evicts it) | startup + replica-open durable re-emit of every `role_grant` row whose `grantor_did == local_did` | RP-2 |
| T24 (critic tick 8, AC-47) | A streamed (never-installed) tileset's meta is signed by a publisher other than the one the petal's terrain row names | `is_valid_inbound_petal_terrain` `did:key` grammar check; petal-authored pin overrides TOFU | RP-4b |
| T25 (critic tick 8, AC-47) | First-contact TOFU race: a second, different publisher's header for the same streamed id arrives in the same session | recorded first-contact pin in `KnownTilesetPublishers`; a later different publisher is refused | RP-4b |
| T26 (critic tick 8, AC-46) | A stale API token (minted before a node's remote demotion) keeps writing replicated rows peers defer | effective API role = `min(token max_role, role_at(local_did, scope))` on every replicated-write door | RP-4 |
| T27 (critic tick 8, AC-44) | Enrolment floor order-dependence: a floor failure is wrongly Denied instead of Deferred, or a pre-manifest enrolment is never retried | floor failure → Deferred `AuthorRole`; pre-manifest enrolment → Deferred `Manifest`; signature/self-authorship failures stay Denied | RP-3, RP-4 |
| T28 (critic tick 8, AC-50) | Blob reach by known hash: an inbound row names any `content_hash` and `/assets/by-id` serves it to anyone who asks | **No guard yet — verdict only.** Ledger row BLOB-HASH-CAPABILITY; trigger = a membership-keyed filter for all hashes, or namespace rotation. Not counted as fixed by C9 | — (recorded, not fixed) |

The review must also challenge:
- whether the promote/demote worklist or the sync re-read can loop or amplify. The expected
  answer: promotions only raise `M`, each round moves at least one row, and re-reads happen
  only on role rises, which need a valid grant;
- whether the `role` cache or the partition can drift from `role_grant`. Cover a crash
  between the statement write and `repartition`, and the startup repair;
- whether a hostile author can occupy another grantor's pending keys, or force a scan of its
  own held rows without gaining authority;
- whether the elevated API or raw query can write `role_grant` or `role_statement_pending`;
- that neither role table enters fe-api's `/query` `ALLOWED_TABLES` allowlist
  (`fe-api/src/query_guard.rs:36`; `role` is not on it either). Record the verdict.

### RP-9b: correctness review-and-fix · M · opus (critic tick 8, AC-41; `/code-review high`, before RP-10)

**Owned files:** any file touched by RP-0..RP-9, with fixes only (reviewer fixes its own
findings), plus `conductor/tracks/role_propagation_20261013/review-rp9b.md` (one-line verdict +
findings list; track-local record, not a report file for the parent).

**Why.** RP-9 is a security pass; it is not scoped or incentivised to catch a purely functional
regression the way W4's dedicated correctness review caught "every unlinked `iot_reading` insert
would fail" — a bug neither of W4's two security passes found. RP rewrites the A3 gate for every
replicated table and the settlement/responder paths; a functional break of that shape is exactly
the kind this pass exists to catch before the campaign-end sweep is the first thing to notice it.

**Steps:** run `/code-review high` (opus) over the whole track's diff (RP-0..RP-9). Record a
one-line verdict in `review-rp9b.md` and mirror it into RUNBOOK §2's review ledger.

### RP-10: docs consolidation · S · haiku

**Owned files:**
- the root `AGENTS.md` (§The 4-Level Hierarchy gains a one-line "roles replicate as per-grantor
  statements" pointer)
- `fe-database/src/AGENTS.md`
- `fe-sync/src/AGENTS.md`
- `fe-policy/AGENTS.md`
- `fe-runtime/src/AGENTS.md` (`ReplicatedRowOutcome::Deferred`, the new meta fields)
- `conductor/tracks/role_propagation_20261013/*`

Consistency pass only: cross-links, no new rationale. The ledger, RUNBOOK and tracks.md
updates are handed to their owning agent (§4).

---

## 4. Docs to update

| Doc | Change |
|---|---|
| `fe-policy/AGENTS.md` | New §role-grants: the statement model, the fixpoint, subtree authority (`role_at`, owner pins cut the walk, Managers only raise), the admission argument (Deferred = held, order-independent), why there is no cross-author last-writer-wins |
| `fe-database/src/AGENTS.md` | §rbac-policy (`role` is a materialized cache, the raise-only projection); a new §role-propagation (statements, the `role_statement_pending` store, the partition invariant and budget, migration, local/inbound parity); §handlers `replicated_row` (Deferred, confinement, immutable manifest identity, bootstrap narrowed to the manifest only, invite owner pin) |
| `fe-sync/src/AGENTS.md` | §sharding (holders, the gated ledger); §distributed-query (creator copies → holders; settlement closed; filter = hosted ∪ held; hosted-only stays forbidden); §transport-admission-control item 5 CLOSED; new §role-view (one authority function, retain-admitted-only, re-read on empowerment); §inbound-apply (Deferred replaces "permanently skipped") |
| `fe-runtime/src/AGENTS.md` | The new outcome variant and meta fields |
| `fe-test-harness/src/AGENTS.md` | "no role provisioning" becomes "roles flow from the owner via `AssignRole`" |
| Owner-agent hand-off (NOT edited here) | Ledger rows SYNC-ROLE-PROPAGATION, SYNC-RESPONDER and P2P-REF-CONFINE close. SYNC-SIGNED-PLANE splits: the `__shards` gate is DONE, relayed attribution and signed ops stay. RUNBOOK §4 records the refined role-replication assumption and this plan's two decisions (§7 D-RP-1, D-RP-2). tracks.md gains this track |

## 5. Risks

1. **Fixpoint cost.** Rematerializing a verse costs O(passes × statements). It runs over
   ADMITTED statements only, so a flood of held statements never enters it. Admitted counts
   are small (members × scopes). Bound the passes at |statements|+1, with a warning if the
   bound is hit (a bug guard).
2. **Line drift after PAS/CDL phases.** The `ApplyReplicatedRow` and `ExecuteTsPartial` arms,
   `TsPartialRows` and `VerseFabric` may move. Re-grep each symbol.
3. **iroh-docs `Query::key_prefix` behaviour** for the all-authors snapshot needs verifying
   against 0.101 (`store.rs:314`). Fallback: one `Query::author(a).key_prefix("role/")` per
   known author.
4. **Mock replicator semantics** change for the `role/` prefix only. Other tables must keep
   one entry per key, or many existing tests shift.
5. **Bootstrap narrowing (RP-4.3)** changes A2 two-peer timing: interior rows now wait for the
   manifest. The two_peer_* harness scenarios must still converge. Watch the
   `PENDING_WRITES_CAP` interplay.
6. **Behaviour changes users could notice:**
   - Editors lose implicit petal ownership.
   - Managers can no longer strip other grantors' grants, or lower anyone (raise-only).
   - A Manager loses grant authority wherever the owner pins them below Manager.
   - Legacy role rows on non-owner stores are dropped.
   - Invite strings change format.
   All are recorded as assumptions or decisions (§7).
7. **Divergence window.** Rows admitted before a later revocation stay (no retroactive purge).
   A late-joining peer denies them, so stores can differ for revoked authors' old rows.
   This is accepted and documented (§7 A5).
8. **Holder fan-in.** In a verse with many authors, every author answers for its own shards.
   That is bounded by authorship, not fleet size, and still beats today's "everyone answers
   everything".
9. **Pending-store growth.** `role_statement_pending` has no shared cap, by design (a shared
   cap is the eviction vector). Each grantor gets at most 1024 keys per verse, and every row
   mirrors an entry the replica already holds. A hostile author therefore costs the DB at most
   what it already costs the replica, plus one indexed upsert per statement. Watch
   `role_pending_refused_count()`.
10. **Re-read cost.** A role rise re-reads that author's `role/`, `__shards/`, `__holders/` and
    `verse/` entries. That is small per author, and rises need a valid grant. If a
    bulk-granting UI arrives (D-32 FR-4), batch the queue drain per verse.

## 6. Deferral audit (only a missing user decision, a NO_GO class, or a hard external dependency justifies one)

| Item left out | Justification class | Detail |
|---|---|---|
| Relayed-gossip attribution, signed gimbal and editor ops (rest of SYNC-SIGNED-PLANE) | **NO_GO** | Needs per-op signing, which exists only in the canonical log; adopting it on the sync plane is the canonical network rollout, which RUNBOOK §3 ("Carried from earlier") keeps NO_GO. The `__shards` gate IS included (RP-5): doc entries are already author-signed |
| Docs-ALPN peer-level admission and namespace rotation | **Missing user decision** | `fe-sync/src/AGENTS.md` §namespace-id-capability: "needs a decision, no network change in this pass" |
| Scoped-invite redemption, plus the FR-4 role list, invites, presets and audit UI | **User decision (D-32, R5)** | FR-4 is scheduled "after role propagation" in the second campaign. Today `join_verse_by_invite_handler` ignores the scoped role (`handlers/invite.rs:~117-124`), and a proper redeem flow needs the Manager to publish a statement for the invitee DID, which is FR-4 design. This track supplies the substrate |
| Pinning the bootstrap owner for relays that open replicas without an invite | **Missing user decision (D-23)** | The relay host and provisioning are undecided (R6). The pin mechanism ships in RP-4; only the relay-side source of the expected owner waits. TOFU is a documented residual |
| Role-at-authoring-time admission (role history) | Not deferred: **rejected by design** | Author-claimed HLCs make backdating trivial, so current-converged-role evaluation is the secure choice |
| Retroactive purge of rows from later-revoked authors | Not deferred: **rejected by design** | Destructive; the §7 A5 assumption covers it |
| A durable retry store for deferred CONTENT rows (non-role) | Not deferred: **outside R7's requirement; existing behaviour kept (A4)** | R7's order-independence requirement concerns the role set, and §2.4 makes that durable and flood-proof. Content rows already converge on the next replay. The §2.5 re-read also recovers `__shards`/`__holders`/`verse/` rows. A content-row flood can only delay content, never change roles. Trigger to revisit: `deferred_admission_drop_count() > 0` observed in the field |

Nothing else is left out. Reconciliation retries, confinement, the `__shards` gate, the
governance-field gates and the invite owner bug are all in scope.

## 7. Decisions and assumptions

**Decided by the user** (R7, 2026-10-13, RUNBOOK §3). These replace the plan's earlier
defaults.

1. **Role-statement semantics.** The per-grantor model is the implemented basis (§0, §2.2).
   It has owner override, a maximum over valid Manager grants, and cascade on grantor loss.
   R7 settled its two open questions:
   - **Admission:** a statement whose author is not yet Manager+ at its scope is held back
     as **Deferred**. It is never stored inert. It is re-evaluated whenever any author's
     authority changes, so the final role set is independent of delivery order (§2.2
     "Admission", §2.4).
   - **Grant authority:** any Manager may grant within their own subtree: a petal Manager in
     that petal, a fractal Manager in that fractal and its petals, a verse Manager or the
     owner in the whole verse. A Manager can never grant above their scope, beside it, or grant
     Owner (§2.2).

   The model itself replaces the §4 "LWW per (verse, peer)" assumption, which does not
   converge (§0). It keeps the 2026-10-12 decision's properties: ordinary replicated rows, no
   new signature format, and the owner from `verse.created_by`. Reversal: moderate before RP-2
   lands (RP-1 is pure), high afterwards.
2. **Petal creators no longer become petal Owners** (`crud.rs:182-190`). The RP-2 migration
   drops existing implicit rows. They never replicated, so no other peer ever honoured them.

**Decided by this plan** (not asked; the orchestrator mirrors these into RUNBOOK §4 as
assumptions):

- **D-RP-1, the durable retry source for held role statements (AC-1).**
  - DB plane: a persisted `role_statement_pending` table, keyed by attested grantor, with a
    per-grantor budget and no shared cap.
  - Sync plane: a per-author re-read of the replica when an author's role rises.
  - The justification table is in §2.4. Reversal: moderate (RP-2/RP-3 schema) before
    landing; high after.
- **D-RP-2, two refinements of R7's subtree rule (§2.2).**
  - Owner pins cut the authority walk, so the owner can carve a petal out of a Manager's
    authority.
  - Managers only raise, so a narrower Manager cannot demote a wider-scoped peer by
    shadowing.

  Both keep the operator monotone. Without them, R7's subtree authority opens T20 and T21.
  Reversal: cheap (RP-1 is pure).

**Assumptions** (highest reversal cost first):

- A1: Managers only raise (D-RP-2). A Manager's revoke retracts only that Manager's own
  grant. Only the owner lowers a peer or removes a grant someone else issued. To reverse, add
  "Manager may revoke grants from strictly lower grantors" (moderate).
- A2: legacy explicit role rows on stores that are not the owner's are dropped at migration
  (there are no consumers). To reverse, keep them as local-only statements (cheap before
  release).
- A3: `SetVerseDefaultAccess` now replicates (it re-emits the manifest), and local governance
  changes need Manager+ at the verse scope. To reverse, drop the re-emit (cheap).
- A4: a deferred **content** row is not persisted. A restart or overflow relies on the
  existing snapshot replay. Caps are 4096 per verse and 512 per author. Held **role
  statements** are durable (D-RP-1) and are not covered by this assumption. To reverse,
  persist the content queue (moderate).
- A5: admission uses the author's current converged role. Rows already admitted from a
  later-revoked author stay; late joiners do not admit them. To reverse, add an owner "purge
  author" command (moderate).
- A6: `HOLDER_WAIT_MS` = 750 ms (clamped to the deadline). To reverse, it is a constant
  (cheap).
- A7: `role_grant` and `role_statement_pending` stay OUT of fe-api `/query`. `ALLOWED_TABLES`
  (`fe-api/src/query_guard.rs:36`) is an allowlist and does not list `role` either. RP-9
  records the verdict, and FR-4 (D-32) reads roles through a scoped route. To reverse, add an
  allowlist entry with a scope filter (cheap).
- A8: invites carry `owner_did` (a breaking invite-string format; there are no consumers). To
  reverse, accept both formats (cheap).
- A9: `ROLE_PENDING_PER_GRANTOR` = 1024 keys per verse, refuse-new. The cost of a refusal
  falls only on the grantor that exceeds it, and the §2.5 re-read recovers the refused
  statement on empowerment. To reverse, it is a constant (cheap).

## 8. Critic amendments (2026-10-13)

This track runs **last**: PAS-P3P4 → PAS-P5 → CDL-W4 → CDL-SPEC10 → role propagation. It
therefore inherits code those phases land in its files, and owns the edits that reconcile with it.

| # | Change | Class |
|---|---|---|
| C1 | The critic narrowed grant authority to the verse scope, to match the 2026-10-12 wording. **Superseded by R7:** subtree authority, now §2.2. T19 is rewritten. | superseded (R7) |
| C2 | The critic surfaced the 2026-10-12 decision's admission wording as a user question. **Answered by R7:** Deferred. The durable store is D-RP-1 (§2.4). | superseded (R7) |
| C3 | RP-2 owns `DurableAuthorizationView::load` (`canonical_epoch.rs:540`). It must derive the owner from `created_by`, otherwise dropping owner rows silently breaks CDL-W4-7/W4-8's canonical issuer authority. Test + T18 added. | cross-plan regression (later phase breaks earlier) |
| C4 | RP-2 keeps CDL-W4-1's six bypass lines in `crud.rs` byte-identical. | cross-plan collision |
| C5 | RP-3 owns the earlier phases' test files whose `Denied` expectations it flips (PAS-P3P4 S6, CDL-SPEC10 A3), and preserves the W4-8 enrolment branch and the SPEC10 A3 arms. Test + T17 added. | cross-plan collision (`replicated_row.rs`) |
| C6 | RP-4 confinement reuses CDL-SPEC10 A3's resolver for `asset` / `device_payload` / `device_format`, and turns its unknown-parent `Denied` into `Deferred Parent`. Enrolment is exempt and stays verify-first. Tests + T16 added. | cross-plan collision + missing tests |
| C7 | RP-7 lands provisionally as a revertible commit and is reverted alone if RP-6 or the RP-8 scenario is red. This honours §3 *"fix the settlement race, then consider re-enabling responder filtering"*. | RUNBOOK §3 sequencing |

**Corrected cross-cut.** The planner's note that PAS-P3P4's residency ledger and chunk-serving
admission are "sync-plane rows of the same kind as `__shards`" is wrong. The PAS-P3P4
`ResidencyLedger` is a local fe-ui resource, and tileset advertisements are gossip frames gated
by F23 sender identity, not doc rows. Nothing in PAS writes `__*` or `role/` doc keys, so there
is no doc-key collision to manage. If PAS-P3P4 S8b adds a verse-membership blob-request filter,
RP-5's role view is the natural source for it later. That is an option, not a dependency.

**Deferral audit:** every row stands. Each is NO_GO (relayed attribution / signed ops = canonical
network), a missing user decision (namespace rotation; the D-23 relay-host pin), or user-ratified
sequencing (D-32 FR-4 "after role propagation").

### Tick 4 (2026-10-14) — source: critic tick 4

| # | Change | Class |
|---|---|---|
| C8 | `set_petal_terrain_handler` never calls `authorize_edit` though W1 S6 made petal terrain replicate — a Viewer's local edit silently diverges from peers. **RP-4** adds the gate (§3 RP-4 step 7, owned-file bullet, and flow test above). | missing local gate (AC-21(d)) |
| C9 | The blob-ALPN request filter (`fe-sync/src/AGENTS.md` §tileset-security "Risk 2") was deferred at W1 S8b with an explicit revisit trigger ("when RP replicates roles …"). This section's own "Corrected cross-cut" paragraph above called it only "an option, not a dependency," so the trigger could never fire. **RP-5** is now scheduled as the moment it fires (§3 RP-5 step 6, owned-file bullets, and flow test above), tied to the role view RP-5 already builds. | scheduled deliverable, previously unreachable (AC-21(e)) |

Both logged against `.omc/accountability/tick-20261014-0149.md` AC-21(d) and AC-21(e).

### Tick 5 (2026-10-14) — source: critic tick 5

| # | Change | Class |
|---|---|---|
| C10 | New slice, scheduled after RP-4: **STREAMED-TERRAIN-PIN** (AC-24 residual (3) / AC-23(a), `.omc/accountability/tick-20261014-0722.md`). Owned files: ~~`fe-terrain/src/tiles/streamed.rs` (`from_header_doc` pin threading only — the worker-thread move itself is W2b F1, which this slice builds on)~~ `fe-terrain/src/tiles/streamed.rs` — **no change expected**, W2b (`474ab9b`) already threads `pin` through `from_meta_json`/`from_header_doc`; `fractalengine/src/terrain_bridge.rs` (`drive_streamed_tilesets` assembles the pin chain at `~:1312-1321` — the petal-authored source is inserted here, between installed and TOFU); `fe-terrain/src/petal_binding.rs` (`terrain_config_from_petal_json`); `fe-terrain/src/config.rs` (`TerrainConfig`); `fe-ui/src/plugin.rs::tileset_to_terrain_json`; fe-api terrain validation (DEC-C22 strictest reader). W2b F1 moves streamed-meta verification off the main thread and pins to a locally installed publisher; this slice supplies the remaining pin source — a publisher DID carried in the Editor-authored petal terrain row — which needs RP-4's Editor+ gate on `set_petal_terrain_handler` (§3 RP-4 step 7) to be trustworthy before a streamed (never-installed) tileset can pin to it. **Decision item (critic tick 6, AC-31):** the streamed path records no TOFU pin of its own — the W2b reviewer's unfixed LOW notes that with nothing installed or known locally, the first header wins and a later different-publisher header in the same session is also accepted. Decide: record a streamed-verdict pin in `KnownTilesetPublishers` on first contact (symmetric with the installed path's TOFU), or rely solely on the petal-authored pin and document why no streamed-side TOFU pin is needed. **Default: record the first-contact pin**; if chosen, add the test `a_second_streamed_publisher_for_the_same_id_is_refused_in_session`. Reversal: drop the recorded pin and rely only on the petal-authored pin (cheap — delete the write, keep the read). Named test: `a_streamed_tileset_signed_by_a_publisher_other_than_the_petal_maps_pin_is_refused`. | scheduled deliverable (depends on RP-4) |

Logged against `.omc/accountability/tick-20261014-0722.md` AC-24 residual (3) and AC-23(a).

### Tick 8 (2026-10-15) — source: critic tick 8

| # | Change | Class |
|---|---|---|
| C11 | The plan had ONE review (RP-9) after all build slices, and no correctness pass — W4 needed three. **RP-4s** (DB-plane security review-and-fix, fresh context, over RP-0..RP-4b) added before RP-5; **RP-9** stays the sync-plane + cross-plane security review with its threat matrix; **RP-9b** (correctness review-and-fix, `/code-review high`) added before RP-10. Each records a verdict row (RUNBOOK §2). | missing review coverage (AC-41) |
| C12 | A local role grant/revocation published only through `replicate_row` can be lost if the verse's replica is closed, or evicted by a pending-writes flood — never reaching any peer. **RP-2** gets a durable re-emit step (DB startup + replica open, re-emits every `role_grant` row whose `grantor_did == local_did`) and two tests. T23 added. | missing durability guard (AC-42) |
| C13 | `r5_referential_confinement`'s helpers conflate "known foreign/mismatched" and "unknown locally" behind one boolean/`Option<String>`. **RP-4** splits every helper into three outcomes (Known-ok / Known-foreign-or-mismatched → Denied / Unknown → Deferred `Parent{table,id}`), adds wait reasons for `device_format`/`asset`/`device_payload`, states the gate order, and converts AC-34's pre-manifest Deny to Deferred `Manifest`. Four new tests. | conflated validity states (AC-43) |
| C14 | The enrolment floor (verse-scope `resolve_role`) can never fire for a petal-scoped member under R7 subtree semantics, and a pre-manifest enrolment skipped the floor entirely. **RP-3**: a floor failure is Deferred `AuthorRole` (not Denied); a pre-manifest enrolment is Deferred `Manifest`; membership = `EffectiveRoles::has_any_role` (RP-1, AC-44c). Supersedes the W4 plan §9 "never `Deferred AuthorRole`" wording while keeping its intent. Test renamed. T27 added. | enrolment floor order-dependence (AC-44) |
| C15 | **C3's premise was false.** The interim authority keys `VERSE#<hex>[-PETAL#<hex>]` rows directly, deliberately not `build_scope`'s string, and was never watching `role`/`role_grant` — deriving the owner inside `DurableAuthorizationView::load` is a SPEC-2 concern, out of RP's scope. **RP-2** step 9 (ex-step 8), its test and T18 replaced: migration/`rematerialize` touch only rows whose scope `parse_scope`s to the verse's ULID, never an interim `VERSE#<hex>` row. RP-2 owns the seed inventory; `seed_join_verse` citation corrected; `admin.rs` membership decided. **C3 superseded by C15 (false premise).** | cross-plan regression, corrected (AC-45) |
| C16 | An API token's `max_role` is capped only at mint and lives up to 30 days; RP introduces remote demotion, so a demoted node's API keeps writing rows every peer defers. **RP-4** adds a new step: effective API role = `min(token max_role, role_at(local_did, scope))`, applied by one helper on every replicated-write door. Test + T26 added. | stale-token divergence (AC-46) |
| C17 | C10 (STREAMED-TERRAIN-PIN) existed only as a §8 table row, with no slice id or position in §3. Promoted to **RP-4b** (opus, after RP-4, before RP-4s): precedence installed > petal-authored > TOFU (a petal-authored pin replaces a recorded TOFU pin, never the reverse); `is_valid_inbound_petal_terrain` gets `did:key` grammar validation; two new tests plus the two already named. T24/T25 added. | scheduled deliverable, now slotted (AC-47) |
| C18 | The plan's own gate text ("cargo check + clippy on the crates it touched") contradicted AC-17/AC-19/AC-26, and the W4 script's `overrides` bypass pattern was never disclaimed. Plan header and spec.md's gate text replaced with the workspace triple + feature matrix + `gate_green` halt (no commit on red); the workflow prompt must never return a structured result while a cargo process it started is alive, and must retry once on exit `0xc0000142` with no other builder running; the gate script must not inherit a generic `overrides` bypass unless a user authorization is logged first. **RP-8** owns `README.md` + `conductor/final-sweep-manifest.md` (harness 17→18). | stale gate text + missing doc updates (AC-48) |
| C19 | The B3 oversized-window split path (`Prepared::Oversized`) was implemented but never tested, and a non-object descriptor `schema` fell through to an untyped rejection. **RP-0** (new first slice, sonnet, same gate): the split-path flow test plus a typed `InvalidSchema` rejection and door-table row. | untested branch + typed-error gap (AC-49) |
| C20 | secA's "blob reach by known hash" residual was routed to "the planned blob request filter" (C9), but C9's design only ever admitted tileset-root hashes — row and asset blobs stay open over `/assets/by-id`, which no ALPN filter sees. Honest disposition recorded: ledger row **BLOB-HASH-CAPABILITY**, trigger = a membership-keyed filter for all hashes or namespace rotation. Not counted as fixed by C9. T28 added as a recorded-not-fixed row. | mis-routed residual (AC-50) |
| C21 | Bookkeeping: `metadata.json` partitions were stale at `877e801` (missing C3/C5/C8/C9/C10 files, `progress.open: 10`); spec scope/acceptance lacked C8/C9/C10 and the Deferred Parent/Manifest rules; the review ledger lacked W3b/tick 7/tick 8 rows; RUNBOOK §2 `:116` and the ledger's role_propagation row (`:83`) were stale; `accountability-log.md:268-269` mis-diagnosed the B2b DLL failure as concurrent cargo invocations (it was sequential; the real cause was resource pressure); the `seal_observations` cutover precondition was unrecorded; RP-1's tie-break uses canonical-payload-byte order (no hash dependency), and property tests stay seeded/hand-rolled unless a slice adds `fe-policy/Cargo.toml` + `Cargo.lock` for `proptest`. Docs pass (this commit). New ledger row **OBS-SEAL-FIXTURE-SEAMS**. | bookkeeping + dependency note (AC-51) |

Logged against `.omc/accountability/tick-20261015-0908.md`.

---

## W5 fast-forward completion (2026-10-25)

**Status: CODE COMPLETE** (tests deferred to refine phase per 2026-10-24 FF directive)

Per-slice commit SHAs (base 4d3a1a6, strictly sequential):
- **RP-0:** `15ae606` — oversized-window split test + typed schema rejection
- **RP-1:** `a373ae5` — fe-policy pure role-grant model (least fixpoint, Kleene iteration)
- **RP-2:** `6bef9e5` — local role statements, materialized cache, durable re-emit, migration
- **RP-3:** `2a44583` — inbound role statements and deferred admission
- **RP-4:** `4d3a1a6` — owner integrity, P2P-REF-CONFINE, three-valued confinement, scope-precise admission, API role cap
- **RP-4b:** `1fbb4f4` — streamed-terrain publisher pin (installed > petal-authored > TOFU), first-contact TOFU, did:key pin grammar
- **W5b (OF-3):** `80ee19e` — unify scope-chain parser (pure into fe-policy, re-exported from fe-database)
- **RP-5:** `a240a8f` — sync-plane role view and gates
- **RP-6:** `e8335c3` — settlement race closed: __holders, expected answerers + holder wait, deferred_shards honesty, sub-Editor answerer rejection
- **RP-7:** `f6d0fc1` — responder filtering re-enabled, pinning test flipped
- **RP-8:** `a3e249b` — harness end-to-end role propagation

**Merged per FF directive (2026-10-24):**
- **RP-4s, RP-9, RP-9b** merged into single fresh-context security+correctness review-and-fix over W5+W6 diff, scheduled for refine phase after code complete
- Both reviews run AFTER all W5+W6 code is complete; test execution and all review passes deferred to final sweep

**Documentation (RP-10):** AGENTS.md consolidated across root, fe-policy, fe-database, fe-sync, fe-runtime; role propagation sections document:
- Fe-policy: §role-grants (statement model, fixpoint, subtree authority, admission, no LWW)
- Fe-database: §rbac-policy (materialized cache), §role-propagation (statements, pending store, partition, migration)
- Fe-sync: §role-view (authority function, re-read on empowerment), §transport-admission-control item 5 (CLOSED), holders
- Fe-runtime: §deferred-admission (DeferralReason: Manifest, Parent, AuthorRole; durable retry for role statements)
- Root AGENTS.md: 4-Level Hierarchy pointer to role replication per-grantor
