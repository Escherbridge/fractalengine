---
type: Track Spec
title: Per-key (HLC, author) clock and write-time role admission
track: per_key_clock_20261113
tags: [security, p2p, rbac, fe-policy, fe-database, fe-sync]
created: 2026-11-13
status: planned
resource: ./metadata.json
plan: ./plan.md
findings: [QA-0156, QA-0165, QA-0166, QA-0126]
decisions: ../../RUNBOOK.md#3-decisions
---

# Per-key clock: every replicated field is a (HLC, author) register judged by the author's role at write time

## 1. Problem

Four open or half-fixed findings share one root: replicated rows are merged either whole-row in arrival
order or as a single register, and the authority check asks "does the author hold the role NOW".

| Finding | Root cause (quoted from `qa/findings.jsonl`) |
|---|---|
| QA-0156 (S2) | "Replicated node rows merge non-transform fields (properties, display_name) in arrival order: only the transform is an edit_seq register, so two authors' concurrent property writes can converge differently on different peers." The partial fix `e7a65f50` only gave markerless (legacy) rows a deterministic writer rank (`node_legacy_writer`) and made tombstone markers settle on the larger value; complete snapshots remain a whole-row `(HLC, publisher)` register, so two authors editing different keys still overwrite each other. |
| QA-0165 (S3) | "Revoking a member reverts that member's earlier, authorized edits on every peer: the demotion re-judge requires the author's CURRENT role." Mechanism: `rejudge` at `fe-database/src/handlers/scene_publication/inbound.rs:32` keeps only versions whose author holds Editor now, and `RoleStatement::as_of` (`fe-policy/src/role_grants.rs:121`) returns `None` for any `at` before a revoke's hlc, so the history that would prove "was Editor when written" is erased by the revoke itself. Sim p2p-core 13a/13b moves N2 off the revoked editor's transform and drops N4's step-12 properties on every peer. |
| QA-0166 (S2) | "The QA-0164 unchanged-rewrite skip compared only this author's own entry, so restoring bytes another author had since overwritten (verse/fractal/petal rows merge in arrival order) published nothing." Static rows merge in arrival order (`apply_static_row` MERGEs on arrival); `fe-sync/src/replicator.rs:779` `own_entry_holds` plus commit `589b39b0` is a workaround, not an ordering rule. |
| QA-0126 (S3) | "Contested transforms converge, but not by the documented rule: merge.rs's edit_seq + largest-value tie-break predicts [5,0,0] ... every peer settled on [-5,0,0]." The documented transform rule is dead code on the live path: complete snapshots order by HLC then publisher, never by `edit_seq`. |

## 2. Decision

User, 2026-11-13, `conductor/RUNBOOK.md` section 3: **"QA-0165 = role at write time, build it in round 4."**
A write is admissible when its author held the required role at the write's hlc. The role the author
holds now is irrelevant. Steer 2026-11-13 20:55 fixes two sub-decisions: (1) everyone upgrades together
(flag day, no dual format, no negotiation); (2) a local revoke closes the grant window at the highest write
clock the revoker has received from that author.

## 3. Design (normative)

### 3.1 Wire shape: `_fe_clock`

Every replicated verse, fractal, petal and node row carries one extra top-level column:

```
_fe_clock = { "v": 1, "author": "<entry author DID>",
              "own": { "<key>": { "hlc": "<decimal string>", "value": <json> }
                     | { "hlc": "<decimal string>", "deleted": true } } }
```

- `own` holds the entry author's OWN latest version of each key it ever wrote on that row. It never
  carries another author's value: the iroh entry author is the only authenticated identity.
- `hlc` is a positive canonical decimal string (the `SceneClock::from_fields` rule,
  `fe-database/src/handlers/scene_publication/wire.rs:37`); `value` and `deleted:true` are mutually exclusive.
- Parse (`field_clock::wire::parse_own`) rejects: `v != 1`, `author != entry author`, non-canonical hlc,
  more than 512 keys, a key naming an identity column, a `properties/<k>` key on a non-node table, a
  version with both value and `deleted`.

