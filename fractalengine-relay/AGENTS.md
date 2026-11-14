# AGENTS.md — fractalengine-relay

Operator binary that can host the members-only embedded relay. Module notes for the shared relay code live in `fe-sync/src/AGENTS.md` §peer-relay-server and §relay-server.

## embedded-relay

- The relay runtime is built only when `FE_RELAY_SERVE=1` is set and the binary carries the `relay-server` feature. With the feature, an absent opt-in returns `None` and nothing binds (`fractalengine-relay/src/relay_serve.rs:48-56`). Without the feature, a set opt-in only logs a warning and nothing binds (`relay_serve.rs:58-70`).
- The membership gate is installed before the sync thread starts: `relay_serve::maybe_spawn_embedded_relay` runs at `fractalengine-relay/src/main.rs:272`, and `fe_sync::spawn_sync_thread` runs after it at `main.rs:282`. The sync thread's snapshot pushes therefore reach a gate that already exists, and a late gate receives the buffered snapshot (`fe-sync/src/AGENTS.md` §relay-server).
- A refused config is an error, not a silent no-op. `spawn_from_config` names the reason (`embedded relay refused to start: <reason>`), and the `?` at `main.rs:272` returns it from `main`, so the process exits non-zero and binds no listener (`relay_serve.rs:27-40`, `:73-80`).
- A second membership gate in one process is refused (`relay_serve.rs`, "a relay membership gate is already installed"). The desktop binary uses the same gate type, so both cannot run in one process.
- Hosted verses come from `load_hosted_verse_ids` (`fractalengine-relay/src/main.rs:271`) and are the set the relay serves under `FE_RELAY_SERVE_VERSES=hosted` (`fe-sync/src/AGENTS.md` §peer-relay-server).
