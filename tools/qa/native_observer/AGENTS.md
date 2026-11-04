# Native observer operator contract

## Retained UIA worker

worker.cpp and worker_protocol.hpp implement only the fixed Windows x64 worker.
The parent controller/staging/lifecycle modules own invocation provenance, leases,
source/EXE hashes, bootstrap and the durable reservation before worker creation.
The worker never closes inherited handles wholesale: related file/process/lease
pins remain inherited until OS process exit, including if the controller dies.
The inherited FE process handle has query/synchronize rights, never termination.

The worker installs its own five-second watchdog before reading stdin. A stricter
absolute GetTickCount64 deadline from the controller can shorten it. The watchdog
terminates only its own worker process. It stays live through COM cleanup and
output. Request bytes are bounded to 64 KiB; fixed ASCII reason-code JSON output
is bounded to 256 KiB and written once. Raw stderr, provider exception text, tree
names, values, clipboard, screenshots and reusable selectors are never emitted.
An unparsed request conservatively reports attempt_spent=true; the controller's
independent durable ledger, never this flag, determines actual reservation state.

The anonymous request pipe completes on zero-byte success or the documented
FALSE/ERROR_BROKEN_PIPE closed-writer result. Other ReadFile failures refuse.
EOF does not validate content: empty, truncated, oversized and invalid UTF-8/JSON
still fail the existing bounded parser. The watchdog precedes every stdin read.

MTA COM and the retained target run on the main worker thread. A dedicated message
loop installs the nonzero process-scoped OUTOFCONTEXT create/destroy hook BEFORE
any HWND/UIA discovery. Its fixed 256-event queue and nonresetting event count,
25 ms heartbeat, maximum 100 ms heartbeat age and generation-matched posted-loop
barrier capped at 200 ms refuse event/gap/overflow. Posted messages are confined to
this worker's own control thread; no application message or input is synthesized.
The watcher is a progress witness, not lossless proof that no window was born.

Process identity checks use the inherited retained handle, PID/GetProcessTimes
creation FILETIME, exact executable path, same nonzero session/user SID, equal
integrity below High, no elevation/UIAccess and the interactive Default desktop.
The source uses the actual app class `fractalengine` from window_chrome::primary_window
through Bevy's Window.name override; a changed class requires source review.
Top-level enumeration is bounded to eight owned windows;
exactly one normal visible noncloaked/nonminimized ownerless root is accepted.
Visible owned dialogs/overlays, extra plausible roots and any native descendant
refuse. Child enumeration is capped at 128; EnumChildWindows' return value is not
a completeness result. Valid retained window, watcher, property/root binding and
the documented sampled limitations remain necessary.

ElementFromHandle is called once for that admitted HWND. Initial provider
activation may wait only on this original outer root for a unique marked root;
it does not replace FromHandle or repair an admitted root. Only after positive
root admission can an invoke worker acquire one actual target and InvokePattern.
The raw-tree walk caps depth at 32, nodes at 1024 and typed candidates at 128;
it rejects unvisited remainder, duplicate runtime IDs, password/modal/unsupported
markers, unknown native/navigation versions and foreign process/native HWNDs.
It reads general static marker/type/safety fields, the strict marked root's fixed
FullDescription and names only on declared typed Petal candidates. Observe mode
reads no names or InvokePattern. Controlled nonsecret fixtures remain mandatory;
SDK allocations before returned string/array bounds are not claimed bounded.

FEW1 parsing is exact, versioned and no-capture. Only scene/scene_empty_v1 and
navigation/navigation_petal_v1 are observable; only the latter can invoke. Actual
HWND property equality, original process birth/window/context/binding/surface,
nonregressing pass and machine-uptime age at most 250 ms are checked repeatedly.
Runtime IDs are bounded consistency facts and opaque signature inputs, never
lookup keys or decoded HWNDs. Actual UIA screen rectangles require the worker's
own PMv2 DPI context; target bounds must fit the client and marked-root bounds.
Host typed registration supplies its independent clip/DB-proof checks.

Invoke mode requires one exact expected Open Petal name, enabled/visible Button,
strict `fe.nav.petal.v1;s=<epoch>;t=<epoch>`, full original ancestry and advertised
Invoke availability. Original element and pattern are retained on the SAME thread.
Later safety walks may compare against the original element but never replace it,
reacquire its pattern or reselect by name/runtime ID. InvokePattern has no current
fields; its original interface is retained and availability is read on the original
element. Foreground and held-key checks are read-only; focus/activation/selection,
scroll/expand, mutation, HTTP and synthetic input are unavailable.