### 3.2 Keys, identity columns and levels

| Table | Identity columns (insert-once, never clocked) | Clock keys |
|---|---|---|
| verse | verse_id, created_by, created_at | every other top-level column (name, description, default_access, ts_mode, ts_replication_factor, ts_bucket_width_ms, namespace_id, ...) |
| fractal | fractal_id, verse_id, owner_did, created_at | every other column |
| petal | petal_id, fractal_id (and any parent id), created_at | every other column (bounds, terrain, render_distance, hexon_manifest, ...) |
| node | node_id, petal_id, created_at | `transform` (position + elevation + rotation + scale + edit_seq as ONE object value), `properties/<k>` per top-level property key, `tombstone`, every other column |

Required level per key (the level the author must have held at the version's hlc):

| Scope | Keys | Level |
|---|---|---|
| verse | default_access, ts_mode, ts_replication_factor, ts_bucket_width_ms, namespace_id (`VERSE_GOVERNANCE_FIELDS`, `replicated_row.rs:579`) | Manager at verse scope |
| petal | hexon_manifest (today's ManageScope rule, `confine_petal`) | Manager at petal scope |
| any other key | all remaining keys | Editor at the row's deepest scope |

### 3.3 Storage (LOCAL tables, never replicated)

| Table | Columns | Rule |
|---|---|---|
| `row_field_version` | tbl, rid, verse_id, field, author, hlc, value, deleted | unique on (tbl, rid, field, author): exactly one latest version per author per key; on equal hlc keep the larger canonical value bytes |
| `row_field_clock` | tbl, rid, field, hlc, author | materialized winner cache |

Both are written in the same transaction as the row, with a verify statement (SurrealDB 3
`UPSERT ... WHERE` can silently skip; mirror `CLOCK_WRITE`/`CLOCK_VERIFY`,
`scene_publication/mod.rs:233-237`).

### 3.4 Merge order

Winner per key = maximum over ADMISSIBLE versions of `(hlc, author DID bytes, canonical value bytes)`,
compared in that order. Canonical value bytes = `serde_json::to_vec` of the value after recursively
sorting object keys. The fold is a pure function of the set of stored versions and the admitted role
statements, so every peer converges whatever the arrival order (unit test
`field_clock::merge::tests::fold_is_order_independent`).

### 3.5 Admissibility

A version is admissible when ANY of:

1. its author is the verse owner;
2. it is the legacy floor (author `''`, hlc 0, seeded by migration);
3. `fe_policy::held_at(owner, admitted_statements, author, scope, level(key), hlc)` holds.

There is NO conjunction with the author's current role (that is the QA-0165 change). The wire carries
only the per-key hlc; the proof is the verse's admitted role statements. The wire carries no grant
reference and never claims authority.

Inbound admission for a clocked row is only a membership floor (the author is the verse owner or has any
admitted or held statement in this verse); otherwise `Defer(AuthorRole)` exactly as today. Past the floor
every version is STORED; an inadmissible version is simply never a winner.

### 3.6 fe-policy grant windows

`RoleStatement` gains `windows: Vec<GrantWindow>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`),
`GrantWindow { role, since, until }` closed-open `[since, until)`, role Editor or Manager.

| Rule | Value |
|---|---|
| cap | `MAX_GRANT_WINDOWS = 8`; the oldest window is dropped past the cap |
| carry | `continuing(prior)` carries `prior.windows` forward on every later version of the statement key |
| append | a version that BREAKS a level the prior granted (revoke, or Editor+/Manager no longer granted) appends `[prior.granted_since(level), self.hlc)` for the highest broken level (Manager to Editor appends a Manager window and keeps `editor_since`) |
| frontier | `continuing_with_frontier(prior, frontier)` ends each appended window at `min(self.hlc, frontier.max(since))`; an empty window (since == until) is skipped |
| read | `as_of(at)` with `at < self.hlc`: a window with `since <= at < until` yields the statement at that window's role, not revoked; otherwise today's behaviour |
| validate | windows sorted by since, non-overlapping, `since < until <= hlc`, role Editor or Manager, len <= 8, else `RejectReason::InvalidContinuity` |
| ordering | the canonical payload used by `latest_per_grantor` tie-breaks includes `windows` |

`held_at` = `author == owner || materialize_at(owner, statements, at).role_at(author, scope).is_at_least(level)`.

### 3.7 Revoke frontier

A LOCAL revoke or lower closes the revoked author's window at the **frontier**: the highest hlc of that
author's versions the revoker has stored in the verse (`field_clock::store::author_frontier`, also the
max `node_scene_clock.hlc` where publisher = the author), capped at the revoke's own hlc. Versions the
revoker had not received, and any version stamped above the frontier, are refused everywhere because
the window the statement carries ends at the frontier. A revoker holding no version of that author closes
the window empty (frontier 0): nothing it saw needs the window (R-CLOCK-1).

