# Native observer operator contract

## Confirmed worker stop classification

run_worker retains the existing absolute deadline, controller0.2-second cleanup
reserve, collector limits and confirmed wait/join. When it calls kill because
that budget expires, its terminal code is worker_deadline_stop_requested rather
than worker_exited. This records the controller's request, not proof of which
actor caused death; Windows CPython kill uses TerminateProcess(handle,1), and a
natural-exit race remains possible. Collector/unfinished-IO refusal takes priority.
Unknown process/IO lifetimes retain their existing owners and are never replayed.

Controller durably records the same exact ownership/exit receipt first, then
refuses a deadline-stop code before worker JSON decoding. An unexpected natural
helper exit also refuses before decode as worker_exit_refused; only exact-int0/2
are normal worker contract exits. This prevents an empty/invalid JSON message
from hiding an already-observed abnormal exit. No new receipt schema, output
content, guessed byte count, native read or increased time budget is introduced.
Invocation reservation/intent and independent spent/unknown ledger ownership are
unchanged. The existing exercise checks pre-decode refusal in observe/Invoke
modes while the actual ledger remains spent. Windows-only controlled fake
processes/collectors exercise the production parent branch for natural0/2,
unexpected1, deadline request, natural-exit race, collector/overflow failure,
retained IO, join failure and unknown start/kill/wait. A separately mocked owned
context/files harness calls actual controller.execute and requires its three
durable lifetime phases before decoding, with deadline/unexpected exit never
reaching the decoder. Only that ordering is covered by mocked admission; it
does not prove owned-run validation, real timing, native process or UIA behavior.

QA58's historical stage1/80040201 identifies CurrentAutomationId returning the
SDK's ELEMENTNOTAVAILABLE, not the exact element/phase or root cause. Its separate
follow-up exit1/json_size_refused lacks a captured stop reason/raw size. These
records stay unknown; future classification cannot retrospectively resolve them.

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

Window discovery emits only fixed categories: window_owned_count_refused for the
existing eight-owned-window bound, window_class_query_refused for a failed class
query, and window_unexpected_visible_refused for any visible non-app-class window.
The first callback refusal stays primary. Without a callback refusal, an incomplete
EnumWindows call emits window_enumeration_refused; complete enumeration with zero
or multiple exact-class candidates emits window_root_missing or window_roots_multiple.
Admission still requires complete enumeration, no callback refusal and exactly one
candidate followed by all existing window_current checks. The redundant owner/tool
query in the unexpected-visible branch is removed because both outcomes refused;
every such visible window still refuses. No additional discovery, raw titles,
class strings, window counts/handles or names enter output. The controller allowlist
accepts these six codes and retains window_ambiguous for older staged workers.

## Strict winit visual service

The independently reviewed QA44 plan permits one exact zero-area visual service
from locked winit0.30.13 alongside the original primary, not a class exemption.
inspect_windows retains a bounded eight-entry owned-window census including the
service. It requires exactly one fractalengine primary and at most one exact
Winit Thread Event Target; cross-window thread checks happen after collection,
independent of visit order. A failed identity/class query or truncated class refuses.
Other visible windows, including Portal and dialogs, still refuse. Other hidden
windows retain the existing policy and are reconsidered by each complete census.

service_current requires valid same-process/same-root-thread identity, ownerless
top-level/no parent, the exact class and visible state, successful exact outer
(0,0,16,16) and all-zero client rectangles, and no native descendants under the existing
128-child refusal discipline. winit_service_signature_refusal is the production pure
geometry/style seam: normalized 32-bit style must be exactly0x94000000
(WS_VISIBLE|WS_POPUP|WS_CLIPSIBLINGS) and extended style exactly0x080800a0.
There is no extra-bit mask, alternate value or offscreen/size tolerance.
Same-class malformed services refuse even while hidden.
The source recipe is winit0.30.13 windows/event_loop.rs:904-961; WinUser.h SDK22621
defines the named style bits. Strict runtime mismatch remains unavailable.

QA50 observed2483027968 (0x94000000), exactly the original requested0x90000000
plus SDK22621's WS_CLIPSIBLINGS painting/clipping bit. This is an independently
reviewed observed-profile correction: the precise cause of the extra getter bit
is unproved, not a universal OS normalization rule. The full value is required;
old0x90000000 and every missing/additional bit refuse. All other predicates,
sampled trust limits, retained identities, proofs and Invoke floors remain.
QA50's style-refused sample proves no later component or positive UIA admission.

QA53's exact outer(0,0,16,16)/client(0,0,0,0) measurement replaces the original
outer-zero assumption with one restricted observed-host profile. Zero outer,
15/17/asymmetric extents and shifted origins refuse, without a DPI range or size
tolerance. Winit creates an overlapped zero-size service then changes its style;
cached nonclient geometry is plausible, but the exact16 cause remains unproved.
Zero client area is not a claim that the service cannot receive input/messages.
Every other identity/style/children/retained-set/proof/Invoke floor remains.

