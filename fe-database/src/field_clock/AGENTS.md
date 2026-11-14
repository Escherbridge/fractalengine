---
type: Module Guide
module: fe-database/src/field_clock
track: per_key_clock_20261113
spec: ../../../conductor/tracks/per_key_clock_20261113/spec.md
---

# field_clock: per-key (HLC, author) registers

Every replicated verse, fractal, petal and node row is a set of keys. Each key is an independent
register: every member's latest version of it is stored locally, and the winner is the maximum
ADMISSIBLE version under `(hlc, author DID bytes, canonical value bytes)`. Admissible means the author
held the key's required level at the version's hlc (QA-0165, role at write time), never "holds it now".
Normative design: `conductor/tracks/per_key_clock_20261113/spec.md`. This file says how the module
implements it and where each rule lives.

| File | Owns | Filled by |
|---|---|---|
| `wire.rs` | `_fe_clock` parse/encode, row <-> key split | CLOCK-03 |
| `merge.rs` | pure fold: `winner`, `upsert_own`, canonical bytes | CLOCK-03 |
| `store.rs` | LOCAL table reads, in-transaction write fragments | CLOCK-03 |
| `migrate.rs` | `seed_legacy_floor` | CLOCK-03 |
| `judge.rs` | `VerseView`, `admissible` | CLOCK-05 (stub until then) |
| `publish.rs` | `publish_clocked_row` | CLOCK-08 (stub until then) |
| `rejudge.rs` | `rejudge_authors` | CLOCK-10 (stub until then) |

`mod.rs` carries `#![allow(dead_code)]` while the seams are unwired; CLOCK-13 removes it.

## §wire

```
_fe_clock = { "v": 1, "author": "<entry author DID>",
              "own": { "<key>": { "hlc": "<decimal>", "value": <json> } | { "hlc": "<decimal>", "deleted": true } } }
```

- `own` carries only the entry author's own versions. The iroh entry author is the only authenticated
  identity, so `parse_own` rejects an `author` that differs from it; a peer can never attribute a
  version to someone else.
- `parse_own(table, row, entry_author)` is a strictest reader: exactly the three members `v`, `author`,
  `own`; `v == 1`; at most `MAX_OWN_KEYS` (512) keys; every key passes `is_clock_key(table, key)`; every
  entry has exactly two members, `hlc` plus one of `value` / `deleted: true`. The hlc rule is reused
  verbatim from `SceneClock::from_fields` (`handlers/scene_publication/wire.rs`): a positive canonical
  decimal string. `None` = the column is absent (a legacy row, see §compat).
- `parse_own` takes the table: the identity-column and `properties/<k>` checks depend on it and a row
  cannot name its own table reliably (petal rows carry `node_id` and `fractal_id`, fractal rows carry
  `verse_id`).
- `encode_own(author, versions)` writes only `author`'s versions with hlc > 0, so floor versions and
  other authors' versions never reach the wire and the output always re-parses.

## §keys

| Table | Identity (insert-once, never a key) | Keys |
|---|---|---|
| verse | verse_id, created_by, created_at, namespace_id | every other column |
| fractal | fractal_id, verse_id, owner_did, created_at | every other column |
| petal | petal_id, fractal_id, node_id (anchor), created_at | every other column |
| node | node_id, petal_id, created_at | `transform`, `properties/<k>`, every other column (display_name, asset_id, interactive, tombstone) |

- `namespace_id` is identity here, as in `replicated_row.rs` `VERSE_IDENTITY_FIELDS` (T9: a replicated
  manifest may never change it). Spec §3.2 lists it under Manager-level governance keys; the stricter
  existing rule wins until the R-CLOCK-1 gate rules otherwise.
- `transform` is ONE key whose value is the object of the present `position`, `elevation`, `rotation`,
  `scale`, `edit_seq` columns. Folding them retires the merge.rs edit_seq register for clocked rows
  (QA-0126): the documented transform rule becomes the code's rule.
- `properties/<k>` is one key per top-level property, so two authors writing different properties never
  overwrite each other (QA-0156). Keys may contain `/` only through this prefix.
- `id`, `_fe_clock` and `_fe_scene` are never keys. A null column is absent and yields no key.
- `apply_keys(table, base_row, winners)` overlays winners on `base_row`; a deleted winner removes its
  column or property, and a deleted `transform` removes all five transform columns. Columns without a
  winner keep the base value, so callers pass the identity-only row when they want the exact inverse.

## §merge

- Order: `version_order` = `(hlc, author bytes, canonical value bytes)`. Canonical bytes =
  `serde_json::to_vec` after recursively sorting object keys (`canonical_value`), independent of
  serde_json's `preserve_order` feature. A deleted version has empty canonical bytes, so on a full tie
  a value beats a deletion; that only matters for two versions by one author at one hlc.
- `winner(versions_of_one_key, admissible)` = max over admissible versions; `None` when none is
  admissible. Callers group by key first.
- `upsert_own(existing, incoming)` keeps the newer version per (key, author): `Some(incoming)` replaces,
  `None` keeps what is stored. Same order, so the stored set is a function of the versions received,
  not their arrival order; `winner` over that set is then a function of the set and the admitted
  statements. `merge::tests::fold_is_order_independent` drives every arrival permutation through
  `upsert_own` then `winner` (equal hlc, a deletion, an older version by the same author, an
  inadmissible newer version, a canonical-bytes tie).

