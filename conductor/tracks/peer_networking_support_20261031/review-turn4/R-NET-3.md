---
type: Review
track: peer_networking_support_20261031
gate: R-NET-3
reviews: [NET-14, NET-15, NET-16, NET-23]
carries: NET-34 (Gate F1, part for these slices)
security_review: true
date: 2026-11-13
---

# R-NET-3: adversarial review of NET-14/15/16/23 (+ F1 security part)

Scope: `fe-runtime/src/relay_advertisement.rs` (NET-14), `fe-sync/src/relay_trust.rs` (NET-15), `fe-sync/src/relay_reconcile.rs` (NET-16), `fe-sync/src/endpoint.rs` (NET-23), plus the consumers they feed (`fe-sync/src/relay_advertise.rs`, `sync_thread.rs` reconcile loop, `fe-api/src/relay_advertise.rs`) read-only. Diff base `7af0a144`. Refute-first, fresh context. No cargo was run.

N3-01 FIXED - fe-runtime/src/relay_advertisement.rs:537 - (MEDIUM, closes N4-06) `verify_row_at` had no `issued_at <= now + 60 s` check (spec §4.4 item 5), so any reader other than the fe-sync consumer accepted a row issued a year ahead with a 30-day span. New `MAX_ISSUED_AT_SKEW_MS` and `AdvertRefusal::IssuedInFuture` ("issued_in_future", the name the consumer already logs); table row added.
N3-02 FIXED - fe-runtime/src/relay_advertisement.rs:518 - signatures were checked with `verify`, not `verify_strict` as the spec requires (OD-5): malleable signatures and small-order keys were accepted. Honest `sign_row` output is unaffected.
N3-03 FIXED - fe-runtime/src/relay_advertisement.rs:368 - (steer 21:40 rule 2, narrows N4-25) single-label hosts and `.local` / `.home.arpa` / `.internal` / `.lan` names only resolve on the operator's LAN yet passed as Public; iroh dials DialOnly domains with no resolution check. They are now Private reach: refused without `FE_SYNC_ALLOW_PRIVATE_RELAYS`, never home. Table rows added.
N3-04 FIXED - fe-sync/src/endpoint.rs:303 - (HIGH, AC-10 bypass on revoke) iroh's `MemoryLookup::add_endpoint_info` MERGES addresses, so a relay URL stored while allowlisted stayed in the book after its advert was withdrawn or its Manager revoked; and a relay stripped before the first reconcile was never restored when it became allowlisted. `set_relay_allowlist` now re-filters every stored peer from the relay URLs that peer named (`set_endpoint_info`), only when the set changed.
N3-05 FIXED - fe-sync/src/endpoint.rs:20,285 - the remembered relay URLs are peer-authored input: bounded at `MAX_REMEMBERED_RELAYS_PER_PEER` (8) per peer; lock order allowlist, then peer_relays, then the book, in both paths.
N3-06 FIXED - fe-sync/src/endpoint.rs:395 - the NET-23 test ended on a private-field read (`relay_allowlist.read().contains`), which cannot fail for a user; it now asserts the stored address gains the relay when allowlisted, loses it when revoked, and keeps the direct address.
N3-07 FIXED - fe-sync/src/relay_reconcile.rs:145 - spec §4.5 "Under Disabled, advertisements are inert unless pinned", but the planner listed DialOnly adverts as Known under `Disabled`, which put them on the peer-address allowlist: a relay-disabled operator would still be steered to a member's relay. Skipped now; journey row updated.
N3-08 KEPT (churn-resistant, both orders gameable within the 30-day window) - fe-sync/src/relay_trust.rs:38 - (N4-08) `select_bounded` keeps the oldest issued; spec §4.4 item 7 says newest. Newest-first lets any Manager displace the others by re-issuing with `issued_at = now + 59 s`; the consumer test at relay_advertise.rs:929 pins oldest-first. HANDED-OFF (C-NET / spec owner) to amend the spec text.
N3-09 KEPT (deviation consistent) - fe-runtime/src/relay_advertisement.rs:51,438 - signing domain is `fe-relay-adv-v1\0` over a length-prefixed body, not the spec's `fe-relay-ad-v1\0` + canonical CBOR; signer (fe-api) and verifiers (fe-sync) share this one module, the body is injective (length prefixes, presence tags, sorted dial-back JSON) and the format is flag-day. HANDED-OFF (I-DOC-NET) to record the real domain.
N3-10 KEPT (correct) - fe-sync/src/relay_reconcile.rs:131,186 - tripwire: the planner never inserts or removes a Configured URL (skipped before planning, excluded from removes); Default URLs leave only while a covered, inserted, public https relay is the Healthy connected home with auto-home on, and come back on any failure signal in the same pass (journey + 64-seed property). Adoption state lives only in the reconciler.
N3-11 KEPT (correct) - fe-sync/src/relay_trust.rs:20 - coverage is an intersection over every participating verse; an absent verse empties it, no verses yields none.
N3-12 KEPT (by design, AC-10) - fe-sync/src/endpoint.rs:226 - a node on a Custom relay strips a peer's n0 home relay (and vice versa) unless a Manager advertises it: such peers connect directly or not at all. HANDED-OFF (I-DOC-NET) to say so in §relay-trust.
N3-13 KEPT (residual stated) - fe-sync/src/endpoint.rs:276 - the filter guards the only address-book input (`presets::Minimal`, `MemoryLookup` only, no DNS/pkarr lookup); paths iroh already holds for a peer live in its own remote state until they age out. Input filter, not an egress allowlist (§relay-routing-contract).
N3-14 HANDED-OFF (I-SYNC-2) - fe-sync/src/lib.rs - `pub mod relay_trust;`, `pub mod relay_reconcile;` and `pub mod dialback_prober;` are missing although endpoint.rs:283, relay_advertise.rs:16 and sync_thread.rs:45-46 use them: fe-sync does not compile at HEAD, and the pinned seam export is unmet.
N3-15 HANDED-OFF (I-BATCH) - conductor/test-batches.md:274 - the R8 fe-runtime filter (`peer_registry distributed_query ...`) does not reach `relay_advertisement::tests::`; add `relay_advertisement` to it.
N3-16 HANDED-OFF (sync_thread.rs owner, R-NET-5; trigger: next touch of the allowlist push) - fe-sync/src/sync_thread.rs:2072 - DialOnly advertised domains go onto the allowlist unresolved; resolve them with the `advertised_relay_insertable` check first. Even then iroh owns the connect, so DNS rebinding between check and dial stays a residual (N4-25).
N3-17 HANDED-OFF (relay_server.rs owner, R-NET-5) - fe-sync/src/relay_server.rs:852,1265 - SEC-INBOX 7 confirmed by reading: every NotMember denial feeds the breaker and `CloseListener` takes the server down for all members.
N3-18 HANDED-OFF (I-DOC-NET) - fe-sync/src/AGENTS.md §relay-trust, fe-runtime/src/AGENTS.md §relay-advertisement - issued-at skew in the shared verifier, `verify_strict`, local-only names are Private reach, book re-filter on allowlist change (merge semantics), Disabled makes adverts inert.
N3-19 HANDED-OFF (NET-34 file owner) - conductor/tracks/peer_networking_support_20261031/review-fable-f1.md - outside this gate's files; F1 summary for these slices is the paragraph below.
SEC-INBOX 1: CONFIRMED-FIXED - validator side by R-NET-1 (R1-01..R1-04) and the REST side by R-NET-4; this gate adds local-only names (N3-03) and the issued-at skew in the shared verifier (N3-01).
SEC-INBOX 7: HANDED-OFF (relay_server.rs owner, R-NET-5) - see N3-17.

F1 (NET-34, this gate's part): signed adverts are verified strictly against the author DID's own key, bound to the verse, lifetime-capped and skew-checked; the URL policy refuses loopback, link-local/metadata, private, local-only names, http and non-443 ports for signed rows; tripwire grep finds no `default_access`, `open_to_all`, `AllowAll` or `RelayAccessToken` use in the four files beyond endpoint.rs's operator token at bind; Configured URLs are never removed; the peer-address filter now follows revocation. Residuals: DNS rebinding for DialOnly domains (N3-16) and paths iroh already holds (N3-13).

Verdict: PASS-WITH-FIXES
