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
concurrently. All present known tables are deleted and checked empty in ONE
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
