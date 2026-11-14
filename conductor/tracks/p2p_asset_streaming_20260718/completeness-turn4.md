---
type: Review
track: p2p_asset_streaming_20260718
critic: C-NET
round: 4
items: [NET-11, NET-39]
date: 2026-11-13
---

# C-NET completeness: p2p_asset_streaming_20260718

Checked against HEAD `fd61b661` plus the working tree. No cargo was run. Only NET-11 and NET-39 were planned this round. The table also records every other plan task, so the track's open set is visible in one place.

## Spec requirements and plan tasks

| Item | Status | Evidence |
|---|---|---|
| Phase 0 evidence pack (FR-2) | complete | `plan.md` Phase 0 ticked; `evidence.md` §1-§5 |
| FR-1 `RelayConfig` Default/Custom/Disabled + EOL warning | complete | `fe-sync/src/relay_config.rs:17` enum, `:217` `is_default_infra` |
| FR-1 relay health in `SyncStatus` | complete | `fe-sync/src/status.rs:30` `health`, `:163` `RelayHealthChanged` |
| Phase 1 verification: headless run shows relay mode and health (NET-39) | complete | `evidence.md:188` §6, run 2026-10-25, 12/12; `scripts/relay-health-verify.log` holds 12 `[PASS]` lines; `plan.md:45` ticked |
| Phase 2 ratification wall (D-73..D-78) | complete | `plan.md` Phase 2, all ticked |
| FR-3 transport trait + protocol fill + cross-restart resume | complete (code) | `fe-runtime/src/blob_transport.rs:85` `BlobTransport`; resume test `fe-sync/tests/tileset_transfer_test.rs:232` |
| FR-4 residency ledger, lazy tiles, despawn on exit ring | complete (code) | `plan.md` Phase 4 ticked; `fe-terrain/src/tiles/streamed.rs` |
| FR-7 application settings surface | complete | `fe-ui/src/settings.rs:72` `AppSettings` |
| FR-5 `PartialHexonFetch` + import split | complete (code) | `fe-hexon/src/partial.rs:87` |
| FR-6 integrity for partial fetch | complete (code) | `plan.md` Phase 5 (S1 `db5883b` .. S8) |
| PAS-P5-P2P-POLICY rate half: P2P downloads honour `FE_MAX_DOWNLOAD_RATE` (NET-11) | partial (fix in tree) | HEAD `658b5bcf`: `fe-sync/src/tileset_transfer.rs:106` env, `:110-117` `TransferLimits::from_env`, `:794-796` `for_p2p_dir`. At HEAD the pacer is per call of `download` (HEAD `:1246`), so N concurrent installs run at N times the cap. The shared pacer that fixes it (R6-01: `:589` field, `:713` `pace`, `:785` reset in `with_limits`) and the two-download test (`fe-sync/tests/tileset_transfer_test.rs:136-170`) are uncommitted |
| Phase 3 verification: two-instance loopback transfer | blocked-user | scenario `fe-test-harness/src/scenarios/two_peer_tileset_stream.rs:142` written, not run |
| Phase 4 verification: NFR-1/NFR-2 | blocked-user | automated proxies written; the frame-time half needs an in-app look |
| Phase 5 verification: hosted and P2P conformance | blocked-user | suite written (`fe-hexon/tests/partial_fetch_p2p.rs:254`), needs the user-ordered sweep |
| Phase 6 sweep | blocked-user | user-ordered; clippy part obsolete |
| Phase 6 retro + archive, decision record closed | blocked-user | waits on the sweep and the in-app NFR evidence |
| PAS-P5-P2P-POLICY full FetchStrategy rebase | blocked-user | plan §9 q9 default "no"; only the rate cap was scoped (NET-11) |

## Scoper-excluded items (reported by class, not gaps)

All of these are blocked-user: the Phase 3, 4 and 5 verifications, the Phase 6 sweep, the Phase 6 retro and archive, and the full FetchStrategy rebase.

## Gate verdicts and hand-off delivery

R-NET-6 (`review-turn4/R-NET-6.md`, untracked) gave PASS-WITH-FIXES for NET-11 and NET-30..33.

- R6-01, R6-02 (shared rate window and its test): FIXED in the working tree, not committed.
- R6-03 (streamed `fetch_chunk` unpaced; chunks burst while the pacer sleeps): DEFERRED to the FetchStrategy rebase, a ledger residual. It is accepted: the average rate still holds (`tileset_transfer.rs:1278-1288`).
- R6-04 KEPT: an unparseable value or `0` means unlimited, matching fe-hexon `P2pConfig::from_env`.
- R6-05, R6-10, R6-11, R6-17, R6-18, R6-21: traced in `../peer_networking_support_20261031/completeness-turn4.md`.
- SEC-INBOX: no entry names a file of this track's NET items.

## Plan §9 defaults (as built)

- q9 (FetchStrategy rebase): not taken. Only `max_download_rate_bytes_per_sec` exists (`fe-sync/src/tileset_transfer.rs:90`); there is no semaphore, source priority or demotion for P2P.

## HANDED-OFF (owner = next round)

- HANDED-OFF (orchestrator): commit `fe-sync/src/tileset_transfer.rs`, `fe-sync/tests/tileset_transfer_test.rs` and `review-turn4/R-NET-6.md`. Until then NET-11 is per-download at HEAD.
- HANDED-OFF (I-CONDUCTOR): `conductor/doneness-ledger.md:161` still says "`TransferLimits` has no rate field". Update PAS-P5-P2P-POLICY to "rate cap done (NET-11, shared per service); FetchStrategy rebase open, user-gated (plan §9 q9)".
- HANDED-OFF (next NET round, only if the user takes the rebase in q9): pace `fetch_chunk` and bound the in-flight chunk burst (R6-03).

Verdict: NET-39 complete; NET-11 partial until the shared-pacer fix is committed. Every other open task is user-gated (sweep, in-app NFR evidence, rebase decision).
