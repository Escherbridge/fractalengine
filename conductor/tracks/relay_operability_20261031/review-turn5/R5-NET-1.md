---
type: Review
---

# R5-NET-1: adversarial review of NET5-01..NET5-04 (security review included)

Base `a85dc9ad`; upstream commits `e12b5a91` (NET5-01), `252a8a8d` (NET5-03), `f68b348a` (NET5-04). NET5-02 landed no source diff; its manifest line landed through I5-MANIFEST (`518714e6`). First pass fixes: `d7d41890`. Second pass (re-run after the integration chain) re-read every owned file at HEAD: N1-N3 still in place, N7/N16 closed by their owner, docs hand-off N4 only partly applied (N17).

## Findings

- N1 FIXED - fe-sync/src/relay_advertise.rs:418,518 - SECURITY: `tombstone_refuses` cleared the memo for any un-refused row, so a sub-floor author's fresh row (quarantined) erased a Manager withdrawal and a stale cross-author row could then resurrect it. Only `make_eligible` (an admitted row) now clears the memo; journey row added (sub-floor row then stale row stays withdrawn).
- N2 FIXED - fe-sync/src/relay_advertise.rs:418,314 - A cross-author row whose clock ran ahead inside `ADVERT_ISSUED_SKEW_MS` out-dated the memo, so the outcome depended on delivery order. The memo now refuses rows issued up to receipt + 60 s; memo pruning keeps it for TTL + skew. Journey row added (row issued receipt + 30 s stays withdrawn).
- N3 FIXED - fe-sync/src/relay_advertise.rs:465 - The global memo eviction cloned every (verse, record) string pair on each insert at the cap; it now clones only the oldest pair.
- N4 HANDED-OFF (I5-DOC-NET) - fe-sync/src/AGENTS.md §relay-advertise - The tombstone memo is not documented. Needed: Manager+ withdrawal memo (64 per verse, 1024 total); same-record re-advert by another author refused until receipt + 60 s skew; on replica reopen the replayed tombstone makes that record id unusable by other authors; remedy: use a new record id.
- N5 DEFERRED (R1-14 follow-up; trigger: a sim run where role grants lag `__relays` rows) - fe-sync/src/relay_advertise.rs:495 - A Manager tombstone that arrives before the tombstoner's role grant leaves no memo and `rejudge` never re-applies it, so role-vs-tombstone order still decides the outcome.
- N6 DEFERRED (trigger: FE_SYNC_RELAY_ADVERT_SIGNED flips to require) - fe-sync/src/relay_advertise.rs:314 - An unsigned compat row with no `issued_at_ms` delivered after the memo is pruned (TTL + skew) is admitted again; signed rows are already expired by then.
- N7 FIXED (by owner I5-MANIFEST, `518714e6`) - fe-sync/Cargo.toml:12-13 - `net-report = []` and `default = ["net-report"]` are declared; no dependent (fractalengine, fractalengine-relay, fe-ui, fe-hexon, fe-sim, fe-test-harness) sets `default-features = false` on fe-sync, so the real probe (net_diagnostics.rs:113) is the one built.
- N8 KEPT (verified by reading iroh-1.3.0 and iroh-base-1.3.0 source) - fe-sync/src/reachability.rs:139-200 - `Endpoint::net_report() -> impl Watcher<Value = Option<unstable_net_report::NetReport>>`, `Watcher::get(&mut self)`, `Report` fields udp_v4/udp_v6/global_v4/global_v6/captive_portal/preferred_relay (RelayUrl derives Display), `mapping_varies_by_dest()`, `bound_sockets() -> Vec<SocketAddr>` all match. Tests never build the `#[non_exhaustive]` Report.
- N9 KEPT (verdict cached and re-run) - fe-sync/src/sync_thread.rs:1107,1293 - The self-test re-runs on every 30 s adoption tick and on each diagnostics request. A pending report keeps the last cached `diagnostics_verdict`.
- N10 DEFERRED (trigger: UPnP/NAT-PMP mapping shown in Settings) - fe-sync/src/net_diagnostics.rs:116 - The probe is built with an empty portmapped list. Portmapped addresses are never proof of reachability, so the verdict cannot widen, but Settings never shows them.
- N11 KEPT (correct) - fe-sync/src/relay_server.rs:1469-1510 - Lock order is always buffer then gate. `install_gate` sets RELAY_GATE under the buffer lock, so a concurrent push is never lost. First-call-wins is kept. A revoke before install removes the DID from the buffer, which matches what an installed gate does (revoked until a newer snapshot re-admits it). No reset helper exists and none is needed.
- N12 KEPT (pre-existing hazard, not reachable under the R8 filter) - fe-sync/src/relay_server.rs:1607 - The buffer and gate are process-wide statics. Any other lib test that ran the sync thread's admission pusher in parallel could overwrite the buffer between push and install. Today `member_admission_and_caps_journey` is the only caller.
- N13 KEPT (pinned seam) - fe-sync/src/relay_server.rs:1327 - `adopt_running_server` is pub under `relay-server` in production builds and uses the WarnOnly breaker with no config. It is an in-process API, not a trust boundary.
- N14 KEPT (AC-5 SLO by design) - fe-sync/tests/peer_relay_e2e_test.rs:91 - The eviction assertion is bounded by EVICTION_TARGET (5 s), not an ordering race. The production reconciler now does the evicting (lines 82, 284); the stand-in `disconnect` loop is gone.
- N15 KEPT (no change needed) - fe-sync/src/status.rs:92 - INT5-05(b): `drain_sync_events` takes `Option<ResMut<NetworkDiagnosticsState>>`, fe-ui initialises it (fe-ui/src/plugin.rs:890), and status.rs has no plugin `build`. The headless relay needs no init.
- N16 FIXED (by owner I5-MANIFEST, `518714e6`) - fe-terrain/Cargo.toml:31 - INT5-05(c): `fe-format = { path = "../fe-format", features = ["entitlement"] }` is in place.
- N17 HANDED-OFF (I5-DOC-NET, next docs pass) - fe-sync/src/AGENTS.md:2439,2490 and fe-sync/tests/AGENTS.md:12 - `5ea12bd3` left two TODOs. Replace the §relay-advertise TODO with: "A row from another author issued at or before receipt + 60 s skew is refused; a tombstone replayed on replica reopen is re-stamped at reopen, so a withdrawn record id stays unusable by other authors across restarts. Remedy: re-advertise under a new record id. Pruned after TTL + skew, when every refused signed row has expired." Replace the §peer-relay-server TODO (and extend tests/AGENTS.md:12) with: "The e2e wraps the in-process server in `EmbeddedRelay::adopt_running_server`; eviction is done by the production reconciler (`run_reconciler`), the test only polls `tracked_connections` under EVICTION_TARGET."
- N18 KEPT (unreachable, defence in depth) - fe-sync/src/relay_advertise.rs:277 - The memo check in `rejudge` cannot fire today: a Manager tombstone drops the record's quarantined rows (`drop_quarantined_record`), and later rows pass the same check before quarantine. Kept so a future quarantine path cannot bypass the memo; no test can reach it.
- N19 KEPT (bounded, Manager-only) - fe-sync/src/relay_advertise.rs:465 - The 1024 global memo cap is shared by verses: a Manager of 16+ verses on this node can evict another verse's memo by flooding withdrawals. Requires Manager in each flooded verse; eviction is counted (`tombstone_evicted`).

