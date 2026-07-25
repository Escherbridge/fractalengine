# fe-ui/src/ui_shell — area managers for the egui shell

`ui_shell_architecture_20260724` Phase 2 (FR-4/5/6). The top-level shell is
decomposed into per-area **managers**. Each manager module is: a small state
**Resource** + **pure** decision helpers (unit-tested) + a thin render fn that
takes `&egui::Context`/`&mut egui::Ui`. Managers are **called from**
`panels::gardener_console` in a fixed order — they are NEVER registered as their
own Bevy systems (NFR-4: exactly ONE `EguiPrimaryContextPass` entry,
`gardener_ui_system`).

## Topology

| Manager | File | State resource | Pure helpers | Render fn |
| :-- | :-- | :-- | :-- | :-- |
| Topbar (FR-4) | `topbar.rs` | `TopbarState` (minimal/reserved) | — (delegates to `panels::toolbar`) | `render_topbar` |
| Left (FR-5) | `left_sidebar.rs` | `LeftSidebarState { policy, user_intent }` | `left_visibility` | `render_left_sidebar` |
| Right (FR-6) | `right_sidebar.rs` | `RightSidebarState { requested }` | `active_section`, `section_label`, `toggle`/`is_active` | `render_right_sidebar` |
| Modal (FR-7) | `modal.rs` | `ModalManagerState { disabled_panels, last_error }` | `guarded`, `transient_order`, `resolve_exclusive` | transient layer, rendered last |

Resources are `init_resource`'d in `plugin.rs` and threaded into
`gardener_console` via the `UiShellParams` SystemParam bundle (all `ResMut`, so
downstream slices can mutate without re-touching `gardener_ui_system`).

## §topbar (FR-4)

`render_topbar` is the migrated body of the old `panels::toolbar::top_toolbar`.
The single-source data — `TOOL_DEFS`, `shortcut_hint_line`, `active_tool_hint`,
`stash_active_tool`, and `mode_button_fill` — STAYS in `panels::toolbar`; the
topbar only calls it. The tool-switcher, deselect, and Data/Settings/Maps
buttons are byte-for-byte the same behavior.

**Section-toggle wiring.** The **Tools** button toggles
`RightSidebarSection::Tool` via `right.toggle(Tool)` — the SOLE reveal path
(Phase 4 (FR-9) retired the Phase-2 compat shim + the legacy `tool_panel.open`
flag). Data/Settings/Maps have no section variant and are unchanged.

**Tooltips (FR-8).** Each mode button's hover text comes from
`toolbar::tool_tooltip_text(def)`, which joins `TOOL_DEFS` (glyph/name/shortcut)
with `tool_inspector::panel_descriptor` (title/subtitle/Use guidance) into one
string — so button, shortcut, and description can't drift. The redundant
`ToolDef.tip` one-liner was removed (its single-source duty moved to
`panel_descriptor`).

## §left (FR-5)

The left sidebar's auto-collapse is now a **policy**, not a per-frame stomp. The
pure `left_visibility(policy, right_open, user_intent) -> bool`:

- `AutoCollapse` (DEFAULT) → `!right_open`. This reproduces the pre-refactor
  `sidebar.open = !(portal_is_open() || selected_entity().is_some())` EXACTLY —
  `user_intent` is ignored, matching the old manual-toggle no-op. Do not change
  this default without a ratified decision; the no-op-toggle bug must not return.
- `Manual` → `user_intent` (the seam for a future manual toggle; not default).

`render_left_sidebar` applies the policy to `sidebar.open` **before** rendering
(the old stomp ran after render; the one-frame difference is absorbed by
`show_animated`). `right_open` is computed in `gardener_console` post-topbar
(so a topbar deselect is reflected), exactly as the old stomp was.

## §right (FR-6) — precedence + the section-fn seam

`RightSidebarSection = { Inspector, Tool, PathTools, TerrainTools, ProposalReport }`
is the mutually-exclusive right region — **one section at a time** (RATIFIED
Q-2). `RightSidebarState.requested: Option<RightSidebarSection>` is the explicit
toggle (topbar / in-panel rail); `None` means selection-default.

**`active_section(state, selection_present, portal_open)` precedence —
portal > explicit toggle > selection-default:**

1. `portal_open` → `None`. A true **short-circuit**: the caller renders the
   portal toolbar and there is NO section rail underneath.
