# Committed scene publication

## Authority and provenance

This directory owns the post-commit ordinary-node publisher, replica-open current-state
recovery, and the versioned full-node receive adapter. It does not publish NodeDto: that
projection omits properties and is not a reconstruction source.

`prepare` requires exactly one stored node and resolves its stored Petal/Fractal/Verse.
The actual DB-thread identity must currently have Editor access. The sealed authority
snapshot, exact stored ancestry, full nongeometry row, point coordinates, latest local
journal fact, and materialized scene clock are checked again in the transaction immediately
before enqueue. No caller role, Boolean permission, or payload publisher is authority.

A journal proof requires journal_version=1, exact after_state=current normalized row,
exact retained scope, positive original HLC/version, nonempty actor/log identity, and the
shared completed-Save boundary validator, which also refuses a fact restored from a petal
snapshot archive (`payload.archive_import_id`; handlers/AGENTS.md §petal-snapshot-replication
"Archive history is never proof"). `node_log` is LOCAL-only: inbound dispatch does
not admit it. `op_log` is also not an inbound applied table. This structural boundary is
why a locally committed API operation can retain its distinct authenticated operation actor
while the actual local Editor signs the resulting row. Source DID equality alone would
neither prove local provenance nor preserve that legitimate delegated case.

A legacy tombstone without a matching journal fact requires a unique LOCAL NodeTombstoned
operation with the original marker HLC, source, Petal and exact affected node membership.
New lifecycle deletes capture/fence that member set and actual stored actor authority in
the same transaction. Legacy direct-waypoint tombstones whose old operation omitted its
member list remain deferred rather than acquiring invented proof. Elevated/raw/seed,
legacy uncovered, and inbound-only rows without matching local proof also remain deferred.
No recovery path manufactures a baseline, grants, version or HLC for them.

## Wire and ordering

A complete `node` row carries `_fe_scene={version:1, complete:true, hlc:<decimal string>,
publisher:<DID>}`. Original committed HLC is retained on every retry/open. The authenticated
transport entry author must exactly match publisher, and the existing inbound confinement
and actual stored author-role gate run before this adapter. The adapter repeats captured
Editor/ancestry authority and row/version fences at apply time. Payload metadata never
authorizes an author. Full snapshots cannot move an existing node to another Petal.

`node_scene_clock` also retains an exact server-CBOR row fingerprint captured in the same
transaction. A current read reports OriginalSceneCommit only while that fingerprint matches
the current row, so a later unversioned tombstone or uncovered write cannot inherit stale
original-commit provenance. It is LOCAL materialization metadata, never an inbound replicated table or
an admin projection. It is included in explicit development reset. A positive signed-i64
HLC domain with rollover headroom and a maximum five-minute future wall offset bounds
received clocks. Accepted row and clock share ONE DB transaction. Only after that transaction
succeeds is the process HLC advanced, so a subsequent local edit is causally newer. Startup
restores the maximum persisted scene clock alongside op_log; an unreadable/out-of-domain
scene watermark fails initialization rather than silently resetting causality. The startup
success handshake and shared handle are published only after all fatal clock recovery and
process HLC initialization succeed.

The production admission queue carries the actual DB-thread local publisher into receive
ordering so a locally committed delegated actor is never substituted as a publisher in a tie.
The exact latest local journal fact (including actor, payload and record identity) or its
absence is captured with a server-CBOR fingerprint in the same read used for ordering.
The receive transaction fences that capture before any row/clock write, even when a
concurrent same-value local Save leaves row bytes and the scene clock unchanged.
The guard selects both row_version and the raw-record fingerprint before ordering; its
expected object list has that same exact shape, preserving the empty-list absence fence.
Legacy direct apply wrappers retain their signatures and cannot invent that missing identity.
Per-key clock (per_key_clock_20261113): a complete snapshot carries `_fe_clock`, the publisher's
own per-key versions (`field_clock/AGENTS.md` §wire). Each key orders by its own `(hlc, author)`
(`field_clock::merge::winner`), so two editors writing different keys both survive, and the same key
converges to the max on every peer. A snapshot without `_fe_clock` (legacy writer) is seeded at the
legacy floor (`field_clock/AGENTS.md` §legacy-migration) and loses to any clocked version.
Older/equal full live rows are ignored without materialization events. Local tombstones dominate every
live row regardless of clock. An incoming tombstone can dominate a newer live state; an
older tombstone cannot replace a known later tombstone. Replica-open transport timestamps
are never substituted for original commit clocks. The local clock is an ordering watermark,
NOT a delivery acknowledgement. Enqueue failure leaves the exact durable source eligible
for the same-clock retry. A journaled edit below the known scene clock still publishes while it holds a
winning key (`publisher.rs` `journal_covers`); merged keys are never stamped as own (`inbound.rs`
`pending_local_keys`). Tombstones keep the strict rule.

