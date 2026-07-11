---
type: retro
title: Consolidated Wave Retrospectives — Wave 1-3 + Tauri Migration
tags: [chore, retro, wave_retros_20260710]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Consolidated Wave Retrospectives

Evidence base: the 2026-07-10 `conductor/tracks.md` reconciliation pass —
every claimed-complete or claimed-open track was checked against the
codebase (crate existence, named types/systems, test presence) rather than
assumed from the checkbox state. This doc summarizes what that pass found,
wave by wave.

---

## Wave 1: Core Infrastructure (Foundation)

**What shipped:** Seed Runtime, Root Identity, Petal Soil, Mycelium Network,
Bloom Renderer, Petal Gate, Canopy View, Fractal Mesh, Gardener Console — all
verified present and substantially matching their original specs.

**What drifted:** Thorns and Shields (security hardening) did **not** ship
despite being in the original Wave 1 batch. Verification found:
`docs/webview-threat-model.md`, `docs/security-checklist.md`,
`docs/unwrap-audit.md`, `scripts/audit.sh`, and two fuzz targets all exist —
but every one of them is the thin Wave-1 write-only scaffold (8-21 lines
each), never filled in. `unwrap-audit.md` literally still reads "Status:
PENDING — run scripts/audit.sh after Wave 6" and cites a `.expect("SurrealDB
init")` call that `code_review_20260430_db_graceful` already removed months
later — the audit doc is stale relative to the code it's supposed to
describe.

**Lesson:** PLAYBOOK.md's "write-only mode, mark [~] not [x], defer
validation to Wave 6" strategy worked for 9 of 10 Wave 1 tracks but silently
dropped the 10th once Wave 6 validation actually happened — nothing forced a
return pass on Thorns and Shields specifically. A completed-wave checklist
should verify every originally-planned track individually, not just that
"the wave shipped."

---

## Wave 2: Interactive Digital Twin Platform

**What shipped (verified 2026-07-10, previously mis-marked `[ ]`):**
Viewport Foundation, Scene Graph Bridge, Selection System, Transform Gizmos,
Shared Peer Infrastructure, Garden Console, Mycelium Live, Bloom Stage,
Petal Portal, Fractal Atlas, and the UI Manager Architecture Refactor
(FR-1 through FR-4 all present in `fe-ui/src/plugin.rs`).

**What drifted or never shipped:**
- **Drag & Drop Asset Placement** — spec called for native OS
  `FileDragAndDrop` + placement-preview-follows-cursor + Alt-Drag duplication
  + an Asset Library panel. None of that exists. What shipped instead is a
  simpler `ActiveDialog::GltfImport` dialog with a manual file-path text
  field. The feature need (getting a GLB into the scene) is met by a
  different, cheaper mechanism than specced — worth deciding explicitly
  whether to retire the original spec or still build it, rather than leaving
  it in permanent limbo.
- **Petal Seed** (the track `Drag & Drop` depends on) — same story: no
  `AssetRegistry`/asset-browser-panel.
- **Light Box** — no lighting system at all. Not started, not partially
  done, not superseded by anything. A genuine gap: GLTF models still rely on
  Bevy defaults.
- **Inspector Settings** — shipped, but as a *different* feature than
  specced. `InspectorTab` is `{Properties, ApiAccess, Query}`, not the
  specced `{Info, Settings, Access}` with per-hierarchy-level (Node/Petal/
  Fractal/Verse) inspection and an RBAC Access tab. The `ApiAccess`/`Query`
  tabs are arguably *more* useful (they wire in the Phase 6.1 `fe-query`
  work), but they are not what this track's spec.md describes — anyone
  reading spec.md and the shipped code side-by-side would reasonably wonder
  if the track happened at all.
- **User Profile Manager** — no implementation found anywhere.
- **Seedling Onboarding** — no first-launch wizard or entity CRUD UI beyond
  `seed_default_data()`.

**Lesson:** Wave 2 has a real split personality: the *3D editor pipeline*
half (viewport/selection/gizmos/scene-bridge) shipped essentially as
specced, while the *content-ingestion and identity* half (drag-drop,
onboarding, profile, full inspector) mostly didn't, or shipped as a
substitute. Both halves were marked `[ ]` identically in tracks.md before
this pass, which hid a meaningful distinction: "not started" and "shipped
differently than planned" need different next actions (build it vs. update
the spec to match reality), and a flat checkbox erases that difference.

---

## Wave 3: External Access & IoT Platform

