# Actual native window/context witness

## Stable actual provider root

Bevy_winit0.18.1's direct activation handler returns a tree rooted at the actual
Window Entity bits. AccessKit_windows retains that fixed node ID; replacing it
with egui's accesskit_root ID can make an already-retained provider unavailable.
ProviderHandoff therefore normalizes the same real Window root to that existing
ID before navigation remapping, annotation and the actual SDK publisher. The
current OwnedRoot supplies the exact Window generation; no second root or SDK
fork is introduced. Owner QA continues reading physical egui output earlier.

The bounded full-tree/root-role/reference/focus rewrite rejects collisions,
including issued alias tombstones. Source identity must be the public egui root
ID; the destination is only the current OwnedRoot Window generation. It validates
both sides and runs before both positive aliasing and sanitized fallback. Failure
empties the actual output slot and
uses existing provider retirement/quarantine; no unnormalized cached fallback.
Only a fresh independently validated full tree can recover. Original physical
button/private target identities and all guard/deadline/witness/input semantics
remain. Root continuity across Context replacement does not preserve a witness:
existing lifetime observers and binding epochs still retire it. The existing
handoff journey checks stable initial/recovery root, alias/collision refusal and
unchanged real buttons. Source assertions are not native execution evidence.

## Owner QA observation ordering

The guarded Owner QA service reads the current completed physical-ID AccessKit
output after egui ProcessOutput and before ProviderHandoff. Native processing then
remaps or suppresses that same output before the Windows publisher. An inactive
native handler therefore cannot erase the Owner observation before it is served.
This ordering adds no retained snapshot, native activation or authorization bypass;
existing Owner, scope, opt-in and redaction checks remain in the QA service.

This is ordinary read-only accessibility metadata, not a QA endpoint or authority.
The application publishes one positive x64 integer HWND property named
FractalEngine.NativeContext.v1 and matches it in the actual completed AccessKit root
AutomationId fe.native.window.v1 / FullDescription. No names, URLs, tokens, role values,
entity properties, user strings or action acknowledgements are included. Unsupported
platforms/architectures publish nothing. The operator helper and WGC are separate
milestones; this module does not invoke UIA, capture, launch a helper or change HTTP.

## Lifetime and native ownership

book.rs holds a checked process-static allocator and serialized current publication,
not a resettable UI Resource. Exhaustion, foreign property or native operation failure
poisons future publication until process restart. First clears the previous property
before ordinary UI mutation/rendering. Component Insert/Replace/Remove observers for
RawHandleWrapper, EguiContext, Window, primary tags, RenderTarget and multipass schedule
invalidate immediately, even when Entity/handle bits are unchanged. Mutating an existing
raw wrapper is conservatively detected through its change tick before annotation.

Binding retains the actual RawHandleWrapper through native property cleanup. Its
private internal Arc cannot be compared; no equality is invented from numeric HWND.
Full Context Entity/actual egui Context Arc and exact handler identity come from the
read-only native_accessibility::Ownership seam. It calls the SAME unique-context
validation and Adapter matches predicate as the input owner, accounting for its exact
detached Arc and quarantine. Publication never releases that queue or modifies input.

All raw-handle extraction, wrapper retirement and publication run with NonSendMarker.
Observers can execute on an ECS mutator thread. They clear only the cached HWND/value
under the same mutex used for publication, advance the revision and retain the wrapper
in Witness until main-thread cleanup. They never extract/drop a raw wrapper off-thread.
This serializes ownership check + removal/publication, so a delayed observer cannot
remove a later host publication after comparing an earlier value. A failed lock or
property operation stops publication rather than replacing foreign state.

The cross-thread property cleanup is an INFERENCE from primary Microsoft contracts:
SetPropW/RemovePropW explicitly discuss cross-process UIPI and do not state an owner-
thread restriction; it is not an explicit universal thread-safety guarantee. Root
captured these before authoring in common .omc/research/native-properties-contracts-
2026-11-03.md and native-properties-clock-primary-search. Primary sources:
https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setpropw
https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getpropw
https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-removepropw
The ABI and handle retention remain subject to independent review and actual native
acceptance. Same-user equivalent-privilege tampering is outside this identity model.

## Completed root and publication order

After ProcessOutput, annotate before the actual public bevy_egui update_accessibility_
system. Require A1 live exact ownership, complete current output, current positive
surface snapshot and real retained raw window. The root alone gets a bounded ASCII
FEW1 record with process PID/creation FILETIME, window/context/binding/surface epochs,
completed pass, machine uptime milliseconds, fixed Scene/profile and completeness.
Normal root Name/role/actions are not changed. No competing/synthetic tree is emitted.

Last runs after the normal forwarder and A1 intake/restore. It rechecks actual owner,
source Guard, surface epoch/completed pass, nonfuture age <=250ms and serialized
revision, then writes/readbacks the HWND property. CameraCaptureLock also refuses annotation/publication. Missing output, ambiguity, source
change, quarantine, terminal mode, stale/regressing pass or failed check retires the
binding. Recovery allocates new epochs; a prior binding cannot be recreated by Reset.
A normal fresh frame can retain its lifetime epoch while its pass/tick advance.

