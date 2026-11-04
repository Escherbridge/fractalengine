# Ordinary native accessibility semantics

This is the UI half of the general OS accessibility adapter, not a QA command or
permission surface. It holds bounded widget metadata on the actual egui Context.
The host owns window/handler identity and input scheduling. Normal widget handlers,
NodeManager selection, checked edits and DB role admission remain authoritative.
No synthetic-input flag, HTTP floor, clipboard guard or entity role is changed.

## Frame contract

The host activates the private context state only when OS accessibility is requested.
The normal UI root calls begin_pass before widgets and finish_pass after them. Response
observation records actual IDs, layers, clipped/enabled state and confirmation owners,
without labels/values/properties. complete intersects this metadata with the latest
complete AccessKit output, propagating hidden/disabled ancestry. It replaces validity
every pass; missing output, extra/disconnected IDs, duplicate registration, unknown
layers, modal/password/menu nodes or more than 4096 nodes invalidate the pass.

Snapshot pass/lifetime and current product Guard bind a staged native event. The UI
root rechecks the guard and neutral current input before widgets can synthesize a
fake click. Data or unstaged/multiple/stale events are removed. A discarded egui pass
never reinserts an event; only the first pass receives RawInput events. Host retirement
uses retire_input to remove both buffered RawInput and native events already consumed
into the current Context InputState. Terminal traversal need not call the editor root
to refuse an earlier click. Normal physical input is retained.

The product Guard is eligibility, never authority. It compares navigation intent,
manager/access lifetime, hierarchy revision, primary admitted source, selection generation and real role state; blocks any active
dialog, close confirmation, popup/modal, quarantined panel, pending Pen birth or active
tool/selection gesture. ActiveDialog accepts only None, so new dialog variants remain
blocked by default. Current known modal producers are ActiveDialog dialogs, egui popup/
menus, and the topbar close modal. An unregistered target remains unsupported.

## Explicit partial support

Only Click+None is supported. Focus is deliberately unsupported: pinned egui 0.33.3
memory/mod.rs:573 processes it before the current UI-root guard can recheck a modal
opened during Update. Values, numeric/text/custom/scroll actions and every ActionData
payload are unsupported. This is not complete screen-reader/modal action support.
No HWND/status witness, helper nonce/acknowledgement, external observer or capture is
implemented here; those are separately reviewed slices.

Native single-selection requires neutral Ctrl/Command/Shift/Alt/Mac-command and no
competing pointer/key/text/wheel input. Checks occur before delivery and at actual UI
use; physical modifiers are never cleared or synthesized. Sidebar Node/Group callbacks
keep their existing request_row_selection/request_selection semantics and NodeManager
authority. The Inspector and tools therefore observe the same selected set as ordinary
selection; physical Ctrl/Shift behavior is unchanged. General native Inspector/tool
buttons still execute existing role/confirmation/checked-write handlers.

## Confirmation

The shared confirm_button arm now owns its context lifetime and exact previously
rendered Confirm/Cancel IDs, a fresh arm generation, and consecutive egui pass continuity.
Snapshot metadata carries that exact owner/generation. The first armed render publishes
its pair without accepting confirmation; later renders must reconcile both actual IDs
before inspecting either click. A changed pair is never adopted into an existing arm.
A skipped pass retires the arm on reappearance, even within the 4s deadline. A native Click
elsewhere disarms BEFORE the helper evaluates an armed state, independent of widget
traversal order. Missing/recreated context, changed IDs or a mismatched owner disarm.
Current Confirm still fires once, Cancel never fires; physical outside-click and 4s
timeout remain. Normal egui clipboard commands are preserved; the adapter does not
read/copy payloads or weaken the host's separate synthetic clipboard suppression.

journey.rs extends the existing confirmation test rather than declaring another test.
It authors checks for outside-disarm, stale Confirm, valid two-step use, post-stage
modal/gesture/modifier change, Focus/data refusal, disabled/missing/duplicate IDs,
full snapshot replacement/cap, changed control-pair IDs, missing/reappearing helpers,
arm generations, current Cancel and context retirement. It also checks normal copy
commands. Parent owns execution and actual UIA delivery acceptance; no native runtime
or rendering result is proved by these authored assertions.

