---
type: review
track: extension_platform_20261107
title: R-UI — Extensions UI review-and-fix (UI-UX guidance, extensions-ui.md, bindings.md section 9), swarm turn 2
resource: ../metadata.json
---

# R-UI — Extensions UI (turn 2, over I-UI `4842be81`, B9 `1c48a5d0`, B10c `685f4e30`)

Review-and-fix, code-first (nothing built or run; files parse under `rustfmt --check`). Measured against
`C:/Users/atooz/.gemini/UI-UX-guidance.md`, `docs/extensions/extensions-ui.md` and `docs/extensions/bindings.md`
sections 5.2, 5.5, 5.7, 6 and 9. One verdict line per finding: **FIXED** (edited here), **KEPT** (checked, left as is,
reason given), **HANDED-OFF** (owner named), **DEFERRED** (trigger named).

## A contribution rendering outside its slot or after disable

- R-UI-01 FIXED `extensions/mod.rs`, `bound_slots.rs`: a bound `ui.panel` drew every command and stream of the package and hid all its toolbar, sidebar and overlay entries elsewhere, while unbound panel content drew in the plain slots. bindings.md section 6 says only the `uses` ids render, only while the binding resolves ("No extension UI at the scope" otherwise). Now `BoundSlotsState::set_panel_uses` records the ids; plain slots skip panel content; inspector and context entries among it draw in place only while bound; the panel draws only `uses` and names the package. With no `uses` recorded a panel draws nothing (fails closed).
- R-UI-02 FIXED `panel.rs`, `mod.rs`: a disabled or uninstalled package's contributions kept drawing, because nothing unregisters them. The slot host now skips any package the last installed-list snapshot reports not Enabled, or that left the list (`ExtensionsPanelState::stopped_packages`), copied only when `rows_generation` moves (no per-frame clone). J9 asserts Enabled, Disabled and uninstalled.
- R-UI-03 HANDED-OFF I-BIN / R-GOV (`fractalengine/src/plugin_bridge.rs`): no third-party `ui` entry is registered into `UiExtensions` today. At activation, register the build's manifest `ui` entries and call `BoundSlotsState::set_panel_uses(package_id, provides[ui.panel].uses)`; on disable, revoke and uninstall call `UiExtensionRegistry::unregister_all`, `ExtensionSlotState::forget_plugin` and `BoundSlotsState::forget_package`.
- R-UI-04 HANDED-OFF orchestrator (`fe-ui/src/extensions/slots.rs` has no turn-2 owner): `ExtensionCommandRequest` carries no args, so a ContextMenu command never receives the manifest's `{"node_id": "<clicked>"}`; and the menu's `when` reads only node `id` and `name`, so fleet-sim's `start-this` (`when: fleet.config`) can never show. Add `args` to the request (set by `draw_context_menu_items`) and a per-node property source for `when`.
- R-UI-05 FIXED `ui_shell/topbar.rs`: at compact width extension toolbar actions drew inline next to the Tools menu, so third-party buttons could push the right-hand cluster off the bar. They now list inside the Tools menu.
- R-UI-06 FIXED `mod.rs` `draw_context_slot`: a separator drew even when no entry's `when` matched the node. Entries are filtered first; nothing matching draws nothing.
- R-UI-07 KEPT `extension_overlay_system`: non-interactive, read-only stream values in the free viewport's corner, after the same filters as the plain slots.

## Consent or the binding edit form reachable by a non-Manager

- R-UI-08 FIXED `bindings.rs`: only the draw checked `can_manage`; `start_edit`, `build_save` and `request_binding_clear` did not, and a refreshed snapshot that dropped Manager left the form open with Save enabled. All three now check `BindingsSnapshot::manages()`, and `apply` closes the form on demotion. J9 drives a viewer snapshot and asserts no op can be queued.
- R-UI-09 FIXED `bindings.rs`: the form stored the chosen scope as an index into `scopes`, so a refreshed snapshot with a different list could silently move a write (fractal to verse). It now stores the `ScopeTarget`, and `build_save` refuses a target that is not writable in the current snapshot.
- R-UI-10 FIXED `bindings.rs`: Clear was offered on a winning row at a scope the local DID does not manage (a petal Manager clearing a verse row: a guaranteed 403, a dead control). Remove now shows only when that scope is writable, and it moved into the form as `Remove binding at <scope>`.
- R-UI-11 FIXED `bindings.rs`: the kill-list Override offer showed on every peer (the resolver attaches it by reason alone); bindings.md section 5.2 row 6 limits it to the Owner's own host. New `BindingsSnapshot::local_is_owner` gates it; Edit binding stays Manager+ (`visible_offer`).
- R-UI-12 KEPT consent and the other offers: Install, Update, Review permissions, Allow in this verse and Enable are local-DID actions (governance precondition 2, bindings.md section 5.2), not Manager actions; the edit form is the only Manager+ surface.
- R-UI-13 HANDED-OFF R-GOV (`extension_host/ops_drain.rs`, the `RefreshBindings` arm): fill `local_is_owner`; put in `scopes` only the scopes where the local DID holds Manager+ (subtree authority, an owner pin cuts it); attach Install and Update only when a source has a fitting build (sections 5.2 and 5.5), so the view never shows a dead control.

