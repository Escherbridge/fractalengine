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

- 2026-11-09 03:00 (orchestrator fractalengine-71, mandatory for R-TOOL) — the editor's static checker
  (Pyright) reports real runtime errors in the sim harness of this tree: tools/qa/sim/loopback.py uses
  `KEY_ID` without defining or importing it (lines ~50, 58, 375, 379, 412, 424 → NameError at runtime);
  scenario.py:223/228/232 iterates a value that can be None; run.py:120/361/387 dereferences possibly-None
  values. R-TOOL fixes them (owner of I-SIM/B11 files) and runs `python -I -m py_compile` over tools/qa/sim
  (syntax only — no test runs).
- 2026-11-09 02:45 (orchestrator fractalengine-71, SECURITY, mandatory — automated commit review of
  B10a/I-API, confirmed) — **scene hooks are bypassable two ways.** (1) **R-HOOKS — fail-open on
  caller-sized input:** fe-runtime/src/scene_hook_gate.rs skips the bound hook and proceeds when the hook
  input exceeds `MAX_HOOK_INPUT_BYTES`; the caller controls that size (node names, property values). When a
  binding exists and the input is over the cap, REFUSE (REST/MCP/WS answer 413-style `input-too-large`);
  "fail open" (bindings.md) covers hook crash/timeout only, never input the caller sized. (2) **R-API (+R-HOOKS
  keeps the API stable) — door parity:** only the REST create/transform/set-property handlers in
  fe-api/src/rest.rs consult the gate; MCP tools (create_node, update_transform, place_asset, set_property in
  fe-api/src/mcp/tools.rs) and WS entity commands (CreateNode, SetNodeProperty in fe-api/src/ws.rs) write
  through the cores directly. Every door that creates, transforms or sets properties calls the same
  `scene_gate_verdict` with its own `SceneCallerKind` (Mcp, Ws) and stages the augment exactly as REST does.
  R-X verifies parity across REST, MCP, WS and the GUI authoring path (one shared-core gate is preferred if a
  slice owns the core).
- 2026-11-09 02:25 (orchestrator fractalengine-71, SECURITY, mandatory for R-LIFE) — **decode body cap
  checked after allocation** (automated commit review of B10b): fe-plugin/src/bindings/decode.rs reads every
  payload file via `host.payload_files(&job.asset_ids)` and only then sums against
  `MAX_DECODE_BODY_BYTES`, so an oversized device payload is fully loaded before it is refused. Fix: the
  `DecodeHost` returns declared sizes first (asset `size_bytes` from the DB), the sum is checked BEFORE any
  read, and bytes are then read through a bounded reader that stops at the remaining budget (a declared size
  that lies is cut off, not trusted). Extend the decode journey with one oversized payload.
- 2026-11-09 02:15 (orchestrator fractalengine-71, mandatory integration) — **I-UI** adds `pub mod bindings;`
  to fe-ui/src/extensions/mod.rs (B9 left it out); **R-GOV** adds ops_drain.rs arms for B9's new
  `ExtensionOp` variants (RefreshBindings, SetBinding, ClearBinding, TakeBindingOffer) so the match stays
  exhaustive, each going through the same binding authority/consent path as B7/B8 (no op skips judging);
  **I-BIN2** moves terrain_bridge.rs off `RemoteRegistryClient`/`FetchStrategy` onto F-REG's `fetch_hexon`.
- 2026-11-09 01:50 (orchestrator fractalengine-71, SECURITY, mandatory — automated commit review of turn-2
  F-OPS/N5, confirmed) — three fixes:
  **(1) R-NET — credential slots are not package-scoped (HIGH):** fe-plugin/src/net/mod.rs:537 looks up
  `credentials.get(&auth.credential)` with the manifest-declared slot name, and
  fractalengine/src/extension_host/net_sources.rs `FileCredentialStore::get(slot)` keys by slot alone, so any
  extension declaring the same slot name reads another extension's credential. Change the `CredentialStore`
  contract to `get(package_id, slot)` (store `{package_id: {slot: ..}}`), have `Net` pass the instance's
  package id, update fe-plugin/src/net/tests.rs and docs (manifest-v2.md `auth` section); no fallback to an
  unscoped slot.
  **(2) R-GOV (and R-REG if still running) — SSRF denylist incomplete:** `is_internal` in
  fractalengine/src/extension_host/sources.rs:187 misses 0.0.0.0/8, 198.18/15, 240/4, multicast, ::/96,
  64:ff9b::/96 + 64:ff9b:1::/48 (NAT64), 2002::/16 (check the embedded v4), fec0::/10, ff00::/8. Replace with
  an ALLOW-list of global unicast only (resolve once, check every resolved address, connect to the checked
  address — no DNS rebinding); R-NET applies the same predicate to fe-plugin net/ egress and dial-outs (one
  shared function).
  **(3) R-GOV — consent bypass on update:** ops_drain.rs treats any build for an installed installation_id as
  an update judged only on the capability digest. An update whose `publisher_did` (or signing trust root)
  differs from the installed build is a NEW install: fresh consent, with the publisher change shown in the
  ConsentRequest; `activate_update` also checks the consent record's publisher.
