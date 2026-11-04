---
type: ImplementationReport
status: source_frozen_review_pending
---

# NAV-2 / NAV-3 source-only operator/helper freeze

Author `/root/legacy_helper_io_fix`, 2026-11-04. Complete authored source slice for
two independent reviews. This is not approval, compilation, passing fixtures,
actual UIA/lifecycle/durability acceptance or project/CI/release completion.

## Exact foundation, scope and frozen packet

Same isolated checkout `.omc/worktrees/qa-native-navigation-helper`, clean committed
foundation verified before authoring:
`023a5044c27f4255ca4ac48c6a5ccbfa9d0b008c` atop NAV-1A `541aef48`.
All 25 approved NAV-1B v3 raw source/doc anchors were rechecked unchanged. No app,
DB, runtime, renderer, pointer or physical-interaction source was edited. Root's
later reviewed main provider-order/QA-serve correction and QA41 are intentionally
not copied into this checkout; parent must preserve those when integrating tools.

Applied the approved MAIN helper plan SHA-256
`732571e2a04d3d33bdbcf31eac8e5bd232121c20b5611c99f3c061f8e2256b94`,
identity v2 plan `36343fed...`, approved native v3 reviews and shared UI/UX guidance.
One worker owned only C++/header and its additive worker rationale. I owned the
controller/ledger/bootstrap/staging/lifecycle/CLI and offline journey. Worker source
was frozen separately, then its author independently critiqued my wire/lifetime/
cleanup/build integration; that narrow critique is not either required full review.

| Artifact in this checkout's `.omc/research/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `qa-native-navigation-nav23-v1-source.diff` | 152905 | `85a3f1e82fd3e466f2bf7cec6c0393aba77b60a9dbd5a70a4770d1f1c809aea8` |
| `qa-native-navigation-nav23-v1-hashes.json` | 6171 | `6e3b29d54137fe2df6facae39d91af20ec2db643c213bf25d2e3f4a007710bb1` |

The raw snapshot is `.omc/research/qa-native-navigation-nav23-v1-snapshot/` with
all **24** files and the manifest/patch. Eleven tracked files change and thirteen
are new. Raw-file hashes are authoritative for line endings. No new registered
test declaration was added; the existing evidence-boundaries script is extended.
Read-only `git diff --check` passes; added-file whitespace inspection found no
trailing whitespace. The original .gitignore's added rule alone was corrected to
its expected LF ending; no broad source normalization was performed.

Independent worker anchors, also unchanged after integration:

- `worker.cpp`: 40808 bytes, SHA `cb11508933e81da8a90fb49ba69a91152b7bbd94bc855909d2cbb34b59a39c72`.
- `worker_protocol.hpp`: 14503 bytes, SHA `9b9da68575db4ae6db2a72fd4eb1128a8cc304b743b7134c4a833b285d137393`.
- Worker report `.omc/research/qa-native-uia-worker-source-report.md`: 10678 bytes,
  SHA `d427c45f68171ce67df8e990f8b557e5f83e23ce17b8e9e2cc972148e2696903`.

## Exact files / phase ownership

| Files | Responsibility |
| --- | --- |
| New `tools/qa/native_observer/contract.py`, `ledger.py` | Strict bounded UTF-8/unique-key/canonical JSONL and one durable process-incarnation reservation. |
| New `native_observer/controller.py`, `windows.py`, `__init__.py` | Explicit run/member, retained metadata/file/process pins, same interactive user/session/integrity, hidden owned worker with explicit inherited handles and bounded pipes/start/lifetime. |
| New `native_observer/worker.cpp`, `worker_protocol.hpp` | Watcher-before-discovery, original UIA root/element/pattern, bounded safety traversal, strict no-capture witness and single durably armed Invoke. |
| New `native_observer/build.ps1`, `worker.manifest` | Declared x64 MSVC/SDK22621 build, asInvoker/no-UIAccess/PMv2, staged binary/source hash recipe. |
| New `tools/qa/_native_identity.cs`, `_native_observer.ps1` | Bootstrap CreateNew/file identities and lifecycle cleanup bound to exact current/archived run starts and known helper identities. |
| `tools/qa/fe.py` | Native dispatch BEFORE load_run/token/log/HTTP; only observe or declared one-shot Petal invocation. |
| `tools/qa/stage.ps1`, `_lib.ps1` | Opt-in helper stage/manifest, build input fingerprint, shared native lifecycle helper definitions. |
| `tools/qa/launch.ps1`, `restart.ps1` | Explicit observer-capable bootstrap before handoff, input-off requirement, prior native/pair provenance retained, new ledger only for explicit new process. |
| `tools/qa/pair.ps1` | Native options to both roots, explicit member links and safe helper cleanup before pair lifecycle lease. |
| `tools/qa/stop.ps1`, `clean.ps1` | Reap only recorded exact helpers when safe; unknown records/runs/stages retained. |
| New `native_observer/scenario.py`, existing `tools/qa/scenarios/evidence-boundaries.py` | Offline ledger/CLI contract cases in the existing journey, no native/app coverage or new declaration. |
| `tools/qa/AGENTS.md`, new `native_observer/AGENTS.md`, `.gitignore` | Directory rationale/wire/execution limits and ignore generated nested Python bytecode. |

