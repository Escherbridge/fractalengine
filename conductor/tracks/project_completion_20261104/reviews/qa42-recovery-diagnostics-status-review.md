---
type: Review
title: Recovery diagnostics and status source review
date: 2026-11-04
---

# Recovery diagnostics/status v1: independent source review

**APPROVED AT SOURCE** for the complete frozen 15-path slice `.omc/research/qa42-recovery-diagnostics-status-v1/`, baseline `a77cabbee465a690749eba684d8299e51c716b82`. No concrete security/correctness/design blocker was found. Approval permits root's exact staging/commit/build and meaningful execution; it does not establish the unknown QA42 failed guard, successful recovery, transforms/reports, native behavior or campaign acceptance.

Manifest **4190 bytes**, SHA-256 **`176a275203080ce3c8638f099d8e5495ec11da4f507609b30556d7bb641f5ecb`**. Complete review patch **28391 bytes**, SHA-256 **`4018db2744fa7aeae23bbcd4ef3476b48936face43ec141f796d7d881377c78b`**. Independently checked all 15 raw lengths/hashes against the manifest and actual working files. All 14 existing baseline blob hashes match a77, with current HEAD equivalent for those source paths; the new diagnostics module is absent at baseline. The patch uses newline normalization for readability; raw manifest identities govern approval. Author report SHA-256 is `f3b173f304a4c4040a3999eba41c1ee3092458fb0bdacb6a2fc06cac4234f100`.

Only this review was authored. Read-only frozen source, repository/Git, installed primary SDK and hash inspection. No implementation/parser/compiler/formatter/tests/build, apps/helpers, OS/process/native/input/capture, API/DB/network or Git mutation. Other authors' native and root publication work was preserved. Applied shared `C:/Users/atooz/.gemini/UI-UX-guidance.md`.

## Transaction privacy and unchanged persistence

The new `diagnostics.rs` extracts the existing current-read policy into one small operation enum and shared classifier. Sorting error indices makes selection deterministic: earliest non-`NotExecuted` error, otherwise earliest rollback echo, otherwise fixed unavailable. Operation/stage/reason output is fixed; SDK `kind_str()` is a static string in installed `surrealdb-types-3.0.5/src/error.rs`. Neither unknown error text, SQL, bindings, row data nor cause chains are interpolated. Recognized thrown reasons are exact matches, not substring extraction. Numeric statement/count fields are bounded-size representations. Current-read labels/formats and prior recognized reasons are preserved; publication adds only fixed reason alternatives.

Pinned `surrealdb-3.0.5/src/method/query.rs:423-436,454-469` confirms that `take_errors()` retains all indexed statement errors whereas `check()` returns one and discards the rest with the response. Publication now checks that entire set before dependency/node enqueue. A query request failure is reduced to fixed operation and static SDK kind. A statement error still exits the row without enqueue; successful dependency-before-node order remains unchanged. This scope does not claim that every earlier preparation query or all application error paths are redacted.

Full publisher/current diffs preserve actual SQL strings, captured bindings, Editor authority/full ancestry, row projection/point identity, local journal/tombstone provenance, HLC/clock fence/write and asset metadata/proof guards. Publication's parallel labels align with authority statements, five existing row/proof/clock statements and appended asset guards. `BEGIN`/`COMMIT`/unexpected indices have explicit fixed fallbacks. Preparation scope, serialization/replica bytes, transaction construction and successful enqueue/dependency handling are unchanged. The classifier identifies an error; it waives no guard and does not reverse an already successful local property edit. Actual primary QA42 failure remains unknown pending fresh evidence.

## One read-status UI and ownership

`editor_batch/ui.rs:132-179` supplies one status/refresh renderer used by the complete-selection form and both single-selection Waiting branches. Existing terminal controller errors become visible; an ordinary active read is described as a read, and an absent idle capture requests refresh. The accessible Refresh button wraps and retains the shared physical target minimum. Rendering submits no command. The only returned action remains the existing `CheckSelectionSave` reconciliation path.

