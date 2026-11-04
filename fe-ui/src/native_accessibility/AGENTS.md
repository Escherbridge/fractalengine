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
| fe-terrain/src/ruler_plugin.rs::draw_scale_bar -> paint_scale_bar | Foreground ruler_scale_bar; numeric distance from validated scale math, fixed units and primitive strokes | Audited nonsecret ruler output is allowed independently of Areas registration. Same-pass scene anchor, complete fit and clip are required. |
| fe-terrain/src/ruler_plugin.rs::draw_unscaled_chip | Same layer; fixed world-units text | Audited fixed text is allowed within the same bounded current scene anchor. |
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


## Navigation proof-owner foundation (NAV-1A, not yet input wiring)

`navigation.rs` owns at most sixteen exact rendered Petal demands and one accepted
DB-local Viewer read. Source captures context, observed invalidation generation,
navigation/scene/hierarchy/access/selection/role and current ancestry independently
of the owner's proof-set epoch. The later renderer integration must supply the
actual observed generation; a proof-generation counter cannot be fed back as
this base source. No profile, target tag, command pump or input delivery is
enabled by this foundation. Public Rust types are internal application seams,
not capability-bearing serialized messages or API permission surfaces.

Every demand fixes admitted_at and a checked one-second deadline before channel
submission/coalescing. Full preserves its identity and deadline, with bounded
retry spacing; a late result cannot become fresh. A genuinely new read has a new
request/target epoch, while retained old proofs stay expired. Source/membership
changes retire positive bindings but retain the exact accepted read until its
matching reply, keeping one request in flight. These are read-only outcomes.
Disconnection refuses further submission. Process-static checked epochs survive
read-model resets and exhaustion permanently disables this owner's positive path.

The SAME proof_is_current predicate checks exact source, active request/target
membership, successful bounded DB view and strict current monotonic deadline.
Result admission and single/set binding checks call it. Expiry refuses without
requiring a cache pump or generation change. Later Snapshot/Delivery/semantic
Click, annotation/Last and the actual sidebar callback must retain these original
immutable bindings and call this predicate with fresh Instant at EVERY specified
use boundary. They are not implemented here. Never clear an expired native tag
into an ordinary fallback or claim that fresh root age renews target authority.

The existing confirmation journey adds pure owner/clock/channel/source cases,
including exact-deadline refusal with unchanged epochs. It does not yet exercise
actual typed Petal callbacks or native delivery. Stored local Owner fixture reads
in the existing DB journey are not a real Viewer/Editor native acceptance case.
Operator/ledger/staging, original COM retention, one-attempt invocation and all
planned native/runtime checks remain separate unfinished work. Capture, all-Verse
bootstrap and selected-Inspector support remain unavailable in this slice.

## Provider identity translation

`provider_identity.rs` validates a standalone complete original tree independently
of positive profile/proof eligibility. It requires one connected unique child
tree, all typed relationship/focus references present, at most 4096 nodes and
65536 total node references. Cross-reference relationships need not be child
edges. The limit is conservative; unknown or oversized output is unavailable.
Only real nonroot Button/Click targets with enabled/visible ancestry can be
aliased. The caller separately proves the exact original label, geometry, layer,
ancestry and immutable target/set/source and owns actual-output forwarding.

The process-static append-only ledger retains at most 4096 issued identities,
keyed by original set epoch, target epoch and physical widget ID. Exact keys reuse
their alias without adding per-frame history. A new key gets a checked descending
opaque ID; current original-node collisions refuse before any ledger commit.
Previously issued IDs can never become another binding or be forgotten on UI/
context reset. Issued-ID lookup refuses ordinary native fallback on lock failure.
Cap/counter exhaustion permanently disables remapping. No reset API is exposed.

The transform clones the bounded full update and rewrites tuple keys, tree root,
focus, all eight NodeId vectors, all seven single-NodeId properties and both
TextSelection node references with the same bijection, then validates closure
again. Nonidentity values and character indexes stay unchanged. Physical egui IDs,
pointer/focus memory and the original input tree are not mutated. The host must
still recheck the original proof before swapping the actual output and enforce
unsafe-output quarantine across both provider publishers; this pure transform
does not authorize forwarding, native delivery or recovery by itself.

`provider_identity_journey.rs` is an unregistered helper for the existing native
journey. It authors complete reference/focus rewriting, stable original IDs and
values, exact-key reuse, fresh aliases for target/set renewal, retained tombstones,
malformed/delta/unknown-focus/relationship/child refusal, hidden/disabled targets,
collision and exhaustion checks. Its relational fixture exercises typed reference
rewriting; it is not an accepted Windows TextSelection or UIA navigation fixture.
No execution or Windows-provider acceptance is claimed by these source assertions.

`navigation_ui_journey.rs` extends the same registered journey through actual egui
Context/Button output, record/finish/remap/complete, typed staging/begin_pass and
deferred callback admission. Its default manager accepts the same typed
LocalRoleResolved path as production; product_guard is never overwritten. Its
bounded exact-family target-view reply is still a fixture, not real DB Viewer acceptance.
It checks provider-to-physical delivery, stable physical widget ID, native physical
ID refusal, old-binding/new-proof separation and retirement before begin/callback.
The actual host Adapter.next and this journey use the same checked Binding::translate
operation; full host scheduling and Windows-provider acceptance remain separate.

