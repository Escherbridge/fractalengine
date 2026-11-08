# Documentation screenshots

No documentation images exist yet. The docs mark where each one belongs with an
HTML comment of the form `<!-- screenshot: <name> — <what it must show> -->`.
This file lists every marker, what to capture, the content it needs, and how to
capture it.

When you capture an image:

1. Save it here as `<name>.png`.
2. Replace the marker with a Markdown image, for example
   `![Access tab with three peers](docs/images/access-tab-roles.png)` from
   `README.md`, or `![...](images/pen-curve-handles.png)` from a file in `docs/`.

## Standards

- **Width:** 1400 px for whole-window captures (1400 × 875 for a 16:10 window,
  1400 × 788 for 16:9). Panel close-ups keep the app's native pixel size; do not
  upscale.
- **Size budget:** whole-window PNG at most 450 KB, close-up at most 250 KB,
  about 3 MB for all seven. Optimize losslessly (for example with `oxipng`)
  before committing.
- **Content:** fixture data only. A window capture includes the whole UI,
  including the verse list and any open dialog, so never capture a real
  identity or a private verse. QA runs use a throwaway identity in isolated
  storage.
- **Models:** use GLBs whose licence allows redistribution, with embedded
  textures (KTX2 and Basis textures are refused). `fractalengine/assets/samples/README.md`
  lists the two bundled samples and their attribution; credit any model that is
  recognisable in the image.

## Markers

| Name | File | Shows | Fixture / content | Capture | Target |
|---|---|---|---|---|---|
| `editor-populated-petal` | `README.md`, below the "Why this exists" quote | The whole editor: top bar with the tool buttons, hierarchy sidebar, 3D viewport with terrain, three to five placed models and a drawn path, and the Inspector (right sidebar) showing one selected node | A petal with the alpine demo map set as its map (Maps > Installed > Set petal map), the morning-run GPX imported, three to five licence-clean GLB nodes, one node selected. Build the sample packages first: `cargo run -p fe-hexon --example build_sample_hexons` writes `sample-hexons/dist/*.hexon` | Recipe A or C (window) after opening the petal and selecting a node; close the Data window | 1400 × 875, ≤ 450 KB |
| `terrain-hexon-map-view` | `README.md`, above "In progress" | The viewport over an installed map (terrain mesh with satellite imagery), the scale-bar HUD at the bottom left, and the Maps section open on the Installed tab with "Active map ✓" on the petal's map | `sample-hexons/dist/alpine-demo-terrain.hexon` installed and set as the petal map (its satellite layer is flat-colour placeholder tiles generated in memory, so the image is licence-clean; use a real-world imagery tileset only if its imagery licence allows redistribution). `launch.ps1 -Fixture terrain` binds an elevation-only offline tileset, so it will not show imagery. Scale bar on (the default) | Recipe C: click Maps in the top bar, orbit or scroll to an oblique view of the terrain, then `fe.py shot --out docs/images/terrain-hexon-map-view.png` | 1400 × 875, ≤ 450 KB |
| `access-tab-roles` | `README.md`, Peer-to-Peer Sync section | A verse's Settings dialog (the "Options…" entry of its hierarchy menu) on the Access tab: effective roles for several peers (viewer, editor, manager), the role picker with its "Save role..." confirm button, and the "Invite a peer to this scope" form | A verse owned by the capturing node with at least two other peers holding different roles. `tools\qa\peers.ps1 -Action Start -Count 3` followed by `python tools/qa/peers_join.py` joins an editor and a viewer through the real GUI flow (tools/qa/AGENTS.md §peers). Rename the verse on the dialog's General tab so it does not read as a QA run | Recipe C bound to the owner: click the verse's "…" menu in the hierarchy sidebar (accessible name `Options menu for Verse <verse name>`), choose Options…, click the Access tab, then `fe.py --peer owner shot --out docs/images/access-tab-roles.png`. The Access tab needs a stored Manager role at that entry | window crop to the dialog, native size, ≤ 250 KB |
| `copy-for-bi-export-tab` | `README.md`, API Gateway section | The Data window on its Export tab: the "Copy for BI" card with the egress SQL, the `/api/v1/query` endpoint, the `export.parquet` and `export.csv` URLs and the DuckDB `read_parquet` snippet | A petal with a few nodes so the SQL has a visible source (Petal source selected) | Recipe A or C: click Data in the top bar, then the Export tab, then capture the window | 1400 × 875, ≤ 450 KB |
| `pen-curve-handles` | `docs/editor-guide.md`, "Editing curves in the viewport" | Close-up of a path being edited with the Pen tool: yellow anchor markers, cyan handle points joined to their anchors by stems, and one smoothly curved segment | A petal with terrain and a path of at least four anchors, two of them dragged out with handles (press and drag with the Pen tool), the path still in edit mode | Recipe A or C (window), camera zoomed on the path so the handles are legible; the sidebars may stay in frame | 1400 × 875, ≤ 450 KB |
| `pen-options-corner-settings` | `docs/editor-guide.md`, "Corner settings (Options panel)" | The Options panel with the Pen tool active: the Pen section (curve mode, New anchor, sensitivity) and the "Corner settings — point N" card for a selected point | The path from `pen-curve-handles` with one handled anchor selected in the panel's point list | Recipe A or C (window), then crop to the right sidebar | native size, ≤ 250 KB |
| `first-launch-editor` | `docs/guide.md`, "Running" | The editor right after a first launch: the seeded Genesis Verse, Genesis Fractal and Genesis Petal in the hierarchy, an empty viewport with the grid, and the top bar with the tool buttons | A fresh data folder: `tools\qa\launch.ps1 -Fixture none` starts an isolated empty run (the Genesis seed still runs) | Recipe C: `fe.py shot --out docs/images/first-launch-editor.png` once the window settles | 1400 × 875, ≤ 450 KB |

