---
type: Review
track: extension_platform_20261107
gate: R-TOOL
date: 2026-11-13
reviews: [I-FEXT, X-EXAMPLES, I-SIM]
security_review: false
---

# R-TOOL: authoring tool, examples and sim harness (turn 3)

Scope: `tools/fe-ext/{Cargo.toml,AGENTS.md,src/main.rs,src/lib.rs,src/validate.rs,tests/j8_cli.rs}`, `tools/fe-ext/guest/**`,
`extensions/{fleet-sim,device-adapter,bi-push}/**`, `extensions/iot-bridge/.gitignore`, `fe-sdk/schema/examples/**`,
`fe-sim/src/AGENTS.md`, `tools/qa/{stage.ps1,fe.py,AGENTS.md}` and `tools/qa/sim/**` (I-SIM files). `fe-ext admin`
(`src/admin.rs`) was reviewed by R-ADMIN; `src/index.rs` belongs to the R-STORE partition and was read for the J8 legs.
No cargo, build or test run (code-first turn). Static checks run: `python -I -m py_compile` over every module in
`tools/qa/sim` and `tools/qa/fe.py` (all pass), an AST undefined-name scan of the same files (clean; the steer's
`KEY_ID` NameError is gone at HEAD), `scenario.py validate` on ext-fleet, ext-device, ext-bindings and ext-install-gui
(all OK), and `git check-ignore` on every standalone lock path (none ignored).

Verdict: **tooling sound once two R-STORE fixes land.** The two compile/run breaks inside fe-ext that sit in owned files
are fixed, bi-push is packable again, pack can no longer seal a package the host refuses, and the missing GUI install
scenario exists. `fe-ext index build|verify` (and so every J8 index leg and the sim staging) still fails until R-STORE
fixes `src/index.rs` (R-TOOL-02, R-TOOL-03).

