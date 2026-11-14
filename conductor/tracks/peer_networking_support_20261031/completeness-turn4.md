---
type: Review
track: peer_networking_support_20261031
critic: C-NET
round: 4
items: [NET-13, NET-14, NET-15, NET-16, NET-17, NET-18, NET-19, NET-20, NET-21, NET-22, NET-23, NET-24, NET-25, NET-26, NET-27, NET-28, NET-29, NET-30, NET-31, NET-32, NET-33, NET-34, NET-35, NET-36, NET-37, NET-38]
date: 2026-11-13
---

# C-NET completeness: peer_networking_support_20261031

Checked against HEAD `fd61b661` plus the working tree. No cargo was run. "HEAD" means committed. "Tree" means a file that is modified, staged or untracked and not yet committed.

## Commit state (read first)

HEAD does not build on its own. Several NET slices and the R-NET-6 fixes exist only in the working tree:

- Untracked: `fe-runtime/src/dialback.rs` (NET-18; declared at `fe-runtime/src/lib.rs:8`) and `fe-sync/src/reachability.rs` (NET-18; declared at `fe-sync/src/lib.rs:20`).
- Modified but not committed, and needed by committed callers: `fe-ui/src/dialogs/peer_debug.rs`. HEAD has the 3-argument `render_peer_debug_panel`, but `fe-ui/src/panels/mod.rs:394` calls it with 5 arguments. The same holds for `fractalengine/src/relay_serve_bridge.rs`: `fractalengine/src/main.rs:138` calls `register_settings`, which HEAD lacks.
- Modified, not committed (R-NET-6 fixes R6-01, R6-06, R6-07, R6-08, R6-09, R6-12, R6-13, R6-15, R6-16): `fe-sync/src/sync_thread.rs`, `fe-sync/src/net_diagnostics.rs`, `fe-sync/src/tileset_transfer.rs`, `fe-runtime/src/net_diagnostics.rs`, `fe-api/src/network.rs`, `fe-ui/src/settings.rs`, `fe-sync/tests/peer_relay_e2e_test.rs`, `fe-sync/tests/tileset_transfer_test.rs`, plus the untracked `conductor/tracks/p2p_asset_streaming_20260718/review-turn4/R-NET-6.md`.
- Staged, not committed (I-DOC-NET): root `AGENTS.md`, `fe-sync/src/AGENTS.md`, `fe-runtime/src/AGENTS.md`, `fe-api/AGENTS.md`, `fe-sync/tests/AGENTS.md`, `fe-ui/src/AGENTS.md`, `fe-ui/src/dialogs/AGENTS.md`.

Rows below that depend on these files say "tree".

## Spec requirements and plan tasks

