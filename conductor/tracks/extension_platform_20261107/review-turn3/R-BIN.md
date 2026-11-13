---
type: review
track: extension_platform_20261107
gate: R-BIN
reviews: [I-BIN, I-MAIN, X-HOSTUI]
date: 2026-11-13
security_review: mandatory (fresh context, prompted to refute)
verdict: PASS-WITH-FIXES (9 FIXED, 6 KEPT, 8 HANDED-OFF)
---

# R-BIN: binary wiring (turn 3)

Scope: I-BIN (4b80e0c1: plugin_bridge admin channel, portal Disable all and admitted set, ui/slot/import hooks, blob
pumping, kill-list SourceTrust), I-MAIN (ae7c0950: main.rs import hooks + blob fallback slot, portal_host install and
bound-bundle routing, fe-webview OpenBoundBundle), X-HOSTUI (307d6643: ui_registry, slot_drain, import_hooks). Inbound
hand-offs addressed to these files from R-ADMIN, R-GOV, R-API, R-BND, R-PLUG, R-UI and R-POR were re-verified at HEAD.
Code-first: nothing was built or run.

## Verdicts

- R-BIN-01 FIXED plugin_bridge.rs `BusExtensionHost::administer`: still executed DisableAll / EnableAll / Disable / KillListOverride for the local DID; fe-api refuses first today, but one forgetful door would reopen API administration. It now refuses every verb (`admin-gui-only`); `admin_audit` and the unread `ExtensionHostParts.manager` went with it.
- R-BIN-02 FIXED plugin_bridge.rs `ImportPort::convert` (steer 2026-11-13, SECURITY): one thread and one 16 MiB body per upload with no cap. An `ImportSlot` (max 4 in flight) is taken before the body enters the unbounded worker queue, rides the signal into the call thread and frees on drop; a full host answers `Failed` (502) at once.
- R-BIN-03 KEPT import call deadline (same steer): `LiveInstances::call`'s third argument is the decode stamp, not a deadline, and `call_hook` takes none. Each hook call refuels a fixed budget (hooks.rs:257) so a guest cannot spin forever, replies past the 30 s catalogue deadline are dropped (hooks.rs:280), and the adapter stops waiting at `IMPORT_DEADLINE`; with the 4-slot cap a stall cannot pile up threads.
- R-BIN-04 FIXED plugin_bridge.rs `sync_extension_ui`: an installed terrain build would have its first-party entries (main.rs `register_first_party_extension_ui`) unregistered when its instance stopped. Host-managed packages (`slot_drain::is_host_command`) are skipped.
- R-BIN-05 FIXED portal_host/mod.rs `follow_binding_changes`: reloaded an open bound portal on every binding-view republish (lazy starts, blob pumping, navigation all bump the generation), dropping page state; and split `binding:<petal>:<package>` on the first ':' although petal ids may hold one. It now reloads only when the petal's `portal.open` points changed for the same anchor (a new anchor records a baseline) and splits on the last ':'.
- R-BIN-06 FIXED main.rs dev tokens (R-API hand-off): `spawn_dev_token_service` minted a private client DID and portal_host got `local_client_did: None`. One DID is minted when dev tokens are on and passed to `spawn_dev_token_service_as` and `PortalHostConfig.local_client_did`.
- R-BIN-07 FIXED fractalengine/Cargo.toml (R-GOV3-04): added feature `entitlement = ["fe-format/entitlement"]` (off by default), so governance's row-6 code is buildable.
- R-BIN-08 FIXED plugin_bridge.rs + main.rs (R-GOV3-09): `GovernanceConfig::from_env` was used bare (always `NoEntitlementVerifier`). `ExtensionBridgeConfig.device_key` (read, never generated, from the DB thread's `LOCAL_DEVICE_KEY_ACCOUNT`) now feeds `with_device_entitlement`; without the feature or the key licensed builds stay refused.
- R-BIN-09 FIXED portal_host/mod.rs (R-POR-T3-08): the re-judge tick only ran on its 500 ms timer; it now also fires when `resolver.view().changed_since(..)` reports a governance generation move.
- R-BIN-10 FIXED portal_host/mod.rs (R-UI3-09): nothing fed `UiManager::portal_bundle_nodes`, so the bound offer always said "bundle not on this peer". `feed_bundle_node` lists a node whose summary reads `admitted` (local identity only) under its package id and drops it on any other answer. Only nodes the inspector has read are known.
- R-BIN-11 KEPT (verified) Disable all reaches portals (R-X-33): GUI-less CLI verbs go through ops_drain.rs:1691/1699 -> `BridgeOpsPorts::host_switch_changed` -> `portal_host_switch`; startup state is replayed by `link_portal_resolver`; Enable all is refused while `FE_EXT_DISABLE_ALL=1` (ops_drain.rs:1696); the QA kill switch is a separate flag Enable all never clears (resolver.rs:212).
- R-BIN-12 KEPT (verified) every new gate fails closed: unknown verse -> `default_access: None`; `admitted_bundles` empty until the portal links and stripped of installed extension builds; registry offers empty; plugin kill predicate is hash-under-any-id (`is_revoked_any`), terrain `no_kill_list()`; no blob source -> peers only; worker stop clears the slot.
- R-BIN-13 KEPT the plugin SourceTrust kill predicate ignores the Owner override: an override is keyed `(verse, content_hash)` and keeps only an installed build in that verse; a registry fetch has no verse, so refusing it is the fail-closed reading of "refuse + audited override".
- R-BIN-14 KEPT pinned names agree across I-BIN / I-MAIN: `link_portal_resolver(Arc<PortalGovernance>)` <- `resolver.view()`, `import_hooks_handle(&App)` -> `ApiConfig.import_hooks`, `ExtensionBindings::generation()`, one `BlobFallbackSlot` cloned into the sync thread, `BevyHandles` and the bridge, `admin_channel::spawn(AdminChannelPorts::new(ops.admin_jobs(), true))` (the GUI binary always has a window; the headless relay has no bridge).
- R-BIN-15 KEPT `plugin_bridge::portal_learn_consent`: redundant with portal_host's own hydration and Admit outcome, and its "no installed extension carries it" test can let an extension consent into the portal record mirror before the catalogue refresh; impact is bounded (local_facts strips installed builds; a non-bundle pair never mounts). HANDED-OFF the root fix to R-POR: `PortalGovernance::admitted_set` should list only pairs with verified web-bundle build facts.
- R-BIN-16 HANDED-OFF (terminal_reset owner) NEEDS fractalengine/src/terminal_reset/mod.rs `freeze_editor`: call `world.get_resource_mut::<crate::extension_host::admin_channel::AdminChannelHandle>().map(|mut handle| handle.shutdown())` beside the extension workers, so no CLI job lands during a reset.
- R-BIN-17 HANDED-OFF (blob.mirror owner, C-EXT) `ExtensionBlobMirror` is published but nothing calls `MirrorQueue::after_commit`: blob.mirror is inert until a blob-commit tap (fe-database upload/commit result or the binary's blob store wrapper) feeds it.
- R-BIN-18 HANDED-OFF (R-API / orchestrator, R-PLUG-07) NEEDS fractalengine-relay/src/main.rs:305: add `blob_fallback: Default::default(),` to the `BevyHandles` literal (compile break; peers only on the relay).
- R-BIN-19 HANDED-OFF (entitlement owner, C-EXT) `BridgeOpsPorts` does not implement `licence_identity` / `request_licence_token`: Browse "Get licence" and token requests answer `not-available`.
- R-BIN-20 HANDED-OFF (R-UI / C-EXT) the Extensions panel has no Disable all / Enable all control (no `ExtensionOp` variant): only `fe-ext admin` reaches the host switch.
- R-BIN-21 HANDED-OFF (R-UI / C-POR, R-UI3-10) nothing writes `BrowserCommand::OpenBoundBundle`; fe-ui opens a bound bundle through its repository node (`OpenBundle`), so the fe-webview bound route and `follow_binding_changes` stay idle until a binding-anchored open exists. Documented in src/AGENTS.md.
- R-BIN-22 HANDED-OFF (R-STORE) fe-hexon/tests/remote_registry_test.rs:186,225 build `SourceTrust` without `is_killed` (test compile break; same lesson as the steer).
- R-BIN-23 HANDED-OFF (orchestrator / R-Z, leak) the working-tree `conductor/.conductor_session_log` carries hook-written lines naming the private sibling repo's track paths; do not commit those lines to the public tree.

## Inbound hand-offs re-verified

- R-ADMIN-04 closed (extension_host/mod.rs declares `admin_channel`; plugin_bridge spawns it). R-ADMIN-05 / R-GOV3-06 closed except the licence ports (R-BIN-19): `audit_change_via` (channel note), durable `audit_admin_refusal`, `persist_withdrawal`, `host_switch_changed`, `forget_installation`.
- R-GOV3-07 / R-GOV3-08 closed: `note_acceptance` after each durable accept/withdraw; hydration reads `governance.local_acceptances()`.
- R-BND-3 closed (`default_access`, `admitted_bundles` in `LocalFacts`). R-PLUG-06 closed (activator egress buckets shared with `build_net`).
- R-POR-T3-06 / T3-12 closed (fe-webview `OpenBoundBundle` arm; `mod install` + `configure_install`). Steer 2026-11-13 (1): both SourceTrust literals set `is_killed`.

## Tests

None written or run (code-first turn). Journeys to extend in refine, inside existing batches: the import adapter
journey (a fifth concurrent call answers Failed), the ui_registry journey (terrain rows untouched), and the
portal host's summary path (admitted summary lists the node).
