---
type: plan
track: peer_networking_support_20261031
title: "Implementation Plan: peer networking support (embedded relay, members-only access, signed advertisements + gossip, diagnostics)"
created: 2026-10-31
resource: ./spec.md
computed_at_commit: 8ee4a96
partitions_confidence: hypothesis
---

# Implementation plan: peer_networking_support_20261031

> Citations computed @`8ee4a96`. Partitions are a **hypothesis**. Run `/slice` after Gate F0 and after `relay_operability_20261031` closes. By then HEAD will have moved, and the sibling will have created `fe-sync/src/relay_advertise.rs`, `fe-api/src/relay_advertise.rs` and `docs/relay-operations.md`. Re-grep every `owns` path before launch.

## 0. Assumptions

- **D-PN-1..12 = OD-1..12** (spec §6). Each proceeds on its recommended default unless the user or Gate F0 overrides it. Reversal costs are recorded in the spec.
- **The sibling closes first.** W1 starts only after `relay_operability_20261031` S9 records its verdict. Spec §8 items 1-8 were applied to that track at Gate F0 (its OD-6 default is now DialOnly), so no re-ruling on hazard B is pending.
- **Fable gates block.** A FAIL verdict blocks the next wave. F1-F3 reviewers **fix their own findings**, then re-verdict. F0 amended the spec and plan in place (`review-fable-design.md`).
- **AC-9 was decided at F0.** iroh 1.3 has a single home relay, so AC-9 is the documented warning path. `HomeEligible` is reached by a local pin or by the coverage rule (OD-13, awaiting a user decision; until then slices implement the rule behind the Settings toggle, default on, so reversing it is a constant change).

## 1. Doctrine for every slice

- **Agents sketch, the orchestrator inks.** Slices never run cargo. The orchestrator runs one serial gate per wave, prefixed with `CARGO_PROFILE_DEV_DEBUG=0`:
  - `cargo check -p fe-sync`, with and without `--features relay-server`;
  - the tests of the touched crates;
  - the binaries.
  The full sweep runs once, at F3.
- **Security tripwires.** Every gate checks these mechanically:
  - the gate never reads `default_access` or `open_to_all`;
  - access is never `AllowAll`, and plaintext never runs outside `#[cfg(test)]`;
  - the relay stays off unless the feature is built **and** `FE_RELAY_SERVE=1`;
  - advertised relays never carry `RelayAccessToken`;
  - the planner never removes a Configured URL;
  - adoption state is never replicated;
  - a signed-magic frame that fails verification is never re-parsed as legacy;
  - the prober connects only to resolved public IPs;
  - client IPs are not logged by default.
- **Tests model real flows** through public seams. Fake only the process edge:
  - the network, via a relay-only `clear_ip_transports` endpoint and an in-process `run_relay_server_with_access`;
  - the clock;
  - DNS, for the prober.
  No tautologies.
- **Docs.** Code carries terse doc comments. The rationale goes in the slice's named section of `fe-sync/src/AGENTS.md`.
- **Public repo.** Frame everything for self-hosters and operators. `grep -rni "tenant\|billing\|invoice\|pricing"` over every touched file must return nothing.

## 2. Waves

### Gate F0: design and threat-model review (fable, before any code)

- [x] **F0** (fable, 2026-10-31): adversarially reviewed the spec and this plan against the cited iroh sources. **PASS-WITH-FIXES.**
  - Ruled on spec §9 items (a)-(g); all confirmed or corrected in place.
  - Confirmed single home relay, dial-any-URL and `Clients::disconnect` teardown; confirmed the portmapped address appears in `watch_addr`.
  - Decided AC-9 (documented warning path) and added OD-13 (coverage auto-home, user decision pending).
  - Wrote `review-fable-design.md` and the verdict in spec §9; applied the eight §8 changes to `relay_operability_20261031`.

### W1: pure foundations (parallel, disjoint new files)

- [x] **P1** (opus) `fe-sync/src/gossip_auth.rs`: DONE NET-13 (`9faa69a0`)
  - `SignedGossipFrame` v1 in canonical CBOR, with a byte-exact re-encode check;
  - `sign_domain`/`verify_domain` with domain `"fe-gossip-v1\0"`;
  - `ReplayGuard`: per-(verse, author) HLC window floor (`hlc < hwm − max_age` drops), per-(verse, author) LRU of 256, max age per kind, skew bound, frame-size cap before decode;
  - `SigningMode::{Compat,Require}`.
  - *Acceptance:* round-trip, tamper and replay tables pass; a wrong verse, wrong key or stale frame each yields a named reason.