| Item | Status | Evidence |
|---|---|---|
| F0 design and threat-model review | complete | `review-fable-design.md`; plan §2 F0 ticked |
| P1 signed gossip frame v1 + replay guard (NET-13) | complete | `fe-sync/src/gossip_auth.rs:14` domain `fe-gossip-v1\0`, `:131` `SigningMode`, `:166` `SignedGossipFrame`, `:425` `ReplayGuard`, `:441` `starting_at` (R-NET-5 N5-02) |
| P2 strict URL policy + signed v1 body (NET-14) | complete | `fe-runtime/src/relay_advertisement.rs:49` domain `fe-relay-adv-v1\0`, `:58` 60 s skew, `:388,:402` `RelayUrlPolicy::check`, `:487` `sign_row`, `:505-536` `verify_row_at` with `verify_strict` (`:518`) and the issued-at check |
| P2 coverage evaluator, bounded selection, peer-address filter (NET-15) | complete (spec text deviates) | `fe-sync/src/relay_trust.rs:38` `select_bounded` (oldest-first, see N3-08), journey `:95` |
| P2 reconciler tiers Configured/Default/Advertised (NET-16) | complete | `fe-sync/src/relay_reconcile.rs:412` configured never removed (property), `:582` Default absent only while a covered relay is home |
| P3 embedded members-only relay server (NET-17) | partial | `fe-sync/src/relay_server.rs:679` gate, `:936` `AccessControl`, `:1005` `EmbeddedRelay`, `:1155` revoke; journeys `:1404`, `:1453`. Gaps: SEC-INBOX 7 (breaker closes the listener on non-member denials, `:852`, `:1254-1265`); a gate installed after the sync thread starts gets no snapshot until membership next changes (`:1292-1305`, R6-11 / R-INT-6); `ServedVerses::AllHosted` serves every verse in the snapshot (`:146-149`, SEC-INBOX 15 residual) |
| P4 reachability classifier + signed `DialbackRecord` (NET-18) | partial (tree) | `fe-sync/src/reachability.rs:93` `classify`, test `:215`; `fe-runtime/src/dialback.rs:19,109`. Both files untracked. The `net-report` feature they gate on is undeclared in `fe-sync/Cargo.toml:10-11`, so the real probe is compiled out (R6-05) |
| P4 SSRF-pinned HTTPS prober (NET-19) | partial | `fe-sync/src/dialback_prober.rs:148` `resolve_public`, `:180` pinned, `:207` 5 s HTTP timeout; test `:286`. Gap: SEC-INBOX 14. `SystemResolver::resolve` (`:67`) calls `lookup_host` with no timeout, so a slow resolver holds a probe permit (`dialback_responder.rs:163-172`) |
| P5 diagnostics model, connection kind, redaction (NET-20) | complete (redaction fix in tree) | `fe-sync/src/net_diagnostics.rs:27-45` kind from active addresses, `:65` hint; `fe-runtime/src/net_diagnostics.rs:28,81,93` (`unadvertised_home_verses`, `redact_host`, `redact_relay_url`; tree, R6-12/13) |
| P6 admission snapshot pushed on role or `__peers` change (NET-21) | complete | `fe-sync/src/sharding.rs:66` `relay_admission_snapshot`, `:1800` members from explicit roles only |
| P6 hardened `__relays` consumer (NET-22) | complete (residual) | `fe-sync/src/relay_advertise.rs:19` skew, `:22,:25` caps; signed-first selection (N4-07); expiry tick `fe-sync/src/sharding.rs:1618`; Pinned insert check `fe-sync/src/sync_thread.rs:2343`. Residual N3-16/N4-25: DialOnly domains reach the allowlist unresolved (`sync_thread.rs:2076-2084`) |
| P6 peer-address filter at the single choke point (NET-23) | complete | `fe-sync/src/endpoint.rs:303` `set_relay_allowlist` re-filters stored peers; `:20` per-peer cap 8 |
| P7 fe-relay opt-in embedded relay (NET-24) | complete | `fractalengine-relay/src/relay_serve.rs:50-60`; called at `fractalengine-relay/src/main.rs:272`; feature `fractalengine-relay/Cargo.toml:49` |
| P8 REST hardening: expiry cap, URL policy, signed v1, dial-back (NET-25) | complete | `fe-api/src/relay_advertise.rs:115-117` strict policy before signing, `:121-127` expiry required and capped, `:132-141` dial-back required, prober must be a member; journey `:299` and `fe-api/tests/api_integration.rs:1069` |
| P9 sign every broadcast, verify in the pump, per-author latch (NET-26) | complete | `fe-sync/src/distributed_query.rs:865` `seal`, `:1568` `SignedGossipIngress`; latch field `fe-sync/src/sharding.rs:317`; one parser for legacy claims (N5-01) |
| P10 virtual transport carries signed frames byte-identically (NET-27) | complete (contract); run blocked-user | `fe-sync/src/virtual_transport.rs:113,131` |
| P13 diagnostics command, self-test, dial-back responder (NET-28, NET-29) | partial | `fe-sync/src/messages.rs:31`; `fe-sync/src/sync_thread.rs:1291`; `fe-sync/src/status.rs:35`; responder `fe-sync/src/dialback_responder.rs:150-180`. Gaps: the self-test is dead code (`fe-sync/src/net_diagnostics.rs:113` cfg `net-report`, undeclared; R6-05); SEC-INBOX 13 (`dialback_responder.rs:230-233` settles a waiter on the first record for the URL, `ok` or not) |
| P11 desktop relay mode: settings, consent, gated start (NET-30) | partial | defaults `fe-ui/src/settings.rs:133-135` (auto-home on, prober off, serve off); consent toggle `fe-ui/src/ui_shell/right_sidebar.rs:1206-1240`; provisional start `fractalengine/src/main.rs:1153-1168`. Cannot start: no `public` verdict without R6-05. `mirror_self_test_verdict` (`fe-ui/src/settings.rs:165`, tree) is registered nowhere. `register_settings` is tree-only |
| P12 Network panel in Peer Debug (NET-31) | partial (tree) | the 5-argument panel exists only in the working-tree `fe-ui/src/dialogs/peer_debug.rs:16-22`; HEAD has the old 3-argument function |
| P14 REST + MCP diagnostics, scope-confined and redacted (NET-32) | partial | REST `fe-api/src/network.rs:145`, mounted `fe-api/src/server.rs:530` inside the authenticated router; journey `fe-api/tests/api_integration.rs:1069`. Gaps: MCP `get_network_diagnostics` (`fe-api/src/mcp/tools.rs:1907`) has no `TOOLS` row in `fe-api/src/mcp/mod.rs`, so it is unreachable and has no Viewer floor; `network.rs:126` blocking `send` from an async handler; `fe-api/tests/mcp_integration.rs:968` builds `ApiHosts` without `network_diagnostics` (compile break); `NETWORK_DETAIL_GATE.opted_in_from_env()` is never called at startup (R6-21) |
| P15 end-to-end evidence (NET-33) | complete (code); run blocked-user | `fe-sync/tests/peer_relay_e2e_test.rs:173` (AC-3, AC-13), `:273` (AC-4, AC-5), `:325` (AC-6, AC-17), `:435` (AC-11); batch `conductor/test-batches.md:286` |
| Gate F1 (NET-34) | partial | security part done as `review-turn4/R-NET-3.md` and `R-NET-4.md` (PASS-WITH-FIXES); `review-fable-f1.md` not written |
| Gate F2 (NET-35) | partial | done as `review-turn4/R-NET-5.md` (PASS-WITH-FIXES, F2 section); `review-fable-f2.md` not written |
| Gate F3 code half (NET-36) | partial | done as `../p2p_asset_streaming_20260718/review-turn4/R-NET-6.md` (untracked); `review-fable-f3.md` not written |
| P16 docs (NET-37) | complete (root note staged) | `docs/relay-operations.md:187` §6 embedded relay; `SECURITY.md:93` S-1; root `AGENTS.md` crate note is staged, not committed |
| P17 close-out bookkeeping (NET-38) | partial | plan ticks and retro landed in `17dbb035` (`plan.md` §5); `conductor/tracks.md` row and metadata status/progress not updated this round |
| AC-1 no listener without the feature and `FE_RELAY_SERVE=1` | complete | `fe-sync/src/relay_server.rs:324-339`; `fractalengine-relay/src/relay_serve.rs:50-60` |
| AC-2 refuse to start without cert mode or verses, or with open access | complete | `fe-sync/src/relay_server.rs:188` `EmptyVerseSet`, `:191,:351` `OpenAccessRefused` |
| AC-3 two relay-only members sync through the in-process relay | complete (code) | `fe-sync/tests/peer_relay_e2e_test.rs:173` |
| AC-4 non-member denied, caps, fresh Viewer admitted | complete (code) | `peer_relay_e2e_test.rs:273`; `relay_server.rs:1404` |
| AC-5 revoke disconnects within 5 s, incl. the admit/register race | partial | `relay_server.rs:1453`; the e2e leg drives `disconnect` itself (`peer_relay_e2e_test.rs:91`, R6-19 KEPT) |
| AC-6 bad adverts refused at read time, never inserted | partial | book journey `peer_relay_e2e_test.rs:325`; the "unhealthy relay never inserted" leg is not end to end (R6-20) |
| AC-7 default adds nothing; local pin inserts tokenless | partial | planner and tripwire (`relay_reconcile.rs:582`, `relay_config.rs:355`); no live-endpoint leg (same gap as relay_operability AC-8) |
| AC-8 configured survives withdrawal; two-verse refcount | complete | `fe-sync/src/relay_reconcile.rs:412`, refcount `:544` |
| AC-9 hazard B warning path | partial | `fe-sync/src/sync_thread.rs:1961` `unadvertised_home_verses`; only the snapshot builder is tested (R6-20) |
| AC-10 non-allowlisted relay stripped from peer addresses | complete | `fe-sync/src/endpoint.rs:303`; journey asserts gain on allowlist and loss on revoke (N3-06) |
| AC-11 forged, replayed, stale, wrong-verse frames dropped | complete (code) | `peer_relay_e2e_test.rs:435`; `gossip_auth.rs` replay table |
| AC-12 compat vs latched vs require | complete | `fe-sync/src/gossip_auth.rs:140-146` mode parse; latch `sharding.rs:317`; admission journey (N5-01) |
| AC-13 Direct vs Relayed reported truthfully | complete (code) | `fe-sync/src/net_diagnostics.rs:42-45`; `peer_relay_e2e_test.rs:173` |
| AC-14 API and MCP confine and redact | partial | REST complete (`fe-api/src/network.rs:83-126`, R6-12/13 in tree); MCP unreachable (no `TOOLS` row) |
| AC-15 classifier table; prober refuses private, loopback, rebinding, redirects | complete (tree) | `reachability.rs:215` (untracked file); `dialback_prober.rs:286` |
| AC-16 no self-advert without a dial-back no older than 10 min | complete | `fe-api/src/relay_advertise.rs:132-141`; `fe-runtime/src/dialback.rs:10` 10-minute max age (untracked) |
| AC-17 coverage auto-home | complete (code) | `fe-sync/src/relay_trust.rs:95`; `relay_reconcile.rs:582`; `peer_relay_e2e_test.rs:325` |

