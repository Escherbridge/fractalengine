---
type: evidence
title: "Evidence pack: hexon size histograms, prior-art digest, double-residency math (Phase 0 / FR-2)"
tags: [p2p_asset_streaming_20260718, evidence, hexon, residency]
timestamp: 2026-10-11T00:00:00Z
resource: ./plan.md
---

# Evidence pack — P2P Asset Streaming, Phase 0

Measured at HEAD `49e2de0` on 2026-10-11. Reproduction script is in the
appendix (zip-level walk, no product code). Feeds risk #8 and D-74 "mandated
prior art" in `conductor/decisions/p2p-streaming-20260718.md`.

## 1. Corpus

Every `.hexon` on this machine that the app or the ETL produces:

| hexon | archive KiB | members | tile bytes KiB | non-tile KiB |
|---|---:|---:|---:|---:|
| gis-tile-etl `japan-mount-fuji` | 821.1 | 20 | 817.7 | 1.3 |
| gis-tile-etl `switzerland-zurich-alps` | 1445.1 | 24 | 1441.2 | 1.3 |
| gis-tile-etl `north-america-pacific-northwest` | 7503.4 | 144 | 7484.5 | 1.1 |
| sample `alpine-demo-terrain` | 66.0 | 28 | 61.7 | 1.0 |
| sample `lakeside-scene` (entity hexon, no tiles) | 2.3 | 6 | 0.0 | 5.8 |
| sample `morning-run-gpx` (entity hexon, no tiles) | 2.2 | 5 | 0.0 | 5.2 |
| installed `%LOCALAPPDATA%/fractalengine/hexons/tilesets/tileset-north-america-pacific-northwest` | 7503.4 | 144 | 7484.5 | 1.1 |

Layout confirmed against `fe-terrain/src/tiles/hexon_source.rs`: elevation at
`terrain/tiles/{z}/{x}/{y}.png` (terrain-RGB PNG), satellite at
`terrain/satellite/{z}/{x}/{y}.jpg`; tiles are zip **Stored** (method 0), JSON
members are Deflated (method 8) — so "non-tile KiB" is uncompressed size and can
exceed the archive size for entity hexons. The installed PNW tileset is
byte-identical to the ETL output (same member sizes); it is excluded from the
pooled stats below to avoid double counting.

## 2. Per-(hexon, kind, zoom) size distribution

Nearest-rank percentiles, bytes.

| hexon | kind | z | count | total KiB | p50 | p90 | p99 | max |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| japan-mount-fuji | elevation | 8 | 2 | 213.2 | 121 606 | 121 606 | 121 606 | 121 606 |
| japan-mount-fuji | elevation | 9 | 2 | 190.0 | 100 739 | 100 739 | 100 739 | 100 739 |
| japan-mount-fuji | elevation | 10 | 4 | 283.2 | 70 740 | 78 997 | 78 997 | 78 997 |
| japan-mount-fuji | satellite | 8 | 2 | 33.1 | 18 939 | 18 939 | 18 939 | 18 939 |
| japan-mount-fuji | satellite | 9 | 2 | 33.3 | 18 379 | 18 379 | 18 379 | 18 379 |
| japan-mount-fuji | satellite | 10 | 4 | 64.8 | 16 519 | 17 353 | 17 353 | 17 353 |
| switzerland-zurich-alps | elevation | 8 | 2 | 162.8 | 84 251 | 84 251 | 84 251 | 84 251 |
| switzerland-zurich-alps | elevation | 9 | 2 | 281.2 | 144 815 | 144 815 | 144 815 | 144 815 |
| switzerland-zurich-alps | elevation | 10 | 6 | 795.3 | 135 234 | 141 480 | 141 480 | 141 480 |
| switzerland-zurich-alps | satellite | 8 | 2 | 41.5 | 21 617 | 21 617 | 21 617 | 21 617 |
| switzerland-zurich-alps | satellite | 9 | 2 | 42.0 | 22 047 | 22 047 | 22 047 | 22 047 |
| switzerland-zurich-alps | satellite | 10 | 6 | 118.4 | 21 043 | 21 292 | 21 292 | 21 292 |
| north-america-pacific-northwest | elevation | 8 | 70 | 6310.7 | 92 515 | 112 563 | 124 134 | 124 134 |
| north-america-pacific-northwest | satellite | 8 | 70 | 1173.8 | 16 718 | 23 408 | 29 729 | 29 729 |
| alpine-demo-terrain (synthetic) | elevation | 10–12 | 4 each | 17.7 each | 4 527 | 4 528 | 4 528 | 4 528 |
| alpine-demo-terrain (synthetic) | satellite | 10–12 | 4 each | 2.9 each | 740 | 740 | 740 | 740 |
| every hexon | other (manifest/entries/license/meta/entities) | – | 4–6 | ≤ 5.8 | 50–794 | ≤ 3 685 | ≤ 3 685 | 3 685 |