| # | Sev | Finding | Verdict |
|---|---|---|---|
| R-TOOL-01 | HIGH (compile) | `src/index.rs:538` calls `bs58::decode`, but `tools/fe-ext/Cargo.toml` has no `bs58` dependency: fe-ext does not build. | FIXED: `bs58 = "0.5"` (same major as fe-format; already in the root lock). |
| R-TOOL-02 | HIGH (compile) | `src/index.rs:582` builds `SourceTrust { .. }` without the new `is_killed` field (steer 2026-11-13: grep every literal). Same gap in `fe-hexon/tests/remote_registry_test.rs:186,225`. | HANDED-OFF (R-STORE, owner of index.rs and the fe-hexon tests): add `is_killed: fe_hexon::registry_source::no_kill_list()` to each literal. |
| R-TOOL-03 | HIGH | `verify_tree` hands `StaticIndexSource::new` the catalogue dir (`<root>/api/v1/catalog`), but since `7aa81ed3` (8 minutes after `091b3611`) the source appends `api/v1/catalog/` itself, so it reads `<root>/api/v1/catalog/api/v1/catalog/index.json`: `index build` (self-verify), `index verify`, J8's index legs, `stage.ps1 -Loopback` and run.py staging all fail. | HANDED-OFF (R-STORE): give `StaticIndexSource::new` the endpoint root (the folder, or the URL with a trailing `/api/v1/catalog` stripped) and keep the catalogue base only for the web-bundle `locate` calls. |
| R-TOOL-04 | HIGH | bi-push declares `provides: egress.connector`, but the guest SDK only generated the `extension` world, so no example component could export `hooks`: `fe-ext build --project extensions/bi-push` failed `missing-export`, bi-push was not packable, and J8's bi-push leg silently skipped (no `dist/component.wasm`). | FIXED: guest feature `hooks` generates `extension-with-hooks`, adds `Extension::on_hook` (default `Failed`) and the `hooks` export through `__dispatch_hook`; bi-push enables it and answers the connector's `configure` by adopting the bound `format` (kv seed stays the fallback). Rationale in `tools/fe-ext/AGENTS.md` §guest-sdk and `extensions/bi-push/AGENTS.md` §config. |
| R-TOOL-05 | MED | `pack` ran component admission without `check_provides` (`pack.rs` calls `validate_component`), so it sealed and signed packages the host refuses at install. | FIXED in `main.rs`: `pack` runs `validate_component_for` before sealing; J8 extended: bi-push's manifest over fleet-sim's hook-less component is refused `missing-export` by `validate` and by `pack`, and nothing is written. |
| R-TOOL-06 | LOW | The example AGENTS recipes say `fe-ext pack ... --out packages/`, which wrote a FILE named `packages`, so the next `index build --packages packages` failed. | FIXED: `--out` naming a folder (existing, or ending in `/` or `\`) gets `<id>-<version>.hexon`. |
| R-TOOL-07 | MED (completeness) | I-SIM's "new ext-install-gui scenario" did not exist. | FIXED: `tools/qa/sim/scenarios/ext-install-gui.json` (file install, Browse install from the signed static index, both consents by the dialog's button, CLI `disable` + panel `enable`, two uninstalls, viewer stays at zero installations); validates. |
| R-TOOL-08 | LOW | Steer 2026-11-09 03:00: `scenario.py` iterated step groups, `watch` and `checkpoints` that can be JSON `null`. | FIXED: `or []` on each, and a non-list group is a validation error. `loopback.py` `KEY_ID` and the `run.py` None dereferences were already fixed at HEAD (verified by the undefined-name scan and reading `ensure_stage` / `_read_record`). |
| R-TOOL-09 | MED | `fe-ext new bi-connector` copies `fe-sdk/schema/examples/bi-push.extension.json` (it declares `provides`) but scaffolds `fe-ext-guest` without the `hooks` feature, so `new` -> `build` fails `missing-export` for that kind. | HANDED-OFF (owner of `tools/fe-ext/src/new.rs`; no turn-3 slice owns it): when the manifest declares `provides`, write `features = ["hooks"]` on the guest dependency and an `on_hook` stub. Keeping `provides` in the shared example is deliberate (manifest_v2.rs, security_harness and role-propagation tests include it). |
| R-TOOL-10 | MED | ext-bindings names `city-map` / `city-map-legacy`, which have no example crate, so `run.py` staging stops before the run (documented in sim AGENTS). | DEFERRED (trigger: a city-map example crate under `extensions/`, or the scenario switched to fleet-sim/device-adapter providers). |
| R-TOOL-11 | LOW | In a multi-peer run `fe-ext admin` reaches whichever app owns the default endpoint (first started), not necessarily the op's peer. | KEPT: an installation id exists only on the host that installed it, so a misrouted verb answers `not-found` (loud), never acts elsewhere; documented in `tools/qa/sim/AGENTS.md` §ext "CLI routing". |
| R-TOOL-12 | LOW | ext-install-gui has no update-with-delta leg. | DEFERRED (trigger: run.py staging a second version of an example; the driver's `ext_update` + `ext_consent reprompt` are ready). |
| R-TOOL-13 | INFO | `tools/fe-ext/AGENTS.md` held two Latin-1 `§` bytes (invalid UTF-8) and its Tests paragraph said the sign/index legs needed `FE_EXT_TEST_COMPONENT`. | FIXED: re-encoded, Tests paragraph rewritten to the legs J8 now runs, lock policy names every standalone root and iot-bridge's root lock. |
| R-TOOL-14 | — | Verified, no change: `validate.rs` runs `check_provides` after compile (signature matches `ExtensionComponent::check_provides`); `main.rs` wires `admin` and `index`; lock policy (`[workspace]` + root `exclude` for guest/fleet-sim/device-adapter/bi-push, `.gitignore` comments, no lock path ignored, iot-bridge on the root lock); sim administration off REST (remaining REST: policy PUT, bindings PUT/DELETE, bus commands, ingress, GET reads); `stage.ps1 -Loopback` reads `site_dir` + `index_key_id` that `loopback.py` writes; `fe-sim/src/AGENTS.md` §extension-host carries the turn-3 status line; device-adapter keeps the `websocket-dial` example. | KEPT (verified). |