## Scoper-excluded items (reported by class, not gaps)

- blocked-user: Gate W1..W5 (orchestrator serial check and tests, both feature sets).
- blocked-user: F3 second half (the P15 e2e run log, `cargo deny check` over `iroh-relay/server`, the full sweep; clippy obsolete).
- blocked-user: production embedded relay (ACME or operator cert on a real domain), desktop relay serving in production, OD-3 self-signed SPKI phase.
- blocked-user: iroh_1_0_upgrade orchestrator test sweep, the flag-day rollout before 2026-12-31, commit/push, fixture packaging, socket2 shim removal.

## Gate verdicts and hand-off delivery

- N3-08 / N4-08 (oldest-first `select_bounded` vs spec §4.4 item 7 "newest wins") were handed to this critic. Ruling: keep oldest-first. Newest-first lets any Manager displace the others by re-issuing at `now + 59 s`, and `fe-sync/src/relay_advertise.rs` tests pin oldest-first. The spec text is not in this critic's files: HANDED-OFF below.
- N3-09, N3-12, N3-18, N4-26, N4-31, R2-19 reached I-DOC-NET (staged). Signing domain, skew and local-only names at `fe-runtime/src/AGENTS.md:1377,1384`; `FE_RELAY_SERVE_*` names at `fe-sync/src/AGENTS.md` §peer-relay-server; prober cap and timeout at §reachability. Missing from the docs: `verify_strict`, the prober's `no_proxy`, the Custom-relay strip (N3-12), the book re-filter on allowlist change, and "Disabled makes adverts inert".
- N3-14, N4-27 reached I-SYNC-2 for the module declarations (`fe-sync/src/lib.rs:6-27`). The `net-report` feature (R6-05) did NOT land.
- N3-15, N4-30, R6-17, R6-18 reached I-BATCH-2 (`conductor/test-batches.md:284-288`).
- N3-16 (DialOnly domains unresolved) was handed to R-NET-5, which records no disposition. It is still open (`sync_thread.rs:2076-2084`).
- N3-17 / SEC-INBOX 7 was handed to R-NET-5, which did not own `relay_server.rs` and records nothing. Not closed by any gate.
- N3-19 (`review-fable-f1.md`): not written.
- N4-06 reached R-NET-3 (N3-01 FIXED).
- N4-12 (role-row unsent buffer is a global FIFO) was handed to the CLOCK group. No CLOCK review records it. It stays open on its trigger.
- N4-24 reached R-NET-5 (`dialback_responder.rs:162` `authorize`, and N5-05 for results).
- N4-25: residual, open.
- N4-28 reached I-RUNTIME (`fe-runtime/src/lib.rs:8`), but the module file is untracked.
- N4-29 reached I-BIN-2 (`fractalengine-relay/src/main.rs:272`).
- N5-08 reached R-NET-6 (R6-12, in tree).
- R6-10 reached I-BIN-2: provisional start, then dial-back confirmation (`fractalengine/src/main.rs:1153-1168`).
- R6-11 (re-push on gate install) was not done (`relay_server.rs:1301-1305` still drops a snapshot when no gate exists).
- R6-21: rate gate present in the MCP tool (`tools.rs:1909-1914`); startup `opted_in_from_env` absent; the Viewer floor depends on the missing `TOOLS` row.
- R-INT-6 hand-offs (MCP `TOOLS` row, `mcp_integration.rs` field, `network.rs` `try_send`, snapshot kept for a late gate, `mirror_self_test_verdict` registration, `AllHosted` scope, commit of the four files): none had landed at HEAD `fd61b661`.

