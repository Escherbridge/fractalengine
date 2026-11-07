---
type: ProgressCheckpoint
date: 2026-11-07
status: pending
title: Extension platform — decisions, steer and session bus
track: extension_platform_20261107
resource: ./metadata.json
---

## STATUS

- 2026-11-07: spec, plan and partitions are authored (docs only; the build gate is closed). No code yet. Next: the EXT-PLATFORM session opens P0, which is AUTHOR-ONLY.

## DECISIONS

Ratified in the user grill on 2026-11-07. Do not re-litigate (`.omc/state/bus/SESSION-PROTOCOL.md` item 11).

- **D-X1 — package + ABI.** Third-party extensions ship as sealed `.hexon` packages (`HexonType::Plugin`) carrying a Wasm component against WIT `fractalengine:plugin`, bumped from 1.0.0 (`fe-plugin/wit/hexon-plugin.wit`) to 2.0. Native Rust via `fe-sdk` stays first-party only; Rhai stays for user macros.
- **OD-9 closed by D-X1.** `plugin_host_revival_20261031` OD-9 (plugin package shape on the registry wire) resolves to a sealed `.hexon` with `HexonType::Plugin`, so registry protocol v1, `PackageStore` and S-ENTITLEMENT apply unchanged.
- **D-X2 — build policy.** Extension work only authors (specs, WIT, docs, test journeys as text) until the P2P convergence campaign flips `.omc/state/build-gate.json` to `cargo_allowed=true`.
- **D-X3 — sessions.**
  - EXT-PLATFORM (worktree `.omc/worktrees/ext-platform`, branch `track/extension-platform`) owns the runtime, SDK, governance, authoring tools, Extensions UI and first-party migrations.
  - A second session on the private side builds its registry service and store client only against S-REGISTRY / S-ENTITLEMENT; only the seam contract is described in this repo.
  - P2P-QA (orchestrator, main tree) is the merge gate.
- **D-X4 — wave-1 migrations behind the extension surface.**
  - fe-sim fleets and scenarios: fe-sim becomes the first-party "simulation" extension, and a fleet stays DATA (`FleetConfig`).
  - IoT device adapters: HTTP, websocket and MQTT ingress, with ONE adapter shape for simulated and real devices.
  - The hexon registry client.
  - BI egress connectors.

  Terrain is already the first-party extension (`fe-terrain/src/extension.rs`).
- **D-X5** (owned by `portal_surface_20261107`). Web bundles are signed packages in the same sealed `.hexon` family, with a web content type. Extension governance admits them per peer before they get IPC; unsigned or unadmitted bundles render in a no-IPC static sandbox, and replicated rows never auto-execute.
- **D-X6** (owned by `portal_surface_20261107`; the integration bus is specified and built HERE — spec §6.3, R13-R15; P0.3 contract, P1.5 broker). Host-brokered IPC subscriptions plus an integration bus: extensions expose streams and commands and consume hooks through the host broker under capability checks.
- **D-X7** (owned by `portal_surface_20261107`). Portals render in-app first; "through the web" means a static export that talks to the public REST API, later.
- **D-X8** (owned by `portal_surface_20261107`). The portal surface is its own public track, run by a third session, EXT-PORTAL (worktree `.omc/worktrees/ext-portal`, branch `track/portal-surface`), with its own QA lane. That track depends on this one: bus, WIT 2.0 and governance.
- **Proposed, unratified:** OD-E1..OD-E10 (spec §8). Proceed on the recommended defaults, and post each as a bus `question` for the orchestrator to ratify with the user.

## ORCHESTRATOR STEER

<!-- Binding; maintained by the orchestrating session (P2P-QA). Dated bullets, newest first. -->

-

## SESSION BUS

- **Protocol:** `.omc/state/bus/README.md` and `.omc/state/bus/SESSION-PROTOCOL.md` in the main tree. `.omc` is not tracked by git.
  - One append-only file per session: `.omc/state/bus/<SESSION>.md`.
  - Entry shape: `- YYYY-MM-DD HH:MM <SESSION> <milestone|blocker|decision|question|steer|building|ready-to-merge> - text`.
  - Read every bus file at session start and at each milestone.
- **Build gate:** `.omc/state/build-gate.json` in the main tree. It read `cargo_allowed=false` at authoring (D-X2). Check it before every build: BUILD tasks wait for "gate open" on the bus, and a session posts `building` before it builds.
- **Sessions:**
  - P2P-QA — orchestrator, main tree, merge gate;
  - EXT-PLATFORM — this track;
  - EXT-PORTAL — `portal_surface_20261107`;
  - a registry-side session on the private side, named in the bus README.
- **Merging:** post `ready-to-merge <branch> <sha> <scope>` on the bus. P2P-QA reviews and merges; never merge your own branch.
- **Milestones the portal track watches:** P0.3 (bus contract), P0.1 (WIT 2.0), P1.5 (broker), P2.2 (governance).

## SESSION NOTES

<!-- EXT-PLATFORM notes go here (dated bullets). -->
