# QA30 focused history and confirmation layout evidence review

Independent reviewer: `/root/selection_corrections_review`. Review date: 2026-11-03.
Scope: saved artifacts only; no app/API calls, input, process operations, network,
Cargo, tests, or production edits. This is a bounded review of executed evidence,
not a full QA30 verdict. The root-owned app may continue beyond these cuts.

The recorded context identifies Owner PID 95156, start ticks
639292949975462369, stage `481e942b-20261103022608369`, source commit
`481e942bb8150b882a30a0826ad49d89f4a00b64`, and executable SHA256
`59BE87053BE2273615410B9D1971822999E978D304AE2779A96F83F2AD7909E0`.
The manifest records source_dirty=true plus source fingerprint
`A1D02EB5ED94E41C75541BADB9E4EBB0EF8856C4CEB1C498D5D2FF1093C52CFE`.
These are recorded provenance, not a new liveness or executable observation.

## History result: focused positive evidence

The stored complete rows support the requested numeric pair sequence. Cube 1 is
`01M6D0VZSPGMEPRBB47TSJB80V`; Cube 2 is `01M6D0W06VJ2D2XQM0SH0P2VP5`.
Their initial world origins are `[-12,0,0]` and `[0,0,-12]`, giving the independently
calculated mean `[-6,0,-6]`. Positive 90 degree yaw about Y maps relative `(x,z)` to
`(z,-x)`: the expected new origins are `[0,0,0]` and `[-12,0,-12]`.
Scaling both relative displacements by 1.5 about the same mean gives `[3,0,3]`
and `[-15,0,-15]`. This expectation is calculated from the initial rows, not copied
from the application's reported results.

| Cut | Expected Cube 1 origin | Expected Cube 2 origin | Y rotation | Uniform scale | Both edit_seq |
| --- | --- | --- | --- | --- | --- |
| Initial | [-12,0,0] | [0,0,-12] | 0 | 1 | 8 |
| Apply yaw | [0,0,0] | [-12,0,-12] | pi/2 | 1 | 9 |
| Apply scale | [3,0,3] | [-15,0,-15] | pi/2 | 1.5 | 10 |
| Undo scale | [0,0,0] | [-12,0,-12] | pi/2 | 1 | 11 |
| Undo yaw | [-12,0,0] | [0,0,-12] | 0 | 1 | 12 |
| Redo yaw | [0,0,0] | [-12,0,-12] | pi/2 | 1 | 13 |
| Redo scale | [3,0,3] | [-15,0,-15] | pi/2 | 1.5 | 14 |
| Cleanup Undo scale | [0,0,0] | [-12,0,-12] | pi/2 | 1 | 15 |
| Cleanup Undo yaw | [-12,0,0] | [0,0,-12] | 0 | 1 | 16 |

Every corresponding recorded row matches this expectation within
0.000001907349 world units; stored nonzero Y rotation is 1.570796251296997
radians. Both final complete rows exactly restore the initial transform values.
Complete JSON row comparisons at every saved row cut show no differences outside
position, rotation, scale and edit_seq. In particular, Cube 1's nested object,
nested null, measurement array, nested array, boolean, number and literal
whitespace string remain equal; Cube 2's properties also remain equal.

Primary row evidence, all in `qa/passes/30/`, line 1:

- `owner-cube{1,2}-before-row.jsonl`;
- `owner-cube{1,2}-after-{yaw,scale,undo-scale,undo-yaw,redo-yaw,redo-scale}-row.jsonl`;
- `owner-cube{1,2}-history-cycle-final-row.jsonl`.

The final history files independently retain the earlier entries unchanged and
append exactly eight `full_transform` entries each: Cube 1 versions 25 through 32
after baseline 24, and Cube 2 versions 14 through 21 after baseline 13. Both final
history responses report current_state_verified=true. Each corresponding pair
shares the same atomic_save_id, atomic_save_request_id and editor_request_id;
each step has a fresh request ID. The after_state sequence contains the cleanup
Undo scale state at edit_seq 15 as well as the final restored state at 16.
The complete preexisting prefix, including explicitly unavailable historical
intervals, is preserved; this does not establish availability of older intervals.

History evidence: `owner-cube{1,2}-before-history.jsonl` and
`owner-cube{1,2}-history-cycle-final-history.jsonl`, line 1. Successful recorded
inputs: `owner-apply-yaw90.jsonl`, `owner-apply-scale15.jsonl`,
`owner-undo-scale.jsonl`, `owner-undo-older-yaw.jsonl`, `owner-redo-yaw.jsonl`,
`owner-redo-scale.jsonl`, `owner-cleanup-undo-scale.jsonl`, and
`owner-cleanup-undo-yaw.jsonl`. Saved rows/history substantiate the effects;
commands_applied alone would not establish a successful save.

## Selection, tree order and native observation

The started, pair-selected, post-yaw, pair-restored and post-scale UI cuts retain
Cube 1 at `(100,457,64,24)` and Cube 2 at `(100,488,64,24)` physical pixels.
Cube 3 remains at `(100,519,64,24)`. They remain enabled, visible, in that sibling
order. Both cubes are selected through the scale cut, with `2 selected objects`
and `Primary: Cube 2`. After the explicit Cube 1 native observation click, the
Undo/Redo and final UI cuts show Cube 1 selected and Cube 2 unselected, with the
same row bounds/order. Both stored rows and histories nevertheless undergo every
inverse/redo step. This is direct evidence that captured history targets the
original pair independently of the current single selection, and that these
receipts do not reorder these three rows.

