---
type: ImplementationReport
title: QA55 responsive grid first source freeze
date: 2026-11-04
---

# QA55 responsive property grid: frozen source handoff

Author `/root/legacy_helper_io_fix`; ready for independent review, not approval or
repaired runtime acceptance. Baseline22039d5add074d4df0e18f5847286606d95407ac.
Read root/panels guides and shared UI guidance. panels/AGENTS.md exists; no parent
guide ownership extension was needed. Native observer source stays untouched.

Actually viewed the1280x720 baseline
`.qa/run/20261104-174918-156/shots/175750-365-qa55-alpha-property-grid.png`:
the narrow Inspector shows Key/Type and clipped Value, with all row actions beyond
the right edge and only a horizontal-scroll hint. Columns::new forced a wide
minimum containing four columns plus the reserved Confirm/Cancel pair.

Exactly three assigned files,168 insertions/91 deletions:

| fe-ui/src/panels path | Frozen bytes | SHA-256 |
| --- | ---: | --- |
| inspector.rs | 282198 | `2e0127e60f3f8b137c0df2c7b9ef3be59dfbe60de4e29d0442df1d85a32728be` |
| property_grid.rs | 6781 | `6bb19366a3cef4cca93ef81280cd1e20ba9db6952873d4d4d52cef8fe8af7907` |
| AGENTS.md | 70958 | `6c2f3514d564b180f0c2f85fec447d1110a0a29517917855cd6d78442cd97594` |

Packet contains complete raw/ and baseline/ snapshots. Baseline snapshots are
exact Git blobs, not former working EOL bytes. source.patch19675 bytes SHA-256
`d2dbc85d2f6273da06e8654a8edabb9a3ddc2ade46f7233e807da6d31cdc0473`;
manifest.json2119 bytes SHA-256
`ce29050a87e7bb93c031e9ba4759c411d90e946bd7872e4c1eac9c23bd9cc866`.

The shared Columns geometry now uses actual available width. Four columns remain
when measured minima fit; otherwise the same table has Property (key plus type)
and Value columns with a full-width wrapped action row below. Compact summary
cells reserve two control heights. Action slots still reserve the full measured
Confirm/Cancel group and the physical target minimum; they are not squeezed into
the value cell. The obsolete horizontal-action hint is removed. The existing
native horizontal container/ID and observations remain, with fitted content
instead of a forced wide floor. The existing Inspector vertical scroll handles
the extra compact-row height.

One local row-action closure serves both layouts, retaining the original Edit,
exact stored-value Copy and two-step Delete bodies and loading/editing/pending
conditions. Wide rows retain their existing four slots. Key hover/contextual
accessible action names keep full keys; labels can truncate without truncating
identity. Preview formatting, compound expand/detail ID, typed draft model and
readback/authority flows are retained. Expanded read-only JSON/editors use the
unchanged visible-width expansion seam. No state manager/action/schema feature
or dependency is introduced. Narrow scalar previews still use the existing
ellipsis/hover behavior; complete stored values remain available through Copy.

Only the existing inspector_transform_and_typed_property_editing_journey changes.
At260/320/640 points it now leaves horizontal offset at zero and requires complete
normal/Confirm/Cancel leaf targets inside actual hit/child/table clips, unchanged
width/offset and stable Edit/Copy positions while arming/canceling. Existing exact
clipboard/deletion/editor callbacks remain exercised. It checks fitted geometry,
wide value minimum and compact/wide branches, opens a dirty draft, crosses the
width breakpoint and returns with the exact buffer retained before its existing
editor-fit/cancel/no-write checks. The old forced-right-edge test setup is removed.
No new test registration or unrelated coverage expansion.

No tests, implementation parsers, formatter, compiler/build or app/native/API/DB/
network actions were run. Read-only Git show/diff/status for these three paths was
explicitly authorized by root and used for the freeze; no Git mutation. Other
source, evidence/Conductor and live apps were not touched. Root must independently
review, build and inspect fresh narrow/wide/scaled images and actual row/draft/
expansion/permission behavior. Authored journey assertions and prior root checks
provide no executed credit for this source. QA-0074 remains pending live acceptance;
no FULL/CI/release result is claimed.
