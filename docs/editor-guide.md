# FractalEngine Editor Guide

How to use the in-app editor tools. This guide covers drawing and editing
paths with the Pen tool. For build and architecture documentation, see the
[developer guide](guide.md).

Gesture distances (such as the 0.15 m click threshold below) are petal-local
meters. Lists and readouts convert through the active petal map's scale, and
label values "world units" when the petal has no map.

---

## Drawing paths with the Pen tool

Select the **Pen** tool from the top toolbar (hotkey **P**); the **Options**
panel in the right sidebar opens with the Pen settings described below.
Clicking the terrain adds points to the path you are editing; if no path is
being edited, the first click creates a new path in the active petal and starts
editing it. To edit an existing path instead, pick it in the Options panel's
path list.

The Pen is a vector curve tool: every point (anchor) on a path can carry a
pair of curve handles, and the gesture you use to place it decides its shape.

### Placing anchors: click, drag, and Alt

| Gesture | Result |
|---|---|
| **Click** (release without dragging) | A sharp corner — no handles, straight segments in and out. |
| **Press and drag** | A smooth anchor with symmetric handles: the leading handle follows your cursor and the trailing handle mirrors it live, curving the segment you just drew. Drag further for a rounder curve. |
| **Hold Alt mid-drag** | A combination anchor. The trailing handle freezes the moment you press Alt — the curve behind the anchor keeps its shape — while the leading handle keeps following your cursor, so the next segment can head off in an independent direction. |
| **Alt held from the press** | Leading handle only. The segment arriving at the anchor stays straight; only the outgoing segment curves. |

A drag shorter than about 0.15 m counts as a click. What a plain click
places is configurable — see [Pen defaults](#pen-defaults-options-panel).

### Anchor types

| Type | Behavior |
|---|---|
| **Corner** | Handles (if any) are independent. Sharp by default. |
| **Smooth** | Both handles stay aligned in one direction through the anchor; their lengths can differ. |
| **Symmetric** | Both handles mirror exactly — same length, opposite direction. |

---

## Editing curves in the viewport

<!-- screenshot: pen-curve-handles — Close-up of a path being edited with the Pen tool: yellow anchor markers, cyan handle points joined to their anchors by stems, and one smoothly curved segment. -->

While a path is being edited, its anchors show as yellow point markers, and
any anchor that carries curve handles also shows cyan handle points connected
to the anchor by a stem — drag them directly:

- **Symmetric** anchor: dragging one handle mirrors the other exactly.
- **Smooth** anchor: the opposite handle keeps its own length but rotates to
  stay aligned.
- **Corner** anchor: each handle moves independently.
- **Alt while dragging a handle** breaks the pair apart: the opposite handle
  freezes where it is and the anchor becomes a Corner.

Handle points always win the click when they overlap an anchor marker or its
move gimbal, so a handle sitting on top of its anchor is still grabbable.
Dragging the anchor itself moves the anchor and its handles together.

---

## Corner settings (Options panel)

<!-- screenshot: pen-options-corner-settings — Right sidebar Options panel with the Pen tool active: the Pen section (curve mode, New anchor, sensitivity) and the "Corner settings — point N" card for a selected point. -->

With the Pen tool active, pick a path in the **Options** panel, then select a
point — in the panel's point list, or by clicking its marker in the viewport.
A **Corner settings — point N** card appears in the Options panel with:

- **Corner / Smooth / Symmetric** toggle — reclassifies the selected anchor.
  On an anchor without handles, choosing Smooth or Symmetric derives a matched
  pair from the neighboring points; on an anchor whose handles are already
  aligned, it only changes the type and leaves your handles alone.
- **Smoothness slider (0–1)** — the one-knob version of curve editing:
  0 is a sharp corner, 1 is as round as the neighboring spacing allows. The
  slider derives both handles for you, previews live in the viewport, and
  saves when you release it. Setting it to 0 removes the handles and returns
  the anchor to Corner. A **Handle length** readout sits beneath it.

The **Paths** tab of the Data window still lists your paths and their points
and starts or stops editing; it points you here for corner settings. Right-click
a point in the viewport for a quick **Corner / Smooth** action that cycles the
type Corner, Smooth, Symmetric.

**Why the slider sometimes greys out.** Once you drag handles by hand into an
asymmetric shape, no single 0–1 number describes the anchor anymore — a live
slider would overwrite your handle work. So the slider disables, showing an
approximate readback and each handle's length instead. To get it back, click
**Smooth** or **Symmetric** on the toggle: that re-derives a clean symmetric
pair (replacing the hand-edited handles) and re-enables the slider.

---

## Pen defaults (Options panel)

The **Pen** section of the Options panel sets tool-level defaults:

- **New anchor** — Corner / Smooth / Symmetric: what a plain click places.
  Corner gives the classic sharp polyline point; Smooth or Symmetric gives
  every clicked anchor auto-derived rounded handles, no dragging needed.
- The curve mode (Polyline, Catmull-Rom or Bezier), sensitivity slider,
  samples per segment, and **Smooth path** button still act on the whole path
  at once — bulk smoothing an existing path remains available alongside
  per-anchor editing. **Smooth path** is disabled in Polyline mode.
- The **Shapes** controls (Radius X, Radius Z, Segments) with **Add Ellipse**,
  **Add Circle** and **Add Rectangle** append a ring of points to the edited
  path at the origin; drag the points to reposition them.

---

## Undo and redo

**Ctrl/Cmd+Z** undoes and **Ctrl/Cmd+Shift+Z** redoes the last confirmed
transform or single-point move, up to 50 edits per session; the stack starts
empty after a restart or when you navigate to another verse, fractal or petal.
Only transforms and single-point moves are covered; adding, inserting or
removing points, handle edits, and whole-path and segment moves are not.

---

## Compatibility

Existing straight paths are untouched: they load, render, and export exactly
as before, and stay straight until you add handles. Curves are saved with the
path and survive save and reload. Straight and curved anchors mix freely on
one path.
