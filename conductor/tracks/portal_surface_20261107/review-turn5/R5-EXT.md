---
type: Review
---

# R5-EXT review (EXT5-02, EXT5-03, EXT5-05, EXT5-06)

F1 KEPT (correct as written) - fe-webview/src/portal_bundle/ipc.rs:381 - `is_visible` reads `self.lock().visible`, the same field `set_visible` writes; honest.
F2 KEPT (correct as written) - fe-webview/src/portal_bundle/qa_rows.rs:124-127 - Hidden branch captures `was_visible` before hiding and restores it; doc comment at :78 updated.
F3 KEPT (correct as written) - fe-webview/tests/portal_ipc_journey.rs:599-606 - the sender-gate row loop hides first, asserts `!is_visible()` after the Hidden row, then re-shows; goes red if the restore is a constant true; extends an existing journey, reached by batch R6 (no batch change).
F4 KEPT (reason: placeholder, same as open_dash) - tools/qa/sim/scenarios/portal-core.json:448 - `open_act` uses the accessible-name placeholder 'Open bundle' and relies on the last-created repository node (`ract`) being the one opened; this cannot be proven without a run, so the first sim run must confirm the click opens the Actions node (QA line below).
F5 KEPT (correct as written) - tools/qa/sim/scenarios/portal-core.json:438-510 - r_unsigned/r_external stay before `open_act`; r_frame, r_hidden, r_stale, r_control all need `open_act`; `python tools/qa/sim/run.py validate portal-core` (read-only validation) reported OK in the first pass (not re-runnable in this re-check: the sim needs pandas, absent here), 6 steps, 48 actions, 23 expectations.
F6 KEPT (re-checked: already landed, fe-plugin/src/AGENTS.md:333-338 now reads 'Retired v1 layout' and names the live RegistrySource path) - was: the stale "moves to fe-hexon ... when extension_platform E9/E13 land" clause is still present; read_member is still used (fe-plugin/src/registry_client.rs:277,289), so the text must say "retired v1 layout" and name fe-hexon/src/registry_source.rs as the live path. docs/extensions/ itself carries no such clause (grep clean).
F7 KEPT (correct as written) - docs/extensions/extensions-ui.md:3 - home-path reference replaced by "the project UI/UX guidance"; grep of docs/extensions for absolute home paths is clean.
F8 KEPT (correct as written) - tools/qa/sim/gpx_upload.py:11-26 - multipart POST to /api/v1/petals/{id}/import/gpx with form part `file` (the field fe-api/src/gpx.rs reads), boundary `fe-sim-<uuid>`, same verse_token/send calls as Client.upload_multipart, token_source set; relative paths resolve against tools/qa/sim.
F9 KEPT (correct as written) - tools/qa/sim/driver.py:477, scenario.py:16, analyze.py:27 - `upload_gpx` registered in driver dispatch, scenario VIAS and WRITE_REST_OPS; scenario.schema.json `op` is a free string, so no schema edit is needed.
F10 KEPT (correct as written) - tools/qa/sim/scenarios/fleet-terrain.json:114-145 - gpx_a/b/c use rest upload_gpx with assets/roads/road-{a,b,c}.gpx (files exist); later steps read the tracks through lookup_node by name, so no binding depends on the reply envelope; description sentence about the missing op dropped, terrain-height gap sentence kept; `run.py validate fleet-terrain` OK (4 steps, 23 actions, 17 expectations).
F11 KEPT (user default: defer) - fleet-terrain terrain-height and bounds probe stays deferred until the first run shows movers leaving the tileset.
F12 KEPT (user defaults) - EXT-03 package-id list queries stay capped and documented; verify_revocation_list_fresh left as is (not in this slice's files).

## Completeness

EXT5-02 COMPLETE fe-webview/src/portal_bundle/ipc.rs:381, fe-webview/src/portal_bundle/qa_rows.rs:124, fe-webview/tests/portal_ipc_journey.rs:599
EXT5-03 COMPLETE tools/qa/sim/scenarios/portal-core.json:448 (open_act) and :460-510 (needs); node selection is unverified until a sim run
EXT5-05 COMPLETE docs/extensions/extensions-ui.md:3 (home path gone); the fe-plugin/src/AGENTS.md clause is routed to I5-DOC-TOOLS as a NEEDS line, not part of this file's ownership
EXT5-06 COMPLETE tools/qa/sim/gpx_upload.py:11, tools/qa/sim/driver.py:477, tools/qa/sim/scenarios/fleet-terrain.json:114

SEC-INBOX: no entry in .omc/state/turn4-security-inbox.md names these owned files; nothing to confirm.

QA sim: run `python tools/qa/sim/run.py portal-core` step 2 -> r_frame/r_hidden/r_stale/r_control must not return 409 portal_fixture_mount_mismatch (the Actions bundle is open when they pin it).
QA sim: run fleet-terrain step 1 -> upload_gpx for gpx_a/b/c returns success and lookup_node finds Road A, Road B and Road C.

Verdict: PASS
