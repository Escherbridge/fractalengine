---
type: plan
track: plugin_host_revival_20261031
title: "Implementation Plan: plugin host revival"
created: 2026-10-31
resource: ./spec.md
computed_at_commit: f55ca21
partitions_confidence: hypothesis
---

# Implementation plan: plugin_host_revival_20261031

> Citations computed @`f55ca21`. Re-grep line numbers before editing — only symbol names are stable.

## 0. Assumptions

Proceeding on spec §6's recommended defaults unless a reviewer overrides them. Reversal costs repeated here for the person triaging later.

- **A-1 (OD-1)** network_scope/external_http: enforce deny-by-default, no grant path in v1. *Low.*
- **A-2 (OD-2)** WIT marked UNSTABLE/non-normative until bindgen exists. *Low now, medium later.*
- **A-3 (OD-3)** trust roots: static `trusted_publishers.toml` allow-list, no dynamic registration. *Medium.*
- **A-4 (OD-4)** opt-in: Cargo feature (default off) + runtime env flag, both required. *Low.*
- **A-5 (OD-5)** wasmtime bump happens in Phase 0, strictly before the host goes live. *Low.*
- **A-6 (OD-6)** consent UX is a blocking modal, not a passive panel. *Low.*
- **A-7 (OD-7)** rhai aligns upward to iot-bridge's 1.24. *Low.*

## 1. Doctrine for every phase

- **Agents sketch, the orchestrator inks.** No phase agent runs cargo build/test/clippy. The orchestrator gates once per wave (`-p fe-plugin -p fe-sdk -p fe-ui -p fe-api -p fractalengine -p fractalengine-relay` then workspace check), full sweep once at track end.
- **The opt-in flag is the security boundary.** Every phase after S0 must leave the host OFF by default in every build profile, including CI. A phase that flips the default is a bug, not a feature.
- **OD-1 closes inside S1, never deferred** — see spec §4/§7. A PR that wires the host live without touching `capability.rs`'s network fields in the same change is incomplete by definition of this plan.
- **No business logic.** Any diff that references prices, fees, tiers, or a catalogue account model gets rejected on sight — that content does not belong in this repository (`conductor/open-core.md` §The rule).
- **Docs convention:** terse doc-comments in code; rationale in the directory `AGENTS.md`, named per phase below.
- **Tests model real flows** (named below), table-driven for pure logic. No tautologies — every assertion must go red if the covered line were inverted.
- **No read-only reviewers:** the review phase fixes its own findings.

## 2. Phase / slice table

