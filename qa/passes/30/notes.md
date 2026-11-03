# Focused QA30: consecutive history and Group replay succeed; Cancel layout needs a fresh check

The retained Owner store ran on sealed source `481e942b`, with executable SHA256
`59BE87053BE2273615410B9D1971822999E978D304AE2779A96F83F2AD7909E0`.
The recorded manifest has `source_dirty=true` and an exact source fingerprint;
see `owner-context.json`. This packet does not silently relabel that provenance
as a clean checkout. The production GUI/helper stage completed in **334.5 s**
with one unused `with_group_removed` helper warning. Launch completed in **15.5 s**,
preserving the Owner store and port without reseeding. The recipient and fixture
helper were not launched. This was a production build and focused live pass,
not the final integrated test sweep.

The packet uses only saved artifacts. `comparison-oracle.json` contains the
computed expectations and complete-row/history comparisons;
`verdict.json` inventories 250 source files with their SHA256 hashes and
333 parsed source records. Seven screenshots are copied byte-for-byte into
`screenshots/`. The bounded history/layout review is copied unchanged as
`independent-history-review.md`. It does not review every later Group/property
cut. **The full packet's bounded findings are independently approved; see independent-review.md.**

## Consecutive numeric Undo/Redo and stable tree order

Cube 1 starts at `[-12,0,0]`, Cube 2 at `[0,0,-12]`, giving mean pivot
`[-6,0,-6]`. An independently calculated positive 90-degree Y rotation maps
relative `(x,z)` to `(z,-x)`. Uniform scale multiplies the resulting relative
displacement by 1.5. Expected positions are therefore:

| Cut | Cube 1 | Cube 2 | Uniform scale | Both edit_seq |
| --- | --- | --- | --- | --- |
| Initial | [-12,0,0] | [0,0,-12] | 1 | 8 |
| Apply yaw | [0,0,0] | [-12,0,-12] | 1 | 9 |
| Apply scale | [3,0,3] | [-15,0,-15] | 1.5 | 10 |
| Undo scale | [0,0,0] | [-12,0,-12] | 1 | 11 |
| Undo yaw | [-12,0,0] | [0,0,-12] | 1 | 12 |
| Redo yaw | [0,0,0] | [-12,0,-12] | 1 | 13 |
| Redo scale | [3,0,3] | [-15,0,-15] | 1.5 | 14 |
| Cleanup Undo scale | [0,0,0] | [-12,0,-12] | 1 | 15 |
| Cleanup Undo yaw | [-12,0,0] | [0,0,-12] | 1 | 16 |

Maximum saved-row position error is **0.0000019073486328125 world units**, below
the 0.0001 tolerance. Rotation and scale also match their expectations. Complete
saved rows preserve every field outside transform/elevation/edit_seq, including
Cube 1's nested null-bearing object, measurement array, nested array, scalar
values and exact whitespace/escape-bearing string.

Each object's final numeric history preserves its complete prior prefix and
adds exactly eight `full_transform` entries: Cube 1 versions 25-32 and Cube 2
versions 14-21. Each pair shares explicit atomic-save/request identifiers, and
each operation has a fresh identifier. The cleanup scale-Undo state is present
in history at edit_seq 15; its intermediate standalone row is not invented.
Final numeric history reports `current_state_verified=true` for both objects.
Older explicitly unavailable intervals remain unavailable.

This succeeds through the sequence that failed in QA29. Cleanup uses **Undo**,
not new corrective REST writes. The intentional REST write later in this pass
is a separate conflict probe, described below.

Ten saved UI cuts retain Cube 1, Cube 2 and Cube 3 at y=457,488,519 respectively,
with the same sibling order and bounds. The initial pair has both accessible
rows selected and reports `Primary: Cube 2`. Later Cube 1 native observation
changes the current selection to Cube 1 alone. Both original stored objects
still undergo the inverse/redo sequence. These cuts demonstrate original-target
history and stable replacement order for these three rows; they do not cover
every Shift-range/alias/selection convention.

Actual native primary samples match their corresponding saved transform rows.
They include Cube 2 after yaw/scale, separate Cube 1 yaw/scale samples, Cube 1
through inverse/redo cuts, and Cube 2 after final numeric cleanup.
**Cube 2's native ECS transform is not independently sampled at every
intermediate inverse/redo step.** Debug state is primary-only, not a simultaneous
whole-selection ECS census.

## Complete Group lifecycle and repeated replay

Native UI creation saves `QA30 selected pair`, Group
`01M6GCGBZW14FNX3J1HFA7KCGA`, with both cubes as members. Saved registry and
complete object rows verify this chronology:

1. Create, Delete, UndoDelete, UndoCreate, RedoCreate, RedoDelete.
2. Repeat UndoDelete, UndoCreate, RedoCreate, RedoDelete.

Each present registry vector exactly matches the original ID, name, order and
visibility; each absent vector is empty. Both objects remain present throughout.
Membership restoration is exactly `[original_group_id]`; deletion restores the
original absence of `view.groups`, with every other complete-row field unchanged.
The ten corresponding membership facts are retained for each object. Exposed
saved rows and successful replay substantiate this focused lifecycle; this
packet does not independently inspect the internal immutable proof manifests
or claim failed-transaction rollback, protected tombstone, hidden-member or
empty-Group coverage.