## Accessible names

- R-UI-14 FIXED `bindings.rs`: offers were named `binding:<hook>[:<key>]:<install|override|...>`; bindings.md section 9 names the one action `...:action`. Renamed, and J9 now asserts `binding:petal.map:action` and `binding:ingest.decode:acme-v2:action`; the Manager button is `:edit` (was `:edit-form`).
- R-UI-15 FIXED `bound_slots.rs`, `panels/egress_card.rs`: bound snippet rows had no state name and used the provider's label verbatim, so a provider could title its row like a built-in target ("DuckDB"). Rows are now named `binding:egress.snippet:<key>` and labelled `<label> · <package id>`, the label cut to its first line, control characters stripped, 48 characters.
- R-UI-16 HANDED-OFF D-EXT / orchestrator (docs): `extensions-ui.md` lacks the Bindings view row and the names `Extension bindings`, `Refresh bindings`, `Toggle Extensions panel`; bindings.md section 9 should list the Manager control suffixes (`:edit`, `:scope:<petal|fractal|verse>`, `:mode:<extension|builtin>`, `:provider`, `:version`, `:field:<name>`, `:save`, `:cancel`, `:clear`) and the surface names `binding:ui.panel:<key>`, `binding:egress.snippet:<key>`.
- R-UI-17 KEPT `panel.rs`, `details.rs`, `permissions.rs`, `browse.rs`: every name listed in extensions-ui.md is present and stable.

## Helper text standing in for layout

- R-UI-18 FIXED `bindings.rs`: a row carried up to three buttons (offer, Edit, Clear) and a state sentence that repeated the package id. A row is now title and state word, one detail line (scope, package name, range), then at most the offer and Edit; Remove sits in the form beside Save; no second Edit when the offer is Edit binding.
- R-UI-19 KEPT `panels/egress_card.rs` "Paste these into PowerBI, DuckDB, or a spreadsheet.": pre-existing, outside the extension change; flagged for the next egress-card pass.
- R-UI-20 KEPT `topbar_toggle` hover text: the same pattern as the sibling section buttons.

## Empty, loading, error and disabled states

- R-UI-21 FIXED `bindings.rs`: after a failed `RefreshBindings` the view read "No active petal" and offered no retry (`requested` never reset; Refresh drew only with a snapshot), a dead end. It now reads `Bindings are not loaded` with `Refresh bindings`; J9 covers it.
- R-UI-22 FIXED `panel.rs`, `mod.rs`: on a petal change the installed rows (their binding id is the open petal) and the bindings snapshot went stale; Enable then failed "not the open petal", and the form kept the old petal's scopes. `on_scope_changed` makes the next draw ask again; J9 asserts both ops.
- R-UI-23 FIXED `bindings.rs`: the provider picker drew an empty list when no installed extension provides the hook; it now says so in one line.
- R-UI-24 FIXED `bound_slots.rs`: the snippet cache was keyed by target only, so after the export source changed the previous query's provider snippet stayed copyable for up to 1 s. Answers are now keyed by the call arguments: hidden at once and asked again.
- R-UI-25 FIXED `bound_slots.rs`: an undrained `requests` list grew by one per target per second; it now holds one request per target, and the card builds no call arguments when nothing is bound.
- R-UI-26 FIXED `bound_slots.rs`: freshness mixed clocks (egui input time for requests, the feed's Bevy `Time`, virtual and lagging after a hitch, for answers). It now counts from the request on the UI clock; the feed's time argument is ignored and kept so I-BIN's in-flight call compiles unchanged.
- R-UI-27 DEFERRED: an answer still in flight when the query changes is attributed to the newer request and can show for at most the 1 s provider deadline. Trigger: the feed's `SnippetAnswer` carries the request's args (NEEDS on `plugin_bridge.rs`), then `store_snippet` takes the `SnippetRequest`.
- R-UI-28 HANDED-OFF R-GOV (`extensions/consent.rs`): the consent window uses the default Middle order, so clicking the Extensions window raises it over an open review; set `.order(egui::Order::Foreground)` on it.
- R-UI-29 DEFERRED: binding a point that has no row yet (a new `api.routes` key, a keyless hook nobody bound) is REST/MCP-only; the GUI edits existing points. Trigger: the G4-C17 run or a user ask.
- R-UI-30 DEFERRED: the unresolved state of `egress.snippet` and `ui.panel` at the feature itself (section 5.5) does not show, because the feed passes only resolved points; the Bindings view shows it. Trigger: the feed exposes unresolved points.

## An untyped override confirm

- R-UI-31 FIXED `panel.rs` `route_consent`: it forwarded any `confirm` string. A mismatch now queues only `DiscardStaged` and shows `confirmation-mismatch`, a third layer after consent.rs's typed gate and ops_drain's check; J9 covers it.
- R-UI-32 KEPT: the Bindings view's Override goes `TakeBindingOffer` -> bridge `ConsentNeeded` with `kill_list` -> the typed dialog; no other path builds `ExtensionOp::KillListOverride`.

## Other

- R-UI-33 FIXED `actions/mod.rs` `ToggleTerrainVisibility`: the layer flag was read through a borrow held in the match scrutinee across the `&mut petal_map` write; it is now read first.
- R-UI-34 FIXED `extensions/AGENTS.md`: consent.rs and install_offer.rs pointed at sections that did not exist (`§consent`, `§install-offer`); added, with `§bound-slots` and `§bindings-view`, and `§host-handle` corrected to the `uses` rule.
- R-UI-35 KEPT `plugin.rs` ordering: lend before `gardener_ui_system`, overlay and window after, reclaim after both; every slot surface draws inside `gardener_ui_system`, so none runs outside the lent window.
- R-UI-36 KEPT `dialogs/context_menu.rs` and `panels/inspector.rs` mounts: node targets only; the inspector slot waits for `node_properties_loading` to clear.

## G4-C17 GUI QA path (runs at refine; nothing here ran)

Driven through the agent input surface (`tools/qa/fe.py`, tools/qa/AGENTS.md §fe-py) by accessible name, `--exact` on every
locator. `<id>` = `org.example.fleet-sim` (also its installation id in ops_drain), `<B>` = the binding label
`Fleet simulator in <verse name>` (ops_drain `rows_event`).

**Preconditions.** Build gate open; `launch.ps1` run with input opt-in and isolated QA storage; a dev publisher key listed in
the run's trusted publishers with the dev flag set; `tools/fe-ext pack` of `extensions/fleet-sim` at 0.1.0 (file A) and at
0.1.1 with one added egress grant (file B), both signed with that key; B placed in a local Extensions source configured in
the run's `extension_sources.json`; a verse the QA identity owns, with a petal open; a verse policy admitting `<id>` (an
`allow-list` or `trust-root` row set with `fe.py api` as fixture setup; the default `first-party-only` refuses a dev-signed build).