| Phase | Slice | Wave | Model | Task | Owns (files) | Depends |
|---|---|---|---|---|---|---|
| P0 | **S0** | W1 | sonnet | Bump `wasmtime` past 29; re-run `cargo deny check advisories`; shrink/re-reason the 19 ignores in `deny.toml`; fix any API-shape breaks in `wasm/{mod,host_imports,aot,fuel}.rs` from the major bump | `fe-plugin/Cargo.toml`, `deny.toml`, `SECURITY.md`, `fe-plugin/src/wasm/mod.rs`, `fe-plugin/src/wasm/host_imports.rs`, `fe-plugin/src/wasm/aot.rs`, `fe-plugin/src/wasm/fuel.rs` | none |
| P1 | **S1** | W2 | sonnet | Host thread behind opt-in (Cargo feature + runtime flag, A-4); construct `PluginRegistry`/`UiExtensionRegistry` resources; call `fe_terrain::extension::register_terrain_ui`; mount `fe-terrain`'s `ApiExtensionHandle` routes in the real `fe-api` router; close OD-1 in `capability.rs` (enforce deny-by-default or strip the fields — A-1) | `fractalengine/Cargo.toml`, `fractalengine/src/main.rs`, `fractalengine-relay/Cargo.toml`, `fractalengine-relay/src/main.rs`, `fe-plugin/src/lib.rs`, `fe-plugin/src/capability.rs`, `fe-ui/src/plugin.rs`, `fe-terrain/src/extension.rs`, `fe-terrain/src/api_ext.rs`, `fe-api/Cargo.toml`, `fe-api/src/rest.rs`, `fe-api/src/server.rs` (routes register in `build_router`), `fractalengine/src/AGENTS.md`, `fe-ui/src/AGENTS.md` | S0 |
| P5 | **S5** | W2 | haiku | `extensions/iot-bridge` joins the root workspace (drop its own `[workspace]` table, add it to the root `members` list); align rhai to one version (A-7) | `Cargo.toml`, `extensions/iot-bridge/Cargo.toml`, `fe-plugin/Cargo.toml`, `extensions/iot-bridge/AGENTS.md` | S0 |
| **gate W2** | orchestrator | n/a | n/a | Serial build + crate tests once | n/a | S1, S5 |
| P2 | **S2** | W3 | sonnet | Replace `lifecycle.rs::install_plugin`'s BLAKE3 self-check with ed25519 publisher-signature verification against trust roots (A-3); dev-only unsigned-allow flag, never release-reachable | `fe-plugin/src/lifecycle.rs`, `fe-plugin/src/package.rs` (new), `fe-plugin/src/trust_roots.rs` (new), `fe-plugin/src/AGENTS.md` | S1 |
| P3 | **S3** | W3 | sonnet | Capability-consent modal in `fe-ui`: renders scope + capabilities + resolved network posture before Install enables | `fe-ui/src/panels/plugin_install_panel.rs` (new), `fe-ui/src/plugin.rs`, `fe-ui/src/panels/mod.rs`, `fe-ui/src/AGENTS.md` | S1 |
| **gate W3** | orchestrator | n/a | n/a | Serial build + crate tests once | n/a | S2, S3 |
| P4 | **S4** | W4 | sonnet | Install-from-registry over registry protocol v1 (check the `X-Hexon-Registry-Protocol` header first); refuse revoked `(package_id, content_hash)` pairs; reuse `fe_format::entitlement::EntitlementVerifier` for licensed packages (no new verifier code) | `fe-plugin/src/registry_client.rs` (new), `fe-plugin/src/lifecycle.rs`, `fe-plugin/Cargo.toml` | S2; external: `hexon_entitlements_20261031` |
| **gate W4** | orchestrator | n/a | n/a | Serial build + crate tests once | n/a | S4 |
| P2-4 | **S6** | W5 | opus | Review-and-fix, fresh context: `/code-review high` **and** mandatory `/security-review` (sandbox, signature verification, untrusted-code execution, capability enforcement honesty). Fixes its own findings | `conductor/tracks/plugin_host_revival_20261031/review.md` (new) + fix-only edits to S0-S5 files | S2, S3, S4 |
| all | **S7** | W6 | haiku | Docs consolidation: AGENTS.md sections, `conductor/tracks.md` row, plan ticks | `fe-plugin/src/AGENTS.md`, `fe-sdk/src/AGENTS.md`, `fractalengine/src/AGENTS.md`, `fe-ui/src/AGENTS.md`, `SECURITY.md`, `conductor/tracks.md`, this `plan.md` | S6, S5 |
| **sweep** | orchestrator | n/a | n/a | Full workspace test + clippy `-D warnings` once; in-app verify of the consent dialog and opt-in flag is user-gated | n/a | S7 |

## 3. Flow tests per phase

Each name is the test to write. Each asserts an observable outcome through a real seam; fake only the process edge (the network socket for registry fetches, nothing else).

**S0** — `wasm_engine_still_pools_and_meters_fuel_after_bump` (regression, not new behavior). `deny_check_advisory_count_shrinks_or_each_remaining_id_has_a_fresh_reason` (parses `deny.toml` against the pre-bump snapshot).

**S1** — `plugin_host_disabled_by_default_no_thread_no_registry_resource` (App boots with no flag → `get_resource::<PluginRegistry>()` is `None`, AC-1). `plugin_host_enabled_flag_spawns_thread_and_registers_resources`. `terrain_extension_actually_appears_in_ui_registry_after_one_tick` (AC-2 — the regression test for the "never called" finding). `network_scope_table_allow_or_deny_for_none_local_internet` (table-driven; AC-3 — must go red if the enforcement branch is removed). `terrain_api_routes_reachable_through_real_fe_api_router` (closes the `ApiExtensionHandle` dormancy finding).

**S5** — `iot_bridge_compiles_as_workspace_member_on_one_rhai_version`; `bridge_loop_tests_still_pass_unmodified` (AC-7).

**S2** — `signed_package_from_trusted_key_installs`; `signed_package_from_untrusted_key_refused`; `unsigned_package_refused_without_dev_flag`; `unsigned_package_with_dev_flag_installs_and_logs_a_warning` (table-driven over the 4 combinations, collapsing the unhappy-path variants per repo test doctrine; AC-4).

