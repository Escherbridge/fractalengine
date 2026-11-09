---
type: Review
date: 2026-11-08
track: portal_surface_20261107
gate: R-P2 (IPC)
reviewer: P-S6 (fresh context, opus)
verdict: PASS
resource: ./plan.md
---

# R-P2: IPC bridge review (`/security-review`)

**Scope.** S2 at `382f9d7d` (and S4's later lines in the same files): `fe-webview/src/portal_bundle/{ipc,limits}.rs`, `portal-v1.js`, `backends/tauri.rs` (`install_ipc_handler`, `post_page_message`, show/hide), `plugin.rs::pump_portal_ipc`, `tests/portal_ipc_journey.rs`. Contracts: `docs/spec/portal-bundle/ipc-v1.md`, sibling `docs/extensions/bus.md` §8 (A-1).
**Method.** Desk review; nothing built or run.

## Plan §4 checks

- P2-C1 PASS authorization before decode: `BridgeState::ingress` runs checks 1-3 on raw strings. Those are: Source ≤ 8 192 bytes, then `origin::parse`; a live, unfaulted Admitted mount; visible; the constant-time token prefix; id digits; size; message bucket; pending cap. serde runs only in `process_inbox`, with `deny_unknown_fields`.
- P2-C2 PASS no reply to unauthorized senders: checks 1-2 return `Dropped`, count `dropped_origin` and queue nothing. External sites, static mounts, faulted mounts, hidden portals and stale generations all land here.
- P2-C3 PASS document clock is not the capture clock: `PortalMounts.generation` bumps only on admitted top-frame navigations and unmounts. `reposition` bumps the fixture clock only.
- P2-C4 PASS binding host-minted: `bind` builds `BindRequest` from the `Mount` alone. Capabilities are the manifest's declared set ∩ the broker's `granted`. Page data never names the node, petal, package or identity.
- P2-C5 PASS budgets: 50 msg/s, 16 live subscriptions, deltas paced at 250 ms per subscription with last state winning, `set_property` 2/s burst 5 (20 in 1 s gives at most 7), `invoke` 5/s, `intent` 2/s, and emit capped at 10 Hz. The buckets are per mount and survive reloads.
- P2-C6 PASS delivery gated by generation plus envelope token: `take_outbox` re-checks visible + live Admitted mount + generation at send time, and `portal-v1.js` drops any envelope whose `n` is not its token.
- P2-C7 PASS no variable `eval`: host→page is `PostWebMessageAsJson` of serde JSON, and `evaluate_script` appears only with the fixed `history.back()`, `history.forward()` and `location.reload()`.

## Findings (one verdict per line)

- P2-01 PASS SP-9: wry's `with_ipc_handler` is never registered. The native `WebMessageReceived` handler reads `Source` and drops anything over the P-2 cap before parsing it.
- P2-02 PASS `key_allowed` keeps `node_kind` and `portal.*` host-reserved under any declared pattern, so a bundle can never repoint its own repository node. A bare `*` pattern matches nothing.
- P2-03 RESIDUAL (Low) SP-3 is unrun. An envelope already queued in WebView2 just before a declared leave may still reach the new page. The send-time gate and the page-API token check ship regardless; recorded in `portal_bundle/AGENTS.md` §residuals.
- P2-04 PASS document tokens: 128 bits from BLAKE3 keyed by a per-process secret (OS-randomized SipHash keys plus `CoCreateGuid`), one per (mount, generation). `Cache-Control: no-store` and CORP same-origin keep `portal-v1.js` unreadable from other origins.
- P2-05 PASS lock order: neither the protocol workers nor the native handler hold the mount `RwLock` while taking the bridge mutex (`ipc::serve` reads the generation before `document_token`), so no inversion.
- P2-06 PASS node scope: a `node` subscription with a `node_id` waits for the broker's petal-scoped snapshot to prove the node. A missing node answers the same `not_found` as REST, and deltas are filtered to that node.
- P2-07 NOTE (Low) Close, leave and re-open tear down without `bridge.shutdown()`. The same frame's `pump_portal_ipc` (ordered after `dispatch_commands`) sees the mount gone and closes the broker client, so nothing outlives the frame. Revoke calls `shutdown()` directly.
- P2-08 PASS `IpcError` text is fixed per code and never echoes page input. Audit targets are truncated to 128 bytes and rate-limited.

## Verdict

PASS. No S2 code change was needed. The only S2-file edit in this pass is the `portal_ipc_journey.rs` CSP assertion that follows R-P4's P4-02 fix.
