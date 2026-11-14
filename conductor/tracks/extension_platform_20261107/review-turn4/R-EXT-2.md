---
type: Review
---

# R-EXT-2 - adversarial review-and-fix gate (EXT-01, EXT-02, EXT-03, EXT-04), security review included

R2-01 FIXED - fractalengine/src/extension_host/ops_drain.rs:754 - Browse ran the full offer refresh (list x wanted x sources plus up to 16 header reads) before answering the listing; it now marks the refresh due and the idle loop runs it after the reply.
R2-02 FIXED - fractalengine/src/extension_host/registry_offers.rs:124 - an unreachable source cost one connect timeout per wanted package per refresh (up to 32); a failed listing now skips that source for the rest of the refresh; journey gained a dead-source leg.
R2-03 FIXED - fe-hexon/tests/remote_registry_test.rs:240 - trust/kill closures passed through Arc::new now name their &str parameters, so the higher-ranked signature never depends on coercion inference.
R2-04 HANDED-OFF (I-BATCH-2) - conductor/test-batches.md:426 - remote_registry_test (feature remote) is reached by no batch line, and the EXT-01 licence leg needs a --features entitlement line under R11-ENT.
R2-05 HANDED-OFF (I-BIN-2) - fractalengine/src/plugin_bridge.rs:2166 - BindingSignals::send logs a dead binding worker at debug, so registry_offers_changed failures are near-silent; rule 6 asks for warn!.
R2-06 KEPT (host accepts contract 2.0.0; matches the package.rs fixture) - fe-hexon/tests/remote_registry_test.rs:84 - AC-6 fixture names fractalengine:plugin@2.0.0, task text said 2.1.0.
R2-07 KEPT (task-specified bound, fails closed) - fractalengine/src/extension_host/registry_offers.rs:15 - only the 3 newest versions are read, so a version_req met only by an older build shows no offer.
R2-08 KEPT (folder endpoints have no issuer; token_url refuses non-http anyway) - fractalengine/src/extension_host/licence_client.rs:73 - no transport answers not-available before consulting a stored token.
R2-09 KEPT (bounded by source reloads) - fractalengine/src/extension_host/licence_client.rs:53 - per-source clients of a replaced transport are rebuilt, the old entry is overwritten, never evicted otherwise.
R2-10 KEPT (documented default) - fractalengine/src/extension_host/blob_mirror_tap.rs:50 - imports committed before the plugin worker builds the mirror are drained, not mirrored.
R2-11 KEPT (new packages refresh at once; cadence is advisory) - fractalengine/src/extension_host/ops_drain.rs:624 - constant job traffic postpones the 10-minute idle refresh.
SEC-1 REFUTED - fractalengine/src/extension_host/sources.rs:385 - licence POST uses the GET path's pinned client: resolve once, internal refused unless the base is internal, resolve_to_addrs, no proxy, no redirects (3xx refused), timeouts, no cookies or credentials, 64 KiB cap on Content-Length and stream, Retry-After delta-seconds only (client clamps to 1 h).
SEC-2 REFUTED - fractalengine/src/extension_host/licence_client.rs:84 - a token is filed only after EntitlementClient::verify (pinned keyring, licensee, device, content hash); governance store names are 64 lowercase hex (no traversal); empty keyring sends nothing; no token or proof is logged.
SEC-3 REFUTED - fractalengine/src/extension_host/registry_offers.rs:146 - header provides come from verified attestations (sealed, trusted, kill-checked, bound to the listed hash), so the per-hash cache cannot be poisoned across sources; header names another id or version -> no offer.
SEC-4 REFUTED - fractalengine/src/extension_host/blob_mirror_tap.rs:23 - only DbResult::GltfImported with a materialization feeds the tap, emitted solely by local imports (fe-database/src/lib.rs:949,1009); replicated rows never mirror; full queue drops and counts.
SEC-5 REFUTED - fe-hexon/src/package.rs:1213 - tamper table covers flipped component byte, edited extension.json, extra member, removed component, removed extension.json; the control verifies under the same trust.
SEC-INBOX: no entry in .omc/state/turn4-security-inbox.md names a file this gate owns.
Leak scan: no forbidden term in any added line of the four slices.

Verdict: PASS-WITH-FIXES
