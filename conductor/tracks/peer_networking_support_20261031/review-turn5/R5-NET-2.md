---
type: Review
---

# R5-NET-2: adversarial review of NET5-05, NET5-06, NET5-07, NET5-08 (security review included)

Reviewed `git diff a85dc9ad` on the five owned files (commits f7942f0a, ed1b6053, a5431399, 98b5f882), told to refute rather than confirm. No cargo run.

## Findings

N2-01 FIXED - fe-ui/src/settings.rs:200 - `request_self_test_verdict` used a blocking `send` on the bounded(256) sync command queue from a Bevy system, so a full queue would stall the frame. It now uses `try_send`, warns on Full or Disconnected, and retries at the next 10 s interval.
N2-02 FIXED - fe-ui/src/settings.rs:229 - once a verdict was known, the system cloned the verdict `String` through `last_self_test_verdict()` on every frame (a hot-path allocation). Added `self_test_verdict_known()`, which only checks the lock and does not allocate, and moved the interval check first.
N2-03 FIXED - fe-ui/src/settings.rs:855 - the INT5-06 acceptance leg was missing: no test proved that "pending" never overwrites a recorded verdict or that requests stop once a verdict is known. Extended `partial_file_fills_missing_fields_from_defaults` (R6 `settings::`) with an App leg: one request per interval, pending ignored, public recorded, pending ignored again, and no request after the interval once a verdict is known.
N2-04 FIXED - fe-api/src/network.rs:134 - `announce_detail_opt_in` printed the warning on every call, so "once per process" depended on how I5-API wired the caller. A `std::sync::Once` inside the function now guarantees it.
N2-05 FIXED - fe-sync/src/sync_thread.rs:2133 - the `filter_resolvable` resolver defaulted to `Ok(())` for a URL with no lookup result (fail-open). The path is unreachable today, but the default is now `Err("relay lookup did not complete")`.
N2-06 FIXED - fe-sync/src/sync_thread.rs:2147 - the AC-6 leg rebuilt the reconcile loop's failed-relay filter inside the test (a tautology: deleting the filter in `reconcile_advertised_relays` stayed green). Extracted `drop_failed_relays`, which reconcile (:2259) and the test (:5179) both call.
N2-07 KEPT (spec: "no private/loopback unless pinned") - fe-sync/src/sync_thread.rs:2079 - DialOnly URLs go through `resolve_public_relay`, which requires `home_eligible` (public https). So a private or plain-http DialOnly relay is no longer allowlisted, even under the private-relay opt-in.
N2-08 DEFERRED (trigger: a sync-loop stall is observed or more than 8 DialOnly relays become common) - fe-sync/src/sync_thread.rs:2079 - uncached DialOnly lookups run concurrently but are awaited inside the reconcile pass, which can stall the sync loop for up to 5 s once per 10 min per batch. The insert path already has the same shape.
N2-09 DEFERRED (external: iroh owns the connect) - fe-sync/src/sync_thread.rs:2165 - DNS can still rebind between the verdict and iroh's own dial. NET5-06 narrows this but does not close it; it stays a documented residual (plan section 9).
N2-10 KEPT (task asked for the info log; logs are local) - fractalengine/src/relay_serve_bridge.rs:169 - the served verse ids are logged at info, and the consent list shows an id only when the name is blank. Nothing leaves the local user.
N2-11 KEPT (only while the consent prompt is open; egui allocates per label anyway) - fe-ui/src/ui_shell/right_sidebar.rs:1272 - the consent block clones the servable list each frame.
N2-12 HANDED-OFF (I5-BIN) - fractalengine/src/relay_serve_bridge.rs:70 - the pinned seam says `publish_servable_verses(&SharedDb, String)`, but no `SharedDb` type exists. The function takes `&fe_database::repo::Db`, the same as `load_owned_verse_ids`. Until main.rs calls it, the list stays empty and Confirm stays disabled ("You own no verses yet").
N2-13 HANDED-OFF (I5-UI) - fe-ui/src/plugin.rs - `request_self_test_verdict` and `mirror_self_test_verdict` are still registered nowhere, so the toggle cannot become enabled until I5-UI adds them, chained in that order.
N2-14 HANDED-OFF (I5-API) - fe-api/src/lib.rs - `announce_detail_opt_in()` has no caller yet. Since N2-04 the function is idempotent, so a plain call at startup is enough.
N2-15 KEPT (matches the task) - fractalengine/src/relay_serve_bridge.rs:166 - `start()` can run before `publish_servable_verses` finishes, so served ids are then published with blank names. The later publish restores the names.
N2-16 KEPT (the Disconnected state is permanent and the API already limits request rate) - fe-api/src/network.rs:157 - Disconnected warns on every request; only Full has the 10 s limiter.

Security review: the diagnostics send cannot block a tokio worker (try_send, no payload logged). DialOnly domains are checked fail-closed before the allowlist (validator, then public-only resolution, then a 5 s budget, at most 8 lookups per pass, a 10 min verdict cache, and a warn per failing verdict). Configured, Default and inserted entries are not re-resolved and keep their tokens; the live leg covers this (sync_thread.rs:5146-5205). The consent stays explicit, off by default, and disabled with no servable verses. No secrets are logged.
SEC-INBOX 15: STILL-FIXED (fractalengine/src/main.rs:1153 uses `load_owned_verse_ids`, created_by = local DID; the consent UI now names that set)

## Completeness

NET5-05 COMPLETE fe-api/src/network.rs:151 (try_send, Full/Disconnected -> Unavailable), :120 (10 s rate-limited warn), :134 (announce), :340 (full-queue row); TOOLS row present at fe-api/src/mcp/mod.rs:305; lib.rs startup call handed to I5-API
NET5-06 COMPLETE fe-sync/src/sync_thread.rs:2079 (allowlist resolves DialOnly), :2165 (filter_resolvable + TTL cache), :5060 (exclusion, cache-hit, TTL-retry rows)
NET5-07 COMPLETE fe-sync/src/sync_thread.rs:5146 (AC-7 tokenless pinned insert on a live endpoint), :5155 (AC-8 refcount-zero removal), :5164 (AC-6 failure limit, cooldown, re-adopt), configured-token leg after it
NET5-08 STILL OPEN - code is complete at fe-ui/src/settings.rs:170-226, right_sidebar.rs:1272-1290 and relay_serve_bridge.rs:48-84,166-178, but nothing calls publish_servable_verses (main.rs, I5-BIN) and the systems are not registered (plugin.rs, I5-UI) - fractalengine/src/relay_serve_bridge.rs:70
INT5-03 COMPLETE fe-api/src/network.rs:151 (same change as NET5-05 part 1)
INT5-06 STILL OPEN - the mirror and the request system plus the test leg exist (fe-ui/src/settings.rs:200,237,855), but registration in fe-ui/src/plugin.rs is pending I5-UI - fe-ui/src/plugin.rs

Verdict: PASS-WITH-FIXES
