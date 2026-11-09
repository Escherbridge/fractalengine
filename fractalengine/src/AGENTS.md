# fractalengine (GUI binary) — main wiring rationale

Checked selection route commits and Undo feed the same `PathProjection` consumer as
ordinary point edits. `advance_path_edits` drains UiManager's confirmed selection
projections before the existing Undo queue. Each projection carries a real positive
journal version. The consumer checks the current owning Petal, known committed versions
and a bounded consumed-version watermark before rebuilding rendering or editor rows;
queue order cannot let an older ordinary receipt overwrite a newer batch result.
Unversioned reconciliation uses normal hierarchy/property hydration.
Confirmed projections also carry an editor draft policy. The host always advances
accepted authoritative geometry, but compares the current draft against the owned
before-source before replacing editor rows. Changed drafts are retained with a visible
status/advisory; external sources with no owned baseline preserve differing drafts.
This check runs at consumption as well as in fe-ui result handling, closing both
post-submit and result-to-host intervals without relabeling a committed save as failed. Watermarks reset
on Petal/navigation/authority reloads and prune removed node IDs. History previews have no path into
this queue. QA debug and portal adapters read NodeManager's primary accessor; their
actual rendered-entity identity guards are unchanged.

## Admitted scene and route projection

The GPX import/path systems run as one explicit chain after UI PostSelection.
The UI's FIFO DB-result projection therefore admits the scene source before
the host consumes its queues, and deferred import spawns are visible before
route reconciliation. `advance_path_edits` is the current route-render owner:
it coalesces a matching owned local edit policy with the full current property
bag, then renders each current node once. Current peer/read snapshots never
enter the local positive-history queue with a made-up version.

Petal entry and hierarchy replacement build a capped node backlog, admitting
up to64 refresh intents per frame through VerseManager. Its bounded private
current-reader owns the actual RPC; no blocking uncorrelated property wave is
sent by the host. The old property-only renderer has been retired, so a stale
NodePropertiesLoaded that the UI rejected cannot overwrite a newer route.
Legacy deletion still tears down all matching routes and icons.

The QA pending count includes manager read/intents/current projections and
the host's deferred/backlog work. Terminal maintenance clears these host
queues and route/style maps after dropping their normal schedules. This is
read-model lifecycle behavior, not evidence that destructive reset or actual
peer asset/history sharing has been exercised.

## Confirmed local API address

The existing API-listening fan-out always forwards the actual bound SocketAddr
to `api_endpoint::install`, alongside its opt-in dev-token and QA-control feeds.
That bridge writes `fe_runtime::app::RuntimeApiEndpoint`; it does not infer a
port or advertise a listener before binding. Inspector examples consume the
resource mirror. Failure to receive an address leaves those examples unavailable.

## §qa-dialog-window-controls

Actual API listener notification is fanned out once to the dev-token writer and QA control host; cloned receivers
must not compete for that notification. QA dialog controls activate only with the input opt-in, a valid QA root,
and an actual loopback address. Input-enabled hosts without those prerequisites use a disabled provider, never
a native blocking fallback. `FE_QA_API_PORT` selects a nonzero loopback port only when a validated QA root exists;
normal GUI bind behavior remains port 8765. The launcher records the matching `api_base`.

QA-only continuous winit updates keep restore/window operations serviceable while minimized. Requests share
the topbar executor and real close confirmation. The UI state probe adds actual physical window dimensions,
maximized/minimized mirrors, app UI scale and close confirmation; semantic Window/Dialog titles supplement
explicit UiManager state. File selection and safe output are owned by `fe-ui::file_dialogs`, including guards
that survive the real consuming import/export operation.

`src/main.rs` boots the desktop GUI: it spins up the DB / network / sync / API
threads, builds the Bevy `App` (DefaultPlugins + egui + renderer + fe-ui +
terrain + webview), and bridges the background threads to Bevy resources.
`terrain_bridge.rs` and `asset_bridge.rs` are the per-frame drain systems that
turn fe-ui's queued ops into real side effects (the UI crate has no DB / blob /
filesystem access by design).

## §identity-tiers (F6 in-app lockout root cause)

The node keypair load used to be two-tier: OS keystore, else a bare
in-process `NodeKeypair::generate()` on ANY keystore error — including a
transient one (locked Credential Manager, Secret Service hiccup). A fresh
ephemeral DID doesn't match any verse's `created_by`, so `role_manager
::resolve_role`'s owner check (`fe-database/src/role_manager.rs:46-68`)
correctly falls through to `default_access` and the local user becomes a
mere Viewer of their own verse — on their own machine, with their own data
still sitting there under the OLD did. The startup `reconcile` rule
(`orphaned_creator`) correctly refuses to auto-fix this: it never claims a
`did:key:`-shaped creator (OF-4; `fe-database/src/AGENTS.md` §reconcile), on
purpose, because it can't distinguish "my own old identity" from "a remote
peer's verse I never joined".

The fix is a third, durable tier in between: `fe_identity
::load_or_generate_keypair_durable` tries the keystore, then a key file
under a per-user OS data directory, and only mints an ephemeral key if
BOTH fail. A file-tier load also attempts to heal the identity back into
the keystore (best-effort, never fatal) so a later launch — once the
keystore recovers — prefers it again. `KeypairSource::Ephemeral` is the
one case that still degrades silently from the DID's point of view, so it
is surfaced LOUDLY: `tracing::error!` plus `fe_ui::plugin::IdentityBanner`,
a session-sticky, click-to-dismiss warning in the status bar
(`fe-ui/src/panels/AGENTS.md` §identity-banner) — "Your saved identity
could not be loaded — running with a temporary identity; existing content
may appear read-only."

**Security follow-up (same-day, pre-ship review).** Two issues in the
first cut of the file tier, both fixed before any release used it:

1. **The fallback dir must not be CWD-relative.** The first cut derived it
   from `DB_PATH`'s parent (`data/identity/`), which resolves relative to
   whatever directory the process happened to be launched from — fine for
   the DB file (which already has that caveat, `packaged_root` not
   withstanding) but wrong for identity material that must be found
   regardless of launch directory. It now resolves via `dirs::data_local_dir()`
   (`%LOCALAPPDATA%\fractalengine\identity` on Windows — `dirs` was already
   a `fractalengine` dependency, nothing new added); `legacy_identity_dir`
   (the old CWD-relative path) is kept as a one-shot, read-only migration
   source (`fe_identity::file_fallback::load_or_generate_migrating` — read
   from legacy, write securely to the new dir, delete legacy; a crash
   mid-migration just leaves the legacy file for the next launch to retry,
   never a window with neither file present).
2. **Windows must not rely on an inherited ACL for the seed.** The DACL a
   freshly created file inherits from its parent directory can be broader
   than owner-only (e.g. `BUILTIN\Users`). DPAPI (`CryptProtectData` via the
   `windows` crate) was the first-choice design, but `windows` is only a
   deep transitive dependency here (via `keyring`/Tauri, no DPAPI surface
   exposed) and its failure mode is decrypt-time (profile/SID-binding edge
   cases) — exactly the "identity silently stops loading" bug class this
   fix exists to kill. Instead, `fe_identity::file_fallback::windows_acl::
   restrict_to_owner` sets an explicit, protected (non-inheriting),
   owner-only DACL via `advapi32`'s `SetFileSecurityW` + an SDDL string
   (`"D:P(A;;FA;;;OW)"`), immediately after `create_new` creates the file.
   **Fails closed:** if the DACL set errors for any reason, the file is
   deleted and the whole file tier reports `Err`, falling through to the
   loud ephemeral tier rather than ever leaving a plaintext seed at a
   possibly-inherited ACL. Three Win32 functions, bound via a minimal
   direct `extern "system"` declaration rather than adding `windows`/
   `windows-sys` as a new dependency for such a small surface — see
   `fe-identity/src/AGENTS.md` §identity-tiers for the full rationale.

**If this has already happened once** (a verse is now orphaned under a
stale `did:key:`), the explicit, user-confirmed `DbCommand::
ClaimOrphanedVerse` (`fe-database/src/handlers/claim_orphaned_verse.rs`,
`fe-database/src/AGENTS.md` §reconcile) is the recovery path — OF-4 still
refuses to do this automatically, but a user who clicks "Claim ownership"
on a verse they know is theirs, that was never shared with anyone, can
recover it. The `share_signer` keypair (A24/DEC-C9, a separate keystore
slot) deliberately keeps its original two-tier fallback — losing it only
costs previously issued share links, never verse ownership, so it is out of
scope here.

## §proposal-budget

`terrain_bridge::sync_proposal_overlay_budget` mirrors fe-ui's
`MeshInstanceBudget` into fe-terrain's `ProposalOverlayBudget` (fe-terrain
cannot see fe-ui; this binary is the only crate that sees both). The pure
`proposal_overlay_allowance(exceeded, ceiling, last_count)` is 0 once the
watchdog has tripped, else the headroom `ceiling − last_count` capped at
`MAX_PROPOSAL_OVERLAYS`. It runs in **PreUpdate** on purpose: the watchdog
counts `Mesh3d` in `Last`, so by the next frame's PreUpdate the count
already includes the ghosts the terrain plugin spawned — the render system
in Update then sees headroom that excludes its own residents (it adds them
back itself; see fe-terrain `src/AGENTS.md` §terrain-proposals). Running in
Update unordered could let the render read a pre-spawn allowance and
overshoot. A raised `mesh_budget_ceiling` setting flows through
automatically.

## §ruler-settings

`terrain_bridge::sync_ruler_settings` (Update) copies fe-ui's
`AppSettings.show_scale_bar` / `show_world_grid` (the right-sidebar Settings
checkboxes) into fe-terrain's `RulerSettings.show_scale_bar` / `show_grid`,
which gate the scale-bar HUD and the adaptive metric grid (fe-terrain
`src/AGENTS.md` §ruler_plugin). Same reason as §proposal-budget: fe-ui has no
fe-terrain dependency, so only this binary can join them. Each field is
written only when it differs, so `RulerSettings` change detection stays quiet.
Ordering does not matter: a toggle lands at most one frame late.

## §ruler-hud-input

`sync_ruler_hud` is an `EguiPrimaryContextPass` bridge, explicitly after
`GardenerUiSet::GeometryReady` and in terrain's `RulerHudSet::Bridge`, before the
actual ruler `Draw` set. It clears the old terrain input before attempting context
access, then publishes only `ViewportGeometry::overlay_current`: the finalized
same-pass CentralPanel rectangle with the shared divider clearance. The UI stamp
and terrain input both check actual egui context, cumulative pass and
pixels-per-point. The ruler draw runs before that actual egui pass returns its
output, so Owner output observation and the native provider handoff retain their
existing order. This bridge does not change the eight mandatory surface producers
or the separately audited ruler treatment. Missing geometry simply omits the HUD.

QA41 v2 also requires ViewportGeometry::chrome_current, captured only after the
actual four-slot ScenePointerChrome collector finishes this egui pass. Placement
therefore reserves actual clipped navigation/browser/hint responses, moves the
complete ruler/chip above intersections or omits it. No prior Area bounds or
guessed hint height is used. Ordinary Update pointer admission is unchanged.

main and the existing ruler journey call install_ruler_hud. It orders the actual
outer PostUpdate EguiPostUpdateSet::EndPass after both CameraUpdateSystems and
TransformSystems::Propagate, then retains GeometryReady -> Bridge -> Draw within
that nested primary pass. The source-inspected DAG has camera update -> asset
events/frusta and UI layout -> transform propagation; vision pose precedes
propagation. No inspected edge returns from egui output to those producers.
EndPass -> ProcessOutput -> Owner observation -> provider handoff/annotation and
the later WindowChromeApply remain intact. This is a source DAG check, not a
materialized runtime schedule or execution claim.

The existing settings/ruler journey uses that real installer with the SDK's
CameraPlugin and TransformPlugin, checking first-frame resized/custom viewport
and propagated-height values inside EndPass. Its same-pass egui leg renders the
actual viewport header/long Select hint plus the production fixed/numeric painter
at normal/narrow scales with Results. These source assertions remain unexecuted;
real host resize/orbit/focus, UI scaling and live Circle checks remain required.

QA41 L3 closes the intervening raw-FOV writer dependency: the actual existing
vision `(apply_vision_pose, advance_vision_job).chain()` now precedes
CameraUpdateSystems as well as TransformSystems::Propagate. Both pose_job's
apply and finish_job/CameraSnapshot's restore therefore happen before the SDK
caches clip_from_view. The production ruler's raw FOV and render extraction's
cached matrix describe the same frame. Chain order, locks, cancellation, restore
ownership and the EndPass/Owner/provider sequence are otherwise unchanged.

The existing window-mode vision journey uses its real vision installer and
completion path, the SDK CameraPlugin/TransformPlugin and the real ruler
installer. It starts at 60 degrees, applies the actual orbit pose at 45 degrees, and
checks raw projection versus cached render matrix and propagated height in the
first applied EndPass and the first restored EndPass at 60 degrees. No test-only FOV
writer replaces either production path. These remain unexecuted source checks;
runtime DAG materialization and real pose/restore screenshots remain root work.

## §residency-stats (p2p_asset_streaming S8)

`terrain_bridge::mirror_tile_residency_stats` runs in Update. It writes the
active composite's tile residency into `fe_runtime::diag15m::ResidencyStats`:
- `tile_bundles` = `streamed_resident_bundles()`;
- `tile_bytes` = `resident_bytes()` + `streamed_resident_bytes()`.

fe-ui's Settings readout reads those stats ("… K tile bundles (X MiB)",
fe-ui `src/AGENTS.md` §residency-ledger). The reason is the same as for
§proposal-budget: only this binary sees both crates.

It writes only on change, and resets to zero when no composite is active. The
read takes each source's mutex once per frame, which is the same cost as the
ring pass's own pin and release calls.

## §tile-mode

`terrain_bridge::apply_global_tile_mode` applies fe-ui's `AppSettings.tile_mode`
(the Settings section's "Offline tiles (no network)" checkbox) onto
fe-terrain's active `CompositeTileSource`. **Assumption (PAS-P4B, recorded in
the track plan):** only `Offline` is a global override — a no-network switch;
any other value lets the petal's own `TerrainConfig.tile_source_mode` govern
(`effective_tile_mode`). Reversal: drop this system and the checkbox. It is
ordered `.after(apply_terrain_assignments).before(fetch_and_spawn_terrain_chunks)`
because an assignment rebuilds the composite with the petal's mode; running in
between means the first chunk fetch of a new petal already honours Offline (an
unordered system could let one frame of network fetches through). It writes
only when the mode differs, so `ActiveTileSource` change detection stays quiet.

## §app-settings

`main.rs` calls `fe_ui::settings::load_startup_settings()` right after tracing
init — before any thread spawns — because the relay spec must be registered
(`fe_sync::relay_config::set_settings_relay_spec`) before the sync thread builds
its iroh endpoint. The same `AppSettings` is then inserted before
`GardenerConsolePlugin` (whose `init_resource` keeps it), together with
`SettingsPersistence { path }`, which is what enables fe-ui's debounced save.
Other hosts (relay binary, tests) never insert `SettingsPersistence`, so they
never write the user's config dir. See fe-ui `src/AGENTS.md` §app-settings.

## §entity-store-analytics

`main.rs` creates one `Arc<EntityStore>` before spawning the local API thread.
`SharedEntityStore` is the Bevy resource handle over that same allocation, and
`drain_scene_changes_to_store` updates it from the DB scene-change bridge.
Before hydration, startup subscribes the scene-change bridge; then it waits for
the local seed command, replays its DB results for Bevy, queries every live
node through the direct local reader, and then starts the API thread. The seed
barrier prevents a first-run empty snapshot and the early subscription prevents
a change-event gap. Seed failure remains fatal, but a malformed row, query
failure, or 10-second hydration timeout leaves the editor running with
`entity_store = None`; the shared DB reader remains configured for durable
authentication, while the analytics endpoint fails unavailable rather than
exposing a partial cache.

**The reader is the DB thread's own handle (OF-2, 2026-10-13).** `main.rs`
spawns the DB thread with `fe_database::spawn_db_thread_shared` (explicit
`DB_PATH`) and uses the returned `DbThread::shared_db` clone as
`api_db_reader`. It must never open `data/fractalengine.db` a second time:
SurrealKV's per-handle file lock refuses a second in-process open while the DB
thread holds the store (`os error 33` on Windows), which is how packaged builds
used to start with the analytics/BI API silently unavailable. The clone's
router lives on the DB thread's multi-thread runtime, so the throwaway
current-thread runtime used for hydration can drop safely. Runtime and
read-only notes: fe-database `src/AGENTS.md` §shared-handle. `fe-relay` does
the same; it keeps the reader on hydration failure and only fails the
analytics cache closed. A successful hydration gives
`/api/v1/analytics/query` the same lock-free hot-cache view as the desktop app;
it is not a canonical history store and must not replace the future verified-log
materializer.

## §panic-hook

`panic_log.rs` (GUI binary only — `fe-relay` has no egui pass and stays
plain) installs a *chained* panic hook (`std::panic::take_hook` +
`set_hook`) as the very first line of `main`, before any thread spawns.