### §rejudge-pending

A node's own versions are stamped only when `publisher::prepare` commits, so a committed local edit can sit
unstamped (publication deferred, no replication sender, over the row bound). A re-judge (`merge_once` with
`rejudge`) rebuilds the row from `identity_of` plus the admissible winners, which used to drop such an edit;
the next publish then saw the reverted key equal to a winner and stamped nothing (review finding database-r2-2).
Invariants: **a re-judge never discards a committed, unstamped local value whose author is still authorized at
its write time**, and **a key is pending-local only if the local journal proves a local, authorized actor
WROTE its current value.** Appearing in a fact's full `after_state` proves nothing: every fact stores the whole
row, so a remote key merged earlier rides along in every later local fact (review finding: a demoted Editor's
key, carried in the owner's unrelated edit, was kept by the re-judge and re-signed by the host's publish).

Rule (`inbound::local_writes` + `pending_local_keys`, used by the re-judge AND the ordinary inbound merge):

- Precondition (unchanged): the latest `node_log` fact's `after_state` equals the stored row.
- **Walk back** over the unpublished facts, newest first, grouped into segments: one commit is one segment
  (same HLC and `source_did`; an atomic save of up to 201 facts is one segment, so its intermediate states
  are never needed). The walk stops at a fact whose HLC a stored version carries (a publish stamps own
  versions at the latest fact's HLC, so that fact and everything before it is published; a forged colliding
  HLC only stops the walk early), at a non-v1, other-scope or non-consecutive fact, or after 32 segments
  (512 facts read). That fact's `after_state` is the starting state. If the walk reaches row version 1 and
  it is the `object_created` baseline, the start is the empty state and the creation is a local write;
  otherwise the oldest segment only provides the starting state.
- **Diff** consecutive segment end states. A key an ordinary fact (or the creation baseline) changed is
  credited to that fact's `source_did` at its HLC; a key changed across any other `baseline` (an uncovered
  interval: an inbound merge or another unjournaled writer) is cleared, since no local fact made it.
- **Authorized at write time** (re-judge): a credited key is kept only while `VerseView::admits_at` admits
  (`source_did`, that fact's HLC). A revoke that predates the write demotes it. The ordinary merge skips this
  check: the write passed the lane's authority check when committed, and retroactive demotion is the re-judge's
  job (a local API actor authorized by token may hold no role statement).
- **Not already published**: the key's cached winner, read from the versions stored BEFORE this merge's fold
  (an incoming version replacing its own author slot must not hide what was published), must back it; a
  stale cached winner proves nothing, so the key is not pending (as `clocked_row::unstamped_local_keys`). A
  value equal to the published winner is not pending; a key with no cache entry is pending unless absent.
- **Outranks**: as the version the next publish stamps (latest fact's HLC; author = journal author in a
  re-judge, local publisher in a merge), it beats the merge winner.

Kept keys are copied from the stored row into the rebuilt one, so the next publish sees them differ from their
winner and stamps them. A re-judge still demotes: a non-pending key falls back to the admissible winners, the GUI
repaints that pose (`NodeAdded`), or the node is removed when no admissible transform is left. A demoted local
actor's publish is refused by `prepare`'s current-Editor check; a re-grant does not revive it (QA-0131).

Publisher side: `journal_covers` accepts a key the re-judge removed (no committed value and no winner left);
such a key is never stamped. `stamp_own_versions` stamps a committed key over a STALE cached winner only if
`local_writes` lists it. Without that, an unauthorized newer version replacing a winner's slot (ordinary
merge keeps the stale cache entry and the old value) let the next publish of any unrelated local edit stamp
the remote value as the host's own. Keys over a live winner or with no cache entry stamp as before.

