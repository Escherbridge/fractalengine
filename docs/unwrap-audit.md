# Unwrap / Expect Audit

| | |
|---|---|
| Date | 2026-10-11 |
| Commit | `7d40a4e` plus the uncommitted 2026-10-11 completeness-campaign batches |
| Command | `scripts/unwrap-census.sh` (`scripts/unwrap-census.ps1` on Windows) |
| Underlying | `cargo clippy --workspace --lib --bins --message-format=short -- -A clippy::all -W clippy::unwrap_used -W clippy::expect_used` |
| Total | **194 production sites** (86 `unwrap`, 108 `expect`) |

**Scope.** Only `lib` and `bin` targets are checked, so `#[cfg(test)]` modules, `tests/`, examples
and benches are excluded by construction. Code behind non-default features is not compiled and
not counted. Re-run the script for the live list; this document holds the categorised register
and the follow-ups. The script is read-only and writes only to stdout or the path it is given.

**Release builds abort on panic.** The root `Cargo.toml` `[profile.release]` sets `panic = "abort"`.
That makes every site below a process-abort in a release build, not a thread unwind. It also
means mutex poisoning cannot happen in release: poisoning needs a panic to unwind while the lock
is held, and abort never unwinds. Under `cargo test` and dev builds, panics do unwind.

## Per crate

| Crate | Sites | | Crate | Sites |
|---|---:|---|---|---:|
| fe-plugin-test | 27 | | fe-sim | 8 |
| fe-plugin | 25 | | fractalengine-relay | 5 |
| fe-canonical-log | 24 | | fe-runtime | 5 |
| fe-test-harness | 23 | | fractalengine | 4 |
| fe-database | 22 | | fe-format | 4 |
| fe-api | 18 | | fe-ui, fe-query, fe-hexon | 2 each |
| fe-terrain | 11 | | fe-webview, fe-network, fe-identity | 1 each |
| fe-sync | 9 | | | |

## Categories

| Category | Sites | Verdict |
|---|---:|---|
| Test-support code compiled as a library | 71 | Allowed |
| `lock()` / `read()` / `write()` poisoning | 60 | Allowed; unreachable in release |
| Startup / thread bootstrap | 18 | Allowed (intentional-panic register) |
| Guarded invariants | 41 | Allowed; each is preceded by the check that makes it infallible |
| Clock before `UNIX_EPOCH` | 2 | Allowed; needs a pre-1970 system clock |
| **Reachable from untrusted input** | **2** | **Follow-up F-1** |

**Test-support (71).** These are fe-plugin-test (27; it is a published test-utility crate),
fe-test-harness (23), `fe-canonical-log/src/wire/test_support.rs` (13; an unconditional `pub mod`)
and the fe-sim lab binary (8). None of them is reachable from the `fractalengine` or `fe-relay`
product paths.

**Lock poisoning (60).** The sites are fe-plugin (20: lifecycle registry and Rhai host/storage
context), fe-api (15: `hexon.rs` registry ×9 and the `canonical_ws/state.rs` verification cache ×6),
fe-database (12: the `op_log.rs` HLC/wall-clock statics, plus the canon_log storage-fault and
precondition-failure side channels), fe-terrain
(8: `tiles/registry.rs` `RwLock`) and fe-sync (5). See the release note above for why poisoning
cannot happen in release builds.

**Guarded invariants (41).** Each one follows a length, `is_some` or `is_array` check, a
just-assigned value, or an infallible serde of a plain struct. Examples:
`fe-format/src/signature.rs:134` (length checked to be 32 just above),
`fe-database/src/handlers/crud.rs:947` (`is_array()` checked), `fe-runtime/src/app.rs:438-508`
(position comes from the same queue), and `fe-canonical-log/src/wire/frame.rs:224,242`
("checked present above").

## Intentional-panic register

| Site | Justification |
|---|---|
| ~~`fe-database/src/lib.rs` `.expect("SurrealDB init")`~~ | **REMOVED** in `d3c5fab` (2026-05-02); DB init failures now surface as errors. Kept here only as a tombstone. |
| Tokio runtime builders: `fe-api/src/lib.rs:135`, `fe-network/src/lib.rs:22`, `fe-plugin/src/lib.rs:206`, `fe-sync/src/sync_thread.rs:411`, `fractalengine/src/main.rs:311,338,440`, `fractalengine-relay/src/main.rs:240,306,543` | Each worker thread cannot run without its runtime. Failure means the OS refused threads or an epoll fd. |
| `fe-plugin/src/lib.rs:203` thread spawn | Same as above, for the plugin-host thread. |
| `fractalengine/src/main.rs:92`, `fractalengine-relay/src/main.rs:82` keypair from seed | Re-derives a keypair from a seed that was just produced. |
| `fractalengine-relay/src/main.rs:44` `FsBlobStore::open_default()` | The relay cannot serve without its blob store. |
| `fe-api/src/lib.rs:204,206` `local_addr` / `axum::serve` | The API thread's terminal state. |
| `fe-database/src/op_log.rs:109` HLC not initialised | A startup-ordering contract: `init_hlc()` runs during DB startup. |
| `fe-plugin/src/wasm/mod.rs:196` `WasmEngine::default` | `Default` cannot return an error. Callers that need fallibility use `WasmEngine::new()`. |

## Follow-ups (not fixed here)

1. **F-1 (reachable):** `fe-plugin/src/wasm/host_imports.rs:379,406`. Here
   `caller.get_export("memory")?.into_memory().unwrap()` panics if a guest exports a non-memory
   item named `memory`. That is guest-controlled, so a malicious plugin aborts the host. Return the
   same "no memory" fallback instead. The plugin system is dormant today (no shipped binary loads
   plugins), so this is not live.
2. **F-2 (top offender):** `fe-api/src/hexon.rs` has 9 `registry.lock().unwrap()` sites. These are
   harmless under `panic = "abort"`, but in dev and test a single handler panic poisons the
   registry for every later request. Map them to a 500 or use `parking_lot`.
3. **F-3 (top offender):** fe-plugin lifecycle and Rhai host APIs have 20 lock unwraps. Fold them
   into F-1's hardening pass when the plugin system is activated.
4. **F-4:** `fe-canonical-log/src/wire/test_support.rs` is an unconditional `pub mod`. Gate it
   behind `#[cfg(any(test, feature = "test-support"))]` so it drops out of production builds.
