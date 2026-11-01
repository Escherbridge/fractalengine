---
type: Track Plan
title: Polish lane — wave plan (6 waves, disjoint partitions, advocate + critic per wave)
track: polish_lane_20261031
status: pending
created: 2026-10-31
timestamp: 2026-10-31T00:00:00Z
computed_at_commit: 6531d19
tags: [plan, slices, waves, advocate, critic]
---

# Polish lane — execution plan

Spec: [spec.md](./spec.md). Machine truth: [metadata.json](./metadata.json) (`partitions`).
Finding ids: **R1** = `.omc/research/polish-stubs-20261031.md`, and the bare ids (X-01, M-03, …) come
from **R2** = `.omc/research/polish-ux-20261031.md`.

This plan is executed by workflow automation. Each wave is the same seven-step loop:

```
PREFLIGHT (orchestrator, no agent) → LEAVES ∥ (3-5, disjoint files, no cargo, no git)
  → GATE (sole cargo user; inks + commits) → ADVOCATE (review-and-fix) → CRITIC (review-and-fix)
  → RE-GATE (inks review fixes + commits) → [VISUAL, W3/W6 only, optional]
```

---

## §0 Open decisions (proceed on defaults; the user said to go on best guesses)

| ID | Decision | Default (applied) | Reversal cost |
|---|---|---|---|
| OD-1 | Notice durations and queue | Info 2.5 s, Advisory 4 s, Error 7 s. Errors hold while hovered and have a ✕. Queue cap 4 (the oldest Info is dropped first, never an Error). Error history ring of 20. | Trivial (constants in `fe-ui/src/notice.rs`). |
| OD-2 | Correlation ids for GUI sends | Mint ids (`actions::query::next_ui_query_id`) only where a typed failure reply already exists or is added here (petal-map, Join). Every other `DbResult::Error` goes through the humanized stop-gap. A full migration of all 62 emitters is out of scope. | Medium (a later track per emitter). |
| OD-3 | Role-gating style | Disable the control and add a hover reason ("Editor role needed — you are Viewer in <verse>"). Never hide it, so it stays discoverable. | Low (one helper). |
| OD-4 | Reset Database home | Settings → "Danger zone" section. Visible only in debug builds or with `FE_DEV_TOOLS=1`. Requires typing `RESET`, and the result is toasted. | Low. |
| OD-5 | Claim Ownership reach (D-01) | The gear is always shown on verse, fractal and petal cards. Tabs are gated inside the dialog, and the Claim tab is visible to non-owners. | Low. |
| OD-6 | Undo scope | Command pattern over transform commits and path-point moves only. Session-local stack of 50, cleared on petal switch. The inverse is sent through the same broadcast path, so it replicates like any edit. An entry for a node a peer has since changed is skipped with an advisory. | Medium. |
| OD-7 | F7 shape | New `DbCommand::ScopedQuery { sql, vars, petal_scope, correlation_id }` for user-authored GUI SQL. The pure scoped-FROM rewrite moves from `fe-api/src/query_guard.rs` into `fe-query::sql_guard`, and fe-api re-exports it so no fe-api path changes. `RawQuery` is documented as server-built SQL only. | Medium (variant plus module move). |
| OD-8 | Tauri command stubs (unreachable; D-42 NO-GO) | Delete the six stub commands. Keep `resolve_asset` and `get_asset_base_url`. `StubBackend` logs one loud warn at creation. | Low (git revert). |
| OD-9 | fe-plugin read path | Injected `NodeReader` and `TransactionSink` traits in fe-plugin, with no direct `fe-entity-store` dependency. `fe-plugin-test::MockHostEnv` implements both. | Medium. |
| OD-10 | fe-sync peer blob fetch (Phase F) | OUT of this lane. Route to p2p_asset_streaming_20260718. | Adds a W7 if the user overrides. |
| OD-11 | Snap defaults | Grid snap off by default with a 1 m step. Angle snap 15° when on. Snap increment applies to Scale. No modifier key (X-08 forbids Ctrl-tool collisions). | Low. |
| OD-12 | Render-distance single home | The Settings "This petal (shared)" column. Remove the field from the Petal Manifest dialog UI only; the data field stays. Show meters. | Low. |
| OD-13 | Relay "applies after restart" | A "Restart now" button that spawns `std::env::current_exe()` and then sends `AppExit`. | Low. |
| OD-14 | Placeholders with no planned build ("Pivot (soon)", "Selection filters (soon)", "Highlight style (soon)") | Remove the hints. Do not build them. | Low. |
| OD-15 | Polygon draft gesture (O-01) | With Brush + Polygon, a click appends a point. Enter, double-click or "Close polygon" finishes. Backspace removes the last point and Esc cancels. Markers and a live area readout are shown. | Low. |
| OD-16 | InviteDialog (no renderer; M-02) | Remove the variant and its opener (`db_results/roles.rs:10`). Invites are generated inside Entity Settings. | Low. |
| OD-17 | Visual evidence | When Lane B's screenshot surface is present, the W3 and W6 re-gates capture R2's "verify visually" list to `.omc/research/polish-evidence/wN/` (not committed). It never blocks a wave and never replaces the user's in-app look. | Nil. |

---

## §1 Execution protocol (mechanical, applies to every wave)

**PREFLIGHT** (orchestrator, before spawning leaves):
1. `git rev-parse HEAD`. If it differs from the last re-gate commit (or `computed_at_commit` for W1),
   re-grep every `owns` path of the wave and confirm it exists and appears in exactly one leaf. A
   missing or duplicated path voids that wave's partition: recompute it, set `confidence: "none"` in
   metadata for that wave, then proceed.
2. **Lane B freeze (W1, W2):** none of the wave's owned files may appear in Lane B's frozen set (§2) or
   in `git status --short` as modified by another actor.
3. **W3 precondition:** Lane B's commit(s) are in `git log`, and every §2 path is clean in
   `git status`. If Lane B has not landed when the W2 re-gate finishes, run **W5** next. W5 is
   L_B-free and has no W3/W4 contract dependency. Then run W3 and W4. Do not promote W4: it consumes
   W3's `theme::tab_fill` and notice renderer. When reordering, re-point metadata `depends_on`
   (W5 leaves → `W2-REGATE`, W3 leaves → `W5-REGATE`, W6 leaves → `W4-REGATE`) so the DAG stays the
   truth. Waves never run concurrently, because they share central files and the single cargo lock.
4. If `.git/index.lock` exists, wait 10 s and retry. Never delete it.
5. Before relaunching any workflow into this tree, TaskStop the prior workflow id (2026-10-11
   double-launch incident).

**LEAF rules** (every leaf slice):
- Edit only files in your `owns` list. New files are marked `(future)`.
- **Never run cargo, never run git write commands, never commit.** The gate inks. Concurrent builds
  deadlock on the shared lock.
- Re-grep each R2/R1 anchor before editing. Line numbers are hypotheses.
- **Contracts:** each wave's §-section pre-declares the cross-leaf API (names and signatures). Code
  against them exactly. If a contract proves wrong, implement the closest compatible shape and report
  `CONTRACT-DRIFT: <what>` so the gate reconciles it.
- **NEEDS-WIRING:** if a fix needs a value threaded through a file you don't own, write the fix behind
  a parameter or field, and report `NEEDS-WIRING: <file> → <param>`. The named later owner threads it
  (W3-S1 is the standing receiver for `panels/mod.rs` and dialog call sites).
- **Docs:** only the slice that owns a directory `AGENTS.md` edits it. Other slices end their report
  with `DOC NOTE: <dir> §<section>: <one paragraph>`, and the wave's ADVOCATE folds those notes in.
  Code carries one-line doc comments plus a pointer (`see fe-ui/src/AGENTS.md §<x>`).
- **Tests model real flows** (skill `flow-tests`): drive the highest fast public seam with real
  collaborators (a headless `App` with `MinimalPlugins`, the real `DbCommand` channel, or a real
  SurrealKV store in `fe-database/tests`). Assert observable outcomes (state, emitted `DbCommand`, DB
  rows, notices), never mock call counts. Collapse validation variants into one table-driven test.
  Every test must go red if its covered line is inverted. Name the tests exactly as listed (rename only
  with a `CONTRACT-DRIFT` note).
- Report: files changed; the findings closed (ids); tests written (names); and any CONTRACT-DRIFT,
  NEEDS-WIRING or DOC NOTE lines.

