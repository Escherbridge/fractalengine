---
type: Review
track: relay_operability_20261031
critic: C-NET
round: 4
items: [NET-01, NET-02, NET-03, NET-04, NET-05, NET-06, NET-07, NET-08, NET-09, NET-10, NET-12]
date: 2026-11-13
---

# C-NET completeness: relay_operability_20261031

Checked against HEAD `fd61b661` plus the working tree. No cargo was run. "HEAD" means committed. "Tree" means a file that is modified, staged or untracked and not yet committed. The tree-only gaps that break the build are listed once, in the peer_networking file (§ commit state), because none of them is in this track's files.

## Spec requirements and plan tasks

| Item | Status | Evidence |
|---|---|---|
| FR-1 `UsageEvent` + `UsageSink`, no-op and JSON-lines sinks (S1, NET-01) | complete | `fe-runtime/src/usage.rs:15` `UsageEvent`, `:32` trait, `:38` `NoopUsageSink`, `:47` `JsonLinesUsageSink` (bounded queue, one writer thread, R1-16); tests `:182` reopen, `:201` concurrent |
| FR-1 middleware + env selection (S2, NET-03) | complete | `fe-api/src/lib.rs:274-278` outermost `Router::layer(from_fn_with_state(usage_sink_with_metrics(), record_usage))`; `fe-api/src/auth.rs:36` fills `UsageCallerSlot`; env read `fe-api/src/network_ops.rs:38` |
| FR-2 opt-in `/metrics` on its own listener (S3, NET-04) | complete | `fe-api/src/lib.rs:286-291` spawns only when both `metrics_bind` and `metrics` are set; `fe-api/src/metrics.rs:31` `listener_count`; non-loopback bind warns `network_ops.rs:63` |
| FR-3 `FE_LOG_FORMAT=json` (S5, NET-02) | complete | `fe-runtime/src/log_format.rs:8,18,26`; GUI `fractalengine/src/main.rs:119-123`; relay `fractalengine-relay/src/main.rs:48-53`; workspace `tracing-subscriber` has `json` (`Cargo.toml:116`) |
| FR-4 backup/restore runbook (S8, NET-10) | complete (doc) | `docs/relay-operations.md:34` §2 |
| FR-5 self-hosted iroh-relay recipe (S8, NET-10) | complete (doc) | `docs/relay-operations.md:75` §3; `docker/compose.dev.yml:35-42` digest-pinned `iroh-relay` service; `docker/README.md:70` pointer |
| FR-6 reverse-proxy TLS recipe (S8, NET-10) | complete (doc) | `docs/relay-operations.md:113` §4 (Caddy `:118`, nginx `:130`) |
| FR-7 row type + the one URL validator (S6 part 1, NET-05) | complete | `fe-runtime/src/relay_advertisement.rs:322` `validate_advertised_relay_url`; `:211` `AdvertisedRelayUrl` newtype; `:226` `home_eligible`; `:289` `check_resolved_relay_address`; `:12` `FE_SYNC_ALLOW_PRIVATE_RELAYS`; unknown `v` `:313` |
| FR-7 read-time role-gated book, quarantine, demotion re-judge (S6 part 2, NET-06) | partial | `fe-sync/src/relay_advertise.rs:116-181` `observe_at` (Manager floor `:28`, quarantine), `:184` `rejudge`, `:262` `expire`; per-author caps `:22,:25`. Gap: cross-author withdrawal still depends on delivery order (R1-14, see HANDED-OFF) |
| FR-7 wiring into the fabric and inbound pump (S6 part 3, NET-07) | complete | `fe-sync/src/sharding.rs:1618` `expire_relay_adverts`; empowerment re-read replays `__relays/` at `fe-sync/src/sync_thread.rs:2754` (R2-04) |
| FR-7 `RelayAdvertisePolicy` + refcounted reconciliation (S7, NET-08) | complete | `fe-sync/src/relay_reconcile.rs:112,134` (Deny inert), `:544` refcount removal; insert/remove `fe-sync/src/sync_thread.rs:2222-2290`; tokenless entry tripwire `fe-sync/src/relay_config.rs:355`; DialOnly default per `docs/relay-operations.md:22` |
| FR-7 REST write path (S6b, NET-09) | complete | `fe-api/src/relay_advertise.rs:206` `post_relay`, `:252` `delete_relay`, Manager gate `:213`, safe id `:216`, `try_send` `:192` |
| FR-7 OD-10: advertised relays never carry a token | complete | `fe-sync/src/relay_config.rs:355` asserts `auth_token.is_none()`; token only for configured Custom relays `:249` |
| S4 binary wiring of `usage_sink`/`metrics_bind` | complete | absorbed into `NetworkOps::from_env()` at `fractalengine/src/main.rs:836-839` and `fractalengine-relay/src/main.rs:542-545`; signer from the node seed `main.rs:273`, relay `main.rs:129` (R1-26, R2-15) |
| S9 review-and-fix with verdicts (NET-12) | complete (review); sweep blocked-user | `review-turn4/R-NET-1.md` and `R-NET-2.md`, both PASS-WITH-FIXES; plan §4 verdict lines present (`plan.md` §4) |
| AC-1 no second port when unset | complete | `fe-api/src/metrics.rs:260-263` asserts `listener_count() == 0` |
| AC-2 `/metrics` shows route class and status | complete | `fe-api/src/metrics.rs:259` `metrics_endpoint_reports_request_count_and_status_for_a_real_request` |
| AC-3 one `UsageEvent` per request | complete | layer at `fe-api/src/lib.rs:274-278`; `fe-api/tests/api_harness_smoke.rs:8` |
| AC-4 JSON lines survive restart | complete | `fe-runtime/src/usage.rs:182` |
| AC-5 json vs byte-identical text | complete | `fe-runtime/src/log_format.rs:36` table; text path keeps the plain `fmt` layer (`fractalengine/src/main.rs:122`, `fractalengine-relay/src/main.rs:53`) |
| AC-8 Owner POST; DialOnly lists without insert; Pinned inserts tokenless without restart; DELETE removes at refcount zero, never a configured URL | partial | planner proofs `fe-sync/src/relay_reconcile.rs:412` (configured never removed), `:582`; REST leg `fe-api/tests/api_integration.rs:1069`. No test drives a live endpoint and observes `insert_relay` fire with `auth_token == None` (the planned S7 two-endpoint flows were written as planner tests); see R-NET-6 R6-20 |
| AC-9 Deny never inserts | complete | `fe-sync/src/relay_reconcile.rs:134` and the Deny leg at `:416` |
| AC-10 Viewer/Editor 403 before any write; forged row refused at the gate; demoted Manager withdrawn | complete | `fe-api/tests/api_integration.rs:1098` (403 for viewer and editor); book journey `fe-sync/src/relay_advertise.rs:810`, demotion leg `:835` |
| AC-11 unknown field applied, unknown `v` skipped | complete | `fe-runtime/src/relay_advertisement.rs:23,575-576`; consumer `fe-sync/src/relay_advertise.rs:681-687`, journey `:851` |

