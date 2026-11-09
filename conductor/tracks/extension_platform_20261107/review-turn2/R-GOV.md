---
type: review
track: extension_platform_20261107
title: R-GOV — governance security review-and-fix (trust roots, kill list, Owner override, consent, install offer), swarm turn 2
resource: ../metadata.json
---

# R-GOV — governance (turn 2, over F-TRUST, F-GOV `3b43c4b1`, F-OPS `f03795c4`, B7 `f2391f2f`, I-BIN `de5881ab`)

Security review-and-fix, code-first. Nothing was built or run; every edited file parses (`rustfmt --emit stdout`,
no writes). The refutation targets come from the slice task. The binding steers are 2026-11-09 01:50 (2) and (3),
and 02:15. The hand-offs are R-REG-13, R-TOOL-24, R-UI-03, R-UI-13 and R-UI-28. Each finding has one verdict line:

- **FIXED**: edited here.
- **KEPT**: checked and left as is, with the reason.
- **HANDED-OFF**: names the owner.
- **DEFERRED**: names the trigger.

## Trust roots and tiers

- R-GOV-01 FIXED `fe-plugin/src/trust_roots.rs` `verify_revocation_list_fresh` compared `issued_at` as text. Two consequences:
  - An older list in offset form (`2026-11-09T23:00:00+05:00` = 18:00Z) sorts above a newer `…T20:00:00Z`, so the replay un-revokes.
  - A newer list with a fraction (`…00.5Z`) sorts below `…00Z` and is refused.

  `issued_at_instant` now parses strict UTC (`Z` only, at most 9 fraction digits) and compares by value; any other shape gives `RevocationError::BadIssuedAt`. Governance drops its private copy, which also accepted `…00.Z`, and uses this one parser. Journey rows: a fraction is newer, an offset form is refused.
- R-GOV-02 KEPT: a release tier needs a `release` root.
  - `TrustIndex::tier_of` matches roots by key bytes, never by label.
  - A key listed under several roles gets its least-privileged tier.
  - `DevUnsigned` comes only from `tier_for(None)`, under the debug-only dev flag.
- R-GOV-03 FIXED `plugin_bridge.rs` `BridgeOpsPorts::trust_tier` and `InstalledCatalog::verify` mapped "no vouching root" to `TrustTier::DevUnsigned`. `first-party-only` admits `DevUnsigned` (`check_mode`). Seal verification refuses such builds today, so this was a latent fail-open. The fallback is now `Publisher`, which `first-party-only` refuses.
- R-GOV-04 KEPT: dev roots load only under `cfg!(debug_assertions) && dev_flag`. `dev_flag_from_env` is false in release builds, and `unsigned_policy` is `Refuse` in release whatever the flag says. Duplicate and weak keys fail the whole file, even for skipped dev entries (fail closed).

## Kill list and revocation pull

- R-GOV-05 KEPT: revocation applies under a renamed id and to the component or plaintext digest.
  - `RevocationSet` keys on the lowercased digest only.
  - `PackageFacts::hashes` yields the content, component and plaintext digests.
  - `check_activation`, `build_facts` (`is_revoked_any`) and governance all test every one of them.
- R-GOV-06 KEPT: an old list never un-revokes.
  - Governance and the manager both keep the newest list per signer, and an equal instant unions (fail closed).
  - `revocation_pull` caches the last accepted list per signer and seeds it before the first pull, so the high-water mark survives a restart. This closes R-REG-13.
- R-GOV-07 KEPT revocation transport: https, or loopback http, only; https-only redirects (at most 5); a 1 MiB cap; an anonymous reqwest client with no cookie store. The list signature, not the transport, decides.
- R-GOV-08 KEPT: two lists from one signer with the same instant are unioned in memory. The cache keeps only the last one received (a signed list cannot be merged), so after a restart the union shrinks to that list until the next pull. This needs signer equivocation; the next newer list supersedes it anyway.

## Owner kill-list override

