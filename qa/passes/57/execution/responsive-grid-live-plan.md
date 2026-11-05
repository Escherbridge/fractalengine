# Responsive property grid live plan — QA57

Plan only; nothing below has been executed by its author. Parent moved the short native observation to QA56 and this grid check to **QA57**. Use actual QA57 run/IDs throughout; this historical filename is intentional. Root reports combined stage `6ca9f49b` built successfully; root must bind its exact immutable stage/executable hashes and current process identity before these steps. Apply shared `C:/Users/atooz/.gemini/UI-UX-guidance.md`: readable content, semantic controls, complete visible targets, actual images and measured evidence.

## Command convention and admission

`FE(...)` below means the existing CLI, with each argument passed separately. A root-owned Python orchestration session may use this minimal wrapper to avoid PowerShell JSON quoting. Save each stdout/stderr and exit status to a fresh numbered artifact; never print credentials. This is a plan snippet, not a new helper implementation:

```python
import json, subprocess, sys
RUN = r"<actual absolute QA57 owned run>"
VERSE = "<actual fresh owned verse ULID>"
PETAL = "<actual fresh petal ULID>"
def FE(*args):
    return subprocess.run([sys.executable, "tools/qa/fe.py", "--run", RUN,
                           "--verse", VERSE, *map(str,args)],
                          capture_output=True, text=True, check=True).stdout
```

Root performs existing owned lifecycle/navigation. Read `FE("state")`, `FE("camera")`, and `FE("api","GET","/api/v1/hierarchy")`; require actual active Verse/Petal and matching ancestry, ready role, input opt-in, current-source provenance and no conflicting operator. Sidebar names are `Open verse <name>`, `Open fractal <name>`, `Open petal <name>`; resolve fresh tree targets for each step, respecting cross-Verse token changes. Do not manufacture UI state or use a capture pose as navigation evidence.

Before the rich driver, make the **read-only** distributed readiness request:

```python
FE("api","POST","/api/v1/query",json.dumps({"distributed":{"kind":"all_readings","petal_id":PETAL}}))
```

Require a complete empty result with `distributed.missing_shards`, `missing_hosts`, `incomplete_shards`, `missing_holders`, `rejected_answerers`, `unverified_answerers` all empty and `truncated=false`. Then root runs **once**, with an exclusive fresh evidence directory:

```text
python tools/qa/rich_data.py --scenario tools/qa/scenarios/rich-fleet.json --run <RUN> --pass-id 57 --verse-id <VERSE> --petal-id <PETAL>
```

Preserve any refusal/unknown outcome; no driver replay or silent fresh scope. Its existing bounded read-only rate-limit handling is distinct from a mutation retry. Completed context must show Alpha9/Beta3/Gamma2, all original independent assertions and actual diagnostics, not just exit0. Preserve each final state, transform, property map and full history before UI checks. Alpha/Beta/Gamma UI activity here is read-only.

## Disposable probe and independent oracle

Create **one separate asset-free node** after driver completion. Different fixture tag avoids the driver's rich-fleet membership filter; use unused keys with no matching enforced field definition. Check `GET /api/v1/field-defs/{percent-encoded-scope}` first, using context scope `VERSE#<VERSE>-FRACTAL#<FRACTAL>-PETAL#<PETAL>` and `urllib.parse.quote(scope,safe="")` as in rich_data.py. No rich-object writes, schema installs, measurements or group changes:

```python
body = {"name":"QA57 grid probe","position":[40,0,0],"rotation":[0,0,0],"scale":[1,1,1],
        "properties":{"qa_fixture":"qa57-grid-probe","grid_number":1.25,
          "grid_payload":{"samples":[None,True,{"sequence":9007199254740993,"value":12.5}]},
          "grid_remove":True,"grid_text":"  exact\ntext  "}}
FE("api","POST",f"/api/v1/petals/{PETAL}/objects",json.dumps(body,separators=(",",":")))
```

This is `rich_data.py::create_object`'s actual route/body shape; `asset_id` is optional and omitted. Require envelope `ok=true`, `data.persist="committed"`, valid returned `data.node_id=PROBE`, `history_version=1`, exact state identity/petal/transform/property map. Never infer the ID from display name. Unknown create means stop and read back; do not issue another create.

For each asserted version `V`, save these actual readbacks (the same paths as `Driver.current/state_at`):

```python
FE("api","GET",f"/api/v1/nodes/{PROBE}/properties")
FE("api","GET",f"/api/v1/nodes/{PROBE}/transform")
FE("api","GET",f"/api/v1/nodes/{PROBE}/history?after_version=0&limit=64")
FE("api","GET",f"/api/v1/nodes/{PROBE}/state?at_version={V}")
```

Require journal coverage v1/per-object commit ordering/current-state verified, exact contiguous versions, available replay, matching current version. Independent oracle: **v1** exact body; **v2** only `grid_number=-150.25`; **v3** additionally removes `grid_remove` completely. All other values/types, nested integer `9007199254740993`, transform/identity remain exact. GUI Save/Delete must each add one commit; no Undo/Redo or API mutation substitutes. The corresponding documented mutation shapes are `POST /api/v1/nodes/{PROBE}/mutations` with `{"op":"property_set","key":"grid_number","value":-150.25}` and `{"op":"property_deleted","key":"grid_remove"}`; these are oracle references, **do not execute them**.

Select via `FE("click","--name","Select node QA57 grid probe")`. Open actual Inspector/Properties using `Choose sidebar section: <current>` → `Toggle Inspector section`, and `Choose Inspector tab: <current>` → `Inspector tab: Properties` where those compact menus exist; at ordinary width use actual exposed section/tab names. Selection normally reveals Inspector already: do not toggle an active Inspector off. Inspect the tree rather than guessing a current label. `Custom Properties` defaults open; refresh is `Refresh selected node properties`. Confirm selected identity before each write.