## Security inbox

- SEC-INBOX 7: STILL-FIXED - relay_server.rs:1029 feeds the breaker only from distinct global-cap sources (`global_cap_sources.record`); NotMember denials never trip it.
- SEC-INBOX triaged-3: STILL-FIXED - relay_advertise.rs quarantine evicts per author at its own cap and globally from the largest holder.

## Completeness

- NET5-01 COMPLETE fe-sync/src/relay_advertise.rs:418,448,495,518 (memo refuses stale cross-author rows in every row/tombstone order; bounded 64/1024; journey rows ~1000-1065)
- NET5-02 COMPLETE fe-sync/Cargo.toml:12-13 (feature default on); fe-sync/src/reachability.rs:139-200 and net_diagnostics.rs:113-118 match iroh 1.3.0 by reading (N8); verdict cached and re-run at sync_thread.rs:1107,1293 (N9)
- NET5-03 COMPLETE fe-sync/src/relay_server.rs:1469-1510 (journey 1607-1632: pushed before install, revoked seed 7 denied)
- NET5-04 COMPLETE fe-sync/src/relay_server.rs:1327; fe-sync/tests/peer_relay_e2e_test.rs:82,91,284
- INT5-04 COMPLETE fe-sync/src/relay_server.rs:1499 (pending revoke fails closed by removing the DID from the buffered snapshot)
- INT5-05 COMPLETE (a) fe-sync/Cargo.toml:12-13; (b) fe-sync/src/status.rs:92 takes the resource as Option and fe-ui/src/plugin.rs:890 inits it (N15); (c) fe-terrain/Cargo.toml:31

Verdict: PASS-WITH-FIXES (N1-N3 fixed in `d7d41890`, N7/N16 fixed by I5-MANIFEST; docs TODOs N17 handed to I5-DOC-NET; N5/N6/N10 deferred with triggers).