QA46 splits the same signature conjunction into fixed refusal categories in its
original order: window_service_style_refused, window_service_exstyle_refused,
window_service_outer_rect_refused, then window_service_client_rect_refused. The
pure classifier uses the same already-read DWORD/RECT facts and returns the first
failed component or no refusal. There are no additional native reads, raw flag/
coordinate values or output fields. The controller retains the legacy combined
window_service_signature_refused code for older stages. This diagnosis does not
relax any exact predicate or identify the failed component in the prior QA46 run.

## Optional service signature refusal diagnostic

QA48's older workers add only optional service_style_u32 to the exact base result
shape. It is a JSON integer in0..4294967295, including zero; bool/float/string/null,
overflow, duplicate and unknown keys refuse. The worker captures the normalized
DWORD from its existing style read after exact service identity and rectangle-read
checks. The strict style comparison, extended-style/rectangle/child predicates and
their order remain. No additional native read, raw window identity, string, error
text, selector or action is exposed. This is one terminal mismatch sample, not a
style timeline or proof that subsequent comparisons passed. Zero is not proof of
successful style acquisition or safe admission.

Legacy scalar validation requires the exact
window_service_style_refused code, raw no-spent/no-Invoke/no-return flags and zero
HRESULT; controller requires actual observe mode and confirmed helper exit2 before
its independent ledger-spent override. A prior spent ledger remains spent. Invoke,
success, other refusal and malformed-request outputs cannot carry the field.

QA51 introduced optional service_signature: an exact eleven-member object
with style_u32/exstyle_u32 in0..4294967295,
and outer/client left/top/right/bottom_i32 in-2147483648..2147483647. Its sequential
facts are copied from the same existing two style/two rectangle reads, after exact
known service identity and successful rectangle reads. No native query is added.
This is not an atomic snapshot or proof that later signature/child checks passed.

ServiceSignatureRefusal derives from Refusal, preserving generic catches and
post-Invoke S_OK -> submitted_state_changed. Only the parsed-observe top-level
catch retains facts. Emit repeats raw mode/flags/HRESULT gates and the unchanged
pure classifier's first failure. Its fixed output-only serializer uses signed
decimal coordinates, appends one closed object to the serialized base and retains
the existing total output bound. No generic raw-JSON Atom, request parser or
canonical ledger change is made; output result member order has no acceptance role.

Controller requires exact nested schema/keys/types/ranges, recursively unique keys,
actual observe/confirmed exit2 and raw false flags/zero HRESULT before ledger
override. It recomputes the first failure at exact style0x94000000/exstyle0x080800a0,
then versioned exact outer and all-zero client comparisons. The only schema literals
are fe-native-service-signature-v1 (legacy outer all-zero) and
fe-native-service-signature-v2 (current exact outer0,0,16,16). The worker emits only
v2; old v1 samples retain their original semantics. No unknown-schema fallback.
Contradictory or all-passing samples refuse.
This validates a diagnostic, never grants positive service/Invoke admission.
New schema version pins those comparison values; future policy changes need an
explicit diagnostic-version decision. Generic identity/read/child/watcher errors,
success, malformed requests and invoke mode cannot carry a signature sample.

Exactly base, base+legacy scalar, base+service signature or base+provider failure
is accepted; mixed extensions refuse. Legacy scalar first-failure semantics remain
unchanged. Service failures emit only the signature object; old controllers safely
reject unfamiliar extensions.
Existing sealed workers without the field remain accepted. Older strict controllers
reject extended output. Stage/helper source and binary pinning, request/ledger/
bootstrap schemas, byte bounds and success/exit rules remain. The existing offline
scenario exercises the production output parser and independent spent ledger;
it does not execute native sampling or establish application/Invoke behavior.
Fresh root-owned failed-code/exit/value evidence and SDK interpretation remain
necessary before any separately reviewed style policy change.

WindowSet holds original root/service HWNDs and threads. Its const current method
compares each fully validated census to those originals; initial service absence
is immutable too. It never adopts a new service or root or repairs a signature.
The original primary checks are preserved, with their unchanged child check shared
through no_native_children. Root identity/thread is rechecked after census/profile
validation. Service facts do not enter UIA, output hashes, private host grants or
the retained target/pattern; the service is never an action target.

The shared operation boundary brackets each complete set validation with the
original watcher/process/deadline checks. It covers provider activation waits,
target acquisition/preflight, both sides of durable intent, final pre-Invoke,
post-return reporting and immediately before positive observe output. The last
preflight boundary follows all target/property reads. All work uses the original
five-second deadline and unchanged private one-second host proof. No selector,
provider, target, source identity or proof is renewed. Unknown progress or changed
root/service/presence/style/geometry/relationship refuses; post-intent remains spent.

