---
type: review
track: role_propagation_20261013
phase: refine R2 (DB plane + API security)
base: d91afaf
reviewer: Claude (Opus 5.5, fresh context; refute-not-confirm)
date: 2026-10-25
verdict: FIXED
---

# Refine R2: DB-plane and API security review-and-fix

Scope: campaign diff `4d3a1a6..HEAD` on the DB plane and the API. Worklist: OF-4 verify,
the W6-0 open CreateAsset cap, RP-4s (never run as its own slice), and the W6 threat rows
TC-1..9, EL-1..3, RL-1..6. fe-sync was not reopened (R1 owns it). Gate:
`cargo check -j4 -p fe-database -p fe-api --tests` (exit 0, no warnings in the touched
crates). Tests were written but not run (FF rhythm; R5 sweep).

**Verdict: FIXED.** Six real findings, all fixed with a flow test each. Two are recorded
as accepted design with evidence. No CRITICAL. OF-4 was HIGH-class and now holds.

## Findings and dispositions

| # | Sev | Finding | Disposition | Fix / evidence | Test |
|---|-----|---------|-------------|----------------|------|
| 1 | HIGH (OF-4 class) | RP-4 restricted `orphaned_creator` to non-`did:key:` creators, but the first-manifest TOFU branch admitted ANY `created_by`. A peer publishing a placeholder-creator manifest handed the verse to every receiver as Owner at their next restart (the invitee-becomes-owner corruption, by another route). The restart test still carried the third-member workaround. | FIXED | `handlers/replicated_row.rs` `admit_inbound_row` step 3 denies a TOFU manifest whose `created_by` fails `reconcile::is_owner_identity`, the same predicate the rule uses, so the two can't drift. Workaround removed from `held_role_statements_survive_restart`. | `a_first_manifest_without_a_did_key_owner_is_denied_and_never_claimed`; `held_role_statements_survive_restart` (no extra member) |
| 2 | MED (AC-46 / TC-7) | `CreateAsset` (the W6-0 open item) was not capped: a token minted before the node's remote demotion kept registering anchored assets, whose rows replicate with device payloads. | FIXED | `lib.rs` CreateAsset arm: anchored → `require_node_role(Editor)` at the anchor petal scope, the one W5 helper (no second helper). An anchor that doesn't resolve is refused (fail closed); un-anchored rows never replicate. | `an_api_token_minted_before_the_nodes_demotion_cannot_write_after_it` extended with an upload before/after the demotion + read-back |
| 3 | MED (AC-42 / T23, RP-2 watch item) | The startup and replica-open re-emit ran a tight `try_send` loop into the 256-slot bridge in a deterministic order. Above ~256 live statements, the same tail (possibly revocations) was dropped on EVERY re-emit, so it never reached peers. | FIXED | `role_grants.rs` `publish_paced`: revocations first, async retry every 5 ms within a 2 s budget per call; past the budget the remainder is counted as drops plus a warn naming the dropped revocations. The deadline keeps the DB loop live if the bridge and the sync thread stall each other. | existing `a_pending_writes_flood_cannot_drop_a_local_revocation`, `a_revocation_issued_while_the_replica_is_closed_reaches_peers_after_restart` (both stay valid) |
| 4 | MED (EL-1 / EL-3) | Cap bypass from splitter/parser disagreement. `split_elevated_statements` doesn't model SurrealQL regex literals (`/…/`, real in surrealdb-core 3.0.5 `syn::parser::basic` `compound::regex`), so a `/'/` opens a "string" across real `;` boundaries. One listed entry then hides N statements from the 32-cap and from their `TIMEOUT 5s`. The DB-side check counted the API's list, so it trusted the API split (EL-3). The table whitelist and keyword bans still held: those scans are lexer-free. | FIXED | `elevated_query.rs` `statement_boundary_refusal`: SurrealQL's own parser (`surrealdb_core::syn::parse(...).num_statements()`) must equal the listed count, else refused before any work. New dep `surrealdb-core = "=3.0.5", default-features = false` (exact version `surrealdb` already uses, no features, so the engine was not rebuilt; Cargo.lock gains one edge). | `a_statement_hidden_from_the_api_split_is_refused_before_running` |
| 5 | MED (owner integrity) | `confine_verse` let ANY Editor fill a missing `namespace_id` once. That field picks the replica this node opens next, so an Editor could redirect the verse to a namespace they control. | FIXED | A namespace fill is now a governance change (`Floor::ManageVerse`): verse Manager+ only. | `only_a_verse_manager_may_fill_in_a_missing_namespace_id` |
| 6 | LOW (TC-1/TC-4 hardening) | `UploadOwner` had `pub` fields, so the "every door through `UploadOwner::new`" invariant held only by convention. | FIXED | Fields made private; `new` is the only constructor. Compile-checked: no other reader. | compile-enforced |
| 7 | LOW (T12) | Sybil grantors: `ROLE_PENDING_PER_GRANTOR` (1024) is per attested grantor and author ids are free, so an insider holding the namespace write secret can grow `role_statement_pending` without a global bound (disk). | NOT FIXED: accepted by design | plan.md:1316 (T12) mandates "a per-grantor budget and NO shared cap, so there is no cross-author eviction". A shared cap would let Sybils starve a legitimate future Manager's pre-held statements (T22). A flooder's rows are never scanned (indexed per-grantor promotion), so the impact is disk only, and the writer must already hold the write capability. | n/a |
| 8 | LOW (EL-2) | `FOR` block amplification: one top-level statement, many inner DML statements with no inner TIMEOUT. | NOT FIXED: verdict-only row (plan §4), verdict = keep allowed | Keyword scans are lexer-free, so a `FOR` ban would refuse any string containing the word "for". The endpoint is opt-in and needs an Owner token, which the mint handler cannot produce (`handlers/api_token.rs:54`). Recorded in fe-api AGENTS.md §elevated-query. | n/a |

## Verified clean (refuted, no finding)

- **OF-4 core**: `reconcile.rs` claims only non-`did:key:` creators that no foreign invite joined. Invites
  pin a decodable `did:key` owner (`handlers/invite.rs:158`). With #1 no replicated manifest can take the
  claimable shape. Startup order is reconcile → migration → repartition → re-emit (`lib.rs:549/584`).
- **P2P-REF-CONFINE / three-valued confinement**: `Located::{Here, Foreign, Unknown}` → Gate / Deny /
  Defer for every table. Durable-parent confinement covers re-parent and tombstone. `#`-bearing ids are
  denied (`is_scope_safe_id`).