**Inbound windows are untrusted (security inbox 4).** `fe_database::role_grants::place_statement` keeps a
wire window only when the window this peer re-derives from its stored live version of the same key
(`proven_windows`: `continuing_with_frontier(prior, None)` over the local prior) contains it at an equal
or higher role; a payload can narrow a window, never widen or invent one. A redelivered version keeps the
windows proven when it first arrived. The grantor's own authority at the window's time is then enforced by
`materialize_at`, which admits the window's statement only when the grantor held Manager at `at`.

### 3.8 Deferred interaction and re-judge

A non-owner author with no admitted or held statement is `Defer(AuthorRole)` as today. Otherwise versions
are stored. A DB-local re-judge (`field_clock::rejudge::rejudge_authors`) recomputes the winners of every
row that has a version by the affected DIDs (cap 4096 rows per call, remainder warned and continued in a
later call). It is a pure function of stored versions and admitted statements. Triggers:

| Trigger | Source |
|---|---|
| risen DIDs (held statement promoted) | `role_grants::repartition` after a placement in `apply_role_statement` |
| fallen DIDs (revoke or history change landed) | same |
| verse manifest owner change | `apply_replicated_row_reporting_with_publisher` owner repartition |
| local statement commit | the DB-thread arms calling `role_grants::commit_local_statement` |

The legacy fe-sync `RejudgeReplicatedNodes` arm stays for nodes with no stored versions.

### 3.9 Tombstones

An admissible node `tombstone` version deletes the node: delete-wins, the existing non-resurrection rule.
A local tombstone still dominates, and an older tombstone cannot replace a known later one. A tombstone
whose author is no longer admissible (a revoke window ends below its hlc) stops deleting on re-judge.

### 3.10 Legacy and migration

- v1 `_fe_scene` snapshots without `_fe_clock` fan out to one version per key at `(snapshot hlc, publisher)`;
  keys the same author owned previously and the snapshot now omits become `deleted` at that hlc.
- Unclocked static rows fill only keys whose winner is the legacy floor or absent, recorded as
  legacy-floor versions.
- Markerless node rows keep `legacy_node_row_provable`.
- `field_clock::migrate::seed_legacy_floor` is idempotent (batches of 1024): every verse/fractal/petal/node
  row without `row_field_clock` entries gets floor versions (author `''`, hlc 0) per key; node rows with a
  `node_scene_clock` row seed `(hlc, publisher)` instead.

### 3.11 Size bounds

| Bound | Value |
|---|---|
| serialized row including `_fe_clock` | <= 96 KiB |
| keys in one `own` map | <= 512 |
| grant windows per statement | <= 8 |
| HLC future bound | 5 minutes (`op_log::admit_scene_hlc`, `fe-database/src/op_log.rs:108`) |

## 4. Security analysis