The invoke ledger must already contain exactly canonical bootstrap + reserved
JSONL, bound to the supplied process/run/name hash and exact inherited file ID.
The worker checks the entire prefix SHA-256, appends one canonical intent with
submission_possible=true and actual opaque binding/signature fields, flushes and
rereads the SAME handle before any possible Invoke. It appends no return record.
Partial/corrupt/missing/replaced/oversize state cannot be pristine. After durable
intent, the worker repeats original-object/process/watcher/property/source checks
and calls Invoke exactly once. Every post-arm refusal, HRESULT, S_OK, pipe loss or
timeout leaves the reservation spent; no refresh/replay/restart clears it.

Return code 0 means completed observation or entry into the sole Invoke call,
not successful UI/DB/navigation/rendering. Returned S_OK is submitted with unknown
consumption. Post-source change is reported as submitted_state_changed, without
retagging or inferring a navigation acknowledgement. The controller must still
confirm exact worker exit and inspect its retained durable ledger independently.
Original host one-second target-proof checks remain mandatory at every use point.
No WGC/capture freshness, all-Verse bootstrap or selected Inspector support ships.

Primary local source: Windows SDK 10.0.22621.0 UIAutomationClient.h (interfaces,
properties), fractalengine/src/window_chrome.rs:16 (name fractalengine),
bevy_winit 0.18.1 winit_windows.rs:245 (native class override for Window.name), and
accesskit_windows 0.29.2 node.rs (root/native property and author ID mapping).
Source review does not prove compilation, SDK/provider defaults, real scheduling,
window enumeration completeness, live Viewer/Editor behavior or controller cleanup.

## Controller and recipient CLI

`fe.py --run <explicit run> native observe` dispatches before load_run, token
selection, log probes or any HTTP transport. `native invoke-petal` additionally
requires `--controlled-fixture --name "Open petal <exact declared name>"`. There
is no nth/regex/coordinate/focus/scroll/retry/action/capture option. A paired run
also requires `--pair-run <owner run> --member owner|viewer`; recorded pair links,
exact member process and immutable stage must match. A generic --verse HTTP token
selector is refused for native operations. The existing Owner HTTP floor is intact.

controller.py pins the lifecycle lease, canonical explicit current/pair/stage
metadata, exact app/helper binaries and ancestors before discovery. Current.json
and stage.json remain read-only/no-delete pinned; neither tokens nor logs/DB files
are opened. SHA-256 is read in bounded chunks under the same five-second operation
budget. The retained FE handle must match PID, GetProcessTimes birth converted
from recorded .NET UTC ticks, and canonical staged executable/hash. Parent and
app must share a nonzero interactive session/user and medium integrity, without
UIAccess. The worker repeats its own native identity checks independently.

windows.py reuses the reviewed owned_stdout_windows path/process pins and adds
exclusive read/write leases/ledger handles. Hard-linked files, reparse/ADS/alias
paths and unknown sharing roles refuse. The hidden CPython Windows child uses an
explicit handle list with only related pins, leases, retained process/ledger and
its three bounded pipes. The ownership receipt handle is not inherited. Start is
asynchronous under the one total five-second deadline; stdout is capped at256KiB,
stderr at zero and request at64KiB. Only the original created worker handle may be
terminated. A timeout never means cancellation of GUI input already submitted.

Unknown start/exit retains parent handles in UNKNOWN_LIFETIMES while that host
lives; accepted children retain the inherited related leases until OS exit even
if the controller dies. A late start receives no request and is reaped through
its owned object. Unconfirmed lifetime never releases exclusivity as confirmed.
Reader threads with unconfirmed EOF are retained separately after a confirmed
worker exit; pipe incompleteness never proves no consumption. Exact worker exit,
strict sanitized output and independent pinned-ledger reread precede acceptance.

## Bootstrap, staging and one process attempt

`stage.ps1 -NativeObserver` builds the ordinary GUI plus the fixed x64 SDK22621
worker recipe. build.ps1 refuses published stage mutation, selects the declared
MSVC BuildTools toolchain and embeds asInvoker/noUIAccess/PMv2. stage.json seals
worker SHA-256, source recipe hash, architecture, SDK and exact wire version.
No observer command compiles or uses an unstaged arbitrary helper.

`launch.ps1 -NativeObserver -DisableSyntheticInput` creates a fresh bootstrap
ledger/observer lease before current.json handoff, bound to actual new process
birth, exe/helper hashes and fresh nonces plus their native file identities.
The launcher still performs its existing explicit Owner-authorized setup/readiness;
that setup is separate from the later ordinary recipient CLI. Paired Start passes
these options to both roots. Restart preserves storage, archives prior bootstrap
metadata and creates a new ledger only after explicit old-process cleanup/new birth.
Pair links survive restart. ResumeRun (including restart.ps1) coordinates an exact
membership reseal inside its existing lifecycle lease; see Paired restart below.