- [x] **P2** (sonnet) `fe-sync/src/relay_trust.rs`: DONE NET-14 (`a97864a9`, policy and signed body in the shared `fe-runtime/src/relay_advertisement.rs:388`) and NET-15 (`dcf46934`)
  - `RelayUrlPolicy`;
  - the `RelayAdvertisementV1` canonical body, with sign/verify, expiry and bounded selection;
  - a pure `RelayReconcilePlan` over three source tiers (`Configured`, `Default`, `Advertised(verse)`) that refcounts URLs, never removes a Configured one, demotes `Default` only while a coverage-eligible relay is connected as home, and re-inserts it on any failure signal;
  - a pure coverage evaluator (OD-13): `HomeEligible` iff every participating verse carries an admitted, healthy advertisement for the URL;
  - a pure peer-address filter;
  - every new `pub mod` line in `fe-sync/src/lib.rs`, with `relay_server` behind a cfg gate.
  - *Acceptance:* a property test proves Configured URLs always survive, that refcount 0 is the only path to removing an Advertised URL, and that `Default` is absent only in states where a covered relay is connected home.
- [x] **P3** (opus, auth surface) `fe-sync/src/relay_server.rs`: DONE NET-17 (`1e60e9b2`)
  - `RelayServeConfig::from_env` with the refuse-to-start rules;
  - `RelayMembershipGate: AccessControl`: snapshot (explicit Viewer+ only, never `__peers`-gated), caps counted only on `Allow`, a gate-side `endpoint_id → {connection_id}` table, generation stamps, rate-limited abuse events, circuit breaker;
  - `EmbeddedRelay::{spawn, revoke}`, revoking via `relay_service().clients().disconnect` inside a 250 ms reconciler that retries until `on_disconnect` has fired for every tracked connection of a revoked DID (closes the `on_connect`-before-`register` race);
  - a OnceLock seam to the sync thread;
  - `fe-sync/Cargo.toml`: feature `relay-server` (optional `iroh-relay/server`), `fe-canonical-log`, iroh `unstable-net-report`, an HTTPS client (reuse a workspace dependency if one exists), and iroh `test-utils` as a dev-dependency.
  - *Acceptance:* an in-process relay admits a member, denies a non-member, and evicts on revoke.
- [x] **P4** (sonnet) `fe-sync/src/reachability.rs`: DONE NET-18 (`3a634a49` wiring) and NET-19 (`c498aa8b`, SSRF-pinned prober)
  - `NetReportSummary` and `classify()`, returning a verdict plus remediation text;
  - the `ReachabilityProbe` adapter;
  - the FR-2b dial-back prober (all resolved addresses public, resolver override with hostname verification, fixed path, no redirects, body and time caps, per-requester and global rate limits);
  - a signed `DialbackRecord {prober_did, requester_did, url, at_ms, ok}` under domain `fe-dialback-v1\0`, with the acceptance rule (fresh, `ok`, requester = advertiser, prober ≠ advertiser).
  - *Acceptance:* classifier tables pass (a `Portmapped`-only public address is `BehindNat`); the prober refuses private, loopback, mixed-record, rebinding and redirect targets; a self-attested record is refused.
- [x] **P5** (sonnet) `fe-sync/src/net_diagnostics.rs`: DONE NET-20 (`4789ac31`)
  - `NetworkDiagnostics`;
  - `PeerConnectionKind`, derived from the active addresses in `remote_info`;
  - IP redaction and the downgrade hint.
  - *Acceptance:* table tests pass; the kind is never derived from config.
- [ ] **Gate W1** (orchestrator): one serial check and test run, with and without `relay-server`.

### W2: access control and advertisement-hardening integration

- [x] **P6** (opus, integration): DONE NET-21 (`45263c28`), NET-22 (`2d672e88`), NET-23 (`aae59b52`)
  - `VerseFabric::relay_admission_snapshot`;
  - snapshot and revoke pushes to the gate on every role or `__peers` change;
  - hardening the sibling's `__relays` consumer: read-time admission, quarantine, adoption modes, coverage evaluation on every verse join/leave and advertisement change, health loop plus the 30 s `home_relay_status()` trigger, expiry, reconcile plan with `Default` demotion/re-insert;
  - the peer-address filter before `add_peer_addr`.
  - Files: `sync_thread.rs`, `relay_advertise.rs`, `relay_config.rs`, `endpoint.rs`, `sharding.rs`. Fold W1 rationale into the §peer-relay-server, §relay-trust and §reachability sections.
  - *Acceptance:* AC-6, AC-7, AC-8, AC-10 and AC-17.
