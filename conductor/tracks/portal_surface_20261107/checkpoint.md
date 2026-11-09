---
type: ProgressCheckpoint
date: 2026-11-07
status: pending
title: portal_surface_20261107 — decisions, steer channel, session bus
track: portal_surface_20261107
resource: ./spec.md
---

# Checkpoint: portal_surface_20261107

## DECISIONS

These decisions are binding. They were ratified in the 2026-11-07 user grills; do not re-litigate them (`.omc/state/bus/SESSION-PROTOCOL.md` rule 11). The full text is in [spec.md](./spec.md) §4.

- **D-X5 — Bundle trust.** A web bundle is a signed package in the same sealed `.hexon` family, with a web content type.
  - The extension governance admits it **per peer** (trust roots, per-verse policy gated by RBAC roles, consent, kill switch) before it gets the IPC data/interaction API.
  - Unsigned or unadmitted bundles still render, but in a no-IPC static sandbox: docs, and read-only dashboards fed only by the public REST.
  - Replicated rows never auto-execute on arrival.
- **D-X6 — Data path.** Host-brokered IPC: the webview gets a small typed API over the IPC bridge.
  - `subscribe(query | readings | node)` streams changes from the same handlers REST uses.
  - Actions (`set_property`, invoke an extension command) go back through the capability manifest and are judged like any write (role at write time).
  - The integration bus is defined in `extension_platform_20261107`. Through it, extensions expose streams and commands and consume hooks via the host broker, and portals act as consumer and producer.
- **D-X7 — Web reach.** Wave 1 renders bundles in-app from the local blob store, through a custom protocol.
  - "Through the web" comes later: a static export of the same bundle, talking to the public REST with a scoped viewer token.
  - Hosting is out of scope.
- **D-X8 — Ownership.** This is its own public track, with its own session (EXT-PORTAL; worktree `.omc/worktrees/ext-portal`; branch `track/portal-surface`) and a dedicated QA lane. Cross-session coordination goes through `.omc/state/bus/README.md`.
- **Shared decisions:**
  - **D-X1:** third-party packages are sealed `.hexon` packages (`HexonType::Plugin` family) carrying Wasm components against WIT `fractalengine:plugin@2.0`. They are signed and checked against trust roots, with a package-level kill switch.
  - **D-X2:** sessions are author-only (specs, docs, journeys as text, scenario JSON) until the build gate flips `cargo_allowed=true`.

## ORCHESTRATOR STEER

*Binding. Dated bullets, newest first, maintained by the orchestrating session. Agents re-read this at every phase boundary and apply it over any other instruction except safety rules.*

- 2026-11-09 02:15 (orchestrator fractalengine-71, mandatory integration for R-POR) — fe-api in this tree
  does not compile: fe-api/src/portal_bundles.rs `origin_allowed` uses `url::Url` but fe-api/Cargo.toml has
  no `url` dependency (add it with the workspace/fe-runtime version). The QA lane's
  `POST /api/v1/debug/portal/killswitch {engage}` route is missing (fe-api/src/qa_portal.rs has no owner this
  turn: R-POR mounts it in server.rs and lists the qa_portal.rs handler as NEEDS; the orchestrator writes the
  handler after the turn if it is still absent). Keep the route behind the existing portal-fixture
  Owner+input gate.
- 2026-11-08 23:40 (orchestrator fractalengine-71, SECURITY relay, mandatory) — **bus `Caller` gains
  `role_ceiling`** (sibling G1-C1, `3e946344` on track/extension-platform): every API token now carries the
  node DID as `sub`; "local" = node DID + an Owner token; the bus `Caller` struct gains `role_ceiling`, so
  every `Caller { .. }` literal in this tree (portal IPC broker routing, fixture ops, probes) must set it —
  a portal action's ceiling is the installing identity's role at the bundle's scope, never above the
  viewer's token. P-S4/P-SI/S6 own the touch points; S6 verifies.
