# Independent QA30 packet verification

Reviewer: `/root/selection_corrections_review`, 2026-11-03.

**Verdict: approve the packet's bounded executed findings, with the recorded
medium confirmation-layout defect still open.** No substantive evidence or scope
correction is required. This is not a complete application QA pass, release
approval, or acceptance of the source-only layout fix or pending restart.

The author and reviewer worked in separate lanes. This review read saved packet
artifacts, the specifically referenced local logs/start receipt/sealed executable,
and seven screenshot sources. It did not call the app/API, send input, inspect
live processes, execute Cargo/tests, access network, or read token files. Only
this review file was authored; original evidence and production code were not
modified. No raw app-log values or credential matches are published here.

## Packet integrity and provenance

- All **263 listed payload files** match their recorded lengths and SHA256
  hashes. Names are unique, with no missing or extra files before this review
  was added. The original packet contains 264 files including its manifest.
- All **250 source evidence files** also match verdict sizes/hashes and parse
  successfully to exactly **333 JSON/JSONL records** under the packet's counting
  convention. Seven screenshot copies are byte-identical to their recorded local
  source images and match both their individual and packet-manifest hashes.
- The unchanged earlier bounded review is retained as
  `independent-history-review.md`; it does not substitute for verification of
  later evidence, which is independently checked below.
- `owner-context.json` and its referenced ready `start.json` agree on exact Owner
  PID 95156, ticks 639292949975462369 and the stage executable path. They record
  retained run root, no reseed, no changed port and no stale-stage override.
- The sealed executable's bytes independently match SHA256
  `59BE87053BE2273615410B9D1971822999E978D304AE2779A96F83F2AD7909E0`.
  Source is the recorded `481e942bb8150b882a30a0826ad49d89f4a00b64` stage,
  with source_dirty=true and fingerprint
  `A1D02EB5ED94E41C75541BADB9E4EBB0EF8856C4CEB1C498D5D2FF1093C52CFE`.
  This is not a claim that the stage equals the later working source or is clean.
- The saved end receipt contains exactly the same eight full identities as the
  context's root plus seven descendants, each captured_identity_alive=false.
  This proves the recorded cleanup observation, not a fresh process query or
  absence of unrecorded children.

Frozen input anchors, before this review file:

| File | SHA256 |
| --- | --- |
| packet-manifest.json | `43491f62b2ec9a737f2ab7b0630f9ad17b5526a4cc2ebed84a6282cbc3cc66c7` |
| verdict.json | `abf8d4a8828dac3b03fe04a7acd11b336a31ff4293a013b004123209343d1ba3` |
| comparison-oracle.json | `44ac5a67411f4d6d344cb67de761aeff2a204fa36c814041c89ffe2a0d690648` |
| notes.md | `a1827a2e3b014f02e9385d091deb583d0c1e4bfc18c009ac07122ad9c200f4a7` |
| log-summary.json | `ef252972db4981e225baaeb8dc9d1580b0d2ad51343c7ecc0d986a9e75405b86` |
| credential-scan.json | `916a19f56fa452ee3d7a82f4dc51fb531f44674172ff1f229d29bf5f25743c2e` |

## Executed effects independently checked

### Numeric history and tree selection

The previously reviewed numeric evidence is retained unchanged. Initial origins
[-12,0,0] and [0,0,-12] independently imply mean [-6,0,-6]. Positive Y yaw 90
maps relative (x,z) to (z,-x), giving [0,0,0] and [-12,0,-12]. Uniform scale 1.5
about that mean gives [3,0,3] and [-15,0,-15]. The saved yaw/scale, Undo twice,
Redo twice and cleanup Undo twice match those expectations; maximum position
error is 0.0000019073486328125 world units. Complete final transforms restore
baseline exactly, with all other fields preserved except advancing edit_seq.

The two numeric history prefixes remain exact; eight new paired full_transform
facts each retain fresh matching pair request IDs. The second cleanup Undo is a
successful owned inverse rather than a corrective API edit. The intermediate
cleanup scale state is supported by its history after_state and native sample,
without inventing a missing standalone complete-row file.

Ten tree cuts preserve Cube 1/2/3 order and their original row rectangles. Pair
selection and Primary: Cube 2 precede the numeric saves; later single Cube 1
selection does not retarget pair history inverses. This is evidence for those
rows/cuts, not every Shift/alias selection convention.

### Group creation, deletion and repeated replay

Independently compared all ten saved registry vectors and all twenty complete
object rows for:

1. Create, Delete, UndoDelete, UndoCreate, RedoCreate, RedoDelete.
2. UndoDelete, UndoCreate, RedoCreate, RedoDelete again.

Every present vector equals the original Group
`01M6GCGBZW14FNX3J1HFA7KCGA`, name `QA30 selected pair`, order 0.0, visible=true.
Every absent vector is empty. Each object remains present; the only complete-row
change is adding the exact flat property `view.groups: [group_id]` when present
or restoring the original absence of that property. No absent/null/empty
representation beyond this fixture's original absence is claimed as exercised.