The first confirmation locator expires and refuses before input:
`owner-confirm-delete-group.jsonl` records `locator_not_found` with zero new log
sequence. A fresh arm and locator dispatch one click, after which the saved
registry and object rows show complete deletion. No mutation was automatically
replayed to recover the expired locator.

## Whole-pair gimbal and rich shared property changes

One actual X-axis gimbal drag translates both saved objects by exactly
`[3.0052757263183594,0,0]`. Non-X coordinates and every other field remain unchanged
apart from monotonic edit_seq. One Undo restores both original poses. Separate
primary native samples observe each cube at the moved state and both cubes
across the Undo samples. This checks the X-only shared effect and inverse;
the pointer-to-world distance is not independently calibrated from the camera.
See `screenshots/after-gimbal-move.png` and the oracle's native-sample inventory.

The shared-property form sets `qa30_shared` to the exact JSON parsed from the
saved text-entry command. It contains nested nulls, a measurement object, a
nested array and a string whose surrounding whitespace and literal escape
sequences are preserved. Set, Delete, UndoDelete, UndoSet, RedoSet and RedoDelete
produce the expected value/presence on both complete rows, without modifying
any other field. The final RedoDelete has saved state plus the later complete
pre-conflict rows/history; no nonexistent immediate row capture is implied.
Each object retains the expected six property facts. All 18 owned operations
after numeric cleanup (ten Group, two gimbal and six property) have matching
fresh pair request IDs in the exposed journal.

## Intentional same-value conflict refusal

An intentional independent API write commits Cube 2's same visible transform
under history version 40, advancing edit_seq 18 to 19 while preserving transform
and property values. Cube 1 remains unchanged. A subsequent native whole-pair
Undo refuses with: “One or more selected objects or group memberships changed;
the complete Undo was refused.”

Both post-refusal row files are byte-for-byte identical to their post-external
counterparts, and both history files are byte-for-byte identical to the
pre-refusal histories. There is no partial inverse and no automatic write
replay in this recorded attempt. Same-value visible data is therefore not
treated as proof of owned history ancestry for this sequence.

## Open usability finding and intentional restart fixture

The actual 1280x720 stage has a **medium Group confirmation layout defect**.
Cancel is 13.46875x70 physical pixels at the panel's right edge, wrapping one
letter per line; Confirm is also cramped at 42.0625x30. Cancel is present and
clickable, with no demonstrated data loss or inferred WCAG verdict. The saved
image is `screenshots/group-delete-fresh-confirm.png`.

Source fix `4336f781` is independently reviewed but **was not installed or
executed in this sealed stage**. A fresh-stage screenshot/target measurement and
an actual Cancel that preserves the Group are still required. Source review
does not close the live finding.

The pass deliberately ends with `QA30 restart group`, Group
`01M6GDVPBSY2W51384QZGGBCAS`, confirmed on Cube 1 and Cube 2. Final saved rows
retain baseline poses, prior rich data and the intentional external revision,
adding only this membership; the registry row is saved in
`owner-restart-group-final-registry.jsonl`. See `screenshots/restart-group-final.png`.
**This is a persistence-test baseline, not completed restart persistence proof.**
It has not been removed with corrective API cleanup.

## Driver failures, local logs and remaining scope

Initial token-file sandbox read refusals in `owner-initial-state.jsonl` and
`owner-initial-hierarchy.jsonl` are driver preflight failures, despite the local
wrapper's `not_running` label. They are not failed application inputs or product
startup evidence. An invalid `ui-tree` CLI alias is likewise a driver preflight
error; no saved successful application input is inferred from it. The expired
confirmation locator refusal is preserved separately from the fresh saved action.

Final raw stdout is **280808 bytes**, with **347 structured WARN**, **1438 INFO**,
zero structured ERROR and zero Rust panic markers under the recorded counters.
Stderr is 78 bytes with no structured levels/panic markers. Warnings remain
visible evidence, not a clean-log claim. Raw app logs remain local. `log-summary.json`
records hashes/counts for stdout, stderr, sealed-stage, launch and stop logs.
`credential-scan.json` records read-only finite-pattern scans of those logs and
all 250 saved source artifacts: no configured bearer/JWT/private-key/credential
assignment patterns matched. This is not exhaustive secret detection; token
files were not read and raw matched values were never published.

The saved end receipt verifies all **eight recorded Owner identities absent**;
this packet performs no new process query and asserts no unrecorded-child census.
Recorded UI state still exposes `settled=false`, `ui_known=4` and a stale terrain
diagnostic. No terrain readiness or global-quiescence claim follows from save
notices or screenshots.

No recipient/peer delivery, native replica asset transfer, closed-route source
acceptance, derived/mixed selection, schema rejection/rollback, role/reset matrix,
full catalog/UX, final integrated tests, CI or tagged releases are accepted here.
The bounded successes above and the remaining live layout/restart obligations
are qualified by the full packet's independent-review.md; full campaign acceptance remains pending.
