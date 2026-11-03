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
shared completed-Save boundary validator. `node_log` is LOCAL-only: inbound dispatch does
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
scene watermark fails initialization rather than silently resetting causality.

The production admission queue carries the actual DB-thread local publisher into receive
ordering so a locally committed delegated actor is never substituted as a publisher in a tie.
Legacy direct apply wrappers retain their signatures and cannot invent that missing identity.
Ordering compares original HLC, then authenticated publisher lexical order. Older/equal
full live rows are ignored without materialization events. Local tombstones dominate every
live row regardless of clock. An incoming tombstone can dominate a newer live state; an
older tombstone cannot replace a known later tombstone. Replica-open transport timestamps
are never substituted for original commit clocks. The local clock is an ordering watermark,
NOT a delivery acknowledgement. Enqueue failure leaves the exact durable source eligible
for the same-clock retry.

The complete receiver replaces row contents with an inline geometry cast and commits that
replacement with its clock. This preserves property deletion instead of recursively merging
old keys back. Only nullable TOP-LEVEL declared columns are normalized to absence/NONE;
nested JSON nulls, including null array elements and null property values, survive exactly.
Raw local record IDs and wire bookkeeping never enter the node row. Complete snapshot
validation caps rows at 96 KiB and checks the renderer's rotation/finite transform contract.

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
recovery with current per-Petal Editor authority. Scene recovery enumerates at most 1024
nodes (including tombstones), probing row 1025; over-limit refuses BEFORE any node sends.
It prepares all eligible rows and dependency bytes up to 8 MiB BEFORE sends, also refusing
oversize before publication. Unsupported/unproven/unauthorized members produce explicit
deferred counts; a partial report never says Ready. Each final emission repeats its fences.
Backpressure uses nonblocking try_send and the shared drop counter. Blob-store/channel/
fence failure reports exactly how many node rows were enqueued before recovery stopped;
"enqueued" does not certify transport arrival or peer apply. Another open retries from
unchanged durable facts, including equal-clock queue failures. Deletes cap/fence 1024 members.

Local checked batch atomicity and completed per-object Save proof do NOT imply atomic
peer application. Node envelopes can arrive independently. Node journals, historical replay,
Group registry/membership transactions and measurement facts are not made convergent by
this row publisher. Their separate mappings remain deferred.

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

## Verification ownership

Extend the existing real-store op_log journey and existing schema inventory checks; do not
add parallel HLC test functions. Parent owns the end-of-campaign single test sweep and live
owned-pair evidence. Source parsing is not runtime SQL, convergence, asset-rendering, or
failure-recovery proof.

## Current native projection

`current::echo_applied` preserves actual DB result order, echoes successful/failed RowReports,
and coalesces full node current reads only for Applied/AppliedTombstone node reports and
successful asset metadata applies. Deferred retry outcomes go through this same path, including
retries after local role changes. A batch caps 1024 current node refreshes; asset lookup probes
reference 1025 and refuses overflow rather than refreshing an unlabelled prefix. Each read
uses actual recipient Viewer authority, exact stored scope/row/point/server fingerprint, clock
and asset metadata fences. Assets expose a blob URI only for validated scene MIME/hash and
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