## NAV-1B rendered bindings and use boundaries

`navigation_ui.rs` connects the NAV-1A owner to actual sidebar Petal responses.
The plugin-owned NavigationReads resource retains its one accepted DB read across
context retirement; a private result cursor observes only the typed read family.
Product guard calculation refreshes the actual source at each owner/host/UI boundary.
The base observed generation is independent from proof/publication epochs. Guard
carries the current proof-set validity key, while same_base preserves unrelated A1
eligibility when only a background proof key changes. Fresh monotonic predicates
remain mandatory; matching generations never extend the original one-second lease.

Only fully clipped/enabled actual sidebar Open Petal responses collect targets.
No browser-card, eye, Options or label-derived target is tagged. Completed remapping
checks the actual pass/widget, layer, exact Button/Click/name/tag and response bounds.
The renderer tag grammar is `fe.nav.petal.v1;s=<set epoch>;t=<target epoch>` with
checked process-lifetime decimal epochs and no entity names/IDs. Exact original
proof and whole-set bindings flow through Snapshot, Pending, Delivery, begin_pass
and the final callback. Typed requests addressed to original physical Petal IDs
cannot enter the ordinary fallback, even after a native request was stripped.

Native callbacks defer only until the guarded sidebar returns, so the root can
refresh actual topbar/modal/path and manager inputs. The sidebar helper then checks
actual ancestry/names and the retained original proof with fresh now immediately
before its existing deselection/navigation sequence. Physical callbacks keep their
existing path and IDs. Only the exact staged native click suppresses its own
ResponseExt observation; every other click/change still advances observed state.

The explicit navigation_petal_v1 surface requires the complete audited producer
set, no active Petal/selection, resolved can_view and current DB-local Viewer proof
for every visible bounded target. Its classification is navigation, never Scene;
it grants no capture support. Annotation and Last separately require that the real
forwarded mapping still owns the same unexpired bindings. Missing/unsafe forwarding
retires typed intake independently of a positive profile. Existing scene_empty_v1,
ordinary A1 authority and the Owner HTTP floor remain separate.

## NAV-1B-S1 provider failure retirement

Routine suppress only clears per-pass forwarded/callback state. The distinct
retire_provider operation first advances the existing checked observed source
generation, then retires ProofOwner eligibility and typed state without releasing
its accepted in-flight read. The generation fence invalidates retained bindings
even if the session try_lock cannot be obtained. Poisoned/exhausted/native-unproved
observed state also refuses current bindings. Fresh source admission therefore
needs new request/set/target epochs and new provider IDs; no old proof deadline
or issued-ID tombstone is changed.

provider_handoff owns the actual output-slot take/validate/safe-unmarked fallback
seam used by the host and existing UI recovery journey. Unsafe/missing original
output invokes failure retirement; routine successful full output does not. Safe
unmarked recovery grants no old typed eligibility. The journey now publishes a
real original alias, quarantines repeated unsafe/missing output, recovers and
remaps through fresh proof inside the original deadline. Old Snapshot/Pending
matching, translate, retained Delivery and callback remain refused. A separate
accepted-read case proves no second submission before the exact stale reply drains.
These are authored model checks, not executed Windows-provider acceptance.

## NAV-F1 fresh accepted-role bootstrap

The UI journey no longer assigns Guard.access or the local role cache directly.
It starts with the manager's real default ReplicaReads and applies a matching
LocalRoleResolved through the same crate-private dispatcher seam that updates
accept_scene_role, the existing role cache and Pen state. Current-Verse with no
Fractal/Petal is explicitly exercised through one bounded target read and later
navigation profile/remap; current-Fractal and Editor cases remain covered. Stale
scope cannot initialize access, None/unknown roles cannot view, and repeated
accepted roles retain their existing source epoch. The existing node_index
invalidation-preservation assertion is unchanged. The target-view reply remains
an explicit fixture; no live DB Viewer or native-provider result is claimed.

## Bounded ruler producer (QA41)

The separately source-audited Foreground ruler_scale_bar remains fixed/numeric
nonsecret direct painting, independent of Areas enumeration. The host now
publishes only a current actual scene anchor after GardenerUiSet::GeometryReady;
RulerHudSet orders its bridge before the real ruler draw in the same egui pass.
Both the numeric bar and fixed unscaled chip require complete bounded fit and
scene clipping. Calibration uses actual primary-egui orbit Camera physical
viewport dimensions plus current pixels-per-point, not the scene anchor height.

No ninth mandatory surface producer, post-finish registration, dynamic capture
classification, Owner/native output reordering or positive grant is introduced.
The eight actual surface producers and their finish stay as reviewed. Edited
path labels retain their independent conservative refusal. This inventory update
does not claim capture evidence or safety for arbitrary direct painters.

QA41 v2 also carries actual same-pass completed viewport chrome into ruler
placement. Complete fixed/numeric footprints move above occupied responses or
omit; the native producer inventory and authority are unchanged. The host orders
outer EndPass after CameraUpdateSystems and TransformSystems::Propagate, retaining
EndPass -> ProcessOutput -> Owner observation -> provider handoff/annotation.