- R-GOV-09 FIXED: override hydration carried over to another identity. `load_kill_list_overrides` returned `(verse_id, content_hash)` without the recorder, and `apply_snapshot` applied every override. An override recorded under a rotated node key, or any foreign `kill-list-override` op-log entry, ran on this host as the new identity. The fix:
  - `KillListOverride` carries `actor_did` (serde default);
  - governance keeps only the overrides of the current local DID;
  - `kill_list_override` also checks the actor the DB echoes back.

  The governance journey has rows for a rotated key and for a missing recorder.
- R-GOV-10 FIXED `fe-database` `record_kill_list_override` with an empty node DID compared `Some("")` against a verse whose `created_by` is empty. An empty local DID is now `role-denied` (governance already refused it before the DB, so this is defence in depth).
- R-GOV-11 KEPT: the override cannot reach another verse, build or peer, and cannot skip the audit.
  - It is keyed `(verse_id, content_hash)` in governance and in the manager.
  - The ops drain requires the staged build's id and hash, and the open verse.
  - The DB checks the verse Owner and the typed confirmation, and audits both refusals and grants.
  - `AppendExtensionAudit` refuses a forged `kill-list-override` (fe-database `lib.rs`).
  - Audit entries live in the local op-log and `append_audit` never publishes them.
  - The manager gains the override only after the DB answers.
- R-GOV-12 KEPT: the REST override names `package_id` only for the typed confirmation, so the hash need not belong to that id. That reach is the Owner's own host and verse; the GUI path binds both to the staged build.
- R-GOV-13 FIXED `plugin_bridge.rs` `hydrate_acceptances` replayed every `bind`/`unbind` with a verse, whoever wrote it, so a rotated key's "Allow in <verse>" carried over. R12 makes acceptance this identity's word. It now requires `actor_did == local DID`.

## Consent and the ops drain

- R-GOV-14 FIXED steer 01:50 (3), the consent bypass on update. `stage` treated any build of an installed id as an update. The panel then sent `ActivateUpdate` without a dialog when the capability digest matched, and `check_activation` compares only `(package_id, capability_digest)`. A build signed by another publisher or root therefore activated silently. Now:
  - `BuildInfo` keeps `trusted_by`.
  - A publisher or root change sends no consented digest, so the dialog always opens. It adds a first section, "Publisher changed: was … now …".
  - `activate_update` refuses a publisher change, and refuses a build that is not a staged update, as `consent-missing`.
- R-GOV-15 FIXED: the `ConsentRequest` literal lacked B7's `portal` field (a build break). It is now `portal: None`; the extension path is not the portal consent path.
- R-GOV-16 FIXED steer 02:15: no arm handled B9's `RefreshBindings`, `SetBinding`, `ClearBinding` and `TakeBindingOffer`, so the match was not exhaustive.
  - **Writes** go through `DbCommand::PutBinding { caller_did: None }`. The DB plane judges the local DID's Manager+ at the scope and the verse policy, and audits. A petal scope without its fractal is refused before `build_scope`, which would panic.
  - **Offers** are taken only when a freshly loaded plane still shows that exact offer for that package at the open verse.
  - **Override** requires the local verse Owner.
  - **Install and Update** go fetch → verify → `ConsentNeeded`. Nothing is stored before consent.
  - **AllowInVerse** requires consent for the installed digest; otherwise it opens the review.
  - **Enable** clears kill layer a, audited.
- R-GOV-17 FIXED R-UI-13, the Bindings snapshot:
  - `local_is_owner` is filled.
  - `scopes` lists only the scopes where the local DID holds Manager+ by `EffectiveRoles::authority` (subtree authority; an Owner pin cuts it). With no role plane, only the Owner gets a scope.
  - Install and Update are dropped unless a configured source lists a fitting build: `version_req` through `version_matches`, and Update newer than the installed build.
  - Points cover every served keyless hook, plus each exact key an installed provider declares.
