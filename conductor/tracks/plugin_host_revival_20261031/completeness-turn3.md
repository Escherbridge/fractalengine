---
type: review
title: "C-PHR completeness critic: plugin host revival at turn-3 HEAD"
track: plugin_host_revival_20261031
slice: C-PHR
date: 2026-11-13
checked_at: 8cacc6ac (plus the uncommitted turn-3 working tree)
verdict: COMPLETE-WITH-GAPS (code complete for every AC on the v2 path; 3 test-reachability gaps, 1 missing journey, 4 stale doc lines, 1 test compile break, all handed off)
---

# C-PHR: plugin_host_revival_20261031 completeness at turn 3

Each spec requirement (spec.md §4 phases, §5 AC-1..AC-8, §6 ODs) and each plan task (plan.md §2, §3, §7) was checked
against the code at HEAD. Nothing was built or run (code-first directive). Statuses: **complete**, **partial**,
**missing**, **superseded** (replaced by a later binding decision, with the replacement verified).
**HANDED-OFF** names a gap outside C-PHR's owned files, along with who should fix it.

## Phases (spec §4, plan §2)

| Item | Status | Evidence |
|---|---|---|
| P0 wasmtime past 29, advisories re-reasoned | complete | `fe-plugin/Cargo.toml:15` wasmtime 49.0.2 (`:18` wasmtime-wasi p2); `deny.toml` carries no "dormant" reason; `SECURITY.md:46-48` records the 19 ignores retired. |
| P1 opt-in gate (feature + env, OD-4) | complete (superseded shape) | governance R16: the host thread is unconditional and inert, and admission is gated. `fe-plugin/src/lib.rs:205-223` (`FE_PLUGIN_HOST_ALLOW`, exact `"1"`, loud WARN, `PackageAdmission`); `fractalengine/src/main.rs:527-530` (`cfg!(feature = "plugin-host") && env`); `fractalengine/Cargo.toml:89`; relay `fractalengine-relay/src/main.rs:311-314` with an optional dep (`Cargo.toml:36,47`). |
| P1 terrain extension registered | complete | `fractalengine/src/main.rs:552` Startup → `:1085-1086` `register_terrain_ui(&mut extensions.0)`; resource `fe-ui/src/plugin.rs:46,896`. |
| P1 `ApiExtensionHandle` mounted in the real router | complete | `fe-api/src/server.rs:526` `.merge(mount_extension_handles(&builtin_extension_handles()))`, `:681-692`, `:706-730` (under `/api/v1/ext/{base}`, unknown handlers skipped). |
| P1 OD-1 network fields closed | complete | `fe-plugin/src/capability.rs:52-58` refuses any `network_scope != "none"` or `external_http` at parse; `:298` denies `HttpRequest` at use. The v2 replacement is declared `ingress[]/egress[]` (`fe-plugin/src/net/**`). |
| P2 ed25519 publisher signatures + trust roots (OD-3) | complete | `fe-plugin/src/package.rs:96-152` (signed id, ≤16 entries, `verify_strict`, a failing trusted entry is never outvoted); `fe-plugin/src/trust_roots.rs:136-168` (weak keys refused, duplicates refused, `dev` roots debug + flag only); call site `fe-plugin/src/lifecycle.rs` `install_plugin`. |
| P2 dev flag never release-reachable | complete | `package.rs:24-27` `AllowDev` is `cfg(debug_assertions)`; `fractalengine/src/plugin_bridge.rs:4285-4298` returns `Refuse` in release whatever the flag; `package.rs:35` warns on every use. |
| P3 consent modal before Install | complete (v2) | Live dialog `fe-ui/src/extensions/consent.rs:229` `confirm_enabled`, `:307` armed gate, `:359` `add_enabled`, `:380` approve only when enabled; wired via `fe-ui/src/extensions/panel.rs:470-475`. |
| P4 install-from-registry, protocol check first | complete | v1: `fe-plugin/src/registry_client.rs:139-168` (header → `/health` body → `config.json`), `:204-215` identity triangle, `:218-227` listed hash. v2 (live): `fractalengine/src/extension_host/sources.rs:220` → `fe_hexon::remote::remote_extension_source`. |
| P4 revocation consumer (wire contract, revocation-capable roots only, OD-8 pull) | complete | `trust_roots.rs:221-244` (schema v1, `may_revoke`, strict); `:196-218` freshness; `lifecycle.rs:731` `accept_revocation_list`; pull `fractalengine/src/extension_host/revocation_pull.rs:71,182-200` (no proxy, same-origin, https except loopback). |
| P4 licensed package via the existing `EntitlementVerifier` | complete (v2) / fail-closed (v1) | `fe_format::entitlement` now ships (`fe-format/src/lib.rs:9-10`, trait `entitlement.rs:541`). Governance row 6 `fractalengine/src/extension_host/governance.rs:198-218` calls `verifier.verify` with no new verifier code, under feature `entitlement` (`fractalengine/Cargo.toml:92`); the default build uses `NoEntitlementVerifier` (`:71-77`), which refuses. The v1 `registry_client.rs:294-296` still refuses every `paid` package (fail closed; fe-plugin has no `entitlement` feature). |
| P5 iot-bridge in the workspace, one rhai | complete | root `Cargo.toml:32`; `extensions/iot-bridge/Cargo.toml` has no `[workspace]` table, `:6` licence, `:11` rhai 1.24 = `fe-plugin/Cargo.toml:14`; `Cargo.lock` has exactly one `rhai` (1.24.0) and an `iot-bridge` entry. |
| S6 review with mandatory security review | complete | `review.md` (24 verdicts) plus the turn-2 status table. R-PLUG's 2026-11-13 SSRF steer is applied: `registry_client.rs:89-106` (`.no_proxy()`, same-origin redirects, which also rules out https→http since origin includes the scheme). |
| S7 docs | partial | Done: `fe-sdk/src/AGENTS.md:318` §live-consumers, `fractalengine/src/AGENTS.md:2167` §plugin-host, `fe-plugin/src/AGENTS.md` §admission-gate/§network-deny/§signed-packages/§registry-install. Stale lines are listed under HANDED-OFF below. |
| Sweep (tests + in-app check) | missing (by directive) | `plan.md:104` is unticked. Tests run only when the user names a batch (AGENTS.md test policy); clippy is never run. |