**S3** — `install_dialog_renders_capability_list_before_confirm_enabled`; `confirm_stays_disabled_until_manifest_rendered` (AC-5).

**S4** — `install_from_registry_against_real_fe_hexon_registry_instance_succeeds` (AC-6, real loopback registry, not a mock); `licensed_package_without_entitlement_refused_by_existing_verifier` (asserts no new verifier code path was added — the call site reuses `EntitlementVerifier` unchanged).

**S6** — review findings table in `review.md`; each fixed finding gets a one-line verdict. Security review explicitly checks: does OD-1's enforcement survive a plugin that requests `network_scope: "internet"` with no capability declared; does the dev-flag unsigned-install path appear in any release feature set; does the revocation-list consumer (if landed as part of S2/S4) accept lists only from trust-root entries marked `revocation = true` in the same trust-roots file (never a package-only key, never an accidentally-separate key set).

**S7** — docs-only; no new tests, verify existing suite still green.

## 4. Review-and-fix gate (S6)

Fresh context, not the agent that wrote S1-S4. Mandatory because this phase makes a previously-dormant, previously-unaudited (SECURITY.md) attack surface live:

1. `/code-review high` on S1-S4's diff as a whole (cross-phase interactions, not phase-by-phase).
2. `/security-review` — non-negotiable per this repo's phase-boundary rule for anything touching sandboxing, signatures, or untrusted-code execution. Specifically checks: OD-1 enforcement is real (not another parsed-but-unchecked field); the dev-only unsigned-install flag cannot reach a release build; trust-root key loading has no injection path from untrusted input; the WIT/ABI reconciliation (OD-2) didn't quietly change `host_imports.rs`'s fail-closed sentinel behavior.
3. Record one verdict line per finding in `review.md`. A finding with no verdict is unreviewed, not done.

## 5. Docs + retro (S7)

- `fe-plugin/src/AGENTS.md`: replace the "dormant" framing throughout (§architecture, §lifecycle) with the real wiring; add §package-signing, §consent-ux, §registry-install sections; note the WIT/ABI resolution (OD-2).
- `fe-sdk/src/AGENTS.md`: note `ApiExtensionHandle` and `UiExtensionRegistry` are now live-consumed, not just defined.
- `SECURITY.md`: update "Known dependency advisories" (wasmtime count) and "The plugin sandbox is young" once S6's security review has run — do not simply delete the honesty, update it to reflect the new state accurately.
- `conductor/tracks.md`: add this track's row (no downstream consumers named).
- Retro note: record whether OD-1 through OD-9 were accepted as recommended or overridden, so downstream implementers of the seam can pick that up accurately.

## 6. Cross-track sequencing

`fractalengine/src/main.rs`, `fractalengine/src/AGENTS.md`, `fractalengine-relay/src/main.rs` and `fe-api/src/server.rs` are also owned by `agent_input_surface_20261031` and/or `relay_operability_20261031`. Fixed order for those files: agent_input_surface -> relay_operability -> this track (S1 last); never concurrent, and S1 re-greps symbols after each predecessor's commit. `fe-ui/src/{plugin.rs,AGENTS.md}` and `fe-ui/src/panels/mod.rs` are owned by `polish_lane_20261031` (in flight): S1/S3 wait for its owning waves to close.

## 7. Task checklist

- [x] S0: wasmtime bump + `deny.toml` advisory re-reasoning (Phase 0) — `c519111f`
- [x] S1: opt-in host thread, terrain extension registered, extension routes mounted, OD-1 closed (Phase 1) — `aea5a29f`
- [x] S5: iot-bridge joins the workspace on one rhai version (Phase 5) — `4d527c0a`
- [x] S2: publisher-signed package format + trust roots (Phase 2) — `3e224cc6`
- [x] S3: capability-consent modal (Phase 3) — `8df49217`
- [x] S4: install-from-registry + revocation-list consumer + licensed-package entitlement check (Phase 4) — `9fdb2efb`
- [x] S6: review-and-fix with mandatory security review, recorded verdicts — `5ed08447`, `review.md`
- [x] S7: docs consolidation and `conductor/tracks.md` row (AGENTS.md, SECURITY.md updated)
- [ ] Sweep: workspace test + clippy `-D warnings` once, then the user's in-app consent and opt-in check (not run; code-first directive)
