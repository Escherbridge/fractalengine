# fe-runtime/src — module rationale

## §scene-change

`messages.rs::SceneChange` is the process-wide scene-delta contract. Every
node-scoped variant carries its owning `petal_id`; a WebSocket receiver must
use `SceneChange::petal_id()` to scope both deltas and transform rollbacks.
Producers must not emit a node-scoped scene change when they cannot determine
that petal. This makes deleted-node events safe to filter without a post-delete
database lookup.

## §property-bridge (`bim_primitives_on_paths_20260712`, C5)

`shared_node.rs::PropertyValue` is the Tauri↔Bevy bridge's authoritative
property shape (`String`/`Number`/`Boolean`/`Array`/`Json`). `fe_sdk::property::PropertyValue`
(`fe-sdk/src/property.rs`) is the extension-facing mirror with the same four
scalar shapes plus its own `Json(serde_json::Value)` catch-all. `From`/`Into`
impls at the bottom of `shared_node.rs` keep the two convertible without a
third enum: `fe_sdk::PropertyValue::Array` doesn't exist on the SDK side, so
the `fe-runtime → fe-sdk` direction folds `PropertyValue::Array` into `Json`
losslessly via `serde_json::to_value`. `fe-runtime` depends on `fe-sdk` (not
the reverse) — `fe-sdk` must stay serde-only/engine-decoupled per its own
`AGENTS.md`.

Primitive descriptors (`fe_sdk::primitive::PrimitiveDescriptor`) ride on this
bridge as `PropertyValue::Json(descriptor.to_json())` — see
`fe-ui/src/verse_manager/AGENTS.md` §primitives for the render-side contract.

**Untagged-deser caveat:** `PropertyValue`'s variant order (`String`, `Number`,
`Boolean`, `Array`, `Json`) means untagged deserialization tries earlier
variants first, so a `Json` payload shaped like a scalar or array (e.g.
`Json("hi")`, `Json([1,2,3])`) deserializes back as `String`/`Array`, not
`Json` — round-trip is lossless only for JSON *objects*. Primitive descriptors
are always objects, so this doesn't affect them. Do not "fix" this by
reordering variants or making the enum tagged without confirming nothing
depends on the current untagged wire shape (Tauri↔Bevy bridge).

**Fast-follow:** the two `FsBlobStore` path conventions in `fe-hexon` need
reconciling before the texture registry is wired at runtime (registry starts
empty today, so it's currently inert) — flagged for whoever wires FR-4
registry population / P2 texture install.

## §blob-store (P2P Mycelium Phase A)

`blob_store.rs` defines the `BlobStore` trait, decoupling asset-byte storage
from the database layer. It lives in `fe-runtime` (no crate dependencies) so
that both `fe-database` and `fe-sync` can depend on it without creating a
cycle:

```text
fe-runtime (trait)  <---  fe-database (uses handle)
       ^
       |
    fe-sync (FsBlobStore impl)  --->  fe-database (existing dep)
```

Hashes are raw BLAKE3 digests (`[u8; 32]`); hex encoding is provided for DB
rows (`content_hash` column) and paths (`blob://{hex}.glb`).

`MockBlobStore` must produce the same digest as `FsBlobStore` so tests can
cross-check hashes — that is why `fe-runtime` takes a direct `blake3`
dependency despite the trait itself needing none. If that dep weight ever
becomes an issue, move the mock into `fe-sync` instead.

## §timeseries (M2/F6)

`timeseries.rs` is the canonical per-verse timeseries-fabric vocabulary:
`TimeseriesMode` (mirror/sharded/balanced) and `VerseTimeseriesSettings`
(mode + `replication_factor` R + `bucket_width_ms`, epoch-aligned). It lives
here for the same reason `RoleLevel` lives in fe-policy: every layer needs it
(fe-database persists and re-emits the `ts_*` verse-row columns, fe-sync's
fabric/placement/retention dispatch on it, fe-ui renders it) and fe-runtime
is the lowest common dependency — one definition, no drift, no cycle.

- `mirror` is the `#[default]` **and** the pre-F6 behavior exactly (every
  peer holds every shard), so a fresh verse and an F5-era peer are
  byte-compatible with the F6 fabric.
- `sanitized(mode, R, bucket)` validates raw settings input and returns a
  human-readable reason on bad input — the DB handler rejects with it; row
  parsing never calls it (see below).
- `from_verse_row` parses the `ts_*` columns with **clamping, never
  rejection**: missing or invalid fields fall back to the mirror defaults,
  because an inbound verse manifest must never fail to converge a fabric
  over a settings typo (a manifest is data, not a form submission). This is
  the same asymmetric-validation split fe-sync's `ShardLedgerEntry::from_row`
  uses. The pre-F6 row (no `ts_*` at all) parses as pure defaults.
- `serde` shape: the enum serializes lowercase (`"mirror"`|`"sharded"`|
  `"balanced"`, `#[serde(rename_all = "lowercase")]`) — the verse-row column
  value, the shard-ledger row value, and the dump-JSON value are all the
  same label, matched by `as_str`/`parse` (not serde) on the DB-column paths
  where the value rides a plain `String`.
- `DEFAULT_BUCKET_WIDTH_MS` = 1 day. Bucket indices are
  `recorded_at_ms.div_euclid(bucket_width_ms)` (fe-sync `ShardId`), so the
  default aligns buckets to day boundaries.

## §api-reply-correlation

`app.rs::PendingApiRequests` holds the API threads' pending `DbResult` waiters.
`DbResult` carries **no correlation id**, and results are not only replies to
API commands: the DB thread also fans unsolicited work into the same channel
(the relay startup scan's `HierarchyLoaded`, GUI-initiated hierarchy reloads).
Pairing the next result with the oldest waiter (plain FIFO) therefore
cross-pairs whenever an unsolicited result races a pending request — the
relay's production `/ready` ping rides exactly that window (a `HierarchyLoaded`
would satisfy the ping while the ping's real `Pong` went to the hierarchy
waiter, which then either mishandled it or dropped it).

The queue is therefore keyed by reply **family** (`ReplyKind`), not by arrival
order:

- `enqueue_for(cmd, tx)` derives the family with `reply_kind_of_command`;
  `reply_kind_of_result` derives it from a result; `try_deliver` only hands a
  result to the oldest waiter of that one family.
- `DbResult::Error` is a wildcard: it is delivered to the oldest waiter of any
  family (`deliver_to_oldest`), because a failure has no family-specific shape.
- A result whose family has no waiter is dropped, never delivered to some
  other family.

**Maintenance rule:** every `DbCommand` that expects a reply must be mapped in
`reply_kind_of_command` and its reply in `reply_kind_of_result` —
`reply_kind_mapping_is_consistent_for_every_awaited_command` fails otherwise.
Fire-and-forget commands (`TransformPersist`, telemetry writes) must send
directly and must NOT enqueue a waiter: registering a waiter for a command the
DB thread never answers leaves a dangling entry that later results can
mis-deliver into.

