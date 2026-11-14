---
type: Review
track: portal_surface_20261107
gate: C-EXT
round: 4
reviews: [T-EXT, R-EXT-4, R-INT-2, I-DOC-API, I-BATCH-1]
checked_at: b817b97e (plus the uncommitted working tree)
date: 2026-11-13
---

# C-EXT: portal_surface_20261107 completeness at round-4 HEAD

Spec requirements R1-R17 (§7), plan tasks (§7) and this track's round-4 items (EXT-05, EXT-06, EXT-07) checked
against `b817b97e`. Nothing was built or run. R1-R15 and R17 are unchanged since `completeness-turn3.md` (all
complete in code, evidence there); this file records what round 4 changed and re-checks the open rows.

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names a portal path (`fe-webview/src/portal_bundle/**`,
`fe-api/src/qa_portal.rs`, `fractalengine/src/qa_portal_bridge.rs`, `fe-runtime/src/qa_portal.rs`, `tools/qa/sim/driver.py`,
`fe-webview/examples/portal_spikes/**`). R-EXT-4 recorded the same.

## Round-4 items

| Item | Status | Evidence |
|---|---|---|
| EXT-05 PJ2 seam `portal_ipc_post` | complete (code) | rows `fe-webview/src/portal_bundle/qa_rows.rs:21` `IpcPostRow`, `post_row` `:79` (sender build for unsigned/unadmitted, R-EXT-4 R4-01), module `portal_bundle/mod.rs:12`; wire enum `fe-runtime/src/qa_portal.rs:28,34`; handler `fe-api/src/qa_portal.rs:419` behind `QA_GATE.admit` + Owner + 256-byte limit, route `fe-api/src/server.rs:408` (R-INT-2 KEPT-verified); bridge `fractalengine/src/qa_portal_bridge.rs:81-122,251`; driver `tools/qa/sim/driver.py:1038-1044` (no longer `portal_op_unwired`); journey `fe-webview/tests/portal_ipc_journey.rs:595` (`test-batches.md:220`). Docs not updated: H4-P1. Scenario pins: H4-P3. |
| EXT-06 spike harness part 1 (SP-1..SP-4) | complete (unrun) | `fe-webview/examples/portal_spikes/main.rs` (dispatch `TABLE` `:21`), `spikes_a.rs:14-29`; how to run `fe-webview/src/portal_bundle/AGENTS.md:188`. |
| EXT-07 spike harness part 2 (SP-5..SP-9) | complete (unrun) | `spikes_b.rs:24-44`, SP-9 child process `main.rs:46-49` (parent never aborts), loopback-only servers (R-EXT-4 R4-10). Compile confirmed only at the wave-barrier `cargo check -p fe-webview --examples` (R4-11). |

## Requirements (spec §7)

| Req | Status | Evidence |
|---|---|---|
| R1-R15, R17 | complete (code) | unchanged; `completeness-turn3.md` table. |
| R8 IPC ingress authorization | complete | the PJ2 rows now have a driver seam (EXT-05); `ipc.rs:431` `ingress`, `:464` `stats`. |
| R16 QA lane | partial (runs blocked-user) | code complete: PJ2 seam added (EXT-05), WebView2 spike harness for T1.2 (EXT-06/07), scenarios `tools/qa/sim/scenarios/portal-{core,replication,load}.json`. Still a run: `run.py validate portal-*`, PJ1-PJ9 receipts, spikes on WebView2. `portal-core.json` PJ2 pins need one fix before that run (H4-P3). |

## Plan tasks (plan.md §7)

| Task | Status | Evidence |
|---|---|---|
| T0.1-T6.2, D, R-P1..R-P4 | complete | ticked; T3.2 `plan.md:292`, T6.2 `plan.md:307` (turn-3 C-POR-04 reached). |
| T1.2 spikes SP-1..SP-9 | blocked-user | harness code done, `plan.md:285-286`; run needs the user's WebView2 box. |
| T2.3 readings broadcast | obsolete | annotated not applicable, `plan.md:299-300` (OD-13 fallback never triggered; readings reach portals over bus `host/readings`). |
| T5.5 PJ1-PJ9 + sim scenarios | blocked-user | `plan.md:304`. |
| full sweep | blocked-user | `plan.md:306`. |

## Hand-offs: did each reach its owner?

- Turn-3 C-POR-03 (broker role source doc): reached, `fractalengine/src/AGENTS.md:1869`.
- Turn-3 C-POR-04 (plan ticks): reached (rows above).
- R-EXT-4 R4-02 (ipc.rs owner, next EXT round) and R4-03 (portal-core.json owner, step 4): both were addressed to the next round; carried below.
- EXT-05's doc NEEDS (routed to I-DOC-API): NOT applied. `tools/qa/portal/AGENTS.md:49` still says "no seam yet: refused as `portal_op_unwired`", and `fe-webview/src/portal_bundle/AGENTS.md` does not mention `qa_rows`.

## HANDED-OFF (owner = next round)

- H4-P1 (I-DOC-API next round) `tools/qa/portal/AGENTS.md:49`: `portal_ipc_post` is wired (route `/api/v1/debug/portal/ipc-post`, rows include `unadmitted`, optional mount pins, receipt `{ipc_post: {row, dropped_origin_delta, delivered, replied}}`); add a `qa_rows` paragraph (fixed origins per row, no caller-supplied JS/URL/body) to `fe-webview/src/portal_bundle/AGENTS.md`.
- H4-P2 (fe-webview/src/portal_bundle/ipc.rs owner) R4-02: add `PortalBridge::is_visible()` (or a `with_hidden` scope) so the `hidden` row restores the prior visibility instead of always re-showing (`qa_rows.rs:124`). LOW: Owner + input opt-in debug op only.
- H4-P3 (portal QA lane, `tools/qa/sim/scenarios/portal-core.json` owner) R4-03: `r_frame`, `r_hidden`, `r_stale`, `r_control` (`:432`) pin `qa.portal.actions` but no step opens that bundle first; add the Actions repo-node open after `adm_act`, or drop the pins, before the T5.5 run.

## Excluded by the scoper (not gaps)

- blocked-user: T1.2 spike runs on WebView2; T5.5 PJ1-PJ9 + sim scenarios, R16 `run.py validate portal-*` receipts, full sweep; R-Z-16 / R-BIN-21 (no writer of `BrowserCommand::OpenBoundBundle`, `fe-webview/src/ipc.rs:17`; the binding reload watcher never fires; §9 q18 default node-only holds); portal deferred R-POR-IPC-21, R-POR-IPC-22, P1-09, P3-10, P3-11.
- obsolete: T2.3 readings broadcast (OD-13 fallback).

Verdict: CODE-COMPLETE - R1-R15 and R17 complete, R16 partial only by its user-gated runs; EXT-05/06/07 complete in code; 3 HANDED-OFF (docs, one LOW debug-op fix, scenario pins).