**Why it exists.** The unified spatial editor's `gardener_ui_system`
(fe-ui) runs the entire egui pass in one function body; a panic anywhere in
that body unwinds mid-pass and the process aborts. The crash cascade the
user hits (`ui_shell_architecture_20260724`, directive 3 — terrain tools
crash) surfaces only as `bevy_egui`'s "Encountered a panic in system" +
exit 101 on stderr — the *actual* panic payload (which `unwrap`/`expect`/
index panicked, where, with what backtrace) is never captured anywhere,
making the root cause undiagnosable from the user's report alone. This hook
is the diagnostic: it appends a timestamp, thread name, panic payload, panic
location, and a `Backtrace::force_capture()` to `data/panic.log` (same
writable-data-dir convention as `DB_PATH`), then calls the previous hook so
stderr output and Bevy's own reporting are unaffected. std-only, no new
dependency. It does not change crash *behavior* (the process still aborts on
an uncaught panic) — see FR-7 in `ui_shell_architecture_20260724` for the
panel-level `catch_unwind` guard that will make single-panel panics
non-fatal; this hook stays useful afterward as the log sink for whatever
still reaches an uncaught panic.

## §durability

SurrealKV's disk-flush cadence is controlled by the `SURREAL_DATASTORE_SYNC_DATA`
environment variable (read by surrealdb-core at datastore open). Valid values:
`never` | `every` | a duration string `>100ms`. Both binaries default it to
`every` **only when unset**, so an operator can still override it, and set it at
the very top of `main` — before the DB thread opens the datastore.

Note the earlier `SURREAL_SYNC_DATA="true"` block was a no-op: that variable
name is not read by surrealdb-core, and `"true"` is not a valid value for the
real `SURREAL_DATASTORE_SYNC_DATA` knob (it would reject at startup), so setting
the correct name to `every` is the actual durability fix.

## §assets

Blob-store initialization is a required startup dependency. Failure is reported
as a fatal startup error with exit code 1 rather than panicking through the
global panic hook; filesystem creation errors whose requested directory already
exists are normalized inside `fe_sync::FsBlobStore`.

Two asset paths converge here:

1. **API asset endpoints.** `fe_api::ApiConfig.blob_store` receives a clone of
   the real `FsBlobStore` handle (`blob_store_for_api`). Without it every asset
   endpoint returns 503 on the GUI binary even though the DB reader is wired
   (see `fe-api/AGENTS.md` §assets). One content-addressed store is shared by
   the DB thread, the sync thread, Bevy's `blob://` asset source, the API
   thread, and the download bridge.

2. **UI-initiated downloads.** The inspector's asset card pushes
   `UiAction::DownloadNodeAsset { node_id }`, which fe-ui drains into
   `asset_ops::PendingAssetOps` (fe-ui never touches the blob store or the
   filesystem). `asset_bridge::drain_asset_ops` runs each frame and, per op:
   - `resolve_asset` resolves the node in `VerseManager`, reads its
     `asset_path` (`blob://{hash}.{ext}`), parses the hex hash, and asks the
     `BlobStoreHandle` for the on-disk path — the path is **never** built from
     user input, only from the content-address store keyed by the parsed hash
     (same guarantee as `BlobAssetReader`). A node with `has_asset == true` but
     no cached `asset_path` gets a distinct, clearer error than a node with no
     asset at all (previously both collapsed to the confusing "node has no
     asset" message even when `has_asset` was true — see 2026-07-11 fix).
   - on success, `prompt_and_copy` opens a native `rfd` save dialog (suggested
     filename = sanitized node name + real asset extension, default dir
     `dirs::download_dir()`) and copies the blob to the user's chosen
     destination on confirm. **The dialog lives bridge-side, not in fe-ui** —
     only the bridge has the resolved node name + extension needed to build a
     sane suggested filename, and `UiAction::DownloadNodeAsset` /
     `asset_ops::AssetOp::Download` only carry `node_id` (kept unchanged
     deliberately: threading a `dest` field through would require editing
     `fe-ui/src/actions/mod.rs`'s `UiAction` enum + dispatch match arm, which
     is out of scope for a bridge-only change). User-cancelled dialog is a
     silent no-op: `PendingAssetOps` still drains the op, but `status` is left
     untouched (no toast, no error).
   - writes the outcome (`saved_path` or `error`) into fe-ui's
     `AssetDownloadStatus` resource. A fe-ui system surfaces it as a toast, and
     (as of the 2026-07-11 fix) the Asset card also renders it as a **persistent
     status row** scoped to the currently-selected node, so success/failure
     doesn't disappear with the toast.

   **INTEGRATION_REQUEST (2026-07-11, asset_download_fix track):**
   `asset_card_section`'s signature grew a `status: &AssetDownloadStatus`
   param (fe-ui/src/panels/asset_card.rs). Wiring it in requires a resource
   that currently isn't threaded to the inspector render call chain at all —
   `right_inspector` (inspector.rs), its caller in `panels/mod.rs`, and
   ultimately the Bevy system in `fe-ui/src/plugin.rs` that calls
   `panels::mod`'s entry point (which needs a new `Res<AssetDownloadStatus>`
   system param) all need one argument/param added each. All three files are
   outside this track's owned-file set (`plugin.rs` explicitly belongs to
   another worker), so this wiring is left for the coordinator/owning worker
   to apply — it's a mechanical thread-through, not a design decision.

## §gpx

`gpx_pipeline_20260711` track — the import→persist→render→serve chain for
GPX files. `gpx_bridge.rs` is the persist step; render (spawning
`GpxTrackLine` entities + populating `TrackRouteMap`) and the `gpx_track`
layer wiring are **out of scope here** (per `metadata.json`, that's W-B's
`petal_binding` territory) and remain unwired after this pass — see
"Residuals" below.

**Current state of the pieces (as found, before this pass):**

- `fe-terrain/src/gpx/{parser,stats,convert,export}.rs` — GPX 1.0/1.1
  parsing (`parse_gpx_bytes` → `GpxData`), stats (`compute_stats` →
  `TrackStats` with `total_distance_m`, `elevation_gain_m`,
  `elevation_loss_m`, `duration: Option<f64>`, `avg_speed_kmh`,
  `max_speed_kmh`, `bounding_box: BoundingBox`), and a **fe-terrain-local**
  `DbCommand` struct (`convert.rs`, fields `petal_id, name, position,
  parent_node_id, properties`) used by `gpx_to_scene_commands`. This local
  struct is a dead end for any real persistence path: it is NOT
  `fe_runtime::messages::DbCommand` and nothing translates its
  `parent_node_id`/`properties` fields into the real command set (see next
  bullet) — `gpx_bridge.rs` does its own mapping instead of reusing it.
- `fe-api/src/gpx.rs`'s `import_gpx` HTTP handler calls
  `gpx_to_scene_commands` and then creates nodes via the *real*
  `fe_runtime::messages::DbCommand::CreateNode { petal_id, name, position,
  correlation_id }` (FR-4: `correlation_id: Option<String>`, `None` here)
  — which has no `properties` or `parent_node_id` field. The handler only
  ever forwards `cmd.name`/`cmd.position`; `cmd.properties` and
  `cmd.parent_node_id` are silently dropped. **This means the shipped HTTP
  import endpoint has never actually written `gpx_type`/stats properties or
  any hierarchy** — confirmed by reading `create_node_handler`
  (`fe-database/src/handlers/crud.rs`): it never populates `properties`, and
  the `node` schema has no `parent_node_id` column at all (grep across
  `fe-database/src`; the HTTP export now reads `properties.gpx_track_id`
  instead — Residual 3). `gpx_bridge.rs` fixes the properties gap for the UI-driven path
  via `SetNodeProperty`; the missing schema column is a residual (below).
- `fe-terrain/src/terrain_plugin.rs`'s `GpxTrackLine` component
  (`{ track_node_id: String }`) + `render_gpx_tracks` system + `TrackRouteMap`
  resource (`fe-terrain/src/iot/animation.rs`, keyed by `track_node_id`,
  values carry **timestamped** route points) are fully implemented but
  **nothing in the codebase ever spawns a `GpxTrackLine` entity or populates
  `TrackRouteMap`** (grep for `GpxTrackLine` outside `terrain_plugin.rs` /
  `petal_binding.rs` returns nothing). Wiring that spawn (reading
  `gpx_type == "track"` nodes for the active petal, and populating
  `TrackRouteMap` from per-point data with timestamps for the animation) is
  the FR-3 render half of this track and is **not done by this worker**.
- `fe-api/src/gis.rs`'s `GET .../gis/tracks` reads nodes via
  `properties.gpx_type = 'track'` and deserializes cached stats straight off
  flat property keys (`row_to_track`: `total_distance_m`,
  `elevation_gain_m`, `elevation_loss_m`, `duration_s`, `avg_speed_kmh`,
  `max_speed_kmh`, `bounding_box`). `GET .../gis/nodes` surfaces
  `gis.annotation.title/body/color` (flat dotted keys, per
  `fe-database/src/AGENTS.md` §gis) via `extract_annotation`. These two
  read shapes are the **contract** `gpx_bridge.rs` writes against.

**`gpx_bridge.rs` design (the persist step, FR-2):**

- Contract with fe-ui (mirrors `asset_ops` exactly, per spec): drains
  `fe_ui::gpx_ops::PendingGpxOps` (`GpxOp::ImportFile { petal_id, path:
  PathBuf }`), writes `fe_ui::gpx_ops::GpxImportStatus` for the UI to
  surface. **fe-ui's `gpx_ops` module did not exist when this file was
  written** (a parallel worker owns it) — the exact field names of
  `GpxImportStatus` (`petal_id: Option<String>, track_count: u32,
  waypoint_count: u32, error: Option<String>`, mirroring
  `asset_ops::AssetDownloadStatus`'s shape) are this worker's best guess and
  may need reconciling once the real module lands (see INTEGRATION_REQUEST
  below).
- **One track node per imported file.** All `<trk>` elements/segments in
  the file are merged into a single synthesized track node (`compute_stats`
  already aggregates across every track in the `GpxData`), positioned at
  the **first trackpoint** in document order (not the bounding-box center
  the HTTP endpoint uses — a petal with a real terrain origin needs the
  track's own position to align with that origin, not an arbitrary
  bbox-center local origin). Node properties: `gpx_type = "track"` plus the
  six stat keys + `bounding_box` object, written via one `SetNodeProperty`
  call per key (flat keys — `CreateNode` has no `properties` field, see
  above). If the file has zero trackpoints, no track node is created and
  every `<wpt>` is persisted as a standalone waypoint instead (see below).
- **Waypoints as the track's "children".** Every `<wpt>` in the file is
  persisted as its own node with `gpx_type = "waypoint"` and
  `gis.annotation.title` set to the waypoint's name (or `Waypoint {n}` if
  unnamed) — matching the reserved annotation-key contract in
  `fe-database/src/AGENTS.md` §gis exactly, so these waypoints also surface
  through `GET .../gis/nodes`. **There is no `parent_node_id` column on the
  `node` table and no DbCommand to set one** (see above), so "child" is
  approximated with a custom flat property `gpx_track_id` = the track
  node's ID, set once the track's `CreateNode` result resolves. This is the
  best available linkage under "existing commands only" — a real
  parent/child relation would need a new schema column + `DbCommand`
  variant, which is out of scope (`fe-database/src/lib.rs` is quarantined).
  Standalone waypoints (no track in the file) get no `gpx_track_id`.
- **Exact host request correlation.** Native GPX track/waypoint imports,
  manual track creation and annotation births now carry private `host-path:{ulid}`
  IDs through the existing CreateNode/NodeCreated contract. Each accepted request
  retains its original parent, name, position and continuation payload; no import
  or annotation consumer matches a Petal/name tuple. Pen auto-creation retains its
  existing unique `pen-track:{n}` ID, and its unsent first-anchor continuation is
  retired on access loss. Creation and initialization-property results are owned
  independently; see ?host-path-outcomes for lifecycle and capacity rules.
- **Projection (petal terrain origin).** Per spec, points project through
  the *petal's* terrain origin, not an arbitrary bbox-center. The only
  already-resident state carrying a resolved terrain origin is
  `fe_terrain::petal_binding::ActivePetalTerrain` (inserted by
  `TerrainPlugin`, tracks whichever petal is currently the *active*
  viewport petal). `gpx_bridge.rs` reads it directly (no new DB round trip)
  when `active_terrain.petal_id` matches the import's target `petal_id`; if
  it doesn't match (importing into a non-active petal, or no terrain
  configured yet) it falls back to the bounding-box-center `Projection`,
  same as `fe-api/src/gpx.rs`'s HTTP endpoint. A dedicated
  `DbCommand::GetPetalTerrain` round trip was considered and rejected: its
  `DbResult::PetalTerrainLoaded` is already consumed by
  `terrain_bridge::bridge_petal_terrain`, which unconditionally reassigns
  the *active* petal's terrain — reusing it here for a possibly-different
  target petal would hijack the viewport's active terrain as a side effect.

**FR-3 point-count materialization (`node_placement_z_axis_20260713`).**
`advance_path_materialization` maps a track's `gpx_points` length to a render
kind via the pure `materialization_kind(len)`: `0` → `None` (render nothing),
`1` → `Node` (a plain, visible, selectable node), `≥2` → `Line` (the
`GpxTrackLine` polyline). A single point spawns `spawn_single_point_node` — a
small unlit `Sphere` tagged with both `SinglePointTrackNode { track_node_id }`
(reconcile key, mirrors `GpxTrackLine`) and `fe_ui::plugin::SpawnedNodeMarker`
(so the glb-mesh-picking AABB test selects it; `Mesh3d` auto-supplies the
`Aabb`). `petal_id` for the marker is best-effort from `ActivePetalTerrain`
(same assumption as AnnotatePoint / `resolve_projection` — selection only needs
`node_id`). The `None`/`Node`/`Line` reconcile is factored into ONE helper,
`reconcile_track_render`, called by BOTH render paths so their spawn decisions
can never diverge:

- **Petal-load path** — `advance_path_materialization` calls it on each
  `NodePropertiesLoaded` with `force_line_redraw = false` (a matching line from
  earlier in the same batch is left alone); `NodeDeleted` despawns both a line
  and a single-point node for the id.
- **Live-edit path** — `persist_and_render_points` (driven by Pen append /
  remove / move / Ctrl-height, via `drain_path_ops` and `advance_path_edits`)
  calls it with `force_line_redraw = true` (an existing `GpxTrackLine` is
  despawned + respawned to force `render_gpx_tracks`, which only rebuilds meshes
  `Without<Mesh3d>`, to redraw). This is HIGH-1's fix: previously the live path
  went through `spawn_track_route` alone, which only handled the `Line`/`None`
  cases and never touched the single-point node — so 0→1 didn't render live,
  2→1 vanished the track, and 1→2 leaked a stale node. All four now reconcile
  live: 0→1 spawns the node, 1→0 despawns it, 2→1 tears the line down + spawns
  the node, 1→2 despawns the node before spawning the line (no duplicate).

Because `advance_path_edits` reconciles to the post-edit count while
`advance_path_materialization` may also see the same seed-read
`NodePropertiesLoaded` (pre-edit count, differing by ≤1), the shared helper's
`is_none()` spawn guards + `force_line_redraw = false` on the petal-load side
keep the double-processing idempotent (neither double-spawns; deferred
`Commands` from one aren't visible to the other in-frame, but the count-kind
boundaries never collide on a "spawn-absent" decision). To thread the reconcile
into the live path, `drain_path_ops` / `advance_path_edits` gained the
`single_nodes: Query<(Entity, &SinglePointTrackNode)>`, `ResMut<Assets<Mesh>>`,
`ResMut<Assets<StandardMaterial>>`, and (for `drain_path_ops`)
`Res<ActivePetalTerrain>` params the reconcile needs; all four path systems live
in one `Update` tuple so Bevy serializes them on the shared `Assets`/`TrackRouteMap`
`ResMut` access (a scheduling constraint, not a conflict). `spawn_track_route`
now assumes `points.len() >= 2` (its only callers — the `Line` arm of the helper
and the import path — both guarantee it). The user's "z-axis" height edits (FR-1)
persist through the `gpx_points` encoding's unchanged 4 leading slots; since
`pen_curve_tool_20260722` Phase 1 the encoding is the mixed 4/12-slot form —
see **§gpx-points-wire-format** below.

**§gpx-points-wire-format (pen_curve_tool_20260722 Phases 1+3).** The
`gpx_points` node property is a positional JSON array with MIXED row lengths —
no version field, no object wrap, no migration pass:

- **Row layout.** A plain legacy corner (`TimestampedRoutePoint::
  is_plain_corner()`: no handles + `CornerKind::Corner` + `smoothness <= 0`)
  encodes as the compact 4-slot `[x, y, z, t]`; any anchor carrying curve data
  encodes as the 12-slot `[x, y, z, t, inx, iny, inz, outx, outy, outz,
  corner_code, smoothness]` (encoder `route_points_to_json`). Handles are
  RELATIVE meter offsets from the position; `corner_code` is
  `CornerKind::to_code` (0 = Corner, 1 = Smooth, 2 = Symmetric).