The first ten later journal facts per object alternate the expected membership
set/deletion and agree with the saved property state. Corresponding facts have
matching fresh pair request IDs. This substantiates the successful lifecycle;
it does not independently inspect immutable internal proof manifests, execute
negative proof cases, or establish hidden/tombstoned/empty-Group coverage.

The expired original confirmation is a pre-input locator refusal. A fresh arm
and current locator issue one click; registry and rows substantiate deletion.
The packet does not conceal the failed locator or recover by replaying an
unknown accepted write.

### Gimbal and property form

The actual saved pointer drag is followed by a common nonzero X translation
`3.0052757263183594` on both complete rows. Other coordinates and every other
field remain unchanged except edit_seq 16 to 17. One recorded Undo restores
both complete baseline rows, with edit_seq 18. Both moved-state native primary
samples and both Undo native primary samples agree with the stored rows.
The packet correctly declines to claim independently calibrated pointer/world
distance or all-axis/tool coverage.

The shared value is independently parsed from the actual saved text-entry
command. It retains nested nulls, measurement data, the nested array and literal
backslash-n/backslash-t sequences with surrounding spaces. All five immediate
pair row cuts match Set, Delete, UndoDelete, UndoSet and RedoSet exactly, changing
only `qa30_shared`. The sixth RedoDelete is substantiated by the subsequent full
pre-external rows and sixth matching property history fact, not an invented
immediate row capture. Other complete-row fields remain exact throughout.

After numeric cleanup, both histories preserve their entire earlier prefix and
contain the expected **18 owned operations**: ten membership facts, two transforms
and six property facts. All 18 pair request IDs correspond and are unique.
Cube 1 reaches history version 50. Cube 2 reaches 39 for the owned sequence and
then 40 for the separate external conflict probe.

### External same-value conflict

The recorded API response explicitly commits Cube 2 history version 40 and
edit_seq 19. Compared with the pre-write row, edit_seq 18 to 19 is its only
complete-row difference; Cube 1 is unchanged. The additional Cube 2 fact is
separate from the 18 owned pair facts and lacks an owned editor_request_id.

The subsequent actual Undo input produces the saved explicit complete-Undo
refusal notice. Both post-refusal rows are byte-identical to their post-external
counterparts; both post-refusal histories are byte-identical to the pre-refusal
histories. This is evidence of whole-action refusal without partial inverse for
this same-value revision. It does not establish every concurrent/external-write
scenario.

## Native observations and open obligations

All 13 native-primary samples enumerated in the oracle independently match their
corresponding saved node ID, position, scale and rotation quaternion, allowing
quaternion sign equivalence. Cube 2 is not independently sampled at every numeric
intermediate Undo/Redo; no simultaneous all-selection ECS census is asserted.
The saved state retains settled=false, ui_known=4 and a stale terrain diagnostic.
Save notices and matching rows do not establish global quiescence or terrain
readiness.

The final restart fixture is independently confirmed: registry Group
`01M6GDVPBSY2W51384QZGGBCAS`, `QA30 restart group`, order 0.0, visible=true,
with exact flat view.groups membership on both saved rows. Compared with the
post-conflict rows, membership is the only change; the intentional external
Cube 2 revision remains intact. The final screenshot visibly agrees with the
registry and two-member selection. This is a saved next-stage baseline;
restart persistence remains unexecuted in this packet.

The medium confirmation-layout defect remains valid: the saved fresh UI and
screenshot expose Cancel at 13.46875x70 physical pixels, wrapping into a vertical
strip. Confirm is cramped at 42.0625x30. Cancel is present/clickable; no data loss
or WCAG conclusion is inferred. Source fix `4336f781` is correctly identified
as unexecuted on the 481e stage. A fresh-stage layout observation and actual
Cancel preserving the Group remain required.

## Logs and finite credential scan

Independently recomputed the five referenced local-log byte lengths and hashes,
line counts, and level counts. Stdout is 280808 bytes, 1852 lines, 1438 INFO,
347 WARN, zero structured ERROR; stderr is 78 bytes, one line, no structured
levels. This does not make the warnings benign or accept a clean-log claim.
The sealed-stage log retains its one compiler warning and successful build
provenance; the packet does not claim the integrated test sweep was executed.

Recomputed all four configured regex counts for all 250 source artifacts and
all five logs. Counts match the summaries at zero. The configured patterns also
recognize their synthetic positive examples. Token files were not read and no
matched/raw credential values were emitted. This is finite-pattern checking,
not proof that every possible secret format is absent. Raw app logs remain local.

## Publication bookkeeping

The frozen originals correctly say full-packet independent review is pending.
This separate signed-off review supplies that verdict. If the packet is repacked,
the author should update the pending review pointers and regenerate its manifest
to include this added file and revised metadata. Until then, the original
263-payload manifest is explicitly the pre-review freeze and does not cover this
new review file. Do not rewrite saved application evidence or import later-stage
execution into these QA30 results.

Full catalog/UX, role/reset/negative schema and group cases, closed-source
acceptance, native peer assets, actual restart, final integrated checks, exact-SHA
CI and downloadable tagged releases remain separate obligations.