The original CREATE/DESTROY watcher and sampled enumeration limitations remain:
quiescence is not proof that styles/geometry never changed between reads, and
public class/HWND facts are not unforgeable lifetime identity. No message, hide,
focus/activation, user-data/title read or window mutation is introduced. The visual
service can receive WM_INPUT/WM_PAINT for the event loop; it is not input-free.

Output keeps the existing observed success code and worker exit rule. Only fixed
service/retained-set refusal strings are added to the controller allowlist, keeping
legacy codes. observed cannot independently distinguish service present from
absent; do not claim a live service-profile match from that code alone. QA44's actual
unexpected class remains unproved. No test-mode, capture or generic action channel
is added. Controlled malformed facts and actual native observation/Invoke remain
separate root-owned acceptance gates; source authoring provides no execution credit.

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
worker recipe. build.ps1 refuses published stage mutation, discovers an installed
Visual Studio 2022 edition with the x64 C++ tools through Microsoft's vswhere,
and embeds asInvoker/noUIAccess/PMv2. SDK22621 and x64 remain fixed; no download or
caller-supplied toolchain path is accepted. vcvars supplies the compiler environment. stage.json seals
worker SHA-256, source recipe hash, architecture, SDK and exact wire version.
The compiler subprocess receives the verified installer directory on PATH because
VsDevCmd invokes vswhere by name. The build script restores its prior process
PATH in finally and preserves NoDefaultCurrentDirectoryInExePath; no user or
machine environment setting is changed.
Compiler lines go to the host stream so stage.ps1's manifest collection cannot
hide diagnostics. SDK VSCMD_DEBUG is absent only during the compiler subprocess
to prevent environment-log generation; the caller's value is restored afterward.
The lean Windows header requires an explicit ole2.h include before UIAutomation.h
so SDK interface and OLE Automation declarations exist before UIA forward types.
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

## Optional provider failure diagnostic

QA56 returned provider_unavailable without the actual failed call or HRESULT.
That code also labels the protocol helper's default checked failures, not just
the two explicit property reads. The frozen QA56 cause stays unknown; a later
diagnostic describes only its own execution and grants no positive UIA credit.

ProviderStage gives each existing default HRESULT method one fixed numeric ID:
AutomationId1, RuntimeId2, SafeArray lower/upper/type/element3/4/5/6,
CompareElements7, ProcessId8, ControlType9, NativeWindow10, Enabled11,
Offscreen12, Name13, Rectangle14, Pattern15, FirstChild16, NextSibling17,
RawWalker18, ElementFromHandle19, BooleanProperty20, FullDescription21.
Repeated method calls share an ID; no element, traversal occurrence or validation
phase is identified. The worker-local checked_provider evaluates each original
call once and retains its original FAILED predicate and provider_unavailable code.
The two explicit com_unavailable calls and protocol helper remain unchanged.
Property failures retain the original HRESULT across the existing VariantClear.
No new COM/native read, retry, fallback, action or admission predicate is added.

ProviderFailure derives from Refusal, preserving generic admission-loop rethrows
and post-Invoke S_OK to submitted_state_changed handling. Only a parsed-observe
top-level catch retains its scalar facts. The emitter also requires raw no-spent,
no-Invoke/no-return flags, base method S_OK and exact provider_unavailable code.
The base hr remains the Invoke result; nested hr is the failed provider operation.

Optional provider_failure has exact schema fe-native-provider-failure-v1 and
stage_id/hr keys; property_id is mandatory only at20/21 and forbidden otherwise.
Stage/property IDs are exact JSON integers, not bool/float/string. Accepted
observe stages are the fixed set1..12,16..21;13..15 are Invoke-only and refuse.
Stage20 permits only SDK properties30019/30077;21 permits30159. Invoke-only30031
refuses. Nested hr is exactly eight lowercase hexadecimal characters with its
32-bit failure bit set. No value, VARIANT type, name, handle, runtime ID, path,
selector, count, message translation or arbitrary provider text is emitted.
Extending stages or properties requires explicit review, not range tolerance.

Controller validates original raw bytes before its independent ledger override:
actual observe, confirmed helper exit2 with exact int type, three false raw flags,
base hr00000000, exact code and closed nested stage/property/failed-HRESULT shape.
Existing recursive duplicate rejection and exclusive optional extensions remain.
Older base-only outputs still work; older strict controllers reject the extension.
Canonical scalar output, bounds, one-write rule, durable ledger ownership and
unknown/spent outcomes are unchanged. No request/bootstrap/ledger parser widens.

The existing offline exercise authors every allowed stage/property combination,
failed-HRESULT endpoints and malformed shape/type/code/mode/exit/duplicate/mixed
extension refusals, while preserving independently spent ledger assertions.
Those cases do not execute native sampling or the C++ catch hierarchy. Independent
source review, root-owned build and fresh live observe remain required; no source
authorship or parser example establishes successful provider/root/witness/Invoke.