The task's phase2/3 production source is authored; no additional planned code slice
is intentionally left disabled or deferred within these modes. Native/helper/runtime
acceptance remains entirely pending. There is no capture/WGC, generic action, hidden
HTTP, direct UiAction, focus/activation, SelectionItem/Toggle/ExpandCollapse, synthetic
input, implicit prerequisite input or Owner HTTP-floor modification.

## Recipient CLI and pinned controller

`fe.py --run <explicit run> native observe` calls the native entry before any normal
run loader, token selection, log probe or HTTP transport. `invoke-petal` additionally
requires an exact declared Open Petal name and controlled-fixture declaration.
There is no .qa/current fallback or arbitrary helper/HWND/index/regex selector.
Pairs require explicit owner pair run and owner/viewer member, matching recorded
pair links, actual process identity and complete stage provenance. The generic HTTP
--verse selector refuses in native mode. Viewer/Editor native observation therefore
does not implicitly probe an Owner endpoint or read a token file.

Controller pins current/pair/stage metadata and app/helper files with canonical
no-reparse/ADS/alias ancestry and native identity checks. Hard-linked files refuse.
The actual retained FE process handle must match PID, creation FILETIME after exact
conversion of recorded .NET UTC ticks, staged executable path/hash, same interactive
user/nonzero session and medium integrity/no UIAccess. The worker independently
checks its own process/session/token/desktop/window facts; no elevation is attempted.

windows.py adds exclusive lifecycle/observer/ledger handles to the reviewed owned
stdout pin implementation. Related pins/leases/process/ledger and three standard
pipes alone are inherited via the explicit Windows handle list. The ownership
receipt writer is excluded from inheritance. Hidden start runs asynchronously under
the total five-second operation deadline; a late start receives no request and is
reaped through its original object. Stdout<=256KiB, request<=64KiB and stderr=0.
Only that created worker handle is terminated, never FE. Worker has its own watchdog
before stdin and through COM cleanup/output. A stuck provider cannot create replay.

Unconfirmed start/exit retains parent lease owners while that host lives, and an
accepted child retains related inherited pins until OS exit even if controller
dies. Unknown output/lifetime is not a proof of non-consumption. Exact worker exit,
strict output, independently read pinned ledger and still-live app precede controller
acceptance. S_OK stays submitted/outcome_unknown; failure HRESULT is invocation
outcome_unknown. No COM return, root tick, pass counter or helper exit is a GUI,
DB, navigation or rendering acknowledgement.

## Durable slot / bootstrap / staging

Opt-in stage builds the fixed worker using declared installed SDK22621/MSVC x64
recipe and embeds asInvoker/noUIAccess/PMv2. stage.json seals binary SHA, source
recipe SHA, architecture, SDK and exact contract version. Source fingerprint adds
the worker/header/manifest/build inputs. Native command never compiles or opens an
arbitrary unstaged helper. Actual toolchain execution/link readiness is unverified.

Opt-in launch additionally requires input off and creates the process-bound ledger
and observer lock with CreateNew, durable write/readback and native file identities
BEFORE current.json handoff. This does not replace launcher's existing explicit
Owner-authorized setup/readiness. That separate startup procedure remains; the later
recipient native command invokes none of its Owner probes. Existing input-off runs
without bootstrap can observe only when their immutable stage already contains the
sealed helper; invocation cannot opportunistically create a replacement ledger.

One canonical bootstrap line binds actual process birth, run/ledger nonces and
exe/helper hashes. Before an invoke-capable worker starts, the controller durably
appends the reserved operation/process/run/declared-name-hash line and re-reads it
through the SAME pinned file. The worker requires exactly those two canonical lines,
same native file ID and exact reserved-prefix SHA. After read-only preflight it
appends submission_possible intent with actual opaque binding/method/target hash,
FlushFileBuffers and exact same-handle reread; then it repeats original-object and
source checks before its single possible Invoke.

Partial, missing, replaced, corrupt, unbound and oversized state is never pristine.
Interrupted reservation, post-arm refusal, timeout, pipe loss, HRESULT/S_OK and host
proof expiry/refusal all leave the slot spent. No helper/window/context reset,
refresh, re-selection, new observation or time clears it. Only separately explicit
owned FE stop/new process birth/new bootstrap gives a new slot; old unknown outcomes
are archived and never replayed. The original host's one-second permission deadline
remains mandatory through final delivery/callback; client freshness cannot extend it.