UI evidence: `owner-started-ui.jsonl`, `owner-two-selected-ui.jsonl`,
`owner-after-yaw-ui.jsonl`, `owner-yaw-restored-pair-ui.jsonl`,
`owner-after-scale-ui.jsonl`, `owner-after-undo-scale-ui.jsonl`,
`owner-after-undo-yaw-ui.jsonl`, `owner-after-redo-yaw-ui.jsonl`,
`owner-after-redo-scale-ui.jsonl`, and `owner-history-cycle-final-ui.jsonl`.
This is bounded evidence for these rows and this sequence, not a reverse-Shift,
duplicate-alias, all-tree-order or every selection gesture verdict.

Native primary samples agree with the stored transforms: Cube 2 after yaw/scale,
Cube 1 in the separate yaw and scale primary samples, Cube 1 through the four
inverse/redo states and cleanup, and Cube 2 after final cleanup when explicitly
made primary. Nonzero native quaternion `[0,0.7071067690849304,0,0.7071068286895752]`
matches the expected 90 degree yaw to floating point precision. Final native
samples for both cubes restore identity rotation and unit scale.

Native evidence: `owner-after-{yaw,scale,undo-scale,undo-yaw,redo-yaw,redo-scale}-state.jsonl`,
`owner-yaw-cube1-native-state.jsonl`, `owner-scale-cube1-native-state.jsonl`,
`owner-cleanup-after-undo-{scale,yaw}-state.jsonl`, and
`owner-cycle-final-cube2-native-state.jsonl`. `ui-state.selection` is a primary-only
sample: these files do not independently observe Cube 2's native ECS transform at
each intermediate inverse/redo step. Complete pair stored-state evidence should
not be described as a native all-selection census. Exposed state also records
settled=false/ui_known=4 and a stale terrain diagnostic; this review makes no
terrain readiness or global quiescence claim.

## MED: Group delete confirmation layout needs correction

`qa/passes/30/owner-group-delete-fresh-confirm-ui.jsonl:1` records the actual
`Cancel Delete visibility group QA30 selected pair` button at x=1246.8125,
y=341, width=13.46875, height=70, in a 1280x720 window at 1 pixel per point.
The layout hit rectangle has those same dimensions and is allowed/clickable;
it is not absent. The neighboring Confirm control is only 42.0625x30.
The fresh saved screenshot shows Cancel wrapping one letter per line at the
right edge, with a cramped confirmation row. This makes the cancellation action
hard to read and aim at during a destructive group interaction. It falls short
of the user's request for clear, usable right-panel controls at the actual
supported window size. Severity is medium for this demonstrated usability
defect, with no demonstrated data loss and no invented WCAG conformance verdict.

Screenshot observed directly:
`.qa/run/20261101-191920-658/shots/024237-830-qa30-group-delete-fresh-confirm.png`,
referenced by `owner-group-delete-fresh-confirm-shot.jsonl:1`.
Responsive layout correction is assigned separately. It should keep the group
name and both confirmation choices readable/reachable at this panel width;
fresh post-fix screenshot/hit geometry plus an actual Cancel preserving the
group would resolve the usability finding. No proposed or source-only fix is
accepted by this evidence review.

The earlier `owner-confirm-delete-group.jsonl:2` is locator_not_found/refused
with zero new log sequence and no dispatched input. It is an expired confirmation
locator refusal, not an application delete failure. The fresh confirmation input
in `owner-confirm-delete-group-fresh.jsonl:1` dispatches one click successfully;
this review does not evaluate the whole subsequent Group lifecycle.

## Limits

These cuts support successful consecutive pair numeric Undo/Redo and final
cleanup in this stage, addressing the previously reproduced stale-version
sequence for these objects. They do not close external same-value conflict,
gimbal, closed-path source, property mutation/schema rollback, Group lifecycle,
reload, peer/native asset, full catalog/UX, CI, or tagged-release requirements.
No current app state is asserted after the final saved cut.

## Frozen evidence anchors

SHA256 of the bytes read for this review:

| Artifact | SHA256 |
| --- | --- |
| `qa/passes/30/owner-context.json` | `82e4b3e87b02c339c82727ac46f4f3be6afab47528df570967ac354adb65f562` |
| `qa/passes/30/owner-cube1-before-row.jsonl` | `3cff8519f6b32fb4ad31805d2839155abedcdd95cf5e3957b59652d0053a820a` |
| `qa/passes/30/owner-cube2-before-row.jsonl` | `0193af93f17fe1e7b246250c15e622d29c44ce86d4a6da30fd194aefbf0b4923` |
| `qa/passes/30/owner-cube1-history-cycle-final-history.jsonl` | `d184041f10fcb2e2c7704348f0734e5ee9432b601f545e7b1beb64b4b290ff69` |
| `qa/passes/30/owner-cube2-history-cycle-final-history.jsonl` | `5ebac5366c13a233c3bffa5d1908fb7e33933bc45d415eccf6dd4b16b9d42752` |
| `qa/passes/30/owner-history-cycle-final-ui.jsonl` | `37de43618786539231972d7f94027ec99d5b7496379adc65b77401ba6526f233` |
| `qa/passes/30/owner-group-delete-fresh-confirm-ui.jsonl` | `ac0c71f5d33f008657cdc621c0d50b05c656928da803126510bc08b90a7c17a8` |
| `qa/passes/30/owner-confirm-delete-group.jsonl` | `3838c81cd37115891484387bc458123cf0079ebd2fd294a4a302fce4a299f785` |
| `.qa/run/20261101-191920-658/shots/024237-830-qa30-group-delete-fresh-confirm.png` | `ae3d913043bc8b961e8732896a2c8762dd33e1709d651126c11c8aa8de68c29c` |
