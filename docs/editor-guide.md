# FractalEngine Editor Guide

How to use the in-app editor tools. This guide covers drawing and editing
paths with the Pen tool. For build and architecture documentation, see the
[developer guide](guide.md).

All distances in the editor are petal-local meters.

---

## Drawing paths with the Pen tool

Select the **Pen** tool from the top toolbar (hotkey **P**). Clicking the
terrain adds points to the path you are editing; if no path is being edited,
the first click creates a new path in the active petal and starts editing it.

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
places is configurable — see [Pen defaults](#pen-defaults-tools-window).

### Anchor types

| Type | Behavior |
|---|---|
| **Corner** | Handles (if any) are independent. Sharp by default. |
| **Smooth** | Both handles stay aligned in one direction through the anchor; their lengths can differ. |
| **Symmetric** | Both handles mirror exactly — same length, opposite direction. |

---

## Editing curves in the viewport

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

## Corner settings (Paths tab)

Open the **Paths** tab of the Data window, edit a path, and select a vertex.
A **Corner settings** card appears with:

- **Corner / Smooth / Symmetric** toggle — reclassifies the selected anchor.
  Choosing Smooth or Symmetric derives a matched pair of handles from the
  neighboring points.
- **Smoothness slider (0–1)** — the one-knob version of curve editing:
  0 is a sharp corner, 1 is as round as the neighboring spacing allows. The
  slider derives both handles for you and previews live in the viewport.

**Why the slider sometimes greys out.** Once you drag handles by hand into an
asymmetric shape, no single 0–1 number describes the anchor anymore — a live
slider would overwrite your handle work. So the slider disables, showing an
approximate readback and each handle's length instead. To get it back, click
**Smooth** or **Symmetric** on the toggle: that re-derives a clean symmetric
pair (replacing the hand-edited handles) and re-enables the slider.

---

## Pen defaults (Tools window)

The **Pen** section of the Tools window sets tool-level defaults:

- **New anchor** — Corner / Smooth / Symmetric: what a plain click places.
  Corner gives the classic sharp polyline point; Smooth or Symmetric gives
  every clicked anchor auto-derived rounded handles, no dragging needed.
- The curve mode, sensitivity slider, **Smooth path** button, and shape
  buttons still act on the whole path at once — bulk smoothing an existing
  path remains available alongside per-anchor editing.

---

## Compatibility

Existing straight paths are untouched: they load, render, and export exactly
as before, and stay straight until you add handles. Curves are saved with the
path and survive save and reload. Straight and curved anchors mix freely on
one path.
