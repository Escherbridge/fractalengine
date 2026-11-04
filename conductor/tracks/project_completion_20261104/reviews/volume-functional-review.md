---
type: Review
date: 2026-11-04
title: Earthwork volume correction functional source review
source_artifact_sha256: 2eea77cdcaf2d2395de07328bc160d6049a43bfdd7ac2add70d004140b7d970a
---

# Earthwork volume corrections: independent functional/design source review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for the frozen 19-file correction atop `bef56e790fdd80ce5688055c820ee11709e0a3f9`.** No blocking functional/design defect was found in the reviewed correction. This does not close QA-0069 or establish executed terrain, database, dispatcher, shutdown or restart acceptance.

## Exact reviewed state

Reviewed `.omc/worktrees/qa-volume-measurement-ownership` at the stated HEAD and the exact dirty tracked files in its `.omc/research/qa-volume-correction-frozen-manifest.json`. All 19 independently recalculated raw working-file SHA-256 values match. The captured full diff SHA-256 also matches the manifest: `8df02dd60399e4a53e241b0fc2f31c7f5e23e4c5c856c84a69bb1841686255fb`. Independent declaration inspection confirms 152 existing `#[test]` declarations before and after across the correction-touched Rust files. `git diff --check` returns 0; only ordinary LF/CRLF warnings were emitted.

Read the original functional review and QA33 volume-history investigation, the correction report, the complete background owner, actual Update dispatcher and property/committed helpers, maintenance reset, Supervisor freeze/join/drain paths, confirmed-document mutation seams, authored journey additions and directory contracts. The previously reviewed finite-coverage/source producer and existing atomic checked-batch backend remain the foundation; the correction changes no database transaction or generic replica-projection implementation.

## MED-1: original-demand gate precedes all receipt projection

The actual Update dispatcher now identifies matching owned read/direct commit/Resolve/failure results before ordinary batch handling and consumes them with an unconditional `continue`. Its only receipt projection path first calls `VolumePersistence::projection_receipt` with current navigation, alias map, confirmed raw terrain, latest terrain-owned measurements and assignment, height generation, manager/access and role state.

That gate requires an accepted exact request/family, a nonretired original demand, current complete finite source-bound measurement and actual editable/unblocked scene access, unchanged original navigation/Petal/incarnation/access identity, original node admission and node cache epoch, and a nonregressing positive saved version. Resolve additionally requires an eligible exact current capture and members equal to the actual receipt. Full receipt matching requires only the expected cut/fill property changes while retaining the captured row and group state.

Only after that gate do the unchanged whole committed preflight functions run. Their complete converted results are available before committed version, local admission or property/cache mutations begin. Invalid or stale owned packets are withheld from projection and cannot fall into generic external batch materialization. Recognizing/draining an exact actual save outcome is separately allowed when its current projection is withheld. This repairs the earlier ordering defect where a later owner check could not undo earlier Update mutations.

## MED-3: direct and resolved outcomes share a property-only path

Direct commit and committed Resolve receipts use the same dispatcher branch. It updates genuine committed versions, local property admission, descriptor/property hydration and the selected object's property display. It does not call `committed::apply`, write `Transform`/`DbConfirmedTransform`, invoke NodeManager transform acceptance, invalidate user Redo, or append a user Undo entry.

The property handler performs ordinary property/descriptor/alias hydration; its cache ingestion does not itself queue a generic current read. Earthwork capture excludes path/stamp source representations, and unchanged remaining properties are checked by exact receipt matching. The owner's former `GetNodeProperties` refresh state and sends have been removed entirely, including after an unchanged pair or uncommitted Resolve. No owner-generated properties-to-replica-to-full-TRS escape remains. Generic replica and unrelated receipt behavior is unchanged.

After a successful preflight/project, `finish` requires the actual projected result and still-current original context to confirm the pair. The new local admission/epoch is retained as confirmation state. A pure equal-pair read skips a save without inventing a positive journal version. A completed stale save drains honestly while leaving current volume unavailable; it is not rolled back, replayed or silently rebased.

