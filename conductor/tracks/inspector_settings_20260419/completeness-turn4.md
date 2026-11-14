---
type: Review
track: inspector_settings_20260419
critic: C-UI
round: 4
head: 50c50d29
date: 2026-11-13
---

# C-UI completeness: inspector_settings_20260419 (round 4)

Checked at HEAD `50c50d29` against spec.md FR-1..FR-4, NFR-1..NFR-3 and plan.md Phases 1-4, plus round-4 items UI-06, UI-07, UI-08 and the folded EXT-18. Gates read: `review-turn4/R-UI-3.md` (security review done, PASS-WITH-FIXES), `terrain_proposal_delta_viz_20261025/review-turn4/R-UI-2.md` (UI-06), `project_completion_20261104/review-turn4/R-INT-3.md` (I-UI-1 wiring). No cargo was run.

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names a file owned by this critic or a file of this track's round-4 items; nothing to close.

## Requirements and tasks

| Item | Status | Evidence |
|---|---|---|
| FR-1 / Phase 1 (Tasks 1.1-1.6) URL persistence | complete (carried) | Ticked 2026-07-15; `SaveUrlOutcome` in `fe-ui/src/actions/portal.rs`. |
| FR-2 tab system (as-built fold) | complete (carried) | Phase 2 verification ticked 2026-07-15 (spec FR-2 as-built). |
| Task 3.1/3.2 `InspectedEntity` + selection routing (UI-06) | complete | `fe-ui/src/panels/hierarchy_info.rs:27` enum, `:47` `inspected_entity` (node wins, override, deepest level), `:71-120` `InspectedOverride`; sidebar re-click `fe-ui/src/panels/sidebar.rs:22-31`, called at `:278`, `:411`, `:573`; refreshed per frame `fe-ui/src/panels/inspector.rs:139-158`. Journey tests `hierarchy_info.rs:265-323` (R3 `panels::hierarchy_info`). Tick not applied by T-UI. |
| Task 3.3 petal info (name, ID, node count, creation date) | partial | `hierarchy_info.rs:168-181` name, ID, node count; no creation date. |
| Task 3.4 fractal info (name, ID, petal count) | complete | `hierarchy_info.rs:156-167`. |
| Task 3.5 verse info (name, ID, fractal count, connected peer count) | partial | `hierarchy_info.rs:144-155`; the count is all known peers (`peers.len()`, label "known peer"), not connected peers. |
| Task 3.6 header shows entity type | partial | Card header "Verse: / Fractal: / Petal: <name>" `hierarchy_info.rs:147,160,174`, rendered `:203`; the node form has no "Node: <name>" header (only "Node ID", `inspector.rs:434`). |
| Task 3.7 tab-state reset on kind change | complete | `inspector.rs:150-156` resets per-tab secrets on `kind_changed` (`hierarchy_info.rs:112`); active tab persists by the FR-2 fold. Placeholder half is obsolete (below). |
| FR-3 General: higher-level selection does not deselect the node | partial (deviation) | Re-click deselects the node to show the card (`sidebar.rs:28` `node_mgr.deselect()`); the inspector does show the most recently clicked entity. |
| FR-3 "Your role" on a pinned verse/fractal card | partial | `hierarchy_info.rs:218` shows `LocalUserRole` (deepest active scope), so a verse card can show a petal-only grant (R-INT-3 F2 HANDED-OFF to the UI-06 owner, not fixed). |
| Task 4.1/4.2 access state with explicit + effective roles, online, sort | complete | As built in `fe-ui/src/dialogs/entity_access.rs` (`EntityAccessState`, snapshot from `fe-runtime/src/entity_access.rs:51-56` incl. `grants`); "Granted here" / "Blocked here" / inherited `entity_access.rs:488-490`, `inherited_label` `:519-525`; online dot `:479-481`, `:566`; owner first `:528`. |
| FR-4 owner row "Owner", no dropdown | complete | `entity_access.rs:483-486`. |
| FR-4 `none` blocks inherited access | complete | Owner pin `entity_access.rs:497-499`; "Block access here..." `:505`. |
| FR-4 inherited label names its source level | partial | Petal scope reads "Inherited from fractal or verse" (`entity_access.rs:524`), not the specific level. |
| Task 4.6 invite link UI | complete (carried) | `entity_access.rs:704-800`; expiry is a 1-720 h field, not the 1h/24h/7d/30d/never list (accepted as built). |
| EXT-18 / §9 q20 Genesis verse local-only reason | partial | Reason and disabled Generate invite built (`entity_access.rs:55`, `:763-773`); `mark_verse_local_only` (`:169`) has no caller, so it never shows (R-UI-3-06 HANDED-OFF to I-UI-1 did not land). The doc already claims it works (`fe-ui/src/AGENTS.md` "Access tab local-only verses"). |
| Task 4.7 presets (UI-08) | complete (as re-scoped) | `entity_access.rs:380-438` `preset_controls`; send + gated default + revoke start `fe-ui/src/dialogs/entity_settings.rs:377-400`; sequential revokes `entity_access.rs:90-127`, `:250`, `:360-371`. Public / Members only set the verse default (verse scope only), not per-scope roles as FR-4 first wrote. Journey `role_confirmation_refresh_journey` `:976` via `actions::tests::active_dialog_entity_settings` (R6). |
| §9 q32 default "Managers only" = default none + sequential revokes behind one confirm | complete | Confirm label `entity_access.rs:415-419`; Owner revokes strongest live viewer/editor grant, Manager only its own (R-UI-3-01). |
| Task 4.8 role audit trail (UI-07) | complete (as re-scoped) | op_log entry carries scope: `fe-database/src/role_manager.rs:183-194`; "Grant record (N)" collapsible `entity_access.rs:582-616` from `fe-database/src/handlers/entity_access.rs:189` (`role_grants::load_scope_statements`, `role_grants.rs:484`). A "History" of op_log entries was replaced by the latest statement per (peer, grantor). |
| Task 4.9 loading / error / reactive | complete | Spinners `entity_access.rs:454`, `:634`, `:779`; unconfirmed-write copy `:369-371`; Refresh `:447`. |
| §9 q31 default Access stays in the Entity Settings dialog, linked from the card | complete | Card "Settings..." `hierarchy_info.rs:222-241` opens `dialogs/entity_details.rs:106-121` (opens on General; Access is one tab away, `entity_settings.rs:112-124`). The link is Editor-gated (`hierarchy_info.rs:186-188`), so a Viewer cannot reach the read-only member list from the card. |
| NFR-1 no per-frame DB queries | complete | Card reads the in-memory tree (`hierarchy_info.rs:139-181`); peers mirrored on change only (`inspector.rs:160-167`). |