## Width/scale matrix and target checks

Use `FE("window","size",W,900)`, `FE("window","ui-scale",S)` for `(W,S)` = **(1024,1), (1280,1), (1600,1), (1024,2), (1280,2), (1600,2)**. Read actual `state.window` dimensions/ui_scale and top-level `pixels_per_point`; factor2 is not an assumption of exactly2 physical pixels per point. Restore scale1 at end. If chrome is intentionally hidden by protected viewport rules, record that case and its actual recovery route; do not label an absent Inspector as a grid pass.

At each available Inspector, drag the existing **`Resize right panel`** splitter using its current tree bounds center and `FE("drag",x1,y1,x2,y1)`. Rightward shrinks it; leftward expands it. Read new tree after release; never reuse coordinates after resize. Capture ordinary compact (~260pt body), minimum outer120pt/**actual body near52pt**, and widest permitted width. At1600/1 obtain actual four-column `Key/Type/Value/Actions` if available; other widths may correctly remain compact `Property/Value` with wrapped actions. Do not force a raised Inspector floor or steal protected viewport width to manufacture wide coverage. Capture just below/above observed destructive-pair wrapping transition at an available width too; resize while unarmed, then arm afresh.

For each layout retain `FE("ui")`, `FE("layout-check")`, `FE("shot","--label",unique_label)`; inspect the actual PNG. Geometry is physical. Convert measured table viewport width by recorded pixels_per_point when reporting52pt. In `layout.scrolls`, require **Custom property table `offset[0]==0`**, `axes=[true,false]`; inspect its viewport/content_rect/clip and enclosing `Inspector` scroll. Vertical scrolling is allowed using `FE("scroll",dy,x,y)` over the current Inspector body, `--dx 0`; no horizontal scroll to expose row actions. Each tested target's **whole** node bounds and control rect/hit must fit horizontally inside its child clip, all measured ancestor clips and window (small rounding tolerance only); a clickable center/intersection alone is insufficient. Scroll vertically into view before the full two-axis check. Missing instrumentation is UNKNOWN, not PASS.

For `grid_remove`, inspect full `Edit property grid_remove`, `Copy property grid_remove`, `Delete property grid_remove`. Click Delete once, immediately capture actual `Confirm Delete property grid_remove` and `Cancel Delete property grid_remove` (four-second arm). Check whole targets/labels, no overlap, readable words, no divider collision; click Cancel. Version stays1. At52pt Confirm/Cancel should stack, grow vertically and remain reachable; ordinary width should retain the fitting pair. Do not resize an armed confirmation or claim arm continuity across resize. An expiry is recorded; do not blindly re-click the previous Confirm coordinates. Use the semantic names afresh.

## Draft, Copy, and the two real writes

At compact width click `Edit property grid_number`, `Value of property grid_number`, then `FE("key","ctrl+a")`, `FE("type","-150.250")`. This is a numeric draft whose exact raw spelling differs from canonical stored `-150.25`. Resize compact→wide→minimum→compact with this same selection. Capture editor/status/footer at each breakpoint; raw draft must remain **-150.250**, stored properties/history remain v1, and `Save property grid_number`/`Cancel editing property grid_number` remain reachable. Cancel once, verify v1/raw stored1.25. Reopen and enter the same draft, then **one** `Save property grid_number`; wait boundedly for confirmed stored readbacks v2, with no write retry. A transient/pending/refused save is not a commit.

Copy check uses `Copy property grid_text` (and optionally nested `grid_payload`) at an inspected width. Target/toast may be observed, but **exact clipboard value is unavailable through this synthetic surface**. `fractalengine/src/input_bridge.rs:197–225` suppresses CopyText/CopyImage during injection/settling; `:358–375,517,537` refuses Ctrl/Super C/V/X, and `fe.py` has no clipboard/paste command. Record the expected bounded `synthetic clipboard writes suppressed` warning separately from unrelated warnings. Do not read host clipboard, attempt paste/ShiftInsert workarounds, or count the toast as clipboard verification. Source contract: strings copy exact raw text; other types compact JSON (`property_grid.rs::clipboard_value`). This is source evidence only.

Finally, at the minimum measured width, click `Delete property grid_remove`, inspect current full Confirm/Cancel, then **one** `Confirm Delete property grid_remove` within the arm. Require v3 and replay v1/v2/v3 matching the oracle. Cancelled arms, resize, expand/collapse `grid_payload`, Copy and draft cancellation must add **zero** versions. Retain the probe and all attempts for inspection; no hidden deletion/reset/cleanup mutation.

Re-read each rich object's properties/transform/state and complete history using actual context IDs (history limit64, follow a returned cursor if needed). Compare to saved final Alpha9/Beta3/Gamma2 snapshots/history, excluding no state fields; require no added commits or changed data. Restore the recorded camera/selection/window settings explicitly if changed, document resulting state, and let root perform exact owned lifecycle cleanup. Publish bounded focused UI/data evidence and unavailable clipboard coverage; no FULL, peer, native-Invoke, test, CI or release credit.

Source grounding: `tools/qa/{fe.py,rich_data.py,AGENTS.md}`, `tools/qa/scenarios/rich-fleet.json`; `fe-ui/src/panels/{inspector.rs,property_grid.rs,widgets.rs}`, `fe-ui/src/ui_shell/{layout_policy.rs,right_sidebar.rs}`, `fractalengine/src/input_bridge.rs`, `fe-api/src/node_history.rs`. This plan changes no production source and executes no command.