## Acceptance criteria (spec §5)

| AC | Status | Evidence |
|---|---|---|
| AC-1 default-off | superseded → complete | R16: inert thread, `PackageAdmission(false)`; `lifecycle.rs:1115-1131` `with_admission`/`require_admission` → `HostOff`; `lib.rs:479` env table; the relay without the feature has no host at all. |
| AC-2 terrain in `UiExtensionRegistry` after one tick | complete (code) / test unreachable | Test `fractalengine/src/main.rs` `extension_ui_tests::terrain_extension_appears_in_ui_registry_after_one_tick` (`:1091`+). No `conductor/test-batches.md` filter reaches `extension_ui_tests::` (line 385 filters `extension_host:: plugin_bridge::` only). |
| AC-3 network honesty | complete | `capability.rs:437` `network_scope_table_allow_or_deny_for_none_local_internet` (parse refuses, smuggled token denied at use). |
| AC-4 signed packages | complete | `lifecycle.rs:1791-1846` journey table: host off, unsigned, untrusted, bad and tampered counter-signature, renamed id, debug-only dev flag, activation pinned to the signed payload. |
| AC-5 consent before Install | complete | `consent.rs:509-513` (disabled before a frame, enabled after), `:593-602` (arm delay, focus), `:633` (consent cannot clear a kill-list entry). |
| AC-6 install from a real `fe-hexon-registry` | partial | No journey serves a signed, sealed plugin package through a running `fe-hexon-registry` and installs it. `fe-hexon/tests/remote_registry_test.rs:147-235` exercises only content hexons (and asserts the plugin source lists none). The sim `tools/qa/sim/loopback.py:498` serves a static catalogue, not the registry. The licensed half is code-complete on v2 (row 6 above) with no journey under feature `entitlement` in this track. |
| AC-7 iot-bridge member on one rhai, `bridge_loop.rs` unmodified | complete (unbuilt) | Membership and lock as P5 above; `extensions/iot-bridge/tests/bridge_loop.rs` was last touched in `0cd671e1`, an ancestor of the spec's `f55ca21`; batch `conductor/test-batches.md:162`. |
| AC-8 WIT honesty | complete | `fe-plugin/wit/hexon-plugin.wit:1-3` SUPERSEDED header (v2 contract `fractal-plugin-2.wit`). |

## Open decisions (spec §6)