**GATE** (sonnet, one agent, the sole cargo user):
```
$env:CARGO_PROFILE_DEV_DEBUG = "0"           # PDB/disk pressure (prod-ready doctrine)
# If surrealdb-core recompiles: $env:RUST_MIN_STACK = "67108864"; add -j4
# Check free RAM first (LM Studio OOM blocker): phantom E0463/E0282 in untouched files = environment, not code
cargo check --workspace --tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p <every crate the wave touched>    # fe-terrain ALWAYS with --features render
```
- Reconcile CONTRACT-DRIFT and compile errors with the **minimal** edit in the wave's files. The gate
  adds no features.
- If a test fails because of a leaf defect, the gate fixes it when the fix is ≤ 20 lines in one file.
  Otherwise it records `GATE-DEFER: <test> → CRITIC` and the critic owns it.
- Commit only the wave's owned paths plus gate fixes:
  `feat(polish): wave N — <wave title>`, ending with the trailer line
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.

**ADVOCATE** (opus, fresh context, review-and-fix; it argues *for* patterns):
- Lens: reuse of the W1 spine (notice API, `role_gated`, `confirm_button`, `send_or_toast`, and the
  `clipped_value` family) instead of re-rolled local variants; idiomatic Bevy (plugins own their
  registration, `.chain()` ordering, `Local<T>`, no hot-path allocation); one helper per repeated shape;
  naming (full words, no `padH`-style abbreviations); conformance with
  `conductor/code_styleguides/ui_ux.md` (§1 hue only for abnormal states, §5 two-step destructive, §6 no
  vanishing failure toast, §9 vocabulary); folding the wave's DOC NOTEs into directory AGENTS.md files.
- **Applies** its improvements directly in the wave's files. It may run `cargo test -p <crate>` for
  crates it touched; it runs serially, so it is the sole cargo user at that moment.
- Writes `reviews/wN-advocate.md` (future) with frontmatter `type: Review` and a one-line verdict:
  `VERDICT: APPROVE | APPROVE-WITH-FIXES (n applied) | REWORK (<what>)`.

**CRITIC** (opus, fresh context after the advocate, review-and-fix; it argues *against*):
- Lens: correctness defects and regressions; the AGENTS.md "What NOT to do" list (bare `.ok()` on
  sends, direct manager-field writes, `despawn_recursive` on nodes, hot-path allocation); optimistic UI
  without rollback; role checks that only exist in UI (authorization must stay server-side);
  over-engineering (an abstraction with one caller, config nobody sets); **tests that cannot fail** (do
  the mutation check on every new test: invert the covered line mentally or for real, and delete or
  strengthen the test if it stays green); and any GATE-DEFER items. On W3 and W5 it also runs
  `/security-review` on the diff (role management, scoped SQL).
- **Fixes** its own findings. STOP-RULE: MED/LOW findings outside the wave's scope go into
  `reviews/wN-critic.md` under "Carried to W(N+1) advocate" instead of growing the wave. HIGH findings
  are always fixed in-wave.
- Writes `reviews/wN-critic.md` (future): `VERDICT: PASS | PASS-WITH-FIXES (n) | FAIL (<blocking>)`. A
  FAIL blocks the next wave until the re-gate is green on a fix.

**RE-GATE** (same agent shape as GATE): same commands. Commit
`fix(polish): wave N review — advocate + critic` with the same trailer. Then update metadata
`partitions.verified_waves` and tick this plan's boxes.

---

## §2 Lane B frozen set (do not touch until Lane B's commit lands)

From Lane B's working diff at 6531d19 (this is wider than the brief):
`fractalengine/src/main.rs`, `fractalengine/build.rs`, `fractalengine/Cargo.toml`,
`fractalengine/assets/branding/**`, `fractalengine/src/window_chrome.rs`,
`fractalengine/src/screenshot_bridge.rs`, `fe-ui/src/ui_shell/topbar.rs`, `fe-ui/src/panels/mod.rs`,
`fe-ui/src/plugin.rs`, `fe-ui/src/theme.rs`, `fe-runtime/src/app.rs`, `fe-runtime/src/lib.rs`,
`fe-runtime/src/screenshot.rs`, `fe-api/src/{assets,lib,rest,server,screenshot}.rs`,
`fe-api/src/mcp/{mod,tools}.rs`, `fe-api/src/canonical_ws/router.rs`, `fe-api/tests/**`,
`fe-test-harness/src/api.rs`, `fe-test-harness/src/scenarios/api_db_sync_cross_thread.rs`.

W1, W2, W4 and W5 own none of these. W4 still runs after W3 because of the contract dependency. W3 owns `panels/mod.rs`, `plugin.rs`, `theme.rs` and
`topbar.rs`, and requires Lane B landed. W5-S1 edits `fe-api/src/query_guard.rs`, which is not frozen,
and keeps every `query_guard::` path that Lane B's `rest.rs`/`screenshot.rs` import valid through
re-exports.

---

## §3 Waves

### Wave 1 — Feedback spine, petal-map truth, OF-13 (S1 findings first; L_B-free)

**Contracts (pre-declared; W1-S1 provides, everyone codes against them):**
```rust
// fe-ui/src/notice.rs (future, W1-S1)
pub enum Severity { Info, Advisory, Error }
pub struct Notice { pub id: u64, pub severity: Severity, pub text: String, pub spawned_at: f64 }
pub struct DenialContext<'a> { pub role_label: &'a str, pub verse_name: Option<&'a str> }
pub fn humanize_db_error(raw: &str, ctx: &DenialContext) -> String;
// fe-ui/src/actions/mod.rs — UiManager (W1-S1)
fn show_toast(&mut self, msg, now)                 // unchanged signature = Info
fn show_advisory(&mut self, msg, now); fn show_error(&mut self, msg, now)
fn active_toast(&self, now) -> Option<(&str, f32)> // unchanged signature (highest-priority live notice)
fn active_notice(&self, now) -> Option<(&Notice, f32)>
fn hold_notice(&mut self, id: u64, now: f64); fn dismiss_notice(&mut self, id: u64)
fn error_history(&self) -> impl Iterator<Item = &Notice>   // newest first, cap 20
fn set_condition(&mut self, key: &'static str, text: Option<String>)  // persistent status chips
fn conditions(&self) -> impl Iterator<Item = (&'static str, &str)>
pub join_request: JoinRequestState   // { pub pending: bool, pub error: Option<String> }
pub(crate) fn send_or_toast(tx: &crossbeam::channel::Sender<DbCommand>, cmd: DbCommand,
                            ui_mgr: &mut UiManager, now: f64, what: &str) -> bool;
// fe-ui/src/panels/widgets.rs (W1-S4)
pub(crate) fn role_gated(ui: &mut egui::Ui, allowed: bool, reason: &str, widget: impl egui::Widget) -> egui::Response;
pub(crate) fn role_denial_reason(role: &crate::plugin::LocalUserRole, needed: &str, verse_name: Option<&str>) -> String;
pub(crate) fn confirm_button(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, label: &str, confirm_label: &str) -> bool;
// fe-runtime/src/messages.rs (W1-S2)
DbResult::JoinVerseFailed { error: String }
// fe-ui/src/verse_manager/db_results/terrain.rs (W1-S3); W1-S2's mod.rs dispatch calls these
pub(super) fn handle_petal_terrain_loaded(petal_id, terrain, nav, petal_map, proposal_state, correlation_id: Option<&str>)
pub(super) fn handle_petal_terrain_set_failed(petal_id: &str, error: &str, correlation_id: &str,
    petal_map: &mut PetalMapState, ui_mgr: &mut UiManager, local_role: &LocalUserRole, verse_name: Option<&str>, now: f64)
```

| Slice | Title | Model | Owns |
|---|---|---|---|
| W1-S1 | Notice model + action-side honesty | sonnet | `fe-ui/src/notice.rs` (future), `fe-ui/src/actions/mod.rs`, `fe-ui/src/lib.rs`, `fe-ui/src/actions/AGENTS.md` |
| W1-S2 | DB-result routing, typed Join failure, status-bar error history | sonnet | `fe-ui/src/verse_manager/db_results/mod.rs`, `…/db_results/query.rs`, `…/db_results/hierarchy.rs`, `fe-ui/src/panels/status_bar.rs`, `fe-ui/src/verse_manager/AGENTS.md`, `fe-runtime/src/messages.rs`, `fe-database/src/lib.rs`, `fe-runtime/src/AGENTS.md` |
| W1-S3 | Petal-map truth (M-03) + Maps layout | sonnet | `fe-ui/src/actions/hexon.rs`, `fe-ui/src/terrain_map/mod.rs`, `fe-ui/src/verse_manager/db_results/terrain.rs`, `fe-ui/src/dialogs/hexon_manager.rs`, `fe-ui/src/terrain_map/AGENTS.md` |
| W1-S4 | Shared widgets + Join/Import/Create dialog honesty | sonnet | `fe-ui/src/panels/widgets.rs`, `fe-ui/src/dialogs/join.rs`, `fe-ui/src/dialogs/gltf_import.rs`, `fe-ui/src/dialogs/create_entity.rs`, `fe-ui/src/dialogs/AGENTS.md`, `fe-ui/src/panels/AGENTS.md` |
| W1-S5 | OF-13a/b/c petal-map hardening (fe-sync) | opus | `fe-sync/src/sharding.rs`, `fe-sync/src/AGENTS.md` |