## §storage

LOCAL tables, never replicated (DDL lands in `schema.rs` through I-DB-1):

| Table | Columns | Rule |
|---|---|---|
| `row_field_version` | tbl, rid, verse_id, field_key, author, hlc (int >= 0), value_json (option<string>), deleted (bool) | one row per (tbl, rid, field_key, author) |
| `row_field_clock` | tbl, rid, field_key, hlc (int >= 0), author | one row per (tbl, rid, field_key): the cached winner |

- `field_key` / `value_json` avoid the SurrealQL keywords `FIELD` and `VALUE`. `value_json` is the
  canonical JSON text: SurrealDB never coerces it (a GeoJSON point inside `transform` stays an object,
  ints stay ints), and the stored text is the canonical byte string the merge compares. NONE when
  deleted. hlc is stored as int (the `node_scene_clock` precedent); `stored_hlc` refuses values above
  `i64::MAX`. The legacy floor is hlc 0, so the column asserts `>= 0`, not `> 0`.
- `versions_write_sql(table, rid, verse_id, versions)` and `clock_write_sql(table, rid, winners)` return
  `(statements, bindings)` for the caller's own `BEGIN TRANSACTION ... COMMIT` script, in the
  `guards`/`bindings` style of `scene_publication`. Each write is followed by a verify statement that
  THROWs when a row did not land, because SurrealDB 3 can silently skip a create inside
  `UPSERT ... WHERE` (same reason as `CLOCK_WRITE`/`CLOCK_VERIFY` in `scene_publication/mod.rs`).
  `clock_write_sql` also prunes cached winners of keys absent from `winners`.
- Binding names are fixed (`fe_clock_versions`, `fe_clock_winners`, `fe_clock_tbl`, `fe_clock_rid`,
  `fe_clock_fields`): splice ONE clocked row per transaction. Each binding element carries its own
  tbl/rid, which is how the migration writes many rows in one transaction with the same statements.
- `versions_write_sql` writes what it is given: callers fold incoming versions through `upsert_own`
  against `load_versions` first, under the same fence that guards the row write.
- `load_rows_by_authors(db, verse_id, dids)` lists distinct (tbl, rid) for re-judge.
  `author_frontier(db, verse_id, did)` = max positive hlc over the author's `row_field_version` rows in
  the verse and the `node_scene_clock` rows it published on that verse's nodes (spec §3.7).

Required indexes (I-DB-1): UNIQUE `(tbl, rid, field_key, author)` on `row_field_version`, plus
`(verse_id, author)` for re-judge and the frontier; UNIQUE `(tbl, rid, field_key)` on `row_field_clock`,
plus `(tbl, rid)`.

## §admission

`judge::admissible` (CLOCK-05) is true when the author is the verse owner, the version is the legacy
floor (`FieldVersion::is_legacy_floor`), or `fe_policy` `held_at(owner, admitted, author, scope,
level(key), hlc)` holds. There is no conjunction with the author's current role. `VerseView` holds the
owner and admitted statements so a row's keys are judged against one snapshot. Inbound admission of a
clocked row is only a membership floor; past it every version is stored and an inadmissible one simply
never wins. Levels: verse governance keys (`default_access`, `ts_*`) need Manager at verse scope, petal
`hexon_manifest` needs Manager at petal scope, every other key Editor at the row's deepest scope.

## §rejudge

`rejudge::rejudge_authors` (CLOCK-10) recomputes winners of every row `load_rows_by_authors` returns
(cap 4096 rows per call, the remainder warned and handled by a later call) and rewrites the row and its
`row_field_clock` cache in one verified transaction. It is a pure function of stored versions and
admitted statements, so it converges on every peer. Triggers are listed in spec §3.8.

## §legacy-migration

`migrate::seed_legacy_floor(db)` seeds every verse, fractal, petal and node row that has no
`row_field_clock` entry: one version per key from `keys_of`, at the legacy floor (author `''`, hlc 0),
or at `(hlc, publisher)` when the node has a `node_scene_clock` row. Pages of 1024 rows by id; writes
are flushed in transactions of about 2 MiB of bindings, each guarded against a clocked write that landed
since the page was read and verified like any other write. Idempotent: a seeded row has clock entries
and is skipped next time. Rows whose verse cannot be resolved are skipped and counted in one warning;
rows with no keys are not seeded. It returns the number of rows seeded.

## §compat

Flag day (user decision 2026-11-13): every peer upgrades together, as for the iroh 1.x port. There is
no dual-format writing, no version negotiation and no compatibility window. Pre-upgrade peers deny node
rows with the unknown `_fe_clock` column and cannot MERGE it into SCHEMAFULL hierarchy tables. Existing
local data is covered by the legacy floor.

Residual risks are accepted and documented in spec §4: a revoked author can still backdate a write at
or below the revoker's frontier on a key nobody wrote later; edits the revoker had not received at
revoke time are dropped; more than 8 grant breaks on one statement key drop the oldest window.
