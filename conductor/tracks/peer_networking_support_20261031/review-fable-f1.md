---
type: Review
track: peer_networking_support_20261031
gate: F1
model: fable
verdict: PASS-WITH-FIXES
---

# Gate F1 pointer: access control and advertisement hardening

The fable-tier F1 review did not run as a separate pass. Its scope (the P2, P3, P4 diff and all of W2) was covered by the security reviews R-NET-3 and R-NET-4 in `review-turn4/`, both PASS-WITH-FIXES. This file records that those findings were fixed or dispositioned after the gate.

Fixed after the gate:
- SEC-INBOX 7 (relay_server.rs breaker closes the listener on non-member denials): fixed in `1e83a398`, verified at HEAD in `fe-sync/src/relay_server.rs`.
- N3-16 (DialOnly advertised domains reach the peer-address allowlist unresolved): fixed by NET5-06. `fe-sync/src/sync_thread.rs:2165` `filter_resolvable` plus TTL cache, and the policy check at `:2447`.
- N4-25 (same DialOnly residual, from the endpoint side): narrowed by the same NET5-06 allowlist change; the DNS rebind window between verdict and iroh's own dial stays a documented residual (N2-09).

Still handed off (not part of F1 closure):
- N3-17 (SEC-INBOX 7 confirmation at relay_server.rs:852,1265): the fix is in `1e83a398`; the confirmation row is owned by the relay_server.rs owner.
- N3-19 (missing `review-fable-f1.md`): closed by this file.

Verdict: PASS-WITH-FIXES. Fixed-after-gate items above are in place at HEAD. The DNS rebinding residual (N2-09) remains a documented external risk because iroh owns the final dial.
