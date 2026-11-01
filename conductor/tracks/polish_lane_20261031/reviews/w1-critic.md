---
type: Review
title: W1 critic review — feedback spine, petal-map truth, OF-13
track: polish_lane_20261031
wave: 1
role: critic
reviewed_commit: 7ea181b4d78636362a80f1c848a3d103209a4fd3
reviewed_on_top_of: W1 advocate working-tree edits (reviews/w1-advocate.md)
created: 2026-10-31
---

# W1 critic review

Lens (plan §1 CRITIC): correctness defects and regressions, AGENTS "What NOT to
do", optimistic UI without rollback, UI-only authorization, over-engineering,
tests that cannot fail, the GATE-DEFER item, and the two plan-named checks: an
`Error` can never be starved by `Info`, and `humanize_db_error` never hides the
raw message from the history. All fixes are in the working tree, uncommitted;
the RE-GATE inks them with the advocate's.

## Findings fixed in-wave (8)

1. **HIGH — notices stamped and rendered on two different clocks.** bevy_egui
   feeds `RawInput::time` from `Time<Real>` (`bevy_egui-0.39.1/src/input.rs:1113,1184`),
   and `panels/mod.rs::render_toast` reads `ctx.input(|i| i.time)`. Every Bevy
   system stamped notices with `Res<Time>`, which is `Time<Virtual>`. That clock
   clamps each frame to 250 ms, so it falls behind after every hitch (startup,
   a large import, a window drag on Windows). Notices from `process_ui_actions`,
   `apply_db_results`, the asset/gpx/path status systems, brush, pointer, sidebar
   and the mesh watchdog therefore looked older than they were. With more than
   7 s of lag, every Error expired on arrival — the W1 feedback spine's main
   path (`db_results` → `show_error`) went silent. The comment at
   `actions/mod.rs` claiming "the same Bevy clock" was wrong.
   Fix: `notice::NoticeClock = Time<Real>`. All nine notice-stamping systems
   take `Res<NoticeClock>`. Test: `node_manager/sidebar_sync.rs`
   `notice_is_stamped_on_the_render_clock_not_virtual_time` (real clock at 10 s,
   virtual at 0; reverting goes red).
2. **HIGH — a superseded petal-map set rolled back to an unpersisted map.**
   `actions/hexon.rs::set_petal_map`/`set_petal_map_scale` snapshotted the
   current `petal_map` as the rollback target. While an earlier set was still
   pending, that state was the earlier set's *optimistic* value. Two quick Set
   clicks, or a Viewer dragging the scale slider, ended with every reply denied
   and the UI showing the first map as applied: the M-03 lie this wave exists to
   close. Fix: `rollback_baseline` carries the pending snapshot forward, and any
   non-matching `PetalTerrainLoaded` for the active petal becomes the new
   baseline (it is persisted truth). Test:
   `superseded_set_denial_rolls_back_to_last_confirmed_map` (first denied →
   "original"; first confirmed → "first").
3. **HIGH — the status-bar error segment hid the guard error and identity
   banner.** The advocate flagged this layout issue. The segment's right-to-left
   `with_layout` takes the rest of the row. `modal.last_error` (FR-7) and the F6
   identity-degradation banner were drawn after it, so both were pushed
   off-screen once any error had happened. Fix: draw the segment last.
4. **MED — Dismiss did nothing.** The advocate flagged this. Dismiss removed the
   queue entry, but the segment read `error_history()`, which keeps every entry.
   Fix: `dismiss_notice` on an Error records it as acknowledged, and the segment
   reads `latest_unacknowledged_error()`. The plan's 60 s bound also ships now
   (`ERROR_SEGMENT_SECONDS`): the claim that "status_bar has no clock" was wrong,
   because `ui.ctx().input(|i| i.time)` is the same value as `NoticeClock` after
   fix 1. Test: `dismissed_error_clears_the_status_segment_until_the_next_one`.
