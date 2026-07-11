# Database Repository Pattern — Typed CRUD & Schema Macro

**Track:** db_repository_pattern_20260407
**Stack:** Rust, SurrealDB 3.0
**Status:** In Progress

---

## Overview

Replace raw SurrealQL strings scattered across `fe-database` with:

1. A `define_table!` macro that generates both a Rust struct and SurrealQL DDL
   from a single source of truth.
2. A `Repo<T>` generic repository providing typed CRUD with reasonable defaults
   and an escape hatch for domain-specific queries.

Goals: eliminate raw SQL for standard CRUD, reduce `serde_json::Value` usage,
keep a clean override path for spatial / full-text / VERSION queries.

---

## What's Already Done (Phase 0)

| File | Status | Notes |
|------|--------|-------|
| `fe-database/src/repo.rs` | **Created** | `Table` trait + `Repo<T>` (create, find_by_id, find_all, find_where, merge_by_id, delete_all, delete_by_id, count, query_raw, create_raw, apply_schema) |
| `fe-database/src/schema.rs` | **Rewritten** | `define_table!` macro + all 10 tables (Petal, Room, Model, Role, OpLog, Verse, VerseMemberRow, Fractal, Node, Asset) + `apply_all()` + `ALL_TABLE_NAMES` |
| `fe-database/src/rbac.rs` | **Updated** | `apply_schema` delegates to `schema::apply_all` |
| `fe-database/src/admin.rs` | **Updated** | Uses `crate::repo::Db` type alias |
| `fe-database/src/lib.rs` | **Partially migrated** | `mod repo` added; `create_verse_handler`, `create_fractal_handler`, and seed helpers migrated to `Repo::<T>::create()` |

### Known issue

The linter added `surrealdb::SurrealValue` as a super-trait on `Table`. The
`define_table!` macro does **not** yet derive `SurrealValue` — this needs to be
resolved before `Repo::<T>::find_all()` (which calls `db.select()`) will
compile. Options:

- Derive `SurrealValue` in the macro (if it has a derive proc-macro).
- Remove the `SurrealValue` bound from `Table` and use `db.query("SELECT *…")`
  in `find_all` instead of `db.select()`, avoiding the trait requirement.

---

## Phase 1 — Resolve SurrealValue + Build Green

| # | Task | Files |
|---|------|-------|
| 1.1 | Resolve `SurrealValue` trait bound (derive or remove) | `repo.rs`, `schema.rs` |
| 1.2 | Fix all remaining compile errors from the partial migration | `lib.rs` |
| 1.3 | Run `cargo build -p fe-database` — zero errors | — |
| 1.4 | Run `cargo test -p fe-database` — schema tests pass | — |

---

## Phase 2 — Migrate Remaining CRUD to Repo

| # | Task | Files |
|---|------|-------|
| 2.1 | Migrate `queries.rs` (create_petal, create_room, place_model) to `Repo::create` | `queries.rs` |
| 2.2 | Migrate `space_manager.rs` reads/updates to `Repo::find_where`, `Repo::merge_by_id` | `space_manager.rs` |
| 2.3 | Migrate `op_log.rs` write to `Repo::<OpLog>::create` | `op_log.rs` |
| 2.4 | Migrate remaining `lib.rs` handlers (seed_assets, join_verse_by_invite, create_petal_handler, import_gltf_handler, load_hierarchy_handler) | `lib.rs` |
| 2.5 | Migrate `rbac.rs` get_role to `Repo::<Role>::find_where` | `rbac.rs` |

### Escape-hatch queries (keep as raw SurrealQL)

These use SurrealDB-specific features that the generic Repo can't express:

- `CREATE node CONTENT { position: <geometry<point>> … }` — geometry cast
- `SELECT * FROM petal WHERE tags CONTAINS $tag` — array containment
- `SELECT * FROM … WHERE string::lowercase(name) CONTAINS $q` — full-text search
- `SELECT * FROM $record VERSION d'…'` — temporal query
- `UPDATE model SET metadata.{key} = $value` — dynamic field path
- `SELECT … WHERE position INSIDE …` — spatial query
- `SELECT count() FROM … GROUP ALL` — aggregate

For these, callers use `db.query()` directly but deserialise into the typed
struct from `schema.rs` instead of `serde_json::Value`.

---

## Phase 3 — Clean Up Legacy Types

| # | Task | Notes |
|---|------|-------|
| 3.1 | Consolidate `verse::VerseMember` with `schema::VerseMemberRow` (pick one, re-export) | Avoid two structs for same table |
| 3.2 | Replace `serde_json::Value` returns in `space_manager.rs` with typed schema structs | e.g. `list_petals_by_tag` returns `Vec<Petal>` |
| 3.3 | Remove old `DEFINE_*` constants from any import sites (test files, examples) | Grep for `schema::DEFINE_` |

---

## Design Decisions

- **Macro vs hand-written**: The `define_table!` macro is purely declarative
  (no proc-macro crate) so it stays in the same file and adds zero compile-time
  deps. Trade-off: SurrealQL type strings are repeated as literals. Acceptable
  because they change rarely and are co-located with the Rust field types.

- **Static methods on Repo**: No instance state needed for basic CRUD, so
  `Repo::<Petal>::create(db, &item)` rather than `repo.create(item)`.  Domain
  repos (e.g. a `VerseService` with blob_store + repl_tx) can be added later
  on top of `Repo<T>`.

- **Geometry fields as `serde_json::Value`**: SurrealDB returns geometry as
  GeoJSON objects. Using `Value` avoids coupling to a specific geo crate and
  keeps the macro simple. Conversion to `[f32; 3]` happens in the handler layer.
