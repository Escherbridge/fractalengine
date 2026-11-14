---
type: Review
track: polish_lane_20261031
critic: C-UI
round: 4
head: 50c50d29
date: 2026-11-13
---

# C-UI completeness: polish_lane_20261031 (round 4)

Checked at HEAD `50c50d29` against spec.md (Goal 1-6, A1-A8) and plan.md §5, plus the round-4 items UI-01, UI-02, UI-09, UI-10 and UI-13. Gate read: `review-turn4/R-UI-1.md` (PASS-WITH-FIXES, no HANDED-OFF lines; F7 DEFERRED). No cargo was run.

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names a file owned by this critic or a file of this track's round-4 items; nothing to close.

## Requirements and tasks

| Item | Status | Evidence |
|---|---|---|
| UI-01 Volume report never hangs (explicit no-map / wrong-petal reply) | complete | Reply written once per request: `fe-terrain/src/terrain_plugin.rs:2551` (WrongPetal), `:2553` (NoMap); seam `fe-renderer/src/terrain_overlay.rs:335-339`; panel copy `fe-ui/src/panels/volume_report_panel.rs:761` (NoMap, Maps prompt) and `:773-775` ("Waiting for this petal's map to load."); journeys `terrain_plugin.rs:4222`, `:4345` (R4 `cargo test -p fe-terrain --features render`). |
| UI-02 Heatmap overlay toggle (OD-8 residual) | partial | Checkbox `fe-ui/src/panels/terrain_tools_panel.rs:275`, mirrored to the resource in `fe-ui/src/ui_shell/right_sidebar.rs:150-191`, rendered at `:746-756`; fe-terrain honours it (`terrain_plugin.rs:3348` journey). Missing: "show only while the ProposalOverlay layer is visible" (R-UI-1 F7 DEFERRED; fe-ui cannot read `LayerStack`, NFR-5). |
| UI-09 W6-S3 dead-code residue | complete | `TRACK_NAME_KEY`/`cursor()` removed; `is_owner` now `#[cfg(test)]` at `fe-ui/src/node_manager/router.rs:118-120` (R-UI-2 F5, F15). Doc pointers to the removed constant are still stale (see HANDED-OFF). |
| UI-10 W6-S4 copy residue + jargon guard | complete | Guard `fe-ui/src/ui_shell/right_sidebar.rs:3071` `user_copy_has_no_dev_jargon` (R6 `ui_shell::`); "installed maps" copy and Hexon Manager hint fixed by R-UI-1 F9/F10. Scope limited to the Settings section (R-UI-1 F11 KEPT). |
| UI-13 Close bookkeeping (metadata, ledger rows, retro, R2 coverage) | partial | T-UI `13c82cf7` ticked §5 leaf boxes (`plan.md` §5) and metadata; per-id R2 `closed@<commit>` matrix DEFERRED by T-UI; `conductor/tracks.md:160-162` still reads "pending · 0 / 6 waves". |
| Goal 1 / A2 feedback never silent, roles visible before click | complete (carried) | Closed in W1 with recorded verdicts `reviews/w1-advocate.md`, `reviews/w1-critic.md`; not re-derived this round. |
| A1 S1 findings closed with flow tests | complete (carried) | W1 gate verdict (`reviews/w1-critic.md`). |
| A3 no `.send(..).ok()` on a DbCommand channel in fe-ui | partial | One remains: `fe-ui/src/panels/inspector.rs:1523` `db_tx.send(DbCommand::RevokeApiToken { jti, sub }).ok();` (silent failure, AGENTS anti-pattern 6). |
| A4 F7 ScopedQuery | complete | Query tab sends `DbCommand::ScopedQuery` at `fe-ui/src/actions/query.rs:42`, warns on closed channel `:52`. |
| A5 OF-13a/b/c | complete (carried) | `fe-sync/src/sharding.rs:468-497`, `:561`, `:986`, `:1106` (OF-13 markers); W1 gate. |
| A6 per-wave advocate + critic verdicts | obsolete | Scoper: W2-W6 gate/advocate/critic/re-gate boxes (plan.md:760-764). |
| A7 full workspace gate at close | blocked-user | Sweep runs once at campaign end when the user names it; clippy removed (user decision 2026-11-09). |
| A8 ledger/board updated, every R2 id in a slice or OUT | partial | Coverage table `plan.md` (§4 map, e.g. M-01..P-03 rows); per-id close evidence deferred (UI-13 row). |
| §5 CLOSE (ledger, board, retro) | missing | `plan.md:767` unticked. |

## Excluded by the scoper (not gaps)

- obsolete: W2/W3/W4/W5/W6 gate, advocate, critic, re-gate boxes (plan.md:760-764).
- blocked-user: W3 VISUAL / W6 VISUAL and the user in-app look (plan.md:761, 764, 765/767).
- obsolete: W5-S2 undo/redo (Ctrl+Z).
- obsolete: W2-S1..S5, W3-S1..S5, W4-S1, S2, S4, S5, W5-S1, S3, W6-S1, S2 slice bodies.
- obsolete: W3-S4 as written (GetEntitySettings/EntitySettingsLoaded, ActiveDialog::InviteDialog removal).

## HANDED-OFF (owner = next round)

- HANDED-OFF (owner of `fe-ui/src/ui_shell/right_sidebar.rs`, orchestrator to route; blocks every UI track): fe-ui does not compile at HEAD. `panels/mod.rs:168,275` pass `road_state` and `section_profile` to `render_right_sidebar`, which takes neither (`right_sidebar.rs:328-350`). Apply R-INT-4 NEEDS N1 (five edits) verbatim.
- HANDED-OFF (I-UI-1 next round, owner of `fe-ui/src/panels/inspector.rs`): replace `.send(..).ok()` at `inspector.rs:1523` with `if let Err(..) = .. { warn!(..); }` plus a visible notice (closes A3).
- HANDED-OFF (UI next round, fe-renderer seam owner): UI-02 layer-visibility gate (R-UI-1 F7) needs a fe-renderer mirror of ProposalOverlay layer visibility that fe-ui can read.
- HANDED-OFF (I-DOC-UI next round): `fe-ui/src/AGENTS.md:495` and `fe-ui/src/gis/query.rs:15` still name the deleted `actions::node_props::TRACK_NAME_KEY` (R-UI-2 F8 did not land); repoint to `gis::query::TRACK_NAME_KEY`.
- HANDED-OFF (T-UI next round): per-id R2 `closed@<commit>` matrix (A8) and the §5 CLOSE box.
- HANDED-OFF (I-CONDUCTOR next round): `conductor/tracks.md:162` status line ("pending · 0 / 6 waves") does not reflect W1-W6 leaves landed.

Verdict: PARTIAL - UI-01, UI-09 and UI-10 complete; UI-02 partial (layer gate deferred), A3 has one bare DbCommand `.ok()` left, close bookkeeping open, and fe-ui does not compile until right_sidebar.rs takes R-INT-4 N1.