5. **MED — the raw DB error was lost from the history** (plan §3 W1 step 1 and
   the plan's explicit critic check; the advocate flagged it). The ring stored
   only the humanized text, and an unknown error was cut to 80 characters. Fix:
   `Notice.detail` holds the raw string. New `UiManager::show_db_error(raw,
   &ctx, now)` is used by `query.rs::handle_error`, `terrain.rs`'s set-failed
   arm and `hierarchy.rs`'s foreign `JoinVerseFailed`. The history popover shows
   the raw text under each line, and Copy copies it. Covered by the
   detail-asserting tests in fixes 4 and 7.
6. **MED — Info could evict a live Error.** With four live Errors queued, an
   Info push removed the oldest Error. The queue also never pruned expired
   notices. Fix: prune expired notices on push, then never evict a notice that
   outranks the incoming one; the Info is dropped instead. Test:
   `notice_queue_never_evicts_a_live_error_for_an_info` (table: during vs after
   the errors' lifetime).
7. **MED — a failed glTF import locked the dialog for 30 s** with Cancel
   disabled and no close-X, because the failure (`DbResult::Error("GLTF import
   failed: …")`) could not finish the request. Fix: `query.rs::handle_error`
   recognizes that prefix and finishes the dialog's own request with the reason
   inline, the same as Join. The 30 s lock is now only a no-reply safety net.
   Test: `gltf_import_failure_unlocks_only_the_dialog_that_asked`. The
   `finish_gltf_import_request` test gained the "another dialog is active" row,
   which covers its `matches!` guard (it could not fail before).
8. **MED — Join could lock forever.** Every join replies typed, but if the DB
   thread died mid-request, Cancel and the close-X never came back. Fix: the
   same `waiting_within` bound Import uses (`JOIN_LOCK_TIMEOUT_SECONDS` = 30 s).
   Test: `join_dialog_unlocks_when_no_reply_arrives`.

Also strengthened: `denied_petal_map_set_rolls_back_and_explains` only checked
that the history was non-empty. It now asserts the exact role copy and the raw
detail.

## GATE-DEFER resolved

`fe-sync::distributed_query::tests::a_full_event_channel_drops_the_echo_instead_of_stalling_later_queries`
was a test race, not a W1 regression. The runtime's `block_on` ended a fixed
200 ms after the second submit, and dropping the runtime cancels the second
collector if a loaded machine has not polled it yet. That matches the "2/5 under
load" asymmetry. The test now keeps the runtime alive until the main thread
drops a `done` channel after collecting both answers. That exposed a second
race (1/8 failures: `queued.len() == 2`): draining the full event channel while
collectors were still live made room for a late echo. On success the test now
stops the runtime before draining. On the regression path (second query timed
out) it drains first, which is what unparks the blocked echo. The regression it
guards still fails it. After the fix the test passed 15/15 back to back on this
loaded machine (about 400 MB available RAM, more than 1000 processes).

## Checked and kept

- **Over-engineering:** `open_portal`/`open_path_in_options` have one caller
  each, but each exists so the test drives production code. Keep.
  `DenialContext::new`, `active_verse_name` and `role_label` have two or more
  callers. Keep.
- **UI-only authorization:** the Maps Set/Clear/Remove gating is advisory. The
  `SetPetalTerrain` write is policy-checked DB-side, and the denial path is
  exactly what fix 2 hardens. No UI-only authority found.
- **AGENTS anti-patterns:** no bare `.send(..).ok()`, no `despawn_recursive`,
  and no direct NavigationManager field writes in the wave diff (two hits are in
  test fixtures). The status-bar hot path no longer copies the ring.
- **`role_label_names_every_role…`** restates a match, but each row can fail,
  and the None-vs-`RoleLevel::None` distinction it pins is the X-03 bug. Keep.
- `hexon_manager` Remove's `add_enabled_ui(..).response.on_disabled_hover_text`:
  the scope response carries the child Ui's `enabled = false`
  (`egui-0.33.3/src/ui.rs:1194-1210`), so the hover reason does show.

## Carried to W2 advocate

- (LOW) `actions/hexon.rs` send failures only `warn!`. The functions take no
  `UiManager`, so they cannot use `send_or_toast` (X-17 spirit).
- (LOW) A superseded GUI petal-map set that is denied while a newer one is
  pending raises no notice. Fix 2 makes the state correct, but the earlier
  denial is silent. If the newer set then succeeds, the user never learns the
  first one failed.
- (LOW) "Applying…" has no timeout: a lost `SetPetalTerrain` reply leaves the
  card pending until the next confirmed echo.
- (LOW) Opening another dialog over an in-flight Join/Import does not `reset`
  its `DialogRequestState`. A reopen within 30 s comes up locked until the reply
  lands or the bound passes.
- (LOW) The glTF failure is matched by message prefix (`GLTF_IMPORT_FAILED_PREFIX`
  in `query.rs`, mirroring `fe-database/src/lib.rs`'s `ImportGltf` arm). A typed
  `DbResult::GltfImportFailed`, like `JoinVerseFailed`, would remove the string
  coupling.
- (from advocate) D-07 "toast on result" for Create is still open.

## Verification

See the RE-GATE handoff below. Commands were run serially as the only cargo user.

- `cargo check -p fe-ui --tests`: clean.
- `cargo clippy -p fe-ui -p fe-sync --all-targets -- -D warnings`: clean. fe-sync
  was re-run clean after the second test fix.
- `cargo test -p fe-ui`: 1056 passed, 0 failed (advocate 1050 + 6 new critic
  tests).
- `cargo test -p fe-sync`: 295/295 lib + all integration binaries green. The
  GATE-DEFER test then passed 15/15 in isolation after the final fix.
- Not run (for the RE-GATE): the workspace-wide check/clippy. Crates touched:
  fe-ui (src + tests) and fe-sync (one test only).

VERDICT: PASS-WITH-FIXES (8)
