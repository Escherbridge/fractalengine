---
type: Review
date: 2026-11-04
title: Shared scene chrome integration review
---

# QA-0071 v2: independent exact integration review

Reviewer `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the exact 14-file staged integration.** Approved pointer contributions and prior physical-target changes are preserved. No unresolved conflict, unexpected staged path, dropped main contribution or newly authored bypass was found. This is preservation approval, not compilation, live input acceptance or finding closure.

## Exact integration anchors

- Main HEAD: `6f473264be0ba2cc9f4f87328da399f0b9e80be1`.
- Approved isolated pointer v2 source commit: `5b5873d6fe9883aeb680a3afa26112412545db90`, source base `35485826b7008745133918e7ca67f3a278d39a12`.
- Actual staged `git diff --cached --binary`: **50391 bytes**, SHA-256 **`ea80a5817bdee1466da6332382b0c881947e95ac2b42316efdca9070b2f453d7`**, matching main integration receipt.
- Approved frozen v2 source patch remains `b7b414ca5ca66aa0173123582f65f7ac6a0d6c7db6bfb8007384e3dcaaa4ef32`; v2 manifest `dcc1749f76b1dff00f5aad0462594c23f2d1f95f7d595031bc6b12411aa1f1b5`.

Independently verified that the staged path set equals exactly the frozen manifest's 14 paths, every frozen raw source file still matches its approved hash, and every frozen normalized source file equals its committed source blob. Every staged blob hash matches the integration receipt, and each changed working-tree file equals the index after only CRLF-to-LF normalization. No conflict marker was found. `git diff --cached --check` passes.

Read both main v2 source approvals: all three original PTR blockers were corrected and the exact revision was approved for integration. The prior v1 security lock characterization remains explicitly corrected in the v2 review; this integration uses the corrected source.

## Twelve full-file equalities and complete two-file overlap proof

Twelve staged full files equal the complete approved pointer source after newline normalization. Only `fe-ui/src/ui_shell/layout_policy.rs` and `fe-ui/src/viewport.rs` differ, retaining intervening physical-target source from main.

For each of those two files independently compared the **complete content-change operations** from pointer source base `35485826` to main parent HEAD with the complete operations from approved pointer source to index. They match operation-for-operation and exact line-content-for-line-content after newline normalization. There are no additional source-to-index differences.

- `layout_policy.rs` preserves all physical target constants, quantized target sizing and scale refresh, control/text/section-header helpers, populated Results column target checks and existing physical-target journey helper/call. Its approved pointer contribution seeds completed chrome in the existing divider boundary journey and uses strict editor admission there.
- `viewport.rs` preserves all physical sizing of Back, Join, breadcrumb and browser Open/Options controls, including options width budgeting. Its approved pointer contribution records actual clipped header/body/hint geometry and checks the existing secondary-click route. No previous target sizing or header/card behavior was reverted.

All 14 per-file stable patch IDs match the approved source contributions. This is supplementary to the full-file and complete-overlap comparisons, not the sole preservation proof. The staged patch's different total hash/length reflects those preserved main contexts; no semantic merge correction was authored.

## Prior safety and main contributions

The exact approved source preserves PTR-1's copy-outside-Context-lock secondary predicate; PTR-2's actual AccessKit label/ID-to-observed-geometry join; and PTR-3's geometry-only standalone API plus explicit strict editor point/window helpers and EditorScenePointerRequired camera policy. Router/previews use strict editor helpers, and opted-in camera rejects missing Context/region/window. The integration introduces no editor geometry-only fallback.

Terrain grant/API/runtime/DB and native A2/host lifecycle source lie outside this contribution. Additionally compared 13 whole index blobs directly with main HEAD: runtime terrain_grant/app, API terrain_credential/terrain, DB api_auth/petal_terrain, UI native accessibility mod/surface, host witness/native accessibility mod, and physical-target Inspector/widgets/sidebar. All are byte-identical. Other physical-target paths remain unchanged by staged scope. Touched plugin/panels/header/divider code either matches approved complete source or is covered by the exact overlap proof above.

Unstaged findings, `.gitattributes`, QA38/QA39 packets, Conductor checkpoint/metadata/plan and pre-existing session log are excluded. No app or build state was inspected; parent reports none active. This lane provides no fresh cleanup or runtime receipt.

Root retains commit/build and integrated existing-journey execution ownership. Fresh armed Circle Back/ancestor no-authoring checks, unchanged terrain/history, positive uncovered placement, secondary menus/previews, standalone/editor missing-witness behavior, camera/capture/held gestures, physical native input and broader campaign/CI/release acceptance remain pending. Historical QA-0024 is not explained or closed by this integration.

Only this review artifact was authored. No production/index/history mutation, compiler/build/formatter/test, app, OS/process/input/capture, API/DB/store or network operation occurred.