Verified actual unchanged controller methods: `is_pending()` covers pending accepted writes and edit-bearing reads; `refresh()` refuses in that state, otherwise clears only read/capture/interaction/error and retains draft, key and history ownership. A pure selection read may be canceled by explicit Refresh, as before; maintenance subsequently owns one fresh read rather than a write replay. Accepted unknown writes keep their pending owner and Check saved selection. No draft/capture is manufactured from visible ECS state.

Inspector/Options transforms still depend on the same `transform_route`, validated capture/interaction and capability/permission checks. Waiting does not become OrdinarySingle or CheckedWholeObject through this renderer. The complete form's existing action/state restoration is preserved. No new selection owner, hidden collector, mutation channel, automatic terminal-error retry or generic capture escape was introduced.

## Empty diagnostic and test boundaries

The only production `qa_debug_bridge.rs` delta returns existing `unavailable / scene_snapshot_unavailable` for a snapshot with no Petal. A genuinely foreign nonempty Petal still returns `stale / scene_petal_mismatch`. Selected marker/tree, ViewScope, bounds, geometry, terrain/source and epoch guards remain. There is no forced report demand, collection, other-object disclosure, fake current epoch or positive terrain correctness claim.

The complete `op_log.rs` and `vision_bridge.rs` production prefixes before their existing test modules are byte-equivalent after newline normalization. Their hunks extend existing meaningful journeys only. Inspector's new terminal-read/error/actual accessible Refresh interaction lives in its existing test module. Registered test declarations remain unchanged across all affected Rust files. Source inspection supports actual rolled-back transaction/redaction coverage, no automatic retry, one explicit pure-read refresh without edit/capture, and absent/no-Petal versus foreign-Petal diagnostics; none was parsed, compiled or executed here.

## Exact production identities and next gates

| Frozen production path | SHA-256 |
| --- | --- |
| `fe-database/src/handlers/scene_publication/current.rs` | `0a6aad038ebfdc229fe0b3c1852a9bbedce553903e6158518e082a1b04c5bfde` |
| `fe-database/src/handlers/scene_publication/diagnostics.rs` | `59a32dd0e178abad9360d70985ffcf65f34555b0c88ef46de656a47f04e4fef5` |
| `fe-database/src/handlers/scene_publication/mod.rs` | `2654706c2f9399279b4d47e08bb6b790d065a45724f66b994901d39656d26706` |
| `fe-database/src/handlers/scene_publication/publisher.rs` | `3153724566633666ab748aa0550fe5e11a1e06c88b39e429a2592080c31d54d5` |
| `fe-ui/src/editor_batch/ui.rs` | `770dd6e5af27665b1839210a8a143106584b9c112986e1abcc2c718c5c368f66` |
| `fe-ui/src/panels/inspector.rs` | `b06c09f227db45fd2aa248ef846654086d8493db920f2456c944262ae5777c12` |
| `fe-ui/src/ui_shell/right_sidebar.rs` | `582ead889d53c85a5bcb2283a82e7acf32fecb2990f29024217a8de457eb8570` |
| `fractalengine/src/qa_debug_bridge.rs` | `4f12967c472ad0d079a09e05e0eff0012444031824bdf4ab96344416b0b956df` |

Manifest additionally governs exact two test-bearing files and five additive directory guides; approval covers all 15 paths, not only this table. Exclude concurrent native worker/controller/guide changes, Conductor WIP, session log and any later source change.

Root must establish actual compilation and the appropriate integrated journey sweep, then use a fresh disposable normal fixture to capture the new primary publication stage/reason and owned selection-read result. Verify truthful terminal status in Inspector and Options, one explicit refresh issuing only a read, preserved drafts/unknown-write ownership, and transforms gated until validated capture. Diagnose any real authority/row/history/clock/asset failure from that evidence rather than guessing its cause. Separately establish legitimate footprint demand before accepting matching selected-terrain diagnostics. Retained stores remain held under the separate require-existing gate; final sweep, full catalog, CI and release acceptance remain pending.