Ordinary inbound merge: the old rule had the same looseness. An incoming version replacing the cached
winner's own author slot made `published` read `None`, so the stored remote value counted as a local write
that outranked an incoming version with an older HLC. The merge then kept the old remote value, and the next
publish stamped it as the host's, overriding the newer remote edit. The changed-key rule and the pre-fold
lookup close both.

Residuals: (1) pending keys are found only while the latest journal fact still equals the stored row; if an
inbound merge already changed the row after the edit, a later re-judge cannot find that edit's keys. (2) A
local write older than the 32-segment window, or behind a non-journal fact, is not found: the re-judge drops
it, and over a stale winner the publish does not stamp it. (3) A local write over a key whose cached winner
went stale (its author sent an unauthorized newer version) is dropped by a re-judge instead of kept: the
conservative direction. Proof: `op_log::tests::a_failing_statement_rolls_back_the_op_log_row_too`
("Pending" and "Laundered" legs).

The complete receiver replaces row contents with an inline geometry cast and commits that
replacement with its clock. This preserves property deletion instead of recursively merging
old keys back. Only nullable TOP-LEVEL declared columns are normalized to absence/NONE;
nested JSON nulls, including null array elements and null property values, survive exactly.
Raw local record IDs and wire bookkeeping never enter the node row.
Live complete snapshots validate all present values against the recipient's already stored
enforced field definitions through the shared validator. A single bulk capture caps 256
applicable definitions, fences those exact definitions at write time, and refuses malformed
or incompatible values. This is enforcement of known local rules, not schema replication.
Tombstones remain deletion-dominant rather than requiring old data to satisfy later live
schemas. Final assembled receive SQL/bindings cap at 4 MiB before execution. Complete snapshot
validation caps rows at the row bound (§row-bound) and checks the renderer's rotation/finite transform contract.

### §row-bound

Numbers as of 2026-11-14 (owner decision: node limit back to 64 KiB, with correct headroom):

| Constant | Value | Where |
|---|---|---|
| `MAX_NODE_BYTES` | 65_536 (64 KiB) | `fe_sdk::property_schema`; `MAX_PROPERTY_BYTES = MAX_NODE_BYTES`, `MAX_SCHEMA_BYTES` 65_536 |
| `wire::OWN_KEY_OVERHEAD_BYTES` | 64 | measured 50, plus margin |
| `field_clock::wire::MAX_OWN_KEYS` | 512 | spec §3.11 |
| `wire::MAX_ROW_BYTES` | 2N + 512 x 64 + 16 KiB = 180_224 (176 KiB) | inbound check, publisher, clocked rows |
| `wire::WRITE_ROW_BYTES` | `MAX_ROW_BYTES` - 1 KiB = 179_200 | write-time projection ceiling |

Everything derives from `MAX_NODE_BYTES`, so raising the node limit later changes one constant. Every write
path accepts a node snapshot up to `MAX_NODE_BYTES` (`node_history::check_snapshot_size`, refusing with
`node_history::NODE_TOO_LARGE`), and a published row carries the node plus every value its publisher authored
a second time in `_fe_clock.own`.

Per-key overhead, measured on the encoding: a property in the node costs `"name":value,`; the same key in
`own` costs `"properties/name":{"hlc":"<HLC>","value":value},`. The difference is `properties/` (11) +
`{"hlc":"` (8) + up to 20 HLC digits + `","value":` (10) + `}` (1) = 50 bytes. The earlier
`2N + 16 KiB` bound (same bug class as the 96 KiB literal fixed earlier the same day) ignored it: a node at
the limit made of 500 small properties produces a ~153 KB row, over 2N + 16 KiB = 147_456, so it was
accepted locally and deferred by recovery forever. With 512 keys x 64 the bound covers every node at the
limit whose own keys are its present keys; the 16 KiB covers the marker, the clock header (two DIDs of at
most 512 bytes each) and the GeoJSON point.

