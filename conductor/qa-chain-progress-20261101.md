---
type: Campaign
date: 2026-11-01
status: in_progress
source: ./handoff-20261101-qa-chain.md
current_writer: /root/nav_clipboard_executor
current_slice: S4 native dialog and window seams
---

# QA chain continuation

The user authorized finishing the full handoff queue. Implementation remains local until the goal is complete.
Latest steering also authorizes final CI and downloadable tagged-release publication after the goal passes.
Starting HEAD: `5bc2aa0`. Existing H3 files are preserved and continued.

## Execution queue

- [x] H3 drive client, pass skill, scenarios and real two-scenario evidence (`e9a8b25`).
- [~] Harness advocate, critic repairs (`80177b0`) and independent post-repair smoke (pending combined build).
- [x] Navigation ancestry and stale-role repairs (`7ef208b`), pending combined live verification.
- [x] A8 CopyText/CopyImage sink isolation (`a369668`), pending campaign-end journey execution.
- [~] QA surfaces: A8 and S1/S3 (`7dcc0be`) landed; S4 in progress, then S6, S7, S2, S5.
- [ ] Surfaces advocate, critic and first full-coverage live pass.
- [ ] QA loop: up to six passes; all gold tasks, 100% coverage and two consecutive passes with zero new S1/S2.
- [ ] Five UX audits, synthesis, and polish W7+ plan.
- [ ] Polish W2, W3, W4, W5, W6, then synthesized W7+.
- [ ] One campaign-end integrated check, clippy, formatting and test sweep; independent verification.
- [ ] Include the actual 18-scenario harness runner and separate iot-bridge workspace; resolve strict relay skips.
- [ ] Reconcile track metadata, findings, reviews and final handoff.
- [ ] Push the finished goal, wait for CI success, verify release assets and publish tagged downloads/links.

## Binding execution constraints

One source writer and cargo owner at a time. Independent read-only reviews may run concurrently.
Apply all fixes before the campaign-end test sweep. Required live proofs and a harness-specific smoke remain permitted.
Extend or swap existing journey tests; do not add small test functions. Never stage `conductor/.conductor_session_log`.
Only stop exact QA process identities; do not kill by process name. Never publish tokens in logs or reports.
Use accurate attribution for the executing agent. The old scripts' Claude trailer does not describe this continuation.
Commit coherent reviewable changes often. Refactor around core interaction boundaries for readability and extension;
use small named helpers and shared rules, avoid duplicated per-call-site validation or speculative frameworks.
No push, tag or publication during implementation; the user's later release instruction authorizes the final stage
once the complete goal and local verification pass. Follow repository release conventions and verify actual assets.

## Findings from initial inspection

Static review identified stale GUI staging (`--example` did not select the binary), PID-reuse risk in stop,
nonisolated settings/OS-keystore identity, Windows secret-file creation/reparse hazards, and unsafe cleanup boundaries.
The next harness repair pass must address these before broader state-mutating scenarios.

The prior live pass crashed in `resolve_local_role_on_nav_change` because the navigation cursor contained a petal
without a fractal. Read-only investigation found parentless sidebar navigation, incomplete downstream clearing on
fractal exit, and startup auto-navigation that could choose a petal from a different verse. Repair ancestry at the
navigation entry points and extend an existing navigation journey, then reverify live.

## Evidence and remaining human checks

Starting polish metadata records only W1 verified (`7ea181b`, `03d0acc`). All later waves remain pending.
H3 evidence: `qa/passes/01/verdict.md`, `qa/findings.jsonl`, commit `e9a8b25`.
Trend: `pass 01: gold 1/2, new S1/S2 2, coverage 2/2`. Gimbal movement persisted X from 0 to
1.8780697584152222; sidebar navigation reproduced S1 `QA-0001`. Maps viewport takeover is S2 `QA-0015`.
Eight findings were recorded. The failed startup's captured PIDs were stopped, and the successful run's root was
gone after its crash; its orphan descendants were not independently proven gone. No convergence claim is made.
Security repairs landed in `80177b0`. PowerShell parsing, process identity/stale-timestamp refusal, Toolhelp enumeration,
source fingerprinting, targeted formatting and diff checks passed. Compile/live smoke remain pending. Old stages are
intentionally rejected; the next build must run `tools/qa/stage.ps1`. The independent safety re-review produced
the follow-up items below, now implemented and undergoing independent static review.

Independent re-review found two follow-ups before live use: Toolhelp child PIDs need a pre-snapshot creation-time
upper bound to prevent accepting recycled descendants, and QA secrets must persist inside the run across restarts.
The in-memory store loses share signer/namespace/canonical scope keys even though the node key is durable. The short
safety follow-up landed in `89be757`, adding RunLocalBackend and closing the PID race. It
also persists known descendant identities at handoff so crash cleanup can reap verified children after the
root has exited. Children created after the last snapshot must not be claimed cleaned without separate evidence.

Navigation/A8 independent static review found no required defects and confirmed pinned egui sink ordering and
meaningful extended journeys. Runtime execution is still pending. S1/S3 is now the sole source/cargo writer and
will build once for combined safety/navigation/locator/probe live smoke. Grouping S1 with S3 shares one debug
bridge and gives S2's accessibility inventory state/log assertions. Live evidence then promoted S4/S6/S7 before
the full S2 audit: window controls enable the three-size layout checks, oversized Maps/Inspector currently block
the viewport, and the offline fixture unlocks terrain-only controls. S2 can then audit all reachable states.

