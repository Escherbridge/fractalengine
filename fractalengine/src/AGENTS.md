# fractalengine (GUI binary) — main wiring rationale

`src/main.rs` boots the desktop GUI: it spins up the DB / network / sync / API
threads, builds the Bevy `App` (DefaultPlugins + egui + renderer + fe-ui +
terrain + webview), and bridges the background threads to Bevy resources.
`terrain_bridge.rs` and `asset_bridge.rs` are the per-frame drain systems that
turn fe-ui's queued ops into real side effects (the UI crate has no DB / blob /
filesystem access by design).

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
