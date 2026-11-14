---
type: Review
gate: R-NET-6
round: 4
upstream: [NET-11, NET-30, NET-31, NET-32, NET-33]
carries: NET-36 (Gate F3, code half)
security_review: true
---

# R-NET-6: adversarial review-and-fix (NET-11, NET-30..33) plus F3 code half

Read against `git diff 7af0a144` of every owned file, prompted to refute. No cargo was run (round rule); all fixes are uncompiled.

## Findings

R6-01 FIXED - fe-sync/src/tileset_transfer.rs:713 - the rate cap was per call of `download`: every confirmed install spawns its own download task (sync_thread.rs `ConfirmTilesetDownload` arm), so N concurrent downloads ran at N x `FE_MAX_DOWNLOAD_RATE`. The pacer is now one `Mutex<DownloadPacer>` per service (`pace()`, lock released before the sleep); `with_limits` resets it.
R6-02 FIXED - fe-sync/tests/tileset_transfer_test.rs:136 - the rate leg now downloads two tilesets at once through one capped service and asserts the SHARED floor (both byte counts / rate); with per-download pacers it would finish in about half the floor and go red.
R6-03 DEFERRED (FetchStrategy rebase, ledger residual PAS-P5-P2P-POLICY) - fe-sync/src/tileset_transfer.rs:1177 - `fetch_chunk` (streamed per-tile reads) is not paced; pacing after the fetch would only delay the tile's return. Up to `MAX_CONCURRENT_CHUNK_FETCHES` chunks also burst while the pacer sleeps; the average still holds.
R6-04 KEPT (same contract as fe-hexon `P2pConfig::from_env`) - fe-sync/src/tileset_transfer.rs:110 - an unparseable `FE_MAX_DOWNLOAD_RATE` is ignored silently and `0` means unlimited.
R6-05 HANDED-OFF (I-SYNC-2 / fe-sync Cargo owner) - fe-sync/src/net_diagnostics.rs:113 - `reachability_self_test` and `IrohNetReportProbe` are behind `#[cfg(feature = "net-report")]`, but fe-sync/Cargo.toml declares no `net-report` feature, so the self-test is dead code: the verdict is permanently `pending`, Settings always says "Waiting for the reachability check." and the desktop relay can never start (fails closed). Fix: `net-report = []` plus `default = ["net-report"]` in fe-sync/Cargo.toml (iroh `unstable-net-report` is already on unconditionally).
R6-06 FIXED - fe-sync/src/sync_thread.rs:1106 - the self-test re-ran only after something had requested diagnostics, so the verdict never reached the Settings reason line unless Peer Debug was opened. The 30 s tick now refreshes the (cheap, cached-report) verdict every time and emits one redacted snapshot whenever the verdict changes.
R6-07 FIXED - fe-ui/src/settings.rs:165 - nothing fed `record_self_test_verdict` after startup. New Bevy system `mirror_self_test_verdict` copies a changed `NetworkDiagnosticsState` verdict into it (change-gated, no per-frame allocation). Registration HANDED-OFF (I-UI-3, plugin.rs).
R6-08 FIXED - fractalengine/src/relay_serve_bridge.rs:49 - `maybe_start` was documented as "call before the sync thread starts", but the self-test verdict only exists after the sync thread runs, so the gate could never return `Start`. Split into `register_settings` (pre-sync globals) and `maybe_start` (once a verdict and dial-back result exist). Pinned seam name `maybe_start` kept.
R6-09 FIXED - fractalengine/src/relay_serve_bridge.rs:97 - `runtime.block_on` panics when called inside an async context and this binary aborts on panic; `start` now refuses (warn) when a runtime is current.
R6-10 HANDED-OFF (I-BIN-2, then a next-round NET slice) - fractalengine/src/relay_serve_bridge.rs:21 - the dial-back precondition is circular: the probe fetches `https://host:port/ping` of the relay, which is not listening yet. Suggested resolution: start on feature + consent + `public` verdict + `FE_RELAY_SERVE=1`, request the dial-back, drop the handle if it fails; AC-16 already blocks advertising an unproven relay.
R6-11 HANDED-OFF (owner of fe-sync/src/relay_server.rs, then sync_thread.rs) - fe-sync/src/sync_thread.rs:1850 - `install_gate` after the sync thread started (now the normal desktop path, R6-08) gets no admission snapshot until the next role or `__peers` change, so members are denied until then (fail closed). Fix: `pub fn gate_installed() -> bool` in relay_server.rs; `RelayAdmissionPusher::refresh` re-pushes when it flips to true.
R6-12 FIXED - fe-runtime/src/net_diagnostics.rs:28, fe-sync/src/net_diagnostics.rs:150, fe-api/src/network.rs:96 - the hazard-B warning embedded every open verse id in `downgrade_hint`, and `confine_to_claims` filtered only peers, so a token scoped to verse A learned the ids of verses B, C this node is in. The verses are now a list (`NodeDiagnostics.unadvertised_home_verses`, `#[serde(default)]`) that the API confines to the caller's Viewer+ verses; the hint text carries no ids.
R6-13 FIXED - fe-runtime/src/net_diagnostics.rs:81, fe-sync/src/net_diagnostics.rs:53, fe-sync/src/sync_thread.rs:2117, fe-api/src/network.rs:106 - "no IPs by default" (AC-14, P12) leaked through `relay_host` and `home_relay`: a peer-run relay (NET-30 desktop mode) is commonly an IP-literal URL, so its host is that peer's address. New `redact_host` / `redact_relay_url` hide IP-literal hosts unless `detail`; applied at the source and again at the API seam.
R6-14 FIXED - fe-api/src/network.rs:83 - each request judged the scope once per (peer, verse) pair, and `require_role_and_scope` warn-logs every denial: a 2/s poll by a narrowly scoped token became a log flood. Each verse is now judged once per request.
R6-15 FIXED - fe-api/src/network.rs (test), fe-ui/src/dialogs/peer_debug.rs (test), fe-sync/tests/peer_relay_e2e_test.rs:216 - journeys extended for R6-12/13: the API fixture carries an IP-literal home relay, an IP-literal peer relay and a two-verse hazard list, and asserts the redacted forms and that `v2` never leaves the API; the e2e AC-13 leg compares against the redacted host.
R6-16 FIXED - fe-ui/src/dialogs/peer_debug.rs:148 - Peer Debug renders the hazard-B line from the list (count only, no ids); the egui row-striping call was removed because the leak scan matches it as a substring (false positive; the scan needs a word boundary).
R6-17 HANDED-OFF (I-BATCH-2) - fe-sync/tests/peer_relay_e2e_test.rs - a name filter `peer_relay_e2e` reaches none of its four tests (integration test names carry no file prefix). Register `cargo test -p fe-sync --features relay-server --test peer_relay_e2e_test`.
R6-18 HANDED-OFF (I-BATCH-2) - fe-runtime/src/net_diagnostics.rs - the R8 fe-runtime `--lib` line lists modules by name and omits `net_diagnostics`; append it.
R6-19 KEPT (AC-5 is a 5 s requirement) - fe-sync/tests/peer_relay_e2e_test.rs:94 - `evict_until_gone` asserts a wall-clock upper bound and drives `disconnect` itself (`EmbeddedRelay::adopt_test_server` is `cfg(test)` only), so AC-5 proves the gate's revoke and the 5 s budget, not the production reconciler loop.
R6-20 DEFERRED (next NET round) - fe-sync/tests/peer_relay_e2e_test.rs:325 - AC-6 "unhealthy relay never inserted / adopted relay removed when unhealthy" and AC-9 "A to C still connects via C's home relay" are not exercised end to end; AC-9 covers only the snapshot builder.
R6-21 HANDED-OFF (I-API-3) - fe-api/src/network.rs:146 - the MCP tool must apply the same Viewer floor and `NETWORK_DETAIL_GATE.check_rate` as the REST handler before calling `collect_scoped_diagnostics`; and startup should call `NETWORK_DETAIL_GATE.opted_in_from_env()` once so the loud enabled-warning is ever printed (`detail_enabled_from_env` is deliberately silent per request).
R6-22 KEPT (operator's own address, needed for FE_SYNC_BOOTSTRAP copy) - fe-ui/src/dialogs/peer_debug.rs:42 - Peer Debug still prints this node's own address JSON (pre-existing line).
R6-23 KEPT (fail closed by design) - fractalengine/src/relay_serve_bridge.rs:103 - the consent toggle alone never binds: `from_env_desktop` also requires `FE_RELAY_SERVE=1` and a cert mode (AC-1 tripwire).
R6-24 KEPT (cosmetic) - fe-sync/src/sync_thread.rs:2120 - `relay_server_state` reports `available` when the feature is built, not whether a relay is serving.
SEC-INBOX: no entry names an owned file. Entry 7 (relay_server.rs breaker closes the listener on non-member denials) is not in OWNS; the desktop mode inherits it once started - its owner gate disposes of it.

## F3 code half (NET-36) over NET-13..NET-33

- Tripwires, grepped mechanically: gate never reads `default_access`/`open_to_all` (sharding.rs:65, relay_server.rs:1407 comment only; sync_thread hits are tests); no `AllowAll`; no plaintext cert mode (relay_server.rs:226 refuses); relay off unless feature AND `FE_RELAY_SERVE=1` (relay_server.rs:339); no `RelayAccessToken` on advertised relays; `log_client_ips` false in tests and config; signed-magic frames never re-parsed as legacy (e2e AC-11 compat assertion).
- Default-off (standing NO-GO): `serve_relay` false and `act_as_prober` false (settings.rs:134-135, dialback_responder.rs:30), `relay-server` is not a default feature of fe-sync (Cargo.toml:11) and fractalengine has none yet; no relay seeding default changed by these slices.
- Wording rule: the commercial-wording grep plus the round's reserved list over every owned file and the added sync_thread lines: clean after R6-16.
- AC status (code, not spec text): AC-1/2 enforced in relay_server config parse; AC-3/4/11/13 have e2e journeys; AC-5 partial (R6-19); AC-6/9 partial (R6-20); AC-14 complete after R6-12/13 (MCP half pending I-API-3); AC-17 planner leg covered; AC-7/8/10/12/15/16 were reviewed by R-NET-2..R-NET-5 and not re-derived here.
- Pending evidence (user-ordered, not run here): the P15 e2e run log, `cargo deny check` over the iroh-relay/server tree, and the full sweep. The F3 verdict file `conductor/tracks/peer_networking_support_20261031/review-fable-f3.md` is outside this gate's OWNS (NEEDS line in the result).

Verdict: PASS-WITH-FIXES (relay start remains unreachable until R6-05, R6-10 and R6-11 land; fails closed).
