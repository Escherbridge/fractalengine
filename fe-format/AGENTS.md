# fe-format — module notes

Design rationale for the `.hexon` archive crate. Code carries terse one-line
doc comments; the "why" lives here.

## §decompression-limits (REG-DECOMP, 2026-10-11)

`src/limits.rs` closes the security audit 2026-07-31 decompression-bomb
finding for every `.hexon` reader in the engine. `read_entry_capped(entry,
declared, cap, budget)`:

- refuses when the entry's **declared** uncompressed size exceeds `cap` or the
  remaining `budget`, before allocating anything (pre-allocation is
  `declared`, which is then known to be ≤ the limit);
- reads through `take(limit + 1)` and refuses when the **actual** output is
  over the limit — zip readers trust the header, so a lying header would
  otherwise inflate unboundedly;
- drains the shared `budget` by `max(declared, actual)`: the buffer keeps its
  `declared` reservation, so charging only output would let thousands of
  tiny entries that each declare the per-entry cap retain far past the budget.
  (The registry's sink-only `drain_entry_capped` buffers nothing and charges
  output only.)

Limits (stated assumptions, constants in `limits.rs`): JSON/text metadata
entries 8 MiB, binary entries (tiles, assets, GPX, `entities/nodes.json`) 64
MiB, 1 GiB per archive, ≤ 200 000 entries (`check_entry_count`). Every refusal
carries `DECOMPRESSION_LIMIT_ERROR` ("archive entry exceeds decompression
limit").

Callers: `HexonArchive::import` (one budget per import — reached by REST/MCP
tileset install twice, `.hexon` petal import, `TilesetRegistry`, fe-terrain
sources), fe-terrain `read_baked_splats_from_archive` (the `terrain/splats/`
entries `import` does not read), fe-hexon `HexonPackage::open` (crate
install). fe-hexon-registry keeps a mirrored copy (`src/limits.rs` there)
because it must not depend on this crate's bevy-pulling fe-runtime edge.

`import` still loads every blob into memory — a caller that only needs
metadata should read the zip lazily (the registry's `index.rs` pattern).
