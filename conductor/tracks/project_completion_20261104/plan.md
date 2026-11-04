---
type: Plan
date: 2026-11-04
status: in_progress
title: Project completion execution and acceptance queue
---

# Execution queue

Checkboxes represent accepted outcomes. A partial marker records implementation
or focused evidence with remaining acceptance; it does not waive the unchecked
items. Update [checkpoint.md](./checkpoint.md) and metadata after each coherent
reviewed slice. Historical evidence stays linked rather than rewritten.

## P0: preserve and reconcile

- [x] Persist the user-adopted completion scope and release gates in this track.
- [x] Record current source/build, finding counts and unfinished isolated work.
- [x] Independently review this Conductor reconciliation; approval in reviews/conductor-scope-review.md.
- [x] Commit the reviewed Conductor reconciliation: 55cead54.
- [ ] Reconcile the final feature catalog and changed production routes, preserving stable IDs and explicit exclusions; baseline is 145 families / 1,529 checkpoints / 1,515 mandatory cases, not a final denominator.

## P1: correctness and execution blockers

- [ ] QA-0072: preserve guarded Owner QA observation before actual native provider handoff. QA40 three no-input UI reads failed despite state and actual screenshot succeeding; explicit ordering correction d8da3a46 is source-reviewed, compiled d8da3a46; QA41 matching tree/state/screenshot and102physicalID joins recovered, independent focused review approved. Native output suppression and Owner/scope/privacy gates remain required.

- [ ] QA-0071: repair shared scene/UI pointer arbitration so armed Circle/Rectangle/other world tools cannot write through actual viewport Back/breadcrumb/hint UI. QA38 reproduced one Back click causing both navigation and a new terrain region; preserve evidence and existing accepted data. Do not equate this with the unknown historical QA0024 trigger.

- [ ] QA-0070: preserve verified terrain API scope/credential grant and exact admitted ancestry through REST/MCP/runtime to transactional use-time validation; retain local publisher authority and test queued reparent/expiry/revoke/reset refusals.

- [x] QA-0069 source correction: complete finite source-bound measurement/atomic pair stack and all three ownership/projection blockers plus source-hash cache independently approved and integrated0e6668bd. Live acceptance remains below.
- [ ] Verify no new cut/fill history on navigation/reload, true zero versus unavailable, late direct and resolved receipts, current access/source changes, newer gestures and terminal outcome retention.
- [ ] Reproduce and resolve QA-0024 unintended brush edit with independent expected geometry and zero unintended mutations.
- [x] Correct/review the legacy helper stdin ownership (98572fdb); fresh fixture preparation and authenticated app startup passed, then exact owned stop exited0. Offline inspection/adoption/restart acceptance remains unchecked.
- [ ] Review/implement require-existing live restart and fixture checkpoints without reseeding, offline store bypass or invented identity/authority.
- [x] Native witness four source corrections independently approved and integrated2bce88af; pinned SDK annotation setter correction independently reviewed and committed5322a889. Combined compile/native acceptance remains open.
- [ ] Verify normal accessibility delivery; implement/review retained-element operator helper, bounded invocation ownership and any separate navigation profile. Establish capture freshness or retain explicit refusal.
- [~] QA-0063 peer node delivery: source repair integrated; prove fresh paired ordinary-node and real byte/render delivery, then restart/recovery.

## P2: complete interaction and data acceptance

- [ ] Finish selected-set/group edge cases: overlapping, hidden, deleted, empty, ambiguous, 256/257 bounds, mixed values, primary changes and navigation.
- [ ] Exercise the same targets across tree, Inspector/Options, Move/Rotate/Scale, numeric forms, group membership and Undo/Redo; include supported paths/stamps/derived adapters and whole-edit refusals.
- [ ] Execute same-object conflict versus unrelated-object streaming, exact inverse links, rollback, lost receipts and no automatic replay/rebase.
- [~] Final rich-data Both scenario: prior 38 functional checks and local persistence passed; repeat final-build schema/edit/append/transform/replay/chart and distributed restart acceptance.
- [ ] Verify history pagination, completed-save boundaries, malformed journal/baseline refusal, exact integer limits, schema revisions, authority changes and transactional journal failure.
- [~] QA37 independently focused verifies area100 Raise2fill200/Lower1cut100, atomic pair facts, actual departures/returns with byte-stable complete histories, material and4m3 sampled solid-box overlap with loaded outside control. Final-build matrix, exports, scale/other geometry, authority, retained restart and rollback gates remain; conservative bounds never become exact GLB mesh-solid claims.
- [ ] Verify explicit local-copy adoption, before/after native metadata audit, actual adopter operation, peer bytes, loader failure recovery and rendered/pickable GLB.
- [ ] Exercise actual Manager-issued delegation, Viewer/Editor recipients, v1/v2 invite behavior, expiry/revoke, scoped access and relay-only connectivity under the recorded routing contract.
- [ ] Verify the six source-fixed/live-pending findings in the checkpoint and close the cleanup review item using actual evidence.

## P3: full UX and feature coverage

Use [the screenshot inspection checklist](./visual-checklist.md) for the changed hierarchy workflow and subsequent UI fixes.