Existing input-off runs without bootstrap can only observe, and only if their
recorded immutable stage already contains this sealed helper. The controller may
create its separate observe-only lease/worker receipt directory, never a substitute
attempt ledger. Missing, replaced, partial, corrupt or oversized bound ledger is
unavailable/unknown, never pristine. Neither helper/context recreation nor time
can clear a process's reserved slot. Only explicit owned FE restart/new bootstrap
gets a new process-incarnation slot; no command replays the prior action.

contract.py defines strict canonical compact UTF-8/no-BOM JSONL with unique keys,
exact known shapes and at most64KiB. The original bootstrap is one line. Before
launching an invoke-capable worker, AttemptLedger.reserve appends and flushes a
second line binding operation, mode, process/run and declared name hash, then reads
it back through the SAME pinned file handle. The worker accepts exactly those two
lines and their exact prefix SHA, then appends/flushed/rereads one intent before
its possible Invoke. All outcomes remain spent, including interrupted reservation,
post-arm refusal, HRESULT/S_OK, pipe loss, timeout and host-side expiry/refusal.
There is no reset/retry ledger operation or automatic restart. A separate readonly
observation supplies no reusable selector to an invocation worker.

The returned attempt_spent flag comes from the independently pinned ledger; absent
ledger/refusal can be unknown, never an invented pristine state. Helper exit0 means
completed observation or Invoke entry. S_OK is submitted/outcome_unknown, failed
HRESULT is invocation_outcome_unknown, and no code is an application acknowledgement.
No current root tick extends the host's original one-second permission deadline.

## Worker receipts and lifecycle cleanup

Before worker creation the controller durably creates a bounded operation receipt
under its exact incarnation's workers directory. It records controller/app/helper
identity, then actual worker birth from the retained created handle BEFORE sending
the request. Exit or unknown is appended; no names, token values or provider text
are recorded. Bootstrap, reservation and intent use the versioned fixed wire keys;
ownership receipts are separate, with at most three canonical JSONL records.

Stop/restart/pair cleanup checks these receipts before acquiring the lifecycle
lease, so a dead controller's exact known helper can be safely reaped while that
helper still owns inherited leases. A live/unverifiable controller refuses. Only
the recorded helper path/hash/PID/birth bound to the same app/bootstrap incarnation
is eligible; no PID/name scan or unrelated process is killed. Missing worker
identity is unknown and retained, not inferred unarmed. Existing FE cleanup still
proves its own recorded root/descendants and never calls helper termination action
cancellation. Explicit restart archives unknown outcomes instead of resetting them.
Clean retains runs and referenced stages with unconfirmed helper records, including
old incarnations; parse/cap failures refuse cleanup. These sampled ownership checks
do not solve equivalent-privilege same-user metadata tampering.

Confirmed history uses the canonical receipt and exact current/archived start
metadata, not a dependency on old stage files that ordinary clean may prune.
Only reserved -> worker_started -> exit_confirmed with the same valid worker
identity is confirmed. Phase order, terminal fields, worker birth/image and the
Run/app/stage/bootstrap binding are checked before classification. A lone terminal
marker, repeated terminal or changed identity cannot establish confirmation.
Recorded stage/helper metadata must have the exact supported kind/hash/schema/image
and match the receipt's original start. Foreign copied history remains refused.

Unconfirmed/unknown history still materializes the real stage through
Get-QaStageManifest and matches its complete recorded manifest. Reaping additionally
retains the real helper binary/hash and exact process identity checks. Missing stage
files never imply exit. A two-line terminal without original worker_started remains
unconfirmed, preserving valid early-start-failure records without granting absence.
Only unconfirmed history retains its stage/run; fully confirmed history does not
force otherwise dead runs or obsolete stages to live forever. Records stay intact.

## Bounded retained history

limits.json is the shared producer/consumer policy: at most 256 aggregate JSONL
files across BOTH native roots, counting every attempt ledger and worker receipt;
256 aggregate incarnation directories; and 256 archived start directories.
Empty/failed incarnation directories count too. The fixed two-level native layout
is completely inspected; redirected paths, unknown entries and excess remainder
refuse. A full admitted inventory stays inspectable by stop/restart/clean.

history.py admits an operation before observe-only directory allocation and again
before reservation/receipt/worker creation. PowerShell uses the same limits and
Get-QaNativeHistoryInventory for cleanup and allocation. ResumeRun reserves room
for the archive and any new bootstrap BEFORE creating its archive or stopping the
old FE process; New-QaNativeObserverBootstrap repeats admission before allocation.
There is no record pruning, ledger reset or unknown-outcome deletion to gain room.
At capacity, explicit stop/inspection remains available and new activity refuses.