- [x] **P7** (sonnet) `fractalengine-relay`: DONE NET-24 (`93b26355`)
  - a non-default `relay-server` feature;
  - in `main.rs`, an OperatorGate-style opt-in that registers the gate before `spawn_sync_thread`, then spawns `EmbeddedRelay`.
  - *Acceptance:* AC-1 and AC-2.
- [x] **P8** (sonnet) `fe-api/src/relay_advertise.rs` (created by the sibling): DONE NET-25 (`0a11e04e`)
  - require `expires_at` and cap it at 30 days;
  - pre-check URLs with `RelayUrlPolicy`, and enforce the set bound;
  - build the signed v1 row;
  - require a fresh `DialbackRecord` for peer-run relays;
  - update `fe-api/AGENTS.md`.
  - *Acceptance:* AC-16 holds at the REST layer, and expired or `http://` URLs are rejected.
- [ ] **Gate W2** (orchestrator): serial check and tests for fe-sync (both feature sets), fe-api and fractalengine-relay.

### Gate F1: adversarial security review of access control and advertisement hardening (fable)

- [x] **F1** (fable): scope is the P2, P3 and P4 diff plus all of W2. VERDICT PASS-WITH-FIXES (R-NET-3 and R-NET-4, `review-turn4/`); pointer and fixed-finding list in `review-fable-f1.md`. Fixed after the gate: SEC-INBOX 7 (`1e83a398`), N3-16 and N4-25 (NET5-06).
  - Run `/security-review` on `relay_server.rs`, `relay_trust.rs`, `reachability.rs` and the `__relays` admission path.
  - Attack the gate: Sybil peers, the revoke race, an `open_to_all` leak.
  - Attack the planner: removal of a Configured URL, a token leak.
  - Attack the prober (SSRF, DNS rebinding) and the peer-address filter.
  - Fix own findings and write `review-fable-f1.md`.
  - *Acceptance:* PASS. A FAIL blocks W3.

### W3: gossip signing

- [x] **P9** (opus): DONE NET-26 (`18f246c4`)
  - sign at every broadcast site: `distributed_query.rs` ×2, and `sync_thread.rs` tileset advertisements, batched;
  - verify in the pump before `handle_gossip_incoming`, keeping the F23 direct check and adding inner-claim equality;
  - add the `PeerDeclaration.gossip_sig` latch in `sharding.rs`, the verse setting `gossip_signing: required`, and `FE_SYNC_GOSSIP_SIGNING`;
  - write the §gossip-signing section.
  - *Acceptance:* AC-11 and AC-12.
- [x] **P10** (sonnet) `virtual_transport.rs`: the sim hub carries byte-identical signed frames. DONE NET-27 (contract at `fe-sync/src/virtual_transport.rs:113`); the signing-on test run is pending (user-gated).
  - *Acceptance:* the existing sim gossip tests pass with signing on.
- [ ] **Gate W3** (orchestrator): serial fe-sync check and tests.

### Gate F2: gossip protocol review (fable)

- [x] **F2** (fable): attack the protocol. VERDICT PASS-WITH-FIXES (R-NET-5, `review-turn4/`); pointer and fixed-finding list in `review-fable-f2.md`. Fixed after the gate: SEC-INBOX 13 and 14 (`1e83a398`). N5-08 remains HANDED-OFF.
  - Replay across a restart with an empty LRU, and within the max-age window.
  - Key rotation and revoked DIDs.
  - Mixed-version downgrade: a legacy frame from a latched author, a bad signature behind the magic, a fresh peer that has no latch yet.
  - Cross-protocol signature reuse.
  - Benchmark the verify cost on the compute fan-out.
  - Fix own findings and write `review-fable-f2.md`.
  - *Acceptance:* PASS. A FAIL blocks W4.

### W4: relay-capable desktop and diagnostics surfaces

- [ ] **P13** (sonnet, W4a): code done, run pending (user-gated). Command and responder landed (NET-28 `f8191254`, NET-29 `ccd1a19c`). R6-05 fixed by NET5-02: `fe-sync/Cargo.toml` now declares `net-report` (default on), so the self-test in `fe-sync/src/net_diagnostics.rs:113` can run. Open until a person sees a verdict in Settings.
  - `SyncCommand::GetNetworkDiagnostics` → `SyncEvent::NetworkDiagnostics`;
  - the self-test, run on demand and on network change;
  - the dial-back responder: signed `reach_probe` frames, FR-2b controls;
  - files: `sync_thread.rs`, `messages.rs`, `status.rs`, plus the §net-diagnostics section.
  - *Acceptance:* AC-13, and the responder half of AC-15.