**Entity Data Layer:** Phases 1-5 were already correctly marked complete.
This pass additionally confirmed Phase 6.1 (`fe-query` — builder/graphql/
geo/duckdb_compat/columnar modules, wired into the Inspector's `Query` tab)
and Phase 6.5 (Hexon Format — `fe-format` fully rewritten) are both done and
were simply never checked off. Phase 6.2 (DataFusion + GeoParquet) remains
correctly deferred per MEMORY.md — this one was accurately tracked.

**Terrain, GPX & Crate Registry:** Both Phase 7 (`fe-terrain`) and Phase 8
(`fe-hexon`/`fe-hexon-registry`) are done and were also simply never checked
off. Phase 8 carries two known, still-open gaps from its own review (5/7
PASS): terrain auto-config not wired, and — significant — **RBAC is not
enforced anywhere in the fe-hexon crate**. This pass re-confirmed the gap
(zero role/scope/rbac references in the crate) and folded it into the new
`auth_policy_pattern_20260710` spec as a concrete acceptance criterion,
rather than letting it sit as a dangling "known issue" indefinitely.

**Plugin System (Phase 9A-9C):** `fe-plugin`, `fe-sdk`, `fe-plugin-test` all
exist, are substantially complete (89 tests total per MEMORY.md), and — this
is the most surprising finding of the whole pass — **were never registered
in `conductor/tracks.md` at all**. Three fully-shipped tracks were invisible
to the file that's supposed to be the single source of truth for what's
done. They're added in this pass. Also found: `fe-plugin` already depends on
`fe-sdk` (`Cargo.toml`), which appears to resolve the MEMORY.md "known
issue" about parallel type definitions — worth re-verifying on the next pass
rather than continuing to treat it as open.

**External Access:** Realtime API Gateway shipped (as hand-rolled JSON-RPC
2.0 rather than the specced `rmcp` crate — a reasonable substitution, MCP/
REST/WS all present). SSO Federation and Release CI are both genuinely
not started (no OIDC code, no `.github/` directory).

**Code Review 2026-04-30 (6 tracks):** 3 of 6 shipped (egui deprecation
removed, channel-error swallowing fixed, DB init made graceful via
`DbInitError`). 3 of 6 did not: the mega-function refactor
(`apply_db_results` is still ~430 lines), the O(n³) hot-path fix
(`update_node_position`/`update_node_url` still walk the full tree), and the
broader clippy/quality pass (a `peer_registry` dead_code warning persists
per MEMORY.md). The mega-function and clippy items are picked up by the new
`feui_decomposition_20260710` track rather than left as a separate,
overlapping effort.

**Tauri WebView Migration (2026-06-30):** All 3 tracks shipped and verified
(`backend-tauri` is the default feature, `tauri_commands.rs` exists). The
Pear Runtime spike completed with a "hybrid" recommendation; the
Tauri-Host-Shell spike has not been started.

---

## Cross-cutting findings (not specific to one wave)

1. **A real bug in the conductor plugin's SessionStart hook.** The globally
   installed `conductor`/`conductor-okf` plugin's `load-context.js` and
   `session-end.js` hooks count tracks with the regex
   `/\[(completed|in-progress|pending)\]/g` — literal bracketed status
   words. This project's `tracks.md` (correctly, per the plugin's own
   `setup.md`/`implement.md`/`status.md` command docs) uses GitHub-style
   `[ ]`/`[~]`/`[x]` checkboxes instead. The hook regex never matches this
   project's format, so it always reports 0 tracks — this is why "the setup
   reports 0 tracks" even though `tracks.md` has 60+ entries. This is a
   defect in the plugin (outside this repo's `conductor/` directory, not
   fixed by this pass), not something fixable from within `conductor/**`.
   Recorded here so it isn't rediscovered from scratch next time.
2. **`conductor/setup_state.json` was stuck mid-Phase-3** ("3.3_initial_track_generated")
   despite 60+ tracks existing — missing `project_type`/`created_at` per its
   own documented schema. Fixed this pass.
3. **21 of ~49 track folders had no `metadata.json`** at all (schema
   documented in `conductor-okf`'s `commands/new-track.md`). One had the
   wrong key (`track_id` instead of `id`). All repaired this pass.
4. **The "leave it unchecked, verify rather than assume" instruction paid
   off in both directions.** Several tracks assumed-open (Wave 2's 3D
   pipeline, plugin system, Terrain/Hexon Registry) turned out shipped; at
   least one assumed-shipped-by-hint (Drag & Drop) turned out to have a
   different, simpler mechanism instead. Both mistakes (marking done work as
   open, and marking undone/substituted work as done) are costly for
   planning — verification against actual code, not against a prior
   session's summary, is the only reliable check.