## Original UIA worker

The worker's actual app class is `fractalengine`, from main -> window_chrome ->
Bevy's Window.name override; the initial default-winit assumption was corrected
before freeze. Watcher installation precedes the sole ElementFromHandle call.
Process-scoped OUTOFCONTEXT create/destroy, 256-event bound, <=100ms heartbeat and
<=200ms posted-loop barrier refuse unknown progression; they are not lossless
window-birth proof. Retained process plus original HWND property/provider remains
necessary. At most8 owned roots/128 native children; any descendant under the
admitted HWND, including another PID renderer, is conservatively unsupported.

One original outer root is retained. Bounded initial activation waits only under it
for one static marked root; after admission there is no FromHandle repair or root
replacement. Full raw traversal caps depth32/nodes1024/candidates128 and rejects
unvisited remainder, duplicates, password/modal/unsupported markers and unknown
profile/version. Observe reads no names and acquires no InvokePattern. Root fixed
FullDescription is read only after its static marker identity; no values, arbitrary
descriptions, token captions, clipboard or tree names are dumped.

Invoke requires navigation/navigation_petal_v1, one exact declared tagged Petal
Button, visible/enabled/fully clipped geometry, original ancestry/runtime signature
and advertised Invoke. Original actual element and InvokePattern stay on the SAME
worker thread. Later walks compare consistency but never replace the target/pattern
or lookup by name/RuntimeId. Fresh original root/target/process/property/watcher
checks run before and AFTER durable intent, then exactly one Invoke occurs. Normal
pass/tick advance is allowed only with unchanged lifetime/surface/target identity.
Positive output is emitted while original references/watch remain alive; controller
still requires worker exit independently. No capture or frame-freshness claim.

## Cleanup correction found before freeze

The narrow independent integration critique found that scanning a native subtree
alone could accept a genuine subtree copied from RunB into RunA and target B's helper.
That was corrected before freezing this packet. Every record must now match RunA's
actual current or archived start: exact run_dir, input-off mode, app PID/birth/exe,
stage/helper hash; bootstrap records also match that start's run/ledger/file ID and
canonical ledger path. Receipt prefix binds those ledger identities; restart now
archives run_dir as well. Observe-only histories require the same run-start proof.
An unmodified copied foreign subtree cannot provide cleanup authority.

Cleanup does not try to read an exclusively held live ledger to establish ownership;
it uses original recorded start provenance and actual known helper identity. A live
or unverifiable controller refuses. Only the exact staged helper path/hash/PID/birth
can be reaped before the lifecycle lease is acquired. No PID/name scan, dead-parent
tree inference or unrelated kill is added. Unknown worker identity stays unknown
and retained; explicit FE restart archives it without claiming action cancellation
or worker absence. Clean retains runs/referenced stages for unconfirmed records,
including old incarnations; malformed/oversize/reparse/cap failures refuse cleanup.
Equivalent-privilege same-user metadata tampering is outside this local ownership
model, as for the existing launcher.

## Authored checks and actual remaining gates

The existing evidence-boundaries journey calls the new offline scenario helper:
actual ledger reservation/read path, all-outcome spend invariant, malformed/BOM/
duplicate/foreign/oversized/replaced state, interrupted durable append and strict
intent binding. Its actual fe.main parser/dispatch case rigs run-loader, token,
HTTP and log probes to fail if touched, replacing only native entry. These are
controlled parser/ownership seams and ZERO native/application checkpoint credit.
Labels such as HRESULT/timeout in the ledger invariant are not actual COM/crash
execution. No new registered test function or passing result is claimed.

Performed read-only source/SDK/Git inspection, directory rationale, raw hash/snapshot
checks and whitespace inspection only. Existing Git config-ignore warnings were
reported by reads. No compiler, parser/formatter/test, app, native API/OS/process/
input/capture, HTTP/DB/store/network operation or Git mutation was performed.

Both complete independent reviews must approve these exact files before parent
integration/build. C++ linking, Python/PowerShell parsing, existing offline journey,
live inheritance/deadline/pipe behavior, durability/crash/corruption/cleanup tests,
actual process/HWND/context reuse, watcher gap/overflow, provider activation/caps,
real Viewer/Editor one-visible-Petal invocation and unchanged Owner HTTP refusal
remain unexecuted gates. Parent's later main source fixes must survive integration.

All-Verse bootstrap, selected Inspector/node/group audits, trustworthy WGC/capture,
independent rendered/data acceptance, CI and tagged-release artifacts remain later
campaign work. No helper return or source report completes those gates.
