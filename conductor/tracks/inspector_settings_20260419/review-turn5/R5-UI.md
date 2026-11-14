---
type: Review
track: inspector_settings_20260419
slice: R5-UI
reviews: [UI5-01, UI5-02, UI5-03, UI5-04, UI5-05, UI5-06]
date: 2026-11-14
---

# R5-UI adversarial review (round 5, group UI)

Reviewed against `git diff a85dc9ad` for every upstream-owned file, prompted to refute. Security review included (UI5-05 bound). Second pass (resumed run): the first pass's fixes (commit 39e0899e) were re-read, its two hand-offs were re-checked against HEAD and are now landed, and one new defect was fixed.

## Findings

- F1 FIXED - fe-ui/src/panels/hierarchy_info.rs:207 - INT5-08 asks for the role line AND the Settings button to follow the card's own scope; UI5-02 scoped only the role line, so a petal-only Editor could open a verse's Settings from the verse card. Settings is now denied ("Open this verse to use its settings") when the card is not the deepest active scope (fail-closed); the journey row goes red without it.
- F2 FIXED - fe-database/src/role_grants.rs:484 - `load_scope_statements` took any caller limit: no hard cap. `MAX_SCOPE_STATEMENTS = 1024` is applied inside the function (:492), so no caller can bypass it; the journey asks for `usize::MAX` and asserts the cap.
- F3 FIXED - fe-database/src/role_grants.rs:495 - SQL literal carried a run of stray spaces from a lost line continuation; restored as a `\` continuation (same query).
- F4 FIXED - fe-ui/src/panels/inspector.rs:1582 - revoke send-failure copy told the user to reopen the Inspector, which cannot help a closed DB channel; now names the cause and the action.
- F5 FIXED - fe-terrain/src/volume_report.rs:85 - doc typo "Palace proposals" -> "Palette proposals".
- F6 FIXED - conductor/tracks/polish_lane_20261031/r2-closed-matrix.md:98 - DW-04 note duplicated the OUT reason; X-13 residual ended in a dangling "Owner:".
- F7 FIXED (by I5-MANIFEST, 518714e6; re-verified) - fe-terrain/src/lib.rs:25-26 - `#[cfg(feature = "render")] mod proposal_layer_mirror;` now present, matching the call at fe-terrain/src/terrain_plugin.rs:623.
- F8 FIXED (by I5-UI, a49adc8b; re-verified) - fe-ui/src/panels/mod.rs:403 - `mark_verse_local_only_from_tree(&mut ui_mgr.active_dialog, hierarchy)` runs every frame inside the `dialog_entity_settings` guard, before the dialog renders.
- F9 KEPT (task-authorized; the pure rule is the seam fe-ui can test without fe-terrain) - fe-ui/src/ui_shell/right_sidebar.rs:1998 - the `show_heatmap_checkbox` rows are close to a restatement of `a && b`; the mirror rows carry the mutation value.
- F10 KEPT (task allows the temp-data key; goes red if `.ok()` is restored) - fe-ui/src/panels/inspector.rs:4580 - the revoke leg asserts the temp key, not the painted label.
- F11 KEPT (an egui card, not a per-frame ECS hot path; `describe` already clones its strings) - fe-ui/src/panels/hierarchy_info.rs:192 - the scope compare builds two scope Strings per frame while the card is visible.
- F12 KEPT (verified) - fe-ui/src/dialogs/entity_access.rs:187 - the pinned wrapper allocates nothing (iter/find/is_none on the borrowed tree); unknown verse => not local only.
- F13 KEPT (verified) - fe-ui/src/panels/inspector.rs:1534,1604 - no bare `.send(..).ok()` remains in inspector.rs; revoke and list sends warn, revoke also paints its error.
- F14 KEPT (display only; authz unchanged) - fe-database/src/handlers/entity_access.rs:190 - LIMIT 257 applies before the in-handler `retain(bounded_identity)`, so oversized-identity rows among the newest could shorten the page below 256; they never widen it.
- F15 KEPT (disclaimed in the file) - conductor/tracks/polish_lane_20261031/r2-closed-matrix.md:11 - anchor line numbers drift a few lines after round-5 edits. Second pass: scripted check of all 82 rows: 82 unique ids, every cited file exists and every line is within the file; I-02 (inspector.rs:1617 PROPERTY_TYPES) and X-06 (terrain_tools_panel.rs:316 single-click delete) re-read and still accurate.
- F16 FIXED - fe-ui/src/ui_shell/right_sidebar.rs:157-164 - UI5-03 called `mirror_proposal_layer_hidden(&mut state, ..)` through `ResMut`'s DerefMut, so `RightSidebarState` was flagged changed on EVERY frame, defeating the system's "writing only on change" contract (anti-pattern 7). Now compares through `bypass_change_detection()` and calls `set_changed()` only when the hidden flag flips, the same pattern as the heatmap mirror below it.
- F17 KEPT (residual, not this round's scope) - fe-ui/src/panels/terrain_tools_panel.rs:296,316 - R2 X-06/X-13/R-02 (Debug op names, single-click proposal delete) are listed as open in the matrix residuals for round 6; fixing them is UX work outside every UI5 task.
- F18 KEPT (verified) - fe-terrain/src/section_responder.rs:77 - the closure now delegates to `volume_report::combined_height` (:86), whose body is identical to the deleted closure (proposal loop, then `composite_proposed_height`); the journey leg at :216-233 pins both the shared function and the section's fill against it.

## Security review (UI5-05)

- Bound cannot be bypassed: the cap lives inside `load_scope_statements` (fe-database/src/role_grants.rs:492); `limit` is a `usize` formatted into the SQL (no text input), `scope` stays a bound parameter (no injection). The only callers are the handler (entity_access.rs:190) and the R7 journey.
- Authz on the read unchanged: `load` still runs `authority(db, target, local_did)` (entity_access.rs:71) before `grant_record` (:174); the diff is one call-site line.
- Ordering: `hlc` is written as a decimal string (`stmt.hlc.to_string()`; invite.rs:363 also binds `.to_string()`), so `string::len(hlc) DESC, hlc DESC` orders numerically; the handler's `u64` sort stays the display authority.
- Fail-closed: the hierarchy card denies Settings for any card that is not the active scope (F1); the DB side still refuses non-Managers.
- Nothing new is logged: the revoke warning prints only the channel error, never the jti or token.
- Grant windows: `proven_windows` (fe-database/src/role_grants.rs:171) untouched by this round's diff.
- SEC-INBOX 4: STILL-FIXED for the fe-database side (the entry names fe-policy/src/role_grants.rs, not an owned file; the local re-derivation at role_grants.rs:171 is intact).

## Completeness

- UI5-01 COMPLETE fe-ui/src/dialogs/entity_access.rs:187 (wrapper), :1386 (journey leg, both namespace states); caller fe-ui/src/panels/mod.rs:403.
- UI5-02 COMPLETE fe-ui/src/panels/inspector.rs:1534 (revoke send warns + visible error), :440 (Node header); fe-ui/src/panels/hierarchy_info.rs:157 (online of known), :192/:207 (scope-strict role and Settings).
- UI5-03 COMPLETE fe-renderer/src/terrain_overlay.rs:21 (ProposalLayerVisible), fe-terrain/src/proposal_layer_mirror.rs:23, fe-terrain/src/lib.rs:26, fe-terrain/src/terrain_plugin.rs:587/:623, fe-ui/src/ui_shell/right_sidebar.rs:157, fe-ui/src/panels/terrain_tools_panel.rs:276.
- UI5-04 COMPLETE fe-terrain/src/section_responder.rs:77 calls fe-terrain/src/volume_report.rs:86 `combined_height`; journey assertion at section_responder.rs:216.
- UI5-05 COMPLETE fe-database/src/role_grants.rs:484-498 (cap 1024, numeric order, LIMIT); fe-database/src/handlers/entity_access.rs:190; journey fe-database/tests/role_propagation_durable_test.rs:316.
- UI5-06 COMPLETE conductor/tracks/polish_lane_20261031/r2-closed-matrix.md:15-98 (82 ids, each with file:line), residuals :104.
- INT5-08 COMPLETE fe-ui/src/panels/hierarchy_info.rs:192 and :207 (role line and Settings gated on the card's own scope).
- INT5-09 COMPLETE fe-ui/src/panels/mod.rs:403 calls fe-ui/src/dialogs/entity_access.rs:187.
- EXT5-04 COMPLETE same caller (fe-ui/src/panels/mod.rs:403); its test leg is folded into fe-ui/src/dialogs/entity_access.rs:1386.

Verdict: PASS-WITH-FIXES (7 fixed in owned files across two passes, both hand-offs landed; nothing built or run yet).