## MED-2: accepted unknown ownership survives real UI reset

`VolumePersistence` is now an independent initialized Bevy Resource. UiManager holds only the derived `VolumeView`; its actual `clear_for_terminal_reset` replacement cannot drop the capture/request/Resolve owner. `clear_read_models` first retires unsent/display/measurement state and marks any accepted write retired while retaining its exact identity and capture. Terminal PostUpdate returns before pumping commands or repopulating the display.

The production Supervisor's freeze replaces ordinary Update and primary UI schedules before clearing read models. Its bounded terminal DB drain now releases the receiver lock before visiting results and gives them only to the outcome observer; that observer does no current UI projection or command emission. An exact valid committed outcome can settle ownership; failure retains the unknown owner.

Remaining unknown state clears through the existing trusted host cleanup seam, now named `clear_retained_outcomes_after_database_join`. The actual production caller is after Supervisor takes and successfully joins its owned DB worker. A panicking/abnormal join marks the outcome unconfirmed and returns before the hook. No arbitrary-thread join proof or replacement authority capability was introduced. The renamed scene-asset callback use preserves its existing lifecycle behavior.

## LOW: confirmed raw-document cache replaces frame-change hashing

The fingerprint now belongs to `PetalMapState`'s confirmed-document state. Only the actual active-Petal terrain DB echo calls `confirm_terrain_document`, which hashes the replacement document. Navigation clears and optimistic GIS/proposal/layer edits call `replace_terrain_document`, which invalidates that identity, including the opacity slider's document edit. Volume admission reads the cached value rather than Bevy `is_changed()` on an ordinarily mutably borrowed resource. Source inspection found no remaining production direct assignment bypass in the inspected UI mutation sites; direct assignments in unrelated test fixtures do not grant confirmed identity.

This avoids the prior recursive per-frame hashing cost without substituting a rounded parsed config or a newer binding hash for the original raw source. Runtime assignment, region, coverage and pristine height witnesses remain separate checks.

## Authored coverage and limits

The extended existing production dispatcher journey is wired into the actual `apply_db_results` test App. It exercises direct and resolved receipts, changed height/raw source/node epoch/access and an already retired request. Assertions distinguish actual version/property/admission confirmation from stale nonprojection, preserve newer live Transform and DbConfirmedTransform baseline, selection and gesture/commit pulse, and user Undo counts/generation, and reject generic property/replica refresh commands.

The same helper invokes actual `clear_read_models` and UiManager reset, checks the accepted capture/Resolve identity survives, runs the terminal owner system to assert zero commands, checks failure retains ownership and invokes the trusted post-join cleanup callback. These authored checks do not execute Supervisor's actual worker join/reset; that production relationship was source-inspected. No test or SQL assertion was run in this lane.

The separation of pure source/coverage admission, accepted request ownership, immutable outcome matching, current projection and derived display is readable and fits the existing manager/checked-batch architecture. It repairs the concrete ownership seams without a new generic mutation or lifecycle-proof framework. Directory contracts now describe the inspected implementation rather than the previous stronger claims.

Parent integration must preserve newer main source, then compile and execute no-new-history-on-navigation/reload, genuine zero versus unavailable coverage, direct/lost-receipt recovery, source/access changes, newer gestures and actual terminal lifetime acceptance. Existing false-zero facts 103-106 remain preserved and auditable. Complete-state conflict rules, independent backend authority/schema/terrain/history fences, generic replica behavior and the final campaign sweep remain mandatory. Source approval supplies no peer convergence, field-level automatic rebase or QA-0069 closure.

No Cargo/compiler/rustfmt/Clippy/tests, apps, OS/process inspection, input, API/DB/store access, network requests, Git mutations or production edits were performed by this reviewer. Work consisted of read-only source/diff inspection, hash/declaration checks and this review artifact.
