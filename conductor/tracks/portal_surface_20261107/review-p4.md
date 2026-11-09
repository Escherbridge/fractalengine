---
type: Review
date: 2026-11-08
track: portal_surface_20261107
gate: R-P4 (admission) + W6 consolidated verdict
reviewer: P-S6 (fresh context, opus)
verdict: PASS-WITH-FIXES
resource: ./plan.md
---

# R-P4: admission, revoke and static-feed review (`/security-review`)

**Scope.** S4 at `fc29e431` plus SI's CORS wiring in `eabcef17`: `fe-webview/src/portal_bundle/{admission,feeds,mount}.rs`, `backends/tauri.rs` (storage clears), `plugin.rs::apply_portal_admission`, `fe-api/src/portal_bundles.rs` (admission read, `share_route_allows`), `fe-api/src/server.rs` (CORS predicate), `tests/portal_ipc_journey.rs`, `fe-api/tests/portal_bundle_test.rs`. Contracts: `docs/spec/portal-bundle/admission.md`. Binding steers: checkpoint 2026-11-08 20:45 (kill list by hash under any id, store-client v1) and 22:00 (Owner override, rows stay).
**Method.** Desk review; nothing built or run.

## Plan §4 checks

- P4-C1 PASS kill-switch latency within one frame: `apply_portal_admission` runs after `dispatch_commands`/`drain_backend_events` and before `pump_portal_ipc`. `BundleHost::revoke` unmounts (generation bump, 410/404), shuts the bridge, unloads under the token, queues the storage clear and emits the badge, all in that system and in admission.md §5 order.
- P4-C2 PASS no IPC survives a revoke: `PortalBridge::shutdown` closes the broker client, drops every subscription, call and queued envelope, and clears `visible`. Posts from the dying document fail check 1 (no mount).
- P4-C3 PASS with residual per-origin storage clear: `Storage.clearDataForOrigin` (https spelling, parameters from a parsed label only) is queued until a document of another origin commits, and skipped if that label is mounted again. SP-6 is unrun, so admission.md §9's residual stands.
- P4-C4 PASS CORS limited to the share route: `share_route_allows` needs an exact static `s-` label origin, path `/api/v1/shared/<one segment>`, GET or a preflight for GET, and never sets credentials. `a-` labels never match.
- P4-C5 PASS re-consent cannot be bypassed: Admitted needs a record for the exact pair whose `publisher_did` equals the verified seal and whose `capability_digest` equals the build's. The digest covers csp, navigation, subscriptions, commands, hooks, streams and intents. Unsealed builds are never admitted, and a Static mount reaches Admitted only by re-preparing (`Rejudge::Promote`) on the admitted label.

## Findings (one verdict per line)

- P4-01 FIXED (P0, compile) = P1-01: the `admission` and `feeds` modules are now declared.
- P4-02 FIXED (Medium, hardening) A static mount's `connect-src` gained the bare feed origin (`http://127.0.0.1:8765`). An unsigned, replicated static page could therefore send no-preflight simple requests (a GET, or a text/plain POST) to every route of this peer's loopback fe-api: CORS hides the response, not the send. A kept feed now adds `<origin>/api/v1/shared/` (CSP path match). `csp.rs` accepts a `[a-z0-9/_-]` path, and an over-long source is dropped from the feeds rather than Blocking the mount. Both journeys are updated. Contract change: admission.md §7.4 and protocol.md §5.1.
- P4-03 PASS kill list by hash under any id: `GovernanceView::kill_list` takes no package id. Both the content hash and the archive digest are checked, at open (archive digest before any byte is parsed) and in `revocation_for` for live mounts.
- P4-04 NOTE (Low) An audited Owner override (steer 22:00) is honoured by `decide`, but `revocation_for` still revokes on a `revoked` event naming that build. This fails closed; governance should not emit such an event for an overridden build.
- P4-05 NOTE (Low) `kill_list_status` lets an override on either digest clear a listing on the other, while `prepare`'s pre-byte check (archive digest only) is stricter. Both fail closed at open; record the intended per-entry override semantics in the governance contract.
- P4-06 NEEDS (High, integration) Nothing in this tree injects `PortalBundleResolver`, a `GovernanceView`, `PortalBrokerRes`, or writers of `AdmissionSignal`/`PortalBundleChanged` ordered `.before(apply_portal_admission)`. Every Open is therefore Unavailable, and no revocation can arrive. Expected by A-1/A-2, but it gates every runtime claim above.
- P4-07 RESIDUAL (Low) admission.md §8.5's `Access-Control-Allow-Private-Network` and the WebView2 permission branch are not built; they wait for SP-8. An operator `cors_origins = "*"` still opens every route to every origin (pre-existing behaviour).
- P4-08 PASS the summary read's `admission` appears only when the token's `client` claim equals the local DID (`sub` is always the node), so a remote token never learns this peer's admission.
- P4-09 PASS a race between preparing and installing is handled: signals that arrive while an open is in flight are replayed in `checked_install`, and governance re-judges before the mount is inserted.
- P4-10 PASS rows written through IPC actions stay after a revoke (steer 22:00): revoke closes the client and drops unsent calls, and never withdraws committed writes.

## W6 consolidated verdict

**PASS-WITH-FIXES, merge-blocked on four NEEDS.** Across R-P1..R-P4 the findings are: 5 fixed (P1-01/P4-01, P3-04, P3-05, P3-06, P4-02), 17 pass, 5 needs (P1-02, P3-01, P3-02, P3-03, P4-06), 4 residuals (P1-05, P1-06, P2-03, P4-07) and 8 deferred or noted. The security model holds as written: exact-origin admission in both layers, no-decode-before-authorization IPC, a host-minted binding, a digest-gated re-consent, and revocation inside one frame. The branch is not mergeable until:
1. SI applies P3-01 and P3-02 in `fe-ui/src/actions/mod.rs` (compile, and bundle chrome).
2. The orchestrator merges `track/extension-platform` first, for `fe_format::integrity::content_hash` (P1-02).
3. SI wires the Repository section call site (P3-03).
4. The binary injects the resolver, governance view, broker and signal writers (P4-06) before any native PJ journey can pass.

The refine pass must build and run R6/R9 before any of this counts as verified.
