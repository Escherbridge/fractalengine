//! UI shell area managers (ui_shell_architecture_20260724 Phase 2). The
//! top-level egui shell decomposed into per-area managers; each = a small state
//! resource + pure decision helpers + a thin render fn, all CALLED FROM
//! `panels::gardener_console` (never registered as standalone Bevy systems —
//! NFR-4). See `fe-ui/src/ui_shell/AGENTS.md` for the manager topology, the
//! `active_section` precedence rule, and the section-fn seam contract.

pub mod left_sidebar;
pub mod right_sidebar;
pub mod topbar;
pub(crate) mod modal;
