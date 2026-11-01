---
type: Scenario
id: S09
title: Choose files and control the real application window
regions: [R6, R9]
fixture: basic
---

# Choose files and control the real application window

Goal: import or save a file through the same controls a user clicks, resize the app, and cancel or confirm closing it.

The QA launcher supplies isolated storage, input opt-in, and a loopback listener. Define `F` as
`python tools/qa/fe.py --run <this-run>` and keep that run fixed through cleanup. Paths below are relative to that run.

1. Open the GLTF import dialog using the active petal's add-object control. Queue an open answer, then click Browse:

   ```powershell
   F dialog-answer fractalengine/assets/samples/info_panel.glb --kind open
   F click --name "Browse..."
   F shot --label s09-browse-open
   ```

   Check that the file path and suggested display name are filled without a native dialog or stalled frame loop.
   Import it and verify the new node appears in the current petal, then close the dialog.

2. Open the same import dialog again and queue cancellation:

   ```powershell
   F dialog-answer --cancel --kind open
   F click --name "Browse..."
   F shot --label s09-browse-cancel
   ```

   Check that cancellation preserves the prior form and the app remains responsive. A Browse click with no queued
   answer must show `dialog_answer_missing`; a queued save answer consumed by Browse must show
   `dialog_answer_kind_mismatch`. Neither case opens a native picker.

3. Queue a save answer before the selected node's download/export action:

   ```powershell
   F dialog-answer exported-model.glb --kind save
   # Click the selected node's Download action using the current UI tree.
   F shot --label s09-save
   ```

   Verify the saved file exists inside the run and matches the asset bytes. Exercise GPX export with the same
   save-answer pattern. Input leases must end with their read operation so a later export can overwrite that input path.

4. Reject answers outside the run, alternate streams, drive-relative names, reserved Windows devices, and reparse escapes.
   Each command must return a typed refusal and preserve a previously queued valid answer:

   ```powershell
   F dialog-answer ../escape.glb --kind save
   F dialog-answer model.glb:stream --kind save
   F dialog-answer C:relative.glb --kind save
   F dialog-answer NUL.txt --kind save
   F dialog-answer COM¹ --kind save
   ```

5. Resize and inspect at every size, preserving honest layout findings for S6:

   ```powershell
   F window size 1280 720
   F shot --label s09-1280
   F window size 1920 1080
   F shot --label s09-1920
   F window size 1024 640
   F shot --label s09-1024
   F window maximize
   F window restore
   F window minimize
   F window restore
   ```

   Check that state reports the actual requested physical dimensions and restore remains callable while minimized.

6. Exercise a named control at app UI scale 1.25 and restore 1.0:

   ```powershell
   F window ui-scale 1.25
   F click --name "Maps"
   F shot --label s09-app-scale-125
   F window ui-scale 1.0
   ```

   Check actual `pixels_per_point` and the visible click effect. This tests app UI scaling, not physical monitor DPI.

7. Close safely, as the final scenario in the run:

   ```powershell
   F window close
   F wait-for --name "Cancel close"
   F shot --label s09-close-confirmation
   F click --name "Cancel close"
   F state
   F window close
   F click --name "Close FractalEngine"
   ```

   The first request must keep the process alive and show confirmation. Cancel preserves it; confirming ends the run.
   Always call `stop.ps1 -Run <owned-run>` afterward to verify or clean recorded descendant identities.
