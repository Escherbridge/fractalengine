---
type: Review
track: rich_data_history_20261101
gate: R-DATA-1
upstream: [DATA-01, DATA-02, DATA-03, DATA-04, DATA-05]
security: true
---

# R-DATA-1 adversarial review (round 4)

Scope: `git diff 7af0a144` over the DATA owned files plus the integration call sites (inspector.rs, dialogs/mod.rs, lib.rs). No cargo run. Security inbox: no entry names a file in this gate's owns.

D1 FIXED - fe-ui/src/gis/egress_strings.rs:74 - SECURITY (low): `escape_sql_str` doubled `'`, which SurrealQL does not treat as an escape, and left `\` raw, so an id such as `x\' OR true --` from a replicated petal closed the literal in the copied/pasted read. Now backslash-escapes `\` and `'` exactly like `query_tab::sql_literal`; the two escape assertions updated.
D2 FIXED - fe-ui/src/panels/property_value.rs:93 - geometry_polygon ring closure compared `serde_json::Value`s, so `[0,0]` ... `[0.0,0.0]` (integer vs float Number) was refused as unclosed; closure now compares positions numerically.
D3 FIXED - fe-ui/src/panels/tree_appearance.rs:246 - `interact_size.y` was assigned twice; the "This entry no longer exists" reason sat at the bottom of the scroll body (could be scrolled out of view at 1024x640 x2 while Save is disabled) and a second separator was drawn; the reason now renders beside the fixed action row and the footer budget reserves its line.
D4 HANDED-OFF (I-DB, fe-database/src/lib.rs) - lib.rs:2137 and lib.rs:1070 - after a FullTransform / UpdateNodeTransform on a stamp the DB now writes `stamp.override.scale/rotation`, but the scene-change fan-out sends only `NodeTransform`; the GUI `StampInteractionState` and VerseManager property bag keep the old override until reload, so a later petal respawn or stamp re-materialise can revert the API rotate/scale. Exact change: on Ok, when the committed state has `properties.node_kind == "stamp"`, also send `SceneChange::PropertyChanged` for both keys from `receipt.state.properties` (same loop as the CommitStampOverrides arm at lib.rs:2025).
D5 KEPT (fail-closed by design) - fe-database/src/handlers/stamp_transform.rs:35 - a stamp transform with a scale outside 0.01..=10000 is now refused as a whole, where the row columns used to accept it; matches `stamp_overrides::prepare` and the steer's global scale range.
D6 KEPT (verified) - fe-database/src/handlers/stamp_transform.rs:15 - `EulerRot::XYZ` matches node_history.rs:398 and editor_batch/prepare.rs; the override REPLACES the tangent rotation (path_asset_materialize.rs:382), so mirroring the absolute row rotation is correct; scope authorised before the mirror, all values bound, key names from constants, rule guard re-checked inside the same transaction.
D7 KEPT (verified) - fe-ui/src/editor_batch/ui.rs:519 - the automatic read already exists in `EditorBatchState::maintain` (mod.rs: `capture/read/pending/error` all empty -> one `ReadEditorBatch`); DATA-02 correctly added only the visible "Reading selected objects" reason; the inspector.rs journey asserts exactly one read and enable-on-reply.
D8 KEPT (minor) - fe-ui/src/editor_batch/ui.rs:519 - when a capture exists but a registry write is pending, the buttons are disabled with no reason line; out of QA-0111 scope, trigger: next Groups UX pass.
D9 KEPT (verified) - fe-ui/src/gis/egress_strings.rs:379 - the paste round-trip leg uses only verified seams (`spawn_db_thread_shared`, `ScopedQuery`, `TombstoneNode{auth: Local}` -> `NodeDeleted`), joins the DB thread before the tempdir drops, no timing-dependent assertion.
D10 HANDED-OFF (I-BATCH) - conductor/test-batches.md - `handlers::stamp_transform::tests` (fe-database lib) and `dialogs::tests::active_dialog_starts_empty_and_tree_appearance_actions_stay_visible` (fe-ui lib) are reached by no batch filter; add `handlers::stamp_transform` to the R1 fe-database lib command (line 58) and `dialogs::tests::` to the R6 fe-ui command (line 211).
D11 HANDED-OFF (I-DOC-CORE) - fe-database/src/handlers/AGENTS.md - stamp_transform.rs:1 points at a `§stamp-transform` section that does not exist.
D12 HANDED-OFF (I-DOC-UI) - fe-ui/src/panels/AGENTS.md:695 and fe-ui/src/gis/AGENTS.md - one line each: tree appearance actions live outside the body scroll (QA-0112); every copied read hides tombstones and escapes literals like the Query tab.
D13 KEPT (verified) - DATA-01 Viewer gating is wired (inspector.rs:388, 2204) and typed invalid inputs are covered in the existing typed-property journey (inspector.rs:2871); DB `require_write_role` stays authoritative.
D14 KEPT (verified) - leak discipline: no forbidden token in any added line of the DATA diffs or these fixes; no clippy, no cargo.

Verdict: PASS-WITH-FIXES (3 fixed, 4 handed off, none blocking).