**Write-time check** (`publisher::check_publishable`, called before commit by object creation
(`object_creation::prepare`), property set/delete/clear (`entity_property.rs`) and checked path edits
(`checked_mutation.rs`)). It builds the row exactly as `prepare` would (`stamp_own_versions` over the node's
stored versions, `encode_own`, the marker) at the widest stored HLC, then refuses with
`wire::UNPUBLISHABLE` when the row would carry more than `MAX_OWN_KEYS` own keys, exceed `WRITE_ROW_BYTES`,
or hold a key the strictest reader refuses (`parse_own`, for example a property name over 1 KiB). This
catches what the bound cannot: deleted-property entries (`"key":{"hlc":..,"deleted":true}`, about 60 bytes
plus the key, kept in `own` after the key leaves the node) and the publisher's losing values on keys another
author now wins. A write that does not make an already unpublishable row larger or wider is let through, so
such a node can still shrink. Transform-only writes skip the projection: a transform changes the row by at
most the transform key twice (under 1 KiB), which is the gap between `WRITE_ROW_BYTES` and the bound. A node
with more than 509 properties (512 minus transform, name, interactive) is refused with a message naming the
512-key limit; before this it committed and never published.

Covered as well (2026-11-14): rename (via `node_history::commit_mutations_in_scope`), extension commits (`plugin_host.rs`), petal snapshot import (`petal_snapshot.rs`), editor batches (`editor_batch/prepare.rs` checks every node whose facts write a row, one stored-version read per node, before anything is committed; transform-only members are skipped) and stamp edits (`stamp_overrides.rs` override save, `stamp_promotion.rs` `prepare_birth`, shared with `crud::promote_instance_handler`). Not covered (they still rely on the bound plus `RecoveryReport.oversized`): the promotion of an already-existing stamp node writes nothing journaled; creation of a node by inbound peers is the inbound check's job. Under an API `Scoped` grant the projection is computed for the API actor,
not the DB-thread publisher, so the publisher's deleted and losing entries are not seen there.

**This is a wire limit: every peer of a Verse must run a build with the same value.** The same constant
bounds clocked verse, fractal and petal rows (`clocked_row.rs`) and the inbound check (`inbound.rs`). A peer on
a 144 KiB-bound build refuses a many-key node row between 144 and 176 KiB; a peer on the short-lived 528 KiB
build accepts every row this build publishes. `RecoveryReport.oversized` counts deferred rows that failed the
bound (matched on `wire::ROW_TOO_LARGE`). Data that does not fit one node belongs in blob storage behind a
reference, not in a larger row.

Bounds downstream of the row bound at a 64 KiB node / 176 KiB row:
- The receive transaction cap (4 MiB script + bindings, `inbound.rs`) holds the stored-row CBOR fingerprint,
  the before-row, the merged row and the written own versions: well under 1 MiB at the limit.
- `scene_asset_audit.rs` `ROW_CAP` follows the node limit (`MAX_NODE_BYTES + 32 KiB`).
- fe-sync carries a row as a docs entry plus a blob (re-fetch cap 16 MiB), never as a gossip frame.
- Extension buffers: `fe_sdk::plugin_host::MAX_HOST_BYTES` (64 KiB, non-node JSON) and
  `fe_runtime::scene_hook_gate::MAX_HOOK_INPUT_BYTES` (64 KiB) are again about the node limit; node payloads in
  the plugin host are `N + 8 KiB`.
- Each journaled edit stores the full after-state (up to 64 KiB) plus the change data (a property set carries
  its value again), so history grows by up to about 128 KiB per edit of a full-size node; a history page
  (8 MiB response cap in fe-api) holds at least 64 such entries. Growth is bounded by the write budget
  (`handlers/AGENTS.md` §write-budget), never pruned; retention is an open decision.

### §publish-reclaim

Every publication stores its complete row in the canonical blob store (`enqueue` -> `add_blob`), so a node
edited in a loop used to leave one superseded row blob per edit. `publish_after_commit` (and the tombstone
publisher) now reclaim: after the new row is enqueued, `reclaim::superseded` records its hash as the node's
current published row and removes the previous one, unless
- it is the same hash (identical consecutive rows are content-addressed to one blob), or
- another node's current published row in the same store has that hash, or
- an `asset` row (`content_hash`, indexed) or a hexon `crate_entry` (`asset_hash`) names it.

