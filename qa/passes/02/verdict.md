---
type: QA Pass Verdict
pass: "02"
date: 2026-11-01
stage: 89be757-20261101140417507
run_dirs: [20261101-140506-617]
scenarios: [S03-regression, S1-locators, S3-probes]
scope: focused-surface-smoke
---

pass 02: gold 0/0, new S1/S2 1, coverage 0/8

## Gold tasks

This is a focused S1/S3 smoke and S03 crash regression, not a full scenario
rotation. No full gold task is claimed. The former QA-0001 path was driven
from a verse-level view into its sidebar Petal B and back to Petal A; the
app remained alive and exported the complete verse/fractal/petal ancestry.

## Findings by severity

| ID | Severity | Result | Evidence |
|---|---|---|---|
| QA-0001 | S1 | CLOSED in this regression | `shots/140754-401-s03-former-crash-path-after.png`; authoritative state has QA Basic / QA Fractal / Petal B; input request succeeded. |
| QA-0015 | S2 | Still open | `shots/140642-255-s1-locator-maps-after.png`; after opening and closing Maps, state reports viewport width 81.53125 at a 1280px window. |
| QA-0018 | S2 | NEW: selected Inspector content is laid out below the window | `shots/140836-049-s3-authoritative-selection-after.png`; selection is authoritative and valid, but Inspector appears blank and AccessKit locates its Query tab at y=693 and Portal URL controls at y=1017–1157 in a 720px window. This is current evidence, distinct from the unsupported earlier blank-Inspector interpretation. |

QA-0018 reproduction, with `F` = `python tools/qa/fe.py --run 20261101-140506-617`:

```powershell
F click --name Maps --role button --wait 10
F click --name Close --role button --wait 10
F click --name 'QA Basic' --role label --wait 10
F click 9 290 --wait 10
F click --name 'Petal B' --role label --wait 30
F click --name 'Petal A' --role label --wait 30
F click 44 332 --wait 10
F click --name 'Cube 1' --role label --wait 10 --capture
F state --path selection
F ui --find Portal --interactive
```

The two arrow clicks expand collapsed tree rows; these controls still need
accessible names in S2. The new Inspector finding's root cause is unconfirmed;
it is routed to the forthcoming layout slice, together with QA-0015.

## Coverage

S1 locator proof: Maps exported a button rectangle
`x=827.6875, y=11, w=54.5625, h=18`, physical pixels, ppp=1.0. The named
click injected its exact centre `(854.96875,20)`, opened Maps, and the state
predicate `active_section=maps` matched. Close, hierarchy labels, Petal B
and Cube 1 were subsequently driven by name. Each captured PNG was read.

S3 state proof: Cube 1 selected by sidebar name exported node id
`01M6CEWMNVRCKHPF6JEB28E6HP`, position `[0,0,0]`, identity quaternion and
unit scale, independently of the broken visible Inspector layout.
`wait-for --state selection.node_id=...` and `--name 'No such widget' --gone`
matched. Unknown DB/sync pending totals remain null, with an explicit limited
settled definition. The window list projects known UI resources; the full tree
also exposes standalone egui windows for later dialog/window work.

All three REST probes returned 404 / `not_found_or_denied` under the other
verse's Owner token. All three MCP tools admitted the active Owner and
refused that foreign token. A fake JWT was used as a new fixture verse name:
the exported tree had two `[REDACTED]` nodes, and neither tree nor logs
contained that fake credential. Probe checks are saved locally under the run
as `s1s3-probe-checks.json`; no actual tokens were printed or committed.
Background warning payloads without a proven matching event scope are withheld.

Malformed `api_base: {}` and `wait-for --timeout inf` both produced typed
refusals and exit 2. Off-window locator centres are refused rather than clicked;
`type --into` accepts input roles, not a separate caption label. The complete
UI naming survey and a full named-input journey remain S2's work.

## Loop health

The stage manifest's source fingerprint matched at launch, with an executable
SHA-256 and `source_dirty=true`; its base source commit is
`89be757e3858b052bc041b24d206528321b512a9`. Stage 1 was correctly rejected
when inputs changed during build; a later stage was superseded by reviewed
fixes. Only the final matching stage was launched for this smoke.

Launch took 14.8s, health 6026ms, bootstrap token 2510ms. The fixture still
has no TerrainTileset (`terrain_bound=false`), so no terrain claim is made.
The run contains `identity/node_keypair.key` (64 bytes), the run-local durable
secret store, and `webview/`. The final stage has no WebView2 profile directory.
Settings resolve to `<run>/config/settings.ron`; no Settings save or normal
user-store modification was performed. A same-run restart was not exercised;
its durable-store journey remains in the campaign-end sweep.

`stop.ps1 -Run 20261101-140506-617` reported Stopped at 14:12:39 local time,
using the recorded executable/start identity and exact process tree. No app
or cargo process is intentionally retained by this slice.

Compile check passed. The reviewed GUI and installer builds passed. Existing
runtime/API/GUI input journeys were extended for scope, REST/MCP admission,
redaction, protected linked labels, physical ancestor transforms and bounded
logs. Tests and clippy were not run; the user requires the integrated sweep
once at campaign end.

## Accessibility timing

A separate bounded synthetic egui 0.33.3 panel benchmark used 80 rows of label,
DragValue, checkbox and button widgets, 80 warm-up frames, and 500 measured
frames per condition in four alternating-order rounds. Mean CPU layout frame
cost was 5.018ms with generation disabled and 6.529ms with AccessKit enabled;
individual idle samples were 4.745–5.300ms and enabled samples 6.108–7.253ms.
The driver used the current dev library and ran while the QA GUI was alive.
This excludes Bevy scheduling, rendering, OS accessibility forwarding and
screen-reader behavior: it is not a real FractalEngine idle/active frame-cost
comparison. That application-level measurement remains explicit follow-up.
Local reproducer: `.qa/egui_accesskit_cost.rs`; samples:
`.omc/research/qa-accesskit-cost-20261101.log`.

## Convergence

Not evaluated: this smoke covers selected surfaces, not the full gold set or
feature catalog. QA-0015 and QA-0018 remain open, full accessibility naming is
pending, and terrain, dialog/window seams and later queued scenarios remain.
