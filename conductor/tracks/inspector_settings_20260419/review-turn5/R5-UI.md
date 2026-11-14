---
type: Review
track: inspector_settings_20260419
slice: R5-UI
reviews: [UI5-01, UI5-02, UI5-03, UI5-04, UI5-05, UI5-06]
date: 2026-11-14
---

# R5-UI adversarial review (round 5, group UI)

Reviewed against `git diff a85dc9ad` for every upstream-owned file, prompted to refute. Security review included (UI5-05 bound).

## Findings

- F1 FIXED - fe-ui/src/panels/hierarchy_info.rs:207 - INT5-08 asks for the role line AND the Settings button to follow the card's own scope; UI5-02 scoped only the role line, so a petal-only Editor could open a verse's Settings from the verse card. Settings is now denied ("Open this verse to use its settings") when the card is not the deepest active scope (fail-closed); journey row at :354 goes red without it.
- F2 FIXED - fe-database/src/role_grants.rs:484 - `load_scope_statements` took any caller limit (the test asked for 100_000): no hard cap. Added `MAX_SCOPE_STATEMENTS = 1024` applied inside the function (:492), so no caller can bypass it; the journey now asks for `usize::MAX` and asserts the cap.
- F3 FIXED - fe-database/src/role_grants.rs:495 - SQL literal carried a run of stray spaces from a lost line continuation; restored as a `\` continuation (same query).
- F4 FIXED - fe-ui/src/panels/inspector.rs:1582 - revoke send-failure copy told the user to reopen the Inspector, which cannot help a closed DB channel; now names the cause and the action (restart, then revoke again).
- F5 FIXED - fe-terrain/src/volume_report.rs:85 - doc typo "Palace proposals" -> "Palette proposals".
- F6 FIXED - conductor/tracks/polish_lane_20261031/r2-closed-matrix.md:98 - DW-04 note duplicated the OUT reason; X-13 residual ended in a dangling "Owner:".
- F7 HANDED-OFF (I5-MANIFEST) - fe-terrain/src/lib.rs:23 - `#[cfg(feature = "render")] mod proposal_layer_mirror;` is missing while fe-terrain/src/terrain_plugin.rs:623 calls `crate::proposal_layer_mirror::...`; the render build fails until I5-MANIFEST lands it (already in its task).
- F8 HANDED-OFF (I5-UI) - fe-ui/src/panels/mod.rs:~402 - no caller of `mark_verse_local_only_from_tree` yet; until I5-UI adds the one line, the local-only reason never shows in the app (already in its task).
- F9 KEPT (task-authorized; the pure rule is the seam fe-ui can test without fe-terrain) - fe-ui/src/ui_shell/right_sidebar.rs:1980 - new table test of `show_heatmap_checkbox` is close to a restatement of `a && b`; the mirror rows are the part with mutation value.
- F10 KEPT (task allows the temp-data key; goes red if `.ok()` is restored) - fe-ui/src/panels/inspector.rs:4577 - the revoke leg asserts the temp key, not the painted label.
- F11 KEPT (an egui card, not a per-frame ECS hot path; anti-pattern 7 targets systems like gimbal interaction) - fe-ui/src/panels/hierarchy_info.rs:192 - the scope compare builds two scope Strings per frame while the card is visible.
- F12 KEPT (verified) - fe-ui/src/dialogs/entity_access.rs:187 - the pinned wrapper allocates nothing (iter/find/is_none on the borrowed tree); unknown verse => not local-only.
- F13 KEPT (verified) - fe-ui/src/panels/inspector.rs:1590 - no bare `.send(..).ok()` remains in inspector.rs; the token-list send now warns; the generate path keeps its visible error at :1083.
- F14 KEPT (display only; authz unchanged) - fe-database/src/handlers/entity_access.rs:190 - LIMIT 257 applies before the in-handler `retain(bounded_identity)`, so oversized-identity rows among the newest could shorten the page below 256; they never widen it.
- F15 KEPT (disclaimed in the file) - conductor/tracks/polish_lane_20261031/r2-closed-matrix.md:11 - anchor line numbers drift by a few lines after round-5 edits (spot-checked X-06, X-17, I-01, I-03, I-04, R-01: all within the cited region).

## Security review (UI5-05)

- Bound cannot be bypassed: the cap lives inside `load_scope_statements` (role_grants.rs:492); `limit` is a `usize` formatted into the SQL, `scope` stays a bound parameter (no injection).
- Authz on the read unchanged: the handler diff is the one call-site line (entity_access.rs:190); `load` still checks the caller's role before `grant_record`.
- Ordering: `string::len(hlc)` then `hlc` orders the decimal-string column numerically (no leading zeros); the handler sort by `u64` stays the display authority.
- Grant windows: `proven_windows` (fe-database/src/role_grants.rs:171) untouched by this round's diff.
- SEC-INBOX 4: STILL-FIXED for the fe-database side (entry names fe-policy/src/role_grants.rs, not an owned file; fe-database re-derivation at role_grants.rs:171 is intact).

## Completeness

- UI5-01 COMPLETE fe-ui/src/dialogs/entity_access.rs:187 (wrapper) and :1328 (journey leg, both namespace states); in-app visibility waits on I5-UI (F8).
- UI5-02 COMPLETE fe-ui/src/panels/inspector.rs:1533 (revoke send), :440 (Node header); fe-ui/src/panels/hierarchy_info.rs:157 (online of known), :192/:207 (scope-strict role and Settings).
- UI5-03 STILL OPEN - fe-terrain/src/lib.rs lacks the module declaration (F7, I5-MANIFEST) - fe-terrain/src/proposal_layer_mirror.rs:23, fe-ui/src/panels/terrain_tools_panel.rs:276, fe-ui/src/ui_shell/right_sidebar.rs:185.
- UI5-04 COMPLETE fe-terrain/src/section_responder.rs:77 calls fe-terrain/src/volume_report.rs:86 `combined_height`; journey assertion at section_responder.rs:216.
- UI5-05 COMPLETE fe-database/src/role_grants.rs:484-498 (cap 1024, numeric order, LIMIT); fe-database/src/handlers/entity_access.rs:190; journey fe-database/tests/role_propagation_durable_test.rs:316.
- UI5-06 COMPLETE conductor/tracks/polish_lane_20261031/r2-closed-matrix.md:15-98 (82 ids, each with file:line), residuals :104.
- INT5-08 COMPLETE fe-ui/src/panels/hierarchy_info.rs:192 and :207 (role line and Settings gated on the card's own scope, after F1).
- INT5-09 STILL OPEN - panel caller of the wrapper missing (I5-UI) - fe-ui/src/panels/mod.rs:~402.
- EXT5-04 STILL OPEN - same caller as INT5-09 (I5-UI); its test leg is folded into fe-ui/src/dialogs/entity_access.rs:1328.

Verdict: PASS-WITH-FIXES (5 fixed in owned files; module declaration and panel caller handed to I5-MANIFEST and I5-UI).
