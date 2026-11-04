---
type: Review
date: 2026-11-04
title: Native witness correction functional source review
source_artifact_sha256: ff9fbb3b792922a94863e2992fe2b9912a116cf8f72e6e823c1bdde16f80733c
---

# Native witness A2 corrections: independent functional/design source review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for the frozen seven-file source correction atop `9902f43105e922fa9097c00904976d50ac5e821f`.** No blocking functional/design defect was found in the correction or its interaction with the original witness ownership paths. This is source approval only; compilation, actual Windows lifecycle/Accessibility delivery and runtime acceptance remain pending.

## Exact reviewed state

Reviewed `.omc/worktrees/qa-native-a2` at the stated HEAD with exactly the seven dirty tracked files listed in `.omc/research/qa-native-a2-corrections-source-receipt.json`. Independently recalculated every raw file SHA-256; all seven match the receipt. Read the author report, the original main `qa-native-a2-witness-functional-review.md`, the complete corrected host/UI modules, relevant A1 identity/snapshot paths, the serialized property book, the actual painter sources, directory contracts and the pinned Bevy/Winit source. Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md` to the design review.

## Four MED corrections

**F1: original completed authority is retained.** `SurfaceSnapshot` now carries its original Guard, and `surface::snapshot(ctx, current_guard)` requires equality with the completed classification's Guard. Host annotation obtains actual current `Ownership`, requires that surface, then requires `ui::complete`'s original output Guard to equal the bound root Guard and its pass to equal the surface pass. `Prepared` retains that completed Guard. Last compares it against current exact window/context/handler lifetime, current product Guard, the bound annotation Guard and the same stored surface snapshot before property publication. A newly sampled annotation/current Guard can no longer substitute for the old output's authority. Failure still rejects the binding/publication rather than retagging output.

**F2: Windows suspend/resume has an explicit boundary.** The separate `Lifecycle` resource owns one shared `MessageCursor<AppLifecycle>`. Actual lifecycle messages are read in First, after ProcessOutput before annotation, and in Last before publication. Only Running permits positive output. Other lifecycle states retire preparation, binding and cached property; a transition back to Running also retires prior state. The next binding allocates fresh process-lifetime epochs and still requires an eligible completed output. Pinned `bevy_window` registers the AppLifecycle message, and pinned Winit sends lifecycle changes before its update. Its Windows suspend path can retain the Context/RawHandle, which is why this explicit gate matters. No fabricated raw handle is used in the journey. Retained-wrapper retirement remains in systems with `NonSendMarker`; the original serialized cached-property cleanup is unchanged.

**F3: positive capture requires current view permission and unblocked access.** `product_guard` derives the separate `capture_view` bit from actual `role.can_view()` and `!VerseManager::scene_access_blocked()`. Both classification and later snapshot admission require that bit. `Some(RoleLevel::None)` therefore cannot qualify, and a blocked scene with an older resolved Viewer role remains unavailable. Existing role/access generation fields stay part of Guard equality. The new capture fields do not replace A1's ordinary action checks or grant any role/HTTP authority.

**F4: Areas and direct painters have truthful separate contracts.** The function is now named `areas_supported`, and the directory guide no longer claims its Areas list enumerates all paint layers. Independent source inspection found the same three production direct painter calls in the inspected host/UI/terrain/renderer/webview sources: the Foreground scale bar, the same-layer fixed unscaled chip, and Background viewport point labels. Ruler output is explicitly source-audited numeric/fixed content. The actual label producer returns unless an edited track has points; `capture_labels_clear` conservatively excludes every state where that prerequisite holds, including points whose projection would later be skipped. This bit travels in the same completed Guard and is rechecked through Last. Unknown registered Areas remain refused. This is a bounded, documented source inventory rather than arbitrary runtime painter enumeration; future direct producers require audit or profile refusal.

## Preserved ownership and design

The correction reuses A1's current exact handler Arc, actual egui Context and mapped primary-window identity; it introduces no alternate ownership selector, queue consumer or restoration path. The former-handler quarantine/intake/restore code is unchanged. Full Entity identity remains distinct from HWND equality. Serialized book revision/property checks, nonresetting checked epoch allocation, retained native-window wrapper and conditional owned-property removal are unchanged. Lifecycle observation calls the existing rejection seam instead of adding a competing cleanup mechanism.

The added fields/resources have narrow responsibilities: immutable completed authority, lifecycle availability and two capture eligibility predicates. They avoid paint-payload hashing, per-frame source cloning or a general new framework. The one shared cursor also avoids replaying the same lifecycle event independently at each schedule phase. Directory documentation carries the rationale, consistent with the project's editing/readability conventions.

## Authored coverage and remaining acceptance

Existing UI journey code constructs actual completed Context/AccessKit output, changes real NavigationManager and NodeManager state, verifies old proof refusal under the new Guard and preservation of the original completed output Guard, and authors No-access and active point-label refusals. The classifier journey authors blocked eligibility. The existing pure App journey delivers actual lifecycle messages while retaining the Context entity, checks unavailable state/revision movement and fresh epoch progression, and verifies Running alone cannot fabricate preparation. Independent source comparison found no new `#[test]` declarations in any changed Rust file.

These checks are authored, not executed. They do not exercise a full positive actual-HWND root annotation/normal-forwarder/property publication path, real OS suspend/resume, or all source/access changes through an installed production host. Parent compilation and the integrated/live acceptance campaign must establish those outcomes. The fixed-ruler source audit is not an executed visual or privacy proof. The classifier remains sampled controlled-fixture state, without atomic privacy isolation against competing same-user operations.

No operator helper, same-worker retained UIA selector/one-attempt ledger, navigation profile, selected-Inspector profile or WGC capture/freshness proof is added or approved by this slice. WGC remains unavailable; timestamps and witness counters do not establish fresh rendered pixels. Required peer native rendering/capture and later helper reviews remain outstanding.

`git diff --check` returned 0 with ordinary LF/CRLF warnings only. No Cargo/compiler/rustfmt/tests, applications, OS/process inspection, native input/UIA/WGC, API/DB/store access, network requests, Git mutations or production edits were performed in this lane. Work was read-only source/SDK inspection, hash/declaration checks and this review artifact.