2. `state.requested == Some(sec)` → `Some(sec)` (explicit toggle wins over
   selection).
3. otherwise → `Some(Inspector)`. Inspector is the **never-blank** fallback: it
   self-collapses (`show_animated`) when nothing is selected, reproducing
   today's always-call-`right_inspector` behavior. `selection_present` is
   accepted for a future "welcome vs inspector" split but currently both cases
   resolve to Inspector.

`Option<_>` makes **never-double** structural (at most one section). `toggle`
also enforces it at the state level: requesting a new section replaces, never
stacks.

**The section-fn seam (CRITICAL — do not collapse).** There is exactly ONE
render fn per variant; all five are now filled (the one-fn-per-section split is
what kept the P4 and P5 slices conflict-free):

| Section | Fn | Content | Landed by |
| :-- | :-- | :-- | :-- |
| Inspector | `render_inspector_section` | node inspector (`inspector::right_inspector`) | P2 |
| Tool | `render_tool_section` | live readouts `selection_summary` / `gimbal_affordance_label` / `anchor_readout` — read-only host (no `&mut` path/pen state) | P5 |
| PathTools | `render_path_tools_section` | path-asset stamp + pen + shape tools (ex-"Tools" window) | P4 |
| TerrainTools | `render_terrain_tools_section` | 8-mode palette + proposals (ex-"Terrain Tools" window) | P4 |
| ProposalReport | `render_proposal_report_section` | proposal report body (ex-"Proposal Report" window) | P4 |

Migrated tool sections keep their logic + tests verbatim and their authority
rules (path/stamp sections key ONLY on `PathEditorState.editing_track_id`, never
`NodeManager.selected` — NFR-1). Portal-open swaps the whole right region to
`portal_toolbar::right_portal_toolbar` (preserved). Empty states are calm hints,
never blank (`ui_ux.md §7`).

## §modal (FR-7) — transient layer + panel panic guard

`modal.rs` owns the transient layer (dialogs, context menu, toast) and the
panel panic guard. It is CALLED FROM `gardener_console` (never a standalone
system — NFR-4).

**Panic guard.** `guarded(state, name, f)` runs each panel body inside
`catch_unwind(AssertUnwindSafe(f))`:
- **release:** a panic is caught → `tracing::error!` → the panel is marked
  disabled-for-session (`disabled_panels`) → `last_error` set → the frame
  completes. An already-disabled panel is skipped (not re-run).
- **debug (`cfg(debug_assertions)`):** the panic RE-PROPAGATES so developers
  crash loudly (RATIFIED Q-5).
- `AssertUnwindSafe` is sound by construction: `f` is a generic `FnOnce() -> R`,
  so no raw `&mut` egui borrow escapes; the guarded state is exactly what the
  guard quarantines.
- Surfaced UX (Q-5): a persistent status-bar Error-tier chip (`status_bar.rs`
  reads `last_error`), no auto-clear on petal switch.

**⚠ `panic = "abort"` — RATIFIED 2026-07-25: keep abort.** `Cargo.toml
[profile.release]` sets `panic = "abort"`, under which `catch_unwind` is INERT
— the shipped release binary will NOT quarantine a panicking panel. The user
ratified KEEPING `panic = "abort"` (retaining the tuned `lto=fat` /
`codegen-units=1` release profile): the guard is therefore intentionally
**debug/test-only scaffolding** (tests force `unwind`; debug re-propagates by
Q-5 design), and **FR-1 (root-causing the terrain crash) is the primary
remedy**. To make FR-7 shield in-app later, a future decision would flip the
release profile to `panic = "unwind"` (workspace-wide).

**Transient order.** `transient_order()` = Dialog → ContextMenu → Toast (toast
topmost); `TransientVisibility::resolve_exclusive()` keeps at most one dialog
family visible with toast independent. Rendered LAST in the pass so layering is
preserved.

**Guard granularity.** `gardener_console` guards each top-level render call
(topbar/left/right/viewport/each dialog/context-menu/toast/gis). The
right-sidebar guard is coarse — a panic in any one section quarantines all five
for the session (per-section granularity is a noted follow-up, would need the
guard threaded into `right_sidebar.rs`). `status_bar` is deliberately NOT
guarded (it hosts the error chip).