SEC-INBOX dispositions verified:
- SEC-INBOX 1: closed by R-NET-1/3/4 (see the relay_operability file).
- SEC-INBOX 3: closed by R-NET-2.
- SEC-INBOX 10: closed by R-NET-5 (`fe-sync/src/distributed_query.rs:1658`).
- SEC-INBOX 7: OPEN, no gate disposition (`fe-sync/src/relay_server.rs:852`, `:1265`).
- SEC-INBOX 13: OPEN, no gate disposition. N5-05 limits answers to authorized members, but the first record still decides (`dialback_responder.rs:230-233`).
- SEC-INBOX 14: OPEN, no gate disposition (`dialback_prober.rs:67`).
- SEC-INBOX 15: fixed in part by R-INT-6 (`fractalengine/src/main.rs:1207` owned verses only); the `AllHosted` residual is open.

## Plan §9 defaults (as built)

- q8 OD-13 auto-home: ON (`fe-ui/src/settings.rs:133`; `fe-sync/src/relay_config.rs:75`).
- q10 gossip signing: compat when `FE_SYNC_GOSSIP_SIGNING` is unset; an unknown value fails closed to require (`fe-sync/src/gossip_auth.rs:140-146`).
- q11 unsigned adverts: compat for one release (`fe-sync/src/relay_config.rs:94-95`).
- q12 production relay host (D-23): no code; still a user question.

