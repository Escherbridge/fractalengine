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

## Decision being implemented (RUNBOOK §3, 2026-10-12 grill)

> Replicate the `role` table as ordinary replicated rows. A row is admitted only when its
> author is Manager+ at the verse scope; the owner bootstraps from `verse.created_by`.
> Chosen over signed role grants. After it lands: fix the distributed-query settlement race,
> then consider re-enabling responder filtering.

The §4 assumption carried from 2026-10-12 says: last writer wins by HLC per (verse, peer)
row; revocation is a tombstoned role row; the owner role is never replicated in.
**This track refines that assumption** (see plan §0 and the user-decision list). Read
literally, it does not converge under a concurrent revocation, and a skewed HLC can abuse
it. The refined form keeps every stated property:

- role rows are ordinary replicated rows;
- a row takes effect only if its author is Manager+;
- the owner comes only from `created_by` and is never replicated in;
- revocation is a non-empty tombstone row;
- each grantor's own statements resolve last-writer-wins by HLC.

It also adds one rule: different grantors' statements combine as "owner overrides, then the
maximum of the valid Manager grants". Nothing is decided by last-writer-wins across authors.

## Scope

1. **Role statements replicate.** Each local assign or revoke writes a per-grantor statement
   with the doc key `role/{scope}/{peer_did}/{grantor_did}`. It is published through the
   existing emission seam. The local `role` table becomes a materialized cache, computed by
   one pure, order-independent function in fe-policy and shared by the DB plane and the sync
   plane.
2. **Inbound admission of role statements.** Admission checks the statement's structure
   (grantor = the attested entry author, same verse, not Owner, the owner is not the target,
   the role is known). The fixpoint then decides whether the statement takes effect. Rows
   that fail only because a role or a parent has not converged yet are **deferred**, not
   denied, and are retried automatically when the missing role or parent lands.
3. **Owner integrity.** The verse manifest's identity fields are immutable once they land.
   Today any Editor can MERGE `created_by` and take over the verse:
   `replicated_row.rs:387-393` merges the whole payload. The governance fields need Manager+
   to change. Invitees record the real owner DID: today they record the inviter's hex key,
   at `handlers/invite.rs:167`.
4. **P2P-REF-CONFINE.** Interior rows (fractal, petal, node, iot_reading, and tombstones)
   must reference parents in the replica's own verse, both in their payload and in the
   durable row they overwrite. The role is resolved at the row's most specific scope.
5. **Sync-plane role view.** The `__shards` Editor+ gate is now constructible, so it is
   implemented. `note_verse_row` applies settings only from authors with governance
   authority. Compute requesters need Viewer+.
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
- **Threat matrix** (plan §RP-9): every row has a test that fails if its guard is removed.
- `cargo check` and clippy stay clean after every slice. One full test, e2e and harness sweep
  runs at campaign end (standing rule: no tests until the code is complete).

## Out of scope

The full justification is in plan §6, the deferral audit.

- Relayed gossip attribution and signed editor ops: SYNC-SIGNED-PLANE, NO_GO until the
  canonical network rollout.
- Docs-ALPN peer admission and namespace rotation (needs a decision).
- Scoped-invite redemption and the FR-4 role UI (D-32, user-scheduled after this track).
- Pinning the relay's bootstrap owner (D-23 relay host undecided).