## Scoper-excluded items (reported by class, not gaps)

- blocked-user: plan §2 gates W1/W2/W3 (orchestrator serial build and tests per wave).
- obsolete: close-out full workspace `cargo test` plus clippy (clippy removed 2026-11-09; the test half runs only via `tools/qa/test_batches.ps1` when the user orders it).
- obsolete: S4 smoke tests `relay_binary_wires_usage_sink_and_metrics_bind_from_env_into_apiconfig` and `gui_binary_wires_the_same_two_fields_from_env_into_apiconfig`.
- blocked-user: AC-6 (backup round-trip against a running relay) and AC-7 (relay-assisted sync against the compose service).

## Gate verdicts and hand-off delivery

Each HANDED-OFF line in R-NET-1 and R-NET-2 was traced to its owner.

- R1-07, R1-08 (home eligibility and resolution at the Pinned insert) reached R-NET-2: fixed as R2-06, `fe-sync/src/sync_thread.rs:2343` `advertised_relay_insertable`, `:2295` resolution with a timeout.
- R1-09 (other dial sites) reached R-NET-4: the prober pins checked addresses (N4-23).
- R1-14 (cross-author tombstone memo) did NOT reach R-NET-2: R-NET-2 records no disposition, and `relay_advertise.rs:129-133` still applies a withdrawal only on arrival. Open below.
- R1-15 reached R-NET-2 (R2-01). SEC-INBOX 3 is closed there.
- R1-24, R1-25 reached I-API-1 (`fe-api/src/lib.rs:274-278`, `fe-api/src/auth.rs:36`).
- R1-26 and R2-15 reached I-BIN-1/I-BIN-2 (signer from the node seed, both binaries).
- R1-29 (docs deferred) closed by NET-10 (`docs/relay-operations.md`).
- R2-07 (DNS rebind on Pinned relays) restated by R-NET-3 as a residual (N3-16). It is still open as a residual.
- R2-16 (`known()` has no reader) closed: `fe-sync/src/sync_thread.rs:2082` feeds `known()` into the peer-address allowlist, pushed at `:2184`.
- R2-18 reached R-NET-4 (N4-11, sharding quarantine fairness).
- R2-19 reached I-DOC-NET: `fe-sync/src/AGENTS.md` §relay-advertise (`:2420`) names the per-author caps and the rebinding residual. That doc is staged in the index, not committed.