- 2026-11-08 23:00 (orchestrator fractalengine-71, SECURITY, mandatory for G5) — **CSV formula injection in
  bi-push** (automated commit review of E17): `csv_field` in extensions/bi-push/src/payload.rs quotes `, " \r
  \n` but does not neutralise cells starting with `= + - @ \t \r`; node names/properties come from any verse
  editor (replicated), so an export opened in a spreadsheet can execute formulas. Fix (OWASP CSV injection):
  for STRING cells only, prefix a single quote when the first character is one of `= + - @ \t \r`, then quote
  the field; numeric cells (including negative numbers) stay untouched. Extend the existing payload
  journey/test with one crafted-name row. G5 fixes and records the verdict in review.md.
- 2026-11-08 22:45 (orchestrator fractalengine-71, mandatory) — **bindings contract landed `e440a473`**
  (docs/extensions/bindings.md, manifest v2 `provides`, V17/V18, bus.md §12). Fixes owed by pending slices:
  **E10W** rewrites docs/extensions/governance.md §Provenance and §OD-E1 to the user's no-withdrawal rule
  (stop after in-flight writes; rows stay); **E10** amends governance precondition 5 (`revoked`) with the
  audited verse-Owner kill-list override, scoped to that verse and build on the Owner's own host — a
  kill-listed bound extension WITH that override runs on the Owner's host only and stays unresolved on every
  other peer (confirmed reading); **E18** replaces `"fractalengine:terrain"` (fe-terrain/src/extension.rs:10)
  with a valid v2 package id and makes terrain the first `petal.map` provider per bindings.md. Local
  acceptance (an operator allows an installed package per verse before any binding runs it) is part of the
  contract: E10/E11 keep room for it. Everything else in bindings.md (WIT `hooks` export, the binding table,
  resolver, adapters) is the BND-0..BND-11 turn after this swarm.
- 2026-11-08 22:30 (orchestrator fractalengine-71, mandatory) — **integration bus wiring.** fe-plugin/src/lib.rs
  now declares `pub mod bus`, exposes `BusHandle(pub bus::Bus)` as a Bevy resource and calls `bus.poll()` once
  per plugin-worker loop turn (orchestrator fix; E5 had left the module undeclared). Still owed: **E7**
  pre-inserts a `BusHandle` built with real `BusSources` and the local DID before `PluginHostPlugin` builds
  (today it falls back to `BusSources::fail_closed()` with an empty DID, so nothing is local); **G1** wires
  `Bus::register_instance` / `unregister_instance` (broker.rs:405/445) at activation and stop (TickHost
  add/stop is the natural seam) and per-instance `Bus::dispatch` (broker.rs:1080) through a `BusHooks`
  implementation over the guest component from E1, and verifies the 20:20 credential-header stripping once a
  network path exists. **fe-plugin/src/net/ has no code** (only AGENTS.md): the extension network client
  (ingress/egress/websocket/MQTT, credential policy) is a feature gap scheduled for the next swarm turn — do
  not declare `mod net` until it exists.
- 2026-11-08 22:00 (USER DECISIONS, binding) — **OD-E1 decided: no withdrawal.** On revocation the
  instance stops after its in-flight writes complete (drain the current commit; no new ticks, commands or
  subscriptions); rows it wrote STAY, with provenance `{package_id, content_hash}` kept for audit and
  filtering. R24 shrinks to stamp-and-stop (no withdrawal, no re-judge, no readings exclusion): E10W =
  graceful drain + provenance retention; D2 amends the open-core S-PLUGIN row to "refuse activation; stop
  after in-flight writes complete; rows stay". **Kill list at install: refuse by default, with an explicit,
  audited verse-Owner override for that Owner's own peer** (canonical-log audit entry; E10 owns the policy,
  E11/E13 the confirmation UI). **NEW FEATURE — scoped extension bindings:** a scope in the hierarchy
  (verse/fractal/petal) binds an installed extension to a HOOK POINT; nearest scope wins (petal > fractal >
  verse); a Manager+ at that scope (subtree authority, as role propagation R7) sets bindings; bindings are
  replicated rows judged by the author's role at write time. Resolution is PER PEER — peers need not run the
  same extensions; a peer missing the bound extension (or forbidden by policy) keeps the built-in behaviour,
  shows an unresolved-binding state and is OFFERED an install through the normal consent dialog under the
  verse install policy (never auto-installs). v1 hook points: map/terrain settings (generalises the
  per-petal terrain binding), extra API routes under the scope's REST path, scene operation hooks, ingest
  decoders, UI panels, egress connectors, file/storage/network operations, portal/web operations — "deep
  integration with basically all existing features"; the renderer (shared asset paths) is a stretch goal.
  Contract: `docs/extensions/bindings.md` (being written by the orchestrator now). Pending slices code toward
  it: E6 mounts extension routes per binding scope; E10 holds binding authority + the kill-list override;
  E11 adds the install-offer path; E13 shows bindings and unresolved states; E18 makes terrain the first
  bound hook (`petal.map`). Implementation of the binding table, resolver and the full hook catalogue is a
  NEW TURN after this swarm.
