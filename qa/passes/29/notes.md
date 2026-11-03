# Focused QA29: selection and numeric edits work; two interaction defects remain

The retained Owner store runs on sealed ce31ee6e without reseeding, with the
recipient and fixture helper absent. The combined production GUI/helper build
completes in 392.6 seconds with four compiler warnings. Startup completes in 18.9 seconds. This
is production build and focused live evidence, not the final integrated test
sweep. Exact process and source provenance are in owner-context.json.

Actual plain-click/Ctrl-click selects Cube 1 and Cube 2. Both accessible tree rows
are selected, Cube 2 is primary, and the Inspector reports two objects with mixed
positions and a clearly shared edit scope. The screenshot distinguishes the
additional row from the brighter primary row with compact thin glyphs. A later
reverse Shift range retains the pair and makes clicked Cube 1 primary. The debug
state observes only the primary object; it is not a whole-selection ECS census.

A literal translation [1.25,-2,3.5] updates both stored rows and preserves their
other data. One Undo restores both; Redo reapplies the translation. Selecting
only Cube 1 and then invoking Undo still restores the original pair. Separate
primary observations confirm both rendered transforms at the translated cuts.
The history entries share an explicit editor batch ID; equal timestamps are not
used as membership proof. Cube 2 receives an explicit new baseline before its
transform; its older unavailable intervals are not fabricated or retrofitted.

Rotation by 90 degrees around the independently calculated mean pivot [-6, 0, -6]
matches expected positions with maximum error 9.5367431640625e-7 m. Uniform scale
by 1.5 matches independent expectations within 4.76837158203125e-7 m. Both are below
the 1e-4 m tolerance. Properties, nested JSON nulls and whitespace-bearing data
remain unchanged. The latest scale Undo restores the preceding rotated state.

Two failures block acceptance. QA-0064: committed updates remove/reappend the
existing tree entries, moving selected Cube 1/2 from the top of Petal A to its
bottom and outside the viewport. A locator refuses before input; scrolling
makes them reachable. Same-Petal in-place replacement 21fe2286 is independently
source-approved and is integrated only after this app stops. Fresh-stage runtime
proof is still required.

QA-0065: after rotation, scale and UndoScale, the next UndoRotation refuses with
the complete-Undo-changed message. Both API response files are byte-for-byte
unchanged by the refused attempt. Source review identifies predecessor history
versions that were not advanced by a proven owned inverse. The owned-chain
correction 879286d7 is source-approved in the separate Group worktree; it is not
installed in this stage. Full Group approval still awaits its creation-Redo
proof correction.

Cleanup uses two explicit new confirmed REST mutations to restore the original
cube transforms. Complete rows match the initial rows except monotonic edit
sequences; audit history remains. Cleanup is never counted as successful older
Undo. The final primary ECS observation shows Cube 1 at [-12, 0, 0], unit scale and
identity rotation. All eight recorded owner identities are verified absent.

The verdict inventories 140 input files and 196 parsed JSON records, including six
public PNGs. Final stdout has 25 WARN records, no structured ERROR or Rust panic
markers; raw logs are hashed, scanned and retained locally without copying them
into public evidence. Independent review approves the focused successes and both failures; see independent-review.md. No group creation/deletion,
plural property writes, gimbal drag, derived/closed group, role/reset matrix,
restart persistence, peer delivery, full catalog/UX, final tests, CI or release
acceptance is claimed by this focused pass.
