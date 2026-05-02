# Specification: Fix Silent Channel Send Error Swallowing

## Problem

The codebase uses `.ok()` on crossbeam channel sends, which silently discards send failures. Per `AGENTS.md` anti-pattern #6:

> Do not silently swallow channel send errors. Bare `.ok()` on a send hides DB thread crashes. Always `warn!()` on send failure.

This makes debugging impossible when the DB thread, sync thread, or API thread panics or exits — the sender just fails silently and the UI appears to work but no persistence or P2P sync occurs.

## Affected Locations

### `fe-ui/src/navigation_manager.rs`
- `open_replica()` — `sync.0.send(...).ok()` on `OpenVerseReplica`
- `handle_verse_replica_lifecycle` — `sync.0.send(...).ok()` on `CloseVerseReplica`

### `fe-ui/src/verse_manager.rs` — `apply_db_results` system
- `DbResult::Seeded` → `db_sender.0.send(DbCommand::LoadHierarchy).ok()`
- `DbResult::DatabaseReset` → `db_sender.0.send(DbCommand::LoadHierarchy).ok()`
- `DbResult::VerseJoined` → `db_sender.0.send(DbCommand::LoadHierarchy).ok()`
- `DbResult::EntityRenamed` — no send, but check for pattern
- All `DbResult::ApiTokenRevoked` / `ApiTokenMinted` / etc. → `refresh_inspector_tokens` uses `.ok()`
- `refresh_inspector_tokens` helper → `.ok()` on `ListApiTokens` / `ListApiTokensByScope`

### `fe-ui/src/node_manager.rs` — `broadcast_transform` system
- `db_sender.0.send(DbCommand::UpdateNodeTransform).ok()` — actually has a custom `is_err()` check with `warn!`, which is the **correct** pattern.
- `sync.0.send(SyncCommand::UpdateNodeTransform).ok()` — uses `.ok()` without warning.

## Acceptance Criteria

1. Every crossbeam channel `.send()` in `fe-ui` either:
   - Has an explicit `if let Err(e) = ... { warn!(...) }` check, OR
   - Has a documented comment explaining why failure is truly ignorable (e.g., shutdown sequence).
2. `cargo clippy -- -D warnings -p fe-ui` passes.
3. No `.ok()` calls remain on channel sends in `fe-ui`.

## Scope

- `fe-ui/src/navigation_manager.rs`
- `fe-ui/src/verse_manager.rs`
- `fe-ui/src/node_manager.rs` (sync sender only; db sender already correct)