Harness advocate static verdict: REWORK, three bounded changes. Safety writer owns explicit run capture and
`stop.ps1 -Run <owned-run>` cleanup in a try/finally procedure. The S1/S3 client writer owns shared object/envelope
decoding with typed malformed-response refusals and splitting PowerShell 5.1-incompatible `&&` scenario commands.
S5 replaces remaining pixel placeholders with locators using one uniform scenario template.

S1 prerequisite source inspection confirms bevy_egui 0.39.1 has an existing OS AccessKit path behind its
`accesskit` feature. Enable generation for OS `AccessibilityRequested` or the QA gate, capture `EguiOutput`
after output processing, and preserve OS forwarding. Existing input uses physical screenshot pixels, despite
the queued brief saying logical pixels: export transformed physical bounds with explicit coordinate metadata.
Accessible names use `labelled_by` or `accesskit_node_builder(...set_label...)`; hover text alone is insufficient.

S1/S3 implementation preparation: add one typed runtime `qa_debug` bridge and an optional bridge field on the
existing InputSurface, avoiding a new ApiState field across fixtures. Share gated REST/MCP implementation;
use a separate QA read rate key so log polling cannot exhaust the injection budget. Host snapshots run after
egui ProcessOutput, authorize current view at use, discard old navigation generations, and never take the OS tree.
Read selected transforms from NodeManager's selected entity, never stale Inspector form buffers. Exposed pending
operation counts are real; unavailable DB/sync totals are null. Never Debug-format dialogs containing API tokens.
Redact exported tree strings and logs before truncation, including linked label strings, Bearer/JWT values and
named secret fields. Bound both ring entry count and message bytes; do not trace recursively on ring poison.
Clear/partition historical logs on active-verse changes and document cross-thread attribution limitations.

S1/S3 production compile check passed (2m44s); staged AccessKit build and live smoke are in progress. Early
independent review found a HIGH log-origin leak: assigning background events to the current view was unsafe.
The writer changed admission so only an explicit matching event origin retains payload; unscoped/foreign events
return fixed withheld text and static diagnostic metadata. Remaining review corrections being batched before
live use: complete Bearer credential redaction, protected-node checks through labelled_by references, full rate
admission before malformed REST query parsing, typed api_base validation, and finite client wait deadlines.
The QA startup branch also sets a per-run WebView2 profile because the old stage demonstrably held shared
`fractalengine.exe.WebView2` state; Microsoft documents WEBVIEW2_USER_DATA_FOLDER as the supported override.

Current-source stage `89be757-20261101140417507` built successfully in 115.8s. Live run
`20261101-140506-617` (root PID 17824) demonstrated named Maps clicking, ui-state/log/wait probes, and the former
QA-0001 sidebar path surviving with complete ancestry. Per-run `webview/` profile exists, with no new stage-side
profile. Parent independently inspected pass02 launch/Maps/navigation/selected-node screenshots.
The probe measured an 81.53125px viewport after Maps open/close; selected Cube1's Inspector controls were at
y=693..1157 in a 720px window, matching a blank Inspector screenshot. Writer is recording S2 QA-0018 for S6.
S1/S3 landed in `7dcc0be`; `qa/passes/02/verdict.md` records the focused smoke, not coverage/convergence.
Owner and foreign-verse checks passed through both REST and MCP; fake JWT text was redacted. The client returned
typed refusals for object-shaped api_base and infinite deadlines. QA-0001 is closed in pass02; QA-0015 and QA-0018
remain open. The run's recorded process tree was stopped at 14:12:39 local; no owned app/cargo/rustc remained.
Final independent static recheck is in progress against the committed diff.
AccessKit idle/active frame-time comparison remains explicitly pending; CPU time must not be mislabeled frame cost.
An isolated 80-row egui benchmark measured 5.018ms disabled / 6.529ms enabled, which is not actual FE frame cost.
Same-run restart and complete standalone/modal title aggregation remain pending; S4 owns the latter.

S4 preparation found no existing close confirmation: native/X requests currently auto-close. The writer will
add one shared confirmation for OS, X and debug close requests, queued native-dialog answers, physical window
controls and QA-only loopback port selection. It owns source/cargo. S6 root-cause/layout preparation is read-only.
S4 source and client slices landed as `572da0b` and `c002e9f`; final staging/live proof and exact independent
rechecks are in progress. The implementation includes operation-owned input leases, atomic QA save/copy,
fail-closed queued dialogs, one chrome mailbox/confirmation path, and monotonic client waits.

The user's later steering explicitly prioritizes readable/extensible design patterns and frequent coherent
commits. The final publication gate is now authorized: complete the goal and local verification, wait for green
same-SHA main CI, then publish a versioned QA prerelease through the existing tagged pipeline and verify all
seven archives, checksums, full Release run including Docker, and actual download links. The current version is
0.1.0; no immutable tag has been created. Use the actual completion date for the QA prerelease suffix.

Final verification and release preflights are saved in `.omc/research/qa-final-verification-plan.md` and
`qa-release-preflight.md`. Workspace tests alone omit the ordinary 18-scenario harness main and separate
extensions/iot-bridge workspace. Strict relay acceptance still has skip-and-return paths that must not masquerade
as exercised assertions. Use the pinned toolchain, explicit terrain/render feature and bounded log extraction.
The original user in-app checks remain explicit: AC-11 screenshots, opted-in input, and delta-viz P1/P2.
