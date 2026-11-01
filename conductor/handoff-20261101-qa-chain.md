---
type: Handoff
date: 2026-11-01
from_session: 2aa714d7-2e5e-41c5-a829-7bcf912fcf2a
status: paused-by-user
---

# Handoff: agent QA chain (paused 2026-11-01)

The user paused this session ("let's pause here and let another agent take over"). Nothing is running and nothing is pushed.

## State at pause

- **HEAD `425c725`**, all local; pushes need the user's go-ahead. Landed this session, in order:
  - agent_vision runs 1 and 2: vision and input, closed green; status done-pending-user-verify. See
    `conductor/tracks/agent_vision_20261031/metadata.json` `user_verify`.
  - INPUT-ADDENDUM 8 (`b2e67b7`): pinned, **not implemented**. It is folded into qa-surfaces slice A8.
  - qa-harness H1 (`215b12f`, dev-token emission) and H2 (`e7a1c53`, staged isolated launcher + fixture seed).
- **qa-harness was STOPPED mid-H3** (workflow wf_64a59101-caa, task weqg3rs9b). H3's work is on disk, UNCOMMITTED:
  - new: `tools/qa/fe.py`, `.claude/skills/fe-qa-pass/`, `qa/`
  - modified: `tools/qa/launch.ps1`, `tools/qa/AGENTS.md`, `.gitignore`
  - Its live app (PID 68268) has exited. No cargo or app processes are left.
- **Resume option A** (cached replay of H1/H2, re-runs H3 on top of the partial files):
  `Workflow({scriptPath: ".omc/plans/qa-harness.js", resumeFromRunId: "wf_64a59101-caa"})`. This works from the same session
  only. From a new session, use option B.
- **Resume option B** (new session): one agent finishes H3 per the brief in `.omc/plans/qa-harness.js` (H3 section), working from
  the partial files. Then run that script's advocate and critic. Edit the script to skip H1-H3 by passing their SHAs, or run the
  two reviewers as standalone agents with the same prompts.

## Queued chain (scripts staged, LF line endings, not launched)

1. `.omc/plans/qa-harness.js`: finish H3, then advocate, then critic.
2. `.omc/plans/qa-surfaces.js`, run serially. All new surfaces are default-OFF behind INPUT_GATE (`FE_API_INPUT_ALLOW=1`).
   - **A8** clipboard sink.
   - **S1** AccessKit: ON in normal builds for screen readers (user: "turn on AccessKit"); the debug ui-tree surface is gated.
   - **S2** accessible names for every interactive widget.
   - **S3** ui-state + log probes.
   - **S4** FileDialogProvider seam (5 rfd sites) + window controls.
   - **S6** layout policy, from the user's screenshot findings:
     - text overflow with ellipsis + hover;
     - fluid %-based input limits; query editor capped at about 35-40% of the panel height;
     - a protected 3D viewport; Maps must not take over the central area;
     - a query **Results dock** (Blender Spreadsheet / QGIS attribute-table pattern), where a row click selects and frames the node;
     - `fe.py layout-check` at 3 window sizes.
   - **S7** offline terrain fixture with analytically known geometry. The current QA run has NO map installed.
   - **S5** feature catalog `qa/features.md` + full-coverage scenarios.
   - Then advocate, critic, and one full-coverage pass.
3. `.omc/plans/qa-loop.js`: up to `args.max` (default 6) iterations of pass, fix S1/S2, independent review, until convergence.
   Every pass runs layout-check; a terrain volume mismatch is S1.
4. `.omc/plans/ux-audit.js`: five region audits compared against Blender, ZBrush, Civil 3D, QGIS and Unreal, plus a synthesis.
   The synthesis writes `conductor/tracks/ux_audit_20261101/` and appends polish Wave 7+. Launch W7+ with
   `polish-wave.js` `{wave, leaves}`. DATA-MODEL recommendations go to the user.
5. Polish waves 2-6 (`.omc/plans/polish-wave.js {wave:N}`) are still pending. They share the cargo tree with items 1-4, so run them
   serially with those.

## Binding rules for whoever continues

- **Never push** without the user's go-ahead. Never stage `conductor/.conductor_session_log`. Commit trailer:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **TEST POLICY:** no new test fns. Extend or swap journey tests. Run only the touched region batches
  (`conductor/test-batches.md`), and the full suite once, at campaign end.
- **One writer per tree:** never SendMessage an agent inside a running workflow. TaskStop the workflow, confirm the tree is
  quiet, then hand off.
- **Process hygiene:** kill only exact PIDs (`tools/qa/stop.ps1`); never kill by pattern.
- **Workflow scripts must be LF.** CRLF fails the launch with "control characters".
- **Accepted LOW findings:** vision C8-C10 and input C2-C4. See `conductor/tracks/agent_vision_20261031/reviews/`.
- **User in-app checks pending:** screenshot capture (AC-11); input with `FE_API_INPUT_ALLOW=1`; delta-viz P1+P2.
