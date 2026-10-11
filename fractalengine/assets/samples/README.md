# Sample models

Dev/test GLBs, loadable through the Bevy AssetServer as `samples/<name>.glb` (moved from the repo root 2026-10-11; the 14-byte `camera.glb` was a saved "404: Not Found" page, not a model, and was deleted).

- `duck.glb` — Khronos glTF sample "Duck" (COLLADA2GLTF export). It carries an embedded glTF camera, the cause of the 2026-07-17 stamped-camera crash fixed by `deactivate_foreign_cameras` in `fe-renderer/src/camera.rs` — keep it as the regression asset for that guard.
- `info_panel.glb` — Khronos glTF sample "Box" (red unit cube, COLLADA2GLTF export).

**Licence: UNVERIFIED.** Upstream lists Duck under the SCEA Shared Source License 1.0 (© Sony) and Box under CC-BY 4.0 (© Cesium), as far as is known. Neither has been confirmed against the upstream README, and SCEA is not OSI-approved. Confirm both, or replace the files, before a public release (oss_release_20260717 task 4.3).
