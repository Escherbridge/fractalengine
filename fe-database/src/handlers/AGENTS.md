# Database handlers

## Node measurement projections

The desktop's `ReadNodeMeasurements` command uses `node_measurements.rs`, rather than user-authored
RawQuery. It resolves stored ancestry and requires Viewer access at database use. Both series and
point queries bind node, Petal, Fractal and Verse and require the node's current Petal membership.
A post-read ancestry/role check rejects intervening changes. Node, metric and units distinguish the
series; event timestamp and reading ID determine order. Shared runtime limits cap the series catalog
at 64 and points at 1,024, with explicit truncation flags. Units are never merged into one plotted
series. This is a local persisted projection; it does not claim distributed completeness,
missing-shard coverage, or a historical object anchor.


## Plugin reads and revocation

Plugin reads enforce the host's shared revocation lease at DB use and read return.
Prepared writes check it again immediately before invoking the ordinary atomic
commit helper. That check admits the transaction; later revocation does not
pretend to cancel an already committing transaction or change its true receipt.

Structured queries read at most 1,025 candidates in deterministic node-ID order,
with optional exact name narrowing. More than 1,024 candidates or 2 MiB refuses
explicitly. The shared NodeQuery.matches predicate runs before the final page
limit; Surreal numeric equality cannot admit float/int mismatches or hide a
later exact match behind a coerced first result. Overflow returns no partial page.

## local-catalog

CreatePetal used to commit only to SurrealDB, while live ingest built shard
placement in memory. Its missing replicated Petal mapping stranded signed shard
rows in quarantine after a restart. local_catalog publishes the committed Petal
readback and repairs existing stores on replica open. It never reinserts readings.

Repair uses the actual DB-thread identity, preflights stored Manager access,
and fences each current target's ancestry/policy in the existing hierarchy
transaction before reading. Viewer startup skips publication quietly. Explicit
wire columns restore Verse, Fractal, then Petal; storage IDs, view_hidden and
unlisted private columns are excluded. Each child read constrains the requested
Verse again. Candidate sets cap at 1024 Fractals and 1024 Petals; oversize refuses
instead of silently truncating. SELECTs have 5s DB timeouts. This is bounded local
catalog repair, not permission to publish arbitrary foreign rows or reconstruct
historical grants. Existing replication backpressure can still drop a publish;
failed/missing ancestry remains not-ready and is recoverable on another open.

## editor-undo

checked_mutation is a local-only compare-and-swap seam for full transforms and
one path point's position. It uses the ordinary reducer/journal and atomic op
commit, with captured Editor authority, exact ancestry/row/latest-fact/schema
fences before effects. Quaternions compare by orientation with 1e-4 position/
scale and 1e-6 dot tolerances; the final transaction fences exact captured data.
Path point edits preserve the complete JSON row metadata and array identity.

Receipts decode prepared after-state before commit and return only after the
atomic transaction succeeds. A UI request ID is retained in the actual change
fact. Resolve reads that exact immutable fact after current/stored scope checks;
matching coordinates are never proof of a committed request. Existing ordinary
API constructors and unguarded compatibility routes retain their signatures.

## Petal requirements

The handler stores validated declarations in existing petal.hexon_manifest. It
does not change render_distance, fetch content or gate the scene. Viewer reads
and Manager writes fence exact captured authority/ancestry. Set also fences
the raw loaded document and scope; explicit legacy metadata survives. Document,
readback equality guard and one ordinary op_log entry commit atomically. No DDL
change, backfill or object replay coverage is implied.

Committed declarations use local_catalog's existing exact Petal allowlist for
best-effort publication and restart repair. Inbound documents meet the same
bounded decoder. Changed requirements raise whole-row admission to Manager at
each current/requested Petal scope. Unchanged requirements preserve the Editor
floor for other Petal updates. Automatic installation/fetch, fallback rendering
and residency policy remain separate product decisions.


## Development reset

The DB thread captures DevelopmentResetPermit before spawning; no command owns
an enablement boolean. The live GUI and active test-peer permits are not runtime-
ready, so both legacy and correlated reset routes refuse before store reads or
writes. A trusted offline maintenance host may capture an offline permit only
while it exclusively owns an isolated store with no API/sync/plugin writers.

After host admission, clear_for_development_reset requires a resolved local DID,
at most1024 unique stored Verse identities and literal creator ownership of
EVERY Verse. ZeroVerse stores require the same developer gate. The immutable prepared intention carries its SQL and exact global ownership
snapshot with private fields; commit fences that snapshot in the clear transaction. Metadata is read through
INFO FOR DB and every table must belong to the dedicated complete reset allowlist;
unknown tables refuse. This list includes all generated schema tables plus
api_token, including private and canonical history. It is intentionally separate
from projection/admin/replication table lists. Trusted hosts must not run DDL
concurrently. All reset content tables are deleted and checked empty in ONE
transaction. Schema definitions and external blob files remain.

