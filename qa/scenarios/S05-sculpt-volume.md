---
type: Scenario
id: S05
gold: G5
title: Apply exact raise and lower regions and inspect cut/fill
fixture: fresh terrain with two bound petals
verified_in_pass: "pending S7 build and live validation"
tags: [gold, terrain-tools, sculpt, earthwork, volume, analytic-oracle]
---

# Raise and lower a known flat area and read its volume

Use a fresh `launch.ps1 -Fixture terrain` run, both petals bound to the
descriptor's offline TerrainRGB source at native/world scale 1. S04 must
first establish actual resident terrain at the origin in each petal.
The hill lies away from the oracle; the square[-5,5]x[-5,5] encodes flat
absolute elevation 0m. Runtime grounding subtracts the center-tile mean
(approximately 0.14m), so its local surface can be Y=-mean. Authored Y=0
nodes are not a claim of exact surface seating. Inspect grounding and align
the live setup if needed; constant-depth volume expectations are relative
to the resident base and do not change with this uniform offset.
Value nodes are outside that square, and origin markers are asset-free.

1. Enter Petal A. Press B for Brush, then open Options. Locate controls
   by current names and inspect each checkpoint. Set Radius 5m, Operation
   Raise, positive Delta 2m, Shape Rectangle. Defined shapes use uniform
   delta and no radial falloff; Strength is not part of this oracle.
2. Click **Apply Rectangle region** once. Capture the terrain and region
   node. There must be a visible region/heatmap change over the known flat
   origin area, not merely a new list row or an initial zero-volume node.
3. Keep Brush active and open **Volume report** from the rail or Earthwork
   report link. The selection resolver can use the current rectangle draft.
   Wait for **Computing true cut/fill** to become actual numbers; inspect
   Area/Cut/Fill/Net in the true cut/fill section, not a geometry-only
   report above it. Require units and these displayed two-decimal values:

   | Area | Cut | Fill | Net |
   |---|---|---|---|
   |100.00m²|0.00m³|200.00m³|200.00m³|

4. Capture the displayed report. As independent persistence evidence,
   read `/api/v1/petals/{petal_a_id}/earthwork/summary` and the created
   earthwork region's properties through the API. Wait for asynchronous
   persisted volumes; initial zeros are not completion. Require one new
   region, finite raw values within absolute 0.001 of the expected cut/fill
   and net, and nonzero expected fill 200. Record sample/coverage counts
   where exposed; never invent a count from area alone.
5. Enter the fresh Petal B and establish resident origin terrain. Repeat
   with Radius 5m, Operation **Lower**, positive Delta**1m**, Rectangle and
   one Apply Rectangle region. A negative Delta would invert the intent.
   The independent lower report and persistence must show:

   | Area | Cut | Fill | Net |
   |---|---|---|---|
   |100.00m²|100.00m³|0.00m³|-100.00m³|

   Require one new region and raw absolute tolerance0.001, including nonzero
   expected cut 100. Do not overlap raise 2/lower 1 on the same petal: that
   yields net fill 100 and tests a different operation sequence.

The 128x128 cell-center lattice has 16384 expected centers with step 0.078125m.
The analytic area is 100m²; volume is area times constant depth, with
net=fill-cut. The descriptor records these expected counts and units.
The separate 8m/radius 40m tent hill has theoretical integral 12800m³ but is
only a visual diagnostic because pixel spacing and 0.1m TerrainRGB
quantization affect its sampled integral.

Current hardening gaps must be reported honestly: missing base samples can
be silently skipped and produce a zero-only report, and late tile arrival
does not by itself refresh the report. These are unavailable-state and
refresh defects, not valid zero measurements. If encountered, capture
before/after evidence; once residency is established, changing Radius away
and back to 5 can explicitly request another report. Record that workaround
and any latency; do not silently count it as healthy automatic refresh.
Do not use Copy TSV as readback: synthetic clipboard writes are suppressed.