Pooled over the real (ETL) tiles only — 176 tiles: elevation p50 ≈ 92–100 KB,
satellite p50 ≈ 17–21 KB; a co-located **(elevation + satellite) pair is
~110 KB at p50 and ~165 KB at max**. Elevation size is roughly flat to slightly
*falling* with zoom (Fuji 121 → 101 → 71 KB for z8 → z10): each tile is a fixed
256² raster and higher-zoom tiles are smoother. So tile **bytes scale with tile
count, ×4 per zoom step over the same area**, not with zoom per se. The
synthetic `alpine-demo-terrain` tiles (4.5 KB / 740 B) are not representative
and must not be used for sizing.

## 3. Bundle-size implication for D-73 (A4)

D-73 A4 ratified "HashSeq of size-tuned tile bundles, 16 KiB – tens of MB".
Measured against that range:

| bundle size | real (z,x,y) pairs per bundle (p50 ≈ 110 KB … max ≈ 165 KB) | notes |
|---|---:|---|
| 16 KiB | **0** | below a single real elevation tile (smallest real elevation tile is 65 KB). Only the synthetic demo tiles fit. |
| 128 KiB | ~1 | the natural floor: one co-located elevation+satellite pair |
| 1 MiB | ~6–9 | ≈ one 3×3 spatial block; also the smallest unit the existing packager can express (`builder.rs:288-297` takes `max_chunk_size_mb: u32`) |
| 4 MiB | ~25–38 | ≈ one 5×5–6×6 block |
| 16 MiB | ~100–150 | the whole PNW z8 set (7.3 MiB) fits in one |

Consequences:

- **The 16 KiB lower bound is dead weight for terrain**: no real tile is that
  small. The effective floor is one tile pair (~128 KiB); the existing packager's
  1 MiB granularity is a sensible default (≈ a 3×3 PMTiles-ordered block, so
  over-fetch at a ring edge is bounded to one block).
- **Every archive measured today fits in 1–8 one-MiB bundles** (Fuji: 1,
  Zurich: 2, PNW: 8). At today's corpus size, streaming buys almost nothing over
  A1 whole-hexon fetch; the A4 benefit appears only at coverage ≥ ~z11 over a
  regional area. Projection (not measured): PNW's 70 z8 coordinates at z12 over
  the same bounds is 70 × 4⁴ = 17 920 pairs × ~110 KB ≈ **1.9 GB** — that is the
  regime the ratified design targets, and nothing in the corpus exercises it
  yet. A z11+ ETL output is the first real streaming test fixture.
- **Satellite and elevation should share a bundle** (same spatial block): they are
  always requested together for a coordinate, and satellite alone (~20 KB) is too
  small to be worth its own HashSeq child.
- Entity hexons (≤ 6 KiB total) are below any bundle threshold; they should
  ship whole (one blob), never chunked.

## 4. Prior-art digest (D-74 "mandated prior art")

### `_archive/render_distance_lod_20260407` (render-interest half)

Designed, never built (apart from what later landed independently):

| archived design | status at 49e2de0 | delta vs D-74 |
|---|---|---|
| `AppSettings` RON at `~/.config/fractalengine/settings.ron`, defaults on missing/corrupt, debounced save | resource landed 7bdcfbe; persistence lands in this track's Phase 4b (PAS-P4B) | same design; path is the platform config dir (`dirs::config_dir()`), not a hard-coded `~/.config` |
| `render_distance` clamp [50, 1000] world units, default 300 | default 500 (matches `PetalManifest.render_distance`), clamp widened to [0, 50 000] | D-74 uses `0` as "halt distance-ranked spawning" and real-world-scale petals exceed 1 km, so the 1000 m hard limit is not carried over |
| `RenderVolume` + per-entity `Visibility` toggle | superseded | D-74 gates **spawns** (prevention, not recovery; render buffers never shrink), it does not hide already-spawned entities |
| 3-tier LOD (`lod_mid_hash`/`lod_low_hash` node fields, 30/70/100 % bands, 0.5 s / 2 m hysteresis) | not built | out of scope for D-74; the tier thresholds and the hysteresis numbers are the reusable part if LOD returns |
| `FetchBlob` priority (Urgent/Background/Prefetch), fetch only within 1.5 × render_distance | not built | maps 1:1 onto D-74's demand-driven `FetchStrategy`; the ×1.5 prefetch ring equals the existing terrain despawn-hysteresis factor |

