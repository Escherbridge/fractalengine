---
type: review
title: "S6 review-and-fix: plugin host revival (code review high + security review)"
track: plugin_host_revival_20261031
slice: S6
date: 2026-11-08
reviewed: [c519111f (S0), aea5a29f (S1), 8df49217 (S3), 3e224cc6 (S2), 9fdb2efb (S4), 4d527c0a (S5)]
contracts: [docs/extensions/package-format.md, docs/extensions/governance.md, docs/extensions/manifest-v2.md]
verdict: PASS-WITH-FIXES (13 fixed, 1 routed as NEEDS, 5 deferred with owners)
---

# S6 review: plugin host revival

Fresh context over S0-S5 as one diff, plus the mandatory security review (sandbox, signatures,
untrusted-code execution, capability enforcement). Nothing was compiled or run (2026-11-08 code-first
directive): every fix was written against the real signatures in this tree, and the wasmtime 49 API
was checked against the 47.0.4 sources (49 is not in the local registry).

Verdicts: **FIXED** (fixed here), **NEEDS** (fix belongs to a file no PHR slice owns),
**DEFERRED** (named owner and trigger), **OK** (checked, no defect).

## Findings

| # | Sev | Area | Finding | Verdict |
|---|---|---|---|---|
| 1 | blocker | S2/S4 build | `lib.rs` never declared `package`, `trust_roots` or `registry_client`, so no S2/S4 code compiled. | FIXED: three `pub mod` lines. |
| 2 | blocker | S4 build | `fe-plugin/Cargo.toml` asked for fe-format feature `entitlement`, which does not exist (hexon_entitlements S1 has not landed and is not in this swarm). Every cargo command that resolves the workspace, including CI's `cargo deny --all-features`, would fail. | FIXED: plain `fe-format` dependency; a `paid` package is refused with `EntitlementUnavailable` (fail closed); the one-call unlock is documented in `src/AGENTS.md` §registry-install. |
| 3 | blocker | S2 build | `trust_roots.rs` calls `toml::from_str`, but fe-plugin had no `toml` dependency. | FIXED: `toml = "0.8"`, the line already in Cargo.lock. |
| 4 | high | S1 regression | S1 gated `plugin_bridge::install` itself, but `terminal_reset` requires `PluginHostService`. In a default build, a confirmed reset froze jobs and stopped every service, then failed with "Plugin service owner is unavailable". | FIXED: the inert host thread is unconditional again (as since `2cc05f27`). OD-4's feature + env now gate package ADMISSION (`PackageAdmission`, `PluginManager::with_admission`, `LifecycleError::HostOff`), per governance.md §Host switch (R16). |
| 5 | high | security | **Revocation bypass by renaming** (P2P-QA steer 2026-11-08 20:10). Signatures did not cover the package id, `install_plugin` ignored the manifest, and `is_revoked` matched only exact pairs, so a mirror could serve a revoked, validly signed build under a new id. | FIXED: `RevocationSet::is_revoked(hash)` matches under ANY id. The signed manifest's `id` must equal the installing id (`IdMismatch`), and `fetch_package` refuses a served id that differs from the requested uri id (`IdentityMismatch`). |
| 6 | high | security | Activation was not bound to install. `activate_with_bytes` ran any bytes under any caller-supplied `CapabilityManifest`, so the signature gate could be passed with one module and another executed with wider grants. | FIXED: install records `{content_hash, manifest}` from the signed bytes. Activation refuses other bytes (`PayloadMismatch`) and unverified ids (`NotVerified`), and mints the token from the signed manifest. The separate manifest parameters are removed. |
| 7 | high | security / consent | The consent modal rendered an empty verse/petal/property scope as "None", but v1 enforces an empty scope as a wildcard (`validate_access`). The user was told a plugin got nothing when it got everything. | FIXED: `PluginInstallRequest::consent_rows` shows the enforced meaning ("All verses/petals/properties"; empty capabilities "None"). `requests_network` now mirrors OD-1 exactly (`!= "none"`). The journey asserts the rows. |
| 8 | medium | security | A failing trusted signature beside a valid one was accepted, and a malformed trusted entry aborted early, which contradicts package-format.md §Signatures. | FIXED: any failing or malformed trust-root entry yields `InvalidSignature`, never outvoted; at most 16 entries (`TooManySignatures`). |
| 9 | medium | security | ed25519 used non-strict `verify`, and a weak (small-order) trust-root key would accept forged signatures. | FIXED: `verify_strict` for packages and revocation lists; `is_weak` keys are refused at trust-root load (`BadKey`). |
| 10 | medium | robustness | The registry client had no timeouts, and the `/health` body was read unbounded. A stalled or hostile registry could hang the installing thread or exhaust memory. | FIXED: connect 10 s, request 120 s; `/health` capped at 64 KiB; one `read_capped` helper for every body. |
| 11 | low | tests | Lifecycle tests used `UnsignedPolicy::AllowDev` unconditionally, so they would not compile under a release-profile test run. | FIXED: helpers sign with the trusted key; the dev-flag row is `cfg(debug_assertions)`. The AC-4 journey is one table covering admission off, unsigned, untrusted, bad, tampered counter-signature, renamed id, dev flag, counter-signed success and activation pinning. |
| 12 | low | S5 | `extensions/iot-bridge` joined the workspace with no `license`, which `cargo deny check licenses` reports as unlicensed (deny.toml has no private-crate exemption). | FIXED: `license.workspace = true`. |
| 13 | low | docs | The code pointed at `§network-deny` and `§registry-install`, which did not exist. fe-ui AGENTS claimed empty scopes "fail closed", and SECURITY.md said "the host is off". | FIXED: sections added; claims corrected to the shipped behaviour. |
| 14 | low | docs | `fe-api/src/server.rs` points at `fe-api/AGENTS.md` §extension-routes, which does not exist. | NEEDS `fe-api/AGENTS.md` (no PHR slice owns it). |
| 15 | medium | security | The OD-8 revocation pull is unbuilt (`revocation_sources` has no caller). When built, a replayed older signed list would silently un-revoke. | DEFERRED to extension_platform E10 (kill switch): keep the newest `issued_at` per signer and refuse older lists (documented in §signed-packages). |
| 16 | medium | AC-6 | A licensed package cannot be verified by the existing `EntitlementVerifier`, because that verifier does not exist yet. | DEFERRED to hexon_entitlements S1. Interim behaviour refuses every `paid` package. |
| 17 | medium | wiring | No `PluginManager` is built in either binary, and the consent modal is not wired to any install flow, so nothing can be installed in-app yet (also nothing activates silently). | DEFERRED to extension_platform P1.7 (`plugin_bridge` builds the manager with `with_admission(PackageAdmission)`) and P2.4 (consent v2). |
| 18 | low | sandbox | The AOT cache loads `.cwasm` from disk via `unsafe Module::deserialize`, keyed only by the source hash. Write access to the cache dir is native code execution. | DEFERRED (pre-existing; needs same-user write access): revisit in E1, when components replace core modules. |
| 19 | info | contracts | S4's interim v1 layout, the `[[publisher]]` trust-roots file and the env name `FE_PLUGIN_HOST_ALLOW` differ from package-format.md, governance.md `[[root]]` (did:key) and its `FE_EXT_ALLOW` placeholder. | DEFERRED to E9/E13/E10, which retarget to the sealed format; recorded as contract changes. |
| 20 | - | OD-1 | A manifest asking for `network_scope: "internet"` with no capability is refused at parse, and a smuggled token is still denied at use (S1 table test). | OK |
| 21 | - | dev flag | `AllowDev` exists only under `debug_assertions`; the release profile does not enable it, and no env var reaches it. | OK |
| 22 | - | sandbox | S0 left `wasm/host_imports.rs` fail-closed sentinels untouched. `async_support` removal is right (a deprecated no-op in 47+), and `wasmtime::Error` is its own type there. | OK, with the assumption that 49.0.2 matches 47.0.4. |
| 23 | - | fe-api | Extension routes mount inside the auth layer and alias handlers that enforce `require_role` themselves; unknown handler ids are skipped. | OK |
| 24 | - | AC-8 | `hexon-plugin.wit` now carries E0's SUPERSEDED header (`1534d9b2`); the WIT no longer poses as the live ABI. | OK |

## Acceptance criteria after S6

- **AC-1** is superseded by governance.md R16: with the flag unset, the thread runs inert and
  `PackageAdmission(false)` refuses every install and activation.
- **AC-2** holds unconditionally: terrain is first-party, compiled in, never an admitted package.
- **AC-3** holds (fields refused at parse, denied at use).
- **AC-4** holds, as one journey table.
- **AC-5** holds, with honest rows.
- **AC-6** holds for unlicensed packages; the licensed half waits on hexon_entitlements S1.
- **AC-7** holds, pending the build sweep.
- **AC-8** holds via E0.
