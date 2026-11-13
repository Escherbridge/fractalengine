---
type: review
track: extension_platform_20261107
gate: R-Z
reviews: [C-EXT, C-PHR, C-POR, C-PM, C-HRM]
date: 2026-11-13
head: f2a0a03d
verdict: NOT MERGE-READY (seams agree; 2 FIXED; 2 compile breaks, 2 GUI scene-door gaps and 1 working-tree leak open as NEEDS)
---

# R-Z: final cross-tree gate, turn 3 (public record)

Read-mostly, fix-only, nothing built or run (code-first directive; clippy is never run). Checked at `f2a0a03d`
(54 commits since `f087dd9a`, 233 public files touched). The private half of this gate (R-ZM) keeps its own record;
this file names no private paths.

Verdict key: FIXED (landed here, in a file a C-* slice owns), CLOSED (verified at HEAD), KEPT (reason),
NEEDS (exact change, owner outside R-Z's partition), DEFERRED (trigger).

## 1. Pinned seams (plan §4): both sides agree at HEAD

| Seam | Verdict | Evidence |
|---|---|---|
| admin protocol + transport | CLOSED | `fe-plugin/src/admin/protocol.rs:26,38,189`; `transport.rs:244,257`; `fe-plugin/src/lib.rs:4` |
| `admin_channel::spawn` | CLOSED | `fractalengine/src/extension_host/admin_channel.rs:164`, spawned `plugin_bridge.rs:4693` |
| `AdminJob`, `AdminJobSender`, `ConsentOrigin::Cli` | CLOSED | `ops_drain.rs:181,188`; `fe-ui/src/extensions/consent.rs:35,293` |
| `scene_write::{SceneWriteDoor, SceneWrite, submit}` | CLOSED | `fe-runtime/src/scene_write.rs:81,366,463` |
| `SetExtensionPolicy { actor_did }` | CLOSED | `fe-runtime/src/messages.rs:640`; `fe-database/src/lib.rs:798`; `governance.rs:916` |
| `StagedReading.provenance` | CLOSED | `fe-sdk/src/plugin_host.rs:116` |
| `LocalFacts.{default_access, admitted_bundles}`, `RegistryBuild.revoked` | CLOSED | `fe-plugin/src/bindings/resolver.rs:59,134,136`; every literal sets both (`resolver.rs:601`, `activation.rs:1442`, `plugin_bridge.rs:2554`) |
| portal resolver `admitted_set`, `host_disabled_handle`; `link_portal_resolver` | CLOSED | `portal_host/resolver.rs:257,271,528`; `plugin_bridge.rs:440` |
| `install_from_registry` | CLOSED | `portal_host/install.rs:87`, declared `portal_host/mod.rs:5`, configured `mod.rs:146` |
| governance generation | CLOSED | `governance.rs:659,664`; re-judge wakes on `changed_since` `portal_host/mod.rs:883` |
| ui_registry, slot_drain, import_hooks; `import_hooks_handle` | CLOSED | `extension_host/mod.rs:8,18,24`; `plugin_bridge.rs:4081` |
| store client `registry_source` v1 + `SourceTrust.is_killed` | CLOSED in production; **NEEDS** in one test (R-Z-03) | `fe-hexon/src/registry_source.rs:228`; `plugin_bridge.rs:4308`, `terrain_bridge.rs:912`, `tools/fe-ext/src/index.rs:590` |
| `fe_format::entitlement` | CLOSED | package-bound `WrapBinding` `fe-format/src/entitlement.rs:287`, `wrap/unwrap_content_key:337,366`, `encrypted_key_for(.., package_id):725`, `token_request_proof:810`; the private consumers call the 3-argument forms with `WrapBinding::Archive` and the shared `token_request_proof` |
| `fe_hexon_registry::store::{PackageStore, FsPackageStore, StoreError}` | CLOSED | trait `fe-hexon-registry/src/store.rs:51` (list/put/catalog_doc); the private object-store implementation matches all three signatures and uses only existing `StoreError` variants and `index::scan_dir` |
| `run_admin`, `run_index` | CLOSED | `tools/fe-ext/src/admin.rs:65`, `index.rs:111` |
| `BrowserCommand::OpenBoundBundle` | CLOSED (seam); writer still absent, see §3 | `fe-webview/src/ipc.rs:17`, arm `fe-webview/src/plugin.rs:389` |
| `FixtureOp::PortalKillSwitch` | CLOSED | `fe-runtime/src/qa_portal.rs:26`; `fe-api/src/qa_portal.rs:392`; `qa_portal_bridge.rs:73` |
| store-client contract v1 text = code = services | CLOSED | contract id, `registry_protocol: 1`, `x-hexon-registry-protocol`, `api/v1/catalog/` paths, signing domains `catalog-index/v1` and `entitlement-token-request/v1`, 300 s `iat` skew, `requires` rows and the eight token error codes are identical in `docs/spec/store-client/contract-v1.md:3,11,40,58-74,244-251`, the bus text, `fe-hexon` (`registry_source.rs:28,50`, `fe-hexon-registry/src/routes.rs:29`) and the private producer (which re-exports the public `IndexEntry`, `Publisher`, `SearchQuery`, `SearchSort`, `search_entries`, `valid_package_id`, `valid_version`, `INDEX_*` rather than copying them). The public text is a superset of the bus text (adds error codes and helper names, no conflict). Contract states issuers mint only from their own catalogue record's `encrypted_key` (`contract-v1.md:253`). |

## 2. Findings

- R-Z-01 FIXED (R-TOOL-03, HIGH, never closed or named by any critic) `fe-ext index verify` and the self-verify in
  `index build` handed `StaticIndexSource::new` the catalogue dir, and the source appends `api/v1/catalog/` itself
  (`registry_source.rs:1574`), so every verify read `<root>/api/v1/catalog/api/v1/catalog/config.json` (J8 index legs,
  `stage.ps1 -Loopback` and run.py staging would all fail). New `endpoint_base` strips the suffix for the source only;
  `locate(base, ..)` keeps the catalogue dir for index-relative locations (`tools/fe-ext/src/index.rs` `endpoint_base`, `verify_tree`).
- R-Z-02 FIXED (C-PHR-05 doc) `fe-hexon/src/registry_source.rs` `ProtocolV1Transport` doc named fe-plugin's
  `RegistryClient`; it now names its only implementation, `remote::RemoteProtocolV1Transport`.
- R-Z-03 NEEDS (compile break, `cargo test -p fe-hexon --features remote`; C-EXT H-3 / R-STORE-07 / R-BIN-22) `fe-hexon/tests/remote_registry_test.rs:186,225`:
  add `is_killed: fe_hexon::registry_source::no_kill_list(),` to both `SourceTrust { .. }` literals.
- R-Z-04 NEEDS (compile break, relay binary; C-EXT H-2 / R-PLUG-07 / R-BIN-18) `fractalengine-relay/src/main.rs:297`:
  add `blob_fallback: Default::default(),` to the `BevyHandles { .. }` literal (every other literal has it: `fe-runtime/src/app.rs:1085,1115`, `fractalengine/src/main.rs`).
- R-Z-05 NEEDS (MISSING scene door, C-EXT-05) `fe-ui/src/dialogs/gltf_import.rs:39` sends `ImportGltf` (census row
  `scene.create`, `scene_write.rs:33`) through raw `send_or_toast`: route it through `try_submit_scene_write_on` and start
  `gltf_import_request` only on an admitted or queued answer.
- R-Z-06 NEEDS (MISSING scene door, C-EXT-06 / R-UI3-07) `fe-ui/src/verse_manager/db_results/nodes.rs:71` sends the
  earthwork `SetNodeProperty` keys straight to `db_sender.0`: send each through `submit_scene_write(db_sender, ..)`.
- R-Z-07 NEEDS (C-EXT-07) `fe-ui/src/node_manager/transform_broadcast.rs:178` still gates a proxy `UpdateNodeTransform`
  while `undo.rs:559` submits the real commit through the door: one gimbal move asks `scene.transform` twice. Drop the
  proxy and read the refusal from the undo commit's door outcome (steer 2026-11-13).
- R-Z-08 NEEDS (leak, merge blocker; R-BIN-23) the working-tree `conductor/.conductor_session_log` carries hook-written
  lines with the private sibling tree's absolute paths. Do not commit them: restore the file
  (`git checkout -- conductor/.conductor_session_log`) or drop those lines before any commit.
- R-Z-09 KEPT (R-STORE-18) the CI signing secret name added by `6d90b241` (`.github/workflows/build-artifacts.yml:472,496`,
  `release.yml:78,101,154,178`, `docs/release-signing.md:14,21`) contains a leak-list word, but it is Azure's own
  directory-id variable consumed by Microsoft's credential chain, not commercial content; renaming the env key breaks signing.
  The repository secret could be renamed (the env key cannot); user call. Every other turn-3 added line and every touched
  file's full text is clean of the leak list (pre-existing generic security wording in `fe-api` only).
- R-Z-10 CLOSED (C-EXT H-7) R19 order: the code tests `not-local-identity` first (`fe-plugin/src/lifecycle.rs:663`), and
  `docs/extensions/governance.md` §Activation preconditions says "For a non-local actor, row 2 is evaluated first". For a
  local actor row 2 always passes, so the first refusal equals the table order: code and spec agree; no change.
- R-Z-11 CLOSED (C-PM AC-9 web half) a kill-listed web bundle has no static-sandbox fallback: revocation is judged before
  bytes (`docs/spec/portal-bundle/admission.md:25-27,36`) and a revoked verdict on a Static mount refuses
  (`fe-webview/src/portal_bundle/admission.rs:473`).
- R-Z-12 CLOSED at HEAD (re-verified): R-ADMIN-04 spawn, R-GOV3-04 (feature `entitlement` exists, off by default,
  `fractalengine/Cargo.toml:92`), R-GOV3-06 ports (`plugin_bridge.rs:3061-3113`), R-GOV3-07 (`note_acceptance` at
  `plugin_bridge.rs:3074,3092`), R-GOV3-08 (`local_acceptances` `:2526`), R-POR-T3-06/08/12, R-API dev token shared DID
  (`main.rs:826`), R-PLUG-06 (C-EXT R9 row), R-BND-3, R-TOOL-02, R-STORE-08 (C-EXT-02).
- R-Z-13 NEEDS (R-BIN-16, LOW) `fractalengine/src/terminal_reset/mod.rs` `freeze_editor` (~:267) stops services and plugin
  leases but not the admin channel: add `if let Some(mut handle) = world.get_resource_mut::<crate::extension_host::admin_channel::AdminChannelHandle>() { handle.shutdown(); }`.
- R-Z-14 NEEDS (docs) `fractalengine/src/AGENTS.md:1861` §Broker role source (C-POR-03: the broker shares
  `ExtensionBusParts` via `from_parts`; `NodeDidRoles` only when no extension host ran); `fe-plugin/wit/fractal-plugin-2.wit:1-2`
  drop the stale `UNSTABLE - not yet bound` header (H-6); `fe-ui/src/actions/AGENTS.md` add the §scene-write-door section
  `actions/node.rs:4` points at (H-9); `fe-plugin/src/AGENTS.md:317-331` registry-install header and `open_paid_entry`
  signature (H-9, C-PHR); `SECURITY.md:97-99` and `conductor/tracks.md:115` stale claims (C-PHR).
- R-Z-15 NEEDS (test reachability, D-BATCH owner) `conductor/test-batches.md`: add `extension_ui_tests::`,
  `server::extension_mount_tests::` and `panels::plugin_install_panel` filters, or delete the orphaned v1 panel
  (`fe-ui/src/panels/mod.rs:15`) with its test (C-PHR-02/03).
- R-Z-16 DEFERRED, named with triggers by their critics (not code gaps this turn): entitlement token wiring in the bridge
  (H-4, trigger: identity key handed to `ExtensionBridgeConfig`), blob.mirror commit tap (H-5 / R-BIN-17), R26 doc link
  check (H-8), AC-6 real-registry plugin journey (C-PHR-04), `BrowserCommand::OpenBoundBundle` writer and
  `portal_bundle_nodes` feeder (R-BIN-21 / R-UI3-09/10; the inspector shows "bundle not on this peer", never a dead Open),
  registry-offered builds (`plugin_bridge.rs:2565` `registry: Vec::new()`, fail closed), R-PLUG-08 (LAN-resolving
  operator registry name; operator-trusted by standing default), R-ADMIN-07 (KEPT by C-EXT).
- R-Z-17 PARTIAL by directive (not counted as missing): P4.4 GUI QA, P6.3 runs, R16 portal QA lane, PHR sweep, P6.5 retro:
  written, unrun; they belong to the refine pass and the orchestrator's turn-end step.

## 3. Critics' tally

C-EXT 0 missing requirements but 2 MISSING GUI write paths (R-Z-05/06, open); C-PHR 0 missing ACs (one missing journey,
deferred R-Z-16); C-POR 0 missing. The two private critics' items are judged in R-ZM's record. **Zero missing is not reached**
until R-Z-05 and R-Z-06 land.

## 4. Merge readiness (read-only)

- `main` is 54 commits ahead of `origin/main`; no conflict markers, `todo!`, `unimplemented!` or `dbg!` in turn-3 added lines.
- Working tree: `Cargo.lock` (+9 dependency lines from the barrier checks: fe-ext bs58/chrono/tokio, fe-format entitlement
  deps) is expected and belongs to the refine L4 commit; `extension_platform_20261107/checkpoint.md` is orchestrator-owned;
  `conductor/.conductor_session_log` must not be committed as is (R-Z-08).
- Blocking before the refine build: R-Z-03, R-Z-04 (compile), R-Z-05, R-Z-06 (missing door), R-Z-08 (leak).
  Everything else is non-blocking (NEEDS docs/tests, DEFERRED with triggers).
