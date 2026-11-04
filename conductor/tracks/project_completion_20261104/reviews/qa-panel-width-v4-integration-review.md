---
type: Review
date: 2026-11-04
title: Divider v4 exact integration review
---

# Divider v4 integration preservation: independent review

Reviewer `/root/legacy_helper_io_review`, 2026-11-04. **APPROVED for the exact staged source integration.** No lost main contribution, unintended source addition, unresolved conflict marker, or source/receipt mismatch was found. This is an integration-preservation review, not compilation, journey execution, accessibility compliance, native capture or live acceptance.

## Exact anchors and scope

Main HEAD is `9df6a0484300e610100907d952fa915bbb6b57da`. Independently computed staged `git diff --cached --binary` SHA-256 is **`9746837a10f05cdd49f104cdb1d6b98fe7653b2f1bc43307350940377c628566`**, matching `.omc/research/qa-panel-width-v4-integration-receipt.json`. The index changes exactly the ten paths in that receipt; no additional staged path is present.

Read the independent v4 functional and design source approvals in main, the isolated v4 report/receipt/source, preserved v2 snapshot, and actual staged diff. All ten isolated raw source hashes match the approved receipt. Independently re-hashed the three source patch artifacts:

| Approved source artifact | SHA-256 |
|---|---|
| v2-to-v4 delta | `50980329980c0b66e1e48884aaeb97215b3ca7065249f871a9e9ec27462dfba6` |
| v3-to-v4 correction | `37f5689dc3c5754694810bf7121dcfba75308f0e2495ba66aac8747a272f1f88` |
| Full v4 from `2bce88aff512a9998eb120ddbcde8b78b4f7aa84` | `6b726d6ed18c998ffad5dcccbf87e01e8e5bbf39c09bac330f4244ddf9dc3a7f` |

All ten current working-tree files equal their staged blobs after CRLF-to-LF normalization. Every staged blob was scanned for conflict markers; none found. `git diff --cached --check` passes. No operation modified the index or source.

## Full-file equality and the single overlap

Nine staged files equal the complete approved isolated v4 source after only CRLF-to-LF normalization. This verifies all approved source and directory documentation contributions, rather than merely selected lines. The ninth includes `topbar.rs`, whose two existing test-call vector conversions are part of the approved v4 source; no new production topbar geometry is introduced.

For the preserved v2 snapshots present in the source checkout, all eight non-plugin main parent files also equal those snapshots after newline normalization. Thus replacing them with exact approved v4 files drops no intervening main content. The remaining topbar file is full-equal to approved v4, whose delta was independently source-reviewed.

`plugin.rs` is the only overlapping source. Its entire **HEAD-to-index** diff is the approved replacement of `topbar::viewport_interaction_rect(rect, viewport, maximized, SDK radius, Results-open flag)` with `layout_policy::scene_interaction_rect(ectx, rect, maximized)`. The visual `viewport_rect.0 = rect`, pixels-per-point assignment, and subsequent window-edge handling remain unchanged.

Independently compared all content differences between isolated approved v4 `plugin.rs` and its staged blob. They equal, operation-for-operation and byte-content-for-byte-content after newline normalization, the complete differences between source base `2bce88af` and main parent `9df6a048` for this file:

1. Preserve initialization of `earthwork_volumes::VolumePersistence` after EarthworkNodeMap.
2. Preserve initialization of renderer `EarthworkVolumeState` and `EarthworkTerrainBinding` after EarthworkVolumeReport registration.
3. Preserve the newer `PostUpdate, earthwork_volumes::persist` registration in place of the original `Update, terrain_proposal::persist_earthwork_volumes`.

There are no other source-to-index differences. These are the already integrated volume ownership contributions, not new authored merge semantics. The approved scene-interaction hunk has the same stable per-file patch ID in source and index. Stable patch IDs were supplementary only; full-file and complete-parent-delta comparisons supply the preservation proof.

## Existing interaction and security contributions

The staged scope does not touch native Guard/lifecycle/annotation/host witness modules, volume ownership dispatcher/backend/property projection modules, runtime/database types, or their constructors. All such index paths remain main HEAD by scope. Within touched files, the complete approved source retains the native begin-pass/finish-pass, product Guard and eight producer coverage hooks in `panels/mod.rs`; `plugin.rs` retains Inspector-tab `surface::transition`; Results keeps its actual popup surface marker. No native eligibility/profile/capture authority or original API/DB role contract is changed by this integration.

The two additive directory documentation sections explain shared finite DividerMetrics, reserved content gutters and actual inner clipping, bounded Results-only native style scope, unchanged outer widths, visual viewport versus interactive routing, current-pass Results edge ownership, and authored-but-unexecuted journeys. They explicitly preserve popup ownership and pending actual target/neighbor/high-scale/live evidence. No documentation promotes the code integration to live acceptance.

Affected Rust `#[test]` declaration counts remain equal between main HEAD and index: panels/mod 1, sidebar 1, plugin 19, layout_policy 0, left_sidebar 3, results_dock 0, right_sidebar 13, topbar 10. Existing shared journeys were extended; no declaration was added by this staged integration.

## Exact staged blob SHA-256

| Path | SHA-256 |
|---|---|
| `fe-ui/src/panels/AGENTS.md` | `89771af56c8af590e17adbdd5f0ef8080ca319e030a3220cfe9d2215d56f9c88` |
| `fe-ui/src/panels/mod.rs` | `1efae0d5a269af4bdb62b89af9d0e22d7a80d351ec77b9c5da3056a06d188286` |
| `fe-ui/src/panels/sidebar.rs` | `e82172fdb7307abc0bfc2bd05426ee8232f343075ded27c3c2ac9cc7a321cf99` |
| `fe-ui/src/plugin.rs` | `aa96127ffc59e0bbed389db3ec850d7a3a807c748dcec58216065d309fb7e307` |
| `fe-ui/src/ui_shell/AGENTS.md` | `d657d997aa64041db03b5a11b213092a9de40be9dae671d22be0ca7d7d33f593` |
| `fe-ui/src/ui_shell/layout_policy.rs` | `e5f23f18f8bd007d3f49fa30afd50be741e9a7bdf59c477715658c61f352cda4` |
| `fe-ui/src/ui_shell/left_sidebar.rs` | `4a8cf213cfabece1bc9f7eb4fbe9e1271c53be6918566845848bc87ffc68310b` |
| `fe-ui/src/ui_shell/results_dock.rs` | `8c0f87e911c206668b9745a938de991d0235f3a111e62badc6edecaf2a109e41` |
| `fe-ui/src/ui_shell/right_sidebar.rs` | `63e999dc685927d00c6ee7a1e1705b5659187219d3582326b2fb488d960dc748` |
| `fe-ui/src/ui_shell/topbar.rs` | `e5b509b26fd515308ad578a4f1adb027c642794851838950a6324315ed691775` |

QA37 untracked payloads and the unrelated unstaged Conductor session log are excluded. Parent reports both focused apps stopped and eight recorded identities absent for each; no new process or cleanup verification was performed here. Root still owns commit, compilation, authored journey execution, actual divider target/adjacent-control/scale/modal/cancellation acceptance and all broader project gates. No build, test, app, OS/process, API, DB, network, production edit or Git mutation occurred; only this review was authored.
