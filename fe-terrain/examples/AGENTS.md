# Terrain examples

## QA terrain generator

`build_qa_terrain <run-root>` writes only beneath an existing isolated QA run.
It builds nine real 256px TerrainRGB tiles at zoom 17, exports a TerrainTileset,
imports and bundles it, then verifies and assembles the bundles through the
normal HexonStore installation boundary. Registry readback must recover all
nine byte-identical PNGs before a descriptor is published. This local unsigned
fixture does not change network sealing or publisher trust requirements.

The geographic origin is the bounds center, with native/world scale 1 and a
strictly offline configuration serialized through TerrainConfig. The entire
origin oracle rectangle and its interpolation margin are flat 0m. A separate
8m separable tent hill centered at (80,80), radius 40m, gives a visible landmark
away from that rectangle. Its continuous integral is 12800m³; raster resolution,
projection and TerrainRGB 0.1m quantization make it a diagnostic landmark rather
than an exact hill-volume acceptance oracle. PNG sampling includes shared tile
edges. Ground sample distance records the standard tile width / pixel count.

The exact acceptance oracle uses existing defined-shape brush operations on
two fresh petals: rectangle [-5,5]², Raise 2 or Lower with positive Delta 1.
The 128² cell-center integration lattice has 16384 samples, area 100m² and
respective fill 200m³ or cut 100m³. Nonzero expected volumes and residency checks
are mandatory because the current report can silently skip missing samples.
No query/report result generated here proves rendered runtime coverage.
