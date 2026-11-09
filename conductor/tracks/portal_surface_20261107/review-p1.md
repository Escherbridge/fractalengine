---
type: Review
date: 2026-11-08
track: portal_surface_20261107
gate: R-P1 (protocol)
reviewer: P-S6 (fresh context, opus)
verdict: PASS-WITH-FIXES
resource: ./plan.md
---

# R-P1: protocol review (`/security-review` + `/code-review high`)

**Scope.** S1 at `134d00c5`, plus the S1-owned lines that S2/S4 touched up to `eabcef17`: `fe-webview/src/portal_bundle/{mod,origin,mount,protocol,csp,navigation}.rs`, `backends/{tauri,qa_navigation}.rs`, `plugin.rs` (trust bar, dispatch), `ipc.rs`, `lib.rs`, `tests/{portal_protocol_journey,security_test}.rs`. Contracts: `docs/spec/portal-bundle/{protocol,README}.md`.
**Method.** Code-first desk review. Nothing was built or run (2026-11-08 directive). A fix is "applied", not "verified", until the refine pass builds it.

## Plan §4 checks

- P1-C1 PASS raw policy untouched: `fe-runtime/src/portal_url.rs` and `fe-webview/src/security.rs` have no diff since `cfd7fce6`; `security_test.rs::test_localhost_blocked` refuses all seven bundle spellings.
- P1-C2 PASS exact-origin admission in both layers: `decide_navigation` (`backends/tauri.rs:52`) and `qa_navigation::enforce` both call `NavigationGate::decide` first; `DeferToRawPolicy` falls through to their old logic unchanged.
- P1-C3 PASS traversal and encoding: `protocol.rs:49-53` never percent-decodes, `%` and `\` answer 404, and `is_member_path` refuses dot-leading and empty segments plus the `:` an F-3 rewrite leaves.
- P1-C4 PASS tampered members are never served: `read_member` checks every lazy read against the seal; an error answers 500 and faults the mount (Blocked: integrity on the next `poll`); `is_live` re-checks before responding (410).
- P1-C5 PASS service workers: any `Service-Worker` header answers 403 (`protocol.rs:35`); bundles stay Blocked without `ICoreWebView2_22` (P-1). SP-4 still unrun.
- P1-C6 PASS CSP from the vocabulary only: `csp::compose` takes `PortalCsp` + mode + feed sources and re-validates every source token; no script `'unsafe-eval'` / `'unsafe-inline'` path exists.
- P1-C7 PASS trust bar: `TRUST_BAR_JS` checks the same `[as]-[a-z2-7]{26}` grammar as `origin::parse`; the external text is unchanged.
- P1-C8 PASS unload token is one-shot: `AllowHostUnload` clears the slot; any new mint voids it (residual P1-05).

## Findings (one verdict per line)

- P1-01 FIXED (P0, compile) `portal_bundle/mod.rs` never declared S4's `admission` and `feeds` modules, which `plugin.rs`, `mount.rs`, `tauri.rs` and both journeys import. Added `pub mod admission;` and `pub mod feeds;`.
- P1-02 NEEDS (P0, compile, cross-track) `mount.rs:392` calls `fe_format::integrity::content_hash`, which exists only on `track/extension-platform` (`e8b3c9cf`). This branch compiles only after that branch merges first, so the orchestrator must set the merge order.
- P1-03 PASS `origin::split` refuses userinfo, ports, trailing dots, extra labels and uppercase. `https://fe-portal.x@a-….localhost` and `…localhost@evil` both fail the suffix or label grammar.
- P1-04 PASS `admits` decision table: rows 1-13 plus the frame rule. `Detached` and the host `Navigate` token close the gap between an unmount and the host's own replacement load (recorded additions).
- P1-05 RESIDUAL (Low) A page-initiated `about:blank` racing the host's unload can consume the one-shot token. The host's own unload is then refused, and the unmounted document keeps running hidden with no bytes, no IPC and no storage clear. P-4's fallback (Stop, controller Close, recreate) is not built; recorded in `portal_bundle/AGENTS.md` §residuals.
- P1-06 RESIDUAL (Low) The production wry callback has no `IsRedirected` (F-4). Rows 10-11 hold only on captured hosts. After a declared leave, a redirect hop to an undeclared https origin is judged by the raw policy alone. IPC is already detached by then, so this is not a data path; P-3 stays deferred.
- P1-07 PASS downloads: `allows_download` refuses bundle origins (`blob:` included) and any download while a bundle or detached document is current.
- P1-08 PASS headers: every response, errors included, carries the §4 set. The CSP value is built with `HeaderValue::from_str`; a failure is Blocked (F-1), never a response served without CSP.
- P1-09 DEFERRED (Low, perf) Each request inflates the whole member, including each media `Range` request. Two protocol workers bound memory at about 2 × 64 MiB, but seeking re-inflates. Revisit with a cached member when media-heavy bundles land.
- P1-10 PASS `tauri_commands.rs` deleted and its `lib.rs` line dropped (OD-9); nothing else referenced it.

## Verdict

PASS-WITH-FIXES. P1-01 is fixed here. P1-02 is a merge-order item for the orchestrator, not a code change in this tree. The residuals are documented in `fe-webview/src/portal_bundle/AGENTS.md`.