- **Only 4 or 12 — never an intermediate length.** The encoder emits exactly
  one of the two forms per row, and a decoded legacy 4-slot row re-encodes as
  the identical 4-slot row (NFR-3 byte-identity: legacy polylines round-trip
  unchanged, decode to all-corner/no-handle, and flatten/mesh/RDP/centroid
  exactly as before). An absent handle encodes as three `0.0` slots in the
  12-slot form; the decoder's `read_handle` treats an all-zero vector as
  `None` (zero == absent), so the two representations are one state.
- **Decode defaults + the Open-Q6 guard.** `json_to_route_points` reads slots
  4-11 with `.get()/unwrap_or` defaults (`None` handles / `Corner` / `0.0`),
  so short and legacy rows are backward-compatible for free; a row claiming
  Smooth/Symmetric with NO decoded handles is normalized to `Corner`
  (partial-row guard — prevents silent state drift). fe-ui's twin decoder
  (`fe-ui/src/gis/query.rs::decode_gpx_points`) must stay slot-compatible;
  the two `CornerKind` enums stay separate local twins by design (NFR-4),
  talking only through `corner_code` floats.
- **Phase 3 ops.** `PathOp::{AppendSmoothPoint, SetAnchorHandles,
  SetAnchorCorner}` extend the FR-7 persistence path. `drain_path_ops`:
  `AppendSmoothPoint` mirrors `AppendPoint`'s direct-mutate vs seed-read
  split (including the seed-pending dedup); the two `SetAnchor*` ops mirror
  `MovePoint`'s buffer-first split. `advance_path_edits`' append arm is
  unified over BOTH append variants, and after ANY deferred reply
  `drain_queued_appends` hoists all queued append-variant reads past in-place
  entries (Move/SetAnchor*/Annotate/Export — order-safe: appends only touch
  the list tail), stopping only at the index-shifting `RemovePoint` (never
  seed-deduped, so its own reply resumes the drain) — appends are seed-deduped
  to one read, so a reply-per-entry drain would strand them; the deferred
  `SetAnchor*` arms keep
  `in_flight_points` in sync and persist through the existing
  `persist_and_render_points` (live re-render for free). `smooth_anchor_point`
  widens the fe-ui f32 payload to the f64 route point;
  `apply_anchor_handles` mutates ONLY the bezier fields (position/timestamp/
  corner untouched). `MovePoint` needs no handle rewrite — relative offsets
  ride the anchor. Render + pick both flatten the decoded anchors through
  `fe_terrain::mesh::curve::flatten_route` (`render_gpx_tracks` and
  `track_pick_shape` — see `fe-terrain/src/mesh/AGENTS.md` §curve), so the
  clickable polyline is the visible curve.

**§track-styling (track_styling_20260713).** Per-track color / thickness /
visibility and zone fill, edited in the Paths tab, persisted as `gis.track.*` node props
(`gis.track.color` = `#rrggbbaa` hex, `gis.track.width` = number,
`gis.track.visible` = bool, `gis.track.closed` = bool,
`gis.track.fill.color` = `#rrggbbaa`). `style_from_properties(&Value) -> TrackStyle` (pure,
unit-tested) parses them field-by-field, each falling back to its default on
missing/invalid input (FR-4, never panics). `advance_path_materialization` now
also threads `ResMut<TrackStyleMap>` + `Res<DbCommandSender>`:

- On a track's `NodePropertiesLoaded`, it refreshes `TrackStyleMap[node_id]`
  from the style props BEFORE reconciling. If the style actually changed and a
  `GpxTrackLine` already exists, it despawns every matching line so the reconcile
  respawns it `Without<Mesh3d>` and `render_gpx_tracks` rebuilds the ribbon with
  the new color/width (its build is gated on `Without<Mesh3d>` — same
  force-redraw mechanism the point-edit path uses).
- A live style edit lands as `NodePropertySet { key }` (no property bag), so a
  `gis.track.*` key triggers a `GetNodeProperties` re-read → the arm above fires
  with the fresh values. Non-style keys don't trigger the round trip.
- `NodeDeleted` also drops the `TrackStyleMap` entry.

fe-ui side: `UiAction::PathSetStyle { track_node_id, color?, width?, visible?,
closed?, fill_color? }`
writes the changed keys via `SetNodeProperty` directly (mirrors
`PathAssetApply`; `actions::path::style_property_writes` is the pure, tested
`(key,value)` builder — only `Some` fields write, so an untouched control never
clobbers a stored value). The Paths-tab edit view seeds its controls from
`PathEditorState::edited_track_style` (a fe-ui-local `TrackStyleFields` mirror —
fe-ui must not depend on fe-terrain), populated at the same
`NodePropertiesLoaded` seam that seeds `points`.

`PathOp::DeleteTrack` tears down route and stamp projections only after the
`DeleteNode` command is successfully queued. Channel failure leaves projections
visible and reports an error. Generated unpromoted stamps follow the path
lifecycle; promoted stamp rows are preserved by the database and projection
cleanup never deletes `owning_path_id` records.
Successful enqueue also despawns every matching ribbon and
`SinglePointTrackNode` immediately.

**§track-picking (in-app fix).** A rendered track ribbon
(`fe_terrain::terrain_plugin::GpxTrackLine` + `Mesh3d`) wasn't viewport-selectable:
the fe-ui picker only iterates `SpawnedNodeMarker`, which `render_gpx_tracks`
can't attach (fe-terrain must not depend on fe-ui). `tag_track_lines_selectable`
(this crate — it sees both `GpxTrackLine` and `fe_ui::plugin::SpawnedNodeMarker`)
closes the gap: it queries ribbons `(With<Mesh3d>, Without<SpawnedNodeMarker>)`
and inserts a `SpawnedNodeMarker { node_id: track_node_id, petal_id }`, sourcing
`petal_id` from `ActivePetalTerrain` (the same pattern `spawn_single_point_node`
uses). Idempotent — the `Without<SpawnedNodeMarker>` guard means it tags each
ribbon once, the frame after `render_gpx_tracks` adds its mesh. Registered in
the same `Update` tuple as `advance_path_materialization`. The fe-ui side
(`viewport_pick::open_track_on_select`) then turns that selection into a Paths-tab
open — see `fe-ui/src/node_manager/AGENTS.md` §track-picking.