- **Scope-precise admission**: `role_gate` resolves `role_at` at every gated scope (both scopes on a
  move), Manager+ at the verse for governance fields and `device_format`.
- **Enrolment floor (AC-44/T27)**: verified-first (Denied), then the manifest wait and the membership
  wait (Deferred). Membership counts `default_access` by design (§device-enrolment).
- **Role statement validator (T1/T3/T6)**: grantor == attested author, payload == key, scope verse ==
  replica verse, owner never a target (`fe-policy/src/role_grants.rs:180`).
- **T18 interim `VERSE#<hex>` rows**: `repartition`, `rematerialize`, `write_cache` and
  `migrate_legacy_role_rows` all filter on `verse_of_scope` / `is_interim_authority_verse_id`. Interim
  keys never parse to a hierarchy verse.
- **AC-46 doors**: `lifecycle_auth_context` (Identified → `min(token, node_role_at)`), plus
  `require_node_role` on IoT readings, device payloads, device formats, and now CreateAsset.
  CreateNode / CreateNodeWithAsset are NOT replicated by the DB thread (no `repl_tx` in their arms, and
  there is no NodeAdded→sync route), so they are not replicated-write doors. Elevated writes do replicate
  but are Owner-token-only, and an owner token cannot be minted (`api_token.rs:54`). Peers' A3 gate is
  the backstop.
- **TC-1** `client` is read only in `owner_key` (`upload.rs:148`); `caller_auth` / `api_token_context`
  keep `sub`. **TC-2/3** `asset_reach` anchor filter: unowned, `sub` or own key only. The multi-id call
  site (`get_asset`) only groups rows sharing one content hash, so the bytes are identical. **TC-4/5**
  `validate_client_did` (canonical ed25519 `did:key`, ≤128 bytes) runs at mint and at verify. Minting is
  GUI-only and Manager+. **TC-6** owner_did is stripped outbound (`device_payload.rs:643`) and inbound
  (`replicated_row.rs` anchored-asset apply). **TC-8** revoke is by `jti` + `sub`. **TC-9** EdDSA is
  pinned (`Validation::new(EdDSA)`) and `token_type == "api"`.
- **RL-1** `RelayAccessToken` has a redacting `Debug` and no Display or Serialize; the bind log carries a
  boolean only; client-side iroh 1.3.0 logs only `relay.url`. **RL-2/3** the token attaches only to
  `Custom` maps over https or loopback http; bracketed IPv6 fails closed. **RL-4**
  `sanitize_provider_addr` drops relays outside the node's map. Direct IP addresses are inherent P2P
  dialing, not a relay SSRF. **RL-5** the test relay is digest-pinned, `--rm`, and bound to
  `127.0.0.1`. **RL-6** `fe-sync/src/AGENTS.md:692-702` recommends `access.shared_token`.

## Notes for R3/R4/R5

- New tests are rustfmt-unchecked (R4 runs fmt). `fe-database` gained a direct `surrealdb-core` edge
  (Cargo.lock, one line).
- Sweep triage hint: an elevated script that relied on a `/…;…/` regex literal is now refused (it was
  corrupted by the split before). A re-emit against a stalled bridge now takes up to 2 s instead of
  dropping instantly.