## Positive native window capture profile

surface.rs is an independent, fail-closed observation model. The gardener root
registers all eight actual producer regions before rendering: topbar, status, tree,
Inspector, results, viewport, transients and Data/GIS. A missing/duplicate region,
incomplete pass, current Guard change, unresolved role, input competition, keyboard
focus or an unsupported visible layer refuses publication. Only an active Petal with
normal tree and empty Properties Inspector can produce scene_empty_v1. Selected
Inspector (including its asset card) still needs a separate mandatory audit.

Every non-None dialog, non-Inspector section, API/Query/Data tab, visible result dock,
GIS, notice/error history, identity banner, popup, native dialog, portal or quarantine
blocks this profile before drawing. API Access/Join/entity settings also emit a
static credential marker without reading secret content. Unknown regions emit an
Other marker. Bounded-window creation and actual help/footer/tree popup entries mark
the pass before their content. The Areas check rejects unsupported registered
Areas and permits only Background and the exact viewport_tool_hint Middle area.
It is not an inventory of every directly painted layer. Pinned egui exposes only
an allocating visible_layer_ids view; the optional check caps that view at32
without claiming a bound on egui's prior internal allocation.

The enabled production direct-painter inventory was inspected separately:

| Source | Layer/content | Positive-profile treatment |
| --- | --- | --- |
| fe-terrain/src/ruler_plugin.rs::draw_scale_bar | Foreground ruler_scale_bar; numeric distance from validated scale math, fixed units and primitive strokes | Audited nonsecret ruler output is allowed independently of Areas registration. |
| fe-terrain/src/ruler_plugin.rs::draw_unscaled_chip | Same layer; fixed world-units text | Audited fixed text is allowed. |
| fe-ui/src/viewport_labels.rs::draw_viewport_point_labels | Background viewport_point_labels; fixed type glyph plus point ordinal, painted after gardener | Conservatively excluded whenever an edited track has any point rows, including states where projection would later skip them. |

There are no other production layer_painter/debug_painter calls in the inspected
host/UI/renderer/terrain/webview sources. This is a source-audited producer
contract, not runtime enumeration of arbitrary painters. Any new direct-painter
producer must be audited and admitted or explicitly block this profile before
positive capture is enabled for it. No clone/drain of GraphicLayers or paint
payload hashing is used. Unknown registered Areas still refuse. Capture's Guard
contains the actual current can_view/unblocked result and conservative label
state; a resolved RoleLevel::None is never positive. Ordinary A1 ready/action
semantics are unchanged by these separate capture fields.

The process-static surface counter is checked and never reset. Dialog replace,
Inspector tab change, observed control activation/change, native dialog enter/exit,
source Guard changes and unsupported/input transitions advance it. Actual renderer
markers keep an unsupported pass sticky even if its UI returns to Scene before the
end. Popup begin/end cannot reconstruct a previous epoch. No secret value is hashed
or inspected. A native file-dialog RAII scope marks both boundaries. Portal opening
permanently poisons capture availability until process restart because the current
backend has no native-child-closed proof; ordinary Portal behavior is unchanged.
The eventual helper must still reject unsupported native descendants independently.

Snapshot reads require a complete current pass and unchanged global epoch/native
state; source changes after drawing invalidate a prepared root before publication.
Global state intentionally spans read-model resets. This is sampled classification,
not atomic privacy isolation or prevention of competing same-user operators. Declared
nonsecret fixture content and operator coordination remain required. Existing UI
journeys add pure full/missing/blocked-producer decisions and real API tab/dialog/
native-dialog transition counters; no new test declaration or execution is claimed.


## Completed capture authority correction

SurfaceSnapshot retains the Guard of the actual completed classification pass;
snapshot requires the current product Guard and equality with that stored Guard.
The host also requires ui::complete's actual output Guard/pass before annotation,
then retains the same completed Guard through Last. Fresh navigation, primary,
scene/access or other product identity cannot retag an earlier output. Existing
journeys author real completed Context output followed by navigation and primary
changes, resolved No access, edited-point labels and blocked-access eligibility.
Source assertions are not execution evidence or proof of WGC freshness.
