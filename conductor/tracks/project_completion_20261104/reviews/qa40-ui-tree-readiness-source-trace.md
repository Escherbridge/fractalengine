---
type: Review
status: source_reviewed_live_pending
---

# QA40 Owner UI-tree readiness: source trace and narrow correction design

Reviewer `/root/legacy_helper_io_review`, 2026-11-04. Source/evidence only; no app or process operation.

**This is a regression mechanism, not a required long-startup state.** New native provider handoff can remove the completed tree before the existing Owner QA reader sees it. The retained QA40 state/409 records strongly fit this path. The actual runtime AccessibilityRequested/ownership branch was not independently inspected; the classification below is grounded in a concrete reachable production source ordering.

## Retained observation and production path

`qa/passes/40/owner-initial-state.jsonl` reports frame2175, normal1280x720/1x GUI, current Genesis Verse and no pending known UI operation. `owner-initial-ui.jsonl` and `owner-ui-retry-1.jsonl` both retain409/ui_tree_not_ready. The screenshot receipt identifies the ordinary window image; parent inspected it as normal. This review did not open/operate the live app, repeat requests or infer GPU readiness from the generic settled flag.

The exact source path is:

1. `qa_debug_bridge::prepare_accessibility` enables the primary egui AccessKit builders when **QaEnabled OR AccessibilityRequested**. Owner QA mode deliberately works without activating an OS accessibility client.
2. Native A1 deliver, separately, retires/quarantines Adapter when native AccessibilityRequested is false (or exact handler/context ownership is unavailable). `Ownership::current` requires the exact adapter/window/context/handler match; ordinary QA-enabled output alone does not create that native ownership.
3. New `native_window_witness::provider_output::prepare` runs after ProcessOutput. For a PrimaryEguiContext without an exact owned native candidate it sets the **actual EguiOutput.platform_output.accesskit_update to None**, retires native eligibility and holds the native provider. This is intentional native safety behavior, not a reason to force native activation.
4. Existing `qa_debug_bridge::serve` also runs after ProcessOutput, but has no explicit order against that prepare. Their EguiContext/output access conflicts serialize them; absent an edge, a legal consistent schedule runs native suppression before serve.
5. Owner `QaOp::UiTree` reads that same EguiOutput field. None becomes409/ui_tree_not_ready. Owner UiState primarily reads manager state and can succeed without the tree, and window screenshot capture is a separate path. Thus normal state/image with repeated UI-tree failure is coherent.

There is a second related correctness boundary: when native proof remapping succeeds, provider NodeIds differ from physical egui response IDs. Owner QA flatten/layout output is used for ordinary physical response geometry and should not inadvertently combine native aliases with physical ControlObservation IDs. Reading the current completed original output before native mutation preserves that existing identity join.

This trace does not weaken native ownership, permit an unsafe tree to reach Windows, claim native capture, or classify the read refusal as a data-authority bypass.

## Minimal explicit-order correction: design supported

Parent proposed a crate-visible **ProviderHandoff SystemSet**, with the actual native prepare system in that set, and Owner serve explicitly **after ProcessOutput and before ProviderHandoff**. This is the preferred narrow correction; a copied snapshot/read-model cache is unnecessary.

It establishes the existing completed-output fanout deterministically:

1. EgUI ProcessOutput produces this frame's original completed physical-ID output.
2. Existing guarded Owner QA service reads/flattens that same completed output and existing response geometry.
3. ProviderHandoff independently validates/remaps/suppresses the actual output slot.
4. Existing native annotation/real Windows forwarder/alternate-provider hold continue in their reviewed order.

Keep Owner serve's existing before-WindowChromeApply constraint. Actual WindowChromeApply is after PostProcessOutput; the proposed after-ProcessOutput/before-prepare edge fits the current schedule and introduces no evident cycle. Keep the native prepare's existing before-annotation/before-named-egui-forwarder edges. A dedicated public(crate) set communicates this one shared-output ownership boundary without exposing private function internals or globally ordering every unrelated PostProcessOutput system.

The correction must preserve Owner minimum role, loopback/input opt-in, exact view scope, FrameNavigation currentness, primary-context checks, password/text redaction, bounded flatten traversal and existing request limits. It must not force AccessibilityRequested, synthesize a native Viewer/role/proof, bypass adapter identity, stop actual unsafe-output suppression, change alternate publisher ownership or present the Owner tree as native provider/capture evidence.

## Required review and evidence

Before implementation approval, inspect the exact small source delta and directory rationale after parent stops the owned app. Extend an existing journey to verify actual scheduling/fanout, not only a helper copy: a complete ordinary tree with native requested=false/no exact native root must be available to Owner read first, while the same actual output is subsequently suppressed before the stand-in native forwarder. Include successful native remapping: Owner observations retain physical IDs/response joins while the later native update carries fresh aliases. Unsafe/missing original output must remain unavailable to native publication and may only yield the Owner reader's existing bounded readiness behavior.

After compilation, repeat the fresh source-matching Owner UiState/UiTree/screenshot checks and actual named ordinary input without native activation; retain the original failed QA40 receipts. Verify scope/role/privacy refusals still apply. Native provider output, real Viewer/Editor Invoke, typed deadline/retirement and WGC capture remain separately unaccepted. Screenshot-based ordinary synthetic input on the current app can support bounded independent progress if root chooses, but must not be counted as a UI-tree or native proof and must not replay accepted unknown mutations.

No source or Git mutation, build/test, app, OS/process/input/capture, API/DB/store or network operation occurred. Only this trace artifact was authored. Main's frozen live source and native gates remain untouched.
