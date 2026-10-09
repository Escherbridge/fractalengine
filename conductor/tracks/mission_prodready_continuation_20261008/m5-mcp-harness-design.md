---
type: Design Note
track: mission_prodready_continuation_20261008
title: "M5 (F13/F14) — MCP dispatcher + harness coverage map (recon 2026-10-09)"
timestamp: 2026-10-09T10:30:00Z
---

# M5 — MCP + API integration: implementation map

Recon (sonnet explore, 2026-10-09). Line cites at recon time (post-2023809).

## Load-bearing facts

- **16 tools exist** (11 + 5 sim_*); EXPECTED_TOOLS confirms. The
  mcp_scene_primitives track metadata ("still 6-tool surface") is stale —
  read_node/node_address/delete_node/promote_instance (endpoint_api_surface),
  query_timeseries (F7), sim_* (F9) all landed since.
- **The three warts, precisely**: create_node (mcp.rs:472-485) and
  create_petal (:433-441) scope-check ONLY when the CALLER supplies hierarchy
  ids — omitting them degrades to role-only (trust-the-client-for-authz);
  update_transform (:515-518) has NO scope check at all despite carrying
  node_id. The strong pattern is in the same file (read_node :566-579:
  resolve_node_scope → require_scope; delete_node adds caller_auth). Fix =
  always DB-resolve the resource's real scope; never use client-supplied
  verse_id for authz.
- **FR-5 inversion**: every arm today calls require_* INLINE (17 call sites);
  the dispatcher refactor moves all of them into one table-driven guard.
  Minimal shape ratified below; the sim dispatch (one arm → per-verb fns,
  mcp.rs:775-795) is the proven template to generalize.
- **GLB upload: no upload endpoint exists anywhere** — assets.rs is
  download-only; the only ingest is DbCommand::ImportGltf reading a LOCAL
  file path on the DB thread (GUI-dialog-only). The shared FsBlobStore handle
  already reaches ApiState.blob_store (one handle, three consumers) — the new
  multipart/base64 handler calls blob_store.add_blob API-side (spawn_blocking)
  and sends only {name, content_type, size, hash} over the channel. The
  "bytes never cross the channel" constraint is NEW-path discipline, not a
  regression fix.
- **Harness gaps vs A27**: WS = serde-only (no live handshake test);
  hexon/tileset = zero fe-api round-trip tests; IoT ingest/export + share
  mint→redeem = covered; token lifecycle = JWT/store layers covered but
  NOTHING drives expiry/revocation through the live auth_middleware over
  HTTP; cross-thread = emulators on the same tokio runtime, not the
  harness-routed DB↔API↔sync seams the api_mcp track FR-5 specifies; MCP
  negatives exist (incl. the KNOWN-WEAK wart markers F13's flip must turn
  into strict assertions) but no structured fuzz pass.
- **peer.rs bare-.ok() inventory**: the :705/:709 note is stale by line, but
  ~21 awaited request/reply sends remain bare .ok() (lines 212,225,722-912
  region) — silent drop on full channel → generic test timeout instead of a
  loud diagnostic. The M3 fix pass converted only the ingest reply (:556,561)
  and the unsolicited replication echo (:892-899, try_send by design).

## Decisions

- **DEC-C14 (2026-10-09)** — A26 "20 tools" arithmetic: treat it as "the
  mcp_scene_primitives 20-name vocabulary fully present" (7 exist incl.
  delete_node; 13 to ship: upload_asset, place_asset, set_property,
  get_properties, delete_property, create_waypoint, move_waypoint,
  import_gpx, set_petal_terrain, list_tilesets, install_tileset,
  get_gis_nodes, query). The true tools/list length becomes 24 non-sim + 5
  sim = 29 — a documented, intentional superset recorded in fe-api/AGENTS.md
  §mcp-dispatch, never a silent redefinition and never deletion of the 9
  legitimately-shipped extras. install_tileset role = Manager+ (the track's
  open question 1 default).
- **DEC-C15 (2026-10-09)** — M5 execution: F13 then F14 strictly sequential
  (same files, F14's wart-flip is definitionally downstream of F13's
  dispatcher). F13 runs on OPUS (security-authz refactor, 3 CVE-shaped
  warts); F14 on sonnet. F13's ToolSpec shape: `{name, description,
  input_schema, min_role, scope_rule, handler}` with `ScopeRule {None,
  Global, PetalArg(&str), NodeArg(&str), HierarchyArgs}`; dispatcher owns
  resolution + require_* BEFORE the handler; unresolvable scope → deny.
