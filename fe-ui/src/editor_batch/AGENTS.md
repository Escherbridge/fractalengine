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

`source.rs` is the shared live admission fence for every UI selection batch. Both
form dispatch and gimbal release call `SelectionSources::start`; its per-member
predicate is also rechecked during held previews. It compares the open editor's full
decoded point rows (handles, corner settings and times included) with captured rows,
then verifies rendered path anchors/polyline or the stamp's exact cached path source.
This applies to property and Group operations too: their full-snapshot projections
could otherwise replace an unsaved path draft. A mismatch refuses the complete batch
before channel admission, without changing the draft or creating pending ownership.
Missing derived geometry/cache refuses; ordinary-only batches require no path cache.
`EditorBatchState::start` remains the channel owner behind this UI admission seam.

Batch Undo/Redo performs the same live source check after its fresh checked read and
immediately before inverse admission. `RenderedSources` resolves the original stable
IDs and promoted-stamp aliases independently of the current selection. Missing,
hidden, ambiguous, previewed or dirty targets refuse the complete inverse before send.
Accepted receipts do not pass this admission gate again or pretend to roll back.

Closed-path checks currently refuse with one shared visible capability reason and
disabled form actions. The prior unconditional closed-shape bypass is removed. Exact
zone cleanup/decimation belongs to the terrain renderer; restoring this capability
requires a host-authored complete-source witness on TrackPickShape, which is planned
as a separate follow-on before feature acceptance, not a permanent regression.

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

Each batch projection carries the owned request's before-path baseline. A later local
handle/point/time draft is retained when authoritative rows arrive, including edits
between DB result handling and host queue consumption. Property hydration clears its
pending-read overwrite flag for that conflict; the host still advances saved geometry
and history. The editor keeps a persistent draft-retained message and shows an advisory.
External current projections have no owned baseline, so differing open drafts are
preserved conservatively. Ordinary checked point projections carry their matching
pending-history baseline through the same policy. Queued projections can be invalidated
by stable node ID when another authoritative source supersedes them.


## Verification boundary

The existing NodeManager selection journey extends its reducer assertions for order,
primary removal, reverse ranges, duplicates, size limits and Petal transitions. The
existing Inspector editing journey extends native rendering to three widths/scales,
common/mixed/missing/null values, explicit primary-only URL labeling, accessible fields
and target sizes. It also exercises complete typed reads, partial-receipt rejection
before projection, busy/disconnected channels, coalesced pure reads and one complete inverse request.
Its existing ECS fixture also exercises inactive handle drafts, property/transform/
Group refusal before any send, changed rendering, missing stamp source and one complete
admission after matching sources are restored. These are source-authored checks;
the parent campaign owns compilation, execution, live QA and acceptance.

## Complete Group deletion and chronological replay

Group Delete owns a typed asynchronous server read, independently of NodeManager selection. Its complete stored targets pass the same original-target source/draft/render checks immediately before the one checked write; hidden Group targets do not need to be pickable, but still need unique current identity, actual transforms and exact derived source/draft agreement. The Group may have no members. Role/navigation/gesture and competing registry-write gates remain active. Unknown accepted outcomes use the original current-read resolution, never a write retry, even for a registry-only receipt.

One confirmed deletion records one chronological history entry. Undo recaptures the original IDs through DeletedGroup and submits only the durable deletion proof ID. Redo re-expands the live Group and compares it to the original expected complete targets/state, then deletes in one fresh transaction; its new receipt replaces the proof used by the next Undo. The inverse does not borrow the current selected set. Complete receipt validation updates a promoted stamp's expected owning-path context from the same complete receipt when its owner is another captured member, and still validates every parent's submitted effect before any projection.

Groups displays persistent errors even with zero selected objects. Registry metadata controls and their left-panel/layer-manager eyes disable while a batch edit/read owns the controller. Rename buffers remain until the authoritative name matches or Escape discards the draft, so full queues or admission refusals cannot silently consume unsaved text. Existing packet/native render journeys cover dependent path/stamp contexts, no-selection Delete/Undo/Redo, fresh proofs, empty outcome reconciliation, pending controls and retained rename text. Source authoring has not executed them.

## Exact owned history links

Both ordinary and checked histories exchange bounded OwnedRestore links before chronological dispatch. Only a complete validated inverse whose reconciliation current state is exactly its own receipt emits a link. Batch endpoints compare complete row/version and promoted-path context; matching registry and world scale retain their independent guards. Shared members and dependent path contexts advance independently, while untouched or unmatched members retain strict original expectations. Links never infer ancestry from a body-only match or an external receipt.

Ordinary commands have typed transform/path preconditions rather than a complete before row. Their exact positive version link must match, and advancing a complete batch expectation also compares every untouched row field with the inverse receipt. Only the ordinary adapter's named fields and transform edit_seq can differ. The shared version proves the predecessor; equivalent visible values under another version do not. Queues are capped at 50 links and 4 MiB; evicted links cause conservative conflict refusal, never implicit rebasing.

The existing Inspector journey extends real controller packets through property + transform, consecutive Undo/Redo, same-value external revision refusal, subset overlap, dependent stamp context and both ordinary/batch link directions. It remains source-authored until the parent sweep.

## Creation lifecycle replay

Creation history retains its latest creation request and the matching successful Undo request. UndoCreatedGroup names the first; RedoCreatedGroup submits only the second. After each confirmed Redo, the new creation receipt becomes the proof for the next Undo. Generic CreateGroup is never reused to resurrect an old ID. Receipt preflight compares a proven Redo with the owned original creation registry vector and exact memberships before recording history or projecting any member; missing owned context refuses while accepted-write ownership remains available for reconciliation. Existing source/draft/render gates and chronological owned-history links apply unchanged.

Group creation explains that large stored values may require smaller groups to preserve Undo/Redo. The capture-times-two check is only the generic local history-memory gate; authoritative initial Group Save admission now reserves the complete unchanged-context lifecycle envelope on the DB writer before persistence. See the database editor_batch/AGENTS.md component table.