An inactive AccessKit provider may not consume its update factory. SetProp success
therefore proves NO provider readiness. An external observer must independently match
the ORIGINAL provider's fresh marked root and actual HWND property before and after
its operation; missing/mismatched metadata is unavailable. No pass/epoch is an action
acknowledgement. Counter serialization is bounded and checked; native pointer-shaped
property bits are opaque integers, never dereferenced or resource handles to close.

## Existing journey and limits

journey.rs is called by the existing native input/clipboard journey, not a new test.
It authors actual EguiContext insertion/replacement/removal observer checks, process-
allocator persistence across terminal/read-model reset, exhausted successor refusal,
and exact/absent/foreign property decisions. No native handle is fabricated. The
stale-revision publication case returns before FFI. It has not been executed here.

A read-only BrowserCommand reader runs before annotation and permanently marks native
overlay state unproved on every non-Close/GetUrl command, without reading URL payloads.
This covers standalone PetalPortal paths as well as the UiManager portal action; it
neither drains the backend's reader nor touches the separately owned QA Portal bridge.

The UI classifier owns the positive empty-Properties profile; see its directory guide.
Selected Inspector/asset peer audit remains mandatory campaign work. UIA controller,
bootstrap ledger, same-worker original COM selector and one-attempt latch are not
implemented in this milestone. WGC remains unavailable: primary documentation proves
SystemRelativeTime units/QPC but duplicate/stale DWM frame behavior still blocks an
accepted freshness predicate. No timestamps/pixels/metadata fabricate rendering proof.


## Lifecycle and completed-pass corrections

A separate Lifecycle resource keeps one MessageCursor over the actual AppLifecycle
stream, shared by First, pre-annotation and Last observation. Only Running is
eligible. Idle/WillSuspend/Suspended/WillResume retire property/binding/preparation
on the main thread, even when Windows retains its RawHandleWrapper and Context.
A later Running transition also retires prior state; a new completed output and
new binding epochs are required. Running alone cannot prepare or publish a root.
The cursor is outside ordinary UI/Witness reset and does not replay events between
phases. The existing pure App journey sends actual suspend/resume messages while
retaining the same Context entity and authors unavailable/fresh-epoch assertions.

The actual completed AccessKit Snapshot Guard/pass must equal the owned root and
positive SurfaceSnapshot before annotation. Prepared retains that completed Guard;
Last compares it with actual current ownership and the same stored surface proof.
An annotation-time Guard never substitutes for the rendered pass's authority.
The UI guide records the separate source-audited direct-painter contract. Fixed
ruler output is admitted; active edited-point labels are conservatively unavailable.
Areas visibility is not described as exhaustive paint-layer accounting. These
changes add no helper/capture mechanism or claim of runtime/native acceptance.

## NAV-1B actual provider handoff and quarantine

provider_output.rs runs after ProcessOutput and before annotation/the named egui
forwarder. Its independent bounded full-tree validator covers unique connected
root/children and all relationship/focus closure. Only that safe original full tree
may replace an alias baseline unmarked when positive remapping fails. Unsafe,
incremental, missing or ambiguous output is removed from the actual EguiOutput;
no cached remainder, partial identity mix or empty replacement is forwarded.

The process-static quarantine is independent of Witness/read-model resets and
positive annotation. Owned primary-context early failures suppress actual output;
fresh validated complete output is the only recovery. Issued-ID tombstones live
in the UI process-static identity ledger. A retained provider tree is stale
presentation, never typed navigation authority. Quarantine clears original typed
mapping/intake and prevents both annotation and Last publication.

After the named egui forwarder and before AccessibilitySystems::Update, the
existing ManageAccessibilityUpdates gate stays disabled. Pinned bevy_egui can
otherwise enable this global gate on any None output; pinned bevy_winit's gated
ECS publisher updates the primary window. The validated external GUI remains its
sole primary publisher after safe full recovery as well, so another context's None
branch cannot re-enable an alternate primary update. Other window handler/input
ownership is unchanged; another context cannot supply recovery for this root.

The same real Petal Button is remapped before actual forwarding, including every
typed relationship, root and focus reference. Annotation emits class=navigation,
profile=navigation_petal_v1 only when the completed original proof/set and actual
forwarded map remain current. Last checks them again with fresh time. Scene metadata
keeps class=scene/profile=scene_empty_v1. Neither profile adds WGC capture authority.
The existing pure journey now exercises actual EguiOutput handoff, unsafe delta/
reference/focus refusal, the alternate gate and validated full unmarked recovery.
These are authored assertions; no Windows provider or live navigation is proved.

## NAV-1B-S1 irreversible unsafe-output retirement

The pre-forwarder handoff uses navigation_ui::provider_handoff on the actual output
slot, coupling unsafe/missing full-tree validation with failure retirement. Unsafe
or ambiguous owned contexts are retired even when no positive OwnedRoot is available;
the query uses the actual mutable EguiContext getter. Failed positive remapping and
held aggregate quarantine also retire current eligibility. Routine prepare-entry
cleanup still uses suppress, so a healthy multi-pass bounded read can complete.

Safe complete unmarked recovery cannot restore the former set/target epochs or
provider alias. Only newly admitted DB proof bindings can later remap, while old
COM aliases remain tombstoned and typed requests/deliveries/callbacks remain stale.
The owner retains an accepted DB read through retirement until its exact reply;
repeated held passes do not fabricate a second in-flight request.
