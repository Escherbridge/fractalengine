---
type: Review
track: peer_networking_support_20261031
gate: F2
model: fable
verdict: PASS-WITH-FIXES
---

# Gate F2 pointer: gossip protocol review

The fable-tier F2 review did not run as a separate pass. Its scope was covered by R-NET-5 in `review-turn4/`, PASS-WITH-FIXES. Findings N5-01 to N5-07 were fixed at the gate (`fe-sync/src/gossip_auth.rs`, `distributed_query.rs`, `sync_thread.rs`, `dialback_responder.rs`, `net_diagnostics.rs`).

Fixed after the gate:
- SEC-INBOX 13 (dial-back responder: first-message binding): fixed in `1e83a398`, `fe-sync/src/dialback_responder.rs`.
- SEC-INBOX 14 (dial-back prober: resource cap bypass, DNS and answer steps time-bounded): fixed in `1e83a398`, `fe-sync/src/dialback_prober.rs`.

Still open:
- N5-08 (HANDED-OFF, fe-api/src/network.rs owner): `net_diagnostics.rs:170` node id text. Not closed by NET5-05.
- N5-12, N5-17 are DEFERRED with triggers recorded in R-NET-5.

Verdict: PASS-WITH-FIXES. Gossip signing stays in compat mode until every declared peer has latched (plan section 9 default).
