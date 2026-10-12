# Sample models

Dev/test GLBs, loadable through the Bevy AssetServer as `samples/<name>.glb` (moved from the repo root 2026-10-11; the 14-byte `camera.glb` was a saved "404: Not Found" page, not a model, and was deleted).

Every file here comes from the [Khronos glTF-Sample-Assets](https://github.com/KhronosGroup/glTF-Sample-Assets) repository. Licences were verified on 2026-10-12 against each model's upstream `README.md`, `metadata.json`, `LICENSE.md` and the repo's `REUSE.toml` (upstream commit `edc7c9e6`).

| File | Upstream model | Licence | Attribution / notes |
|---|---|---|---|
| `cameras.glb` | [Cameras](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/Cameras) | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/legalcode) (public domain) | By Marco Hutter ([javagl](https://github.com/javagl/)). Upstream ships it only as `.gltf` + `.bin`; packed into a single GLB here (glTF JSON unchanged apart from dropping the external buffer `uri`). |
| `info_panel.glb` | [Box](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/Box) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/legalcode) | © 2017 Cesium. Byte-identical to upstream `glTF-Binary/Box.glb`; renamed only, not modified. |

## Camera regression asset

`cameras.glb` is the regression asset for the 2026-07-17 stamped-camera crash fixed by `deactivate_foreign_cameras` in `fe-renderer/src/camera.rs`: an imported scene whose embedded glTF camera spawned as a second active Bevy camera. The file has a 1×1 plane mesh plus two camera nodes at the scene root — one perspective, one orthographic. Any replacement must still embed at least one glTF camera node.

It replaces `duck.glb` (Khronos "Duck", © 2006 Sony, SCEA Shared Source License 1.0). That licence is not OSI-approved, so the file was removed on 2026-10-12.