OD-1 accepted (deny, no grant; v2 replaces it with ingress/egress). OD-2 superseded by D-X1 (WIT 2.0 is the
contract; 1.0 is marked SUPERSEDED). OD-3 accepted (`[[publisher]]` file, plus `role`). OD-4 accepted, then
retargeted to admission (R16). OD-5 accepted (Phase 0 first). OD-6 accepted (blocking modal with an arm delay).
OD-7 accepted (rhai 1.24). OD-8 accepted (pull, newest per signer, cached). OD-9 accepted (sealed `.hexon`
`HexonType::Plugin`, closed in the extension platform's D-X1).

## HANDED-OFF (outside C-PHR's owned files)

- HANDED-OFF `fe-hexon/tests/remote_registry_test.rs:186,225` (R-STORE / store-client owner): the `SourceTrust { .. }` literals
  lack the new `is_killed` field (`fe-hexon/src/registry_source.rs:228`). This is a compile break for
  `cargo test -p fe-hexon --features remote`. Fix: add `is_killed: fe_hexon::registry_source::no_kill_list()` or use `SourceTrust::new(..)`.
- HANDED-OFF `conductor/test-batches.md` (D-BATCH owner): no filter reaches three PHR tests. Add
  `extension_ui_tests::` to the line-385 `fractalengine --bin` command, `server::extension_mount_tests::` to the
  fe-api `--lib` list (line 315), and `panels::plugin_install_panel` to an fe-ui command, or delete that test with the panel (next item).
- HANDED-OFF `fe-ui/src/panels/plugin_install_panel.rs` (fe-ui owner): the v1 consent modal has no caller (only
  `panels/mod.rs:15` declares it). `extensions/consent.rs` is the live dialog. Delete it, or mark it superseded
  in `fe-ui/src/AGENTS.md` §plugin-consent.
- HANDED-OFF AC-6 journey (R11-STORE owner, `fe-hexon/tests/remote_registry_test.rs`): extend the round-trip with a
  sealed, signed `HexonType::Plugin` fixture served by `build_router`. `remote_extension_source(..).list` should offer it,
  `fetch` should verify it, and a renamed or killed copy should be refused.
- HANDED-OFF `SECURITY.md:97-99`: "Paid (licensed) packages are refused until the entitlement verifier ships. The automatic
  revocation-list pull is not built yet." Both claims are stale. The pull is built (`revocation_pull.rs`). Licensed builds are verified under feature
  `entitlement` and refused in default builds.
- HANDED-OFF `conductor/tracks.md:115`: "no package can be installed in-app until P1.7" is stale (P1.7 closed in turn 2;
  `plugin_bridge.rs:4319-4338` builds the manager).
- HANDED-OFF `fe-plugin/src/AGENTS.md` §registry-install (`:317-318`, `:328-331`): the doc says "the download response must carry
  the header too", but code treats the header as optional and falls back to `config.json`
  (`registry_client.rs:139-168,195-199`). It also says "Licensed packages are refused until ... ships `fe_format::entitlement`", which has now shipped.
  It should say the v1 client stays fail-closed and v2 row 6 is the verifier call site.
- HANDED-OFF `fe-hexon/src/registry_source.rs:1860` doc: "the binary adapts `fe-plugin`'s `RegistryClient`". The binary
  uses only `RemoteRegistryClient`, and fe-plugin's `RegistryClient` / `PluginManager::install_from_registry` have no
  production caller (tests and the interim 1.0 path only).
- HANDED-OFF `fe-plugin/src/trust_roots.rs` (C-PHR-owned, intentionally unchanged): `verify_revocation_list_fresh` has no production caller
  (documented in AGENTS.md §signed-packages). `compile_cached` (`wasm/component/mod.rs:179`) has no caller, which keeps
  review.md #18 deferred.

## Per-finding verdicts

- C-PHR-01 remote_registry_test missing `is_killed` → HANDED-OFF (compile break, not owned).
- C-PHR-02 three PHR tests unreachable by any batch filter → HANDED-OFF (test-batches.md).
- C-PHR-03 orphaned v1 consent panel → HANDED-OFF (fe-ui).
- C-PHR-04 AC-6 real-registry plugin journey missing → HANDED-OFF (R11-STORE).
- C-PHR-05 stale SECURITY.md, tracks.md, fe-plugin AGENTS.md and registry_source doc lines → HANDED-OFF (docs owners).
- C-PHR-06 R-PLUG SSRF steer (no proxy, same-origin redirects) in `registry_client.rs` → OK, verified.
- C-PHR-07 dev unsigned path in release → OK (`package.rs:24-27`, `plugin_bridge.rs:4294-4298`).
- C-PHR-08 kill list matched by hash under any id in source trust → OK (`plugin_bridge.rs:4303-4311`).
