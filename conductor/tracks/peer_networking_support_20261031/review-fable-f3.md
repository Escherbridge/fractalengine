---
type: Review
track: peer_networking_support_20261031
gate: F3
model: fable
verdict: PASS-WITH-FIXES
---

# Gate F3 pointer: full networking security review

The fable-tier F3 review did not run as a separate pass. It ran as R-NET-6 in `../p2p_asset_streaming_20260718/review-turn4/R-NET-6.md`, PASS-WITH-FIXES, and fails closed for the relay start.

Status of R-NET-6 items at HEAD (read in round 5):
- R6-05 FIXED by NET5-02. `fe-sync/Cargo.toml:12-13` declares `net-report` (default on).
- R6-07 FIXED. `fe-ui/src/settings.rs:237` `mirror_self_test_verdict`, registered in `fe-ui/src/plugin.rs:892-899` (I5-UI).
- R6-08 and R6-09 FIXED in the gate. `fractalengine/src/relay_serve_bridge.rs:49` split, and `:97` refuses to `block_on` inside a runtime.
- R6-11 FIXED by NET5-03. `fe-sync/src/relay_server.rs:1469-1510` buffers the latest admission snapshot; `install_gate` at `:1477` applies it when the gate is installed late.
- MCP reachability FIXED by NET5-05. `fe-api/src/mcp/mod.rs:312` lists `get_network_diagnostics`; `fe-api/src/network.rs` uses `try_send`.
- SEC-INBOX 15 (`fractalengine/src/main.rs` hosted set): STILL-FIXED per R5-NET-2.
- R6-10 STILL OPEN. Verified at HEAD: `fractalengine/src/relay_serve_bridge.rs` `decide()` still requires `dialback_passed` before `Start`, and the dial-back needs the relay to listen first. The suggested fix (start on feature, consent, `public` verdict and `FE_RELAY_SERVE=1`, then request dial-back, drop the handle on failure) is not in the code. Handed off to a next-round NET slice.
- R6-17 and R6-18 HANDED-OFF to the batch owner (test-batches filter names).

Verdict: PASS-WITH-FIXES. The relay start stays unreachable until R6-10 lands. In-app and batch evidence is user-gated.
