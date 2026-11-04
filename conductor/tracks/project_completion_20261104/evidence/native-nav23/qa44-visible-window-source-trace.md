---
type: SourceTrace
title: Strict winit noninteractive service window plan
date: 2026-11-04
---

# QA44 visible-window source trace and proposed strict service profile

Author `/root/legacy_helper_io_fix`, source-only, 2026-11-04.
Inspected main `9d1e061d53d91db7680819609c8049129c708eee`.
Only this report is authored; no production source edit or implementation approval.

## Finding and limits

The strongest concrete source explanation is the **locked winit0.30.13 thread
event target**, which is intentionally WS_VISIBLE despite having no displayed
content. The primary-window class override is intact. The eager Portal popup is
another source of a visible non-app-class top-level window during initialization
or actual browser use, but its normal initialization path subsequently hides it.
The winit target persists with the event loop and better explains a repeated
fresh ordinary-window refusal.

Root's actual QA44 observation reports window_unexpected_visible_refused. That
proves the existing worker saw a visible non-app-class window belonging to the
retained FE PID. This investigation did not inspect live HWNDs or learn its actual
class. Source explains a normal reachable cause; it does not establish that this
particular observed HWND was the winit target rather than Portal/another surface.

## Installed primary source chain

Registry root:
`C:/Users/atooz/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.
Cargo.lock:16752-16754 pins winit0.30.13; fe-webview's direct wry is0.54.4
(Cargo.lock:16974-16976). There is also transitive wry0.55.1, not evidence that
a Tauri application builder creates a second main window.

1. `fractalengine/src/main.rs:488-504` passes
   `window_chrome::primary_window()` into build_app_with_options.
   `window_chrome.rs:14-17` sets title FractalEngine and name fractalengine.
   `fe-runtime/src/app.rs:199-203` forwards that exact Window as the primary.
2. Installed `bevy_winit-0.18.1/src/winit_windows.rs:205,245-248` maps the name
   to with_class_name; the created window uses those attributes at302.
   `winit-0.30.13/src/platform/windows.rs:578-580` stores the string directly;
   `platform_impl/windows/window.rs:1309,1367-1369` registers/creates that class.
   The Window Class default at windows/mod.rs:51 is not selected by this path.
3. Separately, Bevy WinitPlugin builds its EventLoop in
   `bevy_winit-0.18.1/src/lib.rs:91,126-128`.
   `winit-0.30.13/src/platform_impl/windows/event_loop.rs:202` unconditionally
   calls create_event_target_window before constructing the runner.
4. That file:904-905 defines the fixed public class Winit Thread Event Target.
   At910-950 it registers the class and creates a top-level window, parent/owner0,
   origin0,0 and size0,0. Extended styles at940 are exactly
   WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW.
5. At954-961 it explicitly overwrites GWL_STYLE with WS_VISIBLE | WS_POPUP.
   The adjacent source explains that visibility is needed for WM_PAINT during
   resizes, while the layered target is not displayed to the user. Its callback
   at2475+ routes event-loop/input messages. Hiding or destroying this target is
   not a safe operational workaround. Its lifetime ends with the event loop
   (`DestroyWindow` at623).
6. windows/mod.rs:170-180 forwards set_window_long directly to SetWindowLongPtrW
   on x64. No additional style-mask policy is implemented in that wrapper.

Relevant installed primary file SHA-256 values:

| File | SHA-256 |
| --- | --- |
| winit0.30.13 windows/event_loop.rs | `f99c748e189ea8ce4bde08ec186ee27306756ce93d5235371e198c07e582c52d` |
| bevy_winit0.18.1 winit_windows.rs | `e25dd4e5cea949cf005dbf492cfeba3516474c63f0f2f270b0ab26b04ca57c24` |
| wry0.54.4 webview2/mod.rs | `2551394dccec7f0f45be580e011a6ca82ad5e2dc8585749decf79b958fcfd38b` |

## Portal alternative

Main installs WebViewPlugin at958 regardless of whether a node URL is selected.
`fe-webview/src/plugin.rs:25,94,150` eagerly initializes ActiveBackend once the
primary winit handle exists; default backend-tauri selects TauriBackend, which
actually constructs wry directly. Repo documentation confirms there is no
tauri::Builder in the binary.

`backends/tauri.rs:103,141` requests a visible WebView and creates its popup before
build_as_child. `win32_popup.rs:115,149-161` uses public class FE_Portal,
WS_EX_TOOLWINDOW and WS_POPUP|WS_VISIBLE with the primary as owner. It deliberately
creates and fronts the popup while initializing WebView2. The success path hides
it at tauri.rs:172; errors destroy it. Later navigate/show can show it again.
The geometry sync at win32_popup.rs:221-234 does not include SWP_SHOWWINDOW.

Installed wry0.54.4 webview2/mod.rs:205,224-226 creates WRY_WEBVIEW with WS_CHILD,
optionally WS_VISIBLE, beneath the supplied FE_Portal parent. That child alone
does not explain a top-level EnumWindows entry. An initializing/shown FE_Portal
does. Other WebView2 internal HWNDs are not established from this source trace.
An actual visible Portal remains an unsupported surface and must still refuse.

## Proposed explicit noninteractive service classification

This is a **review proposal**, not an arbitrary class exemption or an approved
change to the original blanket visible-window predicate. A single narrowly
defined winit service profile can be reviewed as framework housekeeping rather
than a UIA/actionable window. It never grants role, target or provider authority.
It receives no UIA lookup or action. Everything outside the exact profile keeps
the existing refusal behavior.

Keep the existing single EnumWindows census and its eight-owned-window cap.
Increment that counter for the service too. Store at most one service candidate
and exactly one original fractalengine root. Do not select a replacement root,
skip an unexpected window, relax native-child checks, or use the service as a
target. A same-class malformed target refuses even if hidden; it cannot fall
through to the ordinary hidden-window handling. Other hidden windows retain
the existing policy, with later full census detecting if they become visible.

Proposed complete service predicate, all checked from successful native reads:

- Exact public class Winit Thread Event Target, compared using the existing
  bounded class buffer; no prefix, substring, arbitrary caller class or title.
- Valid HWND, same retained FE PID/birth context, ownerless top-level:
  GetAncestor(GA_ROOT)==self, GetWindow(GW_OWNER)==null and GetParent==null.
  No service with a child/owner/native overlay relationship is admitted.
- Nonzero service thread ID equal to the retained original root's thread ID.
  Source justifies this: EventLoop construction and ActiveEventLoop.create_window
  execute on that same Windows event-loop thread. Do not learn/adopt a new thread.
- GetWindowRect succeeds and is exactly (0,0,0,0), including origin. GetClientRect
  succeeds and is exactly (0,0,0,0). Nonzero/negative dimensions or moved zero-size
  origins refuse; there is no size tolerance or offscreen allowance.
- Normalized 32-bit GWL_STYLE is exactly WS_VISIBLE|WS_POPUP, or0x90000000.
  GWL_EXSTYLE is exactly the four declared bits, or0x080800a0. These values follow
  the locked source above and installed SDK22621 WinUser.h:2790,2793,2840,2843,
  2869,2890. Normalize the 32-bit style representation before comparison so
  pointer-width sign extension is not mistaken for extra style flags.
- No additional style bits are presently justified. In particular, do not mask
  away caption/frame/topmost/control/focus flags or add an undocumented
  WS_CLIPSIBLINGS allowance. An actual mismatch remains a fixed refusal until
  specific SDK/source evidence and an independent review justify a revision.
- No native descendants under the service, using the same bounded child census
  discipline/refusal contract as the existing root. Keep its sampled native
  enumeration limitation explicit; EnumChildWindows' return alone is not proof
  of complete absence. No UIA traversal of the service is introduced.
- At most one matching service. Missing required facts, duplicate service,
  mismatched thread/process, changed class/style/rect/relationship/children or
  any unsupported visible window refuses. Merely matching the class never admits.

This describes a zero-area noninteractive **visual** service. It intentionally
receives WM_PAINT/WM_INPUT event-loop messages; it is not an input-free process.
Do not send messages, hide/focus/activate it, read its user data or window text,
reconfigure winit, or infer authority from a public class name.

## Original identity, repeated checks and timing

Return one small retained native window-set context from initial discovery:
original root HWND/thread plus the optional original service HWND/thread/profile.
Initial absence is allowed (at most one); its admitted presence/absence becomes
immutable for this worker. Do not rediscover/adopt a newly appearing service,
replace one by name, or repair a failed signature. Root/UIA objects remain the
same originals as today; no second ElementFromHandle or target-pattern lookup.

The watcher must still start BEFORE initial discovery. Preserve its process-
scoped CREATE/DESTROY refusal, event cap, barriers and heartbeat. Existing hooks
do not prove that an existing window's style/size/visibility stayed unchanged,
so watcher quiescence alone cannot validate this profile.

At EVERY current window_current use boundary, check the retained set as well:

1. Keep original root window_current identity, owner/cloak/desktop/child checks.
2. Repeat a bounded complete eight-owned-window census. It must resolve to the
   same original root and the same optional original service; full service facts
   must match again. Unexpected visible windows and new/duplicate/replaced/
   disappeared service refuse, even without a create/destroy notification.
3. Recheck watcher/process/deadline around this work and the original root after
   it. All operations remain inside the original five-second worker deadline.

That includes initial provider-activation waits, ordinary observe validation and
immediately before its positive output, target acquisition/preflight, both sides
of durable intent, final pre-Invoke and post-return reporting. The existing final
observe path currently ends with watcher.current/process/deadline rather than a
new full native check; the implementation must explicitly include the retained
set there too. Do not rely on a snapshot only taken at initial admission.

The original one-second private host proof, root/window/context/property/source
epochs, retained COM identities and final typed callback checks remain unchanged.
No new profile renews proof or turns uncertain lifetime into permission. Checks
remain sampled, not an atomic/lossless guarantee about every interstitial OS
state or HWND birth. Same-privilege process/window tampering is not solved by
public metadata. Unknown census/lifetime/progress stays unavailable.

## Minimal diagnostic and independent live evidence plan

A separate diagnostic-only build is optional, not a prerequisite to source-plan
review: the locked source already establishes a normal source of this refusal.
If needed to disambiguate the actual live class first, compare only the existing
buffer to the exact known public winit/FE_Portal/default Window Class strings and
emit fixed categories while STILL REFUSING. No raw name/handle/title, extra lookup
or positive exemption is needed for that diagnostic step.

For a reviewed strict implementation, use fixed service-profile refusal codes for
unknown read, bad geometry/signature/thread/relationship, duplicate or changed
retained service. Keep legacy codes in the controller allowlist. A fixed positive
observation code such as observed_winit_service_v1 can optionally distinguish
that the complete service profile was present and passed at all required points;
it changes no output shape, exposes no facts/handles/names and grants no action
acknowledgement. That code and its exact controller handling need review too.
Without such a code, observed alone does not independently report whether a
service was present or absent, so do not overstate the artifact.

Root-owned actual evidence after reviewed source must include the exact staged
app/helper hashes, input-off process-incarnation/run ownership, the ordinary
observer output and independent primary-window/closed-Portal inspection. A positive
service-profile code proves the compiled predicate path matched; it does not prove
arbitrary same-class windows are safe or GUI/DB/navigation consumption. Repeat
ordinary observation, then separately authorized one-visible-Petal invocation;
the original root/UIA/host proof and outcome-unknown semantics remain required.

The existing meaningful native acceptance must cover these refusals without a
new generic action/test-mode bypass:

| Variation | Required result |
| --- | --- |
| Exact original zero-area service and one original root | Observation eligible only after every other existing gate succeeds. |
| Service absent initially | Existing no-service path may remain eligible; later appearance refuses. |
| Moved origin, nonzero client/window rect or geometry read failure | Fixed service refusal; no action. |
| Missing/extra style bit, different class or unreadable style/class | Fixed refusal; no best-effort mask/guess. |
| Foreign PID/thread, owner/parent, child/descendant or duplicate service | Refuse. |
| Visible FE_Portal, dialog, default-class window or other unsupported window | Original refusal remains. |
| More than eight owned windows, even if service itself matches | Original census bound refuses. |
| Original service disappears/reappears/replaced, changes geometry/style or gains children | Refuse retained set; no rediscovery/retry. |
| Same original root/service but watcher gap/overflow, process/context change or host proof expires | Existing refusal remains. |
| Changes after durable intent | Slot remains spent; refuse or report outcome unknown, never replay. |

Use production fact extraction/classification seams for controlled malformed facts
and the actual retained native observer for real normal-state evidence. Any live
mutation fixture must be separately reviewed and root-owned; this report does not
authorize arbitrary SetWindowLong/SetWindowPos/message injection, OS enumeration
or a new helper command. Source-authored tables give zero executed coverage.

## Boundary

Read-only repository/installed-primary-source/Git/hash inspection only. No source
implementation edit, compiler/parser/tests, app/helper/process/OS/native/API,
database/network or Git mutation occurred. No live QA44 state was changed. Only
this report was written. Root owns runtime confirmation, design/security review,
any eventual source implementation, builds and acceptance.
