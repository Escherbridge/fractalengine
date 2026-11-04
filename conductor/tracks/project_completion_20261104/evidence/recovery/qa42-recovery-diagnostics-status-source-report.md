---
type: SourceReport
title: Recovery diagnostics and status source author report
date: 2026-11-04
---

# QA42 recovery diagnostics and checked-read status: source report

Author: `/root/native_nav23_functional_review`, 2026-11-04.

**Source authored and frozen for independent review; no implementation or runtime approval is claimed.** This narrow slice exposes the primary outbound transaction failure, makes terminal checked-read errors/retry visible in single-selection Inspector/Options, and distinguishes an empty scene snapshot from a real foreign-Petal snapshot. It does not change the unknown failing database guard or claim that publication, transforms or reports now pass.

## Exact source freeze

Main baseline at freeze: `a77cabbee465a690749eba684d8299e51c716b82`. Root advanced documentation/evidence commits while this lane worked; no Git mutation was performed here. Packet: `.omc/research/qa42-recovery-diagnostics-status-v1/`.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `manifest.json` | 4190 | `176a275203080ce3c8638f099d8e5495ec11da4f507609b30556d7bb641f5ecb` |
| `source.patch` | 28391 | `4018db2744fa7aeae23bbcd4ef3476b48936face43ec141f796d7d881377c78b` |

`raw/` preserves all **15** complete exact working files; the manifest records each raw byte count/SHA and baseline blob SHA. The review patch normalizes only CRLF to LF for readable comparison; raw hashes govern source identity. Eight production Rust files, two test-only Rust extensions and five directory guides comprise the slice:

- `fe-database/src/handlers/scene_publication/{current.rs,diagnostics.rs,mod.rs,publisher.rs,AGENTS.md}`.
- `fe-database/src/op_log.rs` — existing operation journey only.
- `fe-ui/src/editor_batch/{ui.rs,AGENTS.md}`.
- `fe-ui/src/panels/{inspector.rs,AGENTS.md}`.
- `fe-ui/src/ui_shell/{right_sidebar.rs,AGENTS.md}`.
- `fractalengine/src/{qa_debug_bridge.rs,vision_bridge.rs,AGENTS.md}` — vision bridge change is test-only.

Root explicitly extended ownership to those two existing journey files. No native observer source, application/window behavior, transport/API request shape, Conductor state, evidence packet, index or commit was edited. Concurrent root/native-author changes and the preexisting session log remain outside this freeze.

## Outbound indexed error preservation

The existing private-current transaction classifier moves to the small shared `diagnostics.rs` module. Its fixed `Transaction::{CurrentRead,Publication}` operation labels keep current-read diagnostic format and add a distinct publication prefix. Selection remains deterministic: earliest non-NotExecuted indexed error, otherwise earliest rollback echo, otherwise fixed error-unavailable. Output contains only operation, statement index, fixed stage, static SDK kind, whitelisted reason and count. Unknown error messages, SQL, values, bindings and cause chains are not interpolated. Request failures similarly retain only the operation and SDK kind.

Publication now carries parallel fixed labels for its existing authority, node projection, point/ancestry, local journal/tombstone proof, clock fence/write and asset guards. After executing the unchanged final transaction, `emit` inspects `take_errors()` before any enqueue. The actual SQL strings, binding values, guards, clocks, local history provenance and successful dependency/node enqueue order remain unchanged. A refused transaction still emits nothing from that row. A post-commit publication refusal still does not reverse a successful local edit.

No speculative typed-value fix is included. QA42's actual primary failure and its relationship to the Waiting state remain unknown until a controlled fresh build/run produces the fixed diagnostic. Existing preparation-time queries and batch verification retain their own behavior; this slice adds no broad raw database logger or new inspection endpoint.

## One checked-read status/refresh UI

`editor_batch::ui::read_status_controls` is shared by the complete-selection form and the public crate-local `render_read_status` wrapper. Inspector and Options single-selection Waiting branches call that wrapper. Terminal errors render the controller's existing retained error; active reads show read status; a missing capture with no active read asks for refresh. The existing accepted-write status and Check saved selection action remain available when that owner is present.

Refresh retains its original controller implementation and pending gate: it clears only refreshable read/capture/error state, retains drafts and cannot clear an accepted pending write. Its button wraps within narrow content and retains the shared target minimum. Rendering alone submits no command. A subsequent ordinary maintenance pass owns the pure read; no write is replayed and no visible ECS state becomes an authoritative capture. Existing transform routes and capability/permission gates are untouched.

## Empty diagnostic snapshot

The only production change in `qa_debug_bridge.rs` is an early `scene.petal_id.is_none()` branch returning the existing `unavailable / scene_snapshot_unavailable` shape. A nonempty snapshot for another Petal still returns `stale / scene_petal_mismatch`. ViewScope, selected entity/marker/tree membership, geometry bounds, terrain assignment/source and epoch checks remain intact.

This preserves the producer's footprint-demand contract. It does not force collection, infer terrain correctness, reveal another object's geometry or turn ordinary object selection/report-panel visibility into report demand. A matching actual report footprint and runtime input epochs remain a separate live verification requirement.

## Meaningful existing journeys extended, unexecuted

No new registered test declaration is introduced:

- The existing database operation journey adds an actual rolled-back transaction whose primary publication failure follows an earlier echo, then checks the production shared classifier. Additional cases cover publication journal/tombstone/asset reasons, unknown-secret redaction, unexpected statement index, all-echo fallback and empty errors. Existing private-current rollback/redaction cases remain.
- The existing Inspector journey creates an owned single-selection read, supplies a terminal refusal, verifies maintenance does not retry automatically, renders the shared status and locates the real accessible Refresh control, authors its pointer click, then requires exactly one pure read with no queued edit or invented capture. Existing complete-selection/ownership journeys remain.
- The existing grounded-camera diagnostic journey adds absent and default/no-Petal snapshot refusals with no AABB/probes; the adjacent genuine foreign-Petal stale case remains intact.

These are authored source checks, not parsed, compiled or executed assertions. They do not grant actual UI, transaction, native, report or durability acceptance.

## Inspection and remaining gates

Read applicable repository/directory guidance, shared UX guidance and the Conductor completion context. Per the parent's source-only instruction, no compiler/parser/formatter, tests/journeys, implementation execution, apps/helper/process/OS/native/API/DB/network action or Git mutation occurred. Scoped read-only `git diff --check` returned zero. Review of the scoped production diff shows no SQL/binding/fence or enqueue change. The unknown transaction cause was not guessed or repaired.

An independent reviewer must inspect the complete 15-file raw freeze, shared classifier privacy/statement labeling, UI action/refresh/unknown-write ownership, test-only boundaries and empty-versus-foreign diagnostic behavior. Root alone owns staging/commit/build and subsequent evidence. This author context does not self-approve.

After source review and build, the smallest useful fresh fixture should capture the new primary publication statement/stage/reason during normal fixture setup/Verse recovery and the actual owned selection-read outcome. Verify terminal failure is visible in both Inspector and Options, one explicit refresh issues only a read, and transforms stay unavailable until validated current capture. Then diagnose the actual guard with that evidence; preserve authority and complete history/row/clock/asset fences. Separately create legitimate scoped report demand and verify the matching diagnostic. Existing FULL campaign, final sweep, exact-SHA CI and tagged-release gates remain untouched.
