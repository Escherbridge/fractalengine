---
type: Review
track: relay_operability_20261031
gate: R-NET-2
reviews: [NET-06, NET-07, NET-08]
security_review: true
date: 2026-11-13
---

# R-NET-2 — adversarial review of NET-06 / NET-07 / NET-08 (+ NET-12 share)

Scope: `fe-sync/src/relay_advertise.rs` (NET-06), the `__relays` arm and re-judge hook in `fe-sync/src/sharding.rs` (NET-07), `fe-sync/src/relay_config.rs`, `fe-sync/src/relay_reconcile.rs` and the reconcile/health hooks in `fe-sync/src/sync_thread.rs` (NET-08). Diff base `7af0a144`. No cargo was run; the orchestrator gate compiles.

R2-01 FIXED - fe-sync/src/relay_advertise.rs:356 - SEC-INBOX 3 / steer 22:10: the quarantine was one global FIFO, so one author could evict a Manager's advert queued before its role statement. It is now keyed verse -> author -> rows, one row per (verse, record, author); `RELAY_QUARANTINE_PER_AUTHOR_CAP` = 8 evicts the author's own oldest row; at `RELAY_QUARANTINE_CAP` the oldest row of the largest holder is evicted (ties: oldest). Evictions are counted (`quarantine_evicted`, `quarantine_author_evicted`), never logged per row.
R2-02 KEPT (dedupe key widened on purpose) - fe-sync/src/relay_advertise.rs:356 - steer 22:10 item 1 asks for one entry per (verse_id, record_id). iroh-docs keys are per author, so a key without the author would let a flooder overwrite a pending Manager row by reusing its record id. Dedupe is per (verse, record, author), which keeps the steer's intent.
R2-03 FIXED - fe-sync/src/relay_advertise.rs:561 - steer 22:10 item 5: the journey `relay_advertisement_book_journey` now covers a re-sent row replacing its pending copy, 130 flooding non-Manager authors past both caps, and the Manager admitted when its role arrives. No new test function.
R2-04 FIXED - fe-sync/src/sync_thread.rs:2209 - defence in depth for eviction: the empowerment re-read now also replays `__relays/` rows, so a promoted author's evicted advert comes back from the replica.
R2-05 FIXED - fe-sync/src/relay_advertise.rs:136 - `rejudge` drained and re-checked every quarantined row for every moved DID. It now looks only at the moved DID's own rows, which are the only verdicts that can change.
R2-06 FIXED - fe-sync/src/sync_thread.rs:1775 - steer 21:40 rule 2/5 at the one dial site this group owns: a pinned advert was put in the relay map (and could become home) with no reach check. `advertised_relay_insertable` re-validates it, needs `home_eligible()` (public https), resolves a domain host with a 5 s timeout and refuses any non-public resolved address.
R2-07 HANDED-OFF (NET-16 / R-NET-3: residual) - fe-sync/src/sync_thread.rs:1775 - iroh owns the relay connect, so the address checked at insert cannot be pinned. A DNS rebind between the check and iroh's dial stays open for Pinned relays. DialOnly (the default) never inserts.
R2-08 FIXED - fe-sync/src/sync_thread.rs:1740 - an insert that replaced an existing tokenless entry was kept and later removed by the reconciler, which deleted a relay map entry it did not own. Any collision now restores the replaced entry and forgets the insert, whether or not the entry had a token.
R2-09 FIXED - fe-sync/src/sync_thread.rs:1690 - open-verse change detection compared counts; closing one verse and opening another in the same pass left the refcount stale. It is now a set comparison and allocates only on change.
R2-10 KEPT (correct) - fe-sync/src/relay_reconcile.rs:57 - configured URLs are never planned (Insert or Remove). The Remove arm in sync_thread re-checks `configured()` as a tripwire. Remove only fires when no open verse advertises the URL.
R2-11 KEPT (correct) - fe-sync/src/relay_config.rs:301 - `advertised_relay_entry` builds the entry with `RelayConfig::new(url, None)` (iroh-relay 1.3.0: `auth_token: None`), with a tripwire assert. The token path `to_relay_mode_with_token` only covers Custom configured URLs. OD-10 holds.
R2-12 KEPT (correct) - fe-sync/src/sync_thread.rs:1856 - `__relays` rows reach `VerseFabric::note_entry` and return NotForwarded, so they never reach the DB thread. The role gate runs on `RowChange.author_id` = `author_did_key(entry.author())`, the SignedEntry author, at read time.
R2-13 KEPT (correct) - fe-sync/src/relay_config.rs:209 - `RelayAdvertisePolicy` is read only from the local env (`FE_SYNC_RELAY_ADVERTISE_POLICY`) and is never written to or read from a verse doc. Invalid input becomes DialOnly. Pinned under relays-Disabled becomes DialOnly.
R2-14 KEPT (correct) - fe-sync/src/sync_thread.rs:1623 - a token-gated advertised relay that denies auth becomes `has_error` (Degraded path) with a warn. It is never retried with the configured token.
R2-15 HANDED-OFF (I-BIN-2) - fe-api/src/network_ops.rs:19 - `advertisement_signer` is only ever `None`. When it is wired, it must be the same key as the iroh-docs author: the read gate refuses any row whose `advertised_by` is not the signing author's did:key.
R2-16 HANDED-OFF (NET-09 / status surface owner) - fe-sync/src/relay_reconcile.rs:109 - `RelayReconciler::known()` holds the DialOnly set but no dial allowlist or status surface reads it yet.
R2-17 DEFERRED (trigger: a relay settings change at runtime, or the embedded relay server joining the relay map) - fe-sync/src/sync_thread.rs:965 - the configured set is captured once at thread start from `relay_config`.
R2-18 HANDED-OFF (R-NET-4) - fe-sync/src/sharding.rs:1593 - steer 22:10 item 4: the pre-existing `__shards`/`petal` quarantine (`QUARANTINE_CAP`, global FIFO, `pop_front`) has the same flood-eviction shape. The re-read on empowerment softens it. This round did not add or touch it, and a test pins its FIFO behaviour.
R2-19 HANDED-OFF (I-DOC-NET) - fe-sync/src/AGENTS.md - §relay-advertise needs: per-author quarantine caps and why the dedupe key includes the author; `__relays/` in the empowerment re-read; the pinned-insert reach + resolution check and the rebinding residual; collision restore.
SEC-INBOX 3: CONFIRMED-FIXED - fe-sync/src/relay_advertise.rs:356 (see R2-01..R2-04).
SEC-INBOX 1: HANDED-OFF (R-NET-1 owns the validator and fe-api handler). The dial site this gate owns is fixed in R2-06, with the residual in R2-07.

Security review (fresh context, refute-first), checked as follows. Authz: the read-time Manager floor runs on the signed author, and forgery (`advertised_by` not equal to the author) is refused, not quarantined. Withdrawal: only the advertiser or a Manager+ may withdraw. Demotion withdraws and quarantines. Size caps: the quarantine is bounded globally and per author, and URLs are capped by the validator. Fail-closed: invalid policy becomes DialOnly, and insert failure or collision becomes forget+restore. Secrets: no token is ever logged or attached to an advertised entry. Replay: re-observing the same row is idempotent.

Verdict: PASS-WITH-FIXES
