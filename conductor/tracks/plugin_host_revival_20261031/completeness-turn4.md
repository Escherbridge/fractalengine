---
type: Review
track: plugin_host_revival_20261031
gate: C-EXT
round: 4
reviews: [T-EXT, R-EXT-1, R-EXT-2, I-BATCH-1, I-DOC-CORE, I-DOC-API]
checked_at: b817b97e (plus the uncommitted working tree)
date: 2026-11-13
---

# C-EXT: plugin_host_revival_20261031 completeness at round-4 HEAD

Spec phases (§4), acceptance criteria AC-1..AC-8 (§5), open decisions (§6), plan tasks, and this track's round-4
items (EXT-04, EXT-16, EXT-17) checked against `b817b97e`. Nothing was built or run. The turn-3 table
(`completeness-turn3.md`) still holds for every row not repeated here; the rows below are the ones that changed or
that the turn-3 hand-offs touched.

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names a file this track touched this round
(`fe-hexon/src/package.rs`, `fe-hexon/tests/remote_registry_test.rs`, `fractalengine/src/terrain_bridge.rs`,
`fe-terrain/src/tiles/store.rs`, `fe-terrain/src/ruler_plugin.rs`).

## Round-4 items

| Item | Status | Evidence |
|---|---|---|
| EXT-04 (a) AC-6 real-registry plugin journey | complete (unrun) | sealed, signed `HexonType::Plugin` fixture `fe-hexon/tests/remote_registry_test.rs:82-91` (`org.example.fleet-sim`, contract `fractalengine:plugin@2.0.0`, R-EXT-2 R2-06 KEPT); listed by `remote_extension_source` `:241`; renamed copy refused on identity `:254-258`; killed copy refused `:262`; `is_killed` set in both literals `:227,290`. Batch R11-STORE `conductor/test-batches.md:443,450` (`--features remote`). |
| EXT-04 (b) R1 sealed tamper table | complete | `fe-hexon/src/package.rs:1047` journey, plugin table `:1063`, rows flipped component byte / edited extension.json / extra member / removed component / removed extension.json / signature removed `:1207-1228`, control verifies `:1275` (R-EXT-2 SEC-5). |
| EXT-16 QA-0173 failed local install explains itself | complete | `HexonAssembleOutcome::LocalInstallFailed` `fractalengine/src/terrain_bridge.rs:453`, sent by the Install arm `:697`, drained to `ui_mgr.show_error("Could not install {file_name}: {reason}")` `:1123-1128`; readable legacy message `fe-terrain/src/tiles/store.rs:197`; journey asserts exactly one Error notice naming the file only `terrain_bridge.rs:2955-2975` (R4 `test-batches.md:162`); `Time` added to the shared helpers (R-EXT-1 F1); doc `fractalengine/src/AGENTS.md:2230`. |
| EXT-17 QA-0176 ruler under floating windows | complete (in-app check pending) | both painters `egui::Order::Background` `fe-terrain/src/ruler_plugin.rs:222,252` (ordering verified against egui 0.33.3 by R-EXT-1 F10); doc `fe-terrain/src/AGENTS.md:567-568`. The in-app MANIFEST recipe `export1` is a run (step 4). |

## Acceptance criteria (spec §5)

| AC | Status | Evidence |
|---|---|---|
| AC-1 default-off | complete | turn-3 row unchanged (`fe-plugin/src/lifecycle.rs` `require_admission`, `lib.rs:205`). |
| AC-2 terrain in `UiExtensionRegistry` after one tick | complete (unrun) | `fractalengine/src/main.rs:1270-1275` `extension_ui_tests::`; now reached by `conductor/test-batches.md:405` (turn-3 C-PHR-02 closed). |
| AC-3 network honesty | complete | `fe-plugin/src/capability.rs:437`. |
| AC-4 signed packages | complete | `lifecycle.rs` journey table (turn 3). |
| AC-5 consent before Install | complete | `fe-ui/src/extensions/consent.rs` (turn 3); v1 modal marked superseded `fe-ui/src/panels/plugin_install_panel.rs:3`, its journey reached by `test-batches.md:407`. |
| AC-6 install from a real `fe-hexon-registry` | complete (unrun) | EXT-04 (a) above (turn-3 C-PHR-04 closed). The licensed half: row 6 via `extension_host/licence_client.rs` (EXT-01, R11-ENT). |
| AC-7 iot-bridge on one rhai | complete (unbuilt) | turn-3 row unchanged. |
| AC-8 WIT honesty | complete | `fe-plugin/wit/hexon-plugin.wit:1-3` SUPERSEDED; `fractal-plugin-2.wit:1` BOUND. |

Phases P0-P5 and S6: complete as in turn 3. S7 docs: complete; the turn-3 stale lines are fixed at HEAD
(`SECURITY.md:123-126` licensed/pull text current, `conductor/tracks.md:112-116` names the Extensions panel path,
`fe-plugin/src/AGENTS.md` §registry-install header-optional and licensed bullets corrected). One residual stale
clause is handed off in the extension platform file (H4-2 there, `fe-plugin/src/AGENTS.md:336-337`).
OD-1..OD-9: as recorded in turn 3; none reopened.

## Plan tasks

| Task | Status |
|---|---|
| S0-S7 | complete (ticked; see turn-3 table). |
| Sweep: workspace test + in-app consent / opt-in check (`plan.md:104`) | blocked-user (excluded by the scoper; tests only when the user names them). |

## Turn-3 hand-offs (C-PHR-01..05): all reached

- C-PHR-01 `is_killed` literals: fixed, `remote_registry_test.rs:227,290`.
- C-PHR-02 test reach: fixed, `test-batches.md:405-412`.
- C-PHR-03 orphaned v1 consent panel: marked SUPERSEDED, `plugin_install_panel.rs:3`.
- C-PHR-04 AC-6 journey: done by EXT-04.
- C-PHR-05 stale doc lines: fixed (see S7), except the one clause carried as extension platform H4-2.

## Gate hand-offs from this round

R-EXT-1 F1 (Time resource) FIXED in place; F10-F12 KEPT-verified; F11 (disk-error reason can carry a temp path) KEPT
as local-only UI. R-EXT-2 R2-04 batch lines reached (`test-batches.md:443,450`). Nothing outstanding for this track.

## HANDED-OFF (owner = next round)

None for this track. The one residual doc clause is tracked as extension platform H4-2.

## Excluded by the scoper (not gaps)

- blocked-user: Sweep (`plan.md:104`); PHR review #18 / R-LIFE-25 (`compile_cached` has no caller) and `TrustRoots::verify_revocation_list_fresh` with no production caller.
- obsolete: C-PHR handed-off doc lines (SECURITY.md:97-99, conductor/tracks.md:115, registry_source.rs:1860, broker role-source note, test-batches reach lines, v1 consent panel superseded), verified fixed above.

Verdict: COMPLETE (code) - AC-1..AC-8 and EXT-04/16/17 written and batch-reachable; only the user-gated sweep remains.
