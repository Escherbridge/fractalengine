---
type: Track Spec
title: Role propagation, distributed-query settlement, responder filtering
track: role_propagation_20261013
tags: [security, p2p, rbac, fe-policy, fe-database, fe-sync, role_propagation_20261013]
created: 2026-10-13
status: planned
resource: ./metadata.json
plan: ./plan.md
ledger_rows: [SYNC-ROLE-PROPAGATION, SYNC-RESPONDER, P2P-REF-CONFINE, SYNC-SIGNED-PLANE (the __shards part only)]
decisions: ../../RUNBOOK.md#3-decisions
---

# Role propagation, settlement race, responder filtering

## Why

Roles live only in the local store. `role` rows are `NotApplicable` inbound
(`fe-database/src/handlers/replicated_row.rs:201-207`), and nothing publishes them
outbound. So every peer that is not the verse owner treats every author who is not the
owner as the manifest's `default_access` (`viewer`) and DENIES their rows at the A3 gate
(`replicated_row.rs:308-313`). Sharded or balanced timeseries placement onto those peers
works only because the author's own copy (the "creator copy") answers queries. That is why
responder hosted-shard filtering had to be reverted (SYNC-RESPONDER,
`fe-sync/src/AGENTS.md` §distributed-query). It also leaves the collector's settlement race
open: a non-host answer can be lost when a formal host's empty answer arrives first.

## Decision being implemented (RUNBOOK §3: the 2026-10-12 grill, as amended by R7 on 2026-10-13)

- Replicate roles as ordinary replicated rows. The owner bootstraps from
  `verse.created_by`. This was chosen over signed role grants (simpler; no new signature
  format).
- **Grant authority (R7):** any Manager may grant within their own subtree:
  - a petal Manager in that petal;
  - a fractal Manager in that fractal and its petals;
  - a verse Manager, or the owner, in the whole verse.

  A Manager can never grant above their own scope, beside it, or grant Owner.
- **Admission (R7):** a role statement whose author is not yet Manager+ at its scope is held
  back as **Deferred**. It is re-evaluated whenever any author's authority changes, so the
  final role set does not depend on delivery order.
- **Petal creators (R7):** petal creators no longer become petal Owners, and existing implicit
  owner rows are migrated away.
- After it lands: fix the distributed-query settlement race, then consider re-enabling
  responder filtering.

The §4 assumption carried from 2026-10-12 says: last writer wins by HLC per (verse, peer)
row; revocation is a tombstoned role row; the owner role is never replicated in.
**This track refines that assumption** (see plan §0 and §7). Read
literally, it does not converge under a concurrent revocation, and a skewed HLC can abuse
it. The refined form keeps every stated property:

- role rows are ordinary replicated rows;
- a statement is admitted only when its author is Manager+ within their own subtree (at the
  statement's scope or an ancestor). Until then it is Deferred, held in a durable store that
  a flood cannot evict;
- the owner comes only from `created_by` and is never replicated in;
- revocation is a non-empty tombstone row;
- each grantor's own statements resolve last-writer-wins by HLC.

It also adds one rule: different grantors' statements combine as "owner overrides, then the
maximum of the valid Manager grants". Nothing is decided by last-writer-wins across authors.
Two consequences of combining that rule with R7's subtree authority are made explicit (plan
§2.2, decision D-RP-2):
- an owner pin is exact, and cuts a Manager's authority below it;
- Managers only raise. Only the owner lowers anyone.

## Scope

1. **Role statements replicate.** Each local assign or revoke writes a per-grantor statement
   with the doc key `role/{scope}/{peer_did}/{grantor_did}`. It is published through the
   existing emission seam. The local `role` table becomes a materialized cache, computed by
   one pure, order-independent function in fe-policy and shared by the DB plane and the sync
   plane.
