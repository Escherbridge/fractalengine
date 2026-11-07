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