- 2026-11-08 22:00 (USER DECISIONS, binding) — **Kill list:** a kill-listed bundle is refused by default,
  with an explicit, audited verse-Owner override for that Owner's own peer (same rule as extensions); on
  revocation an open mount unloads, but rows written through the bundle's IPC actions STAY (no withdrawal,
  OD-E1). **Scoped extension bindings** (new feature, contract `docs/extensions/bindings.md` in the
  sibling tree): a verse/fractal/petal binds an installed extension or bundle to a hook point, nearest
  scope wins, Manager+ sets it, resolution is per peer with built-in fallback and a consent install offer.
  Portal/web operations are one hook family: a petal may bind its portal surface. Keep S1-S4 compatible
  with a bound bundle being chosen per scope; the binding table itself is the next swarm turn.
- 2026-11-08 20:45 (orchestrator fractalengine-71) — **answers to SD's A-13/A-14/A-16** (orchestrator
  defaults, user may override): **A-13 yes** — a portal action is a one-op commit on the extension write
  path outside any tick: one DB command, host-stamped provenance `{package_id, content_hash}`, judged by the
  installing identity's role at write time like any editor write (D-X6), subject to the identity's shared
  rate limits; no tick budget applies. **A-14 yes** — an admitted bundle is an installation of kind
  `web-bundle` with no instance, using the sibling R11 status vocabulary. **A-16 yes** —
  `host/admission` carries `{package_id, content_hash, state, reason}`. Drop the "pending EXT-PLATFORM"
  markers on these three.
- 2026-11-08 20:45 (orchestrator fractalengine-71, SECURITY, mandatory) — **kill-list matching is by hash
  under any id**, the same rule as docs/extensions/package-format.md in the sibling tree (revocation bypass
  by renaming, steer 20:10): an entry matches a bundle when its `content_hash` equals the bundle's
  `content_hash` OR its archive/member digest; the entry's `package_id` is informational and never narrows
  the match. Replace admission.md's exact-pair match. Residual s4.6 narrows to genuine repackaging (new bytes,
  new signature), which only publisher-key revocation catches; say so.
- 2026-11-08 20:45 (orchestrator fractalengine-71) — **store-client contract is now v1** (the marketplace
  review, 18 fixes). Deltas that touch this track: `content_hash` = lowercase-hex BLAKE3 of the canonical
  manifest with the signature blanked (it keys the kill list); a kill-listed package is refused at install as
  well as at render; leaving the index (takedown, all versions yanked) is NOT revocation; clients send no
  credentials to the catalogue base or any URL a catalogue document names; signed documents use integers
  only; a kill list or index older (issued_at) than the cached one is ignored.

<!-- no steers yet -->

## SESSION BUS

- **Protocol.** Read `.omc/state/bus/SESSION-PROTOCOL.md` first, then `.omc/state/bus/README.md`. Read every bus file at session start and at each milestone (phase end, commit, blocker).
- **This session is EXT-PORTAL.**
  - Its bus file is append-only: `.omc/state/bus/EXT-PORTAL.md`.
  - Entry shape: `- YYYY-MM-DD HH:MM EXT-PORTAL <milestone|blocker|decision|question|steer|building|ready-to-merge> - text`.
  - It works in worktree `.omc/worktrees/ext-portal` on branch `track/portal-surface`.
  - It never pushes and never merges into main. Post `ready-to-merge` instead; the orchestrator (P2P-QA, main tree) reviews and merges.
- **Build gate.** The gate is `.omc/state/build-gate.json` in the main tree; at authoring it read `cargo_allowed: false` (D-X2).
  - Work AUTHOR-ONLY until P2P-QA posts "gate open".
  - After that, post `building EXT-PORTAL <what>` before any build, and never build while another session's `building` entry is open.
- **Upstream signal.** Watch for EXT-PLATFORM's `milestone` carrying the WIT 2.0 + integration-bus contract.
  - It unblocks P2.
  - P4 also waits on the sibling's governance.
  - Until then, P2 is written against plan.md assumptions A-1 and A-2, marked as such.
- **Open questions to post on the bus:**
  - OD-3: ask EXT-PLATFORM for change-triggered `host/query` re-runs.
  - OD-11: ask the orchestrator for the product.md / tech-stack.md clarifying lines.

## SESSION NOTES

<!-- the EXT-PORTAL session's own notes, dated -->

## STATUS

- 2026-11-07: track drafted (author-only). No phase started. Next: W0 AUTHOR-ONLY slices SD, SU and SQ (plan.md §5).
