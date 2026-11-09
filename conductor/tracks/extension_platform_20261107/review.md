---
type: review
track: extension_platform_20261107
title: Review gates G0-G6 — verdict lines
resource: ./metadata.json
---

# Review gates (G0-G6)

One verdict line per finding (spec R36). Verdicts: **FIXED** (the gate edited the named file),
**KEPT** (checked and left as is, with the reason), **HANDED-OFF** (a file the gate does not own;
the owner is named), **DEFERRED** (needs a measurement or a later phase; the trigger is named).
G1-G6 append their own sections below.

## G0 — P0 contract (2026-11-08, slice G0, over E0 `1534d9b2`)

Scope: `fe-plugin/wit/{fractal-plugin-2.wit,AGENTS.md}`, `fe-sdk/schema/**`,
`docs/extensions/{manifest-v2,bus,package-format,governance}.md`, read against the spec, the
2026-11-08 20:10 security steer, plugin_host_revival (S2's landed `trust_roots.rs`/`package.rs`),
hexon_entitlements, the portal contract (`.omc/worktrees/ext-portal/docs/spec/portal-bundle/**`)
and the real code each contract names (fe-format sealing and `HexonArchiveIndex`, fe-sdk
`plugin_host.rs`, fe-runtime `SceneChange`/`NodeDto`, fe-api `query_guard.rs`/`limits.rs`,
fe-database `iot_reading.rs`/`op_log.rs`). No build or parser run (D-X2, directive 2026-11-08).

**Phase verdict: PASS WITH FIXES.** 42 verdict lines: 29 FIXED, 10 KEPT, 2 HANDED-OFF, 1 DEFERRED.

### ADVOCATE (reuse, ergonomics, what the consumers need)

- G0-A1 FIXED `bus.md` §2.4, `governance.md` §Admission record — `host/admission.reason` was free text; it is now a closed set naming the cause (kill-switch layer a-d, ceiling, role loss), which the portal's chrome needs (portal `admission.md` A-16).
- G0-A2 FIXED `package-format.md` intro + §Members, `governance.md` precondition 3 + §Consent — web bundles now share the container, identity, signature and revocation rules and the one verifier (D-X5); their body members, validator and digest are the portal contract's.
- G0-A3 FIXED schema `allOf`, `manifest-v2.md` identity rows — a `kind: web-bundle` manifest no longer needs `name`/`contract`/`binding` to pass structure; the portal owns `portal` and its §1.8 key ban; portal A-11 is resolved without a cross-tree `$ref`.
- G0-A4 FIXED `governance.md` §Provenance — portal `act` writes (portal A-13) ride `PluginCommit` as a one-op commit outside any tick, under the bundle's own admission and provenance.
- G0-A5 FIXED `governance.md` §Trust roots, `package-format.md` §Signatures — the trust-roots text now matches S2's landed file (`[[publisher]]`, hex `public_key`, free-form `key_id`); signers match roots by key bytes; `role` is E10's additive field.
- G0-A6 FIXED `governance.md` §Host switch — the runtime flag is `FE_PLUGIN_HOST_ALLOW` (`fe_plugin::PLUGIN_HOST_ENV`, landed by S1), not the placeholder `FE_EXT_ALLOW`.
- G0-A7 FIXED `bus.md` §11, `manifest-v2.md` §ingress — E0's petal-scoped command and ingress routes are recorded as a decision superseding spec R8/R15's installation paths (installation ids are host-local; the petal guard is reused).
- G0-A8 FIXED `manifest-v2.md` §ingress — `http-server`/`websocket-accept` endpoints are now stated as a delegation of the host identity (rows authored by the host DID), shown at consent like `external: true`.
- G0-A9 FIXED `manifest-v2.md` §readings — the bind dialog, not consent, shows the `source_class` pick (bind follows consent); the class is recorded in observation membership because `iot_reading` has no class column.
- G0-A10 FIXED `fe-plugin/wit/AGENTS.md` §limits, `manifest-v2.md` §kv — kv is per instance everywhere (the WIT said per instance, the docs said per installation).
- G0-A11 KEPT `fe-plugin/wit/AGENTS.md` §one-file — one 2.0 file instead of plan P0.1's per-interface split is right: a WIT directory parses as one package and already holds 1.0; bindgen takes the file path.
- G0-A12 KEPT `fractal-plugin-2.wit` `ingress.send` — moving spec R9's `egress.ws-send` onto the declared ingress socket lists each socket once at consent; recorded as a contract change.
- G0-A13 KEPT `bus.md` §2.3 — OD-3 change-triggered `host/query` re-runs answer the question spec R13 delegated to P0.3 and the portal's OD-3; it costs zero reads when idle.
- G0-A14 HANDED-OFF EXT-PORTAL (S2 grill) — portal `ipc-v1.md` assumptions that differ from `bus.md` (the sibling wins): the node snapshot is `{part,last,nodes}` with fe-runtime `NodeDto`, readings deltas use key `readings`, an over-cap query truncates (`truncated: true`), a local below-floor call gets `capability-denied`, and README §1.8 should also ban `kv`.

### CRITIC (defects, contradictions, rules that cannot hold)

- G0-C1 HIGH FIXED `package-format.md` §Members — a plugin package without `entries.json` cannot be opened by `HexonArchiveIndex::open` ("missing entries.json"), which fe-hexon-registry's header and download routes use; `entries.json` = `{"entries": []}` is now required and J5's valid row asserts the registry serves the header. Code side handed to E9 (`fe-hexon/src/publisher.rs`).
- G0-C2 MEDIUM FIXED schema `httpUrl`/`wsUrl`/`mqttUrl`, V1 — a `websocket-dial` with an `https://` URL or an egress `http` with `mqtts://` passed every rule; the scheme now has to match the transport. E2 validator side handed off.
- G0-C3 MEDIUM FIXED schema `uiContribution.allOf`, `manifest-v2.md` §ui — fields a slot does not render (a `stream` on a ToolbarAction) were silently ignored, breaking the manifest's own "never silently ignored" rule; they are now `schema`. E2 side handed off.
- G0-C4 MEDIUM FIXED `examples/device-adapter.extension.json`, V13 — the example's ContextMenu sent `{node_id}` to `actuate`, whose `args_schema` requires `action`, so every click answered `invalid-input`; it now calls a new `reset` command, and V13 refuses a slot whose args the command rejects.
- G0-C5 MEDIUM FIXED `bus.md` §4 check 1 — local callers got `not-found`, which WIT `host-error` does not have; they now get `not-available` (REST/MCP 404), matching the portal's `unavailable`.
- G0-C6 MEDIUM FIXED `bus.md` §4 check 6, §6 — every local caller shared the local DID's one 10/s command budget, so a chatty extension could rate-limit the UI's buttons; the local DID is keyed per caller (J4 step 9).
- G0-C7 MEDIUM FIXED `bus.md` §4 check 0, `governance.md` §Local identity + §Audit — every denied remote call appended one op-log entry on the DB thread with no rate in front (the spec §4.10 starvation class); a per-DID route rate now runs first (429, not audited; J6 row).
- G0-C8 MEDIUM FIXED `governance.md` §Policy row — `deny` by package id could be dodged by renaming (the 20:10 steer's class); deny entries may now be digests matched under any id (J6 row).
- G0-C9 MEDIUM FIXED `fractal-plugin-2.wit` `on-events` doc, `bus.md` §5, `wit/AGENTS.md` §limits — a `host/query` guest event may reach 1 MiB but an `on-events` batch was capped at 256 KiB; an oversized snapshot is now delivered alone.
- G0-C10 LOW FIXED `governance.md` §Local identity + precondition note — a remote DID saw `host-off` (503) before the uniform not-found, an existence oracle that contradicts R12's AC; the non-local check now runs first (J6 row).
- G0-C11 LOW FIXED `package-format.md` §Identity, `governance.md` precondition 5 — a licensed component re-encrypted under a new content key changed `component_digest` and escaped revocation; the host now also matches `plaintext_digest` in the body phase.
- G0-C12 LOW FIXED `governance.md` §OD-E1 — the proposed S-PLUGIN v2 row omitted the steer's two identity rules, and the "if refused" fallback would have dropped them; both now stand regardless of OD-E1.
- G0-C13 LOW FIXED `fe-plugin/wit/AGENTS.md` §staged-writes — traps outside `on-tick` were unspecified, so a command hook trapping mid-staging could leave a partial batch; any trap degrades the instance and discards the window.
- G0-C14 LOW FIXED `manifest-v2.md` grant rule 2 — "an empty list is refused" contradicted V4 and the fleet example's `ingress: []`; it now separates scope lists from declaration lists.
- G0-C15 LOW FIXED `manifest-v2.md` V2 — `2.x.y` admitted a minor newer than the host binds, whose imports may not exist; that is now `unsupported-contract`. E2 side handed off.
- G0-C16 LOW FIXED `package-format.md` §Signatures + §Members — the counter-signature encoding was unstated (now base64, not PHR's hex `SignatureEntry`), and "the only uncommitted member" overlooked `manifest.json`.
- G0-C17 LOW FIXED `governance.md` precondition 8 — "writing extension" was undefined (readings-only?); it now means "declares `scene.write` or `readings`".
- G0-C18 LOW FIXED `fe-sdk/schema/AGENTS.md` — it claimed the device-adapter example shows all four ingress transports; it shows three.
- G0-C19 INFO FIXED `fe-plugin/wit/AGENTS.md` §wasi-policy — deterministic `wasi:random` is now documented as not cryptographically secure.
- G0-C20 KEPT `governance.md` §Revocation list — only `entries` + `issued_at` are signed (S2's `signed_bytes`); `schema_version` is checked first and `signer_key_id` only selects a root, so neither can widen what a list revokes. Text now says so.
- G0-C21 KEPT `manifest-v2.md` §Capability digest — dropping the whole `tick` block from the digest is right: cadence and fuel are host-capped resources, not authority, and spec R20's render list omits them.
- G0-C22 KEPT `fractal-plugin-2.wit` — WIT read by hand for keywords, kebab-case identifiers, field names that shadow type names (`level: level`, as wasi-logging does), and exported interfaces using imported types: no defect. A parser run belongs to P1.1's bindgen.
- G0-C23 KEPT steer 20:10 conformance — manifest v2 `id` = signed `hexon_id` = the id the install asks for (`id-mismatch`), and revocation matches by hash under any id, in package-format, governance and manifest-v2. G2 re-verifies the E9/E10/E16 code.
- G0-C24 KEPT `bus.md` §2.3 — the `host/query` change-feed table matches the guard: `PreparedSql.tables` exists, and only `node`, `iot_reading` and `device_payload` get feeds, all in `PETAL_SCOPED_TABLES`, so a guest query can never read an unscoped table.
- G0-C25 HANDED-OFF E2 + E9 — the code side of C1, C2, C3, C4 and C15 lives in files G0 does not own (see the slice result's NEEDS lines).
- G0-C26 DEFERRED E5 / P6 — `host/query` re-runs are charged to the local DID's `QUERY_RATE_PER_SEC`, shared with REST (the portal relies on that), so many subscriptions could push local REST `/api/v1/query` into 429. Trigger: P6's host-wide measurement; the fix would be a per-installation sub-budget inside the shared cap.
- G0-C27 KEPT product fit — P0 still serves `conductor/product.md`: device adapters feed the existing readings door (an IoT reading becomes a spatial row) and BI connectors push guarded `host/query` rows out (BI egress, not lock-in).
- G0-C28 KEPT leak discipline — the forbidden-term grep over every E0 file and every line G0 added found no hits, and no catalogue-operations wording.

## G1 — P1 runtime (2026-11-08, slice G1, over E1 `a9cebb5a`, E2, E3 `e56e438b`, E4 `1be2ad07`, E5 `40cc7c16`/`721ba6bc`, E6 `552e621c`, E7 `1a933278`, E8 `96880f4a`)

Scope: ADVOCATE then CRITIC with `/code-review high` and `/security-review` depth over the component
host (`fe-plugin/src/wasm/**`), host imports, the tick scheduler, the bus broker, the extensions API, the
bridge and the slot host, read against spec R4-R15, bus.md, governance.md §Local identity and the
2026-11-08 20:20 / 22:30 steers. No build, clippy or test run (directive 2026-11-08); the refine pass
reruns R11 (fe-plugin bus/tick/wasm), R9 (`fe-api/tests/extensions_api_test.rs`) and the fe-ui slot journey.

**Phase verdict: PASS WITH FIXES.** 22 verdict lines: 9 FIXED, 7 KEPT, 5 HANDED-OFF, 1 DEFERRED.

### ADVOCATE (wiring, reuse, what the next slices need)

- G1-A1 FIXED `fe-plugin/src/tick.rs`, `wasm/component/bus_bridge.rs` — steer 22:30: `TickHost::add_on_bus` registers the instance with the bus before scheduling it, `poll` dispatches each running instance (`BUS_CALLS_PER_POLL`), and stop/degrade/forget unregister (bus first, then the final flush: OD-E1's drain order); `BusHooks for InstanceHandle` seats the same handle the scheduler ticks; J1 extended.
- G1-A2 FIXED `fe-plugin/src/lib.rs` — `bus.poll()` now runs before `ticks.poll()`, so what the broker settles in a turn is dispatched in that turn.
- G1-A3 FIXED `fractalengine/src/main.rs`, `plugin_bridge.rs` — E7 built `install_with` but `main.rs` still called `install()` (empty local DID, unbound API slot, no scene feed, so every bus source answered not-available); main now passes the node DID and the scene broadcast and binds the API slot when the channel exists; the dead `install` is gone.
- G1-A4 FIXED `fe-api/src/lib.rs` — `pub mod extensions` was missing: E6's module never compiled and `plugin_bridge.rs`'s `use fe_api::extensions::…` could not resolve (binary build break).
- G1-A5 FIXED `fe-ui/src/lib.rs` — `pub mod extensions` was missing, so E8's slot host and its slot journey never compiled.
- G1-A6 HANDED-OFF orchestrator / E10 turn — the extension routes are not mounted: `ApiConfig` needs an extension-host field (literals in both binaries' `main.rs` and `fe-api/tests/{api_harness_smoke,token_lifecycle_test}.rs`), `build_router` must merge `extensions::routes` inside the auth layer, and MCP `list_extensions`/`extension_command` and the `/ws` target are unregistered; `api_extension_host` is dead until then.
- G1-A7 HANDED-OFF E8 owner / E13 — `ExtensionSlotsPlugin` is not added and no panel (toolbar, sidebar, inspector, context menu) calls a slot draw function, so declared contributions do not render yet.
- G1-A8 HANDED-OFF next swarm turn — `fe-plugin/src/net/` holds ~2.6k lines from E5 (the 22:30 steer's "no code" is stale) but stays undeclared: it needs tokio-tungstenite, tokio-rustls, futures-util and webpki roots in `fe-plugin/Cargo.toml` + `deny.toml`, and a `NetHooks` bridge over the component.
- G1-A9 KEPT `plugin_bridge.rs` `host/query` — guarded SQL (`prepare_scoped_sql` on the instance petal) over the API channel's RawQuery is the same gateway REST's query fallback uses; a read, so the write-path RawQuery tripwire does not apply.
- G1-A10 KEPT — no production path instantiates a component yet (only `fe-ext run --sandbox` and tests), so `add_on_bus` has no production caller; activation belongs to E10's lifecycle.

### CRITIC (defects, security)

- G1-C1 HIGH FIXED `fe-api/src/extensions.rs`, `fe-plugin/src/bus/{mod,broker}.rs`, `plugin_bridge.rs` — fe-identity mints EVERY API token with the node DID as `sub`, so `is_local = sub == local_did` made any viewer/editor token the local identity (R12 administration), and the bus judged every REST/MCP caller as the local Owner (role floors and the `external` gate bypassed) as soon as the DID is wired. Local is now node DID AND an Owner token; `ApiCaller`/`Caller.role_ceiling` cap the role in `role_of`; a delegated token is remote with a per-token gate key; J4 and J6 rows.
- G1-C2 MEDIUM FIXED `wasm/component/{state,imports,routes,instance}.rs` — scene and route reads had no per-hook bound; each is a DB round trip (up to 15 s) the fuel meter never sees, so one guest could hold the plugin worker and the DB lane; `MAX_SCENE_READS_PER_HOOK` = 64, reset on every guest entry; journey row.
- G1-C3 MEDIUM FIXED `fe-api/src/extensions.rs` `answer_response` — guest status and bytes are served on the API origin: a status outside 200-599 is now 502, and every answer carries `nosniff` and `content-security-policy: sandbox`.
- G1-C4 KEPT steer 20:20 verified — fe-api's core strips authorization, proxy-authorization, cookie, set-cookie, x-api-key and every `x-fe-*` before the host sees an ingress request (J6 row); `net/ingress.rs` strips again and `net/egress.rs` refuses them; fe-api authenticates only through the Authorization header, so no query-string token can reach a guest.
- G1-C5 KEPT WASI policy — closed import list (monotonic clock excluded, sockets off, no preopens or env, frozen wall clock, seeded randomness); `add_to_linker_sync` links more than the list, but `admit` refuses any other import before instantiation.
- G1-C6 DEFERRED `scene.query-nodes` — host-side results are not row-capped (only the DB read path's limit); trigger: E10 activation hosting real petals, cap with the REST row cap.
- G1-C7 KEPT `TickHost::poll` holds its lock while hooks run (already true of `on-tick`, now also `dispatch`), so `TickHost::counters` waits up to one fuel-bounded hook; trigger: status latency, then read the counters handles outside the lock.
- G1-C8 HANDED-OFF E4 owner / E10W — a flush that deletes a node and stages readings on it passes the readings door before the commit, then the insert refuses: `unknown-outcome` with the scene batch landed, bending R6's whole-flush rule; check anchors against the batch's own deletes.
- G1-C9 KEPT — every delegated token of this node shares the bus caller key `Remote(node DID)` for command rates (the fe-api gate is per token); trigger: agents rate-limiting each other.
- G1-C10 HANDED-OFF E10 — a manifest without a `tick` block gets no `TickScheduler`, hence no bus seat: pure bus/ingress extensions have no host path yet; E10 must also call `Bus::forget` and `TickHost::forget` at uninstall.
- G1-C11 FIXED `fe-api/src/extensions.rs` gate — the remote rate gate keyed delegated tokens by the node DID, which `admit_remote` always admits; it now keys them `sub#jti`.
- G1-C12 KEPT product fit and leak discipline — P1 still serves `conductor/product.md` (readings through the door, guarded `host/query` for BI egress, one flush per tick holds in `TickScheduler::flush`); the forbidden-term grep over every line G1 added found no hits.