2. **Inbound admission of role statements.**
   - Admission first checks the statement's structure: the grantor is the attested entry
     author, the scope is in the same verse, the role is not Owner, the owner is not the
     target, and the role is known.
   - A statement whose grantor has authority at its scope is admitted, and the fixpoint
     computes its effect.
   - Any other statement is **Deferred** into a durable pending store. That store is keyed by
     grantor, has a per-grantor budget and no shared cap, so another author's flood cannot
     evict it. The statement is promoted as soon as its grantor gains authority (plan §2.4,
     decision D-RP-1).
   - Content rows that fail only because a role or a parent has not converged yet are also
     **deferred**, not denied. They are retried automatically when the missing role or parent
     lands.
3. **Owner integrity.** Petal creators no longer get an implicit petal-Owner row, and the
   migration drops existing ones (R7). The verse manifest's identity fields are immutable once
   they land.
   Today any Editor can MERGE `created_by` and take over the verse:
   `replicated_row.rs:387-393` merges the whole payload. The governance fields need Manager+
   to change. Invitees record the real owner DID: today they record the inviter's hex key,
   at `handlers/invite.rs:167`.
4. **P2P-REF-CONFINE.** Interior rows (fractal, petal, node, iot_reading, and tombstones)
   must reference parents in the replica's own verse, both in their payload and in the
   durable row they overwrite. The role is resolved at the row's most specific scope.
5. **Sync-plane role view.** The `__shards` Editor+ gate is now constructible, so it is
   implemented.
   - Every gate resolves roles with the same subtree function as the DB plane, at the row's
     deepest scope.
   - `note_verse_row` applies settings only from Manager+ at the verse scope.
   - Compute requesters need Viewer+ at the requested petal.
   - The fabric keeps only admitted statements. When an author's role rises, it re-reads that
     author's entries from the replica.
6. **Settlement race closed.** A peer that holds a creator copy declares itself
   (`__holders`). The collector waits for hosts and declared holders, with a bounded wait for
   holders. Hosts flag shards whose rows they have deferred. The merge drops rows from
   answerers below Editor.
7. **Responder filtering re-enabled** as `hosted ∪ held ∪ unknown-ledger`, never hosted-only.
   The pinning test flips (plan RP-7).
8. **A dedicated security review-and-fix slice** works through the threat matrix (plan RP-9).

## Acceptance (from the ledger and the decisions)

- **SYNC-ROLE-PROPAGATION:** a non-owner host admits an Editor author's replicated reading
  with no local provisioning. The role arrives only as the owner's replicated statement
  (harness test). The harness `distributed_query` scenario passes with hosts holding every
  planned shard. The collector settlement residual is closed: a test with the host's answer
  first goes green.
- **SYNC-RESPONDER:** responders filter to `hosted ∪ held`. A non-host that holds nothing
  stays silent. The rewritten pinning test fails if filtering ever drops declared holders
  (hosted-only).
- **P2P-REF-CONFINE:** an interior row that references a foreign verse's petal or fractal is
  denied, and so is a tombstone or re-parent aimed at a foreign verse's row (two-peer
  harness test plus DB flow tests).
- **R7:**
  - grants take effect within the grantor's subtree and nowhere else, and granting Owner is
    refused (RP-1/RP-2/RP-3 tests);
  - a held statement is admitted on promotion;
  - every delivery order of a grant/promotion/revocation set yields the same role set and
    partition;
  - held statements survive a restart and a hostile multi-author flood (RP-3 tests);
  - implicit petal-owner rows are gone after migration (RP-2 test).
- **Threat matrix** (plan §RP-9, T1–T22): every row has a test that fails if its guard is
  removed.
- `cargo check` and clippy stay clean after every slice. One full test, e2e and harness sweep
  runs at campaign end (standing rule: no tests until the code is complete).

## Out of scope

The full justification is in plan §6, the deferral audit.

- Relayed gossip attribution and signed editor ops: SYNC-SIGNED-PLANE, NO_GO until the
  canonical network rollout.
- Docs-ALPN peer admission and namespace rotation (needs a decision).
- Scoped-invite redemption and the FR-4 role UI (D-32, user-scheduled after this track).
- Pinning the relay's bootstrap owner (D-23 relay host undecided).