- [~] VIS-001: card count correction9cdd3132 focused-verified in reviewed QA34/35 at actual1024/1280/1600 widths and1.25/2 scales, with Open/Options/Create-Cancel. All-Verses cards, long names, large counts and extreme widths remain catalog gates.
- [~] VIS-002: header contrast source515b88fc compiled; QA36 browse-state screenshots independently focused reviewed. Active-Petal backing correction7f0de44a compiled and readable/wrapped actual screenshots independently reviewed in QA38; broader catalog remains pending.
- [~] VIS-003: preferred/effective width and owned-divider v2 source independently approved, including first-frame pending-close cancellation; exact integration reviewed. Combined compilation and QA36 actual drag/scale/selection/camera focused proof independently reviewed. Two10px divider targets fail layout checks; v4 Results side-gutter/clip correction75e5d7fb is independently source/integration approved; QA38 independently reviewed actual20pxsidehandles/restoration/stablecamera, with .75rounding/2xInspector/6pxResults failures preserved; broader correction6f473264 compiled5a9eeb93, QA41 focused evidence reviewed with 13 residual targets and cramped narrow Inspector; residual implementation and remaining matrix pending. Modal/cancellation and broader scale matrix remain pending.
- [ ] Inspect actual screenshots during every changed workflow; record and fix visible clipping, overlap, wrapping, spacing, contrast, empty/black output and selection glitches before acceptance. Preserve before/after images and state/size/scale; geometry checks alone do not pass visual review.

- [ ] Close/reproduce remaining status, viewport selection, Enter/Escape, empty Inspector, small targets, black capture, restore and input/capture timeout findings.
- [ ] Audit every reachable panel/dialog at three sizes and relevant scales, keyboard/focus/scrolling, normal and destructive actions, and native OS/accessibility behavior.
- [ ] Complete remaining catalog flows for import/export, paths and closed-path transforms/picking, stamps, terrain/Hexon downloads/cancel/residency, webviews, plugins and device delivery.
- [ ] Exercise owned file leases, expiry, geometry, junction/negative paths, writer draining, queue bounds, shutdown and reset on disposable owned data.
- [ ] Complete queued polish waves 2-6; reconcile implemented work rather than redoing it.
- [ ] Perform five actual live UX audits against the planned comparables; synthesize findings and complete accepted wave 7+ work.
- [ ] Run independent advocate/critic acceptance reviews and resolve their findings.
- [ ] Run up to six full QA passes as needed: 100% required gold coverage and two consecutive full passes with zero new S1/S2; focused passes supply no full-pass credit.

## P4: final integrated verification, once implementation is complete

Use Rust 1.99.0, jobs=2, CI=true, CARGO_PROFILE_DEV_DEBUG=0,
RUST_MIN_STACK=134217728, FE_KEYSTORE_BACKEND=env,
FE_REQUIRE_STORE_FIXTURE=1, FE_REQUIRE_RELAY_CONTAINER=1 and
FE_CONTAINER_RUNTIME=podman. Clear FE_API_INPUT_ALLOW, FE_API_DEV_TOKEN_OUT,
FE_QA_STORAGE_ROOT and FE_HARNESS_SCENARIO for the sweep. Do not globally disable
relays or replace required real acceptance with skips/fakes. Preserve complete
logs, exact SHA/toolchain/configuration and every command exit code; never dump
the full environment or credentials.

- [ ] `cargo +1.99.0 fmt --all -- --check`
- [ ] `cargo +1.99.0 clippy --workspace --all-targets --locked --features fe-terrain/render -j2 --keep-going -- -D warnings`
- [ ] `cargo +1.99.0 test --workspace --locked --features fe-terrain/render -j2 --no-fail-fast -- --test-threads=1 --show-output`
- [ ] `cargo +1.99.0 run --locked -p fractalengine-test-harness -j2` reports **18 passed / 0 failed**, never 0/0.
- [ ] Separate `extensions/iot-bridge/Cargo.toml` workspace: fmt check, all-targets locked Clippy with warnings denied, locked tests with one test thread and show-output.
- [ ] Review skip/ignored results and real Podman/store fixture execution; retain explicit external GIS archive and user-check limitations.
- [ ] Reconcile final source fingerprints, immutable evidence, findings, decisions, metadata, manual checklist and retrospective before publication.

## P5: exact-commit CI and downloadable tagged release

- [ ] Push the finished verified commit; wait for green exact-SHA Build Artifacts CI including required checks/fuzz evidence before tagging.
- [ ] Publish `v0.1.0-qa.YYYYMMDD` using the actual completion date and verify the immutable tag resolves to that exact commit.
- [ ] Wait for the whole Release workflow, including Docker, not only GitHub Release publication.
- [ ] Download and hash-check SHA256SUMS plus each nonzero archive below; inspect Windows GUI extraction with adjacent assets, QUICKSTART and licenses.
- [ ] Verify `ghcr.io/escherbridge/fractalengine-relay:<tag>` and print permanent release/download links and a checksum table.
- [ ] Write final retrospective, reconcile/archive completed tracks and mark the goal complete only after all required work is accepted.

Required archives:

- `fractalengine-linux-x86_64.tar.gz`
- `fe-relay-linux-x86_64.tar.gz`
- `fractalengine-windows-x86_64.zip`
- `fe-relay-windows-x86_64.zip`
- `fractalengine-windows-aarch64.zip`
- `fe-relay-linux-musl-x86_64.tar.gz`
- `fe-relay-linux-musl-aarch64.tar.gz`