1. **Open.** `click --name "Toggle Extensions panel"`; `wait-for "Installed extensions"`; the list reads `No extensions installed`; `Install from file` is disabled.
2. **Install.** `type "<file A>" --into "Package file path"`; `click --name "Install from file"`; while verifying, `Install from file` is disabled.
3. **Consent.** `wait-for "Enable <id>"`; `ui` shows the grant sections, `Signer`, `Policy`, `Licence`; `Enable <id>` is enabled only from the second frame (list drawn once). `click --name "Enable <id>"`; `wait-for "Open <id>"`; the row reads `Fleet simulator 0.1.0`, `Disabled`, `<publisher> · trusted publisher`.
4. **Enable.** `click --name "Open <id>"`; `click --name "Enable <B> for <id>"`; `wait-for "Disable <B> for <id>"`; status `Enabled`. Once R-UI-03 lands: `wait-for "ext:<id>/start"` in the topbar and `ext:<id>/movers` in the viewport overlay; `ext:<id>/start-this` is absent from the node context menu (R-UI-04).
5. **Disable.** `click --name "Disable <B> for <id>"`; `wait-for "Enable <B> for <id>"`; `ext:<id>/start` and `ext:<id>/movers` are gone on the next frame (R-UI-02 fails closed even before R-UI-03).
6. **Update with a delta.** `click --name "Browse extensions"`; pick the source (`Extension source`), `click --name "Refresh extension list"`; `click --name "Installed extensions"`; `click --name "Open <id>"`; `click --name "Update <id> to 0.1.1"`; the row reads `verifying update`; `wait-for "Update <id>"` (the re-prompt dialog `Review changes to Fleet simulator`, with the `+` egress line and `The running version stays on until you accept.`); `click --name "Update <id>"`; `wait-for "Open <id>"`; version `0.1.1`. Declining instead (`Cancel extension consent`) must leave 0.1.0 running.
7. **Uninstall.** `click --name "Uninstall <id>"`; `Remove Fleet simulator and stop all its bindings?`; `click --name "Confirm uninstall <id>"`; the view returns to the list; `Open <id>` and every `ext:<id>/*` name are absent.
8. **Bindings leg (optional, same run).** `click --name "Extension bindings"`; `wait-for "binding:petal.map"`; as Owner, a fractal `petal.map` binding to a missing package shows `binding:petal.map:action` (Install); as a Viewer peer neither `binding:petal.map:edit` nor an Override `:action` exists.

Evidence per step: `shot --label rui-<n>` and the `ui` tree. A step that cannot run (fixture missing, R-UI-03 pending) is recorded
as not run, never as passed.