Disjointness: the five owns lists share no path (verified at 6531d19). No path is in §2. `db_results/`
is split by file: S2 owns mod/query/hierarchy and S3 owns terrain. `dialogs/` is split by file: S3 owns
hexon_manager, and S4 owns join/gltf_import/create_entity/AGENTS.md. `dialogs/mod.rs` is
deliberately unowned in W1 (`JoinDialog` keeps its shape, and its pending/error state lives on
`UiManager.join_request`).

#### W1-S1 · Notice model + action-side honesty · sonnet
- **Closes:** X-04 (model half), X-01 (plain-language copy), X-05 (condition API half), X-17 (helper),
  P-01 (toast half), D-12 (handler half), and R1 `actions/mod.rs:296` (verify each per-track leaf
  handler body is filled; report any empty one).
- **Steps:**
  1. Create `notice.rs` with the contract types plus `humanize_db_error`. Map `permission denied: …` to
     "You're a {role} in {verse} — Editor role needed to {verb}". Derive the verb from the failed
     command prefix ("Set petal terrain failed" → "change the map", "Join verse failed" → "join this
     verse", …). Unknown errors show "Something went wrong: {short}" and keep the raw message in the
     history only.
  2. Replace `UiManager.toast: Option<(String, f64)>` (`actions/mod.rs:486`) with a `VecDeque<Notice>`
     queue, an error ring and a conditions map. Keep `show_toast` and `active_toast` signatures so
     `panels/mod.rs::render_toast` (frozen) still compiles and shows the head notice.
  3. Add `send_or_toast`. Replace bare `.send(..).ok()` inside `actions/mod.rs` with it.
  4. P-01: `OpenPortalOutcome::InvalidUrl` → `show_error("Not a valid URL — include https://")`.
     `NoSelection` → advisory (`actions/mod.rs:672-681`).
  5. D-12: the Edit-Path handler (`actions/mod.rs:853-855`) activates Pen, reveals the Options
     section, and shows "Path open in Options". The stale toast strings in `context_menu.rs` are W4-S4's.
  6. Add `pub mod notice;` in `lib.rs`.
- **Acceptance:** an Info that arrives after an Error never hides the Error before 7 s. The error
  history keeps 20 entries, newest first. No `.ok()` on a DB send remains in `actions/mod.rs`.
- **Flow tests:** `notice_queue_error_survives_later_info` (table-driven over severity orders and
  timings); `humanize_db_error_maps_denials_to_role_copy` (table: raw → copy, including unknown);
  `open_portal_invalid_url_shows_error_notice` (push `UiAction`, run `process_ui_actions` in a headless
  `App`, assert the notice); `edit_path_action_activates_pen_and_reveals_options`.

#### W1-S2 · DB-result routing, typed Join failure, status-bar error history · sonnet
- **Closes:** X-01 (routing), D-05 (result half), M-03 (dispatch half), and the X-01 status-bar segment.
- **Steps:**
  1. `fe-runtime/src/messages.rs`: add `DbResult::JoinVerseFailed { error }` at the end of the enum.
     In `fe-database/src/lib.rs:805`, emit it instead of `DbResult::Error("Join verse failed: …")`.
     Grep for exhaustive `DbResult` matches (fe-api reply routing, fe-test-harness) and add arms where
     a match is exhaustive. Report any in §2 files as NEEDS-WIRING.
  2. `query.rs::handle_error`: keep the log, and also call
     `ui_mgr.show_error(humanize_db_error(msg, &ctx))` with `ctx` built from `LocalUserRole` and the
     active verse name. Thread `ui_mgr`, `local_role`, `verse_mgr` and `nav` from the
     `apply_db_results` params (all of them are already system params).
  3. `hierarchy.rs::handle_verse_joined`: if `ui_mgr.join_request.pending`, close the dialog, call
     `nav.navigate_to_verse(id, name)` and toast "Joined {name}". Always reload the hierarchy. On a
     new `JoinVerseFailed` arm: if pending, set `join_request.error = Some(humanized)` and clear
     pending; otherwise call `show_error`.
  4. `mod.rs`: pass `correlation_id.as_deref()` to `terrain::handle_petal_terrain_loaded`, and add a
     `DbResult::PetalTerrainSetFailed` arm that calls `terrain::handle_petal_terrain_set_failed` (the
     contract).
  5. `status_bar.rs`: add a right-aligned "⚠ {last error}" segment while `error_history` has an entry
     less than 60 s old. Clicking it opens a popover listing the history with copy buttons. Render
     every `conditions()` entry as a persistent chip (the data source for X-05; the watchdog writer is
     W3-S2's).
- **Acceptance:** a bare `DbResult::Error("Set petal terrain failed: permission denied: …")` produces an
  error notice naming the role, plus a status-bar entry. A failed Join shows inline in the dialog. A
  successful Join navigates into the verse.
- **Flow tests:** `db_error_reaches_error_notice_and_history` (headless `App` with the
  `VerseManagerPlugin` systems; write `DbResult::Error`, assert notice and history);
  `join_failure_keeps_dialog_open_with_inline_error`; `join_success_navigates_and_toasts`
  (`nav.active_verse_id` set); `fe-database`: `join_with_invalid_invite_replies_join_verse_failed`
  (real DB thread channel, garbage invite string).

#### W1-S3 · Petal-map truth (M-03) + Maps layout · sonnet
- **Closes:** M-03, M-01, M-04, X-02 (Set map / Remove site), X-16 (Maps rows).
- **Steps:**
  1. `terrain_map/mod.rs`: add `PetalMapState.pending_set: Option<PendingMapSet { correlation_id,
     previous_tileset_ids, previous_world_scale }>`.
  2. `actions/hexon.rs::set_petal_map` and `set_petal_map_scale`: mint `next_ui_query_id("petalmap")`
     and send it as `correlation_id: Some(..)`. Keep the optimistic update and record the previous
     values in `pending_set`.
  3. `db_results/terrain.rs`: a loaded echo whose correlation matches clears `pending_set`.
     `handle_petal_terrain_set_failed` (correlation match) restores the previous ids and scale, clears
     pending, and calls `show_error(humanize_db_error(..))`. A foreign correlation is ignored.
  4. `hexon_manager.rs`: replace the 7-column Grid with one card per map (name, a size/zoom/tiles line,
     and an action row). While `pending_set` is `Some`, show "Applying…" in place of "Active map ✓".
     Gate Set/Clear with `role_gated(local_role.can_edit(), role_denial_reason(..))` (if `LocalUserRole`
     is not in this render fn's params, report NEEDS-WIRING → W3-S1). Make Remove a `confirm_button`
     ("Remove" → "Confirm remove"). Use `clipped_value` for the region and storage dir.
- **Acceptance:** after a denied set, the row shows the previous state and an error notice explains the
  role, with no stale "Active map ✓". `fractalengine/src/terrain_bridge.rs` still applies the active
  petal's echo, because a correlated echo is still `PetalTerrainLoaded`. Verify by reading the code;
  do not edit it.
- **Flow tests:** `denied_petal_map_set_rolls_back_and_explains` (headless `App`: send via
  `set_petal_map`, inject `PetalTerrainSetFailed` with the captured correlation, assert rollback and
  notice); `confirmed_petal_map_set_clears_applying`; `foreign_correlation_does_not_clear_pending`
  (table-driven with the previous two where they share setup).

#### W1-S4 · Shared widgets + Join/Import/Create dialog honesty · sonnet
- **Closes:** X-02 (helper), X-06 (helper), D-05 (dialog half), D-06, D-07, X-17 (create_entity,
  gltf_import, join), and L-06 (NEEDS-WIRING for camera focus → W3-S1).
- **Steps:**
  1. `widgets.rs`: add `role_gated`, `role_denial_reason`, and `confirm_button`, extracted from the
     inspector revoke pattern (`inspector.rs:924-970`, read only). The arm state lives in egui temp
     memory and disarms after 4 s or on a click elsewhere.
  2. `join.rs`: Join sets `ui_mgr.join_request.pending = true` and keeps the window open with a spinner.
     Render `join_request.error` inline in error colour. Add an ✕ and themed frame. Use `send_or_toast`.
  3. `gltf_import.rs`: disable Import until there is a path and an active petal. Keep the dialog open
     until the result arrives. Show the position in meters with `widgets::world_to_meters`.
  4. `create_entity.rs`: disable Create on an empty name, `request_focus`, Enter submits, toast on
     result. For a node created at the camera focus, add a `spawn_position: Option<[f32;3]>` parameter
     and report NEEDS-WIRING (W3-S1 threads the focus).
- **Acceptance:** no dialog closes on a no-op. Every destructive site from here on can call
  `confirm_button`.
- **Flow tests:** `confirm_button_requires_second_click_and_disarms` (headless `egui::Context::run`
  pattern from `panels/toolbar.rs` tests); `role_gated_disables_and_explains` (same seam, assert the
  disabled response and hover text); `join_dialog_stays_open_while_pending`;
  `gltf_import_disabled_without_petal_or_path` (table-driven).

#### W1-S5 · OF-13a/b/c petal-map hardening · opus
- **Closes:** OF-13a, OF-13b, OF-13c (`conductor/accountability-log.md:637-658`, using the **preferred**
  shapes from the addendum). Carried if they stay inside `sharding.rs`: OF-12b
  (verse-authority-to-unmap-with-holders) and `reverdict_petal_mappings` author-indexing.
- **Steps:**
  1. Store `from_fractal` on each `PetalMapping` so the rollback authority check doesn't depend on stack
     position. Moves are then always applied, and the refusal path in `push_mapping_layer`
     (`sharding.rs:365`) is deleted. This closes OF-13c and OF-13a, because the evict scan no longer
     drives correctness.
  2. Make `contested_claimed_fractals` (`:449`) a bounded set of `(author, fractal)` pairs. At settle,
     re-check the author is Editor+ and drop lapsed pairs. Clear the set everywhere `contested_petals`
     is removed, including the locally-authoritative branch (`:983`, `:987`). This closes OF-13b.
  3. Apply the carried items only if they need no file outside the owns list. Otherwise record them in
     the report.
- **Acceptance:** each fix has a test that fails on the pre-fix code (mutation-checked). The existing
  fe-sync suite and the role-propagation harness scenario still pass at the gate.
- **Flow tests:** `lapse_rollback_restores_replaced_mapping_not_older_layer` (OF-13a);
  `revoked_author_refused_fractal_stops_blocking_settlement` (OF-13b);
  `authorized_move_applies_when_all_history_layers_protected` (OF-13c). Drive them through the public
  shard-router ingest API used by the existing `sharding.rs` tests.

**W1 GATE crates:** `fe-ui`, `fe-runtime`, `fe-database`, `fe-sync`, plus `fe-test-harness` (exhaustive
matches). Reviewer focus: the W1 spine *is* the pattern every later wave reuses, so the advocate must
leave the notice and widgets APIs minimal and named well. The critic checks that an `Error` can never
be starved by `Info` spam, and that `humanize_db_error` never hides the raw message from the history.

---

### Wave 2 — Role gating + destructive safety in content surfaces (L_B-free)

**Contracts:** W1's notice and widget APIs. Plus:
`RightSidebarState::toggle_collapsed(&mut self)` and `RightSidebarState.collapsed: bool` (W2-S3; W2-S5
binds the key); `ModalManagerState::clear_quarantine(&mut self, panel: &str)` and
`humanized_panel_name(id: &str) -> &'static str` (W2-S4; W3-S1 wires Retry).

| Slice | Title | Model | Owns |
|---|---|---|---|
| W2-S1 | Left sidebar: navigate vs collapse, row menus, gating | sonnet | `fe-ui/src/panels/sidebar.rs`, `fe-ui/src/ui_shell/left_sidebar.rs` |
| W2-S2 | Inspector: empty state, property editing, gating, two-step deletes | sonnet | `fe-ui/src/panels/inspector.rs` |
| W2-S3 | Right sidebar sections: Groups, Settings, Danger zone, rail, Maps chrome | sonnet | `fe-ui/src/ui_shell/right_sidebar.rs`, `fe-ui/src/ui_shell/mod.rs`, `fe-ui/src/dialogs/hexon_manager.rs`, `fe-ui/src/verse_manager/db_results/hierarchy.rs`, `fe-ui/src/ui_shell/AGENTS.md` |
| W2-S4 | Dialog safety: manifest, node options, peer debug, modal copy | sonnet | `fe-ui/src/dialogs/petal_manifest.rs`, `fe-ui/src/dialogs/node_options.rs`, `fe-ui/src/dialogs/peer_debug.rs`, `fe-ui/src/ui_shell/modal.rs`, `fe-ui/src/verse_manager/db_results/nodes.rs` |
| W2-S5 | Shortcuts + Terrain Tools panel | sonnet | `fe-ui/src/node_manager/shortcuts.rs`, `fe-ui/src/panels/terrain_tools_panel.rs`, `fe-ui/src/panels/proposal_report_panel.rs`, `fe-ui/src/node_manager/AGENTS.md` |

Disjointness: verified, with no shared path and none in §2. `panels/AGENTS.md` is unowned this wave,
so DOC NOTEs go to the advocate.

#### W2-S1 · Left sidebar · sonnet
- **Closes:** L-01, L-02 (removal half), L-03, L-04, L-05, L-07, X-02 ("+" buttons and row verbs),
  X-11 (eye and "+" hit targets ≥ 22 px), X-16 (tree names via `clipped_value`), X-17 (`sidebar.rs`
  sends).
- **Steps:**
  1. Extract a pure `tree_click_outcome(level, clicked_id, nav) -> TreeClick { Navigate, Noop }`. The
     chevron toggles and the name navigates. Navigating to the already-active level is `Noop`
     (`sidebar.rs:250-270, 343-364, 421-462`).
  2. Remove Reset Database from the sidebar (`:70-120`). W2-S3 adds the Danger zone.
  3. Nodes in other petals get a tooltip "Enter {petal} to select" and a click that navigates then
     selects (L-03).
  4. Add a right-click menu on every row: Rename, Settings…, Copy ID, and Delete… (two-step). Delete is
     offered only where a `DbCommand` exists; grep for it and omit (and report) otherwise.
  5. Labels "+ Fractal / + Petal / + Node". "Manifest" becomes "Petal requirements…". Drop the "Space"
     grid (L-07).
  6. `role_gated` on every "+" and edit verb.
- **Flow tests:** `tree_click_outcome_table` (table-driven: active/inactive × level × chevron/name);
  `collapsing_active_verse_keeps_petal_scene` (headless `App`: the nav state is unchanged after
  clicking the active verse's name); `viewer_sees_disabled_create_buttons_with_reason`.

#### W2-S2 · Inspector · sonnet
- **Closes:** I-01, I-02, I-03, I-04, X-06 (property ✕, field-def ✕), X-02 (inspector edits),
  X-11 (✕ size), X-13 (token expiry as "expires in 6 d"), X-17 (`inspector.rs` sends), P-01 (inline
  URL validation), and dead code at `inspector.rs:221,226` (delete).
- **Steps:**
  1. With nothing selected, show only the empty state plus petal-level actions. The API tab gets an
     empty state.
  2. Property values are editable in place with inline type validation ("number" rejects non-numeric
     input instead of becoming a string). Show friendly type names and widen the fields.
  3. Field-def delete stays disabled while the id is `pending-N` (`:1359-1366`).
  4. Every copy action toasts. The API base comes from config, with the curl example using an
     authenticated endpoint and a "Generating…" in-flight state (I-04).
  5. `confirm_button` on property ✕ and field-def ✕.
- **Flow tests:** `pending_field_def_cannot_send_delete` (assert no `DeleteFieldDef { "pending-0" }`
  reaches the channel); `property_value_type_validation_table`; `empty_selection_hides_transform_apply`.

#### W2-S3 · Right sidebar sections · sonnet
- **Closes:** G-01, X-06 (Group delete), X-02 (Groups editor), S-01, S-02 (render distance, timeseries
  R and bucket width commit on `drag_stopped()`), S-03 (OD-13), L-02 (Danger zone, OD-4), T-02 (rail
  labels when wide), T-03 (collapse chevron and `toggle_collapsed`), X-14 (`:641`, `:883`, `:1031-1038`),
  X-11 (rail glyphs), M-02 (Maps through `section_chrome`, and the blocking-dialog hint names the
  dialog), and R1 placeholder arms (`right_sidebar.rs:436`: make each arm real or remove the section).
- **Steps:** follow R2's fix column per id. The `DatabaseReset` result toasts "Database reset" in
  `hierarchy.rs::handle_database_reset`. Rename commits on Enter and reverts on Escape (G-01). At the
  cap, Add shows `on_disabled_hover_text("Max N groups")`.
- **Flow tests:** `render_distance_drag_sends_one_command_on_release` (headless `egui` drag sequence,
  then count the emitted `SetPetalRenderDistance` commands, observing the channel rather than a mock:
  1 on release, 0 mid-drag); `group_rename_escape_reverts`; `reset_requires_typed_confirmation`.

#### W2-S4 · Dialog safety · sonnet
- **Closes:** D-08, S-01 (remove the manifest render-distance field UI), X-15 and X-14 (manifest copy
  `:46,70,137,189`; node_options "tombstone" `:185`), D-09 (toast on `NodeRenamed` or the error in
  `db_results/nodes.rs`), X-06 (Clear Properties two-step), D-11 (`modal.rs:200-204`,
  `node_options.rs:208`: "Counting children…" vs "Delete this node?"), B-02 (humanized panel names,
  `clear_quarantine` API), dead code `modal.rs:188` (delete), D-10, and V-04 (peer_debug honest copy).
- **Flow tests:** `manifest_close_with_unsaved_edits_asks_to_discard`;
  `delete_confirm_copy_distinguishes_pending_from_leaf` (table: `None` / `Some(0)` / `Some(3)`);
  `node_rename_echo_toasts_saved`.

#### W2-S5 · Shortcuts + Terrain Tools panel · sonnet
- **Closes:** X-07 (Delete opens the existing two-step confirm, F sets `CameraFocusTarget` for the
  selection), X-08 (ignore tool keys while Ctrl/Cmd/Alt is held; keep the Brush exemption),
  V-06 (Esc closes `ActiveDialog::ContextMenu` as the top rung), T-03 (Ctrl+Shift+B →
  `toggle_collapsed`), O-04, R-02 ("#1 Raise · 314 m² · +2.0 m", click to frame), R-03 (read the
  record's material, hide non-applicable metrics), X-06 (proposal ✕), X-02 (Add proposal), and X-13
  (`{:?}` op names at `terrain_tools_panel.rs:287` and `proposal_report_panel.rs:123`).
- **Flow tests:** `ctrl_s_does_not_switch_tool` (table over the R2 list of Ctrl+key combos);
  `escape_closes_context_menu_first`; `delete_key_opens_confirm_not_delete` (no tombstone command
  emitted on the first press); `proposal_row_label_is_human`.

**W2 GATE crates:** `fe-ui`. Reviewer focus: the advocate checks that every destructive site uses
`confirm_button` and every edit control uses `role_gated` (grep both). The critic verifies that gating
is advisory only (the server-side `fe-policy` gate stays authoritative) and that no new role check
reads a stale role (X-03 lands in W3).

---

### Wave 3 — Lane-B integration surfaces (PRECONDITION: Lane B landed)

**Contracts:** `status_bar(.., local_role: &LocalUserRole)` gains a parameter (W3-S2 adds it, W3-S1
passes it). Theme tokens `theme::NOTICE_INFO_BG`, `NOTICE_ADVISORY_BG`, `NOTICE_ERROR_BG`,
`STATUS_NORMAL`, and `fn tab_fill(active: bool) -> Color32` (W3-S5 defines them, W3-S1 and later waves
use them). `DbCommand::GetEntitySettings { scope }` → `DbResult::EntitySettingsLoaded { scope,
default_access, description }` (W3-S4). The EntitySettings dialog fields `settings_loading: bool`,
`default_access_buf: Option<String>` (`None` while loading) and `description_buf: Option<String>`
(W3-S4 defines them, W3-S3's open sites fill them).

| Slice | Title | Model | Owns |
|---|---|---|---|
| W3-S1 | Notice renderer, per-section panel guard, NEEDS-WIRING receiver | sonnet | `fe-ui/src/panels/mod.rs`, `fe-ui/src/dialogs/create_entity.rs`, `fe-ui/src/dialogs/gltf_import.rs`, `fe-ui/src/dialogs/join.rs`, `fe-ui/src/dialogs/hexon_manager.rs`, `fe-ui/src/panels/sidebar.rs`, `fe-ui/src/ui_shell/left_sidebar.rs` |
| W3-S2 | App-level role state, mesh-budget condition, role chip into status bar | sonnet | `fe-ui/src/plugin.rs`, `fe-ui/src/role_chip.rs`, `fe-ui/src/panels/status_bar.rs` |
| W3-S3 | Viewport overlay: crumbs, hints, card grids, gears | sonnet | `fe-ui/src/viewport.rs`, `fe-ui/src/actions/mod.rs` |
| W3-S4 | Entity Settings: real state, access management, tokens | sonnet | `fe-ui/src/dialogs/entity_settings.rs`, `fe-ui/src/dialogs/mod.rs`, `fe-ui/src/verse_manager/db_results/mod.rs`, `…/db_results/roles.rs`, `…/db_results/tokens.rs`, `fe-runtime/src/messages.rs`, `fe-database/src/lib.rs`, `fe-database/src/handlers/entity_settings.rs` (future), `fe-database/src/handlers/mod.rs`, `fe-database/tests/entity_settings_test.rs` (future) |
| W3-S5 | Topbar gating, theme tokens and contrast, toolbar help, T-01 conformance | sonnet | `fe-ui/src/ui_shell/topbar.rs`, `fe-ui/src/theme.rs`, `fe-ui/src/panels/toolbar.rs` |

Disjointness: verified. W3-S1's dialog and sidebar files are owned only for threading NEEDS-WIRING
params reported in W1 and W2, and no other W3 slice owns them.

#### W3-S1 · Notice renderer + panel guard · sonnet
- **Closes:** X-04 (renderer: severity fill from theme tokens, `interactable(true)`, hover calls
  `hold_notice`, an ✕ calls `dismiss_notice`, and errors use normal-size text), B-02 (guard per
  right-sidebar section plus a Retry button calling `clear_quarantine`), L-06 (thread the camera focus
  into `create_entity`), and every W1/W2 `NEEDS-WIRING` line (for example, `LocalUserRole` into
  `hexon_manager` and `sidebar`).
- **Flow tests:** `error_notice_held_while_hovered` (headless egui with the pointer over the notice
  rect past 7 s: still visible); `quarantined_section_retry_restores_render`.

#### W3-S2 · App-level role state + role chip · sonnet
- **Closes:** X-03 (clear `LocalUserRole` when the scope becomes `None`; unresolved shows "—"),
  X-05 (the watchdog at `plugin.rs:136-142` calls `ui_mgr.set_condition("mesh_budget", ..)` while
  exceeded and clears it after), B-01 (delete the floating chip; the status bar shows
  "Role: Editor · {verse}" with a hover listing what that role can do), B-03, X-17 (`plugin.rs:1200-1203`),
  and R1 `plugin.rs:904` (`fe-identity` is already a dependency, so populate `node_identity`).
- **Flow tests:** `leaving_verse_clears_local_role` (headless `App`: navigate to a verse, then back,
  and assert the role is unresolved); `mesh_budget_condition_persists_while_exceeded`.

#### W3-S3 · Viewport overlay · sonnet
- **Closes:** D-01 (the gear is always shown on every card, OD-5), D-02 (open-site half: stop
  hardcoding `Some("viewer")` at `viewport.rs:301,551,770` and `actions/mod.rs:1377-1378`; open with
  `settings_loading: true` and send `GetEntitySettings`), V-01 (remove the ID crumb), V-02 (wrapped hint
  with a backing plate in an anchored Area), V-03 (`ScrollArea` and pluralization), V-04 (the
  verse-browser empty state offers "Have an invite? Join a verse →"), V-05 (clickable crumbs), X-10
  (viewport hint plate), X-13, X-14 (`viewport.rs:127`), and X-16 (card titles).
- **Flow tests:** `pluralize_table` (pure); `open_entity_settings_requests_real_state` (assert
  `GetEntitySettings` is emitted and the radio is not preset).

#### W3-S4 · Entity Settings · sonnet (critic runs `/security-review`)
- **Closes:** D-02 (the new command, handler and dispatch arm), D-03 (send `ResolveRolesForPeers` when
  the Access tab opens; role combos send `AssignRole`/`RevokeRole`, and a downgrade goes through
  `confirm_button`), D-04, D-01 (in-dialog per-tab gating: Claim is visible to non-owners, Access and
  API need Manager), X-06 (Revoke ×2 at `entity_settings.rs:804-806, 900-902`), X-13 (raw scope behind
  a copyable "ID"; human expiry), X-16 (token scope and client), X-17 (×10 sends), and M-02/OD-16
  (remove `ActiveDialog::InviteDialog` and its opener `db_results/roles.rs:10`).
- **Steps:** `get_entity_settings_handler(db, scope)` reads `default_access` and `description` for
  verse, fractal and petal scopes with no write. Authorization: a read at Viewer+ (consult `fe-policy`
  like the other read handlers).
- **Flow tests (real store):** `entity_settings_load_reflects_private_verse`;
  `viewer_cannot_assign_role_server_side` (assert the DB refuses even though the UI is gated);
  `access_tab_open_requests_peer_roles` (headless `App`, assert the command is emitted).

#### W3-S5 · Topbar, theme, toolbar · sonnet
- **Closes:** T-04, X-02 (topbar edit-tool buttons are gated; activating an edit tool as a Viewer
  shows an advisory), X-10 (TEXT_MUTED ≈ (140,140,160), TEXT_DIM ≈ (155,155,175); prove ≥ 4.5:1 with
  a contrast unit test), X-12 (`tab_fill`, `STATUS_NORMAL`, a neutral primary button, and the
  red-Deselect and topbar Data call sites), X-09 (a "?" shortcut overlay generated from `TOOL_DEFS` and
  the §5 modifier table), and T-01 conformance (check Lane B's title bar for a drag region,
  double-click maximize, min/max/close, and ✕ warning when the Petal Manifest is dirty; fix gaps inside
  `topbar.rs` only, and report anything else).
- **Flow tests:** `theme_text_tokens_meet_wcag_aa_on_panel` (pure contrast-ratio computation over the
  token pairs); `viewer_tool_buttons_disabled_with_reason`.

**W3 GATE crates:** `fe-ui`, `fe-runtime`, `fe-database`, plus `fe-test-harness` and `fe-api` if
exhaustive matches change. **VISUAL** (OD-17): R2 checklist items 1, 2, 5, 6, 8, 13 and 15.

---

### Wave 4 — Tools, reports, context menu, data window (dead and unreachable features)

**Contracts:** `panels::transform_options::render(ui: &mut egui::Ui, tool: Tool, snap: &mut
ToolSnapSettings)` and `#[derive(Resource)] ToolSnapSettings { grid_m: Option<f32>, angle_deg:
Option<f32>, scale_step: Option<f32>, uniform_scale: bool, axis_lock: [bool; 3] }` (W4-S2;
registered by `NodeManagerPlugin`; W4-S1 calls `render` from the Move/Rotate/Scale arms of
`tool_options::render`; W4-S2 threads `&mut ToolSnapSettings` through `right_sidebar.rs`).
`VolumeReportResult` gains `unavailable: Option<VolumeUnavailable { NoMap, WrongPetal }>` (W4-S3).
`DbResult::NodeUrlUpdated { node_id, url }` (W4-S4).

| Slice | Title | Model | Owns |
|---|---|---|---|
| W4-S1 | Terrain tool completion: polygon draft, placement, Pen home | sonnet | `fe-ui/src/node_manager/brush_interaction.rs`, `fe-ui/src/panels/tool_options.rs`, `fe-ui/src/actions/terrain_proposal.rs`, `fe-ui/src/panels/terrain_tools_panel.rs` |
| W4-S2 | Transform snapping (replaces "(soon)") | sonnet | `fe-ui/src/panels/transform_options.rs` (future), `fe-ui/src/panels/tool_inspector.rs`, `fe-ui/src/node_manager/gimbal_interaction.rs`, `fe-ui/src/node_manager/mod.rs`, `fe-ui/src/ui_shell/right_sidebar.rs`, `fe-ui/src/node_manager/AGENTS.md` |
| W4-S3 | Volume report never hangs | sonnet | `fe-terrain/src/terrain_plugin.rs`, `fe-renderer/src/terrain_overlay.rs`, `fe-ui/src/panels/volume_report_panel.rs`, `fe-terrain/src/AGENTS.md`, `fe-renderer/src/AGENTS.md` |
| W4-S4 | Context menu, portal, URL ack | sonnet | `fe-ui/src/dialogs/context_menu.rs`, `fe-ui/src/panels/portal_toolbar.rs`, `fe-ui/src/actions/portal.rs`, `fe-ui/src/portal/**`, `fe-ui/src/actions/mod.rs`, `fe-ui/src/verse_manager/db_results/mod.rs`, `fe-ui/src/verse_manager/db_results/nodes.rs`, `fe-runtime/src/messages.rs`, `fe-database/src/lib.rs` |
| W4-S5 | Paths + Data window + stamps | sonnet | `fe-ui/src/panels/path_editor_card.rs`, `fe-ui/src/panels/gis_panel.rs`, `fe-ui/src/panels/query_tab.rs`, `fe-ui/src/panels/annotation_card.rs`, `fe-ui/src/panels/asset_card.rs`, `fe-ui/src/panels/egress_card.rs`, `fe-ui/src/panels/tool_panel.rs`, `fe-ui/src/panels/layer_manager_card.rs`, `fe-ui/src/panels/AGENTS.md` |

Disjointness: verified. `node_manager/mod.rs` is owned only by S2 (which registers
`ToolSnapSettings`). `right_sidebar.rs` is owned only by S2 (param threading).

#### W4-S1 · Terrain tool completion · sonnet
- **Closes:** O-01 (S1, gesture OD-15: viewport clicks append to `region_draft`, with markers, live
  area, Enter/double-click/"Close polygon", and Backspace/Esc), O-02 ("arm then click to place" for
  shapes and proposals at the last viewport click; remove the dev-promise copy), O-03 (Options → Pen
  hosts a path picker plus points; fix the cross-referencing hints), O-06 ("Volume report →"), X-02
  (brush press gated for Viewer, `brush_interaction.rs:200-230`), and the R1 tool_options "(soon)" UVG
  (Move/Rotate/Scale arms call `transform_options::render`).
- **Flow tests:** `polygon_draft_clicks_append_and_close_applies_region` (headless `App` with the
  Brush+Polygon route: three clicks then Enter → the region apply command is emitted with three
  vertices); `armed_shape_places_at_clicked_point_not_origin`; `viewer_brush_press_emits_no_sculpt`.

#### W4-S2 · Transform snapping · sonnet
- **Closes:** O-05 and the R1 tool_inspector "(soon)" UVG: grid snap (m), angle snap, scale step,
  uniform/per-axis scale, and axis lock, applied in `gimbal_interaction.rs` drag math; remove
  "Pivot", "Selection filters" and "Highlight style" (OD-14). The descriptor `options_hints` keep only
  real hints.
- **Flow tests:** `snap_math_table` (pure: value, grid → snapped, including negatives and the 0.5
  boundary); `gimbal_drag_with_grid_snap_commits_snapped_transform` (headless `App`, assert the
  committed `Transform`).

#### W4-S3 · Volume report never hangs · sonnet
- **Closes:** R-01 (the responder at `terrain_plugin.rs:2205-2210` emits `unavailable` instead of
  `continue`; the panel shows "No map on this petal — assign one in Maps →" with a link that opens
  Maps), R-04 (`copy_row` per figure, names instead of ids), X-13 (`volume_report_panel.rs:466-469`),
  and X-14 (`:429-435` lattice copy).
- **Flow tests** (`cargo test -p fe-terrain --features render`): `volume_request_without_map_replies_unavailable`;
  `volume_request_for_wrong_petal_replies_unavailable`. In fe-ui: `volume_panel_shows_no_map_reason`.

#### W4-S4 · Context menu, portal, URL ack · sonnet
- **Closes:** D-12 (delete the stale toasts at `context_menu.rs:774,786`), D-11 (`:885`), X-02 (every
  edit verb `role_gated`), X-13 (`:621-643`), X-14 (`:143-145,175-177,190-198,316-318,862-871`),
  X-15 (`:109,133,166,175,190-198,615,774,786`), V-06 (verify against W2's Esc; clamp the menu
  position at the screen edges), P-02 (Reload, Forward, Open externally, a loading state, and a "Closed
  because selection changed" notice), and P-03 (a `NodeUrlUpdated` ack; the toast says "Saving…" then
  "URL saved" on ack).
- **Flow tests:** `url_save_confirms_only_on_ack` (headless `App`: the notice before the ack is
  "Saving…", after `NodeUrlUpdated` it is "URL saved"); fe-database: `update_node_url_replies_ack`;
  `viewer_context_menu_edit_verbs_disabled`.

#### W4-S5 · Paths + Data window + stamps · sonnet
- **Closes:** DW-01 (separate Annotations buffer, auto-load, explicit "0 results"), DW-02, DW-03 (meters
  with labels, 1-based indices, "Path N" default name), DW-04 (TTL inline validation only; the mint
  stays OUT per D-64), X-06 (point Remove), S-02 (`path_editor_card.rs:481-490` commits on
  `drag_stopped`), O-07 (pick the stamp in the viewport, one Apply, "Make editable node"), I-05
  (copy-for-BI rows on the Query tab), I-06, I-07, X-02 (layer toggles), X-12 (these panels via
  `theme::tab_fill`), X-15 (`path_editor_card.rs:399`), X-16, and `FEUI-TERRAIN-MODE-TODO`
  (`tool_panel.rs:134,1016`: remove the stale TODOs).
- **Flow tests:** `path_point_drag_sends_one_move_on_release`; `annotations_tab_never_shows_query_rows`;
  `share_ttl_invalid_is_rejected_inline` (table-driven).

**W4 GATE crates:** `fe-ui`, `fe-terrain --features render`, `fe-renderer`, `fe-runtime`,
`fe-database`. Reviewer focus: the advocate checks that the snap math is shared and pure (one
function, not three copies). The critic checks that no preview path writes the DB
(delta-viz tripwire) and that the polygon gesture can't fire while the pointer is over egui.

---

### Wave 5 — Security seam F7, undo/redo, stale-TODO cleanup

| Slice | Title | Model | Owns |
|---|---|---|---|
| W5-S1 | F7: server-side scope for user-authored SQL (`ScopedQuery`) | opus | `fe-query/src/sql_guard.rs` (future), `fe-query/src/lib.rs`, `fe-api/src/query_guard.rs`, `fe-runtime/src/messages.rs`, `fe-database/src/lib.rs`, `fe-database/src/handlers/scoped_query.rs` (future), `fe-database/src/handlers/mod.rs`, `fe-database/src/handlers/raw_query.rs`, `fe-ui/src/actions/query.rs`, `fe-ui/src/panels/query_tab.rs`, `fe-ui/src/verse_manager/db_results/query.rs`, `fe-database/tests/scoped_query_test.rs` (future), `fe-database/src/AGENTS.md`, `fe-api/AGENTS.md`, `fe-runtime/src/AGENTS.md` |
| W5-S2 | Undo/redo for transforms and path points | opus | `fe-ui/src/undo.rs` (future), `fe-ui/src/lib.rs`, `fe-ui/src/node_manager/shortcuts.rs`, `fe-ui/src/node_manager/transform_broadcast.rs`, `fe-ui/src/node_manager/path_point_interaction.rs`, `fe-ui/src/node_manager/mod.rs`, `fe-ui/src/node_manager/AGENTS.md` |
| W5-S3 | Stale TODO and dead-function cleanup (ledgered) | haiku | `fe-database/src/queries.rs`, `fe-database/src/role_manager.rs`, `fe-database/src/handlers/field_def.rs`, `fe-database/src/api_token_store.rs`, `fe-sync/src/status.rs`, `fe-sync/src/blob_store.rs` |

Disjointness: verified. `fe-database/src/handlers/mod.rs` is owned only by S1. S3 deletes the dead
`field_def.rs:79` function in place and needs no `mod.rs` or `lib.rs` change.

#### W5-S1 · F7 ScopedQuery · opus (critic runs `/security-review`)
- **Closes:** F7 (`conductor/accountability-log.md:678-693`), R1 #4.
- **Steps:**
  1. Move the pure functions behind `prepare_scoped_sql` (`fe-api/src/query_guard.rs:856` and its
     helpers and constants, plus `with_statement_timeout` if it is pure) into `fe-query::sql_guard`
     with no new dependencies. In `fe-api/src/query_guard.rs`, `pub use` them so every existing path
     (including Lane B's `rest.rs` and `screenshot.rs` imports) is unchanged. Existing guard tests move
     with the code or stay and pass through the re-export.
  2. Add `DbCommand::ScopedQuery { sql, vars, petal_scope: Vec<String>, correlation_id: String }`.
     Handler: (a) for each petal, resolve its scope and require the local DID at Viewer+ through the
     same policy/role resolution the write gates use, and deny with a plain `QueryFailed` otherwise;
     (b) `prepare_scoped_sql(sql, &petal_scope, GuardMode::Query)`; (c) execute with the statement
     timeout. Reply with `QueryResult` or `QueryFailed` echoing the id.
  3. Query tab: `submit` sends `ScopedQuery` with `[active_petal]`. With no active petal it shows
     inline "Open a petal to query it". The scope chip shows the petal **name**.
  4. Change the `RawQuery` doc comment to "server-built SQL only — user-authored SQL must use
     ScopedQuery". Grep every `DbCommand::RawQuery {` constructor (about 33 across 20 files at
     6531d19) and confirm none carries user-typed SQL. Report any that does.
- **Flow tests (real SurrealKV store, pattern `fe-database/tests/node_data_write_readback_test.rs`):**
  `scoped_query_returns_only_scoped_petal_rows`;
  `scoped_query_where_foreign_petal_returns_empty`;
  `scoped_query_denied_without_viewer_role_on_petal`;
  `scoped_query_refusals_table` (comments, a `verse:` record literal, a non-FROM mention of `node`,
  `;`, and `RETURN`). Existing `fe-api` query_guard and analytics_scope suites must stay green at the
  gate.

#### W5-S2 · Undo/redo · opus
- **Closes:** X-07 (Ctrl+Z / Ctrl+Shift+Z, OD-6).
- **Steps:** use the Command pattern (`UndoEntry { node_id, before, after, kind }`). Record entries at
  the `broadcast_transform` commit and at path-point move commit. Undo dispatches the inverse through
  the same path (DB + P2P + VerseManager). Cap the stack at 50, clear it on petal switch, and clear
  redo on a new edit. An entry whose node's current transform no longer equals `after` (a peer changed
  it) is skipped with an advisory notice.
- **Flow tests:** `undo_move_restores_prior_transform_and_sends_it` (headless `App`, real channel:
  assert the emitted command and the `VerseManager` position); `redo_after_undo_reapplies`;
  `new_edit_clears_redo`; `undo_skips_node_changed_by_peer`.

#### W5-S3 · Stale cleanup · haiku
- **Closes:** ledger `DB-TODO-QUERIES` (`:364`), `DB-TODO-SCOPEMEMBERS` (`:365`, delete the uncalled
  function), `SYNC-STALE-TODOS` (`:360`), R1 `field_def.rs:79` (delete the dead handler if no
  `DbCommand` calls it; grep first), and R1 `api_token_store.rs:232` (delete the stale test-scaffold
  TODO).
- **Acceptance:** the TODOs are gone or reworded as notes, and there is no behaviour change. No tests
  are needed beyond the gate.

**W5 GATE crates:** `fe-query`, `fe-api`, `fe-database`, `fe-runtime`, `fe-ui`, `fe-sync`. Reviewer
focus: the critic's security-review covers W5-S1 (scope bypass through subqueries, string escaping of
the petal filter at `query_guard.rs:820-826`, and timing). The advocate checks that the undo command
pattern stays two operation kinds, not a framework.

---

### Wave 6 — Library seams, dead code, final copy/clipping sweep

| Slice | Title | Model | Owns |
|---|---|---|---|
| W6-S1 | fe-plugin read path + transaction forwarding | sonnet | `fe-plugin/src/**`, `fe-plugin-test/src/**`, `extensions/iot-bridge/**` |
| W6-S2 | fe-webview scaffolding honesty (OD-8) | haiku | `fe-webview/src/tauri_commands.rs`, `fe-webview/src/backends/mod.rs`, `fe-webview/src/backends/stub.rs`, `fe-webview/src/lib.rs`, `fe-webview/src/AGENTS.md` |
| W6-S3 | Dead-code census | haiku | `fe-ui/src/actions/node_props.rs`, `fe-ui/src/verse_manager/residency_ledger.rs`, `fe-ui/src/node_manager/router.rs`, `fe-sim/src/net.rs`, `fe-hexon/src/**` |
| W6-S4 | Copy, terminology, clipping, and the R2 coverage audit | sonnet | `fe-ui/src/panels/inspector.rs`, `fe-ui/src/panels/sidebar.rs`, `fe-ui/src/ui_shell/right_sidebar.rs`, `fe-ui/src/dialogs/hexon_manager.rs`, `fe-ui/src/dialogs/gltf_import.rs`, `fe-ui/src/panels/status_bar.rs`, `fe-ui/src/viewport.rs`, `fe-ui/src/dialogs/petal_manifest.rs`, `fe-ui/src/dialogs/peer_debug.rs`, `fe-ui/src/panels/tool_options.rs`, `fe-ui/src/panels/path_editor_card.rs`, `fe-ui/src/AGENTS.md` |

Disjointness: verified. The `fe-plugin` and `fe-plugin-test` globs belong only to S1, `fe-hexon/src/**`
only to S3, and every fe-ui file in S4 appears in no other W6 slice.

- **W6-S1 (OD-9).** Closes R1 #1: `get_node` and `query_nodes` (`host_api.rs:21-69`) do request-reply
  through `PluginContext`'s injected `NodeReader`. `GetNode` (`fe-plugin/src/lib.rs:272-281`) returns
  the reader's snapshot. `CommitTransaction` forwards ops to an injected `TransactionSink` instead of
  logging. Delete `lifecycle.rs:153` `activate()` if it has no callers, and delete the dead code at
  `transaction.rs:42`. `MockHostEnv` implements both traits. **Flow tests** (`RhaiTestRunner`):
  `rhai_get_node_returns_reader_snapshot`, `rhai_query_nodes_applies_filter`,
  `commit_transaction_forwards_ops_to_sink`.
- **W6-S2 (OD-8).** Delete the six unreachable stub commands and keep `resolve_asset` and
  `get_asset_base_url`. `StubBackend` warns once ("no webview backend compiled in") at creation, which
  closes R1's `backends/mod.rs:13-20` silent no-op. Update §tauri-commands. Keep the existing
  resolve_asset tests.
- **W6-S3.** Delete dead items at `node_props.rs:21`, `residency_ledger.rs:774`, `router.rs:118,124`
  (keep `:160`, which is ledgered for FR-3) and `fe-sim/src/net.rs:750`. Replace the crate-wide
  `#![allow(dead_code)]` at `fe-hexon/src/lib.rs:1` with deletions, or per-item allows that each carry
  a one-line reason. The gate's clippy run is the test.
- **W6-S4.** Re-grep every R2 evidence anchor for X-11..X-16, X-12, B-03 and L-07 in the owned files,
  and close the residuals with `clipped_value`, `tab_fill`, the §9 vocabulary and plain copy. Then fill
  §4's coverage table by marking each R2 id `closed@<commit>` or `OUT`. **Flow test:**
  `user_copy_has_no_dev_jargon` (scan the rendered strings of the owned panels for a forbidden-term
  table, e.g. "tombstone", "MeshInstanceBudget", "endpoint_api_surface", "hexon" as an artifact noun,
  and "Tools panel". This guards against regressions; the critic must confirm it fails on the
  pre-sweep text).

**W6 GATE crates:** the full workspace (track close). **VISUAL** (OD-17): R2 items 3, 4, 7, 9, 10, 11,
12, 16, 17 and 18.

---

### CLOSE (haiku, after the W6 re-gate)
Update `metadata.json` (`status: done-pending-inapp-verify`, progress, `verified_waves`), tick §5, add
`conductor/doneness-ledger.md` rows (POLISH-W1..W6, and mark `DB-TODO-*`, `SYNC-STALE-TODOS` and
`FEUI-TERRAIN-MODE-TODO` DONE with commits), update the `conductor/tracks.md` row, and add a one-line
retro to `conductor/accountability-log.md`. Owns: those four files plus this plan.

---

## §4 R2 coverage matrix (every id is assigned; W6-S4 marks the commits)

| Ids | Slice(s) |
|---|---|
| X-01 | W1-S1, W1-S2, W3-S1 |
| X-02 | helper W1-S4; sites W1-S3, W2-S1, W2-S2, W2-S3, W2-S5, W3-S5, W4-S1, W4-S4, W4-S5 |
| X-03, B-01, B-03, X-05 | W3-S2 (X-05 API in W1-S1) |
| X-04 | W1-S1 (model), W3-S1 (renderer) |
| X-06 | helper W1-S4; sites W2-S2, W2-S3, W2-S4, W2-S5, W3-S4, W4-S5 |
| X-07 | W2-S5 (Delete, F), W5-S2 (undo) |
| X-08, V-06, O-04, R-02, R-03 | W2-S5 (V-06 edge clamp W4-S4) |
| X-09, T-04 | W3-S5 |
| X-10 | W3-S5 (tokens), W3-S3 (viewport plate) |
| X-11 | W2-S1, W2-S2, W2-S3, residual W6-S4 |
| X-12 | W3-S5, W4-S5, residual W6-S4 |
| X-13 | W2-S2, W2-S4, W2-S5, W3-S3, W3-S4, W4-S3, W4-S4, W4-S5 |
| X-14 | W2-S3, W2-S4, W3-S3, W4-S3, W4-S4, W4-S5 (`tool_panel.rs:441`), residual W6-S4 |
| X-15 | W2-S4, W4-S4, W4-S5, residual W6-S4 |
| X-16 | W1-S3, W2-S1, W3-S3, W3-S4, W4-S5, residual W6-S4 |
| X-17 | helper W1-S1; sites W1-S4, W2-S1, W2-S2, W3-S2, W3-S4 |
| T-01 | Lane B (build); W3-S5 (conformance) |
| T-02, G-01, S-01, S-03 | W2-S3 (S-01 manifest field W2-S4) |
| T-03 | W2-S3 + W2-S5 |
| S-02 | W2-S3, W4-S5 |
| L-01..L-05, L-07 | W2-S1 (L-02 Danger zone W2-S3) |
| L-06 | W1-S4 + W3-S1 |
| V-01..V-03, V-05 | W3-S3 |
| V-04 | W2-S4 + W3-S3 |
| I-01..I-04 | W2-S2 |
| I-05..I-07, DW-01..DW-03, O-07 | W4-S5 |
| DW-04 | W4-S5 (TTL validation); mint OUT (D-64) |
| O-01, O-02, O-03, O-06 | W4-S1 |
| O-05 | W4-S2 |
| R-01, R-04 | W4-S3 |
| M-01, M-03, M-04 | W1-S3 |
| M-02 | W2-S3 (chrome), W3-S4 (InviteDialog) |
| D-01, D-02 | W3-S3 + W3-S4 |
| D-03, D-04 | W3-S4 |
| D-05 | W1-S2 + W1-S4 |
| D-06, D-07 | W1-S4 |
| D-08, D-09, D-10 | W2-S4 |
| D-11 | W2-S4, W4-S4 |
| D-12 | W1-S1, W4-S4 |
| B-02 | W2-S4, W3-S1 |
| P-01 | W1-S1, W2-S2 |
| P-02, P-03 | W4-S4 |

R1 coverage: fe-plugin W6-S1. fe-webview W6-S2. Tool Options and Tool Inspector "(soon)" W4-S1 and
W4-S2. Right-sidebar placeholder arms W2-S3. F7 W5-S1. OF-13 W1-S5. DB/sync TODOs and `field_def` W5-S3.
`plugin.rs:904` W3-S2. `actions/mod.rs:296` W1-S1. Dead code W2-S2 (`inspector.rs`), W2-S4
(`modal.rs`), W6-S1 (`fe-plugin`) and W6-S3 (the rest). `tool_panel.rs` TODOs W4-S5. Peer blob fetch,
T28, S-1, D-23, CANON-WS, CDL-NET and the INTENT items are OUT (spec.md).

## §5 Progress

- [x] W1 leaves · [x] W1 gate · [x] W1 advocate · [x] W1 critic · [x] W1 re-gate
- [ ] W2 leaves · [ ] W2 gate · [ ] W2 advocate · [ ] W2 critic · [ ] W2 re-gate
- [ ] W3 (Lane B landed?) leaves · [ ] gate · [ ] advocate · [ ] critic + security-review · [ ] re-gate · [ ] visual
- [ ] W4 leaves · [ ] W4 gate · [ ] W4 advocate · [ ] W4 critic · [ ] W4 re-gate
- [ ] W5 leaves · [ ] W5 gate · [ ] W5 advocate · [ ] W5 critic + security-review · [ ] W5 re-gate
- [ ] W6 leaves · [ ] W6 gate · [ ] W6 advocate · [ ] W6 critic · [ ] W6 re-gate · [ ] visual
- [ ] CLOSE (ledger, board, retro) · [ ] user in-app look (user-gated, OUT of agent scope)
