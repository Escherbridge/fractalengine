# Checked selection editing

`EditorBatchState` is a UI read model and request owner inside `UiManager`.
`NodeManager` alone owns selected membership and primary identity. The read model
keys a complete ordered database capture to the current navigation and selection
generation; neither cached values nor local edit roles grant backend authority.

`interaction.rs` builds one bounded geometry/capability read model when a complete
capture is accepted. It caches member kind, render origin/TRS, shared center and the
Move/Rotate/axis-scale/uniform-scale capability intersection. Inspector, Options,
actions and both gimbal adapters share `TransformRoute`: Waiting, OrdinarySingle,
CheckedWholeObject or PathSubSelection. Idle rendering does not decode complete paths.
The cache grants no authority: navigation/selection generation, real history versions,
role and live path source/draft checks still fence use and held gestures.

`math.rs` adapts ordinary transforms, whole-route affine edits and promoted stamp
overrides. Path rows use the runtime's shared f64 affine helper, including relative
Bezier handles and untouched measurement/time metadata. The pivot is the arithmetic
mean of selected object origins; a route origin is its rendered polyline centroid.
Stamp positions remain path-derived. A path and its stamp cannot participate in one
affine operation. Any unsupported member refuses the complete operation. Non-uniform
world scaling of a rotated ordinary object refuses rather than approximating shear.
Uniform scaling remains available through the numeric form and existing snap setting.
Whole-object path/stamp gimbals and their Inspector transform section use this adapter
even for one selected object. Ordinary single-object tools retain their existing
unit-aware Inspector and Undo flow. Point/segment gestures remain separate.

`ui.rs` supplies the shared Inspector/Options form and selected-membership controls.
Current values report common versus mixed components. Property presence distinguishes
JSON null, a shared value, mixed values and missing keys with present/total counts.
Property writes and deletion cover every captured object; schemas are checked by the
backend for every member. Primary-only URL, Data/history/measurements and API surfaces
are explicitly labeled. No plural label can route an unported single-object edit.

`validation.rs` validates the full ordered capture and receipt before any scene or
cache projection. The committed adapter preflights all members before applying any.
Successful snapshots use their actual positive journal versions. A refused legacy
request can reconcile to a pure current read with no history version: an unchanged
snapshot needs no materialization; changed legacy state requests normal hierarchy and
property hydration. It never manufactures version 0 or a baseline version 1.
Complete external/API commit receipts use the shared current-state adapter after
scope, identity, version and whole-envelope preflight, without creating a fictitious
local Undo entry. Unowned read/history results never enter that projection path.

## Persistence, reconciliation and Undo

All selection-channel operations use `try_send`. Full read queues coalesce the latest
selection read and each owned resolution read; frame maintenance retries only those
pure reads. A rejected unsent write stays out of pending state and requires a new
explicit Apply. Disconnected queues report their distinct failure. Legacy hydration
coalesces at most 256 node reads and drains at most eight per frame; hierarchy refresh
uses its existing coalesced owner. Accepted writes are never automatically replayed.

Each accepted input sends one `CommitCheckedEditorBatch`. There is no per-node save
loop and no subset fallback. Gimbal previews derive from immutable captured originals;
release restores those originals while the checked request saves. Selection, source,
tool, role, navigation changes and stranded gestures cancel and restore the full preview.

A missing or malformed reply retains ownership of the original request. Resolution
reads that exact outcome and current state; it never repeats the write. The visible
"Check saved selection" action allows another read after a failed or delayed check.
An affirmative receipt is validated before history admission or projection. Resolution
without a receipt reports the edit as unsaved, including when normal legacy hydration
is required. Navigation cancels an unsent Undo read; an already submitted write remains
owned until its exact outcome is resolved.

Confirmed selection commands share a process-local monotonic logical order with the
existing ordinary Undo history. Undo/Redo selects the most recent matching command,
reads all of its original targets again, and compares complete current versions and
relevant snapshots before submitting one inverse. Any changed member refuses the whole
inverse. History is bounded to 50 entries and 4 MiB; oversize captures refuse before
saving rather than promising an unavailable inverse.

Group creation uses fresh ULIDs and existing non-owning many-to-many membership.
`UndoCreatedGroup` removes the created registry entry and restores exact absent/null/
empty/populated membership representations in one transaction. Typed `StampRestore`
preserves the same presence distinctions for the three named stamp overrides; it does
not bypass reserved-property rules. An inherited arc still cannot move a stamp origin.

Confirmed route rows enter the existing host path projection queue with their actual
positive history version. The host checks owning Petal and newer known committed
versions, plus a bounded per-Petal consumed-version watermark, before any geometry or
editor-buffer mutation. Old ordinary receipts cannot overwrite newer batch geometry
when the two queues drain in one frame. Unversioned current reads request normal
hierarchy/property hydration and report the unknown or unsaved outcome instead. The GPX bridge
rebuilds the rendered route and current edit buffer from that queue, preserving metadata.
NodeManager rebinds stable selected IDs to replacement rendered entities after redraw.

## Verification boundary

The existing NodeManager selection journey extends its reducer assertions for order,
primary removal, reverse ranges, duplicates, size limits and Petal transitions. The
existing Inspector editing journey extends native rendering to three widths/scales,
common/mixed/missing/null values, explicit primary-only URL labeling, accessible fields
and target sizes. It also exercises complete typed reads, partial-receipt rejection
before projection, busy/disconnected channels, coalesced pure reads and one complete inverse request. These are source-authored checks;
the parent campaign owns compilation, execution, live QA and acceptance.