## Paired restart

_native_pair.ps1 owns the paired membership transition. Initial pair Start records
each member's actual bootstrap (or null), and revision one. A pre-reseal v1 pair
without revision is revision one; absent bootstrap remains ordinary mode.
Every linked ResumeRun validates exact old member/current/link, pair lineage and
the counterpart's actual current/process/stage/bootstrap BEFORE stopping the member.
It retains the counterpart process handle throughout startup. Both native/input-off
and ordinary non-native pairs are supported; link presence never invents bootstrap
or requires native mode. Paired restart preserves stage, API port and input/native
mode. Unpaired restart retains its existing stage/port options.

The start archive retains the exact previous pair bytes/hash and old current,
bootstrap, outcomes and worker records; pair-reseal.json records preparation.
Only the actual newly started FE birth receives a CreateNew ledger. Reseal checks
strictly newer birth, unchanged exe/stage, fresh run/ledger/native-file IDs and paths,
same helper, and actual pristine new bootstrap bytes/file identities. Counterpart
current bytes and retained process must still be unchanged/live. Ordinary mode
receives no ledger. No old action, proof, selector or invocation is replayed.

The authorized launch handoff also parses the subject of its actual newly accepted
protected Owner bootstrap token through the same helper used by initial pair Start.
Scope/type/maximum role and DID syntax must match. That fresh subject must equal the
archived member public DID and differ from the unchanged counterpart DID before
membership replacement; distinctness is never carried forward solely as a flag.
Native observe/invoke does not call this helper, select tokens or probe HTTP.

Under the same lifecycle lease, launch publishes new current.json and then replaces
the authoritative pair.json via same-volume File.Replace. The pair revision increments
and points to its archived parent hash; the unchanged peer's full member record stays
unchanged. Pair replacement is the membership commit point. Native admission requires
exact member process/stage/bootstrap equality and cannot run during this lease.
Links never disappear. A failed/interrupted partial publication remains mismatched
or otherwise unavailable, with prepared/committed lineage and start records for
diagnosis. There is no automatic adoption, rollback, retry or crash-recovery command;
do not claim multi-file transactional durability or consumption acknowledgement.
Restarting a member requires its counterpart live; this supports the explicit Viewer
disconnect/restart flow. Restarting two already-stopped members as a new pair is not
implemented by this single-member transition.

## Existing offline journey and unverified execution

The existing scenarios/evidence-boundaries.py calls scenario.exercise. Cases use
the actual ledger parser/reservation path with controlled pinned-memory/durability
seams, malformed/replaced/partial state, all-outcome spend persistence and strict
intent matching. The actual fe.main parser/dispatch is exercised with Owner token,
HTTP, run-loader and log probes rigged to fail if touched; only the native entry
is replaced. These are offline contracts with ZERO native/application coverage.
scenario.exercise also calls lifecycle_scenario.ps1 on Windows. That unregistered
support script exercises actual shared inventory, complete ownership-record parsing,
last admitted/first refused receipt and start history, aggregate historical ledgers,
and production pair preparation/revision/publication. Pair process and native-file
proofs are controlled seams, clearly granting no native coverage; ordinary/null-
bootstrap and native transitions, stale birth/ledger/stage/counterpart/lineage/issuer refusal
are included. Python exercises the actual receipt admission/inventory at capacity.
No new registered test declaration or passing execution is claimed. The compiled
worker with the controller's real write/flush/close pipe, malformed/bounded request
refusals, real process/bootstrap/reseal/crash behavior and native file IDs still need
the parent's integrated/native acceptance; offline cases cannot credit those gates.

The existing lifecycle support now creates real scratch stage manifests and image
bytes, without overriding Get-QaStageManifest. It archives an old start and changes
the current incarnation/stage, proves unknown history retains the real old stage,
then uses production retention for confirmed history, removes that prunable scratch
stage and repeats complete ownership parsing/helper stop. Unknown/reapable/lone or
duplicated terminal/changed identity and a foreign archived start must still refuse.
Only unrelated process/native-proof seams remain controlled. No application image
fixture is executable and this offline journey grants no actual app/process credit.

Both independent source reviews, C++/Python/PowerShell compilation/parse checks,
the integrated existing journey and real controlled Viewer/Editor UIA/lifecycle
acceptance remain required. Native window/watcher/COM defaults, timing, file flush
durability, inheritance and unknown-worker cleanup are source-authored, unexecuted
claims. Selected Inspector/node/group, all-Verse and WGC/capture remain unavailable.