- [ ] **P11** (sonnet, W4b): code done, run pending (user-gated). Code landed (NET-30, `75256acc` and `6066a4a0`; `fe-ui/src/settings.rs:110-133`). R6-05 (NET5-02), R6-07 (I5-UI mirror) and R6-11 (NET5-03) are fixed. R6-10 is STILL OPEN at HEAD: `fractalengine/src/relay_serve_bridge.rs` `decide()` still requires `dialback_passed` before `Start`, and the dial-back probes a relay that is not listening yet, so the start stays unreachable. Stays open.
  - a non-default `relay-server` feature in `fractalengine`;
  - `AppSettings.serve_relay`, default false; `AppSettings.act_as_prober`, default false; `AppSettings.auto_home_relay`, default true (OD-13 toggle);
  - a toggle and consent dialog in `ui_shell/right_sidebar.rs`;
  - the relay starts only after the self-test and dial-back pass.
  - *Acceptance:* the toggle is inert without the feature, and refuses to start without a passing self-test.
- [x] **P12** (sonnet, W4b) Network panel in `fe-ui/src/dialogs/peer_debug.rs`: DONE NET-31 (`1e4f6634`)
  - per peer: Direct, Relayed or Mixed, plus the relay used;
  - portmapper state, reachability verdict, relay-server state, downgrade hint;
  - update `fe-ui/src/dialogs/AGENTS.md`.
  - *Acceptance:* renders from a fixture and shows no IPs by default.
- [x] **P14** (sonnet, W4b): DONE NET-32 (`ed35b534`, `2b82f6fd`)
  - `GET /api/v1/network/diagnostics` in `fe-api/src/network.rs`, plus the MCP tool `get_network_diagnostics`;
  - peers filtered to the caller's Viewer+ verses;
  - full IPs only for an Owner with `FE_API_NETWORK_DETAIL=1` (OperatorGate).
  - *Acceptance:* AC-14.
- [ ] **Gate W4** (orchestrator): serial workspace check, plus tests for fe-sync, fe-api, fe-ui and both binaries.

### W5: end-to-end evidence

- [x] **P15** (sonnet) `fe-sync/tests/peer_relay_e2e_test.rs`: real endpoints, in-process members-only relay. DONE NET-33 code (`c836d2ee`); the run is pending (user-gated). It covers:
  - two relay-only peers syncing through a peer-run relay (AC-3);
  - a non-member denied (AC-4);
  - eviction on revoke (AC-5);
  - advertisements rejected for a bad signature, expiry or ill health (AC-6);
  - unsigned and wrongly signed gossip rejected (AC-11);
  - diagnostics reporting Direct vs Relayed truthfully (AC-13);
  - hazard B as the documented warning path (AC-9) and the coverage rule (AC-17).
  - *Acceptance:* passes at Gate W5.
- [ ] **Gate W5** (orchestrator): P15 plus the full fe-sync suite, for both feature sets.

### Gate F3: pre-merge full networking security review (fable)

- [ ] **F3** (fable): full-diff review, including `/security-review`. VERDICT PASS-WITH-FIXES (R-NET-6, `../p2p_asset_streaming_20260718/review-turn4/`); pointer in `review-fable-f3.md`. Open: R6-10 (relay start still needs a dial-back that needs a listener), so the relay start stays unreachable. In-app and batch evidence is user-gated.
  - Evidence is the P15 e2e run log: two peers plus a relay.
  - Run `cargo deny check` over the `iroh-relay/server` dependency tree.
  - Check AC-1 to AC-16 against the code, and the tripwires mechanically.
  - Fix own findings and write `review-fable-f3.md`.
  - Then run the full sweep once: `cargo test` plus `clippy --all-targets -D warnings`.
  - *Acceptance:* PASS. A FAIL blocks W6 and the merge.

### W6: docs and retro

- [x] **P16** (haiku) docs: DONE NET-37 (`4768c059`)
  - consolidate `fe-sync/src/AGENTS.md`;
  - in `docs/relay-operations.md`: the embedded-relay env table, ACME and manual recipes, firewall guidance against floods, restart-on-revoke for the standalone binary, desktop guidance;
  - in `SECURITY.md`: S-1 closed, plus residuals (metadata, upstream `accept_conn_limit`);
  - update the root `AGENTS.md` crate note.
