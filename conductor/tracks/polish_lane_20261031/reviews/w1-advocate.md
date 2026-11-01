---
type: Review
title: W1 advocate review — feedback spine, petal-map truth, OF-13
track: polish_lane_20261031
wave: 1
role: advocate
reviewed_commit: 7ea181b4d78636362a80f1c848a3d103209a4fd3
created: 2026-10-31
---

# W1 advocate review

Lens (plan §1 ADVOCATE): reuse of the W1 spine over re-rolled local variants, idiomatic
Bevy, one helper per repeated shape, full-word naming, `ui_ux.md` §1/§5/§6/§9, and
folding every leaf DOC NOTE into the right directory `AGENTS.md`. Edits are applied
in the working tree, not committed (the RE-GATE inks them with the critic's).

## What the wave got right (kept as-is)

- **Priority queue over a single slot** (`fe-ui/src/actions/mod.rs` `active_notice`):
  `Severity`'s derived `Ord` doubles as display priority, so a later Info can never
  hide a live Error. Kept unchanged.
- **Optimistic update with a correlated snapshot (memento)** (`actions/hexon.rs`
  `set_petal_map`): the rollback snapshot and the optimistic write come from the same
  pre-send values, and a foreign correlation is ignored. Kept unchanged.
- **`confirm_button` / `role_gated`** are small and well named; their state lives in
  egui temp memory, which suits a widget with no reply arm. Kept unchanged.
- fe-sync `sharding.rs` (OF-13a/b/c): the names read well (`from_fractal`,
  `refused_contest_claims`) and the test helpers (`grant_at_fractals`,
  `revoke_at_fractal`, `p1_under`) are one helper per repeated shape. No advocate edits.

## Patterns applied (11)

1. **Single source of truth for role copy** — `notice::role_label(Option<RoleLevel>)`,
   `DenialContext::new(&LocalUserRole, verse_name)` and `notice::active_verse_name(&nav)`
   (`fe-ui/src/notice.rs`). The wave had three role labelers that disagreed:
   `query.rs` printed "Visitor" for an unknown role, `terrain.rs` printed lowercase
   "viewer" for an unknown role (the R2 X-03 bug in miniature), and
   `widgets::role_denial_reason` capitalized `Display` and printed "None". There
   were also two copies of the `active_verse_id.map(|_| active_verse_name)` shape.
   All of these now go through the helpers. An unresolved role now reads
   "Editor role needed[ in {verse}] to {verb}" instead of "You're a —".
   New test: `role_label_names_every_role_and_never_guesses_an_unresolved_one`,
   plus an unresolved row in `humanize_db_error_maps_denials_to_role_copy`.
2. **One in-flight dialog-request shape** — `JoinRequestState` became
   `actions::DialogRequestState { pending, error, sent_at }` with
   `start`/`finish`/`reset`/`waiting_within`. The struct name was used nowhere
   outside `actions/mod.rs`; the `join_request` field name in the contract is kept.
   The glTF Import dialog had its own variant of this shape: an egui-temp
   timestamp that no reply arm could clear. As a result the dialog
   **never closed on a successful import**: it stayed locked for 30 s with Cancel
   disabled and no close-X, and a reopen inside 30 s came up locked again. Import now
   uses `UiManager.gltf_import_request`. `db_results/mod.rs::finish_gltf_import_request`
   (called after `nodes::handle_gltf_imported`) closes the dialog only for its own
   request, and only while it is still the active dialog, then toasts
   "Imported {name}", the same as Join's "Joined {name}". This resolves the open
   gltf NEEDS-WIRING. Test: `gltf_import_reply_closes_only_the_dialog_that_asked`
   (table: dialog-sent vs foreign import).
3. **Extract Function so tests drive production code** — `open_portal` (P-01) and
   `open_path_in_options` (D-12) are now named functions that `process_ui_actions`
   calls. Before, the two W1-S1 tests exercised hand-copied "verbatim mirrors" of
   the match arms, so inverting the real arm could never turn them red. Both tests
   now call the real bodies. `open_portal_invalid_url_shows_error_notice` also
   asserts `Severity::Error`.
4. **Severity by meaning (ui_ux §6, OD-1 taxonomy)** — failures moved from
   `show_toast` (2.5 s, then gone) to `show_error`: "URL not saved — …",
   "Transform not applied — …", and the `RequestFailed` visibility-write notice in
   `db_results/mod.rs`. Guidance moved to `show_advisory`: SaveUrl "No node
   selected" (now matching OpenPortal's) and "Navigate to a petal first".
5. **`send_or_toast` reuse** — the `SaveUrl` arm's hand-rolled
   `send().is_err() → warn!` now goes through `send_or_toast(.., "save the URL")`.
   That call is the example in the helper's own doc comment.
6. **Composite role gating instead of a lookalike** (`dialogs/hexon_manager.rs`
   Remove): the denied branch had a hand-styled "Remove" button (white,
   non-small text) that did not match `confirm_button`'s look. It is now one
   `confirm_button` inside `ui.add_enabled_ui(can_edit)`, with
   `on_disabled_hover_text(role_denial_reason(..))` on the scope response. The
   idiom is documented in `panels/AGENTS.md` §widgets.
7. **No hot-path allocation** (`panels/status_bar.rs`): the error segment used to
   collect the whole 20-entry history into a `Vec<(u64, String)>` every frame.
   It now formats only the newest label and iterates the ring in place inside the
   popover.
8. **Dead code / speculative-parameter removal** — deleted
   `dialogs::create_entity::apply_create`. It had zero callers workspace-wide,
   and it was the last bare `.send(..).ok()` in a W1-owned file (AGENTS
   anti-pattern 6). Also dropped `render_create_dialog`'s unused
   `_hierarchy`/`_nav` params ("kept in case a future wave needs") and updated
   the `panels/mod.rs` call site and the `dialogs/mod.rs` re-export.
9. **Stale-marker hygiene** — rewrote leftover "NEEDS-WIRING" comments as plain
   descriptions where the gate had already done the wiring (`panels/mod.rs` ×2,
   `ui_shell/right_sidebar.rs`, `hexon_manager.rs`, `create_entity.rs`), and fixed two
   wrong doc pointers (`join.rs` → `dialogs/AGENTS.md` §join-flow; `JoinRequestState`'s
   pointer → `actions/AGENTS.md` §notices).
10. **DOC NOTE folds** —
    - W1-S3 → `dialogs/AGENTS.md` new §hexon-manager (card layout, Applying…,
      Remove gating, dead `pending_remove`).
    - W1-S2 → `panels/AGENTS.md` status_bar entry (conditions chips + error
      segment).
    - Advocate → new `dialogs/AGENTS.md` §dialog-requests, and §join-flow,
      §gltf-import-flow and §create-entity-flow rewritten.
    - `actions/AGENTS.md` §notices gained the severity rule, the one-role-word
      rule, `DialogRequestState`, and the extracted arm bodies.
    - `verse_manager/AGENTS.md` gained a `finish_gltf_import_request` line.
    - `panels/AGENTS.md` lost its stale "GATE must flip `mod widgets`" item.
    - The W1-S2 fe-runtime note was already folded by its owner.
    - The `portal.rs` doc-comment note stays recorded in `actions/AGENTS.md` for
      W4-S4, which owns that file.
11. **Vocabulary check (§9)** — no "tileset"/"track"/dev jargon in W1 user copy.
    "hexon_id or hexon_id@version" (registry download field) is a package-format
    context, which §9 permits.

## Handed to the W1 critic (not fixed — behavior beyond unifying)

- **Status-bar Dismiss does nothing visible** (`panels/status_bar.rs`):
  `dismiss_notice` removes the id from the notice queue, but the segment reads
  `error_history()`, which never drops entries. The "⚠ newest error" segment
  therefore stays for the whole session after Dismiss. Either give the history a
  "seen" mark or key the segment on the live queue.
- **Status-bar layout:** the error segment's `with_layout(right_to_left)` takes the
  rest of the row, so `modal.last_error` and `identity_banner` (drawn after it) are
  pushed out. Check in-app.
- **Plan §3 W1 vs shipped:** the plan says the raw message goes to "the history
  only". What ships is humanized text in the history ring and the raw text in the
  log. Recorded in `actions/AGENTS.md` §notices; the critic decides whether the
  popover should carry the raw string.
- **D-07 "toast on result"** is still open for Create (see `dialogs/AGENTS.md`
  §create-entity-flow): it needs a `DialogRequestState` for create plus a
  reply-arm finish.
- The GATE-DEFER fe-sync timing test still stands; the advocate did not touch fe-sync.

## Verification

- `cargo check -p fe-ui --tests`: clean.
- `cargo clippy -p fe-ui --all-targets -- -D warnings`: clean.
- `cargo test -p fe-ui`: 1050 passed, 0 failed (the gate had 1048; +2 new tests).

VERDICT: APPROVE-WITH-FIXES (11 applied)
