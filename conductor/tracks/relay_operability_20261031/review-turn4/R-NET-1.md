---
type: Review
track: relay_operability_20261031
gate: R-NET-1
upstream: [NET-01, NET-02, NET-03, NET-04, NET-05, NET-09]
security_review: true
date: 2026-11-13
---

# R-NET-1: adversarial review and fix (group NET, wave 1)

Fresh-context review set out to refute NET-01..05 and NET-09, plus the NET-12 share (S9) for these
slices. The security review was mandatory and was done by hand on `fe-runtime/src/relay_advertisement.rs`,
`fe-api/src/relay_advertise.rs`, `fe-sync/src/relay_advertise.rs` and the `sharding.rs` `__relays`
arm. The `/security-review` skill was not used because it reviews the whole working-tree diff, and
other workers share this tree. No cargo was run, so the fixes below are uncompiled.

## Findings

R1-01 FIXED - fe-runtime/src/relay_advertisement.rs:254 - (steer 21:40, MEDIUM, SSRF) the validator accepted any http/https host. It now refuses loopback, 0/8, link-local incl. 169.254.169.254, multicast, broadcast/240/4, ::1, ::, fe80::/10, ff00::/8, IPv4 embedded in mapped/compatible/NAT64 IPv6 forms (:204), and `localhost` / `*.localhost` after trailing-dot trim and lowercasing. It does no DNS.
R1-02 FIXED - fe-runtime/src/relay_advertisement.rs:9 - private ranges (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7, fec0::/10) and plain http need the operator opt-in `FE_SYNC_ALLOW_PRIVATE_RELAYS=1` (default off, read once by `AdvertisedRelayPolicy::process`).
R1-03 FIXED - fe-runtime/src/relay_advertisement.rs:153 - the validator now returns `AdvertisedRelayUrl`, a newtype that only the validator can build. `home_eligible()` is true only for a public https relay. The operator's configured relay keeps its own type (`RelayConfig` / `iroh::RelayUrl`), so a peer row cannot take that path.
R1-04 FIXED - fe-runtime/src/relay_advertisement.rs:216 - `check_resolved_relay_address(ip, policy)` is the one check that dial sites apply to every resolved address (steer rule 5). It shares classification with the validator. The existing `portal_url::is_url_allowed` has no private/opt-in split, so it could not be reused.
R1-05 FIXED - fe-runtime/src/relay_advertisement.rs:274 - userinfo, query and fragment are refused, and URLs are capped at 2048 bytes. A replicated row can no longer carry a credential in its URL (OD-10 tripwire).
R1-06 FIXED - fe-runtime/src/relay_advertisement.rs (tests) - extended the existing `relay_advertisement_rows_and_urls_table` with strict and opt-in columns, rows that are always refused, private rows, home eligibility, resolved-address rows and env parsing. No new test function was added.
R1-07 HANDED-OFF (R-NET-2 / NET-22, owner of fe-sync/src/relay_advertise.rs:373 and sync_thread.rs:1713) - `judge_structure` reduces `AdvertisedRelayUrl` to a `String`, which drops reach. `AdvertChange::Admitted` must carry `home_eligible`, or the Insert arm must re-validate. `RelayOp::Insert` must skip any URL that is not home eligible, because a relay-map entry can become home.
R1-08 HANDED-OFF (R-NET-2 / NET-22, sync_thread.rs:1713 Insert arm) - steer rule 5. Before `insert_relay`, resolve the host with `tokio::net::lookup_host` and refuse unless `check_resolved_relay_address` passes for EVERY address. Residual: iroh does its own connect, so DNS rebinding between check and connect stays open. Record that residual rather than claim it closed.
R1-09 HANDED-OFF (R-NET-4 / NET-19, NET-25, dial-back prober) - every other place that dials an advertised URL must pin the checked address with the same `check_resolved_relay_address`.
R1-10 FIXED - fe-api/src/relay_advertise.rs:80 - a blocking crossbeam `send` on the bounded(256) bridge inside an async handler could stall a tokio worker. It is now `try_send`, and Full and Disconnected each warn and return 503. No send error is swallowed.
R1-11 FIXED - fe-api/src/relay_advertise.rs:104,145 - `verse_id` is checked with `is_safe_path_id` after the role gate. A narrower token could otherwise post to a verse id like `v1-FRACTAL#f1`, which queued a pending write for a verse that does not exist.
R1-12 KEPT (authz order is correct) - fe-api/src/relay_advertise.rs:101,142 - `require_role_and_scope(manager, VERSE#id)` runs before any validation, blob write or replication write, so the 403 precedes every write. The Json body extractor can return 400/422 first, but nothing has been written by then.
R1-13 KEPT (designed trust boundary) - fe-api/src/relay_advertise.rs:117 - rows are signed as the node DID (`advertisement_signer`), not the token subject. Peers gate on the node's role. A manager-scoped token issued by this node acts as the node.
R1-14 HANDED-OFF (R-NET-2) - fe-sync/src/relay_advertise.rs:228 - withdrawal by a different Manager is a different-author tombstone, so it depends on delivery order. A replica that gets B's tombstone before A's row admits A's row. Add a tombstone memo per (verse, record), bounded like the quarantine.
R1-15 HANDED-OFF (R-NET-2) - fe-sync/src/relay_advertise.rs quarantine - global FIFO flood (steer 22:10). It belongs to R-NET-2; noted here only for completeness.
R1-16 FIXED - fe-runtime/src/usage.rs:102,128 - each JSON-lines record ran `spawn_blocking` behind a std mutex, and the middleware spawns one task per request. A slow disk under a request flood could tie up the 512-thread blocking pool. Records now go through one bounded queue (4096) to one writer thread. A full queue drops and counts (`dropped_events`), warning only at powers of two. Write failures warn on transition only. Drop drains and joins, which makes the reopen and concurrent tests deterministic.
R1-17 FIXED - fe-api/src/usage_middleware.rs:61 - `bytes_out` read only the Content-Length header, which axum handlers almost never set (hyper adds it later), so it was nearly always 0. It now falls back to `size_hint().exact()`.
R1-18 KEPT (spec FR-1) - fe-api/src/usage_middleware.rs:36 - `route_class` comes from `MatchedPath` with fallback `<unmatched>` and never the raw path. `scope` carries the token scope (a verse id), which FR-1 explicitly allows, and it is never a metrics label.
R1-19 FIXED - fe-api/src/metrics.rs:135 - the registry used `#[async_trait]`, but async-trait is only a dev-dependency of fe-api, so the lib would not compile. Replaced it with the desugared impl over a sync `observe`, which adds no dependency.
R1-20 FIXED - fe-api/src/metrics.rs:118,153 - method and status class are now bounded `&'static str` labels, so no per-request `format!` remains. The histogram `le` values use OpenMetrics canonical floats (`1.0`, `10.0`, not `1`).
R1-21 FIXED - fe-api/src/network_ops.rs:60 - `/metrics` is unauthenticated (OD-7), and a bind that is not loopback now logs a warning at startup. The bind value is trimmed.
R1-22 FIXED - fe-api/src/network_ops.rs:129, fe-api/tests/api_harness_smoke.rs:109 - both tests now read the usage file only after the sink is dropped, or parse only newline-terminated lines. This removes a partial-line race.
R1-23 KEPT (bounded) - fe-api/src/usage_middleware.rs:71 - one detached task per request remains. Every in-tree sink is now non-blocking (try_send / a sync counter), so the tasks finish immediately.
R1-24 HANDED-OFF (I-API-1) - fe-api/src/lib.rs - the `record_usage` layer must go on via `Router::layer` (not an outer service), or `MatchedPath` is missing. Its state must be `network_ops.usage_sink_with_metrics()`. `spawn_metrics_listener` may run only when both `metrics_bind` and `metrics` are Some, which keeps AC-1 (no second listener when unset).
R1-25 HANDED-OFF (I-API-1) - fe-api/src/auth.rs auth_middleware - fill `UsageCallerSlot` after `authenticate`. Until then `caller_did` is always None and the api_harness_smoke assertion fails.
R1-26 HANDED-OFF (I-BIN-1) - both binaries: `advertisement_signer` must be the keypair whose seed derives the sync author, or every REST advert is refused at read time (advertised_by != SignedEntry author). FE_LOG_FORMAT unset must keep the existing text layer byte-identical.
R1-27 KEPT (no defect) - fe-runtime/src/log_format.rs - only a trimmed, case-insensitive `json` selects Json, Text is the default, and the table test covers it.
R1-28 KEPT (scan clean) - all owned files - the business-wording grep (pattern from plan section 1) and the PUB leak-list grep both come back empty.
R1-29 DEFERRED (S8 slice lands) - docs/relay-operations.md does not exist yet, so FR-4/5/6 and the D-23 framing cannot be verified this round.

## FR verdicts (for review.md and plan.md section 4)

- FR-1/2 usage + metrics: PASS-WITH-FIXES (R1-16, R1-17, R1-19..R1-22); wiring pending I-API-1 (R1-24, R1-25).
- FR-3 JSON logs: PASS (module); binary wiring pending I-BIN-1 (R1-26).
- FR-4/5/6 docs: DEFERRED (R1-29).
- FR-7 relay advertise: PASS-WITH-FIXES (R1-01..R1-06, R1-10, R1-11); dial-site resolution and home gating HANDED-OFF (R1-07..R1-09, R1-14).

Verdict: PASS-WITH-FIXES
