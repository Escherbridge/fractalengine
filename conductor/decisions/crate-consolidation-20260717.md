---
type: Decision Record
title: Crate Consolidation Audit — fe-auth absorbed, fe-network staged, KEEP verdicts
timestamp: 2026-07-17T00:00:00Z
status: recorded (executed items done; stage-2 fe-network retirement pending D-71)
tags: [decision-record, crates, workspace, consolidation, oss-release]
---

# Decision Record: Crate Consolidation (2026-07-17)

Source: 2026-07-17 audit of all 23 workspace members (verdicts F1–F9), run as part of
OSS-release preparation (see track `oss_release_20260717`). Two vestigial crates were
identified; one was absorbed same-day, one got a zero-risk stage-1 prune with its
stage-2 retirement gated on register entry **D-71** (unratified — not settled). All
other headline merge candidates resolved to KEEP on concrete structural grounds,
recorded below so the questions aren't re-litigated.

## Executed 1 — fe-auth ABSORBED into fe-database (F2)

fe-auth (350 lines, 6 files) had exactly one external consumer:
`fractalengine/tests/security_harness.rs:51` via fractalengine's `[dev-dependencies]`.
Absorbed and deleted 2026-07-17:

- `SessionCache` moved verbatim (incl. `SESSION_TTL_SECS = 60`) from
  `fe-auth/src/cache.rs` to `fe-database/src/session_cache.rs` — new path
  `fe_database::session_cache::SessionCache`; `security_harness.rs:51` re-pointed.
- The revocation placeholder-signature site moved along unchanged:
  `fe_database::session_cache::revoke_session` stamps `sig: "00".repeat(64)`
  (fresh grep: `fe-database/src/session_cache.rs:60`) with its deferred
  `BroadcastRevocation` comment intact. The 13-site placeholder register stays at 13 —
  the fe-auth entry just changed address (register maintained in
  `oss_release_20260717/spec.md` FR-2).
- fe-auth's cache unit tests preserved as a `#[cfg(test)]` module in
  `session_cache.rs`.
- `handshake.rs` / `session.rs` / `verse_invite.rs` deleted — verse_invite superseded
  by the live `fe-database/src/invite.rs` `VerseInvite` (confirmed present);
  `VerseRevocation` had zero consumers.
- Crate removed from root workspace members and from fractalengine
  `[dev-dependencies]` (its only dependent); `Cargo.lock` regenerated with zero
  fe-auth entries.
- Load-bearing rationale recorded in `fe-database/src/AGENTS.md` §session-cache
  (TTL bound, log-first revocation, placeholder-sig register, Sprint-5B deferral).
- Verification: `RUST_MIN_STACK=134217728 cargo check -p fractalengine --tests`
  finished clean in 1m 57s (granted command), no warnings/errors.
- Residual: one stale comment mentioning fe-auth at
  `fe-webview/src/petal_portal.rs:56` — RESOLVED 2026-07-17 (repair pass): the
  `Role` doc-comment now points at `fe-database`/`fe-policy`.

## Executed 2 — fe-network stage-1 dep prune (F1 stage 1)

Grep of `fe-network/src` confirmed zero `use iroh*` / `iroh_blobs::` /
`iroh_gossip::` / `iroh_docs::` references — the same-named local modules are log-only
Sprint-5B stubs. The three never-imported deps (`iroh-blobs`, `iroh-gossip`,
`iroh-docs`) were removed from `fe-network/Cargo.toml`; libp2p and the crate itself
untouched. Zero-risk, executed 2026-07-17.

## Pending — fe-network stage-2 retirement (F1 stage 2, gated on D-71)

**UNRATIFIED (register D-71, [USER]).** The candidate move: relocate `AssetId` (+
`GossipMessage`) into `fe-runtime::messages` (already the cross-thread type hub),
retire the libp2p kademlia swarm + network thread (it only answers Ping/Pong), delete
the crate, and drop libp2p 0.56 from the entire workspace build. Mechanical surface:
3 fe-renderer files, `fe-sync/src/cache.rs`, both binaries' spawn call, two test
files, and the 7-thread architecture docs. Counter-consideration: foundry-candidate
specs (hexon_p2p_bucket "handshake-then-swarm", p2p_mycelium_completion) might intend
a swarm revival, though the board's live P2P direction is iroh. Do not execute until
D-71 resolves.

## KEEP verdicts (rationale on record)

- **fe-entity-store (F5)** — KEEP separate. Merging into fe-database creates the
  activated cycle fe-query → fe-database → fe-query (fe-query's parquet/datafusion
  features dep on fe-entity-store; fe-database deps on fe-query; fe-api activates
  fe-query/datafusion). Merging into fe-query instead would contaminate the BI-egress
  backbone with bevy. Also deliberately a serde-light in-memory hot cache with
  documented O(K) invariants that must not absorb surrealdb.
- **fe-sdk / fe-plugin (F4)** — KEEP BOTH. fe-sdk must stay serde-only: it is the
  plugin-author-facing stable ABI consumed by fe-runtime, fe-terrain, fe-ui,
  fe-plugin-test; folding it into fe-plugin would push wasmtime 29 + rhai + bevy into
  that closure.
- **fe-plugin-test (F8)** — KEEP as a separate published test-utils crate for plugin
  authors (conventional `*-test-utils` shape). Into fe-sdk: feature-unification leaks
  rhai into fe-sdk's consumers. Into fe-plugin: third-party authors pull wasmtime +
  bevy just to run tests. Matters for OSS plugin-author onboarding.
- **fe-hexon-registry (F7)** — KEEP. Zero internal runtime deps (parses manifest.json
  as raw `serde_json::Value` by design); its isolation lets the Docker image build
  without the engine closure and lets the roadmap's hexon-foundry extraction lift the
  crate wholesale. Manifest-shape changes route through hexon_unification_20260716,
  not consolidation work.
- **fractalengine-relay (F9)** — KEEP the thin-binary + shared-libs pattern. A binary
  in fe-runtime inverts layering; folding into the fractalengine package forces the
  headless Docker image to compile the GUI closure (fe-ui, fe-webview/wry,
  fe-renderer, bevy_egui). The real headless compile cost is workspace bevy
  feature-unification — a build-config concern, not crate consolidation.
- **fe-identity / fe-policy (F6)** — KEEP separate: incompatible dependency
  contracts. fe-policy is deliberately no-I/O / no-Bevy / serde-light and is the
  canonical RoleLevel home for fe-database, fe-webview, fe-sync, fe-hexon, fe-plugin;
  fe-identity carries bevy, iroh, keyring, jsonwebtoken. Merging drags Bevy+iroh into
  every policy consumer and violates the auth_policy_pattern track's
  unit-testable-without-I/O criterion. The consolidation win in this trio was
  retiring fe-auth (Executed 1).
- **fe-format / fe-runtime edge (F3)** — KEEP the crate, FIX the edge, but ROUTED
  THROUGH `hexon_unification_20260716` (which owns fe-format/fe-hexon restructuring)
  — no parallel workstream. The single `impl From<fe_runtime::messages::NodeDto> for
  ExportNode` is all that drags the Bevy closure into the lean format crate;
  candidate fix is relocating the conversion to fe-api.

## Stale known-issue struck

"fe-plugin should depend on fe-sdk (currently parallel type definitions)" is
RESOLVED — fe-plugin/Cargo.toml already deps fe-sdk and `fe-plugin/src/lib.rs:47-51`
re-exports the canonical fe-sdk types; the remaining concrete-structs-vs-traits pair
is documented intentional layering. Remove the entry from any known-issues list so
planning doesn't re-schedule solved work.