**What the wire can claim.** Only the iroh entry author's own versions count: `parse_own` rejects a
`_fe_clock.author` that differs from the authenticated entry author, and `own` never carries other
authors' values. The wire carries no grant reference, no role claim and no proof; authority is derived
exclusively from the verse's admitted role statements held locally, which already converge to a fixpoint
independent of message order (role_propagation_20261013). Every value still passes the strictest-reader
per-field validators; a bad value denies the whole row.

**What the wire cannot claim.** An author cannot raise its level (the level is a function of the key),
cannot attribute a write to another author, cannot make an inadmissible version win (admissibility is
judged locally per version), and cannot write identity columns (insert-once, mismatch = Denied).

**Residual risks (accepted, documented, no further mitigation this round).**

1. *Backdating by a revoked author.* A revoked author can still stamp a write at an hlc at or below the
   revoker's frontier for a key nobody has written since: the window `[since, frontier)` genuinely
   covers that hlc, and the (hlc, author) order cannot distinguish a forged old clock from a delayed
   genuine one. Bounded by: the 5 minute HLC future bound does not apply to the past, so the bound is the
   window itself; a key anyone admissible wrote later keeps the later winner.
2. *Frontier gap.* Edits the revoker had not yet received are dropped like any write racing the revoke
   (steer decision 2).
3. *Grantor backdating.* A grantor can only backdate windows inside its own authority at that time:
   `materialize_at` judges the grantor as of `at`, so a grantor cannot mint a window older than its own
   grant.
4. *Window cap.* More than 8 grant breaks on one statement key drops the oldest window; writes from
   before the dropped window stop being admissible. Accepted: a member cycling through nine grants is an
   administrative event, and the loss is deterministic on every peer.
5. *Proof is local, so fresh peers prove less.* A peer that never stored the key's earlier version (it
   joined after the grant, or missed it) keeps none of the claimed windows, so it refuses versions an
   older peer admits from inside them. Fail-closed by the steer of 2026-11-13 22:55; the two peers
   disagree only on rows written inside closed windows. Revisit trigger: a QA report of an edit by a
   since-revoked member missing on a newly joined peer.
6. *Continuity fields are still wire-trusted.* `editor_since` / `manager_since` (QA-0131) are accepted
   from the payload after the shape check, and a later break turns them into a proven window. Same bound
   as risk 3: the grantor must have held Manager at every `at` the span covers. Re-deriving them locally
   would refuse every pre-join edit of a live member on a fresh peer, so it is left to a user decision.
7. *HLC skew.* An inflated hlc within the 5 minute future bound can win ties against slower honest
   writers for keys the author may write; it cannot widen what the author may write.

## 5. Compatibility

Wire bump; peers upgrade together (flag day, precedent: the iroh 1.x port in the root `AGENTS.md`). No
dual-format writing, no version negotiation, no compatibility window. Pre-upgrade peers deny node rows
with an unknown column and fail to MERGE `_fe_clock` into SCHEMAFULL hierarchy tables. fe-sync needs no
protocol change (entries stay `table/record_id`, one per author); its parsers read `serde_json::Value`
and tolerate the new column (verified by CLOCK-11). Existing local data is covered by the legacy floor
migration (3.10).

## 6. Acceptance

1. Sim `p2p-core` steps 13a/13b: the revoke no longer moves N2's transform and no longer drops N4's
   step-12 properties (`after_restart_rest`, `after_restart_mcp`) on any peer.
2. Two editors writing different property keys on one node concurrently keep both keys on every peer, in
   every arrival order (QA-0156).
3. Two editors' concurrent petal-name writes converge to the maximum `(hlc, author)` on every peer in
   every arrival order (QA-0166 ordering).
4. Alice restoring her earlier petal name after Bob's overwrite reaches every peer (QA-0166 restore, over
   real transport).
5. The documented transform winner (QA-0126) is what the code does: the winner of a contested transform
   is the maximum `(hlc, author, canonical value)` among admissible versions.
6. `field_clock::merge::tests::fold_is_order_independent` holds across permutations including an
   inadmissible newer version.