Seeding follows a committed clear. ResetOutcome distinguishes admission/clear
failure from a committed clear whose default-content restoration failed. The GUI
receives a clear notification in both latter cases and the exact correlated
outcome; no success receipt precedes the commit. Live admission stays disabled
until root provides coordinated writer shutdown and hot-store/catalog/session
invalidation. The existing isolated asset schema journey now retains its original
assertions and covers disabled/running/foreign/unknown-table refusals, unchanged
asset data, revocation between preparation and commit preserving all five stored
planes, ordinary history/private/token/canonical rows, complete clear and an
empty-store repeat. This authored journey has not been executed in this lane.

## EntityAccess

The local desktop Access controller resolves its explicit target through stored ancestry and requires Manager using the actual DB actor. It fences owner/default access, materialized nearest-scope role rows and scoped token metadata in one read transaction. Effective role display uses the ordinary owner/nearest explicit/default resolver, not obsolete get_scope_members. Bounds: 256 known peers/role rows, 50 returned tokens with explicit truncation, 1 MiB serialized readback and 5s statement timeout. These are post-read result bounds, not pre-allocation isolation. Current peer-registry entries and existing role subjects supply choices; no arbitrary DID input is offered.

Assign/revoke use ordinary checked statements and convergence. Revoke readback reports the actual effective role; other grants/inheritance can survive. Targeted token revoke independently requires stored Manager, exact ancestry, descendant scope and the captured jti/sub/scope/revoked tuple. The invalidation broadcast fires immediately after commit even when subsequent metadata read fails. Failed/unknown outcomes require an explicit read before another write. Invites and mints use this same exact request owner. Mint persistence fences stored Manager/ancestry and the active-token cap in its write transaction.

### Access token component boundaries

Access token membership compares parsed Verse, Fractal, and Petal IDs at every level present in the settings target. Candidate SQL uses only the next hierarchy marker (`-FRACTAL#` for Verse, `-PETAL#` for Fractal); a Petal admits only its exact scope. Candidate metadata is parsed and checked again before display, and targeted revoke uses the same membership predicate before reading or writing the token tuple. Hyphens inside IDs never authorize adjacent scopes. The existing durable Access journey seeds active hyphen-prefix foreign Verse, Fractal, and Petal tokens, checks that metadata is absent, refuses exact-tuple revokes, and verifies those records remain active.

Parsed Access membership also rejects marker characters (`#`) within any parsed ID, preventing malformed duplicate/misplaced hierarchy markers from becoming descendants. The durable journey includes duplicate Petal and misplaced Fractal markers at every target level. Truncation is recorded before filtering malformed candidates, so a filtered bounded result never claims to be a complete token inventory.


## terrain-write-fence

SetPetalTerrain retains the public CallerAuth/correlation grammar. Preparation first applies the existing validated caller policy, then captures the DB thread?s actual local DID at the stored Editor floor through entity::prepare_scope_permission. PreparedTerrainWrite has private fields and carries that exact target and authority/ancestry snapshot to use. No UI role is treated as authority and an API caller is not substituted for the publishing host identity.

The authority guards precede UPDATE inside the same commit_operation_atomic transaction as the ordinary terrain operation. The helper removes BEGIN/op-log/COMMIT slots; the final caller result is the UPDATE RETURN AFTER row even after adding guard statements. Terrain is read back from that committed row, and publication remains post-commit and best effort. The existing API and GUI success/failure correlation branches are unchanged. No strict input-vs-readback JSON equality policy or request field was added.

The existing Viewer-denial real-store journey now also captures then revokes an explicit role, changes default access after capture, and reparents after capture. Each attempted use must leave terrain and op_log unchanged; a final restored-authority write verifies actual readback and publication. These assertions are source-only until the sole builder's integrated sweep. Existing whole-document replacement/cache-merge and best-effort replication semantics remain; this fence does not claim multi-peer atomicity or stale-client document CAS.

## Field definition lookup ownership

`get_field_def_handler` is live internal infrastructure for checked definition mutations and enforced-schema get/set. Its callers perform their existing target authorization; retaining this lookup does not introduce a separate public single-fetch command.

Checked point edits also admit exactly one removed/restored row under the same full-array CAS.
Every surviving row remains byte-equivalent JSON, including timestamp and Bezier metadata.
The existing operation/journal failure journey covers removal, stale/Viewer refusal and inverse restore.

