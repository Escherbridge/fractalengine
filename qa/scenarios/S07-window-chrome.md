---
type: Scenario
id: S07
gold: G7
title: Window chrome — drag, maximize, close-X confirm
fixture: basic
verified_in_pass: "none"
tags: [gold, window, chrome, frameless, lane-b]
---

# S07 — "Move, maximize and close the window"

The app draws its own title bar (frameless window, polish Lane B / T-01):
app icon + "FractalEngine" at the left, minimize / maximize / close at the
right (1141/1195/1248, 20 in a 1280x720 window).

**Success signal (gold G7):** dragging an empty stretch of the top bar
moves the window; maximize fills the screen and restores on a second
click; close-X asks before quitting when there is unsaved/active work (or
quits cleanly when there is none) — never a silent loss.

Synthetic input is window-relative: a top-bar drag that moves the OS window
changes the window, not the image, so judge it with `F camera` (`window`
block) and the run's log, not pixel offsets.

## Steps

1. Maximize and restore.
   ```
   F click 1195 20 --wait 20
   F camera     # window.width/height grow
   F shot --label s07-max
   F click 1195 20 --wait 20
   F camera     # back to 1280x720
   ```
   **Checkpoint 1:** maximized shot fills the bigger window with no
   unpainted strip or stretched UI; the button glyph switches to "restore".
2. Drag the top bar between "Data" and the window buttons.
   ```
   F drag 1110 20 900 200 --wait 20 --capture --label s07-drag
   ```
   **Checkpoint 2:** the window moved (no visual change inside the frame);
   no toolbar button was activated by the drag.
3. Close-X — LAST step, it may end the run.
   ```
   F click 1248 20 --wait 20 --capture --label s07-close
   ```
   **Checkpoint 3:** a confirm dialog, or a clean exit (exit 3 on the next
   call, stderr.log free of panics). An exit without confirmation while a
   proposal/edit is in flight is S2. Afterwards `tools\qa\stop.ps1` reaps
   anything left (the WebView2 children).
4. Minimize is not exercised: a minimized window refuses captures
   (`window_minimized`) and synthetic input cannot restore it.