## Capture recipes

The agent screenshot surface is on by default (`FE_API_SCREENSHOT_ALLOW=0`
turns it off). It needs an Owner token whose scope covers the open verse and
allows one capture per second per token holder; a token that does not cover the
open verse gets a uniform 404. `POST /api/v1/debug/input` is off unless
`FE_API_INPUT_ALLOW=1`.
Details: `fe-api/AGENTS.md` §screenshot, §camera-state, §agent-input-surface.

**Recipe A: live window, any run.**

1. Start the app with `FE_API_DEV_TOKEN_OUT=<file>`. The file holds
   `{"api_base": ..., "tokens": {"<verse_id>": "<jwt>"}, "roles": {...}}`; the
   token is Owner where the node owns the verse and lasts 8 hours.
2. Size the window to 1400 px wide. A window capture is always the window's own
   size.
3. `curl -fsS -H "Authorization: Bearer <token for the open verse>"
   http://127.0.0.1:8765/api/v1/debug/screenshot -o docs/images/<name>.png`
   returns a PNG of the live window including all UI. Pressing `F12` in the app
   saves the same kind of image to `data/screenshots/` without a token.

**Recipe B: scene only, fixed size.** `POST` the same path with
`{"mode":"scene","width":1400,"height":788,"pose":{...}}`. Scene mode renders
off-screen and leaves the user's camera alone; width and height go up to 4096
(the default is 1280 × 720). The pose is exactly one of `camera`, `orbit`, `geo`,
`tile` or `focus`, and the response carries the resolved pose in the
`X-FE-Capture-Pose` header. `GET /api/v1/debug/camera` reads the live pose, so a
view framed by hand can be reproduced. A `window`-mode `POST` with a pose moves
the real camera, settles, then restores it.

**Recipe C: the QA client, isolated and scripted** (Windows PowerShell 5.1 and
Python 3, standard library only). `tools\qa\stage.ps1` builds a stage and
`tools\qa\launch.ps1 [-Fixture basic|terrain|none]` starts an isolated run with
its own data root, `FE_API_DEV_TOKEN_OUT`, `FE_API_INPUT_ALLOW=1` and a seeded
fixture (tools/qa/AGENTS.md: Storage isolation, Stage provenance, Fixture
modes). Then, from the repository root:

```powershell
python tools/qa/fe.py window size 1400 875                 # physical pixels, 16:10
python tools/qa/fe.py click --name "<widget name>"         # drive the UI by accessible name
python tools/qa/fe.py shot --out docs/images/<name>.png    # window capture (POST)
python tools/qa/fe.py shot --no-ui --width 1400 --height 788 --orbit FX FY FZ YAW PITCH DIST --out docs/images/<name>.png
python tools/qa/fe.py camera                               # live pose
```

`fe.py shot` also takes `--tile Z X Y`, `--geo LAT LON [ALT]`,
`--focus-node ID`, `--focus-petal ID` (with `--navigate` to switch petal) and a
raw `--pose JSON`. Every input command (`click`, `drag`, `key`, `type`,
`scroll`) accepts `--capture` to save a PNG after the action. Without `--out`,
PNGs go to `<run>/shots` or `$FE_QA_SHOTS_DIR`; `fe.py --peer <name>` binds a
command to one member of a `peers.ps1` rig. See tools/qa/AGENTS.md §fe-py.