- R-GOV-18 FIXED: an install offer never recorded "Allow in <verse>". `consent()` used `accept_in_verse` only to decide a local start. It now writes one durable `bind` audit entry carrying the verse, then the manager's `accept_in_verse`. An acceptance for a verse other than the open one is refused, because R19 judged only the open verse's policy.
- R-GOV-19 FIXED: approving an install offer also started a local instance at the open petal. The binding resolver starts its own binding instance under the same `(petal, package)` context key, so the two raced. An offer now installs, consents and accepts; the resolver starts the instance.
- R-GOV-20 FIXED: GUI lifecycle changes went unaudited (governance.md §Audit). REST Disable was audited, but the panel's install, update, enable, disable and uninstall were not. A new port, `audit_change`, now records them.
- R-GOV-21 KEPT: consent cannot be bypassed in the dialog.
  - Enable needs the list drawn once, no `blocked` and no kill list.
  - Override needs the exact typed id.
  - The host re-judges every `Consent` op: anything other than `ConsentMissing` refuses.
  - The DB re-checks the confirmation.
- R-GOV-22 FIXED R-UI-28: the consent window now uses `egui::Order::Foreground`, so a click on the Extensions window cannot bury an open review.
- R-GOV-23 KEPT `install_offer.rs` is pure state and never installs.
  - `action()` offers Install only with policy admission and a registry build, and Override only to the Owner.
  - `consent_request` sets only the origin and the kill-list offer.

## Activation and gates

- R-GOV-24 KEPT `sealed_admission`: a v2 token gets four things:
  - `with_package`, from the build the stager verified under that context, with the same package id;
  - `with_scene_writes`;
  - `with_readings`, only with a bound readings class;
  - fail closed when no verified build is staged.

  The ops drain and the binding stager share the one `DbSceneFactory`.
- R-GOV-25 KEPT: the gates fail closed.
  - The manager starts with `FailClosedGates`.
  - Row 6: `NoEntitlementVerifier` refuses licensed builds.
  - Row 7 refuses until hydrated, while a row is stale, and when `extensions.toml` is unreadable.
  - Row 8 refuses an unresolved role.
  - Package admission defaults off.
  - A manager that fails to build answers every op `Failed`.
- R-GOV-26 KEPT `evaluate` reads a manifest-less `PackageFacts` as a web bundle, so the kinds ceiling is skipped (§10 Q10). Every extension path (the ops drain and the catalog) sets the manifest. Trigger: any new `PackageFacts` producer without a manifest.

## Hand-offs and deferrals

- R-GOV-27 HANDED-OFF orchestrator / R-X, steer 01:50 (2), which names a file outside R-GOV's owns. `fractalengine/src/extension_host/sources.rs` `is_internal` is still a deny-list (R-REG-32 extended it). It misses:
  - `64:ff9b:1::/48` (local NAT64);
  - `2002::/16` (6to4 with an embedded v4);
  - `2001::/32` (Teredo);
  - `100::/64`;
  - `2001:db8::/32`.

  Replace it with a global-unicast allow-list that resolves once and checks every address, shared with fe-plugin `net/`.
- R-GOV-28 HANDED-OFF I-DB / R-X: `DbCommand::SetExtensionPolicy` carries no actor. A remote Manager's REST policy write is therefore audited as the local DID (`set_policy_with_replication`). Add `actor_did` to the command and record it in the `policy-change` entry.
- R-GOV-29 HANDED-OFF orchestrator (R-TOOL-24): `administer` still answers `not-available` for install, consent, bind, enable, update, unbind and uninstall over REST. Serving them means routing REST jobs through the single ops worker with a consent record taken from the request digest. That is a contract decision (governance.md §Local identity vs extensions-ui.md "consent cannot be bypassed"), not a gate fix. It blocks the ext sim runs.
- R-GOV-30 DEFERRED (R-UI-03): registering the manifest `ui` entries and `set_panel_uses` at activation, and forgetting them on stop. This is feature work across Bevy resources. It fails closed today: third-party UI entries do not show. Trigger: the G4-C17 GUI QA path at refine.
- R-GOV-31 DEFERRED: `hydrate_acceptances` reads the newest 10,000 audit rows, so bus and command audits can push older acceptances out. That fails closed: the binding reads `not-accepted`. Trigger: an `actor_did` on fe-database `AcceptanceRecord`, after which the worker reads the `acceptances` that `LoadExtensionGovernance` already computes.
