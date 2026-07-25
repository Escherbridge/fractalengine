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

`modal` is intentionally NOT declared here — a sibling slice adds it later.

Resources are `init_resource`'d in `plugin.rs` and threaded into
`gardener_console` via the `UiShellParams` SystemParam bundle (all `ResMut`, so
downstream slices can mutate without re-touching `gardener_ui_system`).

## §topbar (FR-4)

`render_topbar` is the migrated body of the old `panels::toolbar::top_toolbar`.
The single-source data — `TOOL_DEFS`, `shortcut_hint_line`, `active_tool_hint`,
`stash_active_tool`, and `mode_button_fill` — STAYS in `panels::toolbar`; the
topbar only calls it. The tool-switcher, deselect, and Data/Settings/Maps
buttons are byte-for-byte the same behavior.

**Section-toggle wiring.** The only current topbar button with a matching
`RightSidebarSection` variant is **Tools** → `RightSidebarSection::Tool`. It now
calls `right.toggle(Tool)`, then a **COMPAT SHIM (Phase 2)** mirrors the legacy
`tool_panel.open` flag from `right.is_active(Tool)` so the still-floating Tools
window keeps working until Phase 4 removes floating windows. Data/Settings/Maps
have no section variant and are unchanged.

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
render fn per variant:

| Section | Fn | This phase | Owner |
| :-- | :-- | :-- | :-- |
| Inspector | `render_inspector_section` | hosts the moved `inspector::right_inspector` call | (P2) |
| Tool | `render_tool_section` | calm placeholder | **P5** |
| PathTools | `render_path_tools_section` | calm placeholder | **P4** |
| TerrainTools | `render_terrain_tools_section` | calm placeholder | **P4** |
| ProposalReport | `render_proposal_report_section` | calm placeholder | **P4** |

Downstream slices fill their own fn's body and MUST NOT merge fns together — the
one-fn-per-section split is what keeps P4 and P5 conflict-free. The four
placeholders share `section_placeholder` (rail + a single calm hint line, never
blank — `ui_ux.md §7`); replace the whole fn body when filling. The Inspector
section delegates to `inspector::right_inspector` (which still owns its own
SidePanel + self-collapse); Phase 4 folds that SidePanel under this manager.
Portal-open swaps the whole right region to `portal_toolbar::right_portal_toolbar`
(preserved).