SEC-INBOX 1 (advertised URL request forgery): closed by R-NET-1 (validator), R-NET-2 (Pinned insert), R-NET-4 (REST route, `fe-api/src/relay_advertise.rs:115-117` strict policy before signing). Verified at those lines.
SEC-INBOX 3 (global FIFO quarantine): closed by R-NET-2. Verified at `fe-sync/src/relay_advertise.rs:22,25,507`, journey `:876-887`.

## Plan §9 defaults (as built)

- q8 (OD-13 auto-home): default ON. `fe-ui/src/settings.rs:133` `auto_home_relay: true`; `fe-sync/src/relay_config.rs:75` atomic defaults to true.
- q11 (unsigned `__relays` rows): compat for one release. `fe-sync/src/relay_config.rs:94-95` `#[default] Compat`; an unknown value fails closed to `Require` (`:104-108`).

## HANDED-OFF (owner = next round)

- HANDED-OFF (next NET round, owner of `fe-sync/src/relay_advertise.rs`): R1-14. Keep a bounded tombstone memo per (verse, record), so a Manager's withdrawal that arrives before the row it withdraws still wins. Extend `relay_advertisement_book_journey` with that order.
- HANDED-OFF (next NET round, `sync_thread.rs` owner): AC-8 endpoint leg. Drive a live `SyncEndpoint` under `Pinned` and assert the relay map gains the URL with no token and no restart, then loses it at refcount zero. Extend an existing fe-sync journey (R6-20).
- HANDED-OFF (residual, endpoint/iroh owner): R2-07 and N3-16. A domain checked at insert can be rebound before iroh dials it, on both Pinned and DialOnly relays. Close it only if iroh exposes address pinning.
- HANDED-OFF (orchestrator): commit the staged `fe-sync/src/AGENTS.md`, `fe-runtime/src/AGENTS.md` and `fe-api/AGENTS.md` (I-DOC-NET). The rationale above lives only in the index until then.
- HANDED-OFF (I-CONDUCTOR): S9 stays open in `plan.md` §6 until the user-ordered sweep runs. Do not tick it on review alone.

Verdict: code-complete except R1-14 and the AC-8 endpoint leg (both partial, handed off); AC-6, AC-7 and the sweep are user-gated.
