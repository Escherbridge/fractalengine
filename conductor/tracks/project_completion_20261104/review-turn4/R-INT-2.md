---
type: Review
---

# R-INT-2: adversarial review of I-API-1, I-SYNC-1, I-BIN-1 (security review included)

Diffs reviewed: `5548e28d` (I-API-1), `3a634a49` (I-SYNC-1) and `f0c266e5` (I-BIN-1), read against the pinned seams in turn4-plan.md §4 and the current upstream modules. Security inbox: entries 5 and 6 name `fe-api/src/petal_snapshot.rs` and triaged entry 1 names `fe-api/src/relay_advertise.rs`. Neither file belongs to this gate, and no inbox entry names a file this gate owns. No cargo was run.

## Findings

- FIXED (correctness, compile): `fe-sync/src/lib.rs:22,25` did not declare `relay_reconcile` or `relay_trust`. `endpoint.rs:292`, `relay_advertise.rs:16` and `sync_thread.rs:45-46` already use them, so fe-sync could not build at this barrier. Both modules are now declared, and `RelayAdvertisePolicy` is re-exported at `lib.rs:51` (pinned seam). The mod list is now alphabetical. `dialback_prober` (needs NET-19's reqwest dep) and `dialback_responder` (no file yet) stay with I-SYNC-2, which skips the lines that are already present.
- HANDED-OFF (orchestrator): `fe-runtime/src/dialback.rs` and `fe-sync/src/reachability.rs` are untracked. HEAD's `fe-runtime/src/lib.rs:8` and `fe-sync/src/lib.rs` declare them, and `fe-api/src/relay_advertise.rs:13` and `fe-sync/src/net_diagnostics.rs:4` use them, so a clean checkout of HEAD does not compile. Commit both files (NET-18 output).
- FIXED (security, hardening): `fe-api/src/server.rs:515` had no body cap on the relay advertisement POST, so the 2 MiB axum default applied. It now has `DefaultBodyLimit::max(32 * 1024)`, which still holds the largest valid body: two ≤2 KiB URLs even when fully JSON-escaped, a ≤256-byte note and one signature.
- KEPT (verified): both relay routes (`server.rs:515-516`) sit inside the `authenticated` router, under the `auth_middleware` layer at `server.rs:536-539`. The handlers check Manager+ on `VERSE#{id}` first and refuse when no signer is configured.
- KEPT (verified): `/api/v1/debug/portal/ipc-post` (`server.rs:396`) is in the authenticated router with the pinned 256-byte limit. Its handler runs `QA_GATE.admit` before parsing anything (`qa_portal.rs:425`).
- KEPT (verified): the usage layer is outermost (`fe-api/src/lib.rs:268-272`), so it inserts `UsageCallerSlot` before `auth_middleware` fills it (`auth.rs:36-38`, which only fills it after `authenticate` succeeds). It is a `OnceLock`, so the first fill wins and a later layer cannot overwrite it. `MatchedPath` is visible because `Router::layer` wraps each route endpoint. Events carry only the route template or `<unmatched>`.
- KEPT (by design, OD-7): the metrics listener (`lib.rs:278-286`) serves an unauthenticated `/metrics` only when `FE_API_METRICS_BIND` is set, and `network_ops.rs` warns when that bind is not loopback. A failed bind logs a warning and the API keeps running. It shares the API stop signal.
- KEPT (minor): `record_usage` spawns one detached task per request (`usage_middleware.rs:70`). The sink is a no-op when metering is off and R-NET-1 bounded the writer. Revisit if request rates reach thousands per second.
- KEPT (verified, signing identity): `advertisement_signer` is rebuilt from `node_kp.seed_bytes()` (`fractalengine/src/main.rs:270`, `fractalengine-relay/src/main.rs:128`). That is the same seed as `to_iroh_secret()`, which `docs_engine.rs:823` imports as the iroh-docs author, so the signed `advertised_by` matches the SignedEntry author that the book checks.
- KEPT (verified): every `ApiConfig` literal sets `network_ops`: both binaries, `fe-api/tests/api_harness_smoke.rs:77` and `fe-api/tests/token_lifecycle_test.rs` (`Default::default()`).
- KEPT (verified, fail closed): the licence wiring (`plugin_bridge.rs:4457-4461`, `:3418-3434`) builds the `LicenceTokens` client only with `cfg(entitlement)` and licence keys present. An unreadable issuers file leaves it `None`, and the request then answers `not-available`. Without pinned issuers, `LicenceTokens::request` sends nothing. Verified tokens go only to the governance store that row 6 reads (`licence_client.rs:83`). The blocking HTTP runs on the `extension-ops` thread (`ops_drain.rs:2126`), never on the Bevy frame.
- KEPT (verified): one `RegistryOffers` instance is shared by the binding worker (`LocalFacts.registry`) and the ops worker (`plugin_bridge.rs:4782`). `registry_offers_changed` re-dirties the verse, or every verse when the id is empty (`:3436`). Refresh runs on the ops thread.
- KEPT (minor): `feed_blob_mirror` (`plugin_bridge.rs:4716`) stats one blob file for each committed `GltfImported` on the frame. That is one small syscall per import, not per frame. The queue drops and counts when full, and it drains while unbuilt so nothing replays.
- KEPT (verified): the subscriber init (`fractalengine/src/main.rs:118-123`) uses a pair of optional layers. The text output is byte-identical to before, and the QaLogLayer is kept. In the relay (`fractalengine-relay/src/main.rs:47-55`) the text arm is unchanged. The workspace `tracing-subscriber` has the `json` feature (`Cargo.toml:116`).
- KEPT (verified): the fe-sync features are correct against the registry sources. iroh 1.3.0 has `unstable-net-report` and `test-utils`, and iroh-relay 1.3.0 has `server` (`fe-sync/Cargo.toml:11,29,45`). The workspace `iroh-relay` is `default-features = false` with the same minor (`Cargo.toml:85`). Its tls/metrics features unify through iroh itself.
- HANDED-OFF (I-BIN-2): `fractalengine-relay/src/relay_serve.rs` uses `fe_sync::relay_server`, but `main.rs` does not declare it yet and `fractalengine-relay/Cargo.toml` has no `relay-server = ["fe-sync/relay-server"]` feature. Add both when NET-24/NET-30 are wired, with the `cfg` on the module.
- DEFERRED (trigger: next `cargo deny check`): `iroh-relay/server` pulls in rcgen, tokio-rustls-acme, clap and dashmap. This is feature-gated and off by default, but `deny.toml` licence and ban coverage has not been checked against them.
- N/A: none of these slices touched `DbCommand`/`DbResult` arms or `fe-api/src/mcp/tools.rs`. The leak scan of every added line in the three diffs and in this gate's edits is clean.

## QA

- QA api: POST /api/v1/verses/{v}/relays with a 40 KiB JSON body and a Manager token -> 413, and no `__relays` row appears.
- QA api: POST /api/v1/verses/{v}/relays without a bearer token -> 401, and the usage log line for it has no caller_did.

Verdict: PASS WITH FIXES. Two defects are fixed (missing fe-sync module declarations, an uncapped relay POST body). One blocker is handed to the orchestrator: the untracked `dialback.rs` and `reachability.rs` must be committed. The security-sensitive wiring (auth layering, signer identity, licence fail-closed) holds.
