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
Raw local record IDs and wire bookkeeping never enter the node row.
Live complete snapshots validate all present values against the recipient's already stored
enforced field definitions through the shared validator. A single bulk capture caps 256
applicable definitions, fences those exact definitions at write time, and refuses malformed
or incompatible values. This is enforcement of known local rules, not schema replication.
Tombstones remain deletion-dominant rather than requiring old data to satisfy later live
schemas. Final assembled receive SQL/bindings cap at 4 MiB before execution. Complete snapshot
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
