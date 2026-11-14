---
type: Review
---

# R5-NET-2: adversarial review of NET5-05, NET5-06, NET5-07, NET5-08 (security review included)

Reviewed `git diff a85dc9ad` on the five owned files, told to refute rather than confirm. No cargo run. The first pass (commit 2101dd1f) fixed N2-01 to N2-06. The second pass (2026-11-14) re-read HEAD after the integration slices I5-API (34c611c1), I5-UI (a49adc8b), I5-BIN (065cd218) and R5-INT-2 (0e84f07c), closed their hand-offs and revisited N2-07 and N2-10.

## Findings

N2-01 FIXED - fe-ui/src/settings.rs:200 - `request_self_test_verdict` used a blocking `send` on the bounded sync command queue from a Bevy system, so a full queue would stall the frame. It now uses `try_send`, warns on Full or Disconnected, and retries at the next 10 s interval.
N2-02 FIXED - fe-ui/src/settings.rs:229 - once a verdict was known, the system cloned the verdict String every frame. `self_test_verdict_known()` now checks the lock without allocating, and the interval check runs first.
N2-03 FIXED - fe-ui/src/settings.rs:855 - the INT5-06 acceptance leg was missing. The R6 `settings::` journey now checks: one request per interval, "pending" ignored, "public" recorded, "pending" never overwrites it, and no request once a verdict is known.
N2-04 FIXED - fe-api/src/network.rs:134 - `announce_detail_opt_in` printed the warning on every call. A `std::sync::Once` inside the function now limits it to once per process.
N2-05 FIXED - fe-sync/src/sync_thread.rs:2133 - the `filter_resolvable` resolver treated a URL with no lookup result as allowed (fail-open). It now defaults to `Err("relay lookup did not complete")`.
N2-06 FIXED - fe-sync/src/sync_thread.rs:2147 - the AC-6 leg copied the reconcile loop's failed-relay filter into the test, so deleting the real filter stayed green. Both now call the extracted `drop_failed_relays` (reconcile :2259, test :5205).
N2-07 FIXED (second pass; overrides the first-pass KEPT) - fe-sync/src/sync_thread.rs:2447 - DialOnly URLs were checked with `resolve_public_relay`, which accepts only public https. An operator who set FE_SYNC_ALLOW_PRIVATE_RELAYS=1 therefore lost every LAN DialOnly relay, which the validator had admitted under that same opt-in. Steer rule 2 says private and plain http are refused unless the operator opts in. The new `dial_only_relay_allowed` checks every resolved address under the process policy: loopback, link-local and other refused addresses always fail, and private addresses pass only with the opt-in. Without the opt-in the result is unchanged.
N2-08 DEFERRED (trigger: a sync-loop stall is observed, or more than 8 DialOnly relays become common) - fe-sync/src/sync_thread.rs:2079 - uncached DialOnly lookups run concurrently, but the reconcile pass waits for them. That can stall the sync loop for up to 5 s, once per 10 min per batch. The insert path already has the same shape.
N2-09 DEFERRED (external: iroh owns the connect) - fe-sync/src/sync_thread.rs:2165 - DNS can still rebind between the verdict and iroh's own dial. NET5-06 narrows this window but does not close it, so it stays a documented residual (plan section 9).
N2-10 FIXED (second pass) - fractalengine/src/relay_serve_bridge.rs:169 - the served verse ids were logged at info, so they ended up in any shared log. Info now logs only the count, and the ids moved to debug.
N2-11 KEPT (only while the consent prompt is open, and egui allocates per label anyway) - fe-ui/src/ui_shell/right_sidebar.rs:1278 - the consent block clones the servable list every frame.
N2-12 FIXED by I5-BIN (065cd218) - fractalengine/src/main.rs:447-459 - a one-shot thread named `relay-servable-verses` calls `publish_servable_verses(&shared_db, did)` while the toggle is off. The pinned `SharedDb` is the shared `fe_database::repo::Db` handle, passed by deref.
N2-13 FIXED by I5-UI (a49adc8b) - fe-ui/src/plugin.rs:892-899 - `request_self_test_verdict` then `mirror_self_test_verdict` are registered in Update, chained in the order the settings test uses.
N2-14 FIXED by I5-API (34c611c1) - fe-api/src/lib.rs:204 - `announce_detail_opt_in()` is called once in `spawn_api_service_inner` (the Once guard makes repeated spawns safe).
N2-15 KEPT (matches the task) - fractalengine/src/relay_serve_bridge.rs:166 - `start()` can run before `publish_servable_verses` finishes, so served ids are then published with blank names (the UI falls back to the id). Both lists come from the same `created_by = local DID` query.
N2-16 KEPT (the Disconnected state is permanent and the REST and MCP paths are already rate-limited by `NETWORK_DETAIL_GATE.check_rate`) - fe-api/src/network.rs:157 - Disconnected warns on every request; only Full has the 10 s limiter.

Security review: the diagnostics send never blocks a tokio worker. It uses `try_send`, Full and Disconnected both map to Unavailable, the Full warning is rate-limited, and no payload is logged. Detail stays Owner plus operator opt-in. DialOnly domains are checked fail-closed before they reach the allowlist: the validator runs first, then a policy check on every resolved address, within a 5 s budget, at most 8 concurrent lookups per pass, and a 10 min verdict cache bounded to the current DialOnly set. Configured, Default and inserted entries are not re-resolved and keep their tokens; the live leg covers this (sync_thread.rs:5228). The verse-list query binds the DID, with no string interpolation. The consent stays explicit and off by default, and Confirm is disabled while no verse would be served. No secrets or verse ids are logged at info.
SEC-INBOX: no round-4 inbox entry names an owned file (the only fe-sync entry, 7, is relay_server.rs). SEC-INBOX 15: STILL-FIXED (fractalengine/src/main.rs:1168 hosted set = `load_owned_verse_ids`, created_by = local DID; the consent list uses the same query).

## Completeness

NET5-05 COMPLETE fe-api/src/network.rs:151 (try_send, Full/Disconnected -> Unavailable), :118 (10 s rate-limited warn), :134 (announce once), :340 (full-queue row); MCP row fe-api/src/mcp/mod.rs:311; startup call fe-api/src/lib.rs:204
NET5-06 COMPLETE fe-sync/src/sync_thread.rs:2079 (allowlist resolves DialOnly), :2165 (filter_resolvable + TTL cache), :2447 (policy check), :5091 (exclusion, cache-hit, TTL-retry rows)
NET5-07 COMPLETE fe-sync/src/sync_thread.rs:5177 (AC-7 tokenless pinned insert on a live endpoint), :5186 (AC-8 refcount-zero removal), :5195 (AC-6 failure limit, cooldown, re-adopt), :5228 (configured relay keeps its token)
NET5-08 COMPLETE fe-ui/src/settings.rs:168-226 (servable list, request system), fe-ui/src/ui_shell/right_sidebar.rs:1278-1292 (consent verse list, Confirm disabled when empty), fractalengine/src/relay_serve_bridge.rs:50,70,166 (served ids, publish, start), wired at fractalengine/src/main.rs:453 and fe-ui/src/plugin.rs:895
INT5-03 COMPLETE fe-api/src/network.rs:151 (same change as NET5-05 part 1)
INT5-06 COMPLETE fe-ui/src/plugin.rs:892-899 (registration), fe-ui/src/settings.rs:237 (mirror), :855 (pending-never-overwrites leg)

Verdict: PASS-WITH-FIXES