- 2026-11-08 20:45 (orchestrator fractalengine-71) — **status + relays.** (a) The 20:10 revocation steer is
  CLOSED in fe-plugin by PHR-S6 `5ed08447` (hash under any id; id bound to the signed manifest and the
  requested uri) and in docs/extensions/package-format.md by E0. Remaining: wherever sealed v2 packages hit
  the lifecycle (E1, E10), check revocation against BOTH `content_hash` and `component_digest`; the v1
  fe-plugin registry_client compares the payload hash with the registry's `content_hash`, which under the
  v1 store contract is the manifest hash — reconcile it in E16 (fail-closed today, not a hole). (b) **Bus
  answers to the portal (A-13/A-14/A-16), orchestrator defaults:** a portal action is a one-op commit on the
  extension write path outside any tick (one DB command, host-stamped provenance `{package_id,
  content_hash}`, judged by the installing identity's role at write time, shared rate limits, no tick budget);
  an admitted bundle is an installation of kind `web-bundle` with no instance (R11 status vocabulary);
  `host/admission` carries `{package_id, content_hash, state, reason}`. G0/E5 reflect these in
  docs/extensions/bus.md. (c) **Store-client contract v1** (marketplace review, 18 fixes) — deltas for E16
  and G5: `content_hash` = BLAKE3 of the canonical manifest with the signature blanked (matches
  package-format.md); attestation adds `header_members_b64 {path: base64}` so consent renders before
  download; IndexEntry adds `detail_hash` (BLAKE3 of packages/{id}.json as served) and the client checks the
  detail doc's hash, package_id and kind against the verified index; index signature = ed25519 over
  `fractalengine/catalog-index/v1` LF + canonical JSON of the doc minus signature, verified over generic
  JSON; the index key is pinned per endpoint (key-id set) and is never a package or revocation signer (R29's
  "signed by a trust-root key" must change); downloads are anonymous — no credentials to the base or any
  URL a catalogue doc names; the protocol check reads config.json `registry_protocol` (the header is never
  required); package_id `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`, no case-only
  collisions; SemVer without build metadata; ignore an index/kill list older than the cached one, refuse a
  newer schema_version, integers only in signed docs; kill-listed packages refused at install; leaving the
  index is not revocation; `size_bytes` caps the download; `requires` uses Cargo SemVer requirement syntax.
- 2026-11-08 20:20 (orchestrator fractalengine-71, SECURITY, mandatory) — **credential forwarding into
  guests** found by the automated commit review of E0 (`1534d9b2`) and confirmed: an `http-server` ingress
  endpoint's `forward_headers` (fe-sdk/schema/extension-manifest-v2.schema.json, WIT fractal-plugin-2.wit
  "the endpoint's declared forward_headers") is constrained only by a name pattern, so a manifest can declare
  `authorization`/`cookie` and hand the caller's FractalEngine API token to third-party guest code. Required:
  (1) the HOST strips credential headers before anything reaches a guest, whatever the manifest says —
  denylist in code: authorization, proxy-authorization, cookie, set-cookie, x-api-key, and every header the
  host itself authenticates with — owned by whichever slice implements ingress forwarding (E5/E6), verified
  by G1; (2) the schema rejects those names (`not: {enum: [...]}` on the items) with a matching semantic
  V-row in docs/extensions/manifest-v2.md — G0 if still running, else the orchestrator's targeted fix.
- 2026-11-08 20:10 (orchestrator fractalengine-71, SECURITY, mandatory) — **revocation bypass by renaming**
  found by the automated commit review of PHR-S4 (`9fdb2efb`) and confirmed: the package signature covers
  `manifest_bytes || blake3(payload)` (fe-plugin/src/package.rs `signed_message`) but not the package id;
  `install_plugin` ignores the parsed manifest; `is_revoked` matches only the exact `(package_id,
  content_hash)` pair (fe-plugin/src/trust_roots.rs). A registry or mirror (untrusted by contract) can serve a
  revoked, validly signed package under a new id and it installs and activates. Required: (1) a revoked
  content hash is revoked under ANY package id (kill-list shape unchanged); (2) the package id is bound into
  the signed bytes — the signed manifest's id must equal the installing id and the registry doc's
  `package_id`/requested uri. The orchestrator lands the fix in fe-plugin after PHR-S6 (no EXT slice owns
  those files). E10 (kill switch), E16 (remote registry install) and the manifest v2 id field (E0/E9) must
  keep both properties; G2 verifies them and files any regression.

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
