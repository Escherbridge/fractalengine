---
type: review
track: extension_platform_20261107
gate: R-GOV
reviews: [X-GOV, S-HOST]
date: 2026-11-13
security_review: mandatory (fresh context, prompted to refute)
---

# R-GOV: governance + ops (turn 3)

Scope: X-GOV (79ffc2bc: governance.rs row 6, change generation, actor-keyed acceptances; revocation_pull.rs resolver)
and S-HOST (7b0b7b28: ops_drain.rs admin jobs, store ops, web-bundle install). Code-first: nothing was built or run.

## Verdicts

- R-GOV3-01 FIXED governance.rs `note_acceptance`: before hydration it turned the `None` mirror into a partial `Some(set)`, so a reader would take one live acceptance for a hydrated set; it now leaves `None` until the DB replay lands (fail closed).
- R-GOV3-02 FIXED governance.rs: nothing built a row-6 config with the real verifier; added `GovernanceConfig::with_device_entitlement(device)` over `entitlement_check` so the binary wires it in one call.
- R-GOV3-03 FIXED ops_drain.rs `open_in_system_browser`: explorer.exe splits its argument on commas (`/select,`, `/root,`), so a catalogue-named acquire URL could steer it; commas are now percent-encoded and a URL with userinfo is refused.
- R-GOV3-04 HANDED-OFF (fractalengine/Cargo.toml owner): the crate has no `entitlement` feature, so every `cfg(feature = "entitlement")` in governance.rs is dead (LocalEntitlements, issuer keyring, the feature journey) and licensed builds are always refused; needs `entitlement = ["fe-format/entitlement"]`.
- R-GOV3-05 HANDED-OFF (S-ENT / R-STORE): the feature journey calls `LocalIssuer::encrypted_key_for(&content_key)`; when the package-binding steer changes that signature, update the call (it only compiles with the feature).
- R-GOV3-06 HANDED-OFF (I-BIN, plugin_bridge.rs `BridgeOpsPorts`): none of S-HOST's new ports are implemented: `audit_change_via` (CLI actions lose their `channel`), `audit_admin_refusal` (CLI denials only traced, never durable), `persist_withdrawal` (CLI unbind always `not-available`), `host_switch_changed` (portal resolver not told), `forget_installation`, `licence_identity`, `request_licence_token`.
- R-GOV3-07 HANDED-OFF (I-BIN): no production caller of `ExtensionGovernance::note_acceptance`, so accept/withdraw never move the generation; `persist_acceptance` / `persist_withdrawal` should call it with the local DID after the durable write.
- R-GOV3-08 HANDED-OFF (I-BIN): `hydrate_acceptances` replays `ListExtensionAudit` capped at `ACCEPTANCE_SCAN_LIMIT` (lossy on a long trail); `governance.local_acceptances()` is the lossless, actor-keyed set from `LoadExtensionGovernance`.
- R-GOV3-09 HANDED-OFF (I-BIN): `GovernanceConfig::from_env` still installs `NoEntitlementVerifier` and the admin channel is not spawned (`ExtensionOpsPlugin::admin_jobs()` + `admin_channel::spawn`) at HEAD.
- R-GOV3-10 KEPT (refuted) admin jobs cannot skip consent: no `AdminVerb` path records consent, an acceptance or an override; `admin_stage` auto-activates only an update whose digest is consented, same publisher and root (`activate_update` re-checks), not blocked and with no kill-list offer; `admin_enable` answers `consent-missing`; a pending CLI request settles only on a GUI panel op.
- R-GOV3-11 KEPT (refuted) kill-list override: reachable only from the GUI `KillListOverride` op, typed confirmation, the open verse, the staged pair; governance refuses a non-local actor and a DB read-back that differs; the DB checks the Owner and audits both outcomes; consent is still owed afterwards.
- R-GOV3-12 KEPT (refuted) revocation pull resolver: https (http only to configured loopback), no userinfo, IP literals checked against `is_public_unicast` before any connect, names through `CheckedResolver` (loopback names reach loopback only), `no_proxy`, no referer, `https_only` unless loopback, same-origin redirects (scheme, host, port).
- R-GOV3-13 KEPT (refuted) row 6: unlicensed builds never reach the verifier; a token must verify against a pinned kid, cover exactly this package, content hash and licensee, be unexpired and unwrap with this device's key; the store reads only 64-hex names under an 8 KiB cap.
- R-GOV3-14 KEPT generation churn: re-delivery of the same revocation list (equal `issued_at`) bumps the generation once per pull interval (>= 10 s), so the portal re-judges needlessly; `RevocationSet` has no `len`/`Eq` to detect a no-op union. Trigger: add `RevocationSet::len` in fe-plugin.
- R-GOV3-15 KEPT web-bundle install (GUI or CLI) stores the bundle with no dialog: by design (A-14), admission and consent are per peer at the portal and `install_from_registry` refuses kill-listed bundles.
- R-GOV3-16 KEPT a GUI install of the same id while a CLI request waits replaces the staged build, and the GUI's decision then settles the CLI request; the operator still decided in the GUI, so no consent is skipped.
- R-GOV3-17 KEPT `install-file` opens any path the OS user can read (the CLI is that user behind the pipe DACL / peer uid); a device or pipe path could stall the ops worker. Trigger: if A-PROTO's `check_path` admits device paths.
- R-GOV3-18 KEPT ops arms are judged: `ExtensionOp` matches all 21 variants; Search/LoadDetail/OpenAcquire/RequestLicenceToken run nothing; offers come only from verified, unrevoked extension hits (kill list at offer); fetch re-checks the kill list.