### `_archive/relay_data_horizon_20260407` (data/sync-interest half)

This was the open input flagged in D-74; digested here:

| archived design | delta vs D-73…D-78 |
|---|---|
| Data-horizon layers ACTIVE / WARM (LRU 10 petals) / COLD (metadata only) / PINNED | **Orthogonal to D-74.** D-74's ledger is *render* residency inside one petal; the data horizon is *sync* scope across petals/verses. No conflict; WARM/COLD map onto "header phase only" (metadata-before-bytes, D-75) for non-active petals. Not scheduled. |
| A `RelayConfig { pinned_verses, max_relay_cache_bytes, serve_blobs }` in `AppSettings` | **Name collision, different concept.** Today's `fe_sync::relay_config::RelayConfig` (D-77) selects *iroh relay servers*; the archived one is *verse pinning / seeding*. If pinning is revived it needs a different name (e.g. `SeedingConfig`). D-78's settings surface carries only the relay-server selection. |
| Multi-replica sync driven by the horizon, bounded by `MAX_OPEN_REPLICAS` | not built; independent of the streaming round |
| Blob serving: local → relay peers → verse peers, per-peer rate limit | subsumed by D-73/D-75 `FetchStrategy` (semaphore + peer priority); "prefer seeding peers" is a peer-priority input, not a new mechanism |
| Headless relay binary (`--relay`) | exists as `fractalengine-relay` (the app's headless server, not an iroh relay) |
| Auto-seed on verse creation | not built; would need a decision if revived (it changes disk-usage defaults) |

Net: neither archive contradicts D-73…D-78. The data-horizon layering is a
future sync-scope track, not an input that changes the ratified render/transfer
shapes.

## 5. Double-residency math (risk #5)

### Consumer map

| loader | site | lifetime | consumers |
|---|---|---|---|
| `TilesetRegistry::load_all()` | `fractalengine/src/main.rs:381` (GUI) and `fractalengine-relay/src/main.rs:368` (headless) at startup; `install()` / `reload()` per tileset (`registry.rs:183-216`) | process lifetime, every installed tileset, owned in `RwLock<HashMap<String, HexonTileSource>>` | **only** `fe-api/src/terrain.rs:504,542` (HTTP tile routes via `registry.get_tile` / `get_satellite_tile`, which **copy** the slice into a fresh `Vec` per request) |
| `HexonStore::load_tileset()` per assignment | `fe-terrain/src/petal_binding.rs:153` | per active petal assignment; moved into a fresh `CompositeTileSource.hexon_sources: Vec<HexonTileSource>` (`composite.rs:36,86`) | the terrain renderer (`get_tile_sync` / chunk mesh builds) |

The renderer never reads the registry's copy, and the API never reads the
composite's copy, so the **active** tileset is resident twice.

### RAM for the measured archives

`HexonTileSource` holds decoded member bytes in `HashMap<String, Vec<u8>>`; tiles
are Stored, so resident ≈ tile bytes (+ ~100 B/entry key/allocation overhead,
negligible here).

| scenario | registry copy | petal copy | steady-state RAM |
|---|---:|---:|---:|
| today's install set (PNW only), PNW petal active | 7.31 MiB | 7.31 MiB | **14.6 MiB** (2×) |
| all three ETL tilesets installed, PNW active | 9.48 MiB | 7.31 MiB | 16.8 MiB (vs 9.48 deduped) |
| projected PNW at z12 (§3) active | ~1.9 GB | ~1.9 GB | **~3.8 GB** (2×) |

Transient: `load_tileset` reads the whole file (`std::fs::read`) and
`HexonArchive::import` copies members out of it, so a load peaks at roughly
archive + decoded ≈ 2× the tileset for its duration — on top of whatever is
already resident. A petal switch that reloads the active tileset therefore peaks
at ~3× tileset bytes.

### `Arc<HexonTileSource>` dedup sketch

- **Owner:** `TilesetRegistry` stays the single owner. Change
  `sources: RwLock<HashMap<String, HexonTileSource>>` →
  `RwLock<HashMap<String, Arc<HexonTileSource>>>` and add
  `pub fn source(&self, id: &str) -> Option<Arc<HexonTileSource>>` (clone of the
  `Arc`, no byte copy).
- **Sharer:** `petal_binding.rs:153` calls `reg.0.source(uri)` first and falls back
  to `store().load_tileset(uri)` + `registry.insert` only on a miss (lazily
  populating the registry rather than loading twice).
  `CompositeTileSource.hexon_sources` becomes `Vec<Arc<HexonTileSource>>`;
  `add_hexon_source` takes the `Arc`. All composite reads are `&self`, so no
  interior mutability is needed.
- **Mutation hazard:** `load_tileset` backfills `tileset_meta` scale fields
  (`store.rs:221`) — that already happens at load time before wrapping, so the
  `Arc` content is immutable afterwards. `remove()` / `reload()` replace the
  registry's `Arc`; an active composite keeps the old `Arc` alive until the next
  petal assignment, which is the correct (no tear) behaviour.
- **Interaction with Phase 4 lazy tiles:** once `HexonTileSource` becomes an
  offset index + on-demand member reads, the `Arc` shares the index and the
  member cache; this sketch is the prerequisite, not a competitor.
- Estimated saving: one full copy of every *active* tileset (7.3 MiB today,
  ~1.9 GB at the projected z12 coverage) plus the ~1× transient load spike on
  every petal switch.

## Appendix — reproduction script

Not committed as code; run with any Python 3 (`python hexon_sizes.py`).

```python
"""Hexon member-size histogram (PAS-P0 evidence). .hexon = zip; tiles are Stored."""
import glob, os, re, sys, zipfile
from collections import defaultdict

WS = r"C:\Users\atooz\Programming\fractalengine-workspace"
SOURCES = (
    [os.path.join(WS, "gis-tile-etl", "dist", n + ".hexon") for n in
     ("japan-mount-fuji", "switzerland-zurich-alps", "north-america-pacific-northwest")]
    + sorted(glob.glob(os.path.join(WS, "fractalengine", "sample-hexons", "dist", "*.hexon")))
    + sorted(glob.glob(os.path.join(os.environ["LOCALAPPDATA"], "fractalengine", "hexons",
                                    "tilesets", "**", "*.hexon"), recursive=True))
)
TILE = re.compile(r"(?:^|/)(tiles|satellite)/(\d+)/(\d+)/(\d+)\.\w+$")

def pct(sorted_vals, p):
    # nearest-rank percentile
    if not sorted_vals:
        return 0
    k = max(0, min(len(sorted_vals) - 1, int(round(p / 100 * len(sorted_vals) + 0.5)) - 1))
    return sorted_vals[k]

def kib(n):
    return f"{n / 1024:.1f}"

print("| hexon | archive KiB | members | tile bytes KiB | non-tile KiB |")
print("|---|---:|---:|---:|---:|")
groups = {}
for path in SOURCES:
    if not os.path.exists(path):
        print(f"| {os.path.basename(path)} | MISSING | | | |"); continue
    name = os.path.basename(path)
    zf = zipfile.ZipFile(path)
    g = defaultdict(list); tile_total = other_total = 0
    for info in zf.infolist():
        m = TILE.search(info.filename)
        if m:
            kind = "elevation" if m.group(1) == "tiles" else "satellite"
            g[(kind, int(m.group(2)))].append(info.file_size); tile_total += info.file_size
        else:
            g[("other", -1)].append(info.file_size); other_total += info.file_size
    groups[name] = g
    print(f"| {name} | {kib(os.path.getsize(path))} | {len(zf.infolist())} | {kib(tile_total)} | {kib(other_total)} |")

print()
print("| hexon | kind | zoom | count | total KiB | p50 B | p90 B | p99 B | max B |")
print("|---|---|---:|---:|---:|---:|---:|---:|---:|")
all_tiles = []
for name, g in groups.items():
    for (kind, z), vals in sorted(g.items()):
        vals.sort()
        if kind != "other":
            all_tiles += vals
        print(f"| {name} | {kind} | {'-' if z < 0 else z} | {len(vals)} | {kib(sum(vals))} | "
              f"{pct(vals,50)} | {pct(vals,90)} | {pct(vals,99)} | {vals[-1]} |")
all_tiles.sort()
print()
print(f"ALL TILES: n={len(all_tiles)} p50={pct(all_tiles,50)} p90={pct(all_tiles,90)} "
      f"p99={pct(all_tiles,99)} max={all_tiles[-1] if all_tiles else 0}")
```
