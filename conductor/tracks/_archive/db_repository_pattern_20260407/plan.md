# Plan — Database Repository Pattern

## Phase 1: Build Green _(blocking)_

- [ ] 1.1 Resolve `SurrealValue` trait bound on `Table` (derive in macro or drop from trait + adjust `find_all`)
- [ ] 1.2 Fix compile errors in `lib.rs` from partial Repo migration (3 handlers already migrated, rest still reference old `schema::DEFINE_*` via rbac)
- [ ] 1.3 `cargo build -p fe-database` passes
- [ ] 1.4 `cargo test -p fe-database` — all schema + migration tests pass

## Phase 2: Migrate CRUD

- [ ] 2.1 `queries.rs` → `Repo::create` for petal, room, model
- [ ] 2.2 `space_manager.rs` → `Repo::find_where` / `merge_by_id` for reads + updates
- [ ] 2.3 `op_log.rs` → `Repo::<OpLog>::create`
- [ ] 2.4 Remaining `lib.rs` handlers → Repo calls (seed_assets, join_verse, create_petal_handler, import_gltf, load_hierarchy)
- [ ] 2.5 `rbac.rs` `get_role` → `Repo::<Role>::find_where`

## Phase 3: Consolidate Types

- [ ] 3.1 Merge `verse::VerseMember` ↔ `schema::VerseMemberRow` into one type
- [ ] 3.2 Return typed structs from `space_manager` instead of `serde_json::Value`
- [ ] 3.3 Remove any remaining `schema::DEFINE_*` references across codebase
