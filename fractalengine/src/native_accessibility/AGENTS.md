# General native accessibility input owner

This module connects the ordinary OS AccessKit handler to normal egui RawInput.
It has no HTTP receiver, QA opt-in, token, entity role grant, direct UiAction/DB write,
HWND witness, observer/capture facility or action acknowledgement. Viewer/Editor GUI
controls retain normal authority checks. Only Click+None is supported; Focus is
explicitly deferred because egui processes it before the final product use guard.

## Identity and phases

Require exactly one real primary Window and one actual root context whose forward/
reverse WindowToEguiContextMap, RenderTarget, primary tag and automatic/default pass
schedule agree. Extra mapped/output/actual contexts, manual/nonroot scheduling or
inconsistent maps refuse. The owner binds full window/context Entity generations,
actual egui Context Arc equality and exact Winit handler Arc. Replacement/removal or
terminal retirement discards input/validity; former context IDs cannot become new
context input. The ordinary local UI guard is separate from any future external HWND
identity proof.

PreUpdate delivery follows WriteEguiEvents and precedes BeginPass. It admits at most
one event for the current completed pass after product and neutral-input checks.
The UI root rechecks current product state before widget traversal. PostUpdate intake
uses the latest completed snapshot, after egui output/window chrome and before Bevy's
AccessibilitySystems::Update. A later completed output is required before another
request can be delivered. Delivered input is never replayed after timeout/discard.

## Sole queue consumption across Bevy's poll

Pinned Bevy0.18.1 poll_receivers loses the window identity. Merely draining before it
has a race: the provider can enqueue between intake and the poll. During that narrow
PostUpdate bracket, detach ONLY the bound primary handler entry from the public map
and retain the exact Arc in DetachedHandler. Winit's provider still writes that Arc;
Bevy's poll sees no owned entry and cannot steal a late request or block on its mutex.
After AccessibilitySystems::Update, restore only if that exact Window is still live,
no WindowClosed was observed and no different handler was installed. Never overwrite
a replacement. Other window entries/queues remain unchanged.

Loss of a unique actual PrimaryWindow quarantines the exact former queue instead of
dropping its ownership. PreUpdate cannot rebind to another window while that queue
is still mapped. PostUpdate intake also runs during terminal mode: it detaches/refuses
only the former exact handler and preserves the bracket around every flattened poll,
including late enqueue. The retained owner has only window/context Entity IDs and
handler Arc; its egui Context, snapshots, pending requests and source validity are
retired. A replaced/unmapped/closed owner releases quarantine without removing or
draining a different handler. Returning to the same unique owner requires fresh output
and discards uncertain backlog before normal admission. A different primary with the
old live mapped owner remains conservatively unavailable rather than choosing one.
Terminal retirement uses the same current-InputState plus buffered-RawInput cleanup;
it never restores a held entry before its scheduled post-poll reconciliation.

Only this adapter drains the owned queue. No flattened ActionRequest reader exists.
Unsupported targets/actions/data are consumed once with bounded diagnostic counts.
Unknown context/output clears eligibility; owned queue contents are refused, not sent
through a second route. Only try_lock is used here. Contention clears pending validity;
two consecutive failures report unavailable. The pre-existing SDK queue can allocate
unbounded payloads before intake, and unrelated SDK queues retain SDK behavior; this
slice does not claim an upstream allocation/global locking bound.

Ownership/context/product-source changes, lost complete output, overflow or a lock
continuity gap install a fresh-output barrier. Its first complete snapshot drops all
uncertain upstream backlog before eligibility resumes. Requests that waited through
an unknown interval cannot be stamped fresh merely because the mutex/output returned.
This may refuse an early otherwise legitimate native click; it never replays one.

## Bounds and limits

At most 32 queued/inspected requests, 250ms host-admitted pending age and 4096 snapshot nodes. Queue
overflow refuses the batch and clears remainder, never truncated-success delivery.
No payload clone/logging. Snapshots retain only bounded ID/action/layer/eligibility
metadata. Age/current context is checked again at actual delivery. Refusal diagnostics
contain counts/reasons, not labels or values. UIA HRESULT still means upstream
submission, not confirmed action/DB/history/render success.

journey.rs extends the existing input/clipboard journey with actual pinned handler
queues and ordinary egui input, without creating OS windows. It authors identity-map,
late-enqueue bracket/replacement, unrelated-queue, per-pass order, no-replay, age/cap,
unsupported data/Focus, context/handler/window replacement, contention and missing-output
checks. The existing helper also runs a pure Bevy App through production intake/restore
and a local stand-in for the pinned SDK private poll (its public-map drain), with zero/
two primaries, late enqueue, unrelated-window publication, and fresh-output recovery.
A PreUpdate begin_pass -> Update terminal freeze -> replacement widget traversal
checks current and buffered input retirement while preserving physical input. No OS
windows or actual private SDK poll are invoked by that fixture.
Source authoring is not test execution. Parent owns builds, the final integrated
test sweep and real native UIA/Viewer/Editor acceptance before any observer uses this.

The upstream ActionRequest has no enqueue timestamp or operation nonce. Its age
before host intake cannot be proved; the 250ms bound starts at actual host admission,
not OS submission. No input-consumption acknowledgement is invented from this bound.