## HANDED-OFF (owner = next round)

- HANDED-OFF (orchestrator): commit `fe-runtime/src/dialback.rs`, `fe-sync/src/reachability.rs`, the R-NET-6 tree files listed above, `fe-ui/src/dialogs/peer_debug.rs`, `fractalengine/src/relay_serve_bridge.rs` and the staged I-DOC-NET docs. Then run the `cargo check` barrier.
- HANDED-OFF (I-SYNC, `fe-sync/Cargo.toml`): add `net-report = []` and `default = ["net-report"]` (R6-05). Until then P11 and P13 cannot pass.
- HANDED-OFF (next NET gate, `fe-sync/src/relay_server.rs`): SEC-INBOX 7. NotMember denials alone must not close the listener; use per-source throttling. Also: keep the latest snapshot and apply it in `install_gate` (R6-11 / R-INT-6), and make `AllHosted` serve only `config.hosted_verses` (SEC-INBOX 15 residual).
- HANDED-OFF (next NET gate, `fe-sync/src/dialback_responder.rs`): SEC-INBOX 13. Settle only on an `ok=true` record from an authorized prober tied to the verse the request went to; let the timeout report failure.
- HANDED-OFF (next NET gate, `fe-sync/src/dialback_prober.rs`): SEC-INBOX 14. Wrap `resolve_public` (or `responder.answer` in `dispatch`) in `tokio::time::timeout(PROBE_TOTAL_TIMEOUT, ..)`.
- HANDED-OFF (I-API, `fe-api/src/mcp/mod.rs`, `fe-api/src/network.rs`, `fe-api/tests/mcp_integration.rs`): add the `get_network_diagnostics` `TOOLS` row (`min_role: Viewer`, read-only) and bump the tool count. Use `try_send` at `network.rs:126` and map `Full` to unavailable. Add `network_diagnostics: None` at `mcp_integration.rs:968`. Call `NETWORK_DETAIL_GATE.opted_in_from_env()` once at startup.
- HANDED-OFF (I-UI, `fe-ui/src/plugin.rs`): register `crate::settings::mirror_self_test_verdict` in `Update`, once `settings.rs` is committed.
- HANDED-OFF (`sync_thread.rs` owner, residual): N3-16/N4-25. Resolve DialOnly advertised domains with `advertised_relay_insertable` before they reach the allowlist. Name the remaining rebinding window.
- HANDED-OFF (spec owner / T-NET): amend spec §4.4 item 7 to "oldest issued wins among the highest role (churn-resistant)", matching `relay_trust.rs:38`.
- HANDED-OFF (next NET round, tests): AC-5 production reconciler leg, AC-6 unhealthy-relay leg, AC-7 live-endpoint pin leg and AC-9 end to end. Extend `peer_relay_e2e_test.rs` journeys; no new functions (R6-19, R6-20).
- HANDED-OFF (gate owners F1/F2/F3): write `review-fable-f1.md`, `review-fable-f2.md` and `review-fable-f3.md`, or have I-CONDUCTOR record that the R-NET reviews stand in for them.
- HANDED-OFF (I-DOC): document `verify_strict`, the prober `no_proxy`, the Custom-relay strip (N3-12), the book re-filter on allowlist change, and Disabled-inert adverts in `fe-sync/src/AGENTS.md` §relay-trust and §reachability.
- HANDED-OFF (I-CONDUCTOR): P17. Update the `conductor/tracks.md` row and metadata status/progress.
- HANDED-OFF (CLOCK group): N4-12. Per-author fairness for the unsent role-row buffer in `fe-sync/src/sharding.rs`.

Verdict: partial. The core trust plane (P1-P10, P14 REST, P15 code) is complete. HEAD does not build until the tree files are committed. P11, P12, P13 and the MCP half of P14 are partial, and SEC-INBOX 7, 13 and 14 are open with no gate disposition.