## Excluded by the scoper (not gaps)

- blocked-user: Phase 1/3/4 manual verification checkpoints (plan.md:58, 133, 217).
- obsolete: Task 3.7 placeholder text for Settings/Access per hierarchy level.
- obsolete: Tasks 4.3/4.4/4.5 as an inspector Access tab (`InspectorTab::Access`); built as the Access tab of the Entity Settings dialog (`entity_settings.rs:112-124`, `:377-404`).

## HANDED-OFF (owner = next round)

- HANDED-OFF (I-UI-1 next round, owner of `fe-ui/src/panels/mod.rs`): wire EXT-18 with R-UI-3-06's exact line in the `dialog_entity_settings` closure: `mark_verse_local_only(&mut ui_mgr.active_dialog, |verse_id| hierarchy.verses.iter().find(|v| v.id == verse_id).map(|v| v.namespace_id.is_none()));`.
- HANDED-OFF (I-DOC-UI next round): R-UI-3-08 did not land; `fe-ui/src/dialogs/AGENTS.md` needs the access-record and presets section (Granted here / Inherited / Blocked here, latest-statement-per-grantor record, three presets, sequential Managers only, owner vs manager revoke reach).
- HANDED-OFF (UI-06 owner next round): R-INT-3 F2, resolve "Your role" at the card's own scope, not the deepest active scope.
- HANDED-OFF (UI-06 owner next round): petal creation date (Task 3.3), connected-peer count on the verse card (Task 3.5), and a "Node: <name>" header for the node form (Task 3.6).
- HANDED-OFF (UI next round, low): name the inherited level ("from fractal" / "from verse") at petal scope (`entity_access.rs:524`); let a Viewer open the read-only Access list from the card.
- HANDED-OFF (CLOCK-04 owner next round): R-UI-3-09, bound `load_scope_statements` (`role_grants.rs:484`) with `ORDER BY hlc DESC LIMIT 257`.
- HANDED-OFF (T-UI next round): tick Tasks 3.1, 3.2, 3.4, 3.7, 4.1, 4.2, 4.7, 4.8, 4.9 (code verified above); mark 4.3-4.5 superseded; leave 3.3, 3.5, 3.6 open; update `conductor/tracks.md:459` ("blocked_on_ratification") via I-CONDUCTOR.

Verdict: PARTIAL - FR-3 card and FR-4 Access (explicit vs inherited, grant record, presets) are in code; EXT-18 is unwired, three info-card fields are missing, and none of the satisfied Phase 3/4 boxes are ticked yet.
