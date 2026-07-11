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
   filesystem). `asset_bridge::drain_asset_ops` runs each frame and:
   - resolves the node in `VerseManager` and reads its `asset_path`
     (`blob://{hash}.{ext}`);
   - parses the hex hash and asks the `BlobStoreHandle` for the on-disk path —
     the path is **never** built from user input, only from the content-address
     store keyed by the parsed hash (same guarantee as `BlobAssetReader`);
   - copies the blob to `dirs::download_dir()/fractalengine/`, creating the dir,
     sanitizing the filename (strip path separators / control chars; fall back
     to `{hash}.{ext}` when the node name is empty), and appending `-1`, `-2`, …
     on collision;
   - writes the outcome (`saved_path` or `error`) into fe-ui's
     `AssetDownloadStatus` resource, which a fe-ui system surfaces as a toast.