## stamp-overrides

`stamp_overrides::prepare` seals actual local Editor authority, full ancestry, stamped node
identity/owner path/index, complete before-properties, known versions, exact source path and
Petal map rows, and each enforced-schema snapshot. A separately consumable prepared value
lets the existing real-store journey change those rows between capture and mutation use.
`commit` writes the three existing flat override keys and sequential ordinary property_set
facts in one transaction using the ordinary operation commit boundary. Failure on the second
fact rolls back every property, fact and intention. No position/rotation/scale column is written.
The final fact alone carries the exact request receipt marker, so existing authorized receipt
resolution can recover one outcome without confusing intermediate property versions.
The single PropertySet intention records the bounded override batch; the journal retains
three standard facts and reconstructs ordinary historical versions without a new operation grammar.

Stamp metric bounds use the exact shared flattened Bezier geometry and f32 cumulative
distances used by the renderer. Captured raw positions/handles are compared before preparation,
and complete raw source rows still fence the transaction. Curved route distance is therefore
validated against the visible route rather than its anchor chord.

Checked anchor metadata admits one changed point only: XYZ, timestamp, all other rows and any
unknown trailing fields remain exact. The bounded slots are relative incoming/outgoing handles,
corner code 0/1/2 and smoothness 0..=1. Compact four-slot rows may gain canonical default metadata;
exact Undo can restore the original compact shape. Position moves still require untouched metadata.
The existing receipt journey reorders a route after dispatch before readback and proves no write or
projection; the real-store journey checks metadata inverse and rejects timestamp/multi-row rewrites.

## Portal URL transaction

`PreparedUrlWrite` seals the actual local Editor authority and stored ancestry, exact live
node/asset identity, current model snapshot and exclusive live reference. Capturing and using
node/model snapshots share canonical presence/value SQL projections. SurrealDB NONE becomes
JSON null on decode but is not equal to NULL in SQL; raw optional-field JSON is therefore not
an exact comparison fence. Both NONE and legacy NULL tombstones count as live here. Optional
absence is projected as a boolean plus a scalar, identically at capture and transaction use.

Existing local model metadata updates use the ordinary atomic operation with model URL CAS
and UPDATE RETURN AFTER. A unique imported node can lack this separate local metadata row:
only its first explicit nonempty Portal URL Save may initialize it. The SAME transaction fences
no existing model, exactly one live node reference and one existing asset row, then CREATEs local
model metadata with the URL and transform selected from that stored node's position/elevation,
rotation and scale. It does not alter the node, asset or node history and does not claim new physical
placement or replication. Selection never inserts metadata. Clearing an absent URL refuses
without creating a row or ordinary operation. Shared/assetless targets refuse writes.

The sealed preparation seam lets the existing real-store atomic journey introduce a new reference
or reparent after capture, proving refusal at use. The journey also imports through the production
handler, verifies missing metadata, explicit initialization/set/clear, induced CREATE rollback,
NONE/null binding behavior and unchanged node/asset/history. These are authored, unexecuted
assertions until the sole builder's integrated sweep.

## Current Portal URL read

ReadNodeUrl captures the DB thread identity's actual Viewer authority and stored ancestry, then
fences the exact live node/Petal/asset metadata in the SAME read transaction as the model URL
query. A write-triggered read can require that committed asset ID; an initial current read obtains
its exact metadata anchor in the fenced transaction. The model must belong to that same Petal.
Assetless nodes return a true absent URL. Missing local model metadata and shared assets return
typed storage availability without a selection warning; ambiguous or changed identities refuse. The reply
carries exact node, scope and actual asset identity, without any API waiter fallback.

The projection returns at most8193 Unicode scalar values plus the original scalar count, then
refuses more than8192 bytes/scalars instead of returning a rewritten/truncated URL. A bounded
legacy unsupported URL remains literal data so the UI can display its validation refusal. Pinned
surrealdb-core3.0.5 source confirms string::len counts chars and string::slice accepts an exclusive
end; the existing parser handles IF as a projection expression. This is source/API inspection,
not a query execution claim. Existing real-store URL journey adds Viewer/scope/asset refusals,
raw legacy read, size refusal and model replacement with no operation-log writes.

Portal URL writes use the runtime shared raw production/fixture policy at handler admission and again immediately before atomic commit. The fixture exception is host-captured and nonserialized; callers cannot grant it with request fields. Existing actual Editor, ancestry, unique model reference, CAS and readback checks remain mandatory.

### URL read diagnostics and storage availability