**INTEGRATION_REQUEST (gpx_pipeline_20260711, coordinator-owned `main.rs`):**
Register two new systems and one resource, mirroring the asset-bridge
wiring: `app.init_resource::<gpx_bridge::PendingGpxImports>();` and
`app.add_systems(Update, (gpx_bridge::drain_gpx_ops,
gpx_bridge::advance_gpx_imports));` — placed after `TerrainPlugin` is added
(needs `ActivePetalTerrain` to exist) and after fe-ui's `gpx_ops` module
lands (needs `PendingGpxOps`/`GpxImportStatus` to exist and be
`init_resource`'d, mirroring how `PendingAssetOps`/`AssetDownloadStatus` are
init'd in `fe_ui::plugin::GardenerConsolePlugin::build`). This binary cannot
compile until fe-ui's `gpx_ops` module exists — expected, per the coordinator's
final sweep.

**Residuals (not fixed by this pass, flagged for follow-up):**

1. No `parent_node_id` column on `node` — GPX waypoint "children" (and any
   future track/segment/trackpoint hierarchy) are property-linked
   (`gpx_track_id`) rather than a real FK. A future track should add the
   column + a `DbCommand::SetNodeParent`-style variant.
2. No per-trackpoint nodes are persisted (only the one track node's cached
   stats + standalone waypoint nodes) — `TrackRouteMap`'s
   `TimestampedRoutePoint` data (needed for `render_gpx_tracks`'s animated
   polyline) has no source yet. Whoever wires FR-3 rendering needs either a
   trackpoint-node persistence path added here, or a different route-data
   source (e.g. re-parsing the original GPX file referenced by the track
   node, if its source path/hash were cached).
3. `fe-api/src/gpx.rs`'s HTTP endpoints. **Export parent half CLOSED
   (FE-GPX-HTTP-EXPORT-PARENT, 2026-10-11):** `load_gpx_export_nodes` nests a
   waypoint under the track named by `properties.gpx_track_id` (the
   never-written `parent_node_id` read is gone); a waypoint whose track is
   absent/tombstoned stays a root instead of vanishing; and
   `fe_terrain::scene_nodes_to_gpx` emits every waypoint anywhere in the tree
   as a top-level `<wpt>` before the `<trk>`s (GPX 1.1 allows `<wpt>` only
   there). Tests: `gpx.rs::export_tree_tests`,
   `fe-api/tests/api_integration.rs::gpx_export_emits_track_and_both_waypoints_exactly_once`,
   `fe-terrain/tests/gpx_test.rs::waypoint_nested_under_its_track_is_emitted_once_at_top_level`.
   **Still open:** the exported `<trk>` is empty — `properties.gpx_points` is
   not synthesized into `<trkseg>`/`<trkpt>`. The GUI does this in
   `gpx_bridge.rs::points_to_export_children`; move that helper into
   fe-terrain when the first API consumer needs track geometry. With no
   `db_reader` the export returns an empty GPX. The import half
   (properties/hierarchy dropped, above) is unchanged.
4. `GpxImportStatus`'s exact field names are this worker's assumption,
   pending reconciliation with whatever fe-ui's `gpx_ops` worker actually
   ships (see above).

## §tileset-p2p — host wiring for P2P tileset streaming (`p2p_asset_streaming_20260718` S4)

fe-ui has no fe-terrain dependency, so this binary is the only place that can
assemble a completed P2P download into the hexon store or bundle an installed
tileset for seeding — fe-sync (the P2P transport) sees neither fe-format nor
fe-terrain either (root `AGENTS.md` §petal-map, fe-sync `AGENTS.md`
§tileset-transfer). `terrain_bridge.rs` is the bridge.

- **Download → install.** `fe-ui/src/terrain_map/events.rs::drain_tileset_events`
  turns a `SyncEvent::TilesetDownloadComplete` into
  `HexonOp::AssembleDownload { tileset_id, index_path, bundle_paths }` (no
  more main-thread install call). `terrain_bridge::drain_hexon_ops` spawns a
  **background thread** per `AssembleDownload` (`std::thread::spawn` — never
  Bevy's main thread, S4's NFR-4 invariant): it re-verifies the bundle hashes
  against the index (`fe_terrain::tiles::assemble_bundles_from_index_json`,
  defense in depth over S2/S3's own per-chunk check), installs via
  `HexonStore::install_tileset`, and `TilesetRegistry::reload`s it —
  `TilesetRegistry`/`HexonStore` are internally lock-protected (`RwLock`), so
  this is all safe off-thread. The ONLY step that needs the main thread is
  refreshing `ActiveDialog::HexonManager`'s installed list, so the thread
  reports back over `HexonAssembleResults` (a plain crossbeam channel
  resource) and `drain_hexon_assemble_results` does that one thing each
  frame. This is the regression fix for "closing the Hexon Manager stalls a
  download" (`fe-ui/src/AGENTS.md` §terrain-map) carried all the way through
  to the install step. It also settles fe-ui's `TilesetDownloads` tracker
  (`Complete`, or `Failed` with the install error — nothing else ever set
  `Complete`) and, after a successful install, sends
  `SyncCommand::CancelTilesetDownload` so the now-redundant
  `downloads/<id>/` cache is deleted (final review 2026-10-14: before, it
  stayed on disk forever and was re-reported at every start). A download
  that completed but was never installed (a restart in between) is handed
  back at start-up as `TilesetDownloadComplete` (fe-sync `AGENTS.md`
  §tileset-transfer), so it still installs.
- **Local file install (W2b F2, 2026-10-14).** `HexonOp::Install(path)` (the
  Hexon Manager's "Install local .hexon file" picker) used to `std::fs::read`
  + `TilesetRegistry::install` directly inside `drain_hexon_ops` — on the
  Bevy main thread, the one S4 invariant every OTHER install path already
  respected. It now mirrors `AssembleDownload`: `drain_hexon_ops` spawns a
  background thread that does the read + install and reports
  `HexonAssembleOutcome::Installed { tileset_id }` /
  `Failed { tileset_id, reason }` over the SAME `HexonAssembleResultSender`
  channel; `drain_hexon_assemble_results` refreshes the dialog once the real
  outcome lands (never inside `drain_hexon_ops`, where nothing has happened
  yet for this op). Sharing `Installed`/`Failed` with `AssembleDownload` means
  `drain_hexon_assemble_results`'s `CancelTilesetDownload` send is now gated
  on the id actually being a TRACKED download (`TilesetDownloads` contains
  it) — a local install, which started no P2P download, sends no spurious
  cancel for an id the sync thread never heard of. Test:
  `a_local_hexon_install_runs_off_the_main_thread_and_reports_its_outcome`
  (W3b fixup, AC-30d: the name claimed "off the main thread" but the test
  never checked — it now asserts `rx.try_recv()` is empty immediately after
  `app.update()` returns, before blocking on `recv_timeout`, for both the
  tampered and sealed legs; an inline main-thread install would already have
  queued its outcome by then). The check can only fail one way on correct
  code — a preempted test thread letting the tiny worker finish first — so a
  rare red there is a scheduling flake, not a regression; re-run before
  bisecting.
- **Install from registry (PAS-P5 S7b, A9).** `HexonOp::InstallFromRegistry { uri }`
  (queued by the Hexon Manager's "Install from registry" field, enabled only
  when `AppSettings.hexon_registry_url`/`FE_HEXON_REGISTRY_URL` resolves to
  something — `fe_ui::settings::resolve_hexon_registry_url`, same precedence
  shape as the relay spec) is drained on its own background thread,
  `terrain_bridge::install_from_registry`: `FetchStrategy::fetch_header`
  (`RequireSealed`, no pin — the operator's chosen registry IS the trust
  anchor, same model as `/download`) → `fetch_chunk` for every chunk a
  container names (or the one whole-archive chunk for a monolithic hexon),
  each one checked against the signed integrity root before it is ever
  written anywhere → `assemble_sealed_container` (container) or
  `HexonStore::install_tileset` directly (monolithic) → `TilesetRegistry::reload`.
  This is the FIRST production caller of `fe_hexon::partial::FetchStrategy`/
  `RemoteRegistryClient` (`fractalengine/Cargo.toml` now enables `fe-hexon`'s
  `remote` feature) — closes the "gate built but never wired" class of
  finding for the HTTP partial-fetch half (S5/S7a already wired the P2P half).
  Progress (`HexonAssembleOutcome::RegistryProgress`, header-resolved `0/N`
  through every chunk) and the terminal `RegistryInstalled`/`RegistryFailed`
  travel over the SAME `HexonAssembleResultSender` channel as
  `AssembleDownload`, and `drain_hexon_assemble_results` reuses the existing
  `TilesetDownloads`/Downloads-tab UI to show them, keyed by the REQUESTED
  uri (never routed through `SyncCommand::CancelTilesetDownload` — no P2P
  `DownloadTileset` was ever sent for it). **S8:** the container loop holds
  at most `fe_format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES` of chunk bytes
  (the core's `max_total_bytes` argument) and abandons the fetch past it, so a
  signed index naming many large chunks cannot exhaust memory before
  reassembly (test `install_from_registry_stops_a_container_past_its_byte_budget`).
  With `pin: None` here, a malicious registry still cannot replace an
  installed sealed tileset of another publisher: the store refuses it
  (fe-terrain `src/AGENTS.md` §chunked-containers "publisher continuity").
  **Bearer token (W2b F4, 2026-10-14):** `RemoteRegistryClient::new` took a
  hardcoded `None` token, so S7b could never reach a registry that set its
  own `FE_REGISTRY_TOKEN` (`fe-hexon-registry/AGENTS.md` §client gates ALL
  `/api/v1` reads on it). `drain_hexon_ops` now resolves the **env-only**
  `FE_HEXON_REGISTRY_TOKEN` var at this one seam
  (`resolve_hexon_registry_token`, a pure fn — table-tested without
  mutating the process environment) and threads it through
  `install_from_registry` into the client; there is deliberately no
  `AppSettings` field for it (RUNBOOK §5: secrets never in files) and it is
  never logged. Test (through the pure fetch core, against a token-gated
  in-process registry): `install_from_registry_requires_the_configured_bearer_token`.
  **W3b fixup (AC-30a, 2026-10-14):** that test drives
  `fetch_and_install_from_registry` directly, never `install_from_registry`
  itself (the function that actually owns the `RemoteRegistryClient::new`
  call this bearer token threads through), so it stayed green against a
  reversion of that one line. `install_from_registry_threads_the_bearer_token_to_the_client`
  drives the real seam instead — `install_from_registry` itself, the
  function `drain_hexon_ops`'s `InstallFromRegistry` arm calls in
  production — against the same token-gated in-process registry, both with
  and without the token.
- **Remove/SetSeeding also move off the main thread (W3b fixup, AC-30b,
  2026-10-14).** `HexonOp::Remove`/`SetSeeding` used to call
  `HexonStore::remove_tileset`/`set_seeding` inline inside `drain_hexon_ops`
  — the one install-path invariant (S4/NFR-4: never block the Bevy main
  thread on hexon-store disk I/O) this pair had not yet been brought under.
  Both take `HexonStore`'s single `write_lock`, the SAME lock a worker
  install can hold across a write + `sync_all` of up to
  `fe_format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES` (fe-terrain
  `src/AGENTS.md` §install-lock), so a main-thread Remove/SetSeeding click
  could block on that `std::sync::Mutex` for as long as the install's disk
  write takes. They now run off-thread like `Install`/`AssembleDownload`, but
  on ONE serial worker (`HexonMutationWorker`, a `Local` in `drain_hexon_ops`
  feeding a FIFO channel to a long-lived `hexon-store-mutations` thread), not
  a thread per op: per-op threads contend for `write_lock` in no fixed order,
  so a quick on→off seeding toggle could land off→on (store left seeding, ad
  re-published) or a Remove overtake an earlier toggle (W3b review). The
  worker reports
  `HexonAssembleOutcome::RemoveApplied { tileset_id, result }` /
  `SetSeedingApplied { tileset_id, enabled, result }` over the SAME
  `HexonAssembleResultSender` channel; `drain_hexon_assemble_results` applies
  the result (log, dialog refresh, and the P2P side effect below) once it
  actually lands, never inline. A seeding ENABLE whose write failed is never
  published (fail closed: the store still says "not seeding"); a disable or
  remove still sends `UnpublishTileset` even on failure (withdrawing is the
  safe direction). Tests: `remove_and_seeding_ops_report_through_the_worker_channel`,
  `queued_store_mutations_apply_in_click_order`,
  `a_seeding_enable_that_fails_to_persist_is_never_published`. Known gap
  (pre-existing): `spawn_bundle_and_publish` bundles on its own thread, so an
  on→off toggle faster than the bundling can still see `UnpublishTileset`
  reach the sync thread BEFORE the late `PublishTileset`.
- **Seeding → publish.** `terrain_bridge::spawn_bundle_and_publish(registry,
  sync_tx, tileset_id)` is the GUI's thread+channel wrapper around the ONE
  shared bundling implementation, `fe_terrain::tiles::bundle_for_seeding`
  (writes/reuses `<hexon store>/bundles/<id>/`, see fe-terrain `AGENTS.md`
  §bundles). `drain_hexon_assemble_results`'s `SetSeedingApplied { enabled: true, .. }`
  arm and `main.rs`'s startup loop (every tileset
  `TilesetRegistry::get_seeding_tilesets()` already lists) both call it;
  `SetSeedingApplied { enabled: false, .. }` and `RemoveApplied` send
  `SyncCommand::UnpublishTileset` directly (no bundling needed to stop
  seeding) — same split as before W3b, just applied once the worker's
  outcome lands instead of inline.
- **One seeding implementation, two thin wrappers.** `fractalengine-relay`'s
  startup seeding (its own `main.rs`, right after its `TilesetRegistry`
  loads) calls the SAME `fe_terrain::tiles::bundle_for_seeding` — the
  correctness-bearing part (Hilbert bundling, cache-hash reuse, disk writes)
  is not duplicated. Only the ~15-line "spawn a thread, send
  `PublishTileset`" glue is written twice, once per binary, because
  `fractalengine` is a bin-only crate (no lib target) `fractalengine-relay`
  could depend on — there is no third binary-agnostic home for it that both
  already share without adding a dependency edge neither needs otherwise. If
  a third caller needs this, promote the wrapper into a tiny shared crate
  then.
- **Startup publish ordering.** `main.rs` reads `sync_cmd_tx` for the startup
  seeding loop via a clone taken BEFORE `app.insert_resource(SyncCommandSenderRes(sync_cmd_tx))`
  moves the original — the sync thread is already spawned by then (channel
  sends just queue until it's ready), so a pre-existing seeding tileset gets
  re-advertised on every launch without the user re-toggling it.
- **PAS-P1V (relay posture, not tileset streaming).** `scripts/relay-health-verify.ps1`
  boots `fe-relay.exe` under `FE_SYNC_RELAY=default|http://127.0.0.1:3340|disabled`
  and asserts the resolved-mode log line, the one-time default-relay warning
  (Default only), and a relay-health transition line per mode — orthogonal to
  tileset streaming, but the same binary and the same `fe-sync` relay
  plumbing (`relay_config.rs`/`endpoint.rs`/`status.rs`).
- **Streamed (not-installed) tilesets (S7).** fe-terrain only knows a
  `tileset_hexon_uri` is "pending" (`composite.mark_streamed_pending`); it
  has no peer/provider knowledge (fe-terrain `src/AGENTS.md`
  §streamed-tilesets). `terrain_bridge::StreamedTilesetState` is the bridge:
  - `drive_streamed_tilesets` drains `fe_sync::StreamedTilesetEventBuffer` — a
    SIBLING of `fe_sync::TilesetEventBuffer` carrying a duplicate of just the
    advertisement/meta/chunk events, added specifically so this system never
    races `fe-ui`'s `drain_tileset_events`, which destructively
    `std::mem::take`s the WHOLE install-flow buffer every frame (fe-sync
    `src/AGENTS.md` §tileset-transfer). It records advertised tileset_ids,
    sends `SyncCommand::RequestTilesetMeta { peer_id: String::new(), .. }`
    for any uri the active composite is waiting on once some peer has
    advertised it (an empty `peer_id` lets the sync thread's own provider map
    resolve an address — this binary never needs one), parses a successful
    `TilesetMetaReceived.meta_json` and installs a `StreamedTileSource` via
    `composite.add_streamed_source`. A meta reply it handles must answer ITS
    OWN request (`state.meta_requested`) — the install flow's meta replies
    share the feed — and a uri the active composite still has pending (a
    stale reply after a petal switch is dropped). **W2b F1 (2026-10-14):**
    parsing + verification (`StreamedTileSource::from_meta_json`, which for a
    sealed `HeaderDoc` runs the same `verify_fetched_header` the install-flow
    arm uses) moves to a worker thread — resolved pin precedence (installed
    publisher, then TOFU `KnownTilesetPublishers`, same as the install flow)
    threads through so a header signed by another publisher, or a legacy
    unsealed reply once a pin is known, is refused. The system returns
    before the verdict; `StreamedMetaVerdicts` (mirrors `InstallMetaVerdicts`)
    carries the outcome back, applied by `drive_streamed_tilesets` itself at
    the top of its next invocation — re-checking "still pending" at apply
    time, since the petal may have changed while verification ran off-thread.
    The installed pin itself is resolved ON the worker thread too
    (`installed_publisher` opens and hashes the store's archive); only the
    TOFU map lookup happens on the main thread. While a reply verifies, its
    uri sits in `state.meta_verifying` and is not re-requested (otherwise the
    request loop at the end of the same frame re-sent `RequestTilesetMeta`
    every frame until the verdict landed); a verification that never reports
    expires into the backoff like a stalled request (W2b review).
    Tests: `a_streamed_header_signed_by_a_publisher_other_than_the_known_pin_is_refused`,
    `a_streamed_header_is_pinned_to_the_installed_publisher`,
    `an_unsealed_streamed_meta_is_refused_once_a_sealed_publisher_is_known`
    (all driven through advertisement → OUR request → reply, since an
    unsolicited reply is dropped before verification; each asserts the
    system returns before the verdict, no re-request while verifying, and
    the refusal lands in `meta_backoff`). A
    failed meta resolution backs off (`state.meta_backoff`, same 2 s→120 s
    curve) and is retried, never parked forever. It hands each
    `ChunkReceived` THIS NODE requested (`state.chunk_requested`) to a
    background thread (a size-capped read + `HexonTileSource::from_streamed_bundle`
    under the 64 MiB streamed decode budget — S8b, peer bytes — never the
    Bevy main thread) reporting back over the
    `StreamedBundleResultSender`/`StreamedBundleResults` channel (mirrors
    `HexonAssembleResultSender`/`HexonAssembleResults`).
  - `drain_streamed_bundle_results` applies finished decodes into the active
    composite (`insert_streamed_bundle`) — the one step needing the main
    thread, since decoding itself already ran off it. A FAILED decode (corrupt
    or over-budget archive) comes back over the same channel and backs off
    like a failed fetch; a bundle stays in `state.decoding` while its thread
    runs, so it is never re-requested mid-decode. It also evicts streamed
    bundles no chunk pinned within `STREAMED_BUNDLE_IDLE_GRACE` (10 s): one
    that lands after the ring moved on is never pinned, so `release` alone
    would keep it in memory forever (fe-terrain `src/AGENTS.md`
    §streamed-tilesets).
  - `request_demanded_bundles` reads fe-terrain's `TileBundleDemand` (the
    tile pointer-set, rebuilt every frame by `fetch_and_spawn_terrain_chunks`)
    and sends one `SyncCommand::RequestChunk` per not-yet-requested
    `(tileset_id, bundle_seq)`, bounded by `state.chunk_requested` so a still-
    pending bundle is never re-requested every frame. At most
    `MAX_STREAMED_BUNDLES_IN_FLIGHT` (8) bundles are requested or decoding at
    once — the sync thread spawns one fetch task per request — and requests
    go out with `try_send`, so a full command channel defers to the next
    frame instead of blocking the main thread. **S8b demand-storm
    guard:** a `ChunkFailed` for one of OUR requests starts an exponential
    backoff (`streamed_retry_delay`: 2 s doubling to 120 s) in
    `state.chunk_backoff`, cleared by that bundle's next `ChunkReceived`. A
    bundle that fails fast (hash mismatch from a hostile provider, no
    ledger) would otherwise be re-requested — and re-fetched — every frame.
- **Install binding (S8b).** `assemble_and_install` passes the REQUESTED
  `tileset_id` into `assemble_bundles_from_index_json`, which refuses a peer
  index naming any other id (fe-terrain `src/AGENTS.md` §bundles "Assembly
  trust").
  - All three register in `main.rs` alongside the existing
    `terrain_bridge`/`drain_hexon_*` systems; the channel is built the same
    way as the S4 assemble channel (`streamed_bundle_channel()`).

### Streamed request timeout and bundle checks (PAS-P5 S5, W1 residual)

- **In-flight timeout.** `StreamedTilesetState.meta_requested`/
  `chunk_requested` record when each request was sent. Each frame,
  `drive_streamed_tilesets` calls `expire_stalled_requests(now)`: a request
  unanswered for `STREAMED_REQUEST_TIMEOUT` (120 s, matching
  `FetchStrategy`'s default per-request timeout) frees its slot into the
  existing `meta_backoff`/`chunk_backoff`, so it is retried with backoff
  and never pins one of the `MAX_STREAMED_BUNDLES_IN_FLIGHT` slots forever.
  A reply that arrives after expiry is ignored like any unsolicited one. The
  disk copy, if any, makes the retry cheap. The timeout is host-side
  because the slots live here, not in fe-sync. Test:
  `a_never_answered_request_frees_its_slot_after_the_timeout_and_retries`
  (it fakes only the clock).
- **Bundle check before decode.** The `ChunkReceived` arm takes
  `composite.streamed_bundle_check(id, seq)` on the main thread, and the
  decode thread runs it on the bytes first. That check is a blake3 match
  against the signed (or legacy) index digest, or a whole-archive seal
  check for a sealed monolithic tileset (fe-terrain `src/AGENTS.md`
  §streamed-tilesets). An untracked tileset or unknown seq fails into the
  ordinary decode backoff.
- **Seeding.** `spawn_bundle_and_publish` and the relay inherit the PAS-P5
  seeder rule from `bundle_for_seeding` with no code of their own.
  Completed downloads with a sealed child 0 install through
  `assemble_sealed_download` (fe-terrain §bundles).

### Install-flow header verification — the two-phase gate (PAS-P5 S7a, 2026-10-14)

fe-sync's `DownloadTileset` resolves the root and child 0 ONLY, then stops —
`handle_download_tileset` never calls `TilesetTransfer::download` itself (see
fe-sync `src/AGENTS.md` §partial-fetch-p2p). This binary is the one place
that closes the loop, because it is the one place that both receives those
`TilesetMetaReceived` events AND depends on `fe_format`.

- **`drive_streamed_tilesets` grows a second branch.** A meta reply that is
  not one of its own streamed requests is an install-flow reply **only when
  fe-ui's `TilesetDownloads` tracks that `tileset_id`** (fe-ui's
  `start_download` seeds the tracker before it sends `DownloadTileset`).
  Anything else is ignored. Final review (2026-10-14): before this check a
  streamed reply that arrived after `STREAMED_REQUEST_TIMEOUT` fell into the
  install branch, was confirmed, and turned a streamed cache into a full
  download that fe-ui then auto-installed. fe-sync now refuses such a
  confirm as well (`TransferError::NotRequested`, fe-sync `src/AGENTS.md`
  §partial-fetch-p2p). The STREAMED path (above) and the INSTALL-flow path
  (here) share one system because they already share one event feed
  (`StreamedTilesetEventBuffer`).
- **Verification runs on a worker thread.** The installed pin opens the
  store's archive, and the header check hashes every header member, so
  neither runs on the Bevy main thread. The worker resolves the pin, calls
  `verify_install_meta_and_send`, which sends `ConfirmTilesetDownload` or
  `CancelTilesetDownload`, and posts an `InstallMetaVerdict` on the
  `InstallMetaVerdicts` channel. Next frame, `drive_streamed_tilesets` applies
  it through `record_install_verdict`. A verified publisher becomes the TOFU
  pin. A refusal is written onto the download's tracker as
  `Failed("Integrity check failed: …")`, which fe-ui never downgrades back to
  `Verifying`. Two replies for one never-pinned id that are verified at the
  same moment can both pass first contact. The store's publisher-continuity
  check is the backstop for that case. Test:
  `install_flow_replies_are_verified_off_thread_and_untracked_ones_ignored`.
- **`verify_install_meta(tileset_id, meta_json, ad_publisher_did, known_pin)`** is the
  pure decision core (no Bevy, no I/O — independently unit-tested): parse
  `meta_json` as `fe_format::HeaderDoc`, verify it with
  `fe_hexon::partial::verify_fetched_header` (the one header check
  `FetchStrategy` runs for every transport, so the P2P install path and the
  conformance-tested hosted path share it — final review) under
  `IntegrityPolicy::RequireSealed { pin }` (pin = `known_pin`, else
  `ad_publisher_did` when non-empty, else `None` — first-contact TOFU), and
  on success return the verified publisher DID plus the exact chunk hashes a
  sealed CONTAINER's signed `chunks/index.json` names (`None` for a
  MONOLITHIC archive, which signs no per-chunk table — the caller then uses
  whatever the HashSeq itself already resolved, since there is nothing more
  this layer can check before the whole-archive check S6's install runs).
  Every network path requires a seal (decision A2): an unsealed header is
  always `Err`. **S8:** the signed manifest's `hexon_id` must equal
  `tileset_id` — the pin store is keyed by tileset_id, so a header signed for
  another hexon would otherwise pin its publisher under the wrong id.
- **Pin precedence (S8).** The worker takes
  `installed_pin` = `HexonStore::installed_publisher(tileset_id)` (through
  `SharedTilesetRegistry`, off the main thread) and uses
  installed pin → in-memory TOFU pin → ad hint, in that order. Before S8 a
  fresh process had an empty `KnownTilesetPublishers`, so the advertiser's
  own DID became the pin even for a tileset already installed from another
  publisher (pin provenance gap); the store-side continuity check is the
  backstop either way. Tests: `an_installed_tileset_pins_its_publisher_over_the_advertisers_hint`,
  `a_header_signed_for_another_tileset_is_refused_and_pins_nothing`.
- **`record_install_verdict`** (with `verify_install_meta_and_send`; the
  test-only `verify_install_meta_and_confirm` composes both inline) records a first-successful publisher
  as this node's TOFU pin (`KnownTilesetPublishers`, in-memory, capped at
  `MAX_KNOWN_PUBLISHERS`) and sends `SyncCommand::ConfirmTilesetDownload` on
  success, or `SyncCommand::CancelTilesetDownload` on refusal — **no chunk is
  ever requested from this binary before a `ConfirmTilesetDownload` it sent
  itself**. A pin, once recorded, is never overwritten by a later header for
  the same `tileset_id` (a differently-signed "update" for an already-pinned
  id is refused, not silently trusted).
- **`ad_publisher_did`** (carried on `TilesetMetaReceived`, fe-sync
  `src/AGENTS.md` §partial-fetch-p2p) is the TOFU pin CANDIDATE only — this
  binary is what actually decides whether to trust it (by using it as `pin`
  only when `known_pin` is `None`). fe-sync guarantees it is always the SAME
  provider-map entry the resolved root came from, never a different one's
  claim; this binary still cannot assume the DID is honest on first contact
  — that is what TOFU means.
- **Tests** (`terrain_bridge.rs`, no P2P harness needed —
  `verify_install_meta` is pure): `seeder_with_an_unsigned_chunk_index_is_refused_before_any_chunk_request`,
  `chunk_not_in_the_signed_index_never_reaches_install`,
  `a_header_signed_by_a_publisher_other_than_the_pinned_one_is_refused`,
  `first_contact_tofu_pins_the_publisher_then_refuses_an_impostor`.

### Publisher pin chain (role propagation RP-4b, 2026-10-24)

Both meta-verification arms of `drive_streamed_tilesets` (install flow and
streamed path) resolve the pin a sealed header must be signed by through
`resolve_publisher_pin`: **installed > petal-authored > TOFU** (AC-47; the
install flow then falls back to the ad hint). The petal-authored pin is
`TerrainConfig::tileset_publisher_dids[tileset_id]`, read from
`ActivePetalTerrain` — trustworthy only because `SetPetalTerrain` is
Editor+-gated (RP-4) and every writer validates its `did:key` grammar
(fe-database `is_valid_inbound_petal_terrain`, fe-api
`validate_publisher_pins`, fe-terrain `terrain_config_from_petal_json`).

- **Recording** (`record_publisher_pin`): node-global sources (installed,
  TOFU, first contact) only fill an empty `KnownTilesetPublishers` slot, so
  TOFU never overrides. A header verified under a PETAL-AUTHORED pin is never
  recorded: `KnownTilesetPublishers` is keyed by tileset id alone (node-global),
  so recording a petal-scoped pin would let an Editor on petal X poison the
  TOFU fallback that petal Y trusts (pin poisoning, security review 2026-10-24;
  the petal pin is re-read from `ActivePetalTerrain` on every check anyway).
  Test: `a_petal_authored_pin_never_poisons_the_global_tofu_map`.
- **First contact on the streamed path** (AC-31 default): with no pin at
  all, the verified header's publisher becomes the TOFU pin
  (`verify_streamed_meta` verifies the header first so a container's
  publisher is known). Reversal: delete the write in the verdict drain.
- **Apply-time recheck** (`streamed_verdict_still_trusted`, T25): a verdict
  computed off-thread before a competing first-contact pin or a newly
  authored petal pin landed is refused at apply time, never added.
- **GUI authoring**: "Set petal map" on a sealed installed tileset writes its
  publisher (`InstalledTilesetDto::publisher_did`, read in
  `refresh_installed_dialog`) into the terrain JSON.
- **Tests**: `a_streamed_tileset_signed_by_a_publisher_other_than_the_petal_maps_pin_is_refused`,
  `a_second_streamed_publisher_for_the_same_id_is_refused_in_session`,
  `a_petal_authored_pin_overrides_an_earlier_first_contact_tofu_pin`,
  `the_publisher_pin_chain_prefers_installed_then_petal_then_tofu`.

## §window-chrome (frameless window + app icon, 2026-10-31)

- **Window.** `window_chrome::primary_window()` is handed to
  `fe_runtime::app::build_app_with_window` (which sets
  `WindowPlugin.primary_window`; plain `build_app` keeps Bevy's default window):
  title "FractalEngine", `name` "fractalengine" (X11 class / Wayland app id),
  `decorations: false`, min 960x600. Title bar, drag, resize grips and the close
  X live in fe-ui — fe-ui/src/ui_shell/AGENTS.md §window-chrome. The close X
  routes through `WindowCloseRequested`, the OS-close path; the GUI has no other
  exit handler, so the DB/sync threads see the same shutdown either way.
- **Icon art.** `assets/branding/icon.png` (256x256) + `icon.ico`
  (16/24/32/48/64/128/256): a six-petal flower in the theme's accent colours
  (`theme::BG_TOOLBAR` field, `ICON_TRACK` petals, `ICON_POINT` core).
  Regenerate with `python tools/branding/make_icon.py` (Pillow).
- **Runtime icon.** The PNG is `include_bytes!`-embedded and decoded with the
  `image` crate (0.25 — the line `bevy_image` uses). `set_window_icon` (Update,
  `NonSendMarker`) calls winit's `set_window_icon` through
  `bevy::winit::WINIT_WINDOWS`, retrying each frame until the winit window
  exists (winit creates windows after startup), then goes inert. The same
  pixels become the title bar's brand mark (`TopbarState::with_brand_icon`).
- **Exe icon.** `build.rs` embeds `icon.ico` with `embed-resource` (MIT; already
  in the graph via tauri-winres, so deny.toml is unaffected) as a `cfg(windows)`
  build-dependency, only when `CARGO_CFG_TARGET_OS == windows`. It writes a
  one-line `.rc` into `OUT_DIR` naming the icon by absolute path. A host without
  a resource compiler only gets a `cargo:warning` (the icon is cosmetic);
  non-Windows builds skip it entirely.

## §screenshot (window captures, 2026-10-31)

`screenshot_bridge.rs` is the Bevy end of fe-api §screenshot:

- Every capture goes through ONE helper, `pub(crate) capture_png(commands,
  screenshot, on_encoded) -> Entity`: spawn the given `Screenshot` (primary
  window, or a render-target image for §vision scene mode), and on
  `ScreenshotCaptured` run the one-shot `on_encoded` continuation with the PNG
  on the `AsyncComputeTaskPool` (observers are `Fn`, so the `FnOnce`
  continuation sits in an `Arc<Mutex<Option<_>>>` slot taken on first fire).
  It returns the `Screenshot` entity so a torn-down §vision job can despawn a
  capture that will never fire. `capture_primary_window_png` (F12 + GET) is a
  one-line wrapper over it.
- **Abandoned captures (QA-0080, 2026-11-06).** A torn-down input or vision
  job releases its unread `Screenshot` through `release_unread_screenshot`,
  never a bare despawn. Bevy marks the entity `Capturing` at render
  extraction, later inserts `Captured` from `trigger_screenshots` and despawns
  it in `clear_screenshots`; despawning it between those steps made that
  `insert` fail, and the default command error handler PANICKED (panic=abort:
  the app died right after a 202 `applied_unsettled` navigation with
  `capture_after`). The helper is a queued world command: despawn only while
  the entity is not yet `Capturing` (extraction runs after the main schedule,
  so there is no race); an in-flight one is left for Bevy, and the job's
  dropped result channel turns the late PNG into a WARN.
- **Two stages, two scope checks (agent_vision SEC-ADDENDUM).** `install`
  registers `drain_screenshot_requests` in `Update` (arrival check: an
  uncovered caller gets the uniform denial at once, the rest wait in
  `PendingScreenshots`) and `capture_pending_screenshots` in `PostUpdate`
  (frame-time check: `ViewScope::covers` runs AGAIN against the view open
  now, right before the frame is queued — the §vision "Scope" rule). A user who
  switches to an uncovered verse between the two gets the caller denied and no
  frame taken. Before the advocate pass (2026-11-01) one system checked at
  drain and captured, leaving that one-frame gap.
- While `fe_renderer::camera::CameraCaptureLock` exists (a §vision window job
  holds the camera at an agent's pose), the drain returns WITHOUT draining and
  the capture stage keeps its held requests: a plain GET is served unchanged
  the first frame after the lock lifts, so it never returns a half-posed
  frame. The capture stage's own lock check also covers a job that started in
  the same frame as the drain (its lock is a deferred insert, visible only in
  `PostUpdate`). The GET's own 5 s timeout still applies (a posed job's settle
  can outlast it — the agent then sees 504 and retries).
- `capture_pending_screenshots` serves every covered
  `fe_runtime::screenshot::ScreenshotRequest` (the API thread's bounded bridge,
  created in `main.rs` and handed to `ApiConfig.screenshot`) with ONE
  `Screenshot::primary_window()` capture — the swap-chain frame, so
  egui is included. Its `ScreenshotCaptured` observer clones the image and
  encodes PNG (`encode_png`: `try_into_dynamic` → RGB, alpha dropped as Bevy's
  `save_to_disk` does) on the `AsyncComputeTaskPool`, never on the frame loop,
  then replies to each request echoing its correlation id. The API side owns the
  5 s timeout; a late reply to a timed-out caller is dropped.
- `capture_refusal` answers a batch at once, without spawning a `Screenshot`,
  when there is no primary window or it is minimized (0x0 physical). Bevy's
  `extract_screenshots` marks a `Screenshot` `Capturing` on first sight and only
  ever re-extracts `Without<Capturing>`; if the render pass then skips it (no
  swap-chain view for the minimized surface) the entity, its observer and the
  held reply senders leak forever — and the agent waited out the full timeout
  for a refusal it can now read immediately.
- Known edge (not fixed): an API capture and an F12 press in the SAME frame
  spawn two `Screenshot`s of one window; Bevy despawns the duplicate, so one
  side gets "capture request dropped" (API) or no toast (F12). Merge the two
  into one per-frame capture if it is ever seen in practice.
- `capture_screenshot_on_f12` is the human path: F12 saves
  `data/screenshots/fractalengine-YYYYMMDD-HHMMSS-mmm.png` (UTC; `data/` is the
  app data dir beside the DB) off-thread, and `surface_saved_screenshots` toasts
  the absolute path or the failure. No API gate — it is the local keyboard.

## §vision (agent-vision capture orchestrator, `agent_vision_20261031` V5)

`vision_bridge.rs` is the Bevy end of fe-api §screenshot's `POST` (posed
capture) and §camera-state's `GET /api/v1/debug/camera`, over the separate
`fe_runtime::vision` bridge (fe-runtime §vision-bridge). `main.rs` creates the
bridge, hands the sender to `ScreenshotSurface::with_vision`, and calls
`vision_bridge::install(&mut app, rx)` — the same wiring the tests use.

**Schedule.** `drain_vision_requests` (Update, after
`fetch_and_spawn_terrain_chunks` so it reads the freshly grounded projection)
answers `ReadCamera` in the frame it arrives (any job state; `locked: true`
while a window job holds the camera) and resolves a `Capture`: every dynamic
refusal (`no_projection`, `tile_outside_tileset`, `geo_out_of_projection_range`,
`node_not_found`, `node_hidden`, `not_active_petal`, `petal_not_found`,
`nothing_to_frame`, `window_minimized`, `no_window`, `degenerate_camera`) is
decided there, before anything moves, and a second capture while a job exists
is `capture_in_progress` (fe-api also refuses it earlier via its 1-permit
slot). Then `(apply_vision_pose, advance_vision_job).chain()` runs in
PostUpdate **before `CameraUpdateSystems` and `TransformSystems::Propagate`**: the pose is written after
`orbit_camera_system` (private to fe-renderer, in Update) could have touched
the camera this frame, and before projection caching and propagation, so the
posed frame renders with the current matrix in the same frame.

**State machine** (`VisionJob(Option<ActiveJob>)`, single flight):

```
drain: resolve ──refusal──► reply 422/409 (nothing moved)
   │ ok (window+pose: insert CameraCaptureLock)
   ├─ focus.navigate ─► Navigating ─(target spawned ≥2 frames, or 300-frame cap)─► resolve
   ▼
Posing ─► Settling ─(frames ≥ settle [scene: ≥2 AND capture ring resident])─► Capturing ─► finish_job
              └─(600 frames or 20 s: capture anyway, settled:false)─┘          (PNG on compute pool)
any phase ─(24 s job cap)─► finish_job(capture_timed_out)
any phase ─(caller hung up)─► finish_job(caller_gone)
```

- **Window mode** snapshots `Transform` + the orbit controller (focus,
  distance, yaw, pitch, targets) + fov at pose time, writes the pose directly
  (`orbit_camera_system` is gated off by `run_if(camera_unlocked)` from the
  next frame), captures the primary window (egui included), then
  `restore: true` (default) puts the snapshot back bit-identically;
  `restore: false` re-derives the controller via `orbit_from_look_at` so the
  next drag continues without a jump (roll and |pitch| > 89° are dropped; the
  posed fov stays).
- **Scene mode** spawns a `Camera3d` + `CaptureCamera` + `StreamingFocus`
  (`order: -1`, the main camera's near/far with the pose fov and output
  aspect) rendering into a fresh `Rgba8UnormSrgb` image
  (`new_target_texture` + `COPY_SRC`), so the user's view is never touched and
  a minimized window still serves it (whether Windows keeps ticking the Bevy
  loop while minimized is the AC-11 in-app check). Settle waits at least 2
  frames (an image not yet on the GPU makes Bevy skip and leak the
  `Screenshot`) and until `CaptureFocusResidency` reports this camera's ring
  with 0 pending (fe-terrain §terrain_plugin "Multi-focus streaming").
- **`finish_job` is the ONLY exit**: it removes `CameraCaptureLock`, restores
  the camera (always, on any failure), despawns the capture camera, removes its
  image, despawns a capture that never fired, then replies. The 24 s job cap
  sits under fe-api's 5 s + 20 s reply timeout, so a wedged renderer yields a
  typed `capture_timed_out` with the camera restored, never a frozen camera.
- **Pose resolution** uses only `ValidCaptureSpec` floats (validate-once) plus
  resolver math; `f64`→`f32` casts are re-checked finite (`finite_vec3`) and
  every pose must pass `fe_renderer::camera::pose_transform` before anything is
  written. `geo`/`tile` read the RUNTIME `ActiveTileSource.projection`
  (grounded `origin_ele`), the effective world scale, and the resident
  `TerrainHeightField` for ground (else the grounded origin). `focus` uses
  `fe_ui::gimbal::subtree_world_aabb` over the node's subtree (a node with no
  `Aabb` yet frames its translation), keeps the main camera's view direction,
  and refuses an effectively-hidden node (the D-14 picker rule, plus the
  entity's own `Visibility::Hidden`); a petal focus unions its visible nodes
  and falls back to the tileset bounds.
- **`navigate: true`** (window only, same verse only) switches fractal/petal
  via `NavigationManager`, then waits for the target's `SpawnedNodeMarker`. It
  is undone afterwards like the camera, unless `restore: false` succeeds (see
  "Navigation restore" below).
- **Gizmos**: one PostStartup pass (`isolate_gizmos_from_capture_cameras`) puts
  every `GizmoConfigStore` group on render layer 1 and gives the main camera
  layers 0+1, so scene captures (default layer 0) carry no grid/axis/gimbal
  overlays while the user's view is unchanged. Nothing else in the workspace
  uses `RenderLayers`; a gizmo group registered after PostStartup would render
  into scene captures.
- **Known limits**: node residency (fe-ui `residency_ledger`) follows the main
  camera only, so a scene capture far from the user's view shows terrain but
  may lack nodes not resident there. The V1 `ResolvedPose` has no field for
  tile `coverage: "unbounded"` / out-of-served-zoom notes, so those are not
  reported.
- **Scope confinement (SEC-ADDENDUM, enforced HERE).** Every request carries
  the caller's token scope. `ViewScope::covers` (a `SystemParam`; the pure rule
  is `scope_covers_view`) is THE one gate: the scope must cover the ACTIVE
  verse (fe-policy `scope_contains` against `VERSE#<active_verse_id>`; nothing
  open = denied) **and every petal still rendered** — the terrain binding
  (`ActivePetalTerrain.petal_id` while it has a config) and every
  `SpawnedNodeMarker`'s petal. A rendered petal that is not the active one must
  sit in a covered verse; an unknown one denies. Why (critic pass 2026-11-01):
  terrain is re-bound only when the NEXT petal's `PetalTerrainLoaded` arrives
  and nothing clears it on a verse switch (`navigate_to_verse` clears the petal,
  so no load is ever sent), so after the user leaves verse C for verse A, C's
  map, tracks and overlays stay on screen and in `ActiveTileSource` until an A
  petal with its own terrain is opened. Checking only the cursor let an A token
  capture C's map, pose a scene camera anywhere over it (streaming more of it)
  and read C's projection through `GET /camera`. Nodes despawn a frame late
  (deferred commands), which the marker half covers. It is checked when a
  `ReadCamera`/`Capture` arrives, and again right before the frame is queued,
  so a user switching to an uncovered verse mid-settle yields `denied()` and no
  frame. The plain GET path applies the same two checks (arrival in
  `drain_screenshot_requests`, frame time in `capture_pending_screenshots` —
  §screenshot). Flow tests: `a_view_still_rendering_an_uncovered_verse_is_denied`,
  screenshot_bridge `a_map_still_bound_from_an_uncovered_verse_refuses_plain_captures`.
  The root cause (no terrain clear on leaving a petal) is left alone here
  because a clear-then-load would flash the map on every same-verse petal
  switch; revisit if a "clear terrain when no petal is active" rule lands.
- **Abandoned callers.** A request whose API caller already hung up (client
  disconnect drops the handler future, or the reply timeout fired) has a
  closed `reply_tx`: the drain drops it without moving anything (both bridges),
  and a running job whose caller leaves is torn down the next frame
  (`caller_gone`), so the user's camera is never held for a reply nobody reads
  — and fe-api's single-flight permit (released when the caller left) never
  meets a still-running orphan job as `capture_in_progress`. `authorized_target` resolves a `focus`
  target to its verse (hierarchy tree, else a marker spawned in the active
  petal) and returns `None` for unknown OR uncovered, which is one uniform
  `denied()` with no existence oracle. Cross-verse `navigate` is refused, and
  `navigate_to_petal` itself returns `false` rather than switch verses.
- **Navigation restore.** A navigating job snapshots the
  verse/fractal/petal cursor (`NavSnapshot`), and `finish_job` puts it back
  through `NavigationManager`'s own methods on failure, timeout or
  `restore: true`, by the same rule as the camera. Only a successful
  `restore: false` capture leaves the user in the target petal. The snapshot
  is put back only while the cursor is still on the petal the job navigated
  to (`NavSnapshot.navigated_to`): navigation is not locked during a job, so a
  user who moved on meanwhile (even to another verse) is never yanked back.
- **Humble Object.** Box framing (`fe_renderer::camera::frame_world_box`) and
  the tileset-bounds union (`fe_terrain::camera_framing::union_bounds`) are
  pure, table-tested helpers in their owning crates; this module only gathers
  ECS inputs (`frame_box` passes the view's direction + FOV) and applies the
  result. No framing formula is duplicated here.
- Every started capture logs one INFO line (correlation id, mode, pose kind).

## §agent-input-surface (synthetic input, `agent_vision_20261031` I4 = AIS S4)

`input_bridge.rs` is the Bevy end of fe-api §agent-input-surface
(`POST /api/v1/debug/input`, MCP `inject_input`), over the separate
`fe_runtime::input` bridge (fe-runtime §input-bridge). `main.rs` creates the
bridge, hands the sender to `ApiConfig.input` (`InputSurface::new(
input_allowed_from_env(), Some(tx))` — OFF unless `FE_API_INPUT_ALLOW=1`, the
bridge is wired either way exactly like screenshot), and calls
`input_bridge::install(&mut app, rx)` — the same wiring the test uses. The
relay passes `InputSurface::disabled()` (no window).

**Translation: winit-shaped messages, not resource pokes.** Each command
becomes the same messages winit writes — `CursorMoved`, `MouseButtonInput`,
`MouseWheel` (line units), `KeyboardInput` — so `bevy_input` builds
`ButtonInput<MouseButton>`/`ButtonInput<KeyCode>`/`ButtonInput<Key>`/
`AccumulatedMouseScroll` from them and bevy_egui 0.39 turns the very same
messages into egui events. Synthetic input is indistinguishable from real
input to every downstream system, and never becomes OS-level input.

**Clipboard isolation** (critic-input C1 and INPUT-ADDENDUM 8). bevy_egui's default
`manage_clipboard` feature turns a Control/Super chord on C/X/V into an
arboard read (paste) or write (copy/cut) of the HOST's OS clipboard — so a
synthetic `[ControlLeft, KeyV]` into a text field, followed by
`capture_after`, would exfiltrate the local user's clipboard, and Ctrl+C would
overwrite it. `plan_batch` therefore refuses (`clipboard_chord_refused`, 422,
before anything is injected) a `key_press` of `KeyC`/`KeyV`/`KeyX` and any
`text_entry` while a synthetic Control/Super is held (a one-character run is a
key chord to bevy_egui, `egui::Key::from_name`). Residual: a real Control key
the local user is physically holding still chords with a synthetic `KeyV`
(bevy_egui's `ModifierKeysState` merges both); that is the user's own action.
The input bridge also latches `SyntheticClipboardGuard` for every frame an
injection touches, including settle/capture and the final success/abort frame.
`suppress_synthetic_clipboard` runs after `EguiPostUpdateSet::EndPass` and before
`ProcessOutput`, removing both `CopyText` and `CopyImage` commands from
`EguiFullOutput` before bevy_egui can write to the OS clipboard. Suppression
audits only the command count, never clipboard payload. Copies made by ordinary
user interaction in idle frames retain their normal platform behavior.

Other UI-level effects (a click on the title bar's close button exits the app;
a click on a "Browse…" button opens a native `rfd` dialog that synthetic input
cannot drive and that blocks the frame loop until the user closes it — the
job then replies `input_timed_out`) are inside the "full remote control"
posture `INPUT_GATE.enabled_warning` names, not an escape.

**Schedule (verified against the pinned bevy_egui 0.39.1 / bevy 0.18.1).**
`drive_synthetic_input` runs in `PreUpdate` `.before(InputSystems)
.before(EguiPreUpdateSet::ProcessInput)`. bevy_egui configures
`EguiPreUpdateSet::ProcessInput.after(InputSystems)` and reads Bevy messages
in its `EguiInputSet::ReadBevyMessages` subset, so messages written before
`InputSystems` reach both `bevy_input` and egui in the frame they are
written. `restore_real_cursor` runs in `PostUpdate`; the clipboard sink guard
runs between egui EndPass and ProcessOutput.

**One frame per step** (planned up front by the pure `plan_batch`):

| command | frames |
|---|---|
| `pointer_move` | cursor |
| `pointer_click` | cursor · press · release (protected Join exception below) |
| `pointer_drag` | cursor(from) · press · cursor(to) · release |
| `key_press` | press · release (modifiers: see below) |
| `text_entry` | ONE frame: each control-free run is one `Key::Character(run)` press+release (egui reads it as `Event::Text`), `\n`/`\r\n` → Enter, `\t` → Tab |
| `scroll` | wheel |

- **Pointer start.** Ordinary clicks and drags position in a separate frame before
  pressing, within the same `ActiveInjection`. egui 0.33.3 permits a click-only
  child button and a drag-only ScrollArea background to share the press. A
  same-frame teleport/press therefore lets the background subtract the unrelated
  prior-cursor delta from its retained offset. The positioning-only frame makes
  press-time delta zero; subsequent declared drag movement remains ordinary input.
  Cursor restoration, clipboard suppression, whole-batch preflight, the 64-command
  limit and the 4.5-second wall-clock cap stay in force. The longest ordinary
  64-drag plan is now 256 input frames; slow/long requests can still time out.
  The existing populated-area journey seeds its offset once, checks exact-row
  clicks through settle and modifier release, and checks declared drag displacement.
  **Exception:** the protected Join lease retains same-frame cursor/press and its
  fixed five-frame single-chunk capsule. Its secret insertion, focus, geometry,
  revision and submit guards are indexed by those frames in
  `fe-ui/src/invitations/join_input.rs`; none change here. That modal capsule is
  separately guarded, and is not covered by ordinary ScrollArea timing acceptance.
- **Sticky modifiers.** `ShiftLeft/Right`, `ControlLeft/Right`,
  `AltLeft/Right`, `SuperLeft/Right` key presses stay down through the next
  non-modifier command and are released in a separate following frame (or at batch
  end), so `[ControlLeft, KeyS]` is Ctrl+S and `[ShiftLeft, KeyA]` types `A`
  (the "Sticky Keys" accessibility pattern; the v1 vocabulary has no explicit
  chord command). Logical keys follow a US layout; letters upper-case while
  Shift is held, other shifted symbols are not modelled.
  The release frame contains only modifier releases. bevy_egui 0.39.1 updates
  `ModifierKeysState` in InitReading before pointer events in FocusContext, and
  copies that state to RawInput. Releasing Ctrl alongside mouse-up clears it
  before `.clicked()` reads the chord. The same following-frame rule applies
  to keys, text, cursor-only moves and wheel input. Repeated pointer commands
  each need their own modifier prefix in the same batch. Modifiers also remain
  held through the new positioning frame. The protected five-frame Join protocol
  retains its existing frame counts and guard indexes.
  The existing input journey uses the dependency's actual
  `write_modifiers_keys_state_system` and `to_egui_modifiers`, then checks real
  egui release-frame clicks, every command type and abort cleanup.
- **Coordinates** are primary-window PHYSICAL pixels (AIS OD-4: the size a
  screenshot PNG reports), converted to logical for `CursorMoved`.
- **Settle + reply.** After the last input frame, `wait_frames` (validated
  ≤ 120 in fe-runtime) frames pass, then the reply goes out on the NEXT
  frame, so the input frame's `Update` systems have always run.
  `capture_after` queues one `Screenshot::primary_window()` through the
  shared `screenshot_bridge::capture_png` (sole owner unchanged) and replies
  with that PNG; a minimized window at capture time replies
  `window_minimized`.

**Refusals, all decided BEFORE anything is injected** (the batch is atomic at
arrival): `camera_locked_by_capture` (409 — `CameraCaptureLock` exists; checked
first, INPUT-ADDENDUM item 4), `not_found_or_denied` (404 — `ViewScope::covers`
is false, the §vision scope gate), `input_in_progress` (409 — single flight),
`no_window` (503) / `window_minimized` (422), then the planner's
Bevy-only checks (422): `pointer_out_of_window`, `invalid_key` (name not in
`resolve_key`'s table), `clipboard_chord_refused` (above), `unsupported_text` (control characters other than
`\n`/`\r`/`\t`), `scroll_out_of_range` (|delta| > 1000 lines — a finite but
absurd wheel value never reaches the camera's zoom math), and
`no_effective_input` (the plan has zero frames — a backstop: fe-runtime's
`validate()` already refuses the empty `text_entry` that is its only cause).

**No unchecked index in a per-frame input system** (INPUT-ADDENDUM item 7):
the app is built `panic = "abort"`, so a bad index is a remote kill switch.
The `Injecting` phase takes its frame through `frames.get_mut(next_frame)`
and moves to `Settling` once `next_frame >= frames.len()`, so an empty plan
settles instead of panicking.

**`capture_after` gating lives API-side.** fe-api's
`input::check_capture_after_gate` runs the screenshot gate before the
request ever reaches this bridge (fe-api §agent-input-surface); this side's
contribution is the per-frame view-scope check, which also covers the frame
the capture is queued in.

**Check at use, every frame a job runs** (INPUT-ADDENDUM item 2): caller hung
up → `caller_gone`; `CameraCaptureLock` appeared → `camera_locked_by_capture`;
the view stopped being covered (user switched verse) → the uniform denial;
`INPUT_WALL_CLOCK_CAP` (4.5 s, under fe-api's 5 s reply timeout) while still
injecting → `input_partially_applied` ("k of n input frames"). Past the cap
with every frame applied (only settle/capture outstanding) the job is NOT a
failure: it replies `Ok` with `settled: false` and no PNG (QA-0034/QA-0026 —
a terrain chunk rebuild froze the frame loop for ~4.5 s after the click
landed, and the old blanket `input_timed_out` 500 / API 504 invited a
duplicate world edit). The job shares an `InputProgress` with the API
(admitted, frames applied, settle frames) so even a deadline the frozen host
cannot answer is mapped truthfully by fe-api (fe-runtime §input-bridge
"Truthful late replies"). Every abort goes through `finish`, the single exit, which
writes a release for every button/key the job still holds (a half-done drag
never leaves `ButtonInput` stuck pressed), despawns an unread capture, sends
egui/picking a `CursorMoved` back to the user's real pointer, then replies.
Non-uniform failure messages say how many input frames had been applied.

**The cursor without an OS warp.** Picking, the gimbal and viewport clicks
read `Window::cursor_position()`, so the synthetic pointer must be on the
`Window`. But bevy_winit's `changed_windows` (in `Last`, `Changed<Window>`)
calls `winit_window.set_cursor_position` whenever the window's cursor differs
from its cache — writing the cursor normally would warp the user's real OS
cursor. So the override is written with `bypass_change_detection()` in
PreUpdate (the real value saved in `RealCursor`) and put back the same way in
PostUpdate, before `Last`: every PreUpdate/Update/PostUpdate reader sees the
synthetic pointer while a job runs, and the window/cache pair bevy_winit
compares never diverges because of us.

**Idle cost**: one empty `try_recv`, then return (no allocation); the
PostUpdate restore is one `Option` check.

Flow test: `synthetic_input_replays_as_real_bevy_input_and_refuses_at_arrival_and_at_use`
(one journey over a real `InputPlugin` App — clicks, settle, refusals, chords,
text, drag, scroll, single flight, mid-batch scope loss and capture lock,
wall-clock cap, `capture_after` PNG equality with a direct encode, abandoned
caller). True input-to-pixel confirmation is the standing user-gated in-app
verify (AIS D-AIS-7).

## §qa-dev-token (headless agent bootstrap, `FE_API_DEV_TOKEN_OUT`, 2026-11-01)

### QA storage boundary

The launcher sets `FE_QA_STORAGE_ROOT` to its confined absolute run directory.
Startup validates it before any relative state, uses it instead of packaged
CWD, overrides blob/P2P/hexon paths under it and skips the legacy user-blob
migration. Settings load from its `config/` directory. The node key uses its
`identity/` directory; all other secrets use a run-local persistent backend, initialized
with that node key. The QA branch never constructs the OS-keystore backend
and does not migrate a real user's identity. Without this explicit variable,
desktop storage/keystore behavior is unchanged. See tools/qa/AGENTS.md for
stage fingerprints, process identity and conservative cleanup rules.

The screenshot/camera/input surfaces all need an **Owner** API token scoped to
the active verse, and the only in-app mint door (Inspector → API Access →
`DbCommand::MintApiToken`) caps `max_role` at `manager` by design. A headless
QA agent therefore had no way in. `FE_API_DEV_TOKEN_OUT=<path>` is the opt-in
bootstrap: the GUI writes, and keeps rewriting, a token file the agent reads.

```json
{ "api_base": "http://127.0.0.1:8765",
  "tokens":  { "<verse_id>": "<jwt>", ... },
  "roles":   { "<verse_id>": "owner" | "manager" | "editor" | "viewer", ... },
  "client":  "did:key:z6Mk...",
  "expires_at": "<RFC 3339, every token valid at least until then>" }
```

- **Wiring.** `main.rs` reads the env once (`fe_api::dev_token::dev_token_out_from_env`,
  WARNs with the path when set) and, only then, hands
  `fe_api::dev_token::spawn_dev_token_writer` a seed-copy of the node keypair,
  a clone of the DB thread's shared handle (role reads only), the API's
  `listening_tx` notification (`ApiConfig::listening_tx`, the ACTUAL bound
  address) and a verse-id channel. `qa_dev_token.rs` feeds that channel:
  the full id list on the first frame and again whenever the SET of
  `VerseManager.verses` ids changes (create, join, delete), and again when the
  resolved `LocalUserRole` VALUE changes — a joined verse can enter the tree
  before its role statement replicates, and the writer resolves roles only at
  write time, so without this the file kept the default-access role forever
  (QA-0142 run). Allocation-free on every other frame. The writer coalesces queued lists to the newest and
  rewrites the whole file each time (fresh 8 h tokens for every verse).
- **Never above the node's own role.** Each verse's token is minted at
  `role_manager::resolve_role(local_did, "VERSE#<id>")`: Owner for a verse
  this node created (what scenario-created verses are), the visitor role for a
  joined one, and no token at all for `none`. The file's `roles` map says
  which; only `owner` entries can capture or inject. Same rule the Inspector
  door enforces ("requested role must not exceed caller's own role").
- **One minting implementation.** Tokens come from
  `fe_identity::mint_client_api_token` (the signer the Inspector door uses),
  with one fresh client DID per process (`fe_identity::new_client_did`), so an
  agent's uploads share one owner bucket across rewrites. They are NOT recorded
  in the `api_token` store: the 8 h TTL bounds them, they never show in the
  Inspector's token list, and they cannot be individually revoked from it.
  They obey the stored authentication generation, so terminal reset retires
  old dev tokens without rotating the node key.
- **Loopback only, fail closed.** `loopback_api_base` refuses any bound
  address whose IP is not loopback (`0.0.0.0`, a LAN IP, `[::]`, an
  IPv4-mapped address): the writer logs an error and exits, nothing is minted
  and nothing is written for the life of the process. Today the GUI binds
  `127.0.0.1:8765` hard-coded; the check guards a future configurable bind.
- **Owner-only file, atomic.** `fe_identity::file_fallback::replace_owner_only_file`
  writes a hidden sibling temp through `create_owner_only_file` (the identity
  key file's pattern: `create_new` + `0o600` on unix; on Windows created empty,
  given the protected owner-only DACL, THEN written; any failure deletes it)
  and renames it over the path. A reader sees the old file or the new one,
  never a partial or broadly readable one. No token is ever logged — the
  writer logs path, api_base, the verse→role map and `expires_at`.
- **Unset = no change.** No env var → no writer thread, no Bevy system, no
  `listening_tx`. The relay passes `listening_tx: None` and never emits.

Agent recipe: launch the GUI with `FE_API_DEV_TOKEN_OUT=<scratch>/fe-tokens.json`
(plus `FE_API_INPUT_ALLOW=1` to drive input), poll the file until `tokens`
holds the active verse, then `GET {api_base}/api/v1/debug/camera` with
`Authorization: Bearer <tokens[verse]>`; a scenario that creates a verse
re-reads the file for the new id.

Flow test (one journey, fe-api R9 batch):
`fe-api/tests/screenshot_test.rs::owner_capture_via_the_dev_token_file_returns_the_host_png_then_is_rate_limited`
— non-loopback refused with nothing written; one Owner token per owned verse,
a Viewer token for a visited verse, none for a `none` verse; owner-only on
disk (`icacls` oracle on Windows, mode bits on unix); a new verse rewrites the
file with no temp left behind; then the agent captures with the token read
from the file (Viewer refused, Owner admitted, rate limit).

## §qa-operator (multi-peer QA, 2026-11-06)

A joined Editor/Viewer node holds only its granted role token for a verse another node owns, so the
Owner-gated debug surfaces (camera, screenshot, ui-tree/state, logs, input, window, dialog answers)
could not observe or drive that window. When `main.rs` validates `FE_QA_STORAGE_ROOT` it latches
`fe_runtime::qa_debug::enable_qa_operator()`; the dev-token writer then also mints `operator_token`:
Owner max_role, scope exactly `QA-OPERATOR`, same signer/TTL/generation as the per-verse tokens.
`ViewScope::covers` (the one gate every debug bridge uses) admits that scope for whatever this window
shows, but only while the latch is on. Nothing else changes: the scope parses as no VERSE/FRACTAL/PETAL,
so every scoped REST/MCP route still denies it, invitation consume still requires its own verse scope,
and a verse-B token still never sees verse A. Without the QA root nothing is latched or minted.
Actions taken through it are ordinary local GUI input, gated by the node's real role (a Viewer's
Delete is disabled; its property delete is refused by the DB). One real widening (critic 2026-11-06):
the unscoped node-level routes gated only by a role floor (`POST /verses` Manager, hexon crate publish
Owner, the QA gates) now admit this node's local operator even when it owns no verse. They act on the
node's own registry/new verses, never on a verse it was granted, which is the same power its GUI user
already has; keep any future verse-data route behind `require_scope`. Journey:
`vision_bridge::tests::a_token_outside_the_active_verse_is_denied_capture_and_camera_read`.

## §peer-terrain (live peer map/earthwork, 2026-11-06)

A replicated `petal` row for the ACTIVE petal now re-fetches its terrain doc (fe-ui
`db_results` `ReplicatedRowApplied` arm), so a peer's map binding or earthwork region renders without
re-entering the petal. `terrain_bridge::bridge_petal_terrain` remembers the last applied
(petal, raw doc fingerprint) and skips an identical doc, so non-terrain petal writes (render distance,
rename, replica-open republish) never rebuild the terrain; leaving the petal forgets the key, so
re-entry always rebuilds. A `role` row likewise re-resolves the local role for the active scope, so a
peer's grant or revocation updates the role badge and edit gates live.

## §qa-debug / §ui-tree

`qa_debug_bridge.rs` forces egui AccessKit generation only for active OS
accessibility or the captured QA gate. It runs after Egui InitContexts and before
BeginPass, never resets Bevy's latched AccessibilityRequested flag, and leaves
bevy_egui's existing OS forwarding intact. The workspace enables bevy_egui's
accesskit feature for normal builds. With both OS activation and QA absent,
egui generation is disabled. No debug data is served unless the QA gate is on.

After ProcessOutput the host serves bounded requests from the primary context.
Tree flattening composes every ancestor affine transform, including egui's root
pixels-per-point transform; bounds use transformed bounding rectangles. Names
resolve label and labelled_by targets, including Label/TextRun values. Disabled
and hidden states inherit from ancestors. The output is in PHYSICAL screenshot
pixels, matching input_bridge; older plan wording calling these logical pixels
was incorrect. Tree flattening occurs only on an admitted request, not each
frame. OS tree updates are borrowed, never taken. A navigation signature sampled
before the pass must still match when replying, and ViewScope checks current
terrain/spawned-node ancestry before any reply.

Selection export reads NodeManager and the selected entity's Transform, verifies
SpawnedNodeMarker and current petal/fractal/verse membership, and never exposes
stale InspectorFormState buffers. Dialog titles use an explicit variant match:
Debug formatting would expose token/invite buffers. Password node and descendant
values are omitted; all other exported strings pass credential redaction.

`qa_log.rs` attaches its optional Layer to the existing startup subscriber.
Sensitive field names are wholly redacted; the runtime ring handles credential
patterns, truncation, bounds and verse generations. No token or mutable secret
store is exposed by these surfaces. QA startup also sets
WEBVIEW2_USER_DATA_FOLDER to the run's `webview` directory so stage reuse does not
share a browser profile between fixtures.

## ?qa-a11y-timing

`qa_a11y_timing::install` adds no system unless `FE_QA_A11Y_TIMING=1` and
`fe_identity::file_fallback::qa_storage_root` validates an explicit isolated
root. It never changes `AccessibilityRequested`, egui accessibility settings,
or the normal OS forwarding path. Synthetic-input-enabled runs produce an
incomplete report and cannot serve as the normal-OS baseline.

The collector uses the existing `FrameTimeDiagnosticsPlugin::FRAME_TIME` in
`Last`, deduplicating by the diagnostic measurement's timestamp. Its bounded
sequence is 10 seconds warmup, 20 seconds inactive baseline, baseline-ready,
45 seconds awaiting OS activation, 5 seconds discarded transition, and
20 seconds active. It requires at least 30 samples in each measured phase and
stops at 10,000 total samples. Preactivated accessibility, early activation,
changed observed conditions, absent diagnostics, or no OS activation never
produce a successful paired-comparison claim. A process stopped mid-run leaves
an explicitly incomplete last phase report.

All output names are fixed children of the validated root. The owner marker
and baseline-ready marker use `create_owner_only_file`; reports use
`replace_owner_only_file`. An existing owner marker refuses another collector
in that root, so use a fresh QA root for each paired attempt. The owner epoch
is repeated in reports and the ready marker; the read-only UIA helper compares
it to the recorded process creation time, not just its PID. Reports are
written only at phase boundaries, and the following diagnostic frame is
discarded to exclude output-write time. They contain frame wall time, counts,
activation flags and numeric/enum workload conditions, never UI tree strings,
tokens, invites or input field values. Update-node counts describe the current
AccessKit update, not the complete retained accessibility tree.

Recorded samples share one invariant condition snapshot: focus, physical
window size, window/UI scale, pixels-per-point, present/update modes, camera
transform/FOV, navigation hash and node count. These checks do not prove every
CPU/GPU workload is unchanged; fixture stability remains a runtime obligation.
`FE_QA_A11Y_CONTINUOUS=1` is an additional explicit controlled-workload option,
accepted only inside this timing opt-in and validated root. It selects
continuous winit updates before both phases and records that fact. Otherwise
the application's update policy is preserved. Never compare different gate
states or reset a user's latched accessibility request to manufacture a
baseline.

Timing summaries include count, mean, median, nearest-rank p95, range and
standard deviation; complete pairs include absolute and percentage deltas.
The metric is full application frame wall time including scheduling, vsync,
GPU waits and scene work. A refresh-capped plateau does not establish zero
accessibility CPU cost. This collector is source preparation until built and
verified through the normal owned-window OS accessibility path.

NEEDS-WIRING: the QA launcher currently enables synthetic input unconditionally.
A later launcher owner must add a recorded gate-off option and explicit timing
opt-ins while preserving stage provenance, root isolation and process identity.
The timing collector adds no HTTP endpoint and does not alter `launch.ps1`.

## Terrain assignment ordering

The DB-result terrain bridge is ordered before the terrain plugin's
`ApplyAssignments` set. Reports requested from PostUpdate therefore see the
latest forwarded configuration on the next Update rather than an older
proposal list. This is a host scheduling boundary; UI and renderer do not gain
a dependency on fe-terrain's configuration type.


## Plugin scene bridge

plugin_bridge installs the real PluginHostPlugin plus its SceneBackendFactory
after core DB handles exist. A trusted host binds a manifest to an instance via
PluginContexts::bind; Wasm managers receive that resource with with_contexts.
The factory performs a typed DB scope handshake once and pins full ancestry,
then injects bound NodeReader/TransactionSink implementations. Every read/write
returns on its own embedded channel; it does not depend on a Bevy frame or share
API pending requests. The DB resolves current local identity and rechecks role,
capability and stored ancestry at use.

The worker waits for a persisted receipt, with explicit enqueue refusal versus
unknown commit outcome on transport timeout. Do not automatically retry the
latter. Binding and callbacks belong on the plugin worker, not the render loop.
No automatic package loading or new rule engine is implied by installing the
host; missing manifest admission remains fail closed. This slice adds only a
local workspace dependency on fe-plugin, requiring the integrated Cargo lock
refresh by the sole builder.


The scene factory attaches the actual context's shared PluginLease before its
scope handshake. Requests keep that lease through the DB queue, and the adapter
checks it again before delivering read payloads. Already admitted commits retain
their actual receipt after revocation; transport loss remains an unknown outcome.


## §qa-report-diagnostic

The existing argument-free Owner/input-opt-in ui-state projection includes one
terrain_diagnostic object. Existing ViewScope and same-frame navigation checks
run before projection. Diagnostic selection additionally requires its entity
marker, node ID and active verse/fractal/petal tree membership; there is no
all-object fallback or caller-supplied node/coordinate input.

Only that node's matching SceneAnalysisSnapshot bounds/classification/revision
are exposed, without its name or material JSON. Finite ordered positive AABB
extents are required. The matching ActivePetalTerrain contributes assignment
revision, enabled state and effective world scale. Runtime origin is read from
ActiveTileSource.projection, which grounding can elevate independently of the
stored configuration. Its horizontal origin must equal the active config;
grounding elevation may differ. Missing, mismatched, ambiguous or invalid inputs return
explicit unavailable/stale reasons, without another petal's IDs or geometry.

With valid scoped inputs there are exactly five pristine height_at reads: AABB
XZ center and four XZ corners, in world units. Each has a finite base_height or
null with missing/nonfinite status. Partial coverage is incomplete, never an
invented zero. No baked/composite proposal sampling, terrain mutation, forced
residency, registry access, arbitrary queries or additional transport is added.

sampled_at_frame labels the service-time projection. SceneAnalysisSnapshot is
throttled and exposes no source-update frame; the height field exposes no unique
residency revision. Assignment revision is not a height-field epoch. terrain_epoch_verified is
explicitly false even when all five samples are finite; available describes
measurement availability, not verified resource epochs or fresh report inputs.
This is not
a replay of any earlier report; pair with scene revision and stable assignment/
origin during live verification, preserving temporal uncertainty explicitly.
The existing grounded-camera journey carries diagnostic boundary cases; its
execution and the live overlap oracle remain pending the integrated sweep/stage.
## checked point Undo bridge

CheckedMovePoint uses the shared Undo reservation and an authorized current-state
resolution. It refuses unsettled deferred reads/track creation and queues reads
behind already-enqueued writes. Its before-state comes from persisted raw JSON,
not the optimistic in_flight_points cache. Only one point's xyz may change.

Matching receipt/reconciliation projections replace the in-flight buffer, route
map, line/single-point mesh and matching editor rows through existing helpers.
Checked moves do not call persist_and_render_points before confirmation. Ordinary
writes to the same track wait while a covered operation is pending, preserving
append order. Current-snapshot recovery also hydrates the existing path-stamp
cache. Segment/whole-path moves retain untracked behavior. The sole builder must
refresh the existing workspace ulid dependency edge added to fe-ui; this source
lane does not edit Cargo.lock or execute Cargo.


The checked point bridge receives the live navigation resource when committing a
read result and when draining projections. It cannot rely on the Undo observer
having run first. A changed verse/fractal/petal generation refuses the late write
and drops the old projection even if ActivePetalTerrain has not caught up. The
current typed node ID/petal is checked again before extracting gpx_points.

The existing Owner/scope/input-gated ui-state diagnostic counts queued and in-flight target Access work through UiManager::target_access_pending. It exposes only a pending boolean in ui_known, never roles, token/invite values or a new control route. The retired unrendered InviteDialog is no longer reported as an open window. DB/sync/terrain completion remains explicitly unknown.

## Checked point removal bridge

`CheckedRemovePoint` shares the existing checked-move reservation/read/commit path. Pending legacy
route writes must drain first, stale requests are discarded by editor generation, and the bridge
projects only the Undo controller's matching authoritative receipt. The complete persisted point
array is the CAS input; removal preserves every field of all surviving stored rows.

CheckedAnchorMetadata shares the exact checked route read/commit path with move/removal; its
original context remains inside Undo until the asynchronous read matches. No legacy index-only
SetAnchorCorner op is emitted by the context or ordinary Corner control after admission.


## Process logging owner

`main` installs the tracing registry with EnvFilter, formatting and optional QA-log layers before
starting settings, identity, database, network and sync services. Its runtime AppOptions explicitly
choose `LoggingOwnership::External` and confirmed window close. The runtime then omits Bevy's
LogPlugin while retaining the rest of DefaultPlugins, so startup and in-app events use one process
subscriber and QA capture remains installed. See `fe-runtime/src/AGENTS.md` Logging ownership.

## ?terminal-reset

`terminal_reset` owns the desktop maintenance state machine. A typed, confirmed local request transitions monotonically from Running to Quiescing, then Clearing and a terminal result. At the next First boundary the host replaces Update and EguiPrimaryContextPass with terminal-only work. This drops editor system locals, stops ordinary mutations and all stale result projections together, and retains Bevy input/render/PostUpdate/Last cleanup. PostUpdate report/portal helpers explicitly stop through TerminalMode. The maintenance renderer covers the prior scene and offers Close/manual reopen; there is no helper relaunch or partial service restart.

The supervisor retains API/network/sync/plugin and bridge stop/join ownership. The controlled API routes its sole direct append exception through the serial DB executor. It revokes plugin origins, stops credential publication, joins services, waits for finite terrain jobs, invalidates the dev-token file, and requires unique EntityStore Arc ownership before issuing reset proof. Failed ownership, shutdown, jobs, or deadlines prevent a clear. While waiting, bounded terminal drains discard old DB/network/sync/lifecycle/API/plugin/scene events so blocking result sends cannot deadlock joins or repopulate cleared state. API request owners are dropped rather than receiving invented success.

The final DB command is FIFO behind prior work and is the worker's last operation. Its receipt does not trigger LoadHierarchy or replica reopening. Success requires both the matching committed result and the actual DB thread join; delayed/lost results remain unconfirmed with no retry. A late matching receipt may clarify the stored outcome while the process stays terminal. HLC is process-global, so only a full manual reopen may start writers again. No identity, package, blob, or P2P file tree is removed.

Opt-in QA hosts emit credential-free, create-new phase receipts beneath the already captured run root. Receipts include the request, process ID, joined service names, remaining job count and DB result/join facts. They do not resurrect the stopped API. Capture the terminal UI through an already admitted capture before freeze, retain exact process ownership, and inspect the actual disposable store after close/reopen. This source contract is not executed reset acceptance.

API session shutdown and dev-token file invalidation accompany the durable credential-generation rotation in the reset transaction. Reopened REST and legacy WS admission reject pre-reset JWTs even when their signatures and Owner claims remain valid. Identity/signing keys are retained; token-row deletion alone is not the revocation mechanism. The source-authored lifecycle journeys still require the integrated execution and independent review gate before reset activation.

## §portal-host (portal_surface_20261107 P-S1/P-S2/P-S4/P-S6, swarm turn 2)

`portal_host/` hosts web bundles in the binary: `resolver.rs` (P-RESOLVE), `broker.rs` (P-BROKER), and `mod.rs`.
`mod.rs` holds the one `install()` call, the off-frame worker and the drains behind fe-ui's Repository section.
The specs are `docs/spec/portal-bundle/{admission,ipc-v1,ux}.md`.

**Merge prerequisite.** This tree compiles only after it rebases onto `track/extension-platform`. It names:
- `fe_plugin::{bus, BusHandle, PackageAdmission}`;
- `crate::extension_host::governance::ExtensionGovernance`;
- `fe_ui::extensions::consent`;
- the pinned DB commands `LoadExtensionGovernance` and `AppendExtensionAudit`.

**Install order.** `portal_host::install` runs after `WebViewPlugin` and after the extension host inserts `BusHandle` and
`ExtensionGovernance`, because it reads both at install time. Each missing piece is logged as an error and fails closed:
- no governance: the resolver judges through `NoGovernance`, so nothing is admitted and bundles render static only;
- no bus: there is no `PortalBrokerRes`, so the bridge answers every call `unavailable`.

The worker is an `OwnedService` in `maintenance_services`, so it stops with the other host services.

### Resolver (P-RESOLVE)

- **The chain.** It resolves node -> `portal.bundle_asset` -> `asset` row -> local blob, with an absent link
  answering `Unavailable`. Reads run on the caller's worker through a private current-thread runtime, and are refused
  on a tokio worker.
- **The kill list before bytes.** The blob hash is checked against the kill list before any byte is read (rules 3-4).
  The header is read only after the file's hash is verified, so it comes from bytes the list just cleared.
- **Bounded caches.** Node, petal and build caches hold at most 1 024 entries and evict the oldest; a miss fails closed.
- **The record mirror.** `learn_consent` writes a record (the digest is required), and `host/admission` events update its
  state. An `admitted` event alone never creates a record.
- **Policy per target.** Verse policy is evaluated per target with no manifest, so `ceilings.kinds` never applies to
  a web bundle (E10 default).

### Broker (P-BROKER)

- **The ceiling.** A binding's ceiling is the minimum of three roles: the bus role view, fe-ui's `LocalUserRole` at the
  active petal (Viewer until it resolves) and the viewer token. A bind below Viewer is refused, and the ceiling rides on
  `Bus::open_portal_client`, never on a `Caller` literal (steer 23:40).
- **The role source.** `NodeDidRoles` answers Owner for the node DID and None for everyone else. That is identical to
  `plugin_bridge`'s bus roles for the only DID a portal binding names (`bus.local_did()`), and it avoids reaching into
  that module's private role view.
- **Scope pinning.** The scope is pinned from `BrowserCommand::OpenBundle` (`track_portal_scopes`, before
  `apply_portal_admission`). It is event-driven: `VerseManager` mutates every frame, so there is no tree mirror. Write
  ancestry is pinned at Open.
- **The lock rule.** Never hold the broker lock across `bus.client_set_property`: the bus calls the write sink, which
  takes that lock. `take_events` may hold it.
- **The write path (A-13).** It is installed once with `bus.install_portal_writes(write_sink)` before `into_resource()`.
  Each write is ONE `PluginCommit` with no readings, carrying provenance `{package_id, content_hash}` and the pinned
  ancestry, and it is judged by the DB at write time.
- **Installations and audit.** The installation id is the package id, which is A-14 and the ops drain's convention.
  Refusals are audited through `ExtensionGovernance::audit_sink`.

### Admission drains (R-POR-08, P-S4)

- **Summary.** `repository_view.take_wanted()` -> worker. The node is resolved first, so its scope is known; then the
  `asset` row is read for the blob hash, then `fe_api::portal_bundles::inspect_stored`.
  - The `admission` block uses the *scoped* `admission::verdict`. `Unadmitted { can_admit: false }` (unsealed, untrusted,
    or the policy or role says no) answers `reason: "cannot_admit"`, so the section offers Admit only where the dialog
    can accept it.
  - Every selection change triggers a read: one node read off-frame. A non-repository node answers `Unreadable` and
    shows no section.
- **Admit uses a portal-owned instance of governance's consent dialog (`PortalConsent`).**
  - It does not use the shared `ConsentState`, because fe-ui routes those decisions to the ops drain as
    `ExtensionOp::Consent`, which requires a staged extension package. The dialog code is the same (A-10, never a second
    dialog), so `request.portal` renders the grant list.
  - **Refusals** fill `blocked` in actionable order:
    1. no governance;
    2. host switch off;
    3. Disable all;
    4. unsealed;
    5. untrusted publisher;
    6. kill-listed with no Owner;
    7. policy or role.
  - **A kill-listed build** offers the override only when the local DID is the verse's `created_by`. The DB re-checks
    Owner and the typed confirmation.
  - **Accept** -> `ExtensionGovernance::record_consent`, which writes one durable `consent` audit entry and then the
    plugin manager. Only after it succeeds: `learn_consent` -> `AdmissionSignal::Event(Admitted, Consented)` ->
    `invalidate(node)`. A failed write admits nothing.
- **Revoke** (OD-E1). One blocking `consent-revoke` audit entry; `load_consents` drops the consent on the next start.
  - Then `Event(Revoked, ConsentRevoked)`: the open mount unloads, and the broker closes the client after in-flight
    writes settle. Rows stay.
  - Revocation fails closed: it applies in-session even when the durable write is uncertain, and the user sees a warning.
- **Hydration.** Governance hands hydrated consents only to the plugin manager, so the worker reads
  `LoadExtensionGovernance` once at startup (6 tries) and learns the local DID's consents. A rotated key's consents are
  ignored. A bundle opened earlier re-judges on the `ListAccepted` that follows.
- **Re-judge tick.** While a bundle portal is open, `ListAccepted` is posted every 500 ms (bounded by the reactive
  loop's tick).
  - Why: list acceptance, policy-row refreshes, role resolution and overrides change the view without a
    `host/admission` event.
  - The re-judge is memory-only and emits nothing on `Keep`.
  - To make it change-driven, governance needs a generation counter (NEEDS, extension_host/governance.rs).
- **`PortalBundleChanged`** (a reload of the open portal), only on a real change:
  - a summary read whose asset differs from the last one read for that node;
  - awaited bytes that arrived (checked with `has_blob` on the tick);
  - the open node removed;
  - a `portal.bundle_asset` change on an open node no read has seen.

  A property change on a known node only re-reads. A read that was running when the change happened is re-run (the
  `stale` set).
- **Deferred.**
  - Replace (`bundle_asset_pick`) needs a bundle-asset picker in fe-ui. Trigger: P-UI ships it; the binary half is then
    a `SetNodeProperty portal.bundle_asset` write plus `invalidate`.
  - `local_client_did` is `None`, because `fe_api::dev_token::DevTokenWriter` mints its client DID internally. Trigger:
    it accepts one, and `main.rs` passes the same DID to both.

## QA Portal evidence

`qa_portal_bridge` is the small owner-thread coordinator for fixed native fixture operations. It captures the active full hierarchy scope and exact selected Portal entity/node, requires the local Owner role and existing ViewScope containment, then rechecks them while pending and before reply.

A bundle portal is a Portal here, through `PortalState::opened_for_entity`. Bundle ops (`PortalObserve`, `PortalSlot`, `PortalIpcStats`) take the fixture surface's `start_portal` path, which reads the mount table and the bundle origin. fe-api's handler checks their receipt with `is_bundle_origin_url`. `qa_web::allows` never applies to a bundle URL: it gates only the website fixture ops (`live_source`/`execute`) and their receipts. `/api/v1/debug/ui-state` carries `portal: {node_id, state, badge_text, badge_name}` from `fe_ui::portal::chrome::chrome_model` while a bundle portal is open, and `null` otherwise; the QA probe's `badge` paths read it (R-POR-30). A close, selection/scope/role change, dropped caller or five-second deadline cancels the backend job. Native session/page/visibility and live helper checks remain in the backend, so the binary never enumerates or captures host windows. Idle frames do no context cloning or native work.

Native fixture admission and release additionally require the selected entity actual SpawnedNodeMarker to match its selected node and the active Petal. Existing scope-change coverage exercises the Portal context owner, selection/open state, role loss and changed Petal snapshots. The API completion credential check will use the independently owned durable auth helper once that prerequisite is frozen; current code retains the existing expiry/revocation-cache checks in addition to scope/role/permit fences.

## QA selected stamp pose

UiState preserves the ordinary marker/node/tree selection guard. Its separate
promoted-stamp projection consumes fe-ui's PromotedStampReader, which verifies the
current semantic binding and owning Path ancestry before returning the actual ECS
Transform. The snapshot labels the canonical stamp address, owning Path and index;
it never treats the persisted metadata row's origin TRS as the rendered position.
This read-only projection adds no command route or permission bypass. Existing QA
Owner, caller scope, input enablement and frame ownership gates remain unchanged.


Synthetic input diagnostics expose only event variants, excluding Key::Character payloads.
The ordinary injection audit records actor, scope and command count without input text.
This preserves credentials when the one-use invitation client drives masked Join.


## QA invitation

qa_invitation_bridge is a default-off owner-thread coordinator with capacity one and a fixed
three-second job deadline. Only the open Access panel latest generation can be observed or
consumed. Actual local DID, active Verse, rendered ViewScope, exact target/recipient/role,
navigation revision, short consume TTL and caller lifetime are rechecked while pending and
before release. A fresh readonly PrepareJoinInvite result proves current pinned Owner and
manifest using existing checked storage fences; the recipient never impersonates the issuer.
Selection/dialog/generation changes, expiry or a dropped caller discard pending work. No
credential is retained in host state: its sole release moves from UI into the typed reply.


Guarded Join input retains the ordinary authenticated InputOp and real pointer replay, but
TextEntry frames carry a private marker rather than global KeyboardInput text. The existing
actual masked TextEdit consumes its scoped egui text capsule. Per-frame host checks include
recipient identity, navigation/dialog/input revision, exact primary window and widget/focus
proof; the renderer repeats checks at the insertion and real Join-click boundaries. Every job
exit clears its private lease. Success requires the actual button response, not merely planned
frames. Ordinary unguarded input retains its existing behavior and clipboard suppression.


The one-use invitation host now consumes typed Owner or delegated Manager SourceAuthority
from the actual v2 DB preparation. Metadata includes its exact public proof identity and
current Access context; the fresh result must match before moving the credential once.
Owner API-token/operator gates are unchanged and do not assert entity Owner authority.

## Path access retirement

The GPX host chain handles read-model access before draining path/import operations.
Missing or unresolved local roles, denied roles and the manager's pending role check
retire host point seeds, read/create waiters, deferred projections, local version
watermarks and route/style caches, and despawn both line and single-point projections.
Manager source/read identities are retired too; role-resolution ownership remains.
Queued path/import intents are discarded while unavailable, with no mutation replay.
The host producer systems stay gated until a fresh Viewer-or-higher result permits
rehydration. Returning to the same Petal after a grant starts bounded complete reads;
old queues, cached paths and equal-looking node identities are not reused as proof.
Actual committed Undo/editor-batch outcome ownership remains in its existing owner.


## ?host-path-outcomes ? accepted requests across access changes

Host submission uses nonblocking try_send and acquires an Accepted owner only on
successful channel submission. Creation owners retain exact command identity and
source payload. Initialization properties have their own private IDs and retain
node, key and submitted value until the matching NodePropertySet or RequestFailed.
A generic Error cannot guess which request failed. IDs never match by Petal/name;
a late old birth cannot fulfill a newly submitted same-name import or annotation.

There are at most 256 accepted creation owners in each host workflow and 256
accepted property owners per workflow. Pending imported track sources together
retain at most the existing scene cap of route-point/waypoint rows. Capacity or
Full/disconnected submission refusal issues no write and reports the unsubmitted
portion; already accepted siblings remain owned. An unanswered accepted request
reports unknown after five seconds and retains its owner/payload without replay or
expiry eviction. It continues contributing to the QA pending observation. These
limits can refuse new work while old outcomes remain unknown; no background retry
or success is inferred from a timeout.

Access retirement clears unsent operations, read seeds and render caches separately
from accepted writes. Accepted creates, annotation births and property confirmations
become outcome-only. The always-running advance_host_outcomes consumer has no DB
sender, Commands or navigation resource: it consumes late confirmations/failures
and reports partial or unknown status, but cannot initialize a retired birth, create
its children, retry a write, render it or move the cursor. Viewer access permits
fresh route reads; continuation/mutation producers require Editor. A manager-owned
access generation catches role invalidation and resolution within one DB batch;
manager incarnation and navigation intent changes also retire prior continuations.
The generic UI handler may add a known durable private birth to the tree, but cannot
flush a Pen anchor or run another creation workflow for that private result.

Terminal RESET still replaces PendingPathEdits and PendingGpxImports only after
its existing DB-worker quiescence fence. That terminal lifecycle is the explicit
place where remaining accepted outcome ownership is discarded. The existing App
journey authors accepted submissions, access loss, late success/failure/property
confirmation, a retained unknown owner and a new same-name import followed by an
old duplicate birth. Existing UI journeys cover private results and retired Pen
continuations. These are authored source checks; root owns execution and live QA.

## scene-peer-assets

The GUI registers fe_sync::SceneAssetEventBuffer beside the streamed tileset buffer.
fe_sync::drain_sync_events owns the receiver and forwards bounded scene asset ready/
owned-finality events into this buffer; native projection consumes it after that
system and refreshes current DB state. Asset-reader misses use nonblocking try_send
and the sync service admits them only against the one successfully opened native
Verse when no explicit scope is available. Missing-byte native intents carry their
actual Verse and request owner instead. Neither this host bridge nor a BlobReady
event substitutes for current DB scope/asset admission or a local history version.
## Closed-path source

Track tagging requires RenderedTrackGeometry to match the entity's actual Mesh3d
asset ID. The host reads its frozen input/style and prepared border/fill geometry;
it never tags from later live route/style maps. The resulting TrackPickShape binds
full decoded source rows and its actual picking geometry in a sealed digest used
by checked selection and Undo source predicates. Closed-path sanitation remains
owned by fe-terrain and runs once for the mesh and its captured source. Unknown
or mismatched mesh snapshots remain untagged until rebuilt.

The host and UI witness share SDK RouteSourceRow normalization. Frozen renderer
rows feed the witness directly, retaining f64 positions/handles/time and every
actual corner/smoothness value; the compact persistence writer is not a source
identity encoder. JSON route materialization calls the same decoder, while editor
PathPointRow comparisons remain lower-precision draft checks. The actual Mesh3d ID
and prepared geometry digest remain required; no live-map retagging was added.


## Ordinary native accessibility adapter

`native_accessibility/` owns the unique primary window/context/handler input seam.
It brackets Bevy's flattened poll by retaining only the exact owned handler Arc,
then restores a still-live unreplaced entry. Fresh full-pass metadata and current
product guards permit one ordinary Click per pass; no flattened second reader,
HTTP authorization change, direct UiAction injection or capture/helper is added.
Focus remains unsupported. See `native_accessibility/AGENTS.md` for source/queue
limits, lifecycle and authored existing-journey coverage.
## Scene-asset-publication-audit

qa_debug_bridge's AssetAuditPending owns at most one metadata read. Existing QA current-context and same-frame navigation gates admit it; the owner additionally captures cursor identities and NavigationManager intent_revision. Submission uses bounded DB try_send without a generic blocking API DB round trip. Each host poll refuses closed/expired/navigation-changed/out-of-scope owners, then matches only SceneAssetPublicationRead with the exact nonce and expected node/operation shape. Foreign/wrong-family/late results never consume a newer owner. No native review/adoption state is read or changed, and successful DTO conversion performs no scene projection or demand enqueue.

The DB enforces fresh stored caller/local Owner and full native-state fences; this host owner is correlation/lifetime control, not authorization. The private eight-second deadline survives both queued hops. A timed-out read can finish harmlessly on the single DB worker but its late reply has no live slot. Audit metadata is not file, render, peer delivery or local-history proof.


## Native window/context observation witness

native_window_witness owns the actual retained HWND property and completed AccessKit
root identity/status, using native_accessibility's narrow read-only ownership seam.
It has no operator input, helper, capture or HTTP/role authority. UI surface metadata
only supports the controlled empty Properties Inspector profile; any selected-object
Inspector still needs its separate audit. See native_window_witness/AGENTS.md.
## Earthwork raw assignment continuity

terrain_bridge binds only an active PetalTerrainLoaded echo. It fingerprints the
complete confirmed raw JSON using runtime's canonical hasher, records the exact
parsed assignment identity, and carries the raw fingerprint on the same terrain
assignment. ActivePetalTerrain retains that identity across normal tileset-driven
scale/encoding/grounding reconciliation; the producer fingerprints the operational
config separately. A newer raw document cannot retag rounded-equal old geometry.
Missing source identity is unavailable for background persistence.

Unrelated GPX, vision, screenshot and lazy-tile fixtures mechanically supply
None/false for new unbacked source fields; their behavior is unchanged. The
existing active-Petal forwarding journey checks the same raw witness on the
assignment. No native observer, asset-audit or transfer surface is modified.


## Terminal earthwork outcome ownership

The existing Supervisor's actual DB JoinHandle remains the lifecycle authority.
After its successful join, clear_retained_outcomes_after_database_join clears
scene-asset and background-volume owners together. The abnormal-join branch
returns before that hook. Freeze retires unsent volume intent through
clear_read_models but retains accepted unknown saves independently of UiManager.
Bounded terminal DB drains let that owner recognize exact outcomes without any
current scene projection or new command; no generic joined-thread proof is used.

## Selected terrain diagnostic without report demand

The selected diagnostic returns scene_snapshot_unavailable for an absent or
no-Petal SceneAnalysisSnapshot. Its producer collects only for a resolved volume
footprint, independently of ordinary Node selection or report-panel visibility.
A real snapshot for another Petal still returns stale scene_petal_mismatch.
No diagnostic forces collection, reveals another object or weakens ViewScope,
selected marker/tree, terrain, geometry or epoch checks. The existing grounded
camera journey authors both empty-snapshot forms alongside the foreign-Petal
refusal; actual matching report demand remains a live verification gate.

## QA divider eligibility (QA59)

The existing scoped Owner QA UI layout response adds divider_eligibility_version1
and the opt-in shared layout collector's bounded dividers array. IDs, side/pass,
fixed boolean gates, closed blocker and actual Response enabled state come from
the width owner at registration, not a later reconstruction of UI state. The
egui_input_focused field is window-context input focus, not AccessKit widget focus
or an independent OS focus assertion. Missing records remain unknown. This adds
no HTTP route, input authority, focus request, OS query, dialog payload, clipboard
data, native proof/capture admission or mutation. Existing layout fields and
scope/admission are unchanged.

## §render-loop (QA-0119, 2026-11-07)

Every build runs a reactive loop (`render_wake.rs`): focused `reactive(250 ms)`, unfocused
`reactive_low_power(1 s)`; `FE_QA_A11Y_CONTINUOUS=1` still opts into continuous. Background
work never waits for the idle tick: a `render-wake` thread polls every queue a Bevy system
drains (DB results, sync events, network and lifecycle events, API commands,
input/screenshot/vision bridges, QA bridges when QA is on, inbound transforms, the scene-change
bridge into the EntityStore, plugin results, the terrain bridge's assemble and streamed-bundle results) every 4 ms and sends `WinitUserEvent::WakeUp` at most once
per frame; only queues that are actually drained may be watched, or the loop never sleeps.
`keep_awake_while_working` (Last) requests a redraw while any watched queue is non-empty or a
multi-frame job runs (input batch, posed capture, pending screenshot, live `Screenshot`
entity), so a settle or capture never stalls asleep. The steer asked for `desktop_app()`
(5 s / 60 s); the bounded waits keep deadline- and timer-driven systems (replica reads,
transfer deadlines, notices) within 1 s.
Critic pass 4 (2026-11-08) added the scene-change and plugin-result queues: both are drained every frame
(`drain_scene_changes_to_store`, fe-plugin `drain_plugin_results`) but were unwatched, so an unfocused peer's
EntityStore (analytics queries, the QA-0087 transform rollback to WS clients) lagged up to the 1 s idle wait. Still
unwatched, each waiting at most one idle tick: the tileset meta verdicts (on purpose, see
`terrain_bridge::watch_results`), the one-shot API listener address and the F12 save toast (the window is focused).