Safety argument: the previous hash is only ever a hash this process enqueued as this node's own row, so it
is a scene row JSON that embeds `node_id` and the publisher DID; no other node's row can share it. Assets,
device payloads and observation payloads reach the store through asset rows (checked); path stamps name GLBs
by `blob://<hash>` and a stamp pointing at a row JSON would not render anyway. A pending upload (bytes added
before its `asset` row) is not visible to the DB; it could only collide by uploading this exact row JSON,
which `asset_gc` does not protect against either. The asset check is the rule an `asset_gc` pass also follows;
the blob directory is still never scanned.

Bookkeeping is in memory (`reclaim::LAST_PUBLISHED`, keyed by store instance and node id, 65,536 nodes, cleared
when full), not in `node_scene_clock` (its row is also written by inbound applies with a remote publisher, and a
new column needs a schema change outside this directory). Consequences: recovery on replica open only
remembers what it re-sent (identical bytes after a clean restart, so the next edit reclaims them); a row
published in an earlier process and changed before the next open stays (at most one blob per node per
restart); an enqueue that failed after `add_blob` leaves that blob.

Ownership contract: fe-database owns the canonical blob store copy and keeps at most the newest enqueued row per
published node. fe-sync owns its iroh store copy and its reclaim. A `WriteRowEntry` whose blob is already
gone has been superseded by a later entry for the same `(table, record_id)` that is behind it in the bridge
(the new row is enqueued before the old blob is removed, and the bridge and fe-sync's pending queue are FIFO),
so fe-sync may skip it; today it logs `WriteRowEntry: blob not found in store` at warn.

Legacy markerless partial node rows retain their existing merge path while no complete
version is known. After a complete version is known, an unversioned live row cannot prove it
is newer and is explicitly ignored; legacy tombstones retain non-resurrection behavior.
Older peers may ignore the new marker and keep their former merge behavior. Full-state
property deletion and original-clock guarantees require upgraded peers; this is not a
wire-compatible guarantee about old clients. Unsupported/malformed markers are denied.
Missing parents enter the existing Deferred Parent admission/retry spool before dispatch;
this adapter also returns Deferred when a re-resolved Petal disappeared after admission.

## Publication and recovery bounds

Ordinary successful native/API create/import/property/transform/checked-path/stamp/node-batch
commands call the same publisher AFTER their local commit. Plugin batches also use it;
there is no remaining optimistic raw-node plugin publishing path. Rename now commits its
name and ordinary journal fact together; duplicate uses journaled creation, fences its source,
preserves source interactive state and original f64 stored pose, and keeps the renderer DTO's
existing f32 projection. Native Delete retains a durable sync-safe tombstone rather than
physically removing its replication source. The compatibility hard-delete helper remains for
legacy callers/tests and is not a sync-safe operation.

Replica open first runs the existing Manager-fenced hierarchy catalog repair, then scene
recovery with current per-Petal Editor authority. Scene recovery pages through every node
(including tombstones) and publishes in batches of at most 1024 rows and 8 MiB of prepared
wire bytes (§recovery-batches); a large Verse is no longer refused.
Unsupported/unproven/unauthorized members produce explicit
deferred counts; a partial report never says Ready. Each final emission repeats its fences.
Backpressure uses nonblocking try_send and the shared drop counter. Blob-store/channel/
fence failure reports exactly how many node rows were enqueued before recovery stopped;
"enqueued" does not certify transport arrival or peer apply. Another open retries from
unchanged durable facts, including equal-clock queue failures. Deletes cap/fence 1024 members.

Local checked batch atomicity and completed per-object Save proof do NOT imply atomic
peer application. Node envelopes can arrive independently. Node journals, historical replay,
Group registry/membership transactions and measurement facts are not made convergent by
this row publisher. Their separate mappings remain deferred.

### §recovery-batches

Until 2026-11-14 `recover` refused a whole Verse over 1024 rows or 8 MiB of prepared bytes ("no prefix
published"). The commit that introduced it (`65d3bd41`) states no reason, and none of the candidates holds
against the code:
- Not peer-visible atomicity. `_fe_scene.complete:true` marks one row as a full snapshot, not the scene as
  complete. Every row is its own envelope with its own fences, original HLC and per-key clock; peers
  already apply rows independently (above), and the old path itself stopped mid-sequence on a full channel.
- Not ordering. Each row's fences (`prepare`'s statements) reference only that row, its journal fact or
  tombstone operation, its clock rows and its own asset; nothing in one row's proof depends on another
  row having been emitted. Dependencies are emitted before their own node inside `emit`, so a batch
  boundary never separates an asset from the node that needs it.
- What it did buy: bounded memory (all prepared rows were held at once) and bounded DB-thread time. Both
  are kept by batching instead of refusing.

Current shape: node ids are read in keyset pages (`node_id > $scene_after ORDER BY node_id LIMIT 1024`;
the cursor always advances, so it terminates). Each prepared row joins the current batch until the batch
would exceed 1024 rows or 8 MiB of wire bytes (row plus dependency bytes); the batch is then emitted in id
order and freed before more rows are prepared, so at most one batch (about 3x its wire bytes, since
bindings hold the row and journal fact again) is in memory. A row whose own bytes exceed 8 MiB, which
the row bound makes impossible today, is counted `deferred` + `oversized` and skipped, never retried in a
loop. Per-row failures are unchanged (`deferred`, `oversized`); `dependency_deferred` and the unplaced
upload pass are per row or run once after the last batch, so batch boundaries do not affect them.
Before every batch after the first, recovery waits up to one second for the replication bridge (256
slots in production) to drain below half capacity, so a large Verse does not trip the nonblocking
`try_send` on a stalled bridge; a still-full bridge fails exactly as before ("stopped after N rows
enqueued in batch B") and the next open retries from durable facts. `RecoveryReport.batches` counts
batches sent; the open log line reports it. Rows created during recovery with an id below the cursor are
not re-read; they were published after their own commit.

## Dependencies and honest limits

New native imported assets are stored with their authorized Petal and operation uploader
in the same asset/node/baseline transaction. Only current metadata anchored to that Petal
with retained local upload/admission provenance can be emitted as a scene dependency,
before its node. The inbound asset allowlist dispatches exclusively through
apply_anchored_asset_row, which removes owner_did before CREATE; static-row dispatch excludes
asset, the union dispatch only handles its declared timeseries tables, and node merge writes
node only. Thus a nonempty retained owner_did identifies a locally admitted uploader, which
may differ from the actual publisher just as an API operation actor can differ. This field is
provenance, never role authority. The publisher strips owner_did from the wire, preserving
existing inbound quota protection. Exact asset metadata is fenced; metadata caps at 32 KiB. Inbound asset handling keeps
its existing immutable/insert-if-absent, author-role, Petal-confinement and quota protection.
Unanchored legacy assets, ownerless/inbound-only metadata and missing assets are explicitly
counted as deferred; no foreign metadata is re-signed merely because a local node references it.

Model rows and field-definition schemas are not currently inbound applied tables. A node's
full nested data can arrive without claiming its editing schema or Portal model metadata
arrived. Their presence is diagnosed as deferred. Publishing content_hash metadata does
not certify that model/blob bytes were fetched or rendered by the peer. The asset transport,
model/schema replication and legacy asset anchoring need their own authorized protocols;
this slice does not imply imported-object rendering is complete until live two-peer evidence
establishes those dependencies. Dependency gaps remain explicit even when a node row enqueues.

## uploaded-assets

QA-0158: `DbCommand::CreateAsset` (REST/MCP `upload_asset`, always Petal-anchored) calls
`publish_uploaded_asset` after the row commits and BEFORE the reply, so the asset row's
replication event precedes any placement the caller makes next. It reuses the dependency
path's `proven_asset_wire` (upload/adoption proof, `owner_did`/provenance stripped,
`_fe_scene_asset` marker, metadata fence) under a freshly captured Editor authority fence,
so the sync byte publisher retains the GLB before the metadata reaches the replica and
receivers prefetch on arrival. Non-scene MIME uploads are not published here: their only
consumer is a device payload, whose door publishes the referenced asset rows first.
Replica open republishes proven scene uploads that no node references
(`recover_uploaded_assets`, probe 1025 / cap 1024, counted as `uploads_enqueued`; refusals
fold into `dependency_deferred` and never block node recovery). Referenced assets keep
travelling as their nodes' dependencies. A placement later re-emits identical row bytes
(same content hash), which inbound insert-if-absent treats as a no-op.

## Verification ownership

Extend the existing real-store op_log journey and existing schema inventory checks; do not
add parallel HLC test functions. Parent owns the end-of-campaign single test sweep and live
owned-pair evidence. Source parsing is not runtime SQL, convergence, asset-rendering, or
failure-recovery proof. The existing shared-handle journey also authors an on-disk restart
with an inadmissible retained watermark and requires SceneClockRecovery instead of startup
success; the existing op_log journey uses the production ordering capture/guard for absent
and present latest facts before concurrent same-value Saves. These are unexecuted checks.

## Current native projection

`current::echo_applied` preserves actual DB result order, echoes successful/failed RowReports,
and coalesces full node current reads only for Applied/AppliedTombstone node reports and
successful asset metadata applies. Deferred retry outcomes go through this same path, including
retries after local role changes. A batch caps 1024 current node refreshes; asset lookup probes
reference 1025 and refuses overflow rather than refreshing an unlabelled prefix. Each read
uses actual recipient Viewer authority, exact stored scope/row/point/server fingerprint, clock
and asset metadata fences. LOCAL legacy asset availability follows stored node reference
reach even when uploader/anchor fields are absent; an explicit foreign anchor still refuses.
This does not authorize publication, which separately requires upload or explicit adoption proof. Assets expose a blob URI only for validated scene MIME/hash and
actual local byte presence, rechecked after the read transaction.

The GUI-only ReplicatedNodeCurrent DTO has full NodeRowSnapshot, scope, honest original scene
clock/publisher or LegacyReplicaApply provenance, and typed asset availability; it has no local
journal version and is not a NodeMutationReceipt. LegacyReplicaApply means the current row has
no matching retained original scene-clock fingerprint; it does not assert a new remote clock.
A later settled same-key inbound row can be returned honestly as current, with its own persisted
provenance. Local journal history is never substituted for that provenance. The private
ReadReplicatedNodeCurrent command derives stored ancestry and uses the same bounded Viewer
read. Its request nonce binds a native intent owner, not time, history or cross-command order.
It has no public REST/MCP route, API reply family or peer write; as a pure query it is excluded
from mutation mapping inventory. Native projection must reject stale read-owner/epoch/nav
matches and request fresh data after hierarchy hydration, not replay a held DTO.

These local legacy/journal improvements leave RenameNode/DuplicateNode permanent D-CL20
UnmappedDeferred, including their bypass counters. No canonical intent mapping is promoted.


## Explicit publication responsibility

Ownerless/unanchored legacy copies are never assigned an inferred uploader from a baseline.
The explicit DQA020 adoption action is documented in handlers/AGENTS.md. Publication accepts
only its exact LOCAL operation proof or retained local upload ownership. Wire metadata strips
both local owner and adoption provenance. Missing bytes under a fenced current read emit
SceneAssetNeeded AFTER the current result, with identity/hash/size/provenance and no local
history version. The sync service must independently admit the hint and signal readiness
only after verified bytes reach the shared store; the UI then requests fresh current state.

Scene dependency envelopes add _fe_scene_asset:1 as a routing hint for the native
byte-publication adapter. It is not stored as an asset field or used for admission.
Ordinary asset/API/device publication remains separate; signed scope/author and
current local upload/adoption proofs still determine authorization. Upgraded sync
holds marked metadata until verified source bytes are retained; old peers may ignore
the additive hint and retain their earlier metadata-first behavior.

Current reads verify canonical local regular-file digest and exact captured metadata
size after the guarded read, on blocking IO with a constant-memory streaming bound.
A corrupt/empty/directory entry becomes MissingBlob and produces a bounded Need; it
does not permanently suppress demand merely because a path exists. Existing larger
local files remain available when verified; missing/invalid files above the transfer
cap remain TransferLimitExceeded. This read is event-driven, not per-frame hashing.

## Private-current transaction diagnostics

The final read-fence transaction inspects all indexed SDK errors before returning.
SurrealDB 3.0.5 check() selects the first error, which may be a NotExecuted rollback
echo hiding the later primary failure. current_read_transaction_error chooses the
earliest non-NotExecuted error, with a deterministic earliest-echo fallback. It emits
only the statement index, fixed authority/node/point/clock/asset guard stage, static
SDK kind, whitelisted fixed reason and error count. Unknown stored/error text, SQL,
bindings, values and error cause chains never enter this diagnostic. Request-level
failures expose only a fixed request_failed reason and static SDK kind.

Guard SQL, bindings, actual Viewer/ancestry and full row/asset/clock fences stay intact.
A diagnostic repair does not establish which QA30 guard failed, resolve its pending
read retries or authorize a fallback state. The existing operation journey authors
actual rollback-primary selection and an unknown-secret redaction case. These cases
remain for the parent-owned final integrated sweep; no new test function is added.

## Exact current asset projection witness

QA31's controlled retry identified the asset_metadata guard, but did not identify
which retained field/type differed. The six-field snapshot must not be rebound as
serde_json values for SQL equality: pinned SurrealDB converts NONE and NULL to JSON
null, then binds that JSON null as SQL NULL; other typed values can also lose type.
Capture the decoded projection and a server-computed CBOR/base64 witness together
in one SELECT over the same projected subquery. Final fencing recomputes that exact
six-field projection's witnesses and compares every row/count/order before returning.
The opaque string witnesses never pass through JSON as typed database metadata.

JSON rows remain interpretation only. Keep actual Viewer/ancestry, node row/geometry/
fingerprint and scene-clock guards, and the same availability/file-verification rules.
Do not widen this witness to unrelated asset fields or capture data and witness with
two independent reads. The existing operation journey covers unchanged absent fields,
NONE-to-NULL and integer-to-float changes in schema-less legacy fixtures, every captured
field, creation/deletion and an unrelated descriptive rename. Existing full current/
legacy/Viewer journeys still apply; source authoring is not runtime evidence.

## Shared outbound/current transaction diagnostics

diagnostics::Transaction owns deterministic indexed-error selection for current
reads and the final outbound publication transaction. It skips earlier
NotExecuted rollback echoes when a primary error exists, retaining a deterministic
echo fallback. Only fixed operation/stage/reason labels, statement index, static
SDK kind and error count leave the classifier; request errors expose only fixed
operation and SDK kind. Unknown SQL/error text, bindings, rows and cause chains
are never interpolated. Publication's parallel labels describe its unchanged
authority, node, point, local proof, clock/write and asset guard statements.
The transaction SQL/bindings and post-success enqueue order are unchanged.

QA42's actual failed guard remains unknown until a fresh controlled run supplies
the new diagnostic. No guard is waived and no publication failure reverses a
successful local commit. The existing operation journey extends its actual
rollback and redaction cases to the shared publication classifier; these cases
are authored, not executed acceptance.

## Exact outbound asset projection witness

QA45's fixed diagnostics identify the outbound asset metadata equality guard;
they do not identify the exact differing runtime field. Its decoded JSON capture
cannot preserve native NONE versus NULL or other database types. Publication now
captures its exact nine-field metadata and server CBOR/base64 witness together
from the same projected SELECT, then compares the ordered opaque witness list
inside the existing final transaction. PUBLICATION_ASSET_COLUMNS is shared by
capture and fence, including name, created_at and publication_provenance; the
current reader's distinct six-field projection must not substitute for it.

Decoded metadata still supplies interpretation, local upload/adoption proof and
the separately stripped dependency bytes. Authority/ancestry, node/point/journal/
clock/asset checks, duplicate/count behavior, model/schema deferral and successful
enqueue order remain intact. An actual change still refuses and rolls back; no
field is coerced, dropped or mutated to make equality pass. The existing operation
journey authors unchanged absent-field, NONE/NULL/type and each projected-field
change, creation/deletion/duplicate and rollback cases. Source authorship is not
successful publication, peer delivery or an executed regression check.