The read transaction inspects all response errors before decoding its known result slots. It selects
the first substantive error rather than a canceled-transaction echo and emits only a bounded static
fence/projection reason plus statement index. Raw database diagnostics can include stored URL
values, so they are never forwarded from this read path. The model/reference SELECTs remain
inside the same Viewer/ancestry/node identity transaction and return at most two rows each.
Ready and MissingModel are writable subject to the authoritative write fence; NoAsset and
SharedModel explain unsupported persistence inline. Bounded legacy URL values remain literal.

## Reset credential authority

The authentication singleton is recognized as private retained metadata, outside the reset content list. Reset rotates its generation in the same fenced clear transaction; failure preserves both content and prior credential validity. Ordinary API-token mint persistence additionally fences its captured generation, preventing a signed receipt from committing across a reset. See ../AGENTS.md section api-auth-lifecycle.

## Join credential tracing

The join handler excludes invite_string from its instrument fields. Invites carry
authority; source identity and the ordinary outcome provide diagnostics without
retaining the submitted credential in parent-span formatting or log exports.

## Ordinary creation and imported assets

`object_creation` is the sealed preparation/commit boundary shared by ordinary
empty/asset creation and native GLTF import. It captures the full initial node
state, actual local Editor authority, ancestry, applicable enforced-schema rows
and existing asset content identity. Trusted scoped API grants retain their
existing caller-authority contract; they do not become the host's local actor.
Preparation validates the exact captured rules; transaction guards reject changed
authority, parent bindings, definitions or referenced asset metadata at use.

Native import keeps the file lease and KTX2 admission path. It checks local Editor
access before storing bytes, then prepares again after blob storage. New Asset
metadata, the Node, one NodeCreated intention and one scoped baseline v1 commit in
the same operation transaction. Post-write guards confirm node/position/history
and imported asset content metadata. The baseline references the actual asset and
contains initial position, stored rotation, scale and property data. No second
dispatcher append is emitted. GltfImported/lifecycle publication follows commit.

Blob storage precedes metadata preparation and is not transactional with SurrealDB.
A failed or uncertain metadata operation may leave an unreferenced blob; this path
does not delete blob files or claim cross-store rollback. Existing GC remains its
own operation. Old unsupported/unscoped history is untouched, not relabeled.

The existing real-store operation/journal journey now imports through the actual
handler and checks one scoped baseline, initial replay and actual asset/TRS,
foreign read refusal, Viewer refusal before blob storage, and queued owner,
ancestry, schema and journal-failure rollback across Asset/Node/fact/intention.
These source assertions have not been executed in the author lane.


Creation preparation also decodes the complete typed current-state snapshot
before effects. A successful commit returns that prepared materialization; native
import and ordinary asset placement retain it in their correlated birth reply.
Quaternion-four and Euler-three storage both map to the same full TRS boundary,
with finite render precision checked before commit. No post-commit decoder can
turn a successful creation into an apparent failed creation/retry.

## Checked stamp promotion

The private GUI PromoteCheckedStamp adapter captures full source properties, path version, full
ancestry and metric scale. It uses the same flattened geometry and progress sampler as rendering
to validate the chosen instance. Prepared permission, path/Petal snapshots, path history, target
absence or exact existing stamp identity and enforced creation schemas are checked in the same
transaction as the ordinary NodePromoted operation and row. Existing stamps return confirmed full
properties through a guarded read transaction without creating another operation. The legacy public
PromoteInstance caller-current-index contract remains separate.


## stamp-map-snapshots

Checked GUI stamp promotion and atomic Apply share preconditions::PETAL_TYPED_SNAPSHOT_SQL.
Capture selects the full native Petal document as server-side CBOR/base64 alongside its ordinary
row fields; metric scale comes from that same row. The use-time transaction repeats exactly that
expression and compares the encoded string, including record identity and every stored field.
Only the string crosses JSON bindings, so native bounds geometry and absent optional fields do
not become GeoJSON objects or NULL in the fence. Snapshots above 4 MiB are refused. This does
not replace the independent current Editor, ancestry, source path/version, schema or target checks.

The pinned SurrealDB 3.0.5 source uses sorted BTreeMap object keys, separate NONE/NULL tags,
integer/float/decimal encodings, typed record IDs and typed geometry in its CBOR serializer.
The expression uses $this (the original CursorDoc), not a JSON re-encoding or a hash. The existing
real-store journey asserts these distinctions, absent terrain/default metadata plus native Polygon
bounds, and intervening terrain, bounds and optional metadata changes for both prepare/use paths.
The terrain race keeps world_scale=1, proving the fence protects the full map rather than scale
alone. These journey extensions are authored source; root owns execution and independent review.

