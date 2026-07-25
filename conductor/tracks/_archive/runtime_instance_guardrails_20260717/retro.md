---
type: Track Retro
title: Retro — runtime_instance_guardrails (GPU-OOM crash fix + spawn caps)
tags: [retro, stability, runtime_instance_guardrails_20260717]
timestamp: 2026-07-24T00:00:00Z
resource: ./metadata.json
---

# Retro — runtime_instance_guardrails_20260717

Archived 2026-07-24, status **done**.

## 1. What happened

The 2026-07-17 user-reported GPU-OOM crash (`create_bind_group` panic — buffer
binding 6 range 2,758,820,944 bytes exceeding the 2 GiB
`max_*_buffer_binding_size` limit, then host allocation failure →
STATUS_STACK_BUFFER_OVERRUN) was root-caused and fixed the same day: the
duck.glb's **embedded camera** was stamped 8,192 times, producing 16,387
extracted views and ~2.76 GB of per-view GPU data.

## 2. Verification evidence

- **Root-cause fix committed @ `46e19de`** — glb-embedded-camera deactivation
  (`deactivate_foreign_cameras`) in `fe-renderer/src/camera.rs`. In-commit
  verification: **309s run at full frame rate, 0 validation errors, views max 3,
  data_buffer ~1.5 MB** vs the 2.76 GB failure — satisfying FR-5's primary
  acceptance gate (app launches against the user's existing `data/` without the
  panic).
- **FR-1..4 caps/watchdog landed** — `MeshInstanceBudget` + `MAX_PETAL_NODES`
  wired across `fe-ui/src/verse_manager/{spawn,petal_respawn,primitive_materialize,db_results/*}.rs`
  and `fe-ui/src/plugin.rs`/`settings.rs`.
- **FR-6 render horizon delivered CROSS-TRACK** by p2p_asset_streaming FR-4
  @ `320ebfe`: `residency.settings.render_distance` is consumed in
  `path_asset_materialize.rs:265`, `petal_respawn.rs:100`,
  `primitive_materialize.rs:151`. FR-6's dense-petal live-run acceptance bar
  transfers to that delivery with it.
- **Two subsequent live user runs (2026-07-19 and 2026-07-23/24) with no
  recurrence** of the create_bind_group / instance-blowup crash.

## 3. Boundary note

The **2026-07-24 crash is a DIFFERENT crash**: a panic in
`fe_ui::plugin::gardener_ui_system` (terrain tools), followed by a bevy_egui
`run_egui_context_pass_loop_system` panic and abort. It is **owned by
`ui_shell_architecture_20260724`**, not a recurrence of this track's failure
mode.

## 4. Carried-forward

- FR-6 live-run verification at dense-petal scale rides p2p_asset_streaming's
  residency ledger (its acceptance surface now).
- The instance-count guardrails are load-bearing for any future bulk-stamping
  feature — build on `MeshInstanceBudget`, do not bypass it.