- [ ] **P17** (haiku) close:
  - the `conductor/tracks.md` row, plan ticks, and metadata status and progress;
  - a retro: which ODs held, F0-F3 verdicts, reversals taken.

## 3. Flow tests per slice (each asserts an observable outcome)

- **P1:** `signed_frame_round_trips_byte_exact`, `tampered_payload_or_wrong_verse_is_rejected_with_named_reason`, `replayed_or_stale_frame_is_rejected_and_fresh_one_admitted`.
- **P2:** `planner_never_removes_a_configured_relay_url` (property), `default_relays_are_absent_only_while_a_covered_relay_is_connected_home` (property), `same_url_advertised_in_two_verses_survives_one_withdrawal`, `coverage_breaks_when_a_verse_without_the_advertisement_is_joined`, `advertisement_table_rejects_bad_sig_expired_http_private_ip_and_fifth_entry`, `peer_addr_filter_strips_unallowlisted_relay_urls`.
- **P3:** `member_admitted_non_member_denied_even_when_verse_is_open_to_all`, `fresh_viewer_without_peers_row_is_admitted`, `revoked_member_is_disconnected_and_reconnect_denied`, `revoke_landing_between_admit_and_register_still_evicts`, `per_endpoint_and_global_caps_deny_with_named_reason`, `refuses_to_start_without_cert_mode_or_verses_or_with_unhosted_verse`.
- **P4:** `classify_table_cgnat_symmetric_captive_noudp_public_and_portmapped_only`, `prober_refuses_private_loopback_mixed_record_rebinding_and_redirect_targets`, `dialback_record_refuses_self_attestation_and_stale_or_failed_probes`.
- **P5:** `remote_info_active_addrs_map_to_direct_relayed_mixed_unknown`, `diagnostics_redact_ips_by_default`.
- **P6:** `valid_advertisement_is_dial_only_by_default_and_pinned_one_is_inserted_without_token`, `demoted_advertiser_withdraws_advertisement`, `unhealthy_adopted_relay_is_removed_after_three_failures`.
- **P7:** `relay_binary_binds_no_relay_listener_without_opt_in` (asserts the listener count).
- **P8:** `post_relay_rejects_expired_or_http_url_and_signs_valid_row`.
- **P9:** `relayed_signed_compute_request_is_attributed_to_its_author` (multi-hop), `legacy_frame_for_latched_author_is_dropped`, `require_mode_drops_all_legacy_frames`.
- **P13/P14:** `diagnostics_command_returns_snapshot`, `api_diagnostics_hides_peers_from_unreadable_verses_and_redacts_ips`.

## 4. Docs and retro

- **Rationale placement.** Each slice writes its rationale to a named `fe-sync/src/AGENTS.md` section, with a one-line pointer in code: §peer-relay-server, §relay-trust, §reachability, §gossip-signing, §net-diagnostics. P16 consolidates them.
- **Sibling follow-up.** P17 records whether `relay_operability_20261031` adopted spec §8 items 1-8. Any refused item becomes an explicit residual in `SECURITY.md`.
- **Retro content.** Record the OD outcomes, the F0-F3 verdicts, whether DialOnly alone held for hazard B, and the trigger for flipping gossip signing to `require`.

## 5. Retro (T-NET, 2026-11-13)

- **Verdicts.** F0 PASS-WITH-FIXES (2026-10-31, `review-fable-design.md`). F1 is covered by R-NET-3 and R-NET-4, both PASS-WITH-FIXES. F2 is covered by R-NET-5, PASS-WITH-FIXES. F3 is covered by R-NET-6, PASS-WITH-FIXES, and it fails closed for the relay start. Each of `review-fable-f1.md`, `review-fable-f2.md` and `review-fable-f3.md` is still unwritten.
- **Hazard B.** F0-5 found that "one call onboards every peer" is not true under DialOnly. AC-9 was decided as a documented warning path (F0 ruling), not a code guarantee. P15 covers the coverage rule, and the run is pending.
- **Spec 8 items.** relay_operability adopted the eight F0 section 8 changes (the F0 entry in section 4 says so), and OD-6 moved to DialOnly. That is the default it ships with.
- **Open residuals.** R-NET-3 N3-16 and R-NET-4 N4-25: DialOnly advertised domains reach the peer-address allowlist unresolved (HANDED-OFF to the `sync_thread.rs` owner). R-NET-6 R6-05, R6-10, R6-11: P11 and P13 stay open until these land.
- **Trigger to flip gossip signing to require.** Next minor release, or once every declared peer in the hosted verses is latched (`spec.md:212`).
