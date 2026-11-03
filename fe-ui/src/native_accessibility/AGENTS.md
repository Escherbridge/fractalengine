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
also removes staged RawInput events so terminal UI cannot inherit an earlier click.

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
rendered Confirm/Cancel IDs. Snapshot metadata carries the owner salt. A native Click
elsewhere disarms BEFORE the helper evaluates an armed state, independent of widget
traversal order. Missing/recreated context, changed IDs or a mismatched owner disarm.
Current Confirm still fires once, Cancel never fires; physical outside-click and 4s
timeout remain. Normal egui clipboard commands are preserved; the adapter does not
read/copy payloads or weaken the host's separate synthetic clipboard suppression.

journey.rs extends the existing confirmation test rather than declaring another test.
It authors checks for outside-disarm, stale Confirm, valid two-step use, post-stage
modal/gesture/modifier change, Focus/data refusal, disabled/missing/duplicate IDs,
full snapshot replacement/cap and context retirement. It also checks normal copy
commands. Parent owns execution and actual UIA delivery acceptance; no native runtime
or rendering result is proved by these authored assertions.
