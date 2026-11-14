---
type: Review
---

# R-INT-3: adversarial review of I-UI-1 (security review included)

Diff reviewed: `git diff 7af0a144 -- <I-UI-1 owns>` = commit `54c63f91` only (no later commit touches these seven files). Read against turn4-plan.md §4 pinned seams, the I-UI-1 task list and the upstream modules at HEAD (`ground_snap.rs`, `road_chain.rs`, `hierarchy_info.rs`, `property_value.rs`, `tree_appearance.rs`, `dimension_callout.rs`, `entity_access.rs`, `fe-runtime/src/entity_access.rs`). Security inbox: no entry names a file this gate owns. Leak scan of every added line: clean. No cargo was run.

## Routed edits (each applied once, correctly placed)

- CITY-03 `fe-ui/Cargo.toml:22` fe-format with default-features=false; `fe_format::scale` is not feature-gated (fe-format/src/lib.rs:17). KEPT (correct).
- CITY-04/06/07 `fe-ui/src/node_manager/mod.rs:33-38` three `pub(crate) mod` lines; `RoadBuilderState` init at :425; seventh gesture at :231 (`is_active`) and :255 (`cancel_stage`), arity test renamed to seven sources. Signatures match road_chain.rs:180/:200 and ground_snap.rs:27. KEPT (correct).
- CITY-04 `fe-ui/src/plugin.rs:1534` cursor point now `ground_snap::ground_point(&ray, height_field.as_deref())`; consumers checked (sculpt ring, context-menu placement, GIS card): all take the y from the point, none assume y=0. KEPT (correct).
- CITY-10 Tool::Road added to both activation arrays (plugin.rs:700, :734) and panels/mod.rs:919; the task's third array (":840") does not exist at HEAD. KEPT (no third array).
- HEXON-19 `plugin.rs:1098` callout labels in the same EguiPrimaryContextPass tuple, after `gardener_ui_system`. KEPT (correct).
- UI-06 `panels/mod.rs:14`, `plugin.rs:907` and the card at `panels/inspector.rs:98`; data reaches `right_inspector` through `InspectorFormState.hierarchy_card` (refreshed at plugin.rs:1284 via inspector.rs:139) instead of new params, so right_sidebar.rs (not owned) still compiles. KEPT (sound substitute for the planned param threading).
- UI-07 `token_expiry_label` pub(crate); the one `EntityAccessSnapshot` literal in fe-ui (actions/mod.rs:2687) carries `grants`. KEPT (correct).
- UI-08 presets asserted inside `entity_access::role_confirmation_refresh_journey` (called at actions/mod.rs:2696). KEPT (correct).
- DATA-01 Viewer gating at inspector.rs:2208 (fail-closed: `role: None` cannot edit, plugin.rs:401), save/delete filtered at :2237, Add disabled; typed parsing through `property_value::parse_selected`. KEPT (correct).
- DATA-02 automatic-read leg and DATA-03 TreeAppearance layout leg: every helper they call exists with the used signature (group_controls, maintain, route, replace_groups, dtos_from_groups, AppearanceDraft::new, render, observations().controls). KEPT (correct).

## Findings

- F1 FIXED `fe-ui/src/panels/inspector.rs:2145` (security, UI honesty): an edit draft opened while the role was Editor kept an enabled Save after a demotion to Viewer; the save was dropped silently by the `filter`. `render_property_draft` now takes `can_edit` and disables Save.
- F2 HANDED-OFF (UI-06 owner, `fe-ui/src/panels/hierarchy_info.rs:186`): the card's "Your role" and Settings gate use `LocalUserRole`, which is resolved at the deepest active scope; a verse or fractal card pinned by override can show a petal-only grant as the role there. UI only; the access dialog's DB reads and writes stay authoritative.
- F3 KEPT (converges, by design) `node_manager/shortcuts.rs:322`: the pending-selection branch calls `cancel_all` every frame, so an open road chain ends over two frames (stage per call) rather than one. No stuck state.
- F4 HANDED-OFF (I-UI-2): `panels/tool_options.rs:15` imports `node_manager::road_overlay`, which is not declared yet; I-UI-2 owns the `road_overlay`/`road_interaction` declarations, so fe-ui does not compile at this barrier until I-UI-2 lands.
- F5 KEPT (UI system, not a hot path) `panels/inspector.rs:139`: `inspected_entity` clones an id String each frame; the peer mirror is rebuilt only on registry change.
- F6 KEPT (harmless) `panels/inspector.rs:152`: the kind-change token reset is redundant with `synchronize_token_target`, which already clears tokens whenever no node is selected.

Security review: no DbCommand/DbResult arms, routes, replication admission, relay, licence-token or auth code in these hubs. The only security-relevant wiring is Viewer gating; it is fail-closed and the DB write gate stays authoritative.

Verdict: PASS-WITH-FIXES (1 fixed, 2 handed off, 3 kept); I-UI-1's wiring matches the pinned seams.
